// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Lookahead-consensus tempo tracker. See ADR-0026.
//!
//! An alternative to per-onset PLL feedback (`BeatPll::on_onset`). Buffers
//! detected onsets over a configurable window, computes the median
//! inter-onset interval as the consensus period, and only commits to the
//! PLL once two consecutive consensus computations agree. Trades
//! `lookahead_ms` of latency for a more confident tempo estimate on noisy
//! input. The user compensates for the latency via the existing
//! `latency_offset_ms` parameter.

use std::collections::VecDeque;

/// Tolerance used to declare two consecutive consensus periods "the same"
/// for the stability gate.
const STABILITY_TOL: f64 = 0.005;

pub struct ConsensusTracker {
    onset_buffer: VecDeque<u64>,
    window_samples: u64,
    sample_rate: f64,
    min_period_samples: f64,
    max_period_samples: f64,
    last_consensus: Option<f64>,
    stable_count: u32,
}

impl ConsensusTracker {
    pub fn new(sample_rate: f64, window_ms: f64) -> Self {
        let min_period = sample_rate * 60.0 / 220.0;
        let max_period = sample_rate * 60.0 / 60.0;
        Self {
            onset_buffer: VecDeque::with_capacity(64),
            window_samples: ms_to_samples(window_ms, sample_rate),
            sample_rate,
            min_period_samples: min_period,
            max_period_samples: max_period,
            last_consensus: None,
            stable_count: 0,
        }
    }

    pub fn set_window_ms(&mut self, ms: f64) {
        self.window_samples = ms_to_samples(ms, self.sample_rate);
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.min_period_samples = sample_rate * 60.0 / 220.0;
        self.max_period_samples = sample_rate * 60.0 / 60.0;
    }

    /// Reset the consensus state. Used on silence-end / manual resync.
    pub fn reset(&mut self) {
        self.onset_buffer.clear();
        self.last_consensus = None;
        self.stable_count = 0;
    }

    pub fn on_onset(&mut self, abs_sample: u64) {
        self.onset_buffer.push_back(abs_sample);
    }

