// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Comb-filter resonator bank — Klapuri 2006 §IV-A,B.
//!
//! Bank of IIR comb filters
//!
//! ```text
//! y[n] = α · y[n - τ] + (1 - α) · x[n]
//! ```
//!
//! tuned to candidate periods τ. α per-period is chosen so the
//! half-energy time of the resonator equals one period (Klapuri's
//! convention). Per-resonator output energy is integrated via a leaky
//! integrator and summed across the 4 accent channels — that sum is
//! the period-evidence vector consumed by the period-inference stage.

use crate::dsp::klapuri::accent::N_BANDS;

/// One IIR comb resonator at fixed period τ.
struct Resonator {
    tau: usize,
    alpha: f32,
    /// Ring buffer of past outputs y[n − k] for k ∈ 1..=τ. The slot at
    /// `write` holds y[n − τ] (the next read), and after writing y[n]
    /// there `write` advances.
    y_hist: Vec<f32>,
    write: usize,
    /// Leaky-integrated y² — the period evidence.
    energy: f32,
    /// Per-phase accumulator of |y_new|, smoothed across many cycles.
    /// Indexed in the same coordinate system as `y_hist`. Used by
    /// `phase_of` to find the dominant periodic alignment instead of
    /// the loudest single recent sample (Klapuri 2006 §IV-D
    /// cross-correlation).
    phase_acc: Vec<f32>,
}

/// Pole for the per-phase smoothing in `Resonator::phase_acc`. γ=0.85
/// integrates over ~7 consecutive ticks at the same phase position
/// (so on a τ=86 resonator at 172 Hz OSS, the accumulator spans ~3.5
/// seconds of evidence per phase slot — long enough to reject
/// one-off transients, short enough to track tempo changes).
const PHASE_ACC_GAMMA: f32 = 0.85;

impl Resonator {
    fn new(tau: usize, alpha: f32) -> Self {
        let n = tau.max(1);
        Self {
            tau,
            alpha,
            y_hist: vec![0.0; n],
            write: 0,
            energy: 0.0,
            phase_acc: vec![0.0; n],
        }
    }

    #[inline]
    fn tick(&mut self, x: f32, energy_decay: f32) {
        let y_old = self.y_hist[self.write];
        let y_new = self.alpha * y_old + (1.0 - self.alpha) * x;
        self.y_hist[self.write] = y_new;
        // Update per-phase accumulator at the slot just written.
        // After many cycles, phase_acc[s] is the smoothed long-run
        // |y| at phase position s — the dominant periodic alignment.
        let amp = y_new.abs();
        self.phase_acc[self.write] =
            PHASE_ACC_GAMMA * self.phase_acc[self.write] + (1.0 - PHASE_ACC_GAMMA) * amp;
        self.write += 1;
        if self.write >= self.tau {
            self.write = 0;
        }
        // Leaky-integrate y² for period evidence. `energy_decay` is the
        // pole; smaller = longer integration window.
        self.energy = energy_decay * self.energy + (1.0 - energy_decay) * y_new * y_new;
    }

    fn reset(&mut self) {
        self.y_hist.fill(0.0);
        self.phase_acc.fill(0.0);
        self.write = 0;
        self.energy = 0.0;
    }
}

/// Compute the IIR comb α such that the response decays to half its
/// initial energy after a *fixed wall-clock time* `T₀.₅` regardless
/// of τ (Klapuri 2006 §IV-B). This contrasts with the "half-energy
/// at one period" convention used previously, which gave τ-dependent
/// integration windows.
///
/// Per period τ, the loop gain decays by α; over k periods (= k·τ
/// ticks at OSS rate) the gain is α^k. Setting α^k = 0.5 with
/// k = T₀.₅ · fs_env / τ gives `α = 0.5^(τ / (T₀.₅ · fs_env))`.
///
/// At T₀.₅ = 3 s, τ=86 (120 BPM @ 172 Hz OSS), the resonator
/// integrates ~6 periods of beat evidence — much more discriminative
/// than the 2-period window the prior formula gave.
const HALF_ENERGY_SECONDS: f32 = 3.0;

pub fn alpha_for_period(tau: usize, oss_rate: f32) -> f32 {
    if tau == 0 || oss_rate <= 0.0 {
        return 0.0;
    }
    let periods_per_t_half = (HALF_ENERGY_SECONDS * oss_rate) / tau as f32;
    if periods_per_t_half <= 0.0 {
        return 0.0;
    }
    0.5_f32.powf(1.0 / periods_per_t_half)
}

