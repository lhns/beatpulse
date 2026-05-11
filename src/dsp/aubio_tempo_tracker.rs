// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Beat tracker wrapping `aubio_rs::Tempo` (autocorrelation-based beat
//! tracking). See ADR-0027.
//!
//! This is the third tracking mode (alongside Reactive and Lookahead
//! Consensus). Where Reactive feeds raw onset times to a reactive
//! period-smoothing PLL, and Consensus snaps the PLL from a median-IOI
//! window of onsets, AubioTempo replaces the entire onset-detection
//! stage with aubio's built-in `Tempo` object: it does its own onset
//! detection internally, runs autocorrelation across a tempogram-like
//! state, and emits per-beat events at the inferred beat boundaries.
//!
//! Each emitted beat snaps the PLL (period from inter-beat interval,
//! phase from beat sample), so `PulseGenerator` and downstream
//! (LED/MIDI/Link) consumers stay unchanged.

use aubio_rs::{OnsetMode, Tempo};

use crate::dsp::beat_pll::BeatPll;
use crate::params::OnsetMethod;

const BUF_SIZE: usize = 1024;
const HOP_SIZE: usize = 512;
/// `BeatPll`'s clamp range, mirrored for snap arithmetic.
const MIN_BPM: f64 = 60.0;
const MAX_BPM: f64 = 220.0;

pub struct AubioTempoTracker {
    tempo: Tempo,
    sample_rate: u32,
    method: OnsetMethod,
    threshold: f32,

    accum: [f32; HOP_SIZE],
    accum_len: usize,

    /// Total samples that have been pushed into aubio so far.
    aubio_frames: u64,

    /// Most recent beat sample reported by aubio (absolute, sample-rate
    /// units, as projected onto the host's clock).
    last_beat_sample: Option<u64>,
}

// SAFETY: aubio C objects are not thread-safe; the audio thread is the
// only thread that touches `AubioTempoTracker`. Mirrors the impl on
// `BeatTracker`.
unsafe impl Send for AubioTempoTracker {}

impl AubioTempoTracker {
    pub fn new(
        sample_rate: u32,
        method: OnsetMethod,
        threshold: f32,
    ) -> Result<Self, &'static str> {
        let tempo = Tempo::new(method_to_aubio(method), BUF_SIZE, HOP_SIZE, sample_rate)
            .map_err(|_| "aubio Tempo::new failed")?;
        let mut me = Self {
            tempo,
            sample_rate,
            method,
            threshold,
            accum: [0.0; HOP_SIZE],
            accum_len: 0,
            aubio_frames: 0,
            last_beat_sample: None,
        };
        me.set_threshold(threshold);
        Ok(me)
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
        self.tempo.set_threshold(threshold);
    }

    pub fn set_method(&mut self, method: OnsetMethod) -> Result<(), &'static str> {
        if method == self.method {
            return Ok(());
        }
        let new_tempo = Tempo::new(
            method_to_aubio(method),
            BUF_SIZE,
            HOP_SIZE,
            self.sample_rate,
        )
        .map_err(|_| "aubio Tempo::new failed")?;
        self.tempo = new_tempo;
        self.method = method;
        self.tempo.set_threshold(self.threshold);
        self.aubio_frames = 0;
        self.accum_len = 0;
        self.last_beat_sample = None;
        Ok(())
    }

    /// Reset internal state. Called on sample-rate change or manual
    /// resync.
    pub fn reset(&mut self) {
        self.accum_len = 0;
        self.aubio_frames = 0;
        self.last_beat_sample = None;
    }

    /// Process a mono block of host samples. For each detected beat,
    /// invoke `on_beat(block_offset, abs_sample)`. The caller is
    /// responsible for snapping the PLL at the right per-sample
    /// position via `snap_pll_at_beat`. Allocation-free.
    pub fn process_block<F: FnMut(u32, u64)>(
        &mut self,
        block: &[f32],
        block_start_abs: u64,
        mut on_beat: F,
    ) {
        let mut i = 0usize;
        while i < block.len() {
            let space = HOP_SIZE - self.accum_len;
            let take = space.min(block.len() - i);
            self.accum[self.accum_len..self.accum_len + take].copy_from_slice(&block[i..i + take]);
            self.accum_len += take;
            i += take;

            if self.accum_len == HOP_SIZE {
                let hop_end_block_offset = i;
                self.consume_hop(hop_end_block_offset as u32, block_start_abs, &mut on_beat);
            }
        }
    }

    fn consume_hop<F: FnMut(u32, u64)>(
        &mut self,
        hop_end_block_offset: u32,
        block_start_abs: u64,
        on_beat: &mut F,
    ) {
        let hop_start_aubio_frame = self.aubio_frames;
        let result = self.tempo.do_result(&self.accum[..]);
        self.aubio_frames += HOP_SIZE as u64;
        self.accum_len = 0;
        let Ok(out) = result else {
            return;
        };
        if out <= 0.0 {
            return;
        }
        let beat_aubio = self.tempo.get_last() as u64;
        let beat_within_hop = beat_aubio.saturating_sub(hop_start_aubio_frame);
        let beat_block_offset =
            (hop_end_block_offset as i64 - HOP_SIZE as i64 + beat_within_hop as i64).max(0) as u32;
        let beat_abs = block_start_abs + beat_block_offset as u64;
        on_beat(beat_block_offset, beat_abs);
    }

    /// Snap `pll` from a beat at absolute sample `beat_abs`. Computes
    /// period from the inter-beat interval (with octave correction) if
    /// we have a previous beat; otherwise locks phase only and keeps
    /// the existing period until the next beat establishes one.
    pub fn snap_pll_at_beat(&mut self, pll: &mut BeatPll, beat_abs: u64) {
        if let Some(prev) = self.last_beat_sample {
            let raw_period = (beat_abs as f64) - (prev as f64);
            if raw_period > 0.0 {
                pll.period_samples = octave_correct_period(raw_period, self.sample_rate as f64);
            }
        }
        pll.last_onset_sample = Some(beat_abs as f64);
        pll.phase_samples = 0.0;
        pll.locked = true;
        self.last_beat_sample = Some(beat_abs);
    }
}

/// Halve / double until the period falls in the [MIN_BPM, MAX_BPM]
/// range. Aubio's `Tempo` already does its own octave selection, but a
/// safety net here matches the rest of the DSP's clamp behaviour and
/// keeps `BeatPll` happy.
fn octave_correct_period(mut period: f64, sample_rate: f64) -> f64 {
    let min_period = sample_rate * 60.0 / MAX_BPM;
    let max_period = sample_rate * 60.0 / MIN_BPM;
    for _ in 0..6 {
        if period < min_period {
            period *= 2.0;
        } else if period > max_period {
            period *= 0.5;
        } else {
            break;
        }
    }
    period
}

fn method_to_aubio(m: OnsetMethod) -> OnsetMode {
    match m {
        OnsetMethod::Hfc => OnsetMode::Hfc,
        OnsetMethod::Complex => OnsetMode::Complex,
        OnsetMethod::SpecDiff => OnsetMode::SpecDiff,
        OnsetMethod::Kl => OnsetMode::Kl,
        OnsetMethod::Mkl => OnsetMode::Mkl,
        OnsetMethod::Phase => OnsetMode::Phase,
        OnsetMethod::SpecFlux => OnsetMode::SpecFlux,
    }
}
