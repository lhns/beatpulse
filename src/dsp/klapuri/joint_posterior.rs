// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Joint (tatum, tactus, measure) probabilistic posterior — Klapuri 2006 §IV-C.
//!
//! Replaces the linear weighted-sum scoring in [`super::period_inference`]
//! with a proper probabilistic model over triples
//! `(τ_tactus, k_tatum, k_measure)`:
//!
//! - **Likelihood** `log P(obs | s) = α_t·log(1+e[τ_tactus]) +
//!   α_h·log(1+e[τ_tactus/k_tatum]) + α_m·log(1+e[k_measure·τ_tactus])`
//!   where `e[·]` are the bank's normalised resonator energies.
//! - **Prior** factorises: log-Gaussian on τ_tactus's BPM, weighted
//!   priors on k_tatum and k_measure.
//! - **Temporal continuity** via online forward filter: sparse
//!   transition kernel (stay / ±1 tempo drift / octave / k_tatum
//!   flip / k_measure shift), updated each inference cycle.
//!
//! The state space prunes impossible triples (those whose tatum or
//! measure τ falls outside the bank's period range) at construction
//! time. Typical size ~300–800 states; per-update cost is O(N_states ·
//! avg_fanout) ≈ a few thousand floating-point ops — trivial at the
//! ~3 Hz inference rate.

/// Tunable parameters for the joint posterior. Sensible defaults for
/// Western pop / ballroom genres. Phase D of pass 12 may tune these.
#[derive(Debug, Clone, Copy)]
pub struct JointWeights {
    /// Likelihood weight on the tactus level.
    pub alpha_t: f32,
    /// Likelihood weight on the tatum level (τ_tactus / k_tatum).
    pub alpha_h: f32,
    /// Likelihood weight on the measure level (k_measure · τ_tactus).
    pub alpha_m: f32,

    /// Prior probability of k_tatum = 2 (binary subdivision). 1 - this
    /// → ternary (k_tatum = 3).
    pub prior_binary_tatum: f32,
    /// Prior on k_measure ∈ {2, 3, 4}, normalised internally.
    pub prior_measure_2: f32,
    pub prior_measure_3: f32,
    pub prior_measure_4: f32,

    /// Centre of the log-Gaussian tactus-BPM prior, in BPM.
    pub prior_centre_bpm: f32,
    /// σ of the log-Gaussian tactus-BPM prior.
    pub prior_sigma: f32,

    /// Probability of staying in the same state across one inference
    /// cycle. Higher → smoother but slower tempo adaptation.
    pub p_stay: f32,
    /// Probability of an octave switch (τ → 2τ or τ → τ/2), summed
    /// across both directions.
    pub p_octave: f32,
    /// Probability of changing k_tatum or k_measure ("meta switch"),
    /// summed across all adjacent meta moves.
    pub p_meta: f32,

    /// Log-prior penalty applied to states whose measure τ (or
    /// tatum τ) falls outside the bank's period range. Without this,
    /// slow-tactus states get a "free pass" on missing-pulse-level
    /// evidence and unfairly out-compete fast-tactus states whose
    /// measure pulses fit but happen to have low resonator energy.
    /// Default = -3.0 (≈ prior factor 0.05 vs the measure-present
    /// states), strong enough to require a clear likelihood
    /// advantage before a partial-state can win.
    pub log_prior_missing_level: f32,
}

impl Default for JointWeights {
    fn default() -> Self {
        Self {
            alpha_t: 1.5,
            alpha_h: 0.2,
            alpha_m: 0.4,

            prior_binary_tatum: 0.7,
            prior_measure_2: 0.6,
            prior_measure_3: 0.25,
            prior_measure_4: 0.15,

            prior_centre_bpm: 120.0,
            prior_sigma: 0.8,

            p_stay: 0.92,
            p_octave: 0.01,
            p_meta: 0.01,

            log_prior_missing_level: 0.0,
        }
    }
}

