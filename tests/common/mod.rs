// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Shared test helpers for the integration suite.
//!
//! Lets each integration test exercise the full beat pipeline in either
//! tracking mode (Reactive or Lookahead Consensus) and recover a
//! comparable predicted-beat-time stream. The harness mirrors what
//! `Plugin::process` does in `src/lib.rs`, minus host I/O — so the two
//! code paths share `ConsensusTracker::try_snap_pll` and
//! `PulseGenerator` (the latter at PPQN=1, which means every emitted
//! pulse is a beat boundary). The PulseGenerator's monotonic-progress
//! plus 50%-period wrap test is the same logic that drives the
//! production LED/MIDI output, so the test metric matches what the
//! user sees.

#![allow(dead_code)] // each integration test only uses a subset

#[cfg(feature = "dataset-tests")]
pub mod audio;

use beatpulse::dsp::aubio_tempo_tracker::AubioTempoTracker;
use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::consensus_tracker::ConsensusTracker;
use beatpulse::dsp::pulse_generator::PulseGenerator;
use beatpulse::params::OnsetMethod;

/// Block size used by the shared harness. Matches the value used by the
/// other integration tests so timing-sensitive behaviour is comparable.
pub const BLOCK: usize = 512;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Reactive,
    Consensus { lookahead_ms: f64 },
    AubioTempo,
}

/// Run `audio` through the full BeatTracker → (PLL | Consensus → PLL) →
/// PulseGenerator pipeline at `sr`, returning predicted beat times in
/// seconds. Beats are emitted by `PulseGenerator::new(1)` (PPQN=1, so
/// each pulse is a beat boundary), matching production semantics.
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
    let (beats, _) = run_pipeline_inner(audio, sr, mode, onset_method, threshold, false);
    beats
}

/// Run the pipeline and additionally collect `pll.current_bpm()` once
/// per emitted beat. Used by `bpm_stability.rs` for the σ comparison.
pub fn run_pipeline_with_bpm_log(audio: &[f32], sr: u32, mode: Mode) -> (Vec<f64>, Vec<f64>) {
    run_pipeline_inner(audio, sr, mode, OnsetMethod::SpecFlux, 0.3, true)
}

fn run_pipeline_inner(
    audio: &[f32],
    sr: u32,
    mode: Mode,
    onset_method: OnsetMethod,
    threshold: f32,
    log_bpm: bool,
) -> (Vec<f64>, Vec<f64>) {
    let sr_f = sr as f64;
    // BeatTracker (Onset) is used for Reactive + Consensus only;
    // AubioTempo replaces it with its own onset detection inside the
    // Tempo object. We still construct it for the other modes.
    let needs_onsets = !matches!(mode, Mode::AubioTempo);
    let mut tracker_opt = if needs_onsets {
        Some(BeatTracker::new(sr, onset_method, threshold).expect("BeatTracker init"))
    } else {
        None
    };
    let mut aubio_tempo_opt = match mode {
        Mode::AubioTempo => Some(
            AubioTempoTracker::new(sr, onset_method, threshold).expect("AubioTempoTracker init"),
        ),
        _ => None,
    };
    let mut pll = BeatPll::new(sr_f);
    let mut consensus = match mode {
        Mode::Consensus { lookahead_ms } => Some(ConsensusTracker::new(sr_f, lookahead_ms)),
        _ => None,
    };
    let mut pulse_gen = PulseGenerator::new(1);

    let mut beat_times: Vec<f64> = Vec::new();
    let mut bpm_log: Vec<f64> = Vec::new();
    let mut absolute = 0u64;
    let mut onset_buf: Vec<(u32, u64)> = Vec::with_capacity(16);
    let mut beat_buf: Vec<(u32, u64)> = Vec::with_capacity(16);

    for chunk in audio.chunks(BLOCK) {
        let block_start = absolute;
        let block_len = chunk.len();
        onset_buf.clear();
        beat_buf.clear();

        if let Some(t) = tracker_opt.as_mut() {
            t.process_block(chunk, |offset, _frac| {
                onset_buf.push((offset, block_start + offset as u64));
            });
        }
        if let Some(t) = aubio_tempo_opt.as_mut() {
            t.process_block(chunk, block_start, |offset, abs_sample| {
                beat_buf.push((offset, abs_sample));
            });
        }
        if let Some(c) = consensus.as_mut() {
            for &(_, abs) in &onset_buf {
                c.on_onset(abs);
            }
            c.try_snap_pll(&mut pll, block_start, block_len as u64);
        }

        let mut next_o = 0usize;
        let mut next_b = 0usize;
        for i in 0..block_len as u32 {
            while next_b < beat_buf.len() && beat_buf[next_b].0 == i {
                if let Some(t) = aubio_tempo_opt.as_mut() {
                    t.snap_pll_at_beat(&mut pll, beat_buf[next_b].1);
                }
                next_b += 1;
            }
            while next_o < onset_buf.len() && onset_buf[next_o].0 == i {
                if matches!(mode, Mode::Reactive) {
                    pll.on_onset(onset_buf[next_o].1 as f64);
                }
                next_o += 1;
            }
            pll.advance_one();
            if let Some(_ev) = pulse_gen.observe_advance(&pll, i) {
                let abs_sample = block_start + i as u64;
                beat_times.push(abs_sample as f64 / sr_f);
                if log_bpm {
                    bpm_log.push(pll.current_bpm());
                }
            }
        }
        if matches!(mode, Mode::Reactive) {
            while next_o < onset_buf.len() {
                pll.on_onset(onset_buf[next_o].1 as f64);
                next_o += 1;
            }
        }
        absolute += block_len as u64;
    }
    (beat_times, bpm_log)
}