pub struct ResonatorBank {
    /// Periods (in OSS frames) covered, ascending.
    periods: Vec<usize>,
    /// Per channel, per period. `resonators[c][i]` corresponds to
    /// `periods[i]` driven by accent channel `c`.
    resonators: Vec<Vec<Resonator>>,
    /// Reusable scratch for `total_energies()`.
    energy_buf: Vec<f32>,
    /// Leaky-integrator pole for per-resonator energy. Default `0.99`
    /// → ~1 s smoothing at 172 Hz OSS.
    energy_decay: f32,
}

impl ResonatorBank {
    pub fn new(periods: &[usize], oss_rate: f32) -> Self {
        let resonators: Vec<Vec<Resonator>> = (0..N_BANDS)
            .map(|_| {
                periods
                    .iter()
                    .map(|&t| Resonator::new(t, alpha_for_period(t, oss_rate)))
                    .collect()
            })
            .collect();
        Self {
            periods: periods.to_vec(),
            resonators,
            energy_buf: vec![0.0; periods.len()],
            energy_decay: 0.998,
        }
    }

    pub fn periods(&self) -> &[usize] {
        &self.periods
    }

    pub fn set_energy_decay(&mut self, d: f32) {
        self.energy_decay = d.clamp(0.0, 0.9999);
    }

    pub fn reset(&mut self) {
        for chan in &mut self.resonators {
            for r in chan {
                r.reset();
            }
        }
        self.energy_buf.fill(0.0);
    }

    /// Push one accent frame (4 channels) through the bank. O(N_BANDS
    /// × periods.len()) work per tick — for 4 channels × 160 periods ≈
    /// 640 multiplications, fine at 172 Hz OSS rate.
    pub fn tick(&mut self, accent: [f32; N_BANDS]) {
        for (c, chan) in self.resonators.iter_mut().enumerate() {
            for r in chan.iter_mut() {
                r.tick(accent[c], self.energy_decay);
            }
        }
    }

    /// For period index `i`, find the offset j ∈ [0, τ) where the
    /// long-run |y| accumulator (`Resonator::phase_acc`) is largest
    /// across the 4 channels. j=0 means a beat is "now"; j=τ-1 means
    /// a beat happened almost a full period ago.
    ///
    /// Uses the `phase_acc` smoothed-amplitude buffer, not the raw
    /// `y_hist` argmax. This is the cross-correlation-equivalent of
    /// Klapuri 2006 §IV-D — it's the dominant *periodic* alignment,
    /// robust to one-off loud transients (drum fills, syncopated
    /// accents) that throw off a single-window argmax.
    pub fn phase_of(&self, period_idx: usize) -> usize {
        if period_idx >= self.periods.len() {
            return 0;
        }
        let tau = self.periods[period_idx];
        if tau == 0 {
            return 0;
        }
        let r0 = &self.resonators[0][period_idx];
        let write = r0.write;
        let mut best_j = 0usize;
        let mut best_v = f32::NEG_INFINITY;
        for j in 0..tau {
            // Slot (write - 1 - j) mod tau holds the amplitude
            // accumulator for the phase position that was just-
            // written j ticks ago.
            let slot = (write + tau - 1 - j) % tau;
            let mut v = 0.0f32;
            for chan in &self.resonators {
                v += chan[period_idx].phase_acc[slot];
            }
            if v > best_v {
                best_v = v;
                best_j = j;
            }
        }
        best_j
    }

