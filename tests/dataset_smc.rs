// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! SMC_MIREX evaluation harness — Reactive vs Lookahead Consensus on
//! the explicit "hard cases" set (217 × 40s clips chosen because beat-
//! tracking fails on them). Per-beat ground truth → F-measure +
//! tempo accuracy.
//!
//! Activate with `cargo test --features dataset-tests --test dataset_smc -- --nocapture`
//! and set `BEATPULSE_SMC_DIR` to the audio directory containing
//! `<id>.wav` + `<id>.txt` pairs (one beat time per line, seconds).
//! See `cargo xtask-fetch-smc --help` for setup notes — the canonical
//! mirror is currently flaky, so manual setup is expected.
//!
//! No hard regression gate; numbers reported for diagnosis.

#![cfg(feature = "dataset-tests")]

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use beatpulse::eval::Scoring;
use serde::{Deserialize, Serialize};

mod common;
use common::audio::decode_mono_44k1;
use common::{run_pipeline, Mode};

const TARGET_SR: u32 = 44_100;
const LOOKAHEAD_MS: f64 = 2000.0;

/// (reactive_F, reactive_TA1, reactive_TA2,
///  consensus_F, consensus_TA1, consensus_TA2)
type PerTrack = (f64, bool, bool, f64, bool, bool);

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct ModeAggregate {
    n_tracks: usize,
    f_measure_mean: f64,
    tempo_acc_1_rate: f64,
    tempo_acc_2_rate: f64,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct CompareAggregate {
    n_tracks: usize,
    reactive: ModeAggregate,
    consensus: ModeAggregate,
    delta_f_mean: f64,
    n_consensus_better: usize,
    n_reactive_better: usize,
    n_tied: usize,
}

#[test]
fn smc_compare() {
    let Ok(dir) = env::var("BEATPULSE_SMC_DIR") else {
        eprintln!(
            "BEATPULSE_SMC_DIR not set — skipping. See \
             `cargo xtask-fetch-smc --help` for setup."
        );
        return;
    };
    let dir = PathBuf::from(dir);
    let wavs = walk_wavs(&dir);
    if wavs.is_empty() {
        panic!("no .wav files found under {}", dir.display());
    }

    let mut per_track: BTreeMap<String, PerTrack> = BTreeMap::new();
    for wav in &wavs {
        let txt = wav.with_extension("txt");
        if !txt.exists() {
            continue;
        }
        let Some(audio) = decode_mono_44k1(wav) else {
            continue;
        };
        let Some(reference) = load_beats(&txt) else {
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

        let s_r = Scoring::standard(&reference, &est_r);
        let s_c = Scoring::standard(&reference, &est_c);
        let (f_r, t1_r, t2_r) = (s_r.f_measure, s_r.tempo_acc_1, s_r.tempo_acc_2);
        let (f_c, t1_c, t2_c) = (s_c.f_measure, s_c.tempo_acc_1, s_c.tempo_acc_2);

        let name = wav.file_stem().unwrap().to_string_lossy().into_owned();
        per_track.insert(name, (f_r, t1_r, t2_r, f_c, t1_c, t2_c));
    }
    if per_track.is_empty() {
        panic!("no tracks were successfully evaluated");
    }
    let n = per_track.len();
    let agg_r = ModeAggregate {
        n_tracks: n,
        f_measure_mean: per_track.values().map(|t| t.0).sum::<f64>() / n as f64,
        tempo_acc_1_rate: per_track.values().filter(|t| t.1).count() as f64 / n as f64,
        tempo_acc_2_rate: per_track.values().filter(|t| t.2).count() as f64 / n as f64,
    };
    let agg_c = ModeAggregate {
        n_tracks: n,
        f_measure_mean: per_track.values().map(|t| t.3).sum::<f64>() / n as f64,
        tempo_acc_1_rate: per_track.values().filter(|t| t.4).count() as f64 / n as f64,
        tempo_acc_2_rate: per_track.values().filter(|t| t.5).count() as f64 / n as f64,
    };
    let mut better_c = 0;
    let mut better_r = 0;
    let mut tied = 0;
    for t in per_track.values() {
        let d = t.3 - t.0;
        if d > 0.005 {
            better_c += 1;
        } else if d < -0.005 {
            better_r += 1;
        } else {
            tied += 1;
        }
    }
    let comp = CompareAggregate {
        n_tracks: n,
        reactive: agg_r.clone(),
        consensus: agg_c.clone(),
        delta_f_mean: agg_c.f_measure_mean - agg_r.f_measure_mean,
        n_consensus_better: better_c,
        n_reactive_better: better_r,
        n_tied: tied,
    };

    write_artefacts(&per_track, &comp);

    println!(
        "SMC_MIREX A/B: n={n} | reactive F={:.3} TA2={:.3} | consensus F={:.3} TA2={:.3} | \
         ΔF={:+.3} (consensus better: {} | reactive better: {} | tied: {})",
        agg_r.f_measure_mean,
        agg_r.tempo_acc_2_rate,
        agg_c.f_measure_mean,
        agg_c.tempo_acc_2_rate,
        comp.delta_f_mean,
        better_c,
        better_r,
        tied,
    );
}

fn load_beats(path: &Path) -> Option<Vec<f64>> {
    let text = fs::read_to_string(path).ok()?;
    let mut beats = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
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

fn write_artefacts(per_track: &BTreeMap<String, PerTrack>, comp: &CompareAggregate) {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    let _ = fs::create_dir_all(&out_dir);
    let csv_path = out_dir.join("smc-compare.csv");
    if let Ok(mut csv) = fs::File::create(&csv_path) {
        use std::io::Write;
        let _ = writeln!(
            csv,
            "track,reactive_F,reactive_TA1,reactive_TA2,consensus_F,consensus_TA1,consensus_TA2,delta_F"
        );
        for (name, t) in per_track {
            let _ = writeln!(
                csv,
                "{name},{:.4},{},{},{:.4},{},{},{:+.4}",
                t.0,
                t.1,
                t.2,
                t.3,
                t.4,
                t.5,
                t.3 - t.0
            );
        }
    }
    let json_path = out_dir.join("smc-compare.json");
    if let Ok(json) = serde_json::to_string_pretty(comp) {
        let _ = fs::write(&json_path, json);
    }
}
