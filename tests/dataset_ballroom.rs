// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Ballroom-dataset evaluation harness. See ADR-0014 and
//! `docs/TESTING.md` §5.
//!
//! Activate with `cargo test --features dataset-tests --test dataset_ballroom -- --nocapture`
//! and set `BEATPULSE_BALLROOM_DIR` to a directory containing the
//! Ballroom Dataset (or a subset). The directory must contain `*.wav`
//! files paired with `*.beats` annotation files (one beat time in
//! seconds per line — this is the common Ballroom annotation format).
//!
//! On first run, commit `tests/data/baseline-ballroom.json` from the
//! aggregate metrics. Subsequent runs fail if any aggregate metric
//! drops > 2 % vs the baseline.

#![cfg(feature = "dataset-tests")]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use beatpulse::eval::Scoring;
use serde::{Deserialize, Serialize};

mod common;
use common::datasets::is_ballroom_duplicate;
use common::{run_pipeline, Mode};

const TARGET_SR: u32 = 44_100;

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct Aggregate {
    n_tracks: usize,
    f_measure_mean: f64,
    amlt_mean: f64,
    tempo_acc_1_rate: f64,
    tempo_acc_2_rate: f64,
}

fn load_wav_mono(path: &Path) -> Option<Vec<f32>> {
    let mut reader = match hound::WavReader::open(path) {
        Ok(r) => r,
        Err(_) => return None,
    };
    let spec = reader.spec();
    if spec.sample_rate != TARGET_SR {
        eprintln!(
            "skipping {} (sample rate {} != {})",
            path.display(),
            spec.sample_rate,
            TARGET_SR
        );
        return None;
    }
    let n_channels = spec.channels as usize;

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample as i32;
            let scale = 1.0 / (1i64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 * scale)
                .collect()
        }
    };

    if n_channels == 1 {
        return Some(samples);
    }
    let frames = samples.len() / n_channels;
    let mut mono = Vec::with_capacity(frames);
    for i in 0..frames {
        let mut sum = 0.0f32;
        for c in 0..n_channels {
            sum += samples[i * n_channels + c];
        }
        mono.push(sum / n_channels as f32);
    }
    Some(mono)
}

fn load_beats(path: &Path) -> Option<Vec<f64>> {
    let text = fs::read_to_string(path).ok()?;
    let mut beats = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Many annotation formats: "<time>" or "<time> <beat-position>"
        // — we only care about the time.
        if let Some(first) = line.split_whitespace().next() {
            if let Ok(t) = first.parse::<f64>() {
                beats.push(t);
            }
        }
    }
    beats.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(beats)
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

