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

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::eval::{
    f_measure, tempo_accuracy_1, tempo_accuracy_2, F_MEASURE_TOL, TEMPO_ACC_TOL,
};
use beatpulse::params::OnsetMethod;
use serde::{Deserialize, Serialize};

const TARGET_SR: u32 = 44_100;

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct Aggregate {
    n_tracks: usize,
    f_measure_mean: f64,
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
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
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

fn process_one(audio: &[f32]) -> Vec<f64> {
    let mut tracker = BeatTracker::new(TARGET_SR, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut pll = BeatPll::new(TARGET_SR as f64);
    let mut estimated = Vec::new();
    let mut absolute = 0u64;

    for chunk in audio.chunks(512) {
        let abs_at_block = absolute;
        tracker.process_block(chunk, |offset, _frac| {
            let abs_sample = abs_at_block + offset as u64;
            pll.on_onset(abs_sample as f64);
            estimated.push(abs_sample as f64 / TARGET_SR as f64);
        });
        pll.advance(chunk.len() as u64);
        absolute += chunk.len() as u64;
    }
    estimated
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

    let mut per_track: BTreeMap<String, (f64, bool, bool)> = BTreeMap::new();
    for wav in &wavs {
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
        let estimated = process_one(&audio);
        let f = f_measure(&reference, &estimated, F_MEASURE_TOL);
        let t1 = tempo_accuracy_1(&reference, &estimated, TEMPO_ACC_TOL);
        let t2 = tempo_accuracy_2(&reference, &estimated, TEMPO_ACC_TOL);
        let name = wav.file_stem().unwrap().to_string_lossy().into_owned();
        per_track.insert(name, (f, t1, t2));
    }

    if per_track.is_empty() {
        panic!("no tracks were successfully evaluated");
    }

    // Aggregate
    let n = per_track.len();
    let f_mean: f64 = per_track.values().map(|(f, _, _)| *f).sum::<f64>() / n as f64;
    let t1_rate =
        per_track.values().filter(|(_, t1, _)| *t1).count() as f64 / n as f64;
    let t2_rate =
        per_track.values().filter(|(_, _, t2)| *t2).count() as f64 / n as f64;
    let agg = Aggregate {
        n_tracks: n,
        f_measure_mean: f_mean,
        tempo_acc_1_rate: t1_rate,
        tempo_acc_2_rate: t2_rate,
    };

    // Write per-track CSV
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    let _ = fs::create_dir_all(&out_dir);
    let csv_path = out_dir.join("ballroom-latest.csv");
    if let Ok(mut csv) = fs::File::create(&csv_path) {
        use std::io::Write;
        let _ = writeln!(csv, "track,f_measure,tempo_acc_1,tempo_acc_2");
        for (name, (f, t1, t2)) in &per_track {
            let _ = writeln!(csv, "{name},{f:.4},{t1},{t2}");
        }
    }

    let agg_path = out_dir.join("ballroom-latest.json");
    if let Ok(json) = serde_json::to_string_pretty(&agg) {
        let _ = fs::write(&agg_path, json);
    }

    println!(
        "Ballroom: n={} F={:.3} TA1={:.3} TA2={:.3}",
        agg.n_tracks, agg.f_measure_mean, agg.tempo_acc_1_rate, agg.tempo_acc_2_rate
    );

    // Acceptance gate (per ADR-0014)
    assert!(
        agg.f_measure_mean >= 0.70,
        "aggregate F-measure {:.3} < 0.70 (acceptance gate)",
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