/// One state in the joint posterior: a `(tactus τ, k_tatum, k_measure)`
/// triple. `tactus_period_idx` indexes into the bank's `periods()`
/// slice. `tatum_period_idx` and `measure_period_idx` are `Some(i)`
/// when the corresponding pulse τ is in the bank's range, or `None`
/// when out of range (the likelihood for that level then contributes
/// log(1 + 0) = 0 — the state still exists, it just lacks evidence
/// from that pulse level).
#[derive(Debug, Clone, Copy)]
pub struct JointState {
    pub tactus_period_idx: usize,
    pub tatum_period_idx: Option<usize>,
    pub measure_period_idx: Option<usize>,
    pub tactus_tau: usize,
    pub k_tatum: u8,
    pub k_measure: u8,
}

/// Precomputed joint state space + log-prior. Owned by `PeriodInference`
/// and rebuilt only when the bank's period range changes (effectively
/// once per `KlapuriTracker::new`).
pub struct JointStateSpace {
    pub weights: JointWeights,
    states: Vec<JointState>,
    /// Log-prior per state. Sums to ≈ 0 across normalised states (we
    /// don't normalise rigorously — the posterior `argmax` is shift-
    /// invariant in log-space).
    log_prior: Vec<f32>,
    /// Tactus-restricted period range — only τ values in 60–220 BPM
    /// are eligible as tactus. Stored as bank-index range
    /// `[tactus_lo_idx, tactus_hi_idx)`.
    tactus_lo_idx: usize,
    tactus_hi_idx: usize,
    oss_rate: f32,
}

impl JointStateSpace {
    /// Build the state space against `bank_periods` (assumed ascending
    /// integers in OSS frames, the output of `default_period_range`).
    /// Prunes triples whose tatum or measure τ falls outside the
    /// bank's range.
    pub fn new(bank_periods: &[usize], oss_rate: f32, weights: JointWeights) -> Self {
        // Tactus must correspond to BPM in [60, 220].
        let bpm_of = |tau: usize| 60.0 * oss_rate / tau as f32;
        let mut tactus_lo_idx = bank_periods.len();
        let mut tactus_hi_idx = 0;
        for (i, &tau) in bank_periods.iter().enumerate() {
            let bpm = bpm_of(tau);
            if (60.0..=220.0).contains(&bpm) {
                if i < tactus_lo_idx {
                    tactus_lo_idx = i;
                }
                if i + 1 > tactus_hi_idx {
                    tactus_hi_idx = i + 1;
                }
            }
        }
        if tactus_lo_idx >= tactus_hi_idx {
            // Degenerate range — bank too narrow.
            return Self {
                weights,
                states: Vec::new(),
                log_prior: Vec::new(),
                tactus_lo_idx: 0,
                tactus_hi_idx: 0,
                oss_rate,
            };
        }

        let centre_log = weights.prior_centre_bpm.ln();
        let sigma = weights.prior_sigma.max(1e-6);

        // Normalise measure prior across the three options.
        let p_m_sum = weights.prior_measure_2 + weights.prior_measure_3 + weights.prior_measure_4;
        let p_m2 = weights.prior_measure_2 / p_m_sum;
        let p_m3 = weights.prior_measure_3 / p_m_sum;
        let p_m4 = weights.prior_measure_4 / p_m_sum;

        let p_th2 = weights.prior_binary_tatum.clamp(1e-6, 1.0 - 1e-6);
        let p_th3 = 1.0 - p_th2;

        let mut states = Vec::with_capacity((tactus_hi_idx - tactus_lo_idx) * 6);
        let mut log_prior = Vec::with_capacity(states.capacity());

        for tactus_idx in tactus_lo_idx..tactus_hi_idx {
            let tau_t = bank_periods[tactus_idx];
            let bpm = bpm_of(tau_t);
            let z = (bpm.ln() - centre_log) / sigma;
            // Log-Gaussian tactus prior (up to a constant).
            let log_prior_tactus = -0.5 * z * z;

            for k_t_u in [2u8, 3u8] {
                let tau_tat = tau_t / k_t_u as usize;
                let tatum_idx = bank_periods.binary_search(&tau_tat).ok();
                let log_prior_tatum = if k_t_u == 2 { p_th2.ln() } else { p_th3.ln() };

                for k_m_u in [2u8, 3u8, 4u8] {
                    let tau_meas = tau_t * k_m_u as usize;
                    let measure_idx = bank_periods.binary_search(&tau_meas).ok();
                    let log_prior_measure = match k_m_u {
                        2 => p_m2.ln(),
                        3 => p_m3.ln(),
                        _ => p_m4.ln(),
                    };

                    let missing_penalty = if tatum_idx.is_none() {
                        weights.log_prior_missing_level
                    } else {
                        0.0
                    } + if measure_idx.is_none() {
                        weights.log_prior_missing_level
                    } else {
                        0.0
                    };
                    states.push(JointState {
                        tactus_period_idx: tactus_idx,
                        tatum_period_idx: tatum_idx,
                        measure_period_idx: measure_idx,
                        tactus_tau: tau_t,
                        k_tatum: k_t_u,
                        k_measure: k_m_u,
                    });
                    log_prior.push(
                        log_prior_tactus + log_prior_tatum + log_prior_measure + missing_penalty,
                    );
                }
            }
        }

        Self {
            weights,
            states,
            log_prior,
            tactus_lo_idx,
            tactus_hi_idx,
            oss_rate,
        }
    }

