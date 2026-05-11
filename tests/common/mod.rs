// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Shared test helpers for the integration suite.
//!
//! Each tracking mode runs through its own dedicated function in this
//! module; `run_pipeline` is a thin dispatch on `Mode`. The functions
//! mirror what `Plugin::process` does in `src/lib.rs`, minus host I/O
//! — same `BeatTracker`, `BeatPll`, `ConsensusTracker`,
//! `AubioTempoTracker`, `PulseGenerator`, `AubioPulseEmitter`, all at
//! PPQN=1 (one pulse per beat boundary). Test metric matches what the
//! user actually hears/sees through the production audio path.

#![allow(dead_code)] // each integration test only uses a subset

#[cfg(feature = "dataset-tests")]
pub mod audio;
pub mod datasets;

use beatpulse::dsp::aubio_pulse_emitter::AubioPulseEmitter;
use beatpulse::dsp::aubio_tempo_tracker::AubioTempoTracker;
use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::consensus_tracker::ConsensusTracker;
use beatpulse::dsp::pulse_generator::PulseGenerator;
use beatpulse::params::OnsetMethod;

/// Block size used by the shared harness. Matches the value used by
/// the other integration tests so timing-sensitive behaviour is
/// comparable.
pub const BLOCK: usize = 512;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Reactive,
    Consensus { lookahead_ms: f64 },
    AubioTempo,
}

/// Run `audio` through the full pipeline at `sr` for `mode`. Returns
/// predicted beat times in seconds.
pub fn run_pipeline(audio: &[f32], sr: u32, mode: Mode) -> Vec<f64> {
    run_pipeline_with(audio, sr, mode, OnsetMethod::SpecFlux, 0.3)
}

pub fn run_pipeline_with(
    audio: &[f32],
    sr: u32,
    mode: Mode,
    onset_method: OnsetMethod,
    threshold: f32,
) -> Vec<f64> {
    let (beats, _) = dispatch(audio, sr, mode, onset_method, threshold, false);
    beats
}

/// Run the pipeline and additionally collect `pll.current_bpm()` once
/// per emitted beat. Used by `bpm_stability.rs` for the σ comparison.
pub fn run_pipeline_with_bpm_log(audio: &[f32], sr: u32, mode: Mode) -> (Vec<f64>, Vec<f64>) {
    dispatch(audio, sr, mode, OnsetMethod::SpecFlux, 0.3, true)
}

fn dispatch(
    audio: &[f32],
    sr: u32,
    mode: Mode,
    onset_method: OnsetMethod,
    threshold: f32,
    log_bpm: bool,
) -> (Vec<f64>, Vec<f64>) {
    match mode {
        Mode::Reactive => run_reactive(audio, sr, onset_method, threshold, log_bpm),
        Mode::Consensus { lookahead_ms } => {
            run_consensus(audio, sr, onset_method, threshold, lookahead_ms, log_bpm)
        }
        Mode::AubioTempo => run_aubio_tempo(audio, sr, onset_method, threshold, log_bpm),
    }
}

fn run_reactive(
    audio: &[f32],
    sr: u32,
    onset_method: OnsetMethod,
    threshold: f32,
    log_bpm: bool,
) -> (Vec<f64>, Vec<f64>) {
    let sr_f = sr as f64;
    let mut tracker = BeatTracker::new(sr, onset_method, threshold).expect("BeatTracker init");
    let mut pll = BeatPll::new(sr_f);
    let mut pulse_gen = PulseGenerator::new(1);
    let mut beat_times = Vec::new();
    let mut bpm_log = Vec::new();
    let mut absolute = 0u64;
    let mut onset_buf: Vec<(u32, u64)> = Vec::with_capacity(16);

    for chunk in audio.chunks(BLOCK) {
        let block_start = absolute;
        onset_buf.clear();
        tracker.process_block(chunk, |offset, _frac| {
            onset_buf.push((offset, block_start + offset as u64));
        });
        let mut next = 0usize;
        for i in 0..chunk.len() as u32 {
            while next < onset_buf.len() && onset_buf[next].0 == i {
                pll.on_onset(onset_buf[next].1 as f64);
                next += 1;
            }
            pll.advance_one();
            if pulse_gen.observe_advance(&pll, i).is_some() {
                let abs_sample = block_start + i as u64;
                beat_times.push(abs_sample as f64 / sr_f);
                if log_bpm {
                    bpm_log.push(pll.current_bpm());
                }
            }
        }
        while next < onset_buf.len() {
            pll.on_onset(onset_buf[next].1 as f64);
            next += 1;
        }
        absolute += chunk.len() as u64;
    }
    (beat_times, bpm_log)
}

