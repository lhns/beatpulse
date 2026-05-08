// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Silence detection. See `BeatPulse-SPEC.md` §5.4.
//!
//! State machine:
//! - `Active` → `Silent` after `release_ms` of audio below `threshold_db`.
//! - `Silent` → `Active` immediately on first sample above threshold.
//!
//! When the gate transitions back to Active, the caller is expected to
//! reset the PLL and `PulseGenerator::last_pulse_index`. We expose the
//! transition via [`SilenceGate::tick`]'s return value.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    Active,
    Silent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    None,
    /// Audio just returned. Caller must reset downstream state.
    SilentToActive,
    ActiveToSilent,
}

/// Power-of-two-ish single-pole RMS smoother. We track *squared* magnitude in
/// `mean_sq` and convert to dB only at the threshold check.
pub struct SilenceGate {
    state: GateState,
    /// Smoothed mean-square envelope.
    mean_sq: f64,
    /// Smoothing time constant for the RMS detector, in samples.
    rms_tau_samples: f64,
    /// Linear-amplitude threshold (squared), derived from `threshold_db`.
    threshold_sq: f64,
    /// Counter of consecutive below-threshold samples while `Active`.
    below_count: u64,
    /// Number of below-threshold samples that triggers the transition to
    /// `Silent`.
    release_samples: u64,
    sample_rate: f64,
}

impl SilenceGate {
    /// Construct a gate. Defaults match spec §5.4: -50 dB threshold,
    /// 200 ms release.
    pub fn new(sample_rate: f64, threshold_db: f64, release_ms: f64) -> Self {
        let mut g = Self {
            state: GateState::Active,
            mean_sq: 0.0,
            // ~10 ms RMS window — fast enough that a single loud kick
            // immediately re-arms us.
            rms_tau_samples: sample_rate * 0.010,
            threshold_sq: 0.0,
            below_count: 0,
            release_samples: 0,
            sample_rate,
        };
        g.set_threshold_db(threshold_db);
        g.set_release_ms(release_ms);
        g
    }

    pub fn set_threshold_db(&mut self, threshold_db: f64) {
        let amp = 10f64.powf(threshold_db / 20.0);
        self.threshold_sq = amp * amp;
    }

    pub fn set_release_ms(&mut self, release_ms: f64) {
        self.release_samples = (release_ms * 1e-3 * self.sample_rate).max(1.0) as u64;
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.rms_tau_samples = sample_rate * 0.010;
    }

    pub fn state(&self) -> GateState {
        self.state
    }

    pub fn is_active(&self) -> bool {
        self.state == GateState::Active
    }

    /// Current envelope in dB. -inf clamped to -120.
    pub fn current_db(&self) -> f64 {
        if self.mean_sq <= 1e-24 {
            -120.0
        } else {
            10.0 * self.mean_sq.log10()
        }
    }

    /// Process one sample. Returns the state transition, if any.
    pub fn tick(&mut self, sample: f32) -> Transition {
        // Single-pole IIR on squared magnitude:
        // mean_sq += (x^2 - mean_sq) / tau
        let x = sample as f64;
        self.mean_sq += (x * x - self.mean_sq) / self.rms_tau_samples;

        match self.state {
            GateState::Active => {
                if self.mean_sq < self.threshold_sq {
                    self.below_count = self.below_count.saturating_add(1);
                    if self.below_count >= self.release_samples {
                        self.state = GateState::Silent;
                        self.below_count = 0;
                        return Transition::ActiveToSilent;
                    }
                } else {
                    self.below_count = 0;
                }
                Transition::None
            }
            GateState::Silent => {
                if self.mean_sq >= self.threshold_sq {
                    self.state = GateState::Active;
                    self.below_count = 0;
                    Transition::SilentToActive
                } else {
                    Transition::None
                }
            }
        }
    }