    pub fn n_states(&self) -> usize {
        self.states.len()
    }
    pub fn states(&self) -> &[JointState] {
        &self.states
    }
    pub fn log_prior(&self) -> &[f32] {
        &self.log_prior
    }
    pub fn tactus_range(&self) -> (usize, usize) {
        (self.tactus_lo_idx, self.tactus_hi_idx)
    }
    pub fn oss_rate(&self) -> f32 {
        self.oss_rate
    }

    /// Per-state log-likelihood from the current bank energies. Writes
    /// into `out` (which must have length `n_states()`). Allocation-
    /// free if `out` is reused.
    pub fn log_likelihood(&self, energies: &[f32], out: &mut [f32]) {
        debug_assert_eq!(out.len(), self.states.len());
        let w = &self.weights;

        // Per-cycle empirical noise estimate: the median bank energy
        // is the "everyone-resonates-a-bit" floor; matched resonators
        // sit substantially above it. We use median instead of mean
        // because the energy distribution is skewed (a few peaks,
        // many low values) — median gives a robust noise floor.
        let mut sorted = energies.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mu_n = sorted[sorted.len() / 2].max(1e-9);
        // MAD as the spread (more robust than σ on skewed data).
        let mad = {
            let mut absdev: Vec<f32> = sorted.iter().map(|&e| (e - mu_n).abs()).collect();
            absdev.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            absdev[absdev.len() / 2].max(1e-9)
        };
        let sigma_n = (1.4826 * mad).max(1e-9);
        // "Matched" model — peaks should be substantially above noise.
        let mu_match = mu_n + 3.0 * sigma_n;
        let sigma_match = (2.0 * sigma_n).max(1e-9);

        // Gaussian log-likelihood-ratio per pulse level. Energies near
        // mu_n give log_lr ≈ 0 (no info); energies near mu_match give
        // strongly positive log_lr; energies far below mu_n give
        // negative log_lr (rejected). This is what the paper's joint
        // model requires to reject weak peaks — log(1+e) couldn't.
        let log_lr = |e: f32| -> f32 {
            let z_match = (e - mu_match) / sigma_match;
            let z_noise = (e - mu_n) / sigma_n;
            // log P(e | matched) - log P(e | noise) up to constants
            // (Gaussian normalisation cancels except for the
            // log(σ_n / σ_match) bias, folded into a global constant
            // that doesn't affect argmax).
            -0.5 * z_match * z_match + 0.5 * z_noise * z_noise
        };

        for (s, lik) in self.states.iter().zip(out.iter_mut()) {
            let e_t = energies[s.tactus_period_idx].max(0.0);
            let e_h = s
                .tatum_period_idx
                .map(|i| energies[i].max(0.0))
                .unwrap_or(0.0);
            let e_m = s
                .measure_period_idx
                .map(|i| energies[i].max(0.0))
                .unwrap_or(0.0);
            // Levels with no bank coverage (Option None) get log_lr=0
            // — neutral evidence, neither boost nor penalty. The
            // missing-level prior penalty in `log_prior[]` handles
            // the "this state is partial" downweighting separately.
            let lr_h = if s.tatum_period_idx.is_some() {
                log_lr(e_h)
            } else {
                0.0
            };
            let lr_m = if s.measure_period_idx.is_some() {
                log_lr(e_m)
            } else {
                0.0
            };
            *lik = w.alpha_t * log_lr(e_t) + w.alpha_h * lr_h + w.alpha_m * lr_m;
        }
    }

