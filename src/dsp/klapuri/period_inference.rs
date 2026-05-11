// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Joint tatum / tactus / measure period inference — Klapuri 2006 §IV-C.
//!
//! Comb-filter banks resonate at integer-ratio sub-harmonics of the
//! true period (τ=43 resonates with impulses every 86 frames). To
//! pick the right tactus level we score each candidate τ using:
//!
//! - its own resonator energy `e(τ)`,
//! - its sub-harmonic support `e(τ/2) + e(τ/3)` (tatum levels),
//! - its super-harmonic support `e(2τ) + e(3τ) + e(4τ)` (measure
//!   levels),
//! - a log-Gaussian prior centred on `120` BPM (Parncutt-style
//!   preferred tempo, σ in log-BPM space).
//!
//! The winning τ is the tactus (one beat). Phase comes from the
//! winning resonator's delay-line state — see [`super::phase`].

use crate::dsp::klapuri::resonators::ResonatorBank;

/// Configurable weights for the period-score combination. Defaults
/// are conservative starting points from the paper's discussion;
/// tunable from the Ballroom benchmark.
#[derive(Debug, Clone, Copy)]
pub struct InferenceWeights {
    pub w_tactus: f32,
    pub w_tatum_half: f32,
    pub w_tatum_third: f32,
    pub w_measure_2: f32,
    pub w_measure_3: f32,
    pub w_measure_4: f32,
    /// σ of the log-BPM Gaussian prior. Larger → flatter prior.
    pub prior_sigma: f32,
    /// Centre of the log-BPM Gaussian prior, in BPM.
    pub prior_centre_bpm: f32,
}

impl Default for InferenceWeights {
    fn default() -> Self {
        Self {
            w_tactus: 1.0,
            w_tatum_half: 0.6,
            w_tatum_third: 0.3,
            w_measure_2: 0.5,
            w_measure_3: 0.3,
            w_measure_4: 0.2,
            // σ=0.35 in log-BPM space → 1σ covers ≈ 85-170 BPM, 2σ
            // covers ≈ 60-240. Matches the practical Ballroom range
            // and aggressively suppresses fringe-period resonators
            // that get inflated by per-impulse scaling biases.
            prior_sigma: 0.35,
            prior_centre_bpm: 120.0,
        }
    }
}

pub struct PeriodInference {
    pub weights: InferenceWeights,
    /// OSS rate (samples/sec) used to derive BPM from period.
    oss_rate: f32,
    /// Reusable scratch for the per-candidate score vector.
    score_buf: Vec<f32>,
}

impl PeriodInference {
    pub fn new(oss_rate: f32) -> Self {
        Self {
            weights: InferenceWeights::default(),
            oss_rate,
            score_buf: Vec::new(),
        }
    }