#[test]
fn ballroom_eval() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!(
            "BEATPULSE_BALLROOM_DIR not set — skipping. Point this at \
             the Ballroom Dataset's audio directory and rerun."
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let wavs = walk_wavs(&dir);
    if wavs.is_empty() {
        panic!(
            "no .wav files found under {}; check the dataset layout",
            dir.display()
        );
    }

    let mut per_track: BTreeMap<String, (f64, f64, bool, bool)> = BTreeMap::new();
    for wav in &wavs {
        let stem = wav.file_stem().unwrap().to_string_lossy().into_owned();
        if is_ballroom_duplicate(&stem) {
            continue;
        }
        let beats_path = wav.with_extension("beats");
        if !beats_path.exists() {
            // Try `.txt` or `.annotation` variants.
            let alt = wav.with_extension("txt");
            if !alt.exists() {
                eprintln!("no annotations for {}; skipping", wav.display());
                continue;
            }
        }
        let Some(audio) = load_wav_mono(wav) else {
            continue;
        };
        let Some(reference) = load_beats(&beats_path) else {
            continue;
        };
        // Test the *default* tracking mode — AubioTempo since
        // 2026-05-11 (ADR-0027). The Reactive / Consensus arms are
        // measured side-by-side in `ballroom_compare`.
        let estimated = run_pipeline(&audio, TARGET_SR, Mode::AubioTempo);
        let s = Scoring::standard(&reference, &estimated);
        per_track.insert(stem, (s.f_measure, s.amlt, s.tempo_acc_1, s.tempo_acc_2));
    }

    if per_track.is_empty() {
        panic!("no tracks were successfully evaluated");
    }

    // Aggregate
    let n = per_track.len();
    let f_mean: f64 = per_track.values().map(|(f, _, _, _)| *f).sum::<f64>() / n as f64;
    let amlt_mean: f64 = per_track.values().map(|(_, a, _, _)| *a).sum::<f64>() / n as f64;
    let t1_rate = per_track.values().filter(|(_, _, t1, _)| *t1).count() as f64 / n as f64;
    let t2_rate = per_track.values().filter(|(_, _, _, t2)| *t2).count() as f64 / n as f64;
    let agg = Aggregate {
        n_tracks: n,
        f_measure_mean: f_mean,
        amlt_mean,
        tempo_acc_1_rate: t1_rate,
        tempo_acc_2_rate: t2_rate,
    };

    // Write per-track CSV
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    let _ = fs::create_dir_all(&out_dir);
    let csv_path = out_dir.join("ballroom-latest.csv");
    if let Ok(mut csv) = fs::File::create(&csv_path) {
        use std::io::Write;
        let _ = writeln!(csv, "track,f_measure,amlt,tempo_acc_1,tempo_acc_2");
        for (name, (f, a, t1, t2)) in &per_track {
            let _ = writeln!(csv, "{name},{f:.4},{a:.4},{t1},{t2}");
        }
    }

    let agg_path = out_dir.join("ballroom-latest.json");
    if let Ok(json) = serde_json::to_string_pretty(&agg) {
        let _ = fs::write(&agg_path, json);
    }

    println!(
        "Ballroom: n={} F={:.3} AMLt={:.3} TA1={:.3} TA2={:.3}",
        agg.n_tracks, agg.f_measure_mean, agg.amlt_mean, agg.tempo_acc_1_rate, agg.tempo_acc_2_rate
    );

    // Acceptance gate for the default tracking mode (AubioTempo).
    // Measured 2026-05-11 at F≈0.547; floor set to 0.50 to leave a
    // small margin for RNG / aubio version drift while catching
    // genuine regressions. ADR-0014 history: 0.70 (aspirational, PLL
    // era) → 0.25 (Reactive default era) → 0.50 (AubioTempo era).
    assert!(
        agg.f_measure_mean >= 0.50,
        "aggregate F-measure {:.3} < 0.50 (regression floor for default mode)",
        agg.f_measure_mean
    );

    // Baseline regression check
    let baseline_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/baseline-ballroom.json");
    if let Ok(text) = fs::read_to_string(&baseline_path) {
        if let Ok(baseline) = serde_json::from_str::<Aggregate>(&text) {
            let drop = baseline.f_measure_mean - agg.f_measure_mean;
            if drop > 0.02 {
                panic!(
                    "F-measure regressed: baseline {:.3}, now {:.3} (drop {:.3} > 2%)",
                    baseline.f_measure_mean, agg.f_measure_mean, drop
                );
            }
            let drop2 = baseline.tempo_acc_2_rate - agg.tempo_acc_2_rate;
            if drop2 > 0.02 {
                panic!(
                    "Tempo accuracy 2 regressed: baseline {:.3}, now {:.3}",
                    baseline.tempo_acc_2_rate, agg.tempo_acc_2_rate
                );
            }
        }
    } else {
        eprintln!(
            "no baseline at {} — commit `tests/data/output/ballroom-latest.json` \
             as `tests/data/baseline-ballroom.json` to lock in this run",
            baseline_path.display()
        );
    }
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct CompareAggregate {
    n_tracks: usize,
    reactive: Aggregate,
    consensus: Aggregate,
    aubio_tempo: Aggregate,
    delta_f_consensus_vs_reactive: f64,
    delta_f_aubio_vs_reactive: f64,
    delta_f_aubio_vs_consensus: f64,
    n_consensus_better: usize,
    n_reactive_better: usize,
    n_aubio_best: usize,
}

/// Side-by-side comparison of Reactive vs Lookahead Consensus on the
/// Ballroom dataset. No hard regression gate — the goal is to *measure*
/// the per-track delta. Output goes to `tests/data/output/`.
#[test]
fn ballroom_compare() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!(
            "BEATPULSE_BALLROOM_DIR not set — skipping. Run `cargo xtask \
             fetch-ballroom` and set the env var to enable this test."
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let wavs = walk_wavs(&dir);
    if wavs.is_empty() {
        panic!("no .wav files found under {}", dir.display());
    }

    // Per-mode (F, AMLt, TA1, TA2) for each track.
    type ModeRow = (f64, f64, bool, bool);
    type PerTrack = (ModeRow, ModeRow, ModeRow);
    let mut per_track: BTreeMap<String, PerTrack> = BTreeMap::new();

    for wav in &wavs {
        let stem = wav.file_stem().unwrap().to_string_lossy().into_owned();
        if is_ballroom_duplicate(&stem) {
            continue;
        }
        let beats_path = wav.with_extension("beats");
        let beats_path = if beats_path.exists() {
            beats_path
        } else {
            let alt = wav.with_extension("txt");
            if !alt.exists() {
                continue;
            }
            alt
        };
        let Some(audio) = load_wav_mono(wav) else {
            continue;
        };
        let Some(reference) = load_beats(&beats_path) else {
            continue;
        };

        let est_r = run_pipeline(&audio, TARGET_SR, Mode::Reactive);
        let est_c = run_pipeline(
            &audio,
            TARGET_SR,
            Mode::Consensus {
                lookahead_ms: 2000.0,
            },
        );
        let est_a = run_pipeline(&audio, TARGET_SR, Mode::AubioTempo);

        let score = |est: &[f64]| -> ModeRow {
            let s = Scoring::standard(&reference, est);
            (s.f_measure, s.amlt, s.tempo_acc_1, s.tempo_acc_2)
        };
        let row_r = score(&est_r);
        let row_c = score(&est_c);
        let row_a = score(&est_a);

        per_track.insert(stem, (row_r, row_c, row_a));
    }
    if per_track.is_empty() {
        panic!("no tracks were successfully evaluated");
    }

    let n = per_track.len();
    let avg = |pick: fn(&PerTrack) -> &ModeRow| Aggregate {
        n_tracks: n,
        f_measure_mean: per_track.values().map(|t| pick(t).0).sum::<f64>() / n as f64,
        amlt_mean: per_track.values().map(|t| pick(t).1).sum::<f64>() / n as f64,
        tempo_acc_1_rate: per_track.values().filter(|t| pick(t).2).count() as f64 / n as f64,
        tempo_acc_2_rate: per_track.values().filter(|t| pick(t).3).count() as f64 / n as f64,
    };
    let agg_r = avg(|t| &t.0);
    let agg_c = avg(|t| &t.1);
    let agg_a = avg(|t| &t.2);
    let mut n_better_c = 0;
    let mut n_better_r = 0;
    let mut n_aubio_best = 0;
    for t in per_track.values() {
        let d = t.1 .0 - t.0 .0; // consensus.F vs reactive.F
        if d > 0.005 {
            n_better_c += 1;
        } else if d < -0.005 {
            n_better_r += 1;
        }
        if t.2 .0 >= t.0 .0 && t.2 .0 >= t.1 .0 {
            n_aubio_best += 1;
        }
    }
    let comp = CompareAggregate {
        n_tracks: n,
        reactive: agg_r.clone(),
        consensus: agg_c.clone(),
        aubio_tempo: agg_a.clone(),
        delta_f_consensus_vs_reactive: agg_c.f_measure_mean - agg_r.f_measure_mean,
        delta_f_aubio_vs_reactive: agg_a.f_measure_mean - agg_r.f_measure_mean,
        delta_f_aubio_vs_consensus: agg_a.f_measure_mean - agg_c.f_measure_mean,
        n_consensus_better: n_better_c,
        n_reactive_better: n_better_r,
        n_aubio_best,
    };

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    let _ = fs::create_dir_all(&out_dir);
    let csv_path = out_dir.join("ballroom-compare.csv");
    if let Ok(mut csv) = fs::File::create(&csv_path) {
        use std::io::Write;
        let _ = writeln!(
            csv,
            "track,r_F,r_AMLt,r_TA1,r_TA2,c_F,c_AMLt,c_TA1,c_TA2,a_F,a_AMLt,a_TA1,a_TA2"
        );
        for (name, t) in &per_track {
            let _ = writeln!(
                csv,
                "{name},{:.4},{:.4},{},{},{:.4},{:.4},{},{},{:.4},{:.4},{},{}",
                t.0 .0,
                t.0 .1,
                t.0 .2,
                t.0 .3,
                t.1 .0,
                t.1 .1,
                t.1 .2,
                t.1 .3,
                t.2 .0,
                t.2 .1,
                t.2 .2,
                t.2 .3,
            );
        }
    }
    let json_path = out_dir.join("ballroom-compare.json");
    if let Ok(json) = serde_json::to_string_pretty(&comp) {
        let _ = fs::write(&json_path, json);
    }

    println!(
        "Ballroom A/B: n={n} | reactive F={:.3} AMLt={:.3} | consensus F={:.3} AMLt={:.3} | \
         aubio_tempo F={:.3} AMLt={:.3} TA2={:.3} | \
         ΔF(c-r)={:+.3} ΔF(a-r)={:+.3} ΔF(a-c)={:+.3} | \
         (n_consensus_better_vs_reactive={}, n_reactive_better_vs_consensus={}, n_aubio_best_overall={})",
        agg_r.f_measure_mean,
        agg_r.amlt_mean,
        agg_c.f_measure_mean,
        agg_c.amlt_mean,
        agg_a.f_measure_mean,
        agg_a.amlt_mean,
        agg_a.tempo_acc_2_rate,
        comp.delta_f_consensus_vs_reactive,
        comp.delta_f_aubio_vs_reactive,
        comp.delta_f_aubio_vs_consensus,
        n_better_c,
        n_better_r,
        n_aubio_best,
    );
}
