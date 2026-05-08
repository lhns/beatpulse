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
}

pub struct PulseGenerator {
    pulse_rate: u32,
    last_pulse_index: i64,
}

impl PulseGenerator {
    pub fn new(pulse_rate: u32) -> Self {
        Self {
            pulse_rate: pulse_rate.max(1),
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
        self.last_pulse_index = i64::MIN;
    }

    /// Advance the PLL phase by one sample and emit any pulse events that
    /// land on this sample. The caller drives this in their per-sample loop;
    /// alternatively use [`Self::process_block`] for a faster bulk path.
    pub fn tick(&mut self, pll: &mut BeatPll, sample_offset: u32) -> Option<PulseEvent> {
        pll.advance_one();
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
        let pulse_phase = pll.phase_samples / pulse_interval;
        let current_pulse_index = pulse_phase.floor() as i64;

        if current_pulse_index != self.last_pulse_index {
            self.last_pulse_index = current_pulse_index;
            // Per spec §5.3 / §5.4: after a reset (`last_pulse_index = i64::MIN`),
            // the first pulse fires on the first non-silent sample.
            Some(PulseEvent { sample_offset })
        } else {
            None
        }
    }
}
