// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Experiment: replace BeatPulse's `Onset + custom PLL` with aubio's
//! built-in `Tempo` object (autocorrelation-based beat tracking).
//! Scores the resulting beat times against Ballroom annotations.
//!
//! Hypothesis: the literature's 0.75-0.85 F-measure on Ballroom uses
//! aubio's `Tempo` (or madmom). Our naive period-smoothing PLL is
//! suboptimal — Consensus mode partly compensates (0.288 → 0.436) by
//! using median-IOI period selection, but it's still simpler than what
//! `Tempo` does internally.
//!
//! Run: `BEATPULSE_BALLROOM_DIR=... cargo test --release --features dataset-tests --test aubio_tempo_experiment -- --nocapture`

#![cfg(feature = "dataset-tests")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use aubio_rs::{OnsetMode, Tempo};
use beatpulse::eval::{
    amlt, f_measure, tempo_accuracy_2, trim_beats, F_MEASURE_TOL, TEMPO_ACC_TOL,
};

mod common;
use common::audio::decode_mono_44k1;
use common::datasets::{is_ballroom_duplicate, TRIM_BEATS_MIN_T};

const TARGET_SR: u32 = 44_100;
const BUF_SIZE: usize = 1024;
const HOP_SIZE: usize = 512;

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

/// Run aubio's Tempo over `audio`, return per-beat times in seconds.
fn run_tempo(audio: &[f32], method: OnsetMode) -> Vec<f64> {
    let mut tempo = Tempo::new(method, BUF_SIZE, HOP_SIZE, TARGET_SR).expect("Tempo::new");
    let mut beats: Vec<f64> = Vec::new();
    let mut frame = 0usize;
    for hop in audio.chunks_exact(HOP_SIZE) {
        if let Ok(out) = tempo.do_result(hop) {
            if out > 0.0 {
                let abs = tempo.get_last();
                beats.push(abs as f64 / TARGET_SR as f64);
            }
        }
        frame += HOP_SIZE;
        let _ = frame;
    }
    beats
}

#[test]
fn aubio_tempo_ballroom() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!("BEATPULSE_BALLROOM_DIR not set — skipping");
        return;
    };
    let wavs = walk_wavs(&PathBuf::from(dir));
    if wavs.is_empty() {
        panic!("no .wav under BEATPULSE_BALLROOM_DIR");
    }

    eprintln!("[tempo] pre-decoding {} tracks…", wavs.len());
    let mut bench: Vec<(Vec<f32>, Vec<f64>)> = Vec::with_capacity(wavs.len());
    for wav in &wavs {
        let stem = wav.file_stem().unwrap().to_string_lossy().into_owned();
        if is_ballroom_duplicate(&stem) {
            continue;
        }
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
    eprintln!("[tempo] {} tracks ready", bench.len());

    let methods = [
        ("HFC", OnsetMode::Hfc),
        ("Complex", OnsetMode::Complex),
        ("SpecDiff", OnsetMode::SpecDiff),
        ("KL", OnsetMode::Kl),
        ("MKL", OnsetMode::Mkl),
        ("Phase", OnsetMode::Phase),
        ("SpecFlux", OnsetMode::SpecFlux),
    ];

    println!(
        "\n--- Aubio Tempo full sweep on Ballroom (n={}, trim_beats(5.0), dups skipped) ---",
        bench.len()
    );
    println!("{:<10}  {:>6}  {:>6}  {:>6}", "method", "F", "AMLt", "TA2");
    for (name, method) in &methods {
        let mut sum_f = 0.0f64;
        let mut sum_a = 0.0f64;
        let mut ta2 = 0usize;
        for (audio, truth) in &bench {
            let est = run_tempo(audio, *method);
            let r_t = trim_beats(truth, TRIM_BEATS_MIN_T);
            let e_t = trim_beats(&est, TRIM_BEATS_MIN_T);
            sum_f += f_measure(&r_t, &e_t, F_MEASURE_TOL);
            sum_a += amlt(&r_t, &e_t);
            if tempo_accuracy_2(&r_t, &e_t, TEMPO_ACC_TOL) {
                ta2 += 1;
            }
        }
        let f_mean = sum_f / bench.len() as f64;
        let a_mean = sum_a / bench.len() as f64;
        let ta2_rate = ta2 as f64 / bench.len() as f64;
        println!("{name:<10}  {f_mean:>6.3}  {a_mean:>6.3}  {ta2_rate:>6.3}");
    }
}
