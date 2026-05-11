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
}

impl Resonator {
    fn new(tau: usize, alpha: f32) -> Self {
        Self {
            tau,
            alpha,
            y_hist: vec![0.0; tau.max(1)],
            write: 0,
            energy: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, x: f32, energy_decay: f32) {
        let y_old = self.y_hist[self.write];
        let y_new = self.alpha * y_old + (1.0 - self.alpha) * x;
        self.y_hist[self.write] = y_new;
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
        self.write = 0;
        self.energy = 0.0;
    }
}

/// Compute the IIR comb α such that the response decays to half its
/// initial energy after `tau` samples (Klapuri's "half-energy time =
/// one period" convention).
///
/// Energy of an IIR comb at lag-multiples decays as `α^(2k)` per
/// τ-sample step → α = 0.5^(1/(2τ)) for half-energy at one period.
/// In practice the form `α = 0.5^(1/τ)` is also widely used (half-
/// amplitude rather than half-energy); we use the energy convention.
pub fn alpha_for_period(tau: usize) -> f32 {
    if tau == 0 {
        return 0.0;
    }
    0.5_f32.powf(1.0 / (2.0 * tau as f32))
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
    pub fn new(periods: &[usize]) -> Self {
        let resonators: Vec<Vec<Resonator>> = (0..N_BANDS)
            .map(|_| {
                periods
                    .iter()
                    .map(|&t| Resonator::new(t, alpha_for_period(t)))
                    .collect()
            })
            .collect();
        Self {
            periods: periods.to_vec(),
            resonators,
            energy_buf: vec![0.0; periods.len()],
            energy_decay: 0.99,
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
    /// summed-across-channels |y[n - 1 - j]| is largest. That offset
    /// is the most recent beat's phase: j=0 means a beat is "now",
    /// j=τ-1 means a beat happened almost a full period ago.
    pub fn phase_of(&self, period_idx: usize) -> usize {
        if period_idx >= self.periods.len() {
            return 0;
        }
        let tau = self.periods[period_idx];
        if tau == 0 {
            return 0;
        }
        // All channels share the same write pointer for the same
        // period (each resonator advances by exactly one per tick;
        // they were all initialised together). Read from channel 0
        // and trust the rest.
        let r0 = &self.resonators[0][period_idx];
        let write = r0.write;
        let mut best_j = 0usize;
        let mut best_v = f32::NEG_INFINITY;
        for j in 0..tau {
            // Slot (write - 1 - j) mod tau holds y[n - 1 - j]. Sum
            // |y| across channels.
            let slot = (write + tau - 1 - j) % tau;
            let mut v = 0.0f32;
            for chan in &self.resonators {
                v += chan[period_idx].y_hist[slot].abs();
            }
            if v > best_v {
                best_v = v;
                best_j = j;
            }
        }
        best_j
    }

    /// Per-period energy summed across the 4 channels. Returns a slice
    /// indexed by `periods()`. Borrows internal scratch — no
    /// allocation.
    pub fn total_energies(&mut self) -> &[f32] {
        for (i, e) in self.energy_buf.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for chan in &self.resonators {
                sum += chan[i].energy;
            }
            *e = sum;
        }
        &self.energy_buf
    }
}

/// Default candidate-period range covering 60-220 BPM at the
/// `(sr, hop)` OSS rate.
pub fn default_period_range(sr: u32, hop: usize) -> Vec<usize> {
    let oss_rate = sr as f32 / hop as f32;
    let min_bpm = 60.0_f32;
    let max_bpm = 220.0_f32;
    let max_period = (60.0 * oss_rate / min_bpm).ceil() as usize;
    let min_period = (60.0 * oss_rate / max_bpm).floor() as usize;
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
        let mut bank = ResonatorBank::new(&periods);
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
        let mut bank = ResonatorBank::new(&periods);
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

    /// alpha_for_period should be in (0, 1) and monotonically increase
    /// with τ.
    #[test]
    fn alpha_for_period_monotonic() {
        let prev = alpha_for_period(40);
        let next = alpha_for_period(200);
        assert!(prev > 0.0 && prev < 1.0, "alpha out of range: {prev}");
        assert!(next > prev, "alpha should grow with τ: {prev} → {next}");
    }

    /// default_period_range(44100, 256) covers the 60-220 BPM range at
    /// 172 Hz OSS.
    #[test]
    fn default_period_range_sane() {
        let r = default_period_range(44_100, 256);
        let oss = 44_100.0 / 256.0;
        let min_bpm = 60.0 * oss / *r.last().unwrap() as f32;
        let max_bpm = 60.0 * oss / *r.first().unwrap() as f32;
        assert!((min_bpm - 60.0).abs() < 1.0, "min BPM = {min_bpm}");
        assert!((max_bpm - 220.0).abs() < 5.0, "max BPM = {max_bpm}");
    }
}