fn run_consensus(
    audio: &[f32],
    sr: u32,
    onset_method: OnsetMethod,
    threshold: f32,
    lookahead_ms: f64,
    log_bpm: bool,
) -> (Vec<f64>, Vec<f64>) {
    let sr_f = sr as f64;
    let mut tracker = BeatTracker::new(sr, onset_method, threshold).expect("BeatTracker init");
    let mut pll = BeatPll::new(sr_f);
    let mut consensus = ConsensusTracker::new(sr_f, lookahead_ms);
    let mut pulse_gen = PulseGenerator::new(1);
    let mut beat_times = Vec::new();
    let mut bpm_log = Vec::new();
    let mut absolute = 0u64;
    let mut onset_buf: Vec<(u32, u64)> = Vec::with_capacity(16);

    for chunk in audio.chunks(BLOCK) {
        let block_start = absolute;
        let block_len = chunk.len() as u64;
        onset_buf.clear();
        tracker.process_block(chunk, |offset, _frac| {
            onset_buf.push((offset, block_start + offset as u64));
        });
        for &(_, abs) in &onset_buf {
            consensus.on_onset(abs);
        }
        consensus.try_snap_pll(&mut pll, block_start, block_len);

        for i in 0..chunk.len() as u32 {
            pll.advance_one();
            if pulse_gen.observe_advance(&pll, i).is_some() {
                let abs_sample = block_start + i as u64;
                beat_times.push(abs_sample as f64 / sr_f);
                if log_bpm {
                    bpm_log.push(pll.current_bpm());
                }
            }
        }
        absolute += block_len;
    }
    (beat_times, bpm_log)
}

fn run_aubio_tempo(
    audio: &[f32],
    sr: u32,
    onset_method: OnsetMethod,
    threshold: f32,
    log_bpm: bool,
) -> (Vec<f64>, Vec<f64>) {
    let sr_f = sr as f64;
    let mut tracker =
        AubioTempoTracker::new(sr, onset_method, threshold).expect("AubioTempoTracker init");
    let mut pll = BeatPll::new(sr_f);
    let mut emitter = AubioPulseEmitter::new(1);
    let mut beat_times = Vec::new();
    let mut bpm_log = Vec::new();
    let mut absolute = 0u64;
    let mut beat_buf: Vec<(u32, u64)> = Vec::with_capacity(16);

    for chunk in audio.chunks(BLOCK) {
        let block_start = absolute;
        beat_buf.clear();
        tracker.process_block(chunk, block_start, |offset, abs_sample| {
            beat_buf.push((offset, abs_sample));
        });

        let mut next_b = 0usize;
        for i in 0..chunk.len() as u32 {
            let mut on_beat_emitted = false;
            while next_b < beat_buf.len() && beat_buf[next_b].0 == i {
                tracker.snap_pll_at_beat(&mut pll, beat_buf[next_b].1);
                if emitter.on_beat(beat_buf[next_b].1).is_some() {
                    on_beat_emitted = true;
                    let abs_sample = beat_buf[next_b].1;
                    beat_times.push(abs_sample as f64 / sr_f);
                    if log_bpm {
                        bpm_log.push(pll.current_bpm());
                    }
                }
                next_b += 1;
            }
            pll.advance_one();
            if !on_beat_emitted {
                let abs_sample = block_start + i as u64;
                if emitter.tick(abs_sample).is_some() {
                    beat_times.push(abs_sample as f64 / sr_f);
                    if log_bpm {
                        bpm_log.push(pll.current_bpm());
                    }
                }
            }
        }
        absolute += chunk.len() as u64;
    }
    (beat_times, bpm_log)
}
