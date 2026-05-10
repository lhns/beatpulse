// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Shared test helpers for the integration suite.
//!
//! Lets each integration test exercise the full beat pipeline in either
//! tracking mode (Reactive or Lookahead Consensus) and recover a
//! comparable predicted-beat-time stream. The harness mirrors what
//! `Plugin::process` does in `src/lib.rs`, minus host I/O — so the two
//! code paths share `ConsensusTracker::try_snap_pll` and stay in sync.

#![allow(dead_code)] // each integration test only uses a subset

#[cfg(feature = "dataset-tests")]
pub mod audio;

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::consensus_tracker::ConsensusTracker;
use beatpulse::params::OnsetMethod;

/// Block size used by the shared harness. Matches the value used by the
/// other integration tests so timing-sensitive behaviour is comparable.
pub const BLOCK: usize = 512;

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Reactive,
    Consensus { lookahead_ms: f64 },
}

/// Run `audio` through the full BeatTracker → (PLL | Consensus → PLL)
/// pipeline at `sr`, returning predicted beat times in seconds.
///
/// Predicted beats are sampled from the PLL itself: each time the PLL
/// phase wraps (post-lock), that sample is recorded as a beat boundary.
/// This makes Reactive and Consensus directly comparable — both use the
/// same emit rule.
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
    let sr_f = sr as f64;
    let mut tracker = BeatTracker::new(sr, onset_method, threshold).expect("BeatTracker init");
    let mut pll = BeatPll::new(sr_f);
    let mut consensus = match mode {
        Mode::Consensus { lookahead_ms } => Some(ConsensusTracker::new(sr_f, lookahead_ms)),
        Mode::Reactive => None,
    };

    let mut beat_times = Vec::new();
    let mut absolute = 0u64;
    let mut prev_phase = pll.phase_samples;

    let mut onset_buf: Vec<(u32, u64)> = Vec::with_capacity(16);

    for chunk in audio.chunks(BLOCK) {
        let block_start = absolute;
        let block_len = chunk.len();
        onset_buf.clear();

        tracker.process_block(chunk, |offset, _frac| {
            onset_buf.push((offset, block_start + offset as u64));
        });

        // Consensus path: ingest onsets up-front, snap PLL once per block.
        if let Some(c) = consensus.as_mut() {
            for &(_, abs) in &onset_buf {
                c.on_onset(abs);
            }
            c.try_snap_pll(&mut pll, block_start, block_len as u64);
        }

        // Per-sample loop: advance PLL one sample at a time. In Reactive
        // mode, also feed each onset into the PLL at its sample position.
        let mut next = 0usize;
        for i in 0..block_len as u32 {
            if matches!(mode, Mode::Reactive) {
                while next < onset_buf.len() && onset_buf[next].0 == i {
                    pll.on_onset(onset_buf[next].1 as f64);
                    next += 1;
                }
            } else {
                while next < onset_buf.len() && onset_buf[next].0 == i {
                    next += 1;
                }
            }
            pll.advance_one();
            // Beat boundary = phase wrap (current < previous and we're
            // locked enough to trust it).
            if pll.locked && pll.phase_samples < prev_phase {
                let abs_sample = block_start + i as u64;
                beat_times.push(abs_sample as f64 / sr_f);
            }
            prev_phase = pll.phase_samples;
        }
        // Drain any onsets at sample == block_len.
        if matches!(mode, Mode::Reactive) {
            while next < onset_buf.len() {
                pll.on_onset(onset_buf[next].1 as f64);
                next += 1;
            }
        }
        absolute += block_len as u64;
    }

    beat_times
}

/// Run the pipeline and additionally collect `pll.current_bpm()` once
/// per detected beat (post-lock). Used by `bpm_stability.rs` for the σ
/// comparison.
pub fn run_pipeline_with_bpm_log(audio: &[f32], sr: u32, mode: Mode) -> (Vec<f64>, Vec<f64>) {
    let sr_f = sr as f64;
    let mut tracker = BeatTracker::new(sr, OnsetMethod::SpecFlux, 0.3).expect("BeatTracker init");
    let mut pll = BeatPll::new(sr_f);
    let mut consensus = match mode {
        Mode::Consensus { lookahead_ms } => Some(ConsensusTracker::new(sr_f, lookahead_ms)),
        Mode::Reactive => None,
    };

    let mut beat_times = Vec::new();
    let mut bpm_log = Vec::new();
    let mut absolute = 0u64;
    let mut prev_phase = pll.phase_samples;
    let mut onset_buf: Vec<(u32, u64)> = Vec::with_capacity(16);

    for chunk in audio.chunks(BLOCK) {
        let block_start = absolute;
        let block_len = chunk.len();
        onset_buf.clear();
        tracker.process_block(chunk, |offset, _frac| {
            onset_buf.push((offset, block_start + offset as u64));
        });
        if let Some(c) = consensus.as_mut() {
            for &(_, abs) in &onset_buf {
                c.on_onset(abs);
            }
            c.try_snap_pll(&mut pll, block_start, block_len as u64);
        }
        let mut next = 0usize;
        for i in 0..block_len as u32 {
            if matches!(mode, Mode::Reactive) {
                while next < onset_buf.len() && onset_buf[next].0 == i {
                    pll.on_onset(onset_buf[next].1 as f64);
                    next += 1;
                }
            } else {
                while next < onset_buf.len() && onset_buf[next].0 == i {
                    next += 1;
                }
            }
            pll.advance_one();
            if pll.locked && pll.phase_samples < prev_phase {
                let abs_sample = block_start + i as u64;
                beat_times.push(abs_sample as f64 / sr_f);
                bpm_log.push(pll.current_bpm());
            }
            prev_phase = pll.phase_samples;
        }
        if matches!(mode, Mode::Reactive) {
            while next < onset_buf.len() {
                pll.on_onset(onset_buf[next].1 as f64);
                next += 1;
            }
        }
        absolute += block_len as u64;
    }
    (beat_times, bpm_log)
}