    /// Process a buffer; returns the *last* non-`None` transition observed
    /// during the block, plus its sample offset within the block.
    /// (The audio thread uses this to schedule a reset hook precisely.)
    pub fn process_block(&mut self, block: &[f32]) -> Option<(Transition, usize)> {
        let mut last: Option<(Transition, usize)> = None;
        for (i, &s) in block.iter().enumerate() {
            match self.tick(s) {
                Transition::None => {}
                t => last = Some((t, i)),
            }
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 44_100.0;

    fn db_to_amp(db: f64) -> f32 {
        10f32.powf(db as f32 / 20.0)
    }

    /// S1: audio above threshold → Active.
    #[test]
    fn s1_audio_above_threshold_stays_active() {
        let mut g = SilenceGate::new(SR, -50.0, 200.0);
        let amp = db_to_amp(-30.0);
        for _ in 0..10_000 {
            g.tick(amp);
        }
        assert_eq!(g.state(), GateState::Active);
    }

    /// S2: drop to silence stays Active for release_ms then transitions.
    #[test]
    fn s2_release_timing() {
        let mut g = SilenceGate::new(SR, -50.0, 200.0);
        // Saturate envelope first.
        for _ in 0..2000 {
            g.tick(db_to_amp(-20.0));
        }
        // Now feed silence and count samples until transition.
        let mut transition_at: Option<usize> = None;
        for i in 0..(SR as usize) {
            if let Transition::ActiveToSilent = g.tick(0.0) {
                transition_at = Some(i);
                break;
            }
        }
        let n = transition_at.expect("must transition within 1 s");
        let expected = (0.200 * SR) as usize;
        let tol = (0.005 * SR) as usize; // ±5 ms
        assert!(
            n.abs_diff(expected) < tol,
            "transitioned at {n}, expected ~{expected}"
        );
    }

    /// S3: audio returns from Silent → immediate Active + transition.
    #[test]
    fn s3_silent_to_active_immediate() {
        let mut g = SilenceGate::new(SR, -50.0, 50.0);
        for _ in 0..10_000 {
            g.tick(0.0);
        }
        assert_eq!(g.state(), GateState::Silent);
        let t = g.tick(db_to_amp(-10.0));
        assert_eq!(t, Transition::SilentToActive);
        assert_eq!(g.state(), GateState::Active);
    }

    /// S4–S6: threshold transitions at multiple dB levels.
    #[test]
    fn s4_s6_threshold_levels() {
        for &thr in &[-60.0, -50.0, -40.0] {
            let mut g = SilenceGate::new(SR, thr, 100.0);
            // Saturate
            for _ in 0..2000 {
                g.tick(db_to_amp(thr + 20.0));
            }
            // Silence — must transition
            let mut went_silent = false;
            for _ in 0..(SR as usize) {
                if let Transition::ActiveToSilent = g.tick(0.0) {
                    went_silent = true;
                    break;
                }
            }
            assert!(went_silent, "threshold {thr} dB never went silent");
        }
    }

    /// S7: release timing accurate over 50–2000 ms.
    #[test]
    fn s7_release_timing_range() {
        for &release_ms in &[50.0_f64, 200.0, 1000.0, 2000.0] {
            let mut g = SilenceGate::new(SR, -50.0, release_ms);
            for _ in 0..2000 {
                g.tick(db_to_amp(-20.0));
            }
            let mut at: Option<usize> = None;
            for i in 0..(3.0 * SR) as usize {
                if let Transition::ActiveToSilent = g.tick(0.0) {
                    at = Some(i);
                    break;
                }
            }
            let n = at.expect("must transition");
            let expected = (release_ms * 1e-3 * SR) as usize;
            // Allow ±2 % or 1 ms, whichever is larger.
            let tol = ((expected as f64 * 0.02).max(1e-3 * SR)) as usize;
            assert!(
                n.abs_diff(expected) < tol,
                "release_ms={release_ms}: transitioned at {n}, expected {expected}, tol {tol}"
            );
        }
    }

    /// S9: reset hook fires exactly once per silent→active edge.
    #[test]
    fn s9_one_transition_per_edge() {
        let mut g = SilenceGate::new(SR, -50.0, 100.0);
        for _ in 0..10_000 {
            g.tick(0.0);
        }
        assert_eq!(g.state(), GateState::Silent);

        let mut count = 0;
        for _ in 0..1_000 {
            if let Transition::SilentToActive = g.tick(db_to_amp(-10.0)) {
                count += 1;
            }
        }
        assert_eq!(count, 1, "should see exactly one SilentToActive edge");
    }

    /// S8: process_block returns the last transition with correct offset.
    #[test]
    fn s8_block_tracks_transition_offset() {
        let mut g = SilenceGate::new(SR, -50.0, 50.0);
        // Build a silent state first.
        let zeros = vec![0.0f32; 10_000];
        g.process_block(&zeros);
        assert_eq!(g.state(), GateState::Silent);

        // Block that's silent for first 100 samples then audio.
        let mut block = vec![0.0f32; 100];
        block.extend(std::iter::repeat(db_to_amp(-10.0)).take(50));
        let result = g.process_block(&block);
        let (t, offset) = result.expect("must observe transition");
        assert_eq!(t, Transition::SilentToActive);
        assert_eq!(offset, 100);
    }
}