    /// Per-period energy summed across the 4 channels, normalised for
    /// the per-τ integration-gain bias. Returns a slice indexed by
    /// `periods()`. Borrows internal scratch — no allocation.
    ///
    /// **Normalisation rationale:** for a comb filter `y[n] =
    /// α·y[n-τ] + (1-α)·x[n]` driven by unit-power white noise, the
    /// steady-state response is `E[y²] = (1-α) / (1+α)`. Short τ has
    /// smaller α (since `alpha = 0.5^(1/(2τ))`), so `(1-α)/(1+α)` is
    /// larger — short-τ resonators accumulate ~3.5× more energy on
    /// broadband input than long-τ ones for the same input statistics.
    /// On real audio (vs synthetic impulse trains) this bias completely
    /// dominates the resonator bank: every track's top energies pile
    /// up at the shortest periods regardless of true tempo.
    ///
    /// We divide each resonator's energy by its per-τ baseline so
    /// resonators are comparable across τ — the comparison is then
    /// "does the resonator at τ have *anomalously* high energy
    /// relative to its baseline?", which is the right question for
    /// period inference.
    pub fn total_energies(&mut self) -> &[f32] {
        for (i, e) in self.energy_buf.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for chan in &self.resonators {
                sum += chan[i].energy;
            }
            let a = self.resonators[0][i].alpha;
            let gain = (1.0 - a) / (1.0 + a);
            *e = if gain > 1e-9 { sum / gain } else { 0.0 };
        }
        &self.energy_buf
    }
}

