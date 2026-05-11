// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Standalone Klapuri benchmark on Ballroom — the go/no-go gate for
//! integration into BeatPulse's `BeatSource`. See the plan file at
//! `~/.claude/plans/plan-this-project-i-glowing-lark.md`, Phase 5.
//!
//! Mirrors `tests/aubio_tempo_experiment.rs` exactly so the F-measure
//! is directly comparable.
//!
//! Run: `BEATPULSE_BALLROOM_DIR=... cargo test --release --features dataset-tests --test klapuri_experiment -- --nocapture`

#![cfg(feature = "dataset-tests")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use beatpulse::dsp::klapuri::KlapuriTracker;
use beatpulse::eval::Scoring;

mod common;
use common::audio::decode_mono_44k1;
use common::datasets::is_ballroom_duplicate;

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

/// Stream `audio` through `KlapuriTracker`, collect beat times in
/// seconds, return them.
fn run_klapuri(audio: &[f32]) -> Vec<f64> {
    let mut tracker = KlapuriTracker::new(TARGET_SR);
    let mut beats: Vec<f64> = Vec::new();
    let block = 512;
    let mut absolute = 0u64;
    for chunk in audio.chunks(block) {
        tracker.process_block(chunk, absolute, |_off, abs| {
            beats.push(abs as f64 / TARGET_SR as f64);
        });
        absolute += chunk.len() as u64;
    }
    beats
}

#[test]
fn klapuri_ballroom() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!("BEATPULSE_BALLROOM_DIR not set — skipping");
        return;
    };
    let wavs = walk_wavs(&PathBuf::from(dir));
    if wavs.is_empty() {
        panic!("no .wav under BEATPULSE_BALLROOM_DIR");
    }

    eprintln!("[klapuri] pre-decoding {} tracks…", wavs.len());
    let mut bench: Vec<(Vec<f32>, Vec<f64>, String)> = Vec::with_capacity(wavs.len());
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
        bench.push((audio, truth, stem));
    }
    eprintln!("[klapuri] {} tracks ready", bench.len());

    println!(
        "\n--- Klapuri 2006 on Ballroom (n={}, trim_beats(5.0), dups skipped) ---",
        bench.len()
    );

    let mut sum_f = 0.0f64;
    let mut sum_a = 0.0f64;
    let mut ta1 = 0usize;
    let mut ta2 = 0usize;
    let n = bench.len();
    for (audio, truth, _stem) in &bench {
        let est = run_klapuri(audio);
        let s = Scoring::standard(truth, &est);
        sum_f += s.f_measure;
        sum_a += s.amlt;
        if s.tempo_acc_1 {
            ta1 += 1;
        }
        if s.tempo_acc_2 {
            ta2 += 1;
        }
    }
    let f_mean = sum_f / n as f64;
    let a_mean = sum_a / n as f64;
    let ta1_rate = ta1 as f64 / n as f64;
    let ta2_rate = ta2 as f64 / n as f64;
    println!("Klapuri:      F={f_mean:.3}  AMLt={a_mean:.3}  TA1={ta1_rate:.3}  TA2={ta2_rate:.3}");
    println!("AubioTempo:   F=0.590  AMLt=0.459  TA1=0.592  TA2=0.777  (reference)");
    println!("Verdict: ΔF(klapuri − aubio) = {:+.3}", f_mean - 0.590);
}
