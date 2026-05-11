// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Threshold + onset-method sweep on full Ballroom (n=698).
//!
//! Per the diagnostic, the dominant failure mode is "wrong tempo lock"
//! on 71 % of tracks. The leading hypothesis is that threshold=0.3 is
//! suppressing too many onsets, leaving the PLL with too sparse / too
//! noisy a signal to lock to the right period. This sweep tries
//! threshold ∈ {0.0, 0.05, 0.1, 0.15, 0.2, 0.3} for SpecFlux (current
//! default) and KL (Jive audit winner), Reactive mode only, full
//! corpus each. Prints a table.
//!
//! Run with:
//! `BEATPULSE_BALLROOM_DIR=... cargo test --features dataset-tests --test threshold_sweep -- --nocapture --test-threads=1`

#![cfg(feature = "dataset-tests")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use beatpulse::eval::{f_measure, F_MEASURE_TOL};
use beatpulse::params::OnsetMethod;

mod common;
use common::audio::decode_mono_44k1;
use common::Mode;

const TARGET_SR: u32 = 44_100;

fn walk_wavs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk_wavs(&p));
        } else if p.extension().and_then(|s| s.to_str()) == Some("wav") {
            out.push(p);
        }
    }
    out
}

fn load_beats(path: &Path) -> Option<Vec<f64>> {
    let s = fs::read_to_string(path).ok()?;
    let mut beats: Vec<f64> = s
        .lines()
        .filter_map(|l| l.split_whitespace().next().and_then(|t| t.parse().ok()))
        .collect();
    beats.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(beats)
}

#[test]
fn threshold_sweep_ballroom() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!("BEATPULSE_BALLROOM_DIR not set — skipping");
        return;
    };
    let wavs = walk_wavs(&PathBuf::from(dir));
    if wavs.is_empty() {
        panic!("no .wav under BEATPULSE_BALLROOM_DIR");
    }

    // Pre-decode each track once and pair with its truth — saves ~30s
    // per sweep iteration (audio decode dominates wall time).
    eprintln!("[sweep] pre-decoding {} tracks…", wavs.len());
    let mut bench: Vec<(Vec<f32>, Vec<f64>)> = Vec::with_capacity(wavs.len());
    for wav in &wavs {
        let beats_path = wav.with_extension("beats");
        if !beats_path.exists() {
            continue;
        }
        let Some(audio) = decode_mono_44k1(wav) else {
            continue;
        };
        let Some(truth) = load_beats(&beats_path) else {
            continue;
        };
        bench.push((audio, truth));
    }
    eprintln!("[sweep] {} tracks ready", bench.len());

    let methods = [
        ("SpecFlux", OnsetMethod::SpecFlux),
        ("KL", OnsetMethod::Kl),
        ("HFC", OnsetMethod::Hfc),
    ];
    let thresholds = [0.0_f32, 0.05, 0.1, 0.15, 0.2, 0.3];

    println!(
        "\n--- Threshold sweep on full Ballroom (n={}, Reactive) ---",
        bench.len()
    );
    println!("{:<10}  {:>6}  {:>6}", "method", "thresh", "F_mean");
    for (name, method) in &methods {
        for &th in &thresholds {
            let mut sum_f = 0.0f64;
            for (audio, truth) in &bench {
                let est = common::run_pipeline_with(audio, TARGET_SR, Mode::Reactive, *method, th);
                sum_f += f_measure(truth, &est, F_MEASURE_TOL);
            }
            let f_mean = sum_f / bench.len() as f64;
            println!("{name:<10}  {th:>6.2}  {f_mean:>6.3}");
        }
    }
}
