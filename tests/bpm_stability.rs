// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! BPM-stability contract: the new `tempo_stability` parameter must
//! actually trade off responsiveness for smoothness. See ADR-0024.

use beatpulse::dsp::beat_pll::BeatPll;

mod common;
use common::{run_pipeline_with_bpm_log, Mode};

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
    let var: f64 = bpms.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / bpms.len() as f64;
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
    assert!(
        internal_sigma > 0.2,
        "expected σ > 0.2, got {internal_sigma}"
    );
}

/// Synthesize 60 s of 120 BPM kick clicks plus 30 % spurious offbeat
/// clicks, then run the full audio → BeatTracker → (PLL | Consensus)
/// pipeline in both tracking modes. Reports two metrics per mode:
///
/// - **σ(BPM):** sample-to-sample BPM variance after warmup. Surprising
///   finding: σ is *not* a good proxy for "steadier display". Reactive
///   reports a low σ even when locked on the wrong tempo (it drifts
///   smoothly), and consensus reports a high σ because each commit is a
///   discrete period snap. Recorded for transparency, not asserted.
/// - **BPM accuracy:** fraction of post-warmup beats where the reported
///   BPM is within ±5 % of the true 120 BPM (octave-tolerant). This is
///   the actual user-facing "right tempo" metric. Consensus wins here.
#[test]
fn consensus_more_accurate_bpm_than_reactive_on_noisy_input() {
    const BPM: f64 = 120.0;
    const DURATION_S: f64 = 60.0;
    const WARMUP_S: f64 = 8.0;
    let total = (DURATION_S * SR) as usize;
    let beat_period = SR * 60.0 / BPM;

    let mut click_samples: Vec<usize> = Vec::new();
    let mut t = 0.5 * SR;
    while (t as usize) + 1 < total {
        click_samples.push(t as usize);
        t += beat_period;
    }
    let mut spur: Vec<usize> = Vec::new();
    let mut seed: u32 = 0xC0FFEE;
    let n_spur = (click_samples.len() as f32 * 0.30) as usize;
    for k in 0..n_spur {
        let beat_idx = (k * 7 + 3) % click_samples.len();
        let half = click_samples[beat_idx] as f64 + 0.5 * beat_period;
        seed = seed.wrapping_mul(48271) % 2_147_483_647;
        let r = (seed as f64 / 2_147_483_647.0) * 2.0 - 1.0;
        let p = (half + r * 0.200 * SR) as usize;
        if p < total {
            spur.push(p);
        }
    }
    let mut all = click_samples.clone();
    all.extend(spur);
    all.sort_unstable();
    let signal = make_jitter_clicks(total, &all);

    let (beats_r, bpm_r) = run_pipeline_with_bpm_log(&signal, SR as u32, Mode::Reactive);
    let (beats_c, bpm_c) = run_pipeline_with_bpm_log(
        &signal,
        SR as u32,
        Mode::Consensus {
            lookahead_ms: 2000.0,
        },
    );

    let sigma_r = sigma_after_warmup(&beats_r, &bpm_r, WARMUP_S);
    let sigma_c = sigma_after_warmup(&beats_c, &bpm_c, WARMUP_S);
    let acc_r = bpm_accuracy_after_warmup(&beats_r, &bpm_r, WARMUP_S, BPM, 0.05);
    let acc_c = bpm_accuracy_after_warmup(&beats_c, &bpm_c, WARMUP_S, BPM, 0.05);
    eprintln!(
        "noisy input — σ(BPM) reactive={sigma_r:.3} consensus={sigma_c:.3} | \
         accuracy@5% reactive={acc_r:.3} consensus={acc_c:.3}"
    );

    assert!(
        acc_c > acc_r + 0.10,
        "expected consensus to spend > +10pp more time at the correct BPM than \
         reactive on noisy input; got reactive={acc_r:.3} consensus={acc_c:.3}"
    );
}

/// % of post-warmup BPM samples within `tol_pct` of `true_bpm` or its
/// octaves (×2, ×0.5).
fn bpm_accuracy_after_warmup(
    beats: &[f64],
    bpms: &[f64],
    warmup_s: f64,
    true_bpm: f64,
    tol_pct: f64,
) -> f64 {
    let mut n = 0usize;
    let mut hits = 0usize;
    for (t, b) in beats.iter().zip(bpms.iter()) {
        if *t <= warmup_s {
            continue;
        }
        n += 1;
        let candidates = [true_bpm, true_bpm * 2.0, true_bpm * 0.5];
        if candidates.iter().any(|c| (b - c).abs() / c < tol_pct) {
            hits += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        hits as f64 / n as f64
    }
}

fn make_jitter_clicks(total: usize, click_samples: &[usize]) -> Vec<f32> {
    let mut buf = vec![0.0f32; total];
    let click_len = (0.050 * SR as f32) as usize;
    let decay_tau = 0.020 * SR as f32;
    for &t in click_samples {
        for i in 0..click_len {
            let pos = t + i;
            if pos < total {
                let phase = 2.0 * std::f32::consts::PI * 60.0 * (i as f32) / SR as f32;
                let env = (-(i as f32) / decay_tau).exp();
                buf[pos] += 0.8 * env * phase.sin();
            }
        }
    }
    buf
}

fn sigma_after_warmup(beats: &[f64], bpms: &[f64], warmup_s: f64) -> f64 {
    let samples: Vec<f64> = beats
        .iter()
        .zip(bpms.iter())
        .filter(|(t, _)| **t > warmup_s)
        .map(|(_, b)| *b)
        .collect();
    assert!(
        samples.len() > 10,
        "not enough post-warmup BPM samples ({}); the PLL probably never locked",
        samples.len()
    );
    let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
    let var: f64 = samples.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    var.sqrt()
}