    /// Marginalise the joint posterior across (k_tatum, k_measure) to
    /// produce a per-tactus log-score. Used by Phase-B (no-temporal)
    /// inference so the existing `PeriodInference::select` can keep
    /// returning a single τ_tactus.
    ///
    /// `bank_periods.len()` is `n_periods`; out has the same length.
    /// Entries for tactus indices outside `tactus_range` are filled
    /// with `f32::NEG_INFINITY`.
    /// Build the sparse transition table for the forward filter.
    /// Each source state maps to a list of `(target_state_idx,
    /// log_prob)` pairs. Includes: stay, ±1 tempo drift, ×2/×½
    /// octave jump, k_tatum flip, k_measure ±1 shift. Stored once,
    /// reused every inference cycle.
    pub fn build_transitions(&self) -> Vec<Vec<(usize, f32)>> {
        let w = &self.weights;
        // Allocate "probability budget" excluding stay.
        let p_stay = w.p_stay.clamp(1e-6, 1.0 - 1e-6);
        let p_octave_each = w.p_octave / 2.0;
        let p_meta_each = w.p_meta / 3.0; // tatum flip + measure ±1
                                          // Remaining for tempo drift (±1):
        let budget = (1.0 - p_stay - w.p_octave - w.p_meta).max(1e-6);
        let p_drift_each = budget / 2.0;

        let log_p_stay = p_stay.ln();
        let log_p_drift = p_drift_each.ln();
        let log_p_octave = p_octave_each.ln();
        let log_p_meta = p_meta_each.ln();

        // State-key → state-idx lookup so transitions can address by
        // (tactus_period_idx, k_tatum, k_measure). Sparse — exposed
        // only here.
        let key = |tactus_period_idx: usize, k_t: u8, k_m: u8| -> u64 {
            ((tactus_period_idx as u64) << 16) | ((k_t as u64) << 8) | (k_m as u64)
        };
        use std::collections::HashMap;
        let mut idx_for_key: HashMap<u64, usize> = HashMap::with_capacity(self.states.len());
        for (i, s) in self.states.iter().enumerate() {
            idx_for_key.insert(key(s.tactus_period_idx, s.k_tatum, s.k_measure), i);
        }

        let mut out = Vec::with_capacity(self.states.len());
        for s in &self.states {
            let mut row: Vec<(usize, f32)> = Vec::with_capacity(8);
            let here = idx_for_key[&key(s.tactus_period_idx, s.k_tatum, s.k_measure)];
            row.push((here, log_p_stay));
            // Tempo drift ±1.
            for dx in [-1i32, 1] {
                let ti = s.tactus_period_idx as i32 + dx;
                if ti >= 0 {
                    if let Some(&j) = idx_for_key.get(&key(ti as usize, s.k_tatum, s.k_measure)) {
                        row.push((j, log_p_drift));
                    }
                }
            }
            // k_tatum flip 2 ↔ 3.
            let other_kt = if s.k_tatum == 2 { 3 } else { 2 };
            if let Some(&j) = idx_for_key.get(&key(s.tactus_period_idx, other_kt, s.k_measure)) {
                row.push((j, log_p_meta));
            }
            // k_measure ±1 (within {2, 3, 4}).
            for dkm in [-1i8, 1] {
                let kmi = s.k_measure as i8 + dkm;
                if (2..=4).contains(&kmi) {
                    if let Some(&j) =
                        idx_for_key.get(&key(s.tactus_period_idx, s.k_tatum, kmi as u8))
                    {
                        row.push((j, log_p_meta));
                    }
                }
            }
            // Octave: tactus τ doubles → find state with tactus_tau ×2.
            // Have to scan since `tactus_tau` doesn't map directly.
            let tau = s.tactus_tau;
            for (tau_target, log_p) in [
                (tau.saturating_mul(2), log_p_octave),
                (tau / 2, log_p_octave),
            ] {
                if tau_target == 0 || tau_target == tau {
                    continue;
                }
                // Find any tactus_period_idx whose τ equals tau_target.
                if let Some(target_state_idx) = self
                    .states
                    .iter()
                    .enumerate()
                    .find(|(_, st)| {
                        st.tactus_tau == tau_target
                            && st.k_tatum == s.k_tatum
                            && st.k_measure == s.k_measure
                    })
                    .map(|(i, _)| i)
                {
                    row.push((target_state_idx, log_p));
                }
            }
            out.push(row);
        }
        out
    }

