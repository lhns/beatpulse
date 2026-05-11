// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Failure-mode diagnostic for the Ballroom dataset.
//!
//! Runs `common::run_pipeline(Reactive)` on every Ballroom track and
//! emits per-track classification + corpus summary so we can see *what*
//! is broken before tuning. Read-only — does not change any pipeline
//! parameters.
//!
//! Run with:
//! `BEATPULSE_BALLROOM_DIR=... cargo test --features dataset-tests --test ballroom_diagnostic -- --nocapture`

#![cfg(feature = "dataset-tests")]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use beatpulse::eval::{f_measure, tempo_from_beats, F_MEASURE_TOL};

mod common;
use common::audio::decode_mono_44k1;
use common::{run_pipeline, Mode};

const TARGET_SR: u32 = 44_100;

#[derive(Debug, Clone, Copy)]
enum OctaveClass {
    Correct,
    Half,
    Double,
    Third,
    Triple,
    Wrong,
}

impl OctaveClass {
    fn name(self) -> &'static str {
        match self {
            OctaveClass::Correct => "correct",
            OctaveClass::Half => "half",
            OctaveClass::Double => "double",
            OctaveClass::Third => "third",
            OctaveClass::Triple => "triple",
            OctaveClass::Wrong => "wrong",
        }
    }
}

fn classify_octave(est: f64, truth: f64) -> OctaveClass {
    if truth <= 0.0 || est <= 0.0 {
        return OctaveClass::Wrong;
    }
    let tol = 0.04;
    let candidates = [
        (truth, OctaveClass::Correct),
        (truth * 2.0, OctaveClass::Double),
        (truth * 0.5, OctaveClass::Half),
        (truth * 3.0, OctaveClass::Triple),
        (truth / 3.0, OctaveClass::Third),
    ];
    for (target, cls) in candidates {
        if (est - target).abs() / target < tol {
            return cls;
        }
    }
    OctaveClass::Wrong
}