    /// Drop onsets older than the window. Call at the end of each
    /// `process` block before requesting `dominant_period()`.
    pub fn prune(&mut self, current_abs_sample: u64) {
        let cutoff = current_abs_sample.saturating_sub(self.window_samples);
        while let Some(&front) = self.onset_buffer.front() {
            if front < cutoff {
                self.onset_buffer.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn most_recent_onset(&self) -> Option<u64> {
        self.onset_buffer.back().copied()
    }

    pub fn buffered_onset_count(&self) -> usize {
        self.onset_buffer.len()
    }

    /// Compute the dominant inter-onset interval via median, with octave
    /// correction and outlier filtering. Returns `Some(period_samples)`
    /// once the consensus has been stable across two consecutive calls.
    /// Returns `None` if there aren't enough onsets in the window or the
    /// stability gate hasn't fired yet.
    pub fn dominant_period(&mut self) -> Option<f64> {
        // Need at least 3 onsets (= 2 IOIs) to take a meaningful median.
        if self.onset_buffer.len() < 3 {
            return None;
        }

        // Collect IOIs, octave-correct, drop residual outliers.
        let mut iois: Vec<f64> = self
            .onset_buffer
            .iter()
            .zip(self.onset_buffer.iter().skip(1))
            .map(|(a, b)| (*b as f64) - (*a as f64))
            .map(|ioi| octave_correct(ioi, self.min_period_samples, self.max_period_samples))
            .filter(|ioi| {
                *ioi >= self.min_period_samples * 0.99 && *ioi <= self.max_period_samples * 1.01
            })
            .collect();

        if iois.len() < 2 {
            return None;
        }

        iois.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = iois[iois.len() / 2];

        // Stability gate.
        match self.last_consensus {
            Some(prev) if (median - prev).abs() / prev < STABILITY_TOL => {
                self.stable_count = self.stable_count.saturating_add(1);
            }
            _ => {
                self.stable_count = 1;
            }
        }
        self.last_consensus = Some(median);

        if self.stable_count >= 2 {
            Some(median)
        } else {
            None
        }
    }
}

fn ms_to_samples(ms: f64, sample_rate: f64) -> u64 {
    (ms * 1e-3 * sample_rate).max(1.0) as u64
}

fn octave_correct(mut ioi: f64, min_period: f64, max_period: f64) -> f64 {
    // Iteratively halve / double until in range (or give up after a few
    // iterations to avoid pathological cases).
    for _ in 0..6 {
        if ioi < min_period * 0.99 {
            ioi *= 2.0;
        } else if ioi > max_period * 1.01 {
            ioi *= 0.5;
        } else {
            break;
        }
    }
    ioi
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 44_100.0;

    fn period_for(bpm: f64) -> f64 {
        SR * 60.0 / bpm
    }

    #[test]
    fn consensus_locks_onto_clean_120bpm() {
        let mut t = ConsensusTracker::new(SR, 4000.0);
        let period = period_for(120.0);
        // Feed 8 onsets at exactly 120 BPM (4 s of audio).
        for i in 0..8 {
            t.on_onset((i as f64 * period) as u64);
        }
        t.prune((8.0 * period) as u64);
        // First call: stable_count = 1 → None.
        assert_eq!(t.dominant_period(), None);
        // Add another onset; second call should commit.
        t.on_onset((8.0 * period) as u64);
        t.prune((9.0 * period) as u64);
        let p = t.dominant_period().expect("expected consensus");
        let bpm = 60.0 * SR / p;
        assert!((bpm - 120.0).abs() < 0.5, "expected ~120 BPM, got {bpm:.2}");
    }

    #[test]
    fn consensus_robust_to_jitter() {
        let mut t = ConsensusTracker::new(SR, 4000.0);
        let period = period_for(120.0);
        let jitter_samples = 0.015 * SR; // ±15 ms

        // Park-Miller LCG for deterministic pseudo-random.
        let mut seed: u32 = 0xCAFEBABE;
        let mut rand = || {
            seed = seed.wrapping_mul(48271).wrapping_rem(2_147_483_647);
            (seed as f64 / 2_147_483_647.0) * 2.0 - 1.0
        };

        let n = 12;
        for i in 0..n {
            let pos = i as f64 * period + rand() * jitter_samples;
            t.on_onset(pos as u64);
        }
        t.prune((n as f64 * period) as u64);
        // Drive to stability.
        let _ = t.dominant_period();
        let p = t.dominant_period().expect("consensus");
        let bpm = 60.0 * SR / p;
        // Adjacent-IOI jitter compounds (±15 ms onset jitter → ±30 ms IOI
        // jitter), so median of ~11 IOIs lands within a handful of BPM
        // of the true value, not razor-tight. Still way better than the
        // raw onset rate would imply.
        assert!(
            (bpm - 120.0).abs() < 5.0,
            "expected ~120 BPM under jitter, got {bpm:.2}"
        );
    }

    #[test]
    fn consensus_resists_outliers() {
        let mut t = ConsensusTracker::new(SR, 4000.0);
        let period = period_for(120.0);
        // 10 true onsets at 120 BPM + 2 spurious onsets between beats.
        for i in 0..10 {
            t.on_onset((i as f64 * period) as u64);
        }
        // Spurious onsets at 1.3 × period and 5.7 × period (offbeat).
        t.on_onset((1.3 * period) as u64);
        t.on_onset((5.7 * period) as u64);
        t.prune((11.0 * period) as u64);
        let _ = t.dominant_period();
        let p = t.dominant_period().expect("consensus");
        let bpm = 60.0 * SR / p;
        // Median should still pick the true period despite outliers
        // skewing some IOIs.
        assert!(
            (bpm - 120.0).abs() < 5.0,
            "median should resist outliers, got {bpm:.2}"
        );
    }

    #[test]
    fn consensus_octave_correction() {
        let mut t = ConsensusTracker::new(SR, 4000.0);
        let period = period_for(240.0); // out of range high
        for i in 0..8 {
            t.on_onset((i as f64 * period) as u64);
        }
        t.prune((8.0 * period) as u64);
        let _ = t.dominant_period();
        let p = t.dominant_period().expect("consensus");
        let bpm = 60.0 * SR / p;
        // 240 BPM is above MAX_BPM (220) → octave-corrected to 120.
        assert!(
            (bpm - 120.0).abs() < 1.0,
            "expected octave-correction to 120, got {bpm:.2}"
        );
    }

    #[test]
    fn consensus_stability_gate_holds_one_frame() {
        let mut t = ConsensusTracker::new(SR, 4000.0);
        let period = period_for(120.0);
        for i in 0..6 {
            t.on_onset((i as f64 * period) as u64);
        }
        t.prune((6.0 * period) as u64);
        // First computation: stable_count becomes 1, returns None.
        assert!(t.dominant_period().is_none());
        // Second computation with the same buffer: stable_count = 2,
        // returns Some.
        assert!(t.dominant_period().is_some());
    }

    #[test]
    fn reset_clears_buffer_and_stability() {
        let mut t = ConsensusTracker::new(SR, 4000.0);
        let period = period_for(120.0);
        for i in 0..6 {
            t.on_onset((i as f64 * period) as u64);
        }
        t.prune((6.0 * period) as u64);
        let _ = t.dominant_period();
        let _ = t.dominant_period();
        assert!(t.last_consensus.is_some());
        t.reset();
        assert_eq!(t.buffered_onset_count(), 0);
        assert!(t.last_consensus.is_none());
        assert_eq!(t.stable_count, 0);
    }
}
