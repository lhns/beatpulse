// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Pulse emitter for `TrackingMode::AubioTempo`.
//!
//! `PulseGenerator` (`src/dsp/pulse_generator.rs`) drives pulses from
//! `BeatPll`'s phase, which works fine for Reactive / Consensus where
//! the PLL state is the canonical timing. For AubioTempo the
//! authoritative beat times come *directly from aubio* — running them
//! through the PLL → PulseGenerator path adds a small (~0.03 F-measure)
//! wobble because `PulseGenerator`'s wrap detector fires near (but not
//! exactly at) each aubio beat sample.
//!
//! `AubioPulseEmitter` instead emits beat 0 at the exact aubio beat
//! sample and linearly interpolates the PPQN sub-beats between
//! consecutive aubio beats using the most recent inter-beat interval
//! as the period.

use crate::dsp::pulse_generator::PulseEvent;

pub struct AubioPulseEmitter {
    pulse_rate: u32,
    /// Absolute sample of the most recent aubio beat (= within-beat
    /// pulse 0). `None` until aubio first locks.
    last_beat_sample: Option<u64>,
    /// Period (samples) used to space sub-beats. Updated each time a
    /// new beat arrives (from the inter-beat interval).
    period_samples: f64,
    /// Index of the next sub-beat to emit (0..pulse_rate-1, where 0 is
    /// the on-beat pulse). Reset to 1 when a new beat arrives (since
    /// pulse 0 already fired at the beat moment).
    next_sub_index: u32,
    /// Monotonic guard — absolute sample of the most recent emitted
    /// pulse. Pulses with `target_sample <= last_emitted_sample` are
    /// suppressed (matches `PulseGenerator`'s monotonic-progress
    /// behaviour, see ADR-0025).
    last_emitted_sample: Option<u64>,
}

impl AubioPulseEmitter {
    pub fn new(pulse_rate: u32) -> Self {
        Self {
            pulse_rate: pulse_rate.max(1),
            last_beat_sample: None,
            period_samples: 0.0,
            next_sub_index: 0,
            last_emitted_sample: None,
        }
    }

    pub fn set_pulse_rate(&mut self, rate: u32) {
        self.pulse_rate = rate.max(1);
        // Re-anchor sub-index to whatever's next after the most recent
        // emission. Easiest: clamp.
        self.next_sub_index = self.next_sub_index.min(self.pulse_rate.saturating_sub(1));
    }

    pub fn reset(&mut self) {
        self.last_beat_sample = None;
        self.period_samples = 0.0;
        self.next_sub_index = 0;
        self.last_emitted_sample = None;
    }

    /// Called by `lib.rs::process` whenever `AubioTempoTracker` reports
    /// a beat. Emits an on-beat pulse (`is_beat_boundary = true`) and
    /// resets the sub-beat counter.
    pub fn on_beat(&mut self, beat_abs: u64) -> Option<PulseEvent> {
        // Update period from the IBI.
        if let Some(prev) = self.last_beat_sample {
            let ibi = (beat_abs as f64) - (prev as f64);
            if ibi > 0.0 {
                self.period_samples = ibi;
            }
        }
        self.last_beat_sample = Some(beat_abs);
        self.next_sub_index = 1;

        // Monotonic guard: don't re-emit at the same sample.
        if self.last_emitted_sample == Some(beat_abs) {
            return None;
        }
        self.last_emitted_sample = Some(beat_abs);
        Some(PulseEvent {
            sample_offset: 0, // overwritten by caller with within-block offset
            is_beat_boundary: true,
        })
    }

    /// Per-sample tick. Returns a sub-beat pulse if one lands at
    /// `abs_sample`. Caller fills in the within-block `sample_offset`.
    pub fn tick(&mut self, abs_sample: u64) -> Option<PulseEvent> {
        let last_beat = self.last_beat_sample?;
        if self.period_samples <= 0.0 || self.pulse_rate <= 1 {
            return None;
        }
        let pulse_interval = self.period_samples / self.pulse_rate as f64;
        if pulse_interval <= 0.0 {
            return None;
        }
        // Target sample of the next sub-beat we would emit.
        let target = last_beat as f64 + (self.next_sub_index as f64) * pulse_interval;
        if (abs_sample as f64) >= target {
            // Don't emit beyond the next aubio beat — that's `on_beat`'s job.
            if self.next_sub_index >= self.pulse_rate {
                return None;
            }
            // Monotonic guard.
            if let Some(prev) = self.last_emitted_sample {
                if prev >= abs_sample {
                    self.next_sub_index += 1;
                    return None;
                }
            }
            self.next_sub_index += 1;
            self.last_emitted_sample = Some(abs_sample);
            return Some(PulseEvent {
                sample_offset: 0,
                is_beat_boundary: false,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At PPQN=1, on_beat fires once; tick never adds anything.
    #[test]
    fn ppqn_1_emits_one_per_beat() {
        let mut e = AubioPulseEmitter::new(1);
        assert!(e.on_beat(0).is_some());
        for i in 1..1000 {
            assert!(e.tick(i).is_none(), "PPQN=1 should not emit sub-beats");
        }
        assert!(e.on_beat(22050).is_some());
    }

    /// PPQN=4 with 1.0 s period (44100 sa) emits 4 pulses per beat at
    /// indices 0, 11025, 22050, 33075 from each beat sample.
    #[test]
    fn ppqn_4_interpolates_evenly() {
        let mut e = AubioPulseEmitter::new(4);
        // Seed with two beats so `period_samples` is set.
        assert!(e.on_beat(0).is_some());
        assert!(e.on_beat(44100).is_some());
        // Tick samples between beat 1 (44100) and beat 2 (88200).
        let mut sub_emits = 0;
        for i in 44101..=88199 {
            if e.tick(i).is_some() {
                sub_emits += 1;
            }
        }
        // 3 sub-beats expected between consecutive on-beat pulses
        // (indices 1, 2, 3 of pulse_rate=4).
        assert_eq!(sub_emits, 3, "expected 3 sub-beats between beats");
    }

    /// Reset clears state.
    #[test]
    fn reset_clears() {
        let mut e = AubioPulseEmitter::new(4);
        e.on_beat(0);
        e.on_beat(22050);
        e.tick(11000);
        e.reset();
        assert!(e.last_beat_sample.is_none());
        assert!(e.tick(33000).is_none());
    }
}
