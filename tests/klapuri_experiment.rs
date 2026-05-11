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
    // Per-octave-ratio tally for the residual TA1 gap. Buckets the
    // ratio est_bpm/truth_bpm into common octave/triplet errors.
    let mut tally_correct = 0usize;
    let mut tally_half = 0usize;
    let mut tally_double = 0usize;
    let mut tally_third = 0usize;
    let mut tally_threehalf = 0usize;
    let mut tally_twothird = 0usize;
    let mut tally_other = 0usize;
    let mut other_ratios: Vec<(f64, String, f64, f64)> = Vec::new();
    let n = bench.len();
    for (audio, truth, stem) in &bench {
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
        // Bucket the ratio.
        let truth_bpm = if truth.len() < 2 {
            0.0
        } else {
            let mut iv = Vec::with_capacity(truth.len() - 1);
            for w in truth.windows(2) {
                iv.push(w[1] - w[0]);
            }
            iv.sort_by(|a, b| a.partial_cmp(b).unwrap());
            60.0 / iv[iv.len() / 2]
        };
        let est_bpm = if est.len() < 2 {
            0.0
        } else {
            let mut iv = Vec::with_capacity(est.len() - 1);
            for w in est.windows(2) {
                iv.push(w[1] - w[0]);
            }
            iv.sort_by(|a, b| a.partial_cmp(b).unwrap());
            60.0 / iv[iv.len() / 2]
        };
        if truth_bpm > 0.0 && est_bpm > 0.0 {
            let r = est_bpm / truth_bpm;
            let close = |x: f64, y: f64| (x - y).abs() < 0.04 * y.max(x);
            if close(r, 1.0) {
                tally_correct += 1;
            } else if close(r, 0.5) {
                tally_half += 1;
            } else if close(r, 2.0) {
                tally_double += 1;
            } else if close(r, 1.0 / 3.0) {
                tally_third += 1;
            } else if close(r, 1.5) {
                tally_threehalf += 1;
            } else if close(r, 2.0 / 3.0) {
                tally_twothird += 1;
            } else {
                tally_other += 1;
                if other_ratios.len() < 30 {
                    other_ratios.push((r, stem.clone(), truth_bpm, est_bpm));
                }
            }
        }
    }
    let f_mean = sum_f / n as f64;
    let a_mean = sum_a / n as f64;
    let ta1_rate = ta1 as f64 / n as f64;
    let ta2_rate = ta2 as f64 / n as f64;
    println!("Klapuri:      F={f_mean:.3}  AMLt={a_mean:.3}  TA1={ta1_rate:.3}  TA2={ta2_rate:.3}");
    println!("AubioTempo:   F=0.590  AMLt=0.459  TA1=0.592  TA2=0.777  (reference)");
    println!("Verdict: ΔF(klapuri − aubio) = {:+.3}", f_mean - 0.590);
    println!(
        "  Tempo-ratio tally (est/truth ±4%): correct={tally_correct} half={tally_half} \
         double={tally_double} third={tally_third} 3/2={tally_threehalf} 2/3={tally_twothird} \
         other={tally_other}"
    );
    if !other_ratios.is_empty() {
        println!("  Sample 'other'-ratio mismatches (up to 30):");
        for (r, name, t, e) in &other_ratios {
            println!("    ratio={r:.3}  truth={t:.1} est={e:.1}  {name}");
        }
    }
}
