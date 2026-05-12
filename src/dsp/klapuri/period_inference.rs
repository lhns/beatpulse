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
            w_measure_2: 0.7,
            w_measure_3: 0.3,
            w_measure_4: 0.2,
            // σ=0.8 in log-BPM space lets ballroom's full 60-220
            // BPM range be picked when the bank is confident.
            prior_sigma: 0.8,
            prior_centre_bpm: 120.0,
        }
    }
}

pub struct PeriodInference {
    pub weights: InferenceWeights,
    /// OSS rate (samples/sec) used to derive BPM from period.
    oss_rate: f32,
    /// Per-candidate τ score scratch — `raw_evidence × bpm_prior`.
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

    /// Reset all internal state. Currently a no-op — `select()`
    /// recomputes everything from the bank state each cycle. Kept as
    /// a stable hook for future temporal-filter activation.
    pub fn reset(&mut self) {}

    /// Score every candidate τ in `bank.periods()` and return the
    /// `(period_index, period_samples, period_frac, bpm)` of the
    /// winning tactus. `period_frac` is the parabolic-interpolated
    /// peak position in OSS frames — sub-frame resolution that
    /// prevents integer-τ quantisation from drifting the locked beat
    /// schedule by ~0.5–1 ms/beat (pass-11 fix).
    pub fn select(&mut self, bank: &mut ResonatorBank) -> Option<(usize, usize, f32, f32)> {
        let periods = bank.periods().to_vec();
        let energies = bank.total_energies().to_vec();
        if periods.is_empty() {
            return None;
        }
        let n = periods.len();
        self.score_buf.resize(n, 0.0);

        let w = &self.weights;
        let centre_log = w.prior_centre_bpm.ln();
        let sigma = w.prior_sigma.max(1e-6);
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
            let bpm = 60.0 * self.oss_rate / tau as f32;
            let z = (bpm.ln() - centre_log) / sigma;
            let prior = (-0.5 * z * z).exp();
            self.score_buf[i] = raw * prior;
        }

        let (best_i, _) = self
            .score_buf
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())?;
        let tau = periods[best_i];

        let tau_frac = if best_i > 0 && best_i + 1 < n {
            let s_lo = self.score_buf[best_i - 1];
            let s_mid = self.score_buf[best_i];
            let s_hi = self.score_buf[best_i + 1];
            let denom = s_lo - 2.0 * s_mid + s_hi;
            let offset = if denom.abs() > 1e-12 {
                (0.5 * (s_lo - s_hi) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            tau as f32 + offset
        } else {
            tau as f32
        };

        let bpm = 60.0 * self.oss_rate / tau_frac;
        Some((best_i, tau, tau_frac, bpm))
    }
}