/// Default candidate-period range. Covers 60-220 BPM as tactus
/// candidates AND extends down to ~430 BPM-equivalent (τ≈24 at 172
/// Hz OSS) so `period_inference` can look up sub-harmonic τ/2
/// evidence for every tactus τ. The BPM prior in the inference stage
/// keeps the short-period resonators from being *chosen* as the
/// tactus — they exist only as supporting evidence.
pub fn default_period_range(sr: u32, hop: usize) -> Vec<usize> {
    let oss_rate = sr as f32 / hop as f32;
    let min_bpm_tactus = 60.0_f32;
    let max_bpm_evidence = 440.0_f32;
    let max_period = (60.0 * oss_rate / min_bpm_tactus).ceil() as usize;
    let min_period = (60.0 * oss_rate / max_bpm_evidence).floor() as usize;
    (min_period.max(1)..=max_period).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed an impulse train at exactly τ=86 frames (≈ 120 BPM at
    /// 172 Hz). The resonator at τ=86 AND its sub-harmonics (τ=43,
    /// τ=28, …) all resonate — that's a property of comb filters. The
    /// disambiguation across octaves is the period-inference stage's
    /// job (Phase 3, joint priors). Here we just assert that τ=86 is
    /// among the top-K resonators — the bank gives correct evidence,
    /// even if a prior is needed to pick the right level.
    #[test]
    fn impulse_train_excites_matched_period_among_topk() {
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let target = 86usize;
        for i in 0..3000 {
            let accent = if i % target == 0 {
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        let energies = bank.total_energies();
        let target_idx = target - periods[0];
        let mut sorted: Vec<usize> = (0..energies.len()).collect();
        sorted.sort_by(|&a, &b| energies[b].partial_cmp(&energies[a]).unwrap());
        let rank = sorted.iter().position(|&i| i == target_idx).unwrap();
        assert!(
            rank < 5,
            "τ={target} should be top-5 by energy; rank={rank}, top-5 τ = {:?}",
            &sorted[..5].iter().map(|&i| periods[i]).collect::<Vec<_>>()
        );
        // Sub-harmonics (τ=43, τ=28) are expected to outrank τ=86 —
        // that's the comb's intrinsic behaviour. The "true" period
        // emerges from the joint inference stage (Phase 3).
        let sub_harmonic_idx = (target / 2) - periods[0];
        let sh_energy = energies[sub_harmonic_idx];
        let target_energy = energies[target_idx];
        assert!(
            sh_energy > 0.0 && target_energy > 0.0,
            "both τ=86 and τ=43 should accumulate energy"
        );
    }

    /// Even with ±10 % jitter on impulse positions, the matched
    /// resonator should still register meaningful energy (within the
    /// top half of the bank).
    #[test]
    fn impulse_train_with_jitter_still_above_median() {
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let target = 86usize;
        let mut seed: u32 = 0xABCDEF12;
        let mut next_imp = 0i64;
        for i in 0..3000 {
            let accent = if (i as i64) >= next_imp {
                seed = seed.wrapping_mul(48271) % 2_147_483_647;
                let r = (seed as f32 / 2_147_483_647.0) * 2.0 - 1.0;
                let jitter = (r * 0.1 * target as f32) as i64;
                next_imp = (i as i64) + target as i64 + jitter;
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        let energies = bank.total_energies();
        let target_idx = target - periods[0];
        let mut sorted: Vec<usize> = (0..energies.len()).collect();
        sorted.sort_by(|&a, &b| energies[b].partial_cmp(&energies[a]).unwrap());
        let rank = sorted.iter().position(|&i| i == target_idx).unwrap();
        assert!(
            rank < energies.len() / 2,
            "matched resonator (τ={target}) should be in top half under jitter; rank={rank}"
        );
    }

    /// `alpha_for_period` should be in (0, 1). With the fixed-3-second
    /// half-energy convention, α *decreases* with τ (longer periods
    /// fit fewer cycles in 3s, so each cycle decays faster).
    #[test]
    fn alpha_for_period_in_range() {
        let oss = 44100.0 / 256.0;
        let short = alpha_for_period(40, oss);
        let long = alpha_for_period(200, oss);
        assert!(short > 0.0 && short < 1.0, "α out of range: {short}");
        assert!(long > 0.0 && long < 1.0, "α out of range: {long}");
        // Short τ → fits more cycles in 3s → loop gain per cycle larger
        // → α larger. Long τ → fewer cycles in 3s → α smaller.
        assert!(
            short > long,
            "α should be larger for short τ under fixed-T₀.₅ convention: {short} > {long}"
        );
    }

    /// default_period_range(44100, 256) covers tactus candidates 60-220
    /// BPM AND their τ/2 sub-harmonics (so `period_inference` can look
    /// up support evidence for any tactus). After the fix, the range
    /// extends down to ~τ=24 (≈ 430 BPM-equivalent, used only as
    /// sub-harmonic evidence — the BPM prior keeps these from being
    /// chosen as the tactus).
    #[test]
    fn default_period_range_covers_tactus_and_subharmonics() {
        let r = default_period_range(44_100, 256);
        let oss = 44_100.0 / 256.0;
        let max_bpm = 60.0 * oss / *r.first().unwrap() as f32;
        let min_bpm_tactus = 60.0 * oss / *r.last().unwrap() as f32;
        // Range still covers 60 BPM at the long end.
        assert!(
            (min_bpm_tactus - 60.0).abs() < 1.0,
            "long-end ≈ 60 BPM (got {min_bpm_tactus:.1})"
        );
        // Range extends low enough that every tactus candidate τ ≥ 47
        // has its τ/2 also in the range.
        for &tau in &r {
            if tau >= 47 {
                let sub = tau / 2;
                assert!(
                    r.contains(&sub),
                    "τ/2={sub} for tactus τ={tau} not in range; max BPM = {max_bpm:.1}"
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Phase A1 / A3 — component-correctness tests added during the
    // Klapuri debugging pass (plan §A). These pin down specific bugs
    // in `phase_of` and verify the bank's behavioural invariants.
    // ------------------------------------------------------------------

    /// `phase_of` should report ~0 just after an impulse arrives at a
    /// resonator's matched period. Sanity check that the basic
    /// "impulse just happened" case works.
    #[test]
    fn phase_of_zero_just_after_impulse() {
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let target = 86usize;
        for cycles in 0..10 {
            for i in 0..target {
                let accent = if i == 0 {
                    [1.0_f32, 0.0, 0.0, 0.0]
                } else {
                    [0.0_f32; N_BANDS]
                };
                bank.tick(accent);
            }
            // Just ticked the impulse for cycle `cycles`; phase_of
            // should report ~0 (most recent beat = now).
            if cycles >= 5 {
                let phase = bank.phase_of(target - periods[0]);
                assert!(
                    phase <= 1 || phase >= target - 1,
                    "after impulse, expected phase ≈ 0; got {phase} (τ={target})"
                );
            }
        }
    }

    /// With ±8-frame jitter on impulse positions, `phase_of` should
    /// settle on a stable phase (stdev across last 10 measurements
    /// < 5 frames). Currently *fails* — argmax tracks the loudest
    /// recent impulse rather than the modal alignment.
    #[test]
    fn phase_of_picks_modal_phase_under_jitter() {
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let target = 86usize;
        let mut seed: u32 = 0xABCDEF12;
        let mut next_imp: i64 = 0;
        let mut measurements: Vec<usize> = Vec::new();
        for i in 0..3000i64 {
            let accent = if i >= next_imp {
                seed = seed.wrapping_mul(48271) % 2_147_483_647;
                let r = (seed as f32 / 2_147_483_647.0) * 2.0 - 1.0;
                let jitter = (r * 8.0) as i64;
                next_imp = i + target as i64 + jitter;
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
            // Sample phase periodically once warmed up.
            if i > 1500 && i % target as i64 == 0 {
                measurements.push(bank.phase_of(target - periods[0]));
            }
        }
        assert!(
            measurements.len() >= 10,
            "expected ≥ 10 phase measurements, got {}",
            measurements.len()
        );
        let last10 = &measurements[measurements.len() - 10..];
        // Phase wraps around modulo τ; compute circular stdev.
        let dev = circular_stdev(last10, target);
        assert!(
            dev < 5.0,
            "phase should be stable under jitter; circular stdev={dev:.2} over last 10 samples = {last10:?}"
        );
    }

    /// A single loud transient 10 frames before "now", mixed with a
    /// clean 86-frame impulse train, should NOT shift `phase_of`'s
    /// answer to ~10. The argmax-over-one-period implementation does
    /// shift; the cross-correlation implementation should not.
    #[test]
    fn phase_of_robust_to_loud_offbeat_transient() {
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let target = 86usize;
        // Warm up with 20 cycles of clean impulse train.
        for i in 0..(target * 20) {
            let accent = if i % target == 0 {
                [1.0_f32, 0.0, 0.0, 0.0]
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        // Inject one big offbeat impulse 10 frames before "now".
        for i in 0..10 {
            let accent = if i == 0 {
                [3.0_f32, 0.0, 0.0, 0.0] // 3× the periodic amplitude
            } else {
                [0.0_f32; N_BANDS]
            };
            bank.tick(accent);
        }
        // Phase should still be ≈ 10 (since most recent BEAT was 10
        // frames ago in the periodic train, not the loud spike at
        // frame 10 just before "now"). Tolerance generous; just need
        // to NOT see phase ≈ 0 (which would mean we tracked the loud
        // spike).
        let phase = bank.phase_of(target - periods[0]);
        assert!(
            !(phase <= 2 || phase >= target - 2),
            "phase should NOT lock on the loud offbeat spike at j=0; got {phase} (τ={target})"
        );
    }

    /// Uniform [0,1] random accent → max-energy resonator should not
    /// dwarf the median by more than ~100×. Real accent signals are
    /// positive-only (HWR-diff output), so we test with that signal
    /// shape. Long-τ resonators integrate DC of the positive-only
    /// input and accumulate more energy than short-τ ones — some bias
    /// is inevitable. What we want to catch is a *single* resonator
    /// anomalously winning by orders of magnitude.
    #[test]
    fn energy_distribution_bounded_for_random_accent() {
        let periods: Vec<usize> = (40..=200).collect();
        let mut bank = ResonatorBank::new(&periods, 44100.0 / 256.0);
        let mut seed: u32 = 0xCAFEF00D;
        for _ in 0..6000 {
            seed = seed.wrapping_mul(48271) % 2_147_483_647;
            let r = (seed as f32 / 2_147_483_647.0).abs();
            bank.tick([r, r * 0.7, r * 0.5, r * 0.3]);
        }
        let energies = bank.total_energies();
        let mut sorted: Vec<f32> = energies.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        let max = *sorted.last().unwrap();
        assert!(
            max < median * 100.0,
            "no resonator should dwarf the rest by > 100×; max={max:.4} median={median:.4} ratio={:.2}",
            max / median
        );
    }

    /// Compute circular standard deviation of `samples` modulo `period`.
    /// Returns "stdev" in sample-units, accounting for wrap-around.
    #[allow(dead_code)]
    fn circular_stdev(samples: &[usize], period: usize) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let p = period as f32;
        // Convert to unit-circle vectors and average.
        let mut sx = 0.0f32;
        let mut sy = 0.0f32;
        for &s in samples {
            let theta = 2.0 * std::f32::consts::PI * (s as f32) / p;
            sx += theta.cos();
            sy += theta.sin();
        }
        let n = samples.len() as f32;
        let r = ((sx * sx + sy * sy).sqrt() / n).clamp(0.0, 1.0);
        // Circular stdev formula: sqrt(-2 * ln(r)), in radians.
        // Convert back to sample-units: × p / (2π).
        if r <= 1e-9 {
            // Maximally dispersed; return ~quarter period.
            p / 4.0
        } else {
            (-2.0 * r.ln()).sqrt() * p / (2.0 * std::f32::consts::PI)
        }
    }
}