/// Median signed offset (predicted - nearest-truth) over the middle
/// 60 % of `predicted`, restricted to predictions that have a truth
/// match within ±200 ms (to avoid pulling drift into the median).
fn median_phase_offset_ms(predicted: &[f64], truth: &[f64]) -> Option<f64> {
    if predicted.is_empty() || truth.is_empty() {
        return None;
    }
    // Middle 60% slice.
    let lo = predicted.len() / 5;
    let hi = predicted.len() - predicted.len() / 5;
    let slice = &predicted[lo..hi.max(lo + 1)];
    let mut offsets: Vec<f64> = Vec::new();
    for &p in slice {
        // Find nearest truth via binary search.
        let pos = truth.partition_point(|&t| t < p);
        let cand_a = if pos > 0 { Some(truth[pos - 1]) } else { None };
        let cand_b = truth.get(pos).copied();
        let nearest = match (cand_a, cand_b) {
            (Some(a), Some(b)) => {
                if (p - a).abs() < (b - p).abs() {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => continue,
        };
        let off = (p - nearest) * 1000.0; // ms
        if off.abs() < 200.0 {
            offsets.push(off);
        }
    }
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(offsets[offsets.len() / 2])
}

fn precision_recall(predicted: &[f64], truth: &[f64], tol: f64) -> (f64, f64) {
    if predicted.is_empty() || truth.is_empty() {
        return (0.0, 0.0);
    }
    // Greedy match (mirrors src/eval.rs::match_beats inline).
    let mut used = vec![false; predicted.len()];
    let mut tp = 0usize;
    let mut j = 0usize;
    for &r in truth {
        while j < predicted.len() && predicted[j] < r - tol {
            j += 1;
        }
        let mut best: Option<(usize, f64)> = None;
        let mut k = j;
        while k < predicted.len() && predicted[k] <= r + tol {
            if !used[k] {
                let d = (predicted[k] - r).abs();
                if best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((k, d));
                }
            }
            k += 1;
        }
        if let Some((idx, _)) = best {
            used[idx] = true;
            tp += 1;
        }
    }
    let fp = predicted.len() - tp;
    let fnv = truth.len() - tp;
    let p = if tp + fp == 0 {
        0.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let r = if tp + fnv == 0 {
        0.0
    } else {
        tp as f64 / (tp + fnv) as f64
    };
    (p, r)
}

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
fn ballroom_failure_diagnostic() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!("BEATPULSE_BALLROOM_DIR not set — skipping");
        return;
    };
    let wavs = walk_wavs(&PathBuf::from(dir));
    if wavs.is_empty() {
        panic!("no .wav under BEATPULSE_BALLROOM_DIR");
    }

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    let _ = fs::create_dir_all(&out_dir);
    let csv_path = out_dir.join("ballroom-diagnostic.csv");
    let mut csv = fs::File::create(&csv_path).expect("create csv");

    use std::io::Write;
    writeln!(
        csv,
        "track,true_bpm,est_bpm,octave_class,phase_offset_ms,precision,recall,f_measure,n_truth,n_est"
    )
    .unwrap();

    let mut class_counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut all_offsets: Vec<f64> = Vec::new();
    let mut sum_p = 0.0f64;
    let mut sum_r = 0.0f64;
    let mut sum_f = 0.0f64;
    let mut n_eval = 0usize;
    let mut tracks_with_offset = 0usize;

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
        let est = run_pipeline(&audio, TARGET_SR, Mode::Reactive);

        let true_bpm = tempo_from_beats(&truth);
        let est_bpm = tempo_from_beats(&est);
        let cls = classify_octave(est_bpm, true_bpm);
        let phase = median_phase_offset_ms(&est, &truth);
        let (p, r) = precision_recall(&est, &truth, F_MEASURE_TOL);
        let f = f_measure(&truth, &est, F_MEASURE_TOL);

        writeln!(
            csv,
            "{},{:.2},{:.2},{},{},{:.3},{:.3},{:.3},{},{}",
            wav.file_stem().unwrap().to_string_lossy(),
            true_bpm,
            est_bpm,
            cls.name(),
            phase
                .map(|x| format!("{x:.1}"))
                .unwrap_or_else(|| "nan".to_string()),
            p,
            r,
            f,
            truth.len(),
            est.len(),
        )
        .unwrap();

        *class_counts.entry(cls.name()).or_insert(0) += 1;
        if let Some(off) = phase {
            all_offsets.push(off);
            tracks_with_offset += 1;
        }
        sum_p += p;
        sum_r += r;
        sum_f += f;
        n_eval += 1;
    }

    if n_eval == 0 {
        panic!("no tracks evaluated");
    }
    all_offsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_offset = if all_offsets.is_empty() {
        f64::NAN
    } else {
        all_offsets[all_offsets.len() / 2]
    };
    let mean_offset = if all_offsets.is_empty() {
        f64::NAN
    } else {
        all_offsets.iter().sum::<f64>() / all_offsets.len() as f64
    };
    let stdev_offset = if all_offsets.len() < 2 {
        f64::NAN
    } else {
        let var = all_offsets
            .iter()
            .map(|x| (x - mean_offset).powi(2))
            .sum::<f64>()
            / all_offsets.len() as f64;
        var.sqrt()
    };

    println!("\n--- Ballroom diagnostic (n={n_eval}) ---");
    println!(
        "F={:.3}  P={:.3}  R={:.3}",
        sum_f / n_eval as f64,
        sum_p / n_eval as f64,
        sum_r / n_eval as f64,
    );
    println!("Octave class:");
    for (name, count) in &class_counts {
        println!(
            "  {name:<8}  {count:>4}  ({:.1}%)",
            *count as f64 / n_eval as f64 * 100.0
        );
    }
    println!(
        "Phase offset (predicted-truth, ms) on {tracks_with_offset} tracks: \
         median={median_offset:.2}  mean={mean_offset:.2}  stdev={stdev_offset:.2}"
    );
    println!("CSV: {}", csv_path.display());
}
