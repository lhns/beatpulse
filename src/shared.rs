// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Lock-free atomics shared between the audio thread and the UI thread.
//! See `BeatPulse-SPEC.md` §9.

use atomic_float::AtomicF64;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

pub struct SharedState {
    pub current_bpm: AtomicF64,
    pub locked: AtomicBool,
    pub input_peak_db: AtomicF64,
    pub link_peers: AtomicUsize,
    pub silence_active: AtomicBool,
    /// Monotonic count of pulses emitted by `PulseGenerator`. The UI
    /// detects changes against its previously-seen value and renders a
    /// brief flash on each delta.
    pub pulse_count: AtomicU64,
    /// Monotonic count of detected beats (= pulses with
    /// `is_beat_boundary == true`). Drives the BEAT LED, which flashes
    /// at the beat rate regardless of PPQN.
    pub beat_count: AtomicU64,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            current_bpm: AtomicF64::new(120.0),
            locked: AtomicBool::new(false),
            input_peak_db: AtomicF64::new(-90.0),
            link_peers: AtomicUsize::new(0),
            silence_active: AtomicBool::new(false),
            pulse_count: AtomicU64::new(0),
            beat_count: AtomicU64::new(0),
        }
    }
}

impl SharedState {
    /// All audio-thread writes use Relaxed: the UI is fine with a slightly
    /// stale read, and we don't synchronise other memory through these.
    pub fn store_bpm(&self, bpm: f64) {
        self.current_bpm.store(bpm, Ordering::Relaxed);
    }

    pub fn load_bpm(&self) -> f64 {
        self.current_bpm.load(Ordering::Relaxed)
    }

    pub fn store_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::Relaxed);
    }

    pub fn load_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    pub fn store_peak_db(&self, db: f64) {
        self.input_peak_db.store(db, Ordering::Relaxed);
    }

    pub fn load_peak_db(&self) -> f64 {
        self.input_peak_db.load(Ordering::Relaxed)
    }

    pub fn store_link_peers(&self, n: usize) {
        self.link_peers.store(n, Ordering::Relaxed);
    }

    pub fn load_link_peers(&self) -> usize {
        self.link_peers.load(Ordering::Relaxed)
    }

    pub fn store_silence_active(&self, active: bool) {
        self.silence_active.store(active, Ordering::Relaxed);
    }

    pub fn load_silence_active(&self) -> bool {
        self.silence_active.load(Ordering::Relaxed)
    }

    /// Bumped on every pulse emitted by `PulseGenerator`. Audio thread only.
    pub fn bump_pulse(&self) {
        self.pulse_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn load_pulse_count(&self) -> u64 {
        self.pulse_count.load(Ordering::Relaxed)
    }

    /// Bumped only on pulses where `PulseEvent::is_beat_boundary` is true,
    /// i.e. once per detected beat. Audio thread only.
    pub fn bump_beat(&self) {
        self.beat_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn load_beat_count(&self) -> u64 {
        self.beat_count.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = SharedState::default();
        s.store_bpm(124.7);
        s.store_locked(true);
        s.store_peak_db(-12.5);
        s.store_link_peers(3);
        s.store_silence_active(false);

        assert_eq!(s.load_bpm(), 124.7);
        assert!(s.load_locked());
        assert_eq!(s.load_peak_db(), -12.5);
        assert_eq!(s.load_link_peers(), 3);
        assert!(!s.load_silence_active());
    }

    #[test]
    fn pulse_count_concurrent_bumps_are_coherent() {
        use std::sync::Arc;
        use std::thread;

        let s = Arc::new(SharedState::default());
        let n_threads = 8;
        let bumps_per_thread = 1_000;

        let mut handles = Vec::new();
        for _ in 0..n_threads {
            let s = s.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..bumps_per_thread {
                    s.bump_pulse();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            s.load_pulse_count(),
            (n_threads * bumps_per_thread) as u64
        );
    }
}
