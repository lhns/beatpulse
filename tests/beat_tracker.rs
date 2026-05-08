// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! `BeatTracker` integration tests. See `docs/TESTING.md` §3.5.

use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::params::OnsetMethod;

const SR: u32 = 44_100;

/// Generate a "click" signal: kick-drum-like bursts (60 Hz sine with
/// exponential decay, ~50 ms long) at the given times.
fn make_clicks(total_samples: usize, click_samples: &[usize]) -> Vec<f32> {
    let mut buf = vec![0.0f32; total_samples];
    let click_len = (0.050 * SR as f32) as usize; // ~50 ms
    let decay_tau = (0.020 * SR as f32) as f32; // 20 ms decay
    for &t in click_samples {
        for i in 0..click_len {
            let pos = t + i;
            if pos < total_samples {
                let phase = 2.0 * std::f32::consts::PI * 60.0 * (i as f32) / SR as f32;
                let env = (-(i as f32) / decay_tau).exp();
                buf[pos] += 0.8 * env * phase.sin();
            }
        }
    }
    buf
}

/// B1: hop accumulation across host block sizes 32..=2048.
#[test]
fn b1_hop_accumulation_across_block_sizes() {
    let click_at = [10_000usize, 30_000, 50_000, 70_000, 90_000];
    let n = 110_000;
    for &block_size in &[32usize, 64, 128, 256, 512, 1024, 2048] {
        let mut tracker = BeatTracker::new(SR, OnsetMethod::SpecFlux, 0.3).unwrap();
        let signal = make_clicks(n, &click_at);
        let mut detected = Vec::new();
        let mut absolute_sample = 0u64;
        for chunk in signal.chunks(block_size) {
            tracker.process_block(chunk, |offset, _frac| {
                detected.push(absolute_sample + offset as u64);
            });
            absolute_sample += chunk.len() as u64;
        }
        // We should detect roughly the right number of onsets — aubio may
        // miss the first one (warm-up) or produce a couple of false
        // positives, but should be in the right neighbourhood.
        assert!(
            detected.len() >= click_at.len() - 1 && detected.len() <= click_at.len() + 2,
            "block_size={block_size}: detected {} onsets, expected ~{}",
            detected.len(),
            click_at.len()
        );
    }
}

/// B3 + B4: mono input passes through; stereo gets downmixed.
#[test]
fn b3_b4_mono_and_stereo() {
    let n = 50_000;
    let click_at = [10_000usize, 25_000, 40_000];
    let mono = make_clicks(n, &click_at);

    let mut tracker = BeatTracker::new(SR, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut mono_count = 0;
    tracker.process_block(&mono, |_, _| mono_count += 1);

    // Stereo: same content in both channels — average should equal mono.
    let mut tracker2 = BeatTracker::new(SR, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut scratch = vec![0.0f32; 4096];
    let mut stereo_count = 0;
    let chunk = 1024usize;
    for offset in (0..n).step_by(chunk) {
        let end = (offset + chunk).min(n);
        let l = &mono[offset..end];
        let r = &mono[offset..end];
        tracker2.process_block_multichannel(&[l, r], &mut scratch, |_, _| stereo_count += 1);
    }
    assert_eq!(mono_count, stereo_count);
}

/// B5: sample-rate change re-init (just construct fresh).
#[test]
fn b5_sample_rate_reinit() {
    for &sr in &[44_100u32, 48_000, 96_000] {
        let _t = BeatTracker::new(sr, OnsetMethod::SpecFlux, 0.3).unwrap();
    }
}

/// B7: all seven OnsetMode enum variants construct successfully.
#[test]
fn b7_all_methods_constructable() {
    use OnsetMethod::*;
    for &m in &[Hfc, Complex, SpecDiff, Kl, Mkl, Phase, SpecFlux] {
        let _t = BeatTracker::new(SR, m, 0.3).unwrap();
    }
}

/// Method change recreates the underlying detector without leaking.
#[test]
fn method_change() {
    let mut t = BeatTracker::new(SR, OnsetMethod::SpecFlux, 0.3).unwrap();
    t.set_method(OnsetMethod::Hfc).unwrap();
    t.set_method(OnsetMethod::Complex).unwrap();
}

/// P5: onset *positions* (not just counts) must land within a tight window
/// of the synthetic click positions. Tolerates aubio's analysis delay
/// (~hop-size) plus a generous slack for the kick envelope's energy peak
/// not coinciding with sample 0 of the click.
#[test]
fn p5_onset_position_accuracy() {
    let click_at = [10_000usize, 30_000, 50_000, 70_000, 90_000];
    let n = 110_000;
    let signal = make_clicks(n, &click_at);

    let mut tracker = BeatTracker::new(SR, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut detected_abs = Vec::new();
    let block = 512;
    let mut absolute = 0u64;
    for chunk in signal.chunks(block) {
        let abs_at_block = absolute;
        tracker.process_block(chunk, |offset, _frac| {
            detected_abs.push(abs_at_block + offset as u64);
        });
        absolute += chunk.len() as u64;
    }

    // For each click, find the closest detected onset and assert it's
    // within ±50 ms (≈2200 samples at 44.1 kHz).
    let tol_samples: i64 = (0.050 * SR as f64) as i64;
    let mut matched = 0usize;
    for &c in &click_at {
        let closest = detected_abs
            .iter()
            .map(|&d| (d as i64 - c as i64).abs())
            .min()
            .unwrap_or(i64::MAX);
        if closest <= tol_samples {
            matched += 1;
        }
    }
    // Allow at most one of the five clicks to be missed (aubio sometimes
    // doesn't report the very first onset due to spectral warm-up).
    assert!(
        matched >= click_at.len() - 1,
        "matched {matched}/{} clicks within ±50 ms; detected = {detected_abs:?}",
        click_at.len()
    );
}