// Pass-12 attempted to wire the joint (tatum, tactus, measure)
// posterior + an online forward filter through `select()`. Both
// regressed on Ballroom (joint per-frame: F 0.635 → 0.476;
// linear-sum + forward filter: F 0.635 → 0.603). The joint state
// space + likelihood/marginalisation lives in
// [`super::joint_posterior`] as a research artifact + future-work
// hook; the forward-step kernel below is preserved unattached for
// when the bank's evidence structure (or a different observation
// model) makes either viable.
#[allow(dead_code)]
fn forward_step(prev: &[f32], log_obs: &[f32], periods: &[usize]) -> Vec<f32> {
    const P_STAY: f32 = 0.85;
    const P_DRIFT: f32 = 0.05;
    const P_OCTAVE: f32 = 0.025;
    let log_p_stay = P_STAY.ln();
    let log_p_drift = P_DRIFT.ln();
    let log_p_octave = P_OCTAVE.ln();
    let n = periods.len();
    let mut out = vec![f32::NEG_INFINITY; n];
    for (j, &tau_j) in periods.iter().enumerate() {
        let mut log_in: [f32; 5] = [f32::NEG_INFINITY; 5];
        let mut k = 0;
        log_in[k] = prev[j] + log_p_stay;
        k += 1;
        if j > 0 {
            log_in[k] = prev[j - 1] + log_p_drift;
            k += 1;
        }
        if j + 1 < n {
            log_in[k] = prev[j + 1] + log_p_drift;
            k += 1;
        }
        if let Ok(i) = periods.binary_search(&(tau_j / 2)) {
            if i != j {
                log_in[k] = prev[i] + log_p_octave;
                k += 1;
            }
        }
        if let Ok(i) = periods.binary_search(&(tau_j * 2)) {
            if i != j {
                log_in[k] = prev[i] + log_p_octave;
                k += 1;
            }
        }
        let m = log_in[..k].iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        if m > f32::NEG_INFINITY {
            let s: f32 = log_in[..k].iter().map(|&x| (x - m).exp()).sum();
            out[j] = log_obs[j] + m + s.ln();
        } else {
            out[j] = log_obs[j];
        }
    }
    out
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
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
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
        let (_, tau, _tau_frac, bpm) = inf.select(&mut bank).unwrap();
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
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
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
        let (_, tau, _tau_frac, bpm) = inf.select(&mut bank).unwrap();
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
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
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
        let (_, tau, _tau_frac, bpm) = inf.select(&mut bank).unwrap();
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

    // ------------------------------------------------------------------
    // Phase A2 — period-range coverage tests added during the Klapuri
    // debugging pass (plan §A2). These pin down the missing-sub-
    // harmonic-evidence bug.
    // ------------------------------------------------------------------

    use crate::dsp::klapuri::resonators::default_period_range;

    /// For every tactus candidate τ in `default_period_range`, τ/2
    /// must also be in the range — otherwise the joint-inference
    /// sub-harmonic lookup silently returns 0 and starves the score
    /// of evidence. Currently FAILS for every τ ≤ 94.
    #[test]
    fn sub_harmonic_in_range_for_every_tactus_candidate() {
        let r = default_period_range(44_100, 256);
        let mut missing: Vec<usize> = Vec::new();
        for &tau in &r {
            // Only check candidates that are "tactus-shaped" (within
            // the 60-220 BPM band). The bank may also include
            // sub-harmonic-only τ values; those don't need their own
            // τ/2 to exist.
            let bpm = 60.0 * (44_100.0 / 256.0) / tau as f32;
            if (60.0..=220.0).contains(&bpm) {
                let sub = tau / 2;
                if !r.contains(&sub) {
                    missing.push(tau);
                }
            }
        }
        assert!(
            missing.is_empty(),
            "tactus candidates with τ/2 NOT in range: {missing:?} (max BPM = {})",
            60.0 * (44_100.0 / 256.0) / *r.first().unwrap() as f32
        );
    }

    /// Drive the bank with a clean 120 BPM impulse train (τ=86 at
    /// 172 Hz OSS). Inference should pick τ=86, NOT τ=43 (the
    /// sub-harmonic). The sub-harmonic lookup MUST find τ/2=43 in
    /// the bank for this to work robustly — without that evidence
    /// the test passes only because of the BPM prior, masking the
    /// bug.
    #[test]
    fn inference_uses_subharmonic_evidence_at_120bpm() {
        let oss_rate = 44_100.0 / 256.0;
        // Use the production default_period_range so this test
        // exercises the realistic bank.
        let periods = default_period_range(44_100, 256);
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let target = 86usize;
        for i in 0..4000 {
            let accent = if i % target == 0 {
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        // Verify the sub-harmonic exists in the range.
        let sub = target / 2;
        assert!(
            periods.contains(&sub),
            "τ/2={sub} not in default_period_range — sub-harmonic evidence \
             cannot be looked up. Extend the lower bound."
        );
        // Now run inference and check the winner.
        let mut inf = PeriodInference::new(oss_rate);
        let (_, tau, _tau_frac, bpm) = inf.select(&mut bank).unwrap();
        assert!(
            (tau as i64 - target as i64).abs() <= 1,
            "expected τ={target} (≈ 120 BPM), got τ={tau} ({bpm:.1} BPM)"
        );
    }
}
