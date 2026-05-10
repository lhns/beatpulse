// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! One-off audit: does the choice of `OnsetMethod` materially affect
//! BeatPulse's F-measure on Ballroom? Runs all 7 methods at the default
//! threshold against the Jive subset (60 tracks), Reactive mode only.
//! Prints a comparative table; no assertions.
//!
//! Run with `BEATPULSE_BALLROOM_DIR=... cargo test --features dataset-tests
//! --test onset_method_audit -- --nocapture --test-threads=1`.

#![cfg(feature = "dataset-tests")]

use std::env;
use std::path::PathBuf;
use std::{fs, io::Read};

use beatpulse::eval::{f_measure, F_MEASURE_TOL};
use beatpulse::params::OnsetMethod;

mod common;
use common::audio::decode_mono_44k1;

const TARGET_SR: u32 = 44_100;

#[test]
fn onset_method_sweep_jive() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!("BEATPULSE_BALLROOM_DIR not set — skipping");
        return;
    };
    let jive = PathBuf::from(dir).join("BallroomData").join("Jive");
    if !jive.exists() {
        eprintln!("no Jive subdir at {}; skipping", jive.display());
        return;
    }

    let mut wavs: Vec<PathBuf> = fs::read_dir(&jive)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wav"))
        .collect();
    wavs.sort();
    if wavs.is_empty() {
        panic!("no .wav files in {}", jive.display());
    }
    eprintln!("[audit] {} Jive tracks", wavs.len());

    let methods = [
        ("HFC", OnsetMethod::Hfc),
        ("Complex", OnsetMethod::Complex),
        ("SpecDiff", OnsetMethod::SpecDiff),
        ("KL", OnsetMethod::Kl),
        ("MKL", OnsetMethod::Mkl),
        ("Phase", OnsetMethod::Phase),
        ("SpecFlux", OnsetMethod::SpecFlux),
    ];

    println!(
        "\n--- Onset method audit on Jive (n={}, Reactive, default thresh) ---",
        wavs.len()
    );
    println!("{:<10}  {:>6}  {:>6}", "method", "F_mean", "n_eval");
    for (name, method) in &methods {
        let mut sum_f = 0.0f64;
        let mut n_eval = 0usize;
        for wav in &wavs {
            let beats = wav.with_extension("beats");
            if !beats.exists() {
                continue;
            }
            let Some(audio) = decode_mono_44k1(wav) else {
                continue;
            };
            let Some(reference) = load_beats(&beats) else {
                continue;
            };
            let est =
                common::run_pipeline_with(&audio, TARGET_SR, common::Mode::Reactive, *method, 0.3);
            sum_f += f_measure(&reference, &est, F_MEASURE_TOL);
            n_eval += 1;
        }
        let f_mean = if n_eval > 0 {
            sum_f / n_eval as f64
        } else {
            0.0
        };
        println!("{name:<10}  {f_mean:>6.3}  {n_eval:>6}");
    }
}

fn load_beats(path: &std::path::Path) -> Option<Vec<f64>> {
    let mut s = String::new();
    fs::File::open(path).ok()?.read_to_string(&mut s).ok()?;
    let mut beats: Vec<f64> = s
        .lines()
        .filter_map(|l| l.split_whitespace().next().and_then(|t| t.parse().ok()))
        .collect();
    beats.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(beats)
}