    pub fn marginalise_per_tactus(&self, log_lik: &[f32], n_periods: usize, out: &mut [f32]) {
        debug_assert_eq!(log_lik.len(), self.states.len());
        debug_assert_eq!(out.len(), n_periods);
        // Two-pass log-sum-exp per tactus: first pass finds the per-
        // tactus max (numerical-stability anchor), second pass sums
        // exp(value − max). The marginal log-sum-exp is then
        // `max + ln(sum)`.
        let mut max_per_tactus = vec![f32::NEG_INFINITY; n_periods];
        for (i, state) in self.states.iter().enumerate() {
            let v = log_lik[i] + self.log_prior[i];
            let m = &mut max_per_tactus[state.tactus_period_idx];
            if v > *m {
                *m = v;
            }
        }
        let mut sum_per_tactus = vec![0.0f32; n_periods];
        for (i, state) in self.states.iter().enumerate() {
            let v = log_lik[i] + self.log_prior[i];
            let m = max_per_tactus[state.tactus_period_idx];
            if m > f32::NEG_INFINITY {
                sum_per_tactus[state.tactus_period_idx] += (v - m).exp();
            }
        }
        for (i, out_v) in out.iter_mut().enumerate() {
            let m = max_per_tactus[i];
            *out_v = if m > f32::NEG_INFINITY {
                m + sum_per_tactus[i].ln()
            } else {
                f32::NEG_INFINITY
            };
        }
    }
}

/// Online forward filter over the joint state space. Maintains a
/// log-posterior `log_alpha[s]` representing `log P(s_t | obs_1..t)`,
/// updated each inference cycle via:
///
/// ```text
/// log_alpha_new[s] = log_lik(obs | s) + log_prior(s)
///                  + logsumexp_{s'} ( log_alpha_old[s'] + log_trans[s' → s] )
/// ```
///
/// On the very first call we initialise `log_alpha` from the joint
/// prior so that early observations refine a broad belief rather than
/// committing to the per-frame argmax (the pass-12 mistake).
///
/// Decoding returns `argmax_s log_alpha[s]` — the most likely current
/// joint state. The caller reads its `tactus_period_idx` for the τ
/// output. Continuity is the key advantage over per-frame
/// marginalisation: a momentary likelihood dropout (drum fill, lull)
/// doesn't unlock the tracker, and short-lived octave flips lose to
/// the accumulated evidence for the prior octave.
pub struct ForwardFilter {
    /// `Some(log_alpha)` after first observation; `None` on cold
    /// start. `reset()` clears.
    log_alpha: Option<Vec<f32>>,
    /// Reverse-transition table: for each *target* state, the list
    /// of `(source_idx, log_prob)` that flow into it. Precomputed
    /// from `JointStateSpace::build_transitions` (which is keyed by
    /// source — we invert here for O(N · avg_fanin) updates).
    rev_transitions: Vec<Vec<(usize, f32)>>,
    /// Scratch for one forward step.
    scratch: Vec<f32>,
}

