// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Phase-locked loop. See `BeatPulse-SPEC.md` §5.2 and ADR-0004.

const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 220.0;
const LOCK_THRESHOLD: u32 = 8;

#[derive(Debug, Clone)]
pub struct BeatPll {
    pub period_samples: f64,
    pub phase_samples: f64,
    pub sample_rate: f64,
    pub locked: bool,
    pub onsets_since_reset: u32,
    pub last_onset_sample: Option<f64>,

    pub alpha_period: f64,
    pub alpha_phase: f64,

    pub min_period: f64,
    pub max_period: f64,
}

impl BeatPll {
    pub fn new(sample_rate: f64) -> Self {
        let mut pll = Self {
            period_samples: 0.0,
            phase_samples: 0.0,
            sample_rate,
            locked: false,
            onsets_since_reset: 0,
            last_onset_sample: None,
            alpha_period: 0.09,
            alpha_phase: 0.165,
            min_period: 0.0,
            max_period: 0.0,
        };
        pll.set_sample_rate(sample_rate);
        pll
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.min_period = sample_rate * 60.0 / MAX_BPM;
        self.max_period = sample_rate * 60.0 / MIN_BPM;
        // Initialise to a sane mid-range tempo (120 BPM).
        if self.period_samples == 0.0 {
            self.period_samples = sample_rate * 60.0 / 120.0;
        }
    }

    pub fn set_alphas(&mut self, alpha_period: f64, alpha_phase: f64) {
        self.alpha_period = alpha_period;
        self.alpha_phase = alpha_phase;
    }

    /// Reset lock state. Used when audio returns from silence or the user
    /// triggers a manual resync.
    pub fn reset(&mut self) {
        self.locked = false;
        self.onsets_since_reset = 0;
        self.last_onset_sample = None;
    }

    /// Advance phase by one sample.
    pub fn advance_one(&mut self) {
        self.phase_samples += 1.0;
        if self.phase_samples >= self.period_samples {
            self.phase_samples -= self.period_samples;
        }
    }

    /// Advance phase by `n` samples (faster than calling `advance_one` in a
    /// hot loop when no onsets land in between).
    pub fn advance(&mut self, n: u64) {
        if self.period_samples > 0.0 {
            self.phase_samples =
                (self.phase_samples + n as f64).rem_euclid(self.period_samples);
        }
    }

    /// Observe an onset at absolute sample position `obs_sample`.
    pub fn on_onset(&mut self, obs_sample: f64) {
        if let Some(last) = self.last_onset_sample {
            let mut observed_period = obs_sample - last;

            // Octave correction
            if observed_period > 0.0 && observed_period < self.min_period {
                observed_period *= 2.0;
            } else if observed_period > self.max_period {
                observed_period *= 0.5;
            }

            if observed_period >= self.min_period && observed_period <= self.max_period {
                // Period smoothing
                self.period_samples = (1.0 - self.alpha_period) * self.period_samples
                    + self.alpha_period * observed_period;

                // Clamp to bounds — the smoother could otherwise drift slightly
                // out of range from accumulated rounding.
                self.period_samples =
                    self.period_samples.clamp(self.min_period, self.max_period);

                // Phase lock: an onset should land at phase = 0.
                let mut err = self.phase_samples;
                if err > self.period_samples * 0.5 {
                    err -= self.period_samples;
                }
                self.phase_samples -= self.alpha_phase * err;
                if self.phase_samples < 0.0 {
                    self.phase_samples += self.period_samples;
                } else if self.phase_samples >= self.period_samples {
                    self.phase_samples -= self.period_samples;
                }
            }
        }

        self.last_onset_sample = Some(obs_sample);
        self.onsets_since_reset = self.onsets_since_reset.saturating_add(1);
        self.locked = self.onsets_since_reset >= LOCK_THRESHOLD;
    }

    pub fn current_bpm(&self) -> f64 {
        if self.period_samples > 0.0 {
            60.0 * self.sample_rate / self.period_samples
        } else {
            0.0
        }
    }
}
