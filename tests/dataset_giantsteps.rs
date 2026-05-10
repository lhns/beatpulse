// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! GiantSteps Tempo evaluation harness — Reactive vs Lookahead
//! Consensus on real EDM full mixes (house, techno, trance, DnB,
//! dubstep). Closer to the BeatPulse / Daslight target use case than
//! Ballroom or the synthetic clicks.
//!
//! Activate with `cargo test --features dataset-tests --test dataset_giantsteps -- --nocapture`
//! and set `BEATPULSE_GIANTSTEPS_DIR` to the audio directory populated
//! by `cargo xtask-fetch-giantsteps`. The directory must contain pairs
//! of `<id>.LOFI.mp3` + `<id>.LOFI.bpm` (one integer BPM per file).
//!
//! Tempo-only ground truth → no F-measure; we score with **tempo
//! accuracy 1** (predicted BPM within ±4 % of truth) and **tempo
//! accuracy 2** (within ±4 % allowing octave error).

#![cfg(feature = "dataset-tests")]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod common;
use common::audio::decode_mono_44k1;
use common::{run_pipeline, Mode};

const TARGET_SR: u32 = 44_100;
const TEMPO_TOL_PCT: f64 = 0.04;
const LOOKAHEAD_MS: f64 = 2000.0;

/// (true_bpm, reactive_ta1, reactive_ta2, reactive_bpm,
///  consensus_ta1, consensus_ta2, consensus_bpm)
type PerTrack = (f64, bool, bool, f64, bool, bool, f64);

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct ModeAggregate {
    n_tracks: usize,
    tempo_acc_1_rate: f64,
    tempo_acc_2_rate: f64,
    bpm_mae: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct CompareAggregate {
    n_tracks: usize,
    reactive: ModeAggregate,
    consensus: ModeAggregate,
    delta_ta1: f64,
    delta_ta2: f64,
    n_consensus_better_ta2: usize,
    n_reactive_better_ta2: usize,
    n_tied_ta2: usize,
}

#[test]
fn giantsteps_compare() {
    let Ok(dir) = env::var("BEATPULSE_GIANTSTEPS_DIR") else {
        eprintln!(
            "BEATPULSE_GIANTSTEPS_DIR not set — skipping. Run \
             `cargo xtask-fetch-giantsteps` and set the env var to enable."
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let mp3s = walk_mp3s(&dir);
    if mp3s.is_empty() {
        panic!("no .mp3 files found under {}", dir.display());
    }

    // (true_bpm, ta1_r, ta2_r, est_bpm_r, ta1_c, ta2_c, est_bpm_c)
    let mut per_track: BTreeMap<String, PerTrack> = BTreeMap::new();

    for mp3 in &mp3s {
        let bpm_path = mp3.with_extension("bpm");
        if !bpm_path.exists() {
            continue;
        }
        let Some(true_bpm) = load_bpm(&bpm_path) else {
            continue;
        };
        let Some(audio) = decode_mono_44k1(mp3) else {
            continue;
        };

        let est_r = run_pipeline(&audio, TARGET_SR, Mode::Reactive);
        let est_c = run_pipeline(
            &audio,
            TARGET_SR,
            Mode::Consensus {
                lookahead_ms: LOOKAHEAD_MS,
            },
        );

        let bpm_r = bpm_from_beats(&est_r);
        let bpm_c = bpm_from_beats(&est_c);
        let ta1_r = tempo_within(bpm_r, true_bpm, TEMPO_TOL_PCT, false);
        let ta2_r = tempo_within(bpm_r, true_bpm, TEMPO_TOL_PCT, true);
        let ta1_c = tempo_within(bpm_c, true_bpm, TEMPO_TOL_PCT, false);
        let ta2_c = tempo_within(bpm_c, true_bpm, TEMPO_TOL_PCT, true);

        let name = mp3.file_stem().unwrap().to_string_lossy().into_owned();
        per_track.insert(name, (true_bpm, ta1_r, ta2_r, bpm_r, ta1_c, ta2_c, bpm_c));
    }
    if per_track.is_empty() {
        panic!("no tracks were successfully evaluated");
    }
    let n = per_track.len();

    let agg_r = aggregate(&per_track, |t| (t.1, t.2, (t.3 - t.0).abs()));
    let agg_c = aggregate(&per_track, |t| (t.4, t.5, (t.6 - t.0).abs()));
    let mut better_c = 0;
    let mut better_r = 0;
    let mut tied = 0;
    for t in per_track.values() {
        match (t.5, t.2) {
            (true, false) => better_c += 1,
            (false, true) => better_r += 1,
            _ => tied += 1,
        }
    }

    let comp = CompareAggregate {
        n_tracks: n,
        reactive: agg_r.clone(),
        consensus: agg_c.clone(),
        delta_ta1: agg_c.tempo_acc_1_rate - agg_r.tempo_acc_1_rate,
        delta_ta2: agg_c.tempo_acc_2_rate - agg_r.tempo_acc_2_rate,
        n_consensus_better_ta2: better_c,
        n_reactive_better_ta2: better_r,
        n_tied_ta2: tied,
    };

    write_artefacts(&per_track, &comp);

    println!(
        "GiantSteps A/B: n={n} | reactive TA1={:.3} TA2={:.3} MAE={:.2} | \
         consensus TA1={:.3} TA2={:.3} MAE={:.2} | \
         ΔTA1={:+.3} ΔTA2={:+.3} (consensus better: {} | reactive better: {} | tied: {})",
        agg_r.tempo_acc_1_rate,
        agg_r.tempo_acc_2_rate,
        agg_r.bpm_mae,
        agg_c.tempo_acc_1_rate,
        agg_c.tempo_acc_2_rate,
        agg_c.bpm_mae,
        comp.delta_ta1,
        comp.delta_ta2,
        better_c,
        better_r,
        tied,
    );
}

fn aggregate<F>(per_track: &BTreeMap<String, PerTrack>, pick: F) -> ModeAggregate
where
    F: Fn(&PerTrack) -> (bool, bool, f64),
{
    let n = per_track.len() as f64;
    let mut ta1 = 0usize;
    let mut ta2 = 0usize;
    let mut sum_err = 0.0f64;
    for t in per_track.values() {
        let (a, b, e) = pick(t);
        if a {
            ta1 += 1;
        }
        if b {
            ta2 += 1;
        }
        sum_err += e;
    }
    ModeAggregate {
        n_tracks: per_track.len(),
        tempo_acc_1_rate: ta1 as f64 / n,
        tempo_acc_2_rate: ta2 as f64 / n,
        bpm_mae: sum_err / n,
    }
}

/// Predict tempo from a beat-time stream via the median IOI. Returns
/// 0.0 if there aren't enough beats to compute one.
fn bpm_from_beats(beats: &[f64]) -> f64 {
    if beats.len() < 3 {
        return 0.0;
    }
    let mut iois: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    iois.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = iois[iois.len() / 2];
    if median > 0.0 {
        60.0 / median
    } else {
        0.0
    }
}

fn tempo_within(predicted: f64, truth: f64, tol_pct: f64, octave_tolerant: bool) -> bool {
    if predicted <= 0.0 || truth <= 0.0 {
        return false;
    }
    let candidates: &[f64] = if octave_tolerant {
        &[truth, truth * 2.0, truth * 0.5, truth * 3.0, truth / 3.0]
    } else {
        &[truth]
    };
    candidates
        .iter()
        .any(|c| (predicted - c).abs() / c < tol_pct)
}

fn load_bpm(path: &Path) -> Option<f64> {
    let s = fs::read_to_string(path).ok()?;
    s.trim().parse::<f64>().ok()
}

fn walk_mp3s(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.extend(walk_mp3s(&p));
        } else if p.extension().and_then(|s| s.to_str()) == Some("mp3") {
            out.push(p);
        }
    }
    out
}

fn write_artefacts(per_track: &BTreeMap<String, PerTrack>, comp: &CompareAggregate) {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    let _ = fs::create_dir_all(&out_dir);
    let csv_path = out_dir.join("giantsteps-compare.csv");
    if let Ok(mut csv) = fs::File::create(&csv_path) {
        use std::io::Write;
        let _ = writeln!(
            csv,
            "track,true_bpm,reactive_bpm,reactive_ta1,reactive_ta2,consensus_bpm,consensus_ta1,consensus_ta2"
        );
        for (name, t) in per_track {
            let _ = writeln!(
                csv,
                "{name},{:.2},{:.2},{},{},{:.2},{},{}",
                t.0, t.3, t.1, t.2, t.6, t.4, t.5
            );
        }
    }
    let json_path = out_dir.join("giantsteps-compare.json");
    if let Ok(json) = serde_json::to_string_pretty(comp) {
        let _ = fs::write(&json_path, json);
    }
}