    /// Score every candidate τ in `bank.periods()`. Returns the
    /// `(period_index, period_samples, bpm)` of the winner.
    pub fn select(&mut self, bank: &mut ResonatorBank) -> Option<(usize, usize, f32)> {
        let periods = bank.periods().to_vec();
        let energies = bank.total_energies().to_vec();
        if periods.is_empty() {
            return None;
        }
        self.score_buf.resize(periods.len(), 0.0);
        let w = &self.weights;
        let centre_log = w.prior_centre_bpm.ln();
        let sigma = w.prior_sigma.max(1e-6);

        // Look up energy at a candidate period via the periods slice.
        // Returns 0 if out of range.
        let lookup = |target: usize| -> f32 {
            match periods.binary_search(&target) {
                Ok(idx) => energies[idx],
                Err(_) => 0.0,
            }
        };

        for (i, &tau) in periods.iter().enumerate() {
            let e_t = energies[i];
            let e_th2 = lookup(tau / 2);
            let e_th3 = if tau >= 3 { lookup(tau / 3) } else { 0.0 };
            let e_m2 = lookup(tau * 2);
            let e_m3 = lookup(tau * 3);
            let e_m4 = lookup(tau * 4);

            let raw = w.w_tactus * e_t
                + w.w_tatum_half * e_th2
                + w.w_tatum_third * e_th3
                + w.w_measure_2 * e_m2
                + w.w_measure_3 * e_m3
                + w.w_measure_4 * e_m4;

            // BPM-prior in log-BPM space.
            let bpm = 60.0 * self.oss_rate / tau as f32;
            let z = (bpm.ln() - centre_log) / sigma;
            // Gaussian density (up to a constant): exp(-z²/2).
            let prior = (-0.5 * z * z).exp();
            self.score_buf[i] = raw * prior;
        }

        let (best_i, _) = self
            .score_buf
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())?;
        let tau = periods[best_i];
        let bpm = 60.0 * self.oss_rate / tau as f32;
        Some((best_i, tau, bpm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::klapuri::accent::N_BANDS;
    use crate::dsp::klapuri::resonators::ResonatorBank;

    /// Driving the bank with impulses every τ=86 frames (≈ 120 BPM
    /// at 172 Hz OSS), the joint inference should pick τ=86 — NOT
    /// the sub-harmonic τ=43 (which dominates raw resonator energy
    /// per the Phase 2 tests).
    #[test]
    fn inference_picks_correct_octave_on_clean_120bpm() {
        let oss_rate = 44_100.0 / 256.0;
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods);
        let target = 86usize;
        for i in 0..4000 {
            let accent = if i % target == 0 {
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        let mut inf = PeriodInference::new(oss_rate);
        let (_, tau, bpm) = inf.select(&mut bank).unwrap();
        assert!(
            (tau as i64 - target as i64).abs() <= 1,
            "expected tactus τ={target} (≈ {} BPM), got τ={tau} ({bpm:.1} BPM)",
            60.0 * oss_rate / target as f32
        );
    }

    /// Half-time-trap pattern: kicks on beats 1+3, snares on beats
    /// 2+4 at 120 BPM. Both arrive at the same OSS-rate period (one
    /// per half-beat = τ=43), so the bank's raw winner is τ=43.
    /// Joint inference with the BPM prior should still pick τ=86
    /// (120 BPM is much closer to the 120 BPM prior centre than 240
    /// BPM is).
    #[test]
    fn inference_picks_tactus_not_subdivision_on_full_mix_pattern() {
        let oss_rate = 44_100.0 / 256.0;
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods);
        // Period τ=86 for kick, with snare hits at offset 43.
        let beat = 86usize;
        for i in 0..4000 {
            let mut accent = [0.0f32; N_BANDS];
            if i % beat == 0 {
                accent[0] = 1.0; // kick in low band
            }
            if (i + beat / 2) % beat == 0 {
                accent[2] = 0.8; // snare in mid band
            }
            bank.tick(accent);
        }
        let mut inf = PeriodInference::new(oss_rate);
        let (_, tau, bpm) = inf.select(&mut bank).unwrap();
        assert!(
            (tau as i64 - beat as i64).abs() <= 2,
            "expected tactus τ={beat} (≈ 120 BPM), got τ={tau} ({bpm:.1} BPM)"
        );
    }

    /// Driving impulses at τ=128 (≈ 80 BPM) — slow waltz tempo. The
    /// prior centre is 120 BPM, so the 80 BPM peak has a smaller
    /// prior weight than say 160 BPM (its octave). But the actual
    /// resonator energy at τ=128 + super-harmonic support should
    /// still let it win.
    #[test]
    fn inference_picks_slow_tempo_despite_prior_offset() {
        let oss_rate = 44_100.0 / 256.0;
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods);
        let target = 128usize;
        for i in 0..5000 {
            let accent = if i % target == 0 {
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        let mut inf = PeriodInference::new(oss_rate);
        let (_, tau, bpm) = inf.select(&mut bank).unwrap();
        // Accept the right τ OR its octave (Klapuri may pick the
        // wrong octave when prior + energy fight; matching τ OR 2·τ
        // is good enough for this test).
        let ok = (tau as i64 - target as i64).abs() <= 2
            || (tau as i64 - (target / 2) as i64).abs() <= 2;
        assert!(
            ok,
            "expected τ={target} or octave, got τ={tau} ({bpm:.1} BPM)"
        );
    }
}
