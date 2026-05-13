// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Downbeat (beat-1-of-measure) tracking — Klapuri 2006 §V.
//!
//! Independent of the joint (tatum, tactus, measure) posterior in
//! [`super::joint_posterior`]. Once the tactus is locked and beats
//! are firing, this tracker observes the **low-band accent** at each
//! beat moment (kicks dominate the low band; downbeats are usually
//! the loudest kicks). For each candidate `k_measure ∈ {2, 3, 4}`
//! it maintains a leaky-integrated per-measure-position energy
//! accumulator and reports the position of maximum accumulated
//! energy as the downbeat phase.
//!
//! Why a separate tracker rather than a fourth dimension on the
//! joint state space: the joint posterior runs every 64 OSS frames
//! (~370 ms, ≈ 0.7 beats at 120 BPM) which doesn't align cleanly
//! with the per-beat phase dimension. Tracking downbeat at beat
//! granularity (driven by `KlapuriTracker::accent_step`'s on_beat
//! callbacks) is structurally simpler and gives sub-beat phase
//! independent of the inference rate.
//!
//! Output: `downbeat_position(k_measure) -> Option<usize>` — the
//! 0-based offset within the measure where the downbeat lands. None
//! until enough beats have been observed (warmup). Intended for
//! downstream consumers (DMX cueing differential by downbeat vs
//! other beats); not currently fed back into the joint posterior.

/// Time constant for the per-position energy accumulator. With γ=0.95
/// the effective window is ~20 beats — enough to overcome short-term
/// dynamics (drum fills, accent variations) but adapt within ~10 s
/// of tempo lock.
const DECAY: f32 = 0.95;

/// Beat count before `downbeat_position` returns `Some` rather than
/// `None`. Below this count, all positions have similar accumulators
/// and the answer is essentially random.
const WARMUP_BEATS: u32 = 8;

pub struct DownbeatTracker {
    /// Per-measure-position accumulators, for `k_measure ∈ {2, 3, 4}`.
    /// `position_energy[k_m - 2][pos]` for `pos ∈ 0..k_m`.
    position_energy_2: [f32; 2],
    position_energy_3: [f32; 3],
    position_energy_4: [f32; 4],
    /// Total beats observed since reset. Used for the warmup gate
    /// and (mod k_measure) to know which position the next beat is.
    beat_count: u32,
}

impl Default for DownbeatTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl DownbeatTracker {
    pub fn new() -> Self {
        Self {
            position_energy_2: [0.0; 2],
            position_energy_3: [0.0; 3],
            position_energy_4: [0.0; 4],
            beat_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.position_energy_2 = [0.0; 2];
        self.position_energy_3 = [0.0; 3];
        self.position_energy_4 = [0.0; 4];
        self.beat_count = 0;
    }

    pub fn beat_count(&self) -> u32 {
        self.beat_count
    }

    /// Observe a beat with its accompanying low-band accent reading.
    /// Updates the per-position accumulators for all three candidate
    /// `k_measure` values simultaneously — the `KlapuriTracker` can
    /// query whichever k_measure the joint posterior settles on
    /// without us needing to know it in advance.
    pub fn observe_beat(&mut self, low_band_accent: f32) {
        let v = low_band_accent.max(0.0);
        let pos2 = (self.beat_count % 2) as usize;
        let pos3 = (self.beat_count % 3) as usize;
        let pos4 = (self.beat_count % 4) as usize;
        self.position_energy_2[pos2] = DECAY * self.position_energy_2[pos2] + (1.0 - DECAY) * v;
        self.position_energy_3[pos3] = DECAY * self.position_energy_3[pos3] + (1.0 - DECAY) * v;
        self.position_energy_4[pos4] = DECAY * self.position_energy_4[pos4] + (1.0 - DECAY) * v;
        self.beat_count = self.beat_count.saturating_add(1);
    }

    /// Most-likely downbeat position within a `k_measure`-beat measure.
    /// `None` until `WARMUP_BEATS` observations have accumulated.
    pub fn downbeat_position(&self, k_measure: u8) -> Option<usize> {
        if self.beat_count < WARMUP_BEATS {
            return None;
        }
        let positions: &[f32] = match k_measure {
            2 => &self.position_energy_2,
            3 => &self.position_energy_3,
            4 => &self.position_energy_4,
            _ => return None,
        };
        positions
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
    }

    /// "Confidence" — how peaked is the position-energy distribution.
    /// `(max - mean) / (max + ε)`, in [0, 1]. Near 0 = flat, near 1 =
    /// strongly peaked. Useful for downstream consumers to decide
    /// whether to trust the downbeat output.
    pub fn confidence(&self, k_measure: u8) -> f32 {
        let positions: &[f32] = match k_measure {
            2 => &self.position_energy_2,
            3 => &self.position_energy_3,
            4 => &self.position_energy_4,
            _ => return 0.0,
        };
        let max = positions.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mean = positions.iter().sum::<f32>() / positions.len() as f32;
        if max <= 0.0 {
            0.0
        } else {
            ((max - mean) / (max + 1e-9)).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downbeat_position_locks_on_loud_position() {
        // Simulate 4/4: loud kick on beat 1, soft on beats 2/3/4.
        let mut t = DownbeatTracker::new();
        for cycle in 0..30 {
            // Position 0 (downbeat) — loud kick.
            t.observe_beat(1.0);
            // Positions 1, 2, 3 — softer.
            for _ in 0..3 {
                t.observe_beat(0.2);
            }
            let _ = cycle;
        }
        assert_eq!(t.downbeat_position(4), Some(0));
        // For k=2 (half-time interpretation), beats 0 and 2 are loud
        // (positions 0/0), beats 1 and 3 are soft (positions 1/1).
        assert_eq!(t.downbeat_position(2), Some(0));
        // k=3 mismatches the 4-beat pattern; wraps unevenly. Just
        // assert it returns Some.
        assert!(t.downbeat_position(3).is_some());
    }

    #[test]
    fn downbeat_position_none_during_warmup() {
        let mut t = DownbeatTracker::new();
        for _ in 0..(WARMUP_BEATS - 1) {
            t.observe_beat(1.0);
        }
        assert_eq!(t.downbeat_position(4), None);
        t.observe_beat(1.0);
        // Warmup met, should now return Some (even on flat input).
        assert!(t.downbeat_position(4).is_some());
    }

    #[test]
    fn confidence_high_when_one_position_dominates() {
        let mut t = DownbeatTracker::new();
        for _ in 0..30 {
            t.observe_beat(1.0);
            t.observe_beat(0.0);
            t.observe_beat(0.0);
            t.observe_beat(0.0);
        }
        let c4 = t.confidence(4);
        assert!(c4 > 0.5, "confidence at k=4 should be > 0.5, got {c4}");
    }

    #[test]
    fn confidence_low_when_uniform() {
        let mut t = DownbeatTracker::new();
        for _ in 0..50 {
            t.observe_beat(0.5);
        }
        let c4 = t.confidence(4);
        assert!(
            c4 < 0.1,
            "confidence on uniform input should be ~0, got {c4}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut t = DownbeatTracker::new();
        for _ in 0..20 {
            t.observe_beat(1.0);
        }
        t.reset();
        assert_eq!(t.beat_count(), 0);
        assert_eq!(t.downbeat_position(4), None);
    }
}