impl ForwardFilter {
    pub fn new(state_space: &JointStateSpace) -> Self {
        let forward = state_space.build_transitions();
        let n = state_space.n_states();
        let mut rev: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
        for (src, row) in forward.iter().enumerate() {
            for &(tgt, lp) in row {
                rev[tgt].push((src, lp));
            }
        }
        Self {
            log_alpha: None,
            rev_transitions: rev,
            scratch: vec![0.0; n],
        }
    }

    pub fn reset(&mut self) {
        self.log_alpha = None;
    }

    /// One forward step. `log_lik` is the per-state log-likelihood
    /// (from `JointStateSpace::log_likelihood`); `log_prior` is the
    /// per-state log-prior (from `JointStateSpace::log_prior()`).
    /// Returns the argmax state index.
    pub fn update(&mut self, log_lik: &[f32], log_prior: &[f32]) -> usize {
        let n = self.rev_transitions.len();
        debug_assert_eq!(log_lik.len(), n);
        debug_assert_eq!(log_prior.len(), n);
        self.scratch.resize(n, f32::NEG_INFINITY);

        // First call: log_alpha = log_prior + log_lik (a single
        // observation refining the broad prior).
        let prev = match &self.log_alpha {
            Some(v) => v.clone(),
            None => log_prior.to_vec(),
        };

        // Forward update: for each target, log-sum-exp over sources.
        for (tgt, sources) in self.rev_transitions.iter().enumerate() {
            if sources.is_empty() {
                self.scratch[tgt] = log_lik[tgt] + log_prior[tgt];
                continue;
            }
            let mut max_in = f32::NEG_INFINITY;
            for &(src, lp) in sources {
                let v = prev[src] + lp;
                if v > max_in {
                    max_in = v;
                }
            }
            let mut sum = 0.0f32;
            for &(src, lp) in sources {
                sum += (prev[src] + lp - max_in).exp();
            }
            let log_propagated = max_in + sum.ln();
            // Include log_prior with a small weight on every update —
            // anchors the filter so degenerate observations don't
            // drift the posterior away from plausible tempos. The
            // prior is constant across cycles so doesn't bias the
            // continuity.
            self.scratch[tgt] = log_lik[tgt] + log_propagated;
        }

        // Normalise: subtract max for numerical stability.
        let max = self
            .scratch
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        if max.is_finite() {
            for v in self.scratch.iter_mut() {
                *v -= max;
            }
        }
        self.log_alpha = Some(self.scratch.clone());

        // Argmax.
        self.scratch
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// Per-tactus marginal log-posterior (after forward update) —
    /// used by `period_inference::select` for parabolic-τ-frac
    /// interpolation around the winning tactus. Writes into `out`.
    pub fn marginalise_per_tactus(
        &self,
        state_space: &JointStateSpace,
        n_periods: usize,
        out: &mut [f32],
    ) {
        debug_assert_eq!(out.len(), n_periods);
        let Some(log_alpha) = self.log_alpha.as_ref() else {
            for v in out.iter_mut() {
                *v = f32::NEG_INFINITY;
            }
            return;
        };
        let mut max_per_tactus = vec![f32::NEG_INFINITY; n_periods];
        for (i, state) in state_space.states().iter().enumerate() {
            let v = log_alpha[i];
            let m = &mut max_per_tactus[state.tactus_period_idx];
            if v > *m {
                *m = v;
            }
        }
        let mut sum_per_tactus = vec![0.0f32; n_periods];
        for (i, state) in state_space.states().iter().enumerate() {
            let m = max_per_tactus[state.tactus_period_idx];
            if m > f32::NEG_INFINITY {
                sum_per_tactus[state.tactus_period_idx] += (log_alpha[i] - m).exp();
            }
        }
        for (i, out_v) in out.iter_mut().enumerate() {
            let m = max_per_tactus[i];
            *out_v = if m > f32::NEG_INFINITY {
                m + sum_per_tactus[i].ln()
            } else {
                f32::NEG_INFINITY
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::klapuri::resonators::default_period_range;

    #[test]
    fn state_space_prunes_impossible_triples() {
        let periods = default_period_range(44_100, 256);
        let oss_rate = 44_100.0 / 256.0;
        let ss = JointStateSpace::new(&periods, oss_rate, JointWeights::default());

        // If a state has tatum_period_idx Some, the tau matches; same
        // for measure. None entries are fine (level out of bank range).
        for s in ss.states() {
            if let Some(i) = s.tatum_period_idx {
                assert_eq!(periods[i], s.tactus_tau / s.k_tatum as usize);
            }
            if let Some(i) = s.measure_period_idx {
                assert_eq!(periods[i], s.tactus_tau * s.k_measure as usize);
            }
        }

        // Tactus indices are all in the 60-220 BPM range.
        let (lo, hi) = ss.tactus_range();
        for s in ss.states() {
            assert!(s.tactus_period_idx >= lo && s.tactus_period_idx < hi);
        }

        // Should have meaningful number of states.
        assert!(
            ss.n_states() > 100 && ss.n_states() < 2000,
            "n_states = {}",
            ss.n_states()
        );
    }

    #[test]
    fn marginalise_picks_tactus_for_120bpm_impulse() {
        // Build a synthetic energy vector that's concentrated at τ=86
        // (120 BPM at 172 Hz OSS), τ=43 (its tatum), and τ=172 (its
        // measure ×2). The joint posterior should pick τ_tactus=86.
        let periods = default_period_range(44_100, 256);
        let oss_rate = 44_100.0 / 256.0;
        let ss = JointStateSpace::new(&periods, oss_rate, JointWeights::default());

        let mut energies = vec![0.01f32; periods.len()];
        let idx_86 = periods.iter().position(|&t| t == 86).unwrap();
        let idx_43 = periods.iter().position(|&t| t == 43).unwrap();
        let idx_172 = periods.iter().position(|&t| t == 172).unwrap();
        energies[idx_86] = 1.0;
        energies[idx_43] = 0.6;
        energies[idx_172] = 0.4;

        let mut log_lik = vec![0.0f32; ss.n_states()];
        ss.log_likelihood(&energies, &mut log_lik);
        let mut marginal = vec![f32::NEG_INFINITY; periods.len()];
        ss.marginalise_per_tactus(&log_lik, periods.len(), &mut marginal);

        // Argmax of marginal should be τ=86 (or very close).
        let (best_i, _) = marginal
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();
        let best_tau = periods[best_i];
        assert!(
            (best_tau as i64 - 86).abs() <= 1,
            "expected tactus τ=86, got τ={best_tau}"
        );
    }

    #[test]
    fn marginalise_rejects_tactus_for_double_tempo_trap() {
        // Synthetic doubled-energy pattern: τ=43 (240 BPM) has the
        // highest single-resonator energy, but τ=86 has comparable +
        // its measure τ=172 also has energy. Without the joint
        // posterior, linear-sum picks τ=43; with joint posterior, the
        // tactus prior + measure consistency should pick τ=86.
        let periods = default_period_range(44_100, 256);
        let oss_rate = 44_100.0 / 256.0;
        let ss = JointStateSpace::new(&periods, oss_rate, JointWeights::default());

        let mut energies = vec![0.01f32; periods.len()];
        let idx_43 = periods.iter().position(|&t| t == 43).unwrap();
        let idx_86 = periods.iter().position(|&t| t == 86).unwrap();
        let idx_172 = periods.iter().position(|&t| t == 172).unwrap();
        // Simulate the structural short-τ bias: e(43) > e(86), but
        // e(172) (the measure of 86) also has support.
        energies[idx_43] = 0.6;
        energies[idx_86] = 0.5;
        energies[idx_172] = 0.3;

        let mut log_lik = vec![0.0f32; ss.n_states()];
        ss.log_likelihood(&energies, &mut log_lik);
        let mut marginal = vec![f32::NEG_INFINITY; periods.len()];
        ss.marginalise_per_tactus(&log_lik, periods.len(), &mut marginal);

        let (best_i, _) = marginal
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();
        let best_tau = periods[best_i];
        // The tactus prior alone strongly disfavours 43 (240 BPM is
        // far from 120 BPM centre); combined with measure support at
        // 172, the joint posterior should pick 86.
        assert_eq!(best_tau, 86, "expected τ=86, got τ={best_tau}");
    }
}
