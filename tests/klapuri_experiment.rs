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
    // Per-track AMLt-failure-mode classification (Phase A, plan
    // pass 11). For each track, compute the per-beat signed offset
    // to nearest truth beat, then classify the trajectory shape.
    let mut tally_clean = 0usize;
    let mut tally_drift = 0usize;
    let mut tally_jumps = 0usize;
    let mut tally_oscill = 0usize;
    let mut tally_mixed = 0usize;
    let mut drift_examples: Vec<(String, f64, Vec<f64>)> = Vec::new();
    let mut jumps_examples: Vec<(String, Vec<f64>)> = Vec::new();
    let mut oscill_examples: Vec<(String, Vec<f64>)> = Vec::new();
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

        // Per-beat error trajectory + classification. Only meaningful
        // on tracks where we got the tempo roughly right (TA2) AND
        // AMLt actually failed (< 0.5 = the lock didn't hold) — that
        // narrows to the population we're trying to understand.
        if !s.tempo_acc_2 || s.amlt > 0.5 || est.len() < 12 || truth.len() < 12 {
            continue;
        }
        let truth_sorted: Vec<f64> = {
            let mut t = truth.clone();
            t.sort_by(|a, b| a.partial_cmp(b).unwrap());
            t
        };
        let truth_first = *truth_sorted.first().unwrap();
        let truth_last = *truth_sorted.last().unwrap();
        let nearest = |p: f64| -> f64 {
            let i = truth_sorted
                .partition_point(|&t| t < p)
                .min(truth_sorted.len() - 1);
            let lo = if i > 0 {
                truth_sorted[i - 1]
            } else {
                truth_sorted[i]
            };
            let hi = truth_sorted[i];
            if (p - lo).abs() < (hi - p).abs() {
                lo
            } else {
                hi
            }
        };
        // Filter est to within [truth_first, truth_last] — beats
        // outside the truth range get spurious huge offsets when
        // matched to the closest truth endpoint, which is a
        // measurement artifact, not a real lock-loss.
        let est_in_range: Vec<f64> = est
            .iter()
            .copied()
            .filter(|&p| p >= truth_first && p <= truth_last)
            .collect();
        if est_in_range.len() < 12 {
            continue;
        }
        // Signed offsets in ms.
        let errs_ms: Vec<f64> = est_in_range
            .iter()
            .map(|&p| (p - nearest(p)) * 1000.0)
            .collect();
        let max_abs = errs_ms.iter().fold(0.0f64, |m, &e| m.max(e.abs()));
        if max_abs < 35.0 {
            tally_clean += 1;
            continue;
        }
        // Drop the first 5 beats (post-warmup). Then classify.
        let post: Vec<f64> = errs_ms.iter().skip(5).copied().collect();
        if post.len() < 8 {
            tally_clean += 1;
            continue;
        }
        // Linear-drift score: |slope| of best-fit line in ms/beat.
        let m = post.len() as f64;
        let xbar = (m - 1.0) / 2.0;
        let ybar = post.iter().sum::<f64>() / m;
        let mut num = 0.0;
        let mut den = 0.0;
        for (i, &y) in post.iter().enumerate() {
            let dx = i as f64 - xbar;
            num += dx * (y - ybar);
            den += dx * dx;
        }
        let slope = if den > 0.0 { num / den } else { 0.0 };
        let predicted: Vec<f64> = (0..post.len())
            .map(|i| ybar + slope * (i as f64 - xbar))
            .collect();
        let resid: Vec<f64> = post.iter().zip(&predicted).map(|(&a, &b)| a - b).collect();
        let resid_var = resid.iter().map(|&r| r * r).sum::<f64>() / m;
        let resid_std = resid_var.sqrt();
        // Step-jump score: max single-step delta in errs.
        let max_step = post
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f64, f64::max);
        // Oscillation: count sign changes in the differential.
        let mut sign_changes = 0usize;
        for i in 2..post.len() {
            let d1 = post[i - 1] - post[i - 2];
            let d2 = post[i] - post[i - 1];
            if d1 * d2 < 0.0 && d1.abs() > 5.0 && d2.abs() > 5.0 {
                sign_changes += 1;
            }
        }
        let osc_rate = sign_changes as f64 / post.len() as f64;
        // Classification thresholds.
        let total_drift = (slope * post.len() as f64).abs();
        let is_drift = total_drift > 40.0 && resid_std < 0.5 * total_drift;
        let is_jumps = max_step > 30.0 && resid_std > 15.0;
        let is_oscill = osc_rate > 0.3 && total_drift < 40.0;
        match (is_drift, is_jumps, is_oscill) {
            (true, false, false) => {
                tally_drift += 1;
                if drift_examples.len() < 5 {
                    drift_examples.push((stem.clone(), slope, errs_ms.clone()));
                }
            }
            (false, true, false) => {
                tally_jumps += 1;
                if jumps_examples.len() < 5 {
                    jumps_examples.push((stem.clone(), errs_ms.clone()));
                }
            }
            (false, false, true) => {
                tally_oscill += 1;
                if oscill_examples.len() < 5 {
                    oscill_examples.push((stem.clone(), errs_ms.clone()));
                }
            }
            _ => {
                tally_mixed += 1;
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
    println!(
        "\n  AMLt-failure-mode tally (TA2-correct tracks only, post-warmup): \
         clean={tally_clean} drift={tally_drift} jumps={tally_jumps} \
         oscill={tally_oscill} mixed={tally_mixed}"
    );
    let print_traj = |label: &str, examples: &[(String, Vec<f64>)]| {
        if examples.is_empty() {
            return;
        }
        println!("  {label} examples:");
        for (name, errs) in examples {
            let head: Vec<String> = errs.iter().take(10).map(|e| format!("{e:+.1}")).collect();
            let tail: Vec<String> = if errs.len() > 20 {
                errs.iter()
                    .rev()
                    .take(10)
                    .map(|e| format!("{e:+.1}"))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            } else {
                Vec::new()
            };
            println!("    {name} ({} beats):", errs.len());
            println!("      head: {}", head.join(" "));
            if !tail.is_empty() {
                println!("      tail: {}", tail.join(" "));
            }
        }
    };
    if !drift_examples.is_empty() {
        println!("  drift examples (slope ms/beat shown):");
        for (name, slope, errs) in &drift_examples {
            let head: Vec<String> = errs.iter().take(10).map(|e| format!("{e:+.1}")).collect();
            let tail: Vec<String> = if errs.len() > 20 {
                errs.iter()
                    .rev()
                    .take(10)
                    .map(|e| format!("{e:+.1}"))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            } else {
                Vec::new()
            };
            println!(
                "    {name} ({} beats, slope={slope:+.2} ms/beat):",
                errs.len()
            );
            println!("      head: {}", head.join(" "));
            if !tail.is_empty() {
                println!("      tail: {}", tail.join(" "));
            }
        }
    }
    print_traj("jumps", &jumps_examples);
    print_traj("oscill", &oscill_examples);
}
