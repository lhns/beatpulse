// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Per-sample phase comparator that fires pulse events at PPQN boundaries.
//! See `BeatPulse-SPEC.md` §5.3.

use crate::dsp::beat_pll::BeatPll;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PulseEvent {
    /// Sample offset within the current host block.
    pub sample_offset: u32,
    /// True when this pulse is the first one of a beat (i.e. the
    /// within-beat index is 0). Drives the UI BEAT LED, which fires
    /// once per detected beat regardless of PPQN.
    pub is_beat_boundary: bool,
}

pub struct PulseGenerator {
    pulse_rate: u32,
    /// Beat counter — increments each time the PLL phase wraps. Combined
    /// with the within-beat pulse index this gives a monotonically-growing
    /// absolute pulse index, so wrap-around at the end of a beat is
    /// detected as a boundary crossing.
    beat_counter: i64,
    /// Last seen PLL phase, in samples. Used to detect wraps.
    last_phase: f64,
    /// Most recent absolute pulse index emitted. `i64::MIN` after reset.
    last_pulse_index: i64,
}

impl PulseGenerator {
    pub fn new(pulse_rate: u32) -> Self {
        Self {
            pulse_rate: pulse_rate.max(1),
            beat_counter: 0,
            last_phase: 0.0,
            last_pulse_index: i64::MIN,
        }
    }

    pub fn set_pulse_rate(&mut self, pulse_rate: u32) {
        self.pulse_rate = pulse_rate.max(1);
    }

    pub fn pulse_rate(&self) -> u32 {
        self.pulse_rate
    }

    /// Reset on PLL re-lock or end of silence.
    pub fn reset(&mut self) {
        self.beat_counter = 0;
        self.last_phase = 0.0;
        self.last_pulse_index = i64::MIN;
    }

    /// Advance the PLL phase by one sample and emit any pulse events that
    /// land on this sample.
    pub fn tick(&mut self, pll: &mut BeatPll, sample_offset: u32) -> Option<PulseEvent> {
        pll.advance_one();
        self.check(pll, sample_offset)
    }

    /// Check whether a pulse event lands on the current sample, given the
    /// already-advanced PLL state. Use this when the caller is driving the
    /// PLL phase manually (e.g. interleaving onset arrivals with phase
    /// advances).
    pub fn observe_advance(&mut self, pll: &BeatPll, sample_offset: u32) -> Option<PulseEvent> {
        self.check(pll, sample_offset)
    }

    /// Process a block of `n_samples`. The callback is invoked for each
    /// pulse event with its sample offset within the block. This is the
    /// hot path used by `Plugin::process`.
    pub fn process_block<F: FnMut(PulseEvent)>(
        &mut self,
        pll: &mut BeatPll,
        n_samples: u32,
        mut on_pulse: F,
    ) {
        for i in 0..n_samples {
            pll.advance_one();
            if let Some(ev) = self.check(pll, i) {
                on_pulse(ev);
            }
        }
    }

    fn check(&mut self, pll: &BeatPll, sample_offset: u32) -> Option<PulseEvent> {
        if pll.period_samples <= 0.0 {
            return None;
        }
        let pulse_interval = pll.period_samples / self.pulse_rate as f64;
        if pulse_interval <= 0.0 {
            return None;
        }

        // Detect a real beat-boundary wrap and ignore PLL phase corrections.
        //
        // `BeatPll::on_onset` legitimately reduces phase to align the next
        // pulse with detected beats — that's not a wrap. With α_phase ≤ 0.25
        // and the per-onset error capped at period/2, the maximum legitimate
        // backward correction is ≈ period/8. Requiring a drop greater than
        // half a period leaves a comfortable margin while still catching
        // every natural wrap (which always drops by ~one full period).
        if pll.phase_samples + pll.period_samples * 0.5 < self.last_phase {
            self.beat_counter = self.beat_counter.saturating_add(1);
        }
        self.last_phase = pll.phase_samples;

        let within_beat_index = (pll.phase_samples / pulse_interval).floor() as i64;
        let absolute_index =
            self.beat_counter.saturating_mul(self.pulse_rate as i64) + within_beat_index;

        // Fire only on monotonic forward progress. Backward jumps in
        // `absolute_index` (caused by `BeatPll::on_onset` pulling phase
        // backward across a within-beat boundary) are silent — the next
        // forward progression past `last_pulse_index` re-fires the next
        // real pulse. See ADR-0025.
        if absolute_index > self.last_pulse_index {
            self.last_pulse_index = absolute_index;
            // Per spec §5.3 / §5.4: after a reset (`last_pulse_index = i64::MIN`),
            // the first pulse fires on the first non-silent sample.
            Some(PulseEvent {
                sample_offset,
                is_beat_boundary: within_beat_index == 0,
            })
        } else {
            None
        }
    }
}
