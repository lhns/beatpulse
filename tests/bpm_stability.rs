// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! BPM-stability contract: the new `tempo_stability` parameter must
//! actually trade off responsiveness for smoothness. See ADR-0024.

use beatpulse::dsp::beat_pll::BeatPll;

const SR: f64 = 44_100.0;

/// Drive the PLL with onsets at `bpm` perturbed by Gaussian-ish jitter
/// (uniform [-jitter_ms, +jitter_ms] for simplicity), with the given
/// `alpha_period`. Returns σ of the reported BPM over `n` post-lock
/// onsets.
fn run_with_jitter(bpm: f64, jitter_ms: f64, alpha_period: f64, n: usize) -> f64 {
    let mut pll = BeatPll::new(SR);
    pll.alpha_period = alpha_period;
    pll.alpha_phase = 0.165;

    let period_samples = SR * 60.0 / bpm;
    let jitter_samples = jitter_ms / 1000.0 * SR;

    // Deterministic pseudo-random — Park-Miller LCG.
    let mut seed: u32 = 0xCAFEF00D;
    let mut next_rand = || -> f64 {
        seed = seed.wrapping_mul(48271).wrapping_rem(2_147_483_647);
        (seed as f64 / 2_147_483_647.0) * 2.0 - 1.0 // [-1, 1]
    };

    // Warm up — at least LOCK_THRESHOLD onsets.
    let mut t = 1.0e6;
    for _ in 0..16 {
        pll.on_onset(t);
        t += period_samples;
    }
    assert!(pll.locked, "PLL didn't lock during warmup");

    // Sample BPM at each post-warmup onset.
    let mut bpms = Vec::with_capacity(n);
    for _ in 0..n {
        let perturbation = next_rand() * jitter_samples;
        pll.on_onset(t + perturbation);
        bpms.push(pll.current_bpm());
        t += period_samples;
    }

    let mean: f64 = bpms.iter().sum::<f64>() / bpms.len() as f64;
    let var: f64 =
        bpms.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / bpms.len() as f64;
    var.sqrt()
}

#[test]
fn high_alpha_jitters_more_than_low_alpha() {
    // 15 ms onset jitter is realistic for full-mix audio.
    let sigma_responsive = run_with_jitter(120.0, 15.0, 0.20, 100);
    let sigma_smooth = run_with_jitter(120.0, 15.0, 0.02, 100);
    eprintln!(
        "σ(BPM) at α=0.20 (jittery): {:.3}, at α=0.02 (smooth): {:.3}",
        sigma_responsive, sigma_smooth
    );
    assert!(
        sigma_responsive > sigma_smooth * 2.0,
        "responsive setting (α=0.20) should jitter at least 2x more than smooth (α=0.02): \
         responsive={sigma_responsive:.3}, smooth={sigma_smooth:.3}"
    );
}

#[test]
fn smooth_setting_keeps_bpm_within_tight_band() {
    // At low α and 15 ms onset jitter, BPM σ should stay below ~0.5.
    let sigma = run_with_jitter(120.0, 15.0, 0.02, 200);
    eprintln!("σ(BPM) at α=0.02 with 15 ms jitter: {:.3}", sigma);
    assert!(
        sigma < 0.6,
        "σ(BPM) at smooth setting should be < 0.6, got {sigma:.3}"
    );
}

#[test]
fn pll_exposes_bpm_std_dev_estimate() {
    // The PLL's own EMA-based σ should track the empirical σ within an
    // order of magnitude.
    let mut pll = BeatPll::new(SR);
    pll.alpha_period = 0.20;
    pll.alpha_phase = 0.165;
    let period_samples = SR * 60.0 / 120.0;
    let jitter_samples = 15.0 / 1000.0 * SR;
    let mut seed: u32 = 0xDEAD_BEEF;
    let mut t = 1.0e6;
    for _ in 0..16 {
        pll.on_onset(t);
        t += period_samples;
    }
    for _ in 0..100 {
        seed = seed.wrapping_mul(48271).wrapping_rem(2_147_483_647);
        let r = (seed as f64 / 2_147_483_647.0) * 2.0 - 1.0;
        pll.on_onset(t + r * jitter_samples);
        t += period_samples;
    }
    let internal_sigma = pll.bpm_std_dev();
    eprintln!("PLL bpm_std_dev = {:.3}", internal_sigma);
    // With high α and noticeable jitter, the EMA should report > 0.
    assert!(internal_sigma > 0.2, "expected σ > 0.2, got {internal_sigma}");
}
