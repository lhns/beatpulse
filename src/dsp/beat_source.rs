// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! `BeatSource` enum unifying the three tracking modes.
//!
//! Each mode owns its own front-of-pipeline state (onset detector or
//! aubio Tempo) plus its own pulse emitter. `lib.rs::process` calls
//! `BeatSource::process_block` once per host block to ingest audio and
//! schedule events, then `BeatSource::tick` per sample to advance the
//! PLL phase and emit pulses. The `BeatPll` itself stays in
//! `DspState` because the UI/Link consume `current_bpm()` from it.

use crate::dsp::aubio_pulse_emitter::AubioPulseEmitter;
use crate::dsp::aubio_tempo_tracker::AubioTempoTracker;
use crate::dsp::beat_pll::BeatPll;
use crate::dsp::beat_tracker::BeatTracker;
use crate::dsp::consensus_tracker::ConsensusTracker;
use crate::dsp::klapuri::KlapuriTracker;
use crate::dsp::pulse_generator::{PulseEvent, PulseGenerator};
use crate::params::{OnsetMethod, TrackingMode};

/// Maximum onsets / beats per host block. 16 covers ~24 PPQN at 240 BPM
/// inside a 1024-sample block (~6 onsets at most), well below the cap.
const MAX_EVENTS_PER_BLOCK: usize = 16;

pub enum BeatSource {
    Reactive(ReactiveSource),
    Consensus(ConsensusSource),
    AubioTempo(AubioTempoSource),
    Klapuri(KlapuriSource),
}

impl BeatSource {
    pub fn new(
        mode: TrackingMode,
        sr: u32,
        onset_method: OnsetMethod,
        threshold: f32,
        lookahead_ms: f64,
        pulse_rate: u32,
    ) -> Result<Self, &'static str> {
        Ok(match mode {
            TrackingMode::Reactive => BeatSource::Reactive(ReactiveSource::new(
                sr,
                onset_method,
                threshold,
                pulse_rate,
            )?),
            TrackingMode::LookaheadConsensus => BeatSource::Consensus(ConsensusSource::new(
                sr,
                onset_method,
                threshold,
                lookahead_ms,
                pulse_rate,
            )?),
            TrackingMode::AubioTempo => BeatSource::AubioTempo(AubioTempoSource::new(
                sr,
                onset_method,
                threshold,
                pulse_rate,
            )?),
            TrackingMode::Klapuri => BeatSource::Klapuri(KlapuriSource::new(sr, pulse_rate)),
        })
    }

    pub fn mode(&self) -> TrackingMode {
        match self {
            BeatSource::Reactive(_) => TrackingMode::Reactive,
            BeatSource::Consensus(_) => TrackingMode::LookaheadConsensus,
            BeatSource::AubioTempo(_) => TrackingMode::AubioTempo,
            BeatSource::Klapuri(_) => TrackingMode::Klapuri,
        }
    }

    pub fn reset(&mut self) {
        match self {
            BeatSource::Reactive(s) => s.reset(),
            BeatSource::Consensus(s) => s.reset(),
            BeatSource::AubioTempo(s) => s.reset(),
            BeatSource::Klapuri(s) => s.reset(),
        }
    }

    pub fn set_threshold(&mut self, t: f32) {
        match self {
            BeatSource::Reactive(s) => s.tracker.set_threshold(t),
            BeatSource::Consensus(s) => s.tracker.set_threshold(t),
            BeatSource::AubioTempo(s) => s.tracker.set_threshold(t),
            // Klapuri's accent stage is internal; threshold is a no-op.
            BeatSource::Klapuri(_) => {}
        }
    }

    pub fn set_method(&mut self, m: OnsetMethod) -> Result<(), &'static str> {
        match self {
            BeatSource::Reactive(s) => s.tracker.set_method(m),
            BeatSource::Consensus(s) => s.tracker.set_method(m),
            BeatSource::AubioTempo(s) => s.tracker.set_method(m),
            // Klapuri uses its own multi-band STFT-based accent —
            // the OnsetMethod selector doesn't apply.
            BeatSource::Klapuri(_) => Ok(()),
        }
    }

    pub fn set_pulse_rate(&mut self, rate: u32) {
        match self {
            BeatSource::Reactive(s) => s.pulse_gen.set_pulse_rate(rate),
            BeatSource::Consensus(s) => s.pulse_gen.set_pulse_rate(rate),
            BeatSource::AubioTempo(s) => s.pulse.set_pulse_rate(rate),
            BeatSource::Klapuri(s) => s.pulse.set_pulse_rate(rate),
        }
    }

    pub fn set_lookahead_ms(&mut self, ms: f64) {
        if let BeatSource::Consensus(s) = self {
            s.consensus.set_window_ms(ms);
        }
    }

    /// Ingest one host block. After this returns, the source is ready
    /// for the per-sample `tick` loop.
    pub fn process_block(&mut self, mono: &[f32], block_start_abs: u64, pll: &mut BeatPll) {
        match self {
            BeatSource::Reactive(s) => s.process_block(mono, block_start_abs),
            BeatSource::Consensus(s) => s.process_block(mono, block_start_abs, pll),
            BeatSource::AubioTempo(s) => s.process_block(mono, block_start_abs),
            BeatSource::Klapuri(s) => s.process_block(mono, block_start_abs),
        }
    }

    /// Per-sample step. `i` is the within-block sample offset;
    /// `abs_sample` is the host-clock absolute sample. Applies any
    /// scheduled events at `i`, advances `pll` by one sample, returns
    /// a `PulseEvent` if one fires.
    pub fn tick(&mut self, i: u32, abs_sample: u64, pll: &mut BeatPll) -> Option<PulseEvent> {
        match self {
            BeatSource::Reactive(s) => s.tick(i, abs_sample, pll),
            BeatSource::Consensus(s) => s.tick(i, abs_sample, pll),
            BeatSource::AubioTempo(s) => s.tick(i, abs_sample, pll),
            BeatSource::Klapuri(s) => s.tick(i, abs_sample, pll),
        }
    }
}

// ----- Reactive ------------------------------------------------------

pub struct ReactiveSource {
    pub tracker: BeatTracker,
    pub pulse_gen: PulseGenerator,
    onsets: [(u32, u64); MAX_EVENTS_PER_BLOCK],
    n_onsets: usize,
    next: usize,
}

impl ReactiveSource {
    pub fn new(
        sr: u32,
        onset_method: OnsetMethod,
        threshold: f32,
        pulse_rate: u32,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            tracker: BeatTracker::new(sr, onset_method, threshold)?,
            pulse_gen: PulseGenerator::new(pulse_rate),
            onsets: [(0, 0); MAX_EVENTS_PER_BLOCK],
            n_onsets: 0,
            next: 0,
        })
    }

    pub fn reset(&mut self) {
        self.pulse_gen.reset();
        self.n_onsets = 0;
        self.next = 0;
    }

    pub fn process_block(&mut self, mono: &[f32], block_start_abs: u64) {
        self.n_onsets = 0;
        self.next = 0;
        let onsets = &mut self.onsets;
        let n_onsets = &mut self.n_onsets;
        self.tracker.process_block(mono, |offset, _frac| {
            if *n_onsets < MAX_EVENTS_PER_BLOCK {
                onsets[*n_onsets] = (offset, block_start_abs + offset as u64);
                *n_onsets += 1;
            }
        });
    }

    pub fn tick(&mut self, i: u32, _abs_sample: u64, pll: &mut BeatPll) -> Option<PulseEvent> {
        while self.next < self.n_onsets && self.onsets[self.next].0 == i {
            pll.on_onset(self.onsets[self.next].1 as f64);
            self.next += 1;
        }
        pll.advance_one();
        self.pulse_gen.observe_advance(pll, i)
    }
}

// ----- Lookahead Consensus -------------------------------------------

pub struct ConsensusSource {
    pub tracker: BeatTracker,
    pub consensus: ConsensusTracker,
    pub pulse_gen: PulseGenerator,
    onsets: [(u32, u64); MAX_EVENTS_PER_BLOCK],
    n_onsets: usize,
}

impl ConsensusSource {
    pub fn new(
        sr: u32,
        onset_method: OnsetMethod,
        threshold: f32,
        lookahead_ms: f64,
        pulse_rate: u32,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            tracker: BeatTracker::new(sr, onset_method, threshold)?,
            consensus: ConsensusTracker::new(sr as f64, lookahead_ms),
            pulse_gen: PulseGenerator::new(pulse_rate),
            onsets: [(0, 0); MAX_EVENTS_PER_BLOCK],
            n_onsets: 0,
        })
    }

    pub fn reset(&mut self) {
        self.consensus.reset();
        self.pulse_gen.reset();
        self.n_onsets = 0;
    }

    pub fn process_block(&mut self, mono: &[f32], block_start_abs: u64, pll: &mut BeatPll) {
        self.n_onsets = 0;
        let onsets = &mut self.onsets;
        let n_onsets = &mut self.n_onsets;
        self.tracker.process_block(mono, |offset, _frac| {
            if *n_onsets < MAX_EVENTS_PER_BLOCK {
                onsets[*n_onsets] = (offset, block_start_abs + offset as u64);
                *n_onsets += 1;
            }
        });
        // Lookahead: ingest all onsets up-front, then snap the PLL via
        // `try_snap_pll`. The per-sample `tick` only advances phase.
        for &(_, abs) in &self.onsets[..self.n_onsets] {
            self.consensus.on_onset(abs);
        }
        self.consensus
            .try_snap_pll(pll, block_start_abs, mono.len() as u64);
    }

    pub fn tick(&mut self, i: u32, _abs_sample: u64, pll: &mut BeatPll) -> Option<PulseEvent> {
        pll.advance_one();
        self.pulse_gen.observe_advance(pll, i)
    }
}

// ----- AubioTempo ----------------------------------------------------

pub struct AubioTempoSource {
    pub tracker: AubioTempoTracker,
    pub pulse: AubioPulseEmitter,
    beats: [(u32, u64); MAX_EVENTS_PER_BLOCK],
    n_beats: usize,
    next: usize,
}

impl AubioTempoSource {
    pub fn new(
        sr: u32,
        onset_method: OnsetMethod,
        threshold: f32,
        pulse_rate: u32,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            tracker: AubioTempoTracker::new(sr, onset_method, threshold)?,
            pulse: AubioPulseEmitter::new(pulse_rate),
            beats: [(0, 0); MAX_EVENTS_PER_BLOCK],
            n_beats: 0,
            next: 0,
        })
    }

    pub fn reset(&mut self) {
        self.pulse.reset();
        self.n_beats = 0;
        self.next = 0;
    }

    pub fn process_block(&mut self, mono: &[f32], block_start_abs: u64) {
        self.n_beats = 0;
        self.next = 0;
        let beats = &mut self.beats;
        let n_beats = &mut self.n_beats;
        self.tracker
            .process_block(mono, block_start_abs, |offset, abs_sample| {
                if *n_beats < MAX_EVENTS_PER_BLOCK {
                    beats[*n_beats] = (offset, abs_sample);
                    *n_beats += 1;
                }
            });
    }

    pub fn tick(&mut self, i: u32, abs_sample: u64, pll: &mut BeatPll) -> Option<PulseEvent> {
        let mut on_beat: Option<PulseEvent> = None;
        while self.next < self.n_beats && self.beats[self.next].0 == i {
            self.tracker.snap_pll_at_beat(pll, self.beats[self.next].1);
            if let Some(mut ev) = self.pulse.on_beat(self.beats[self.next].1) {
                ev.sample_offset = i;
                on_beat = Some(ev);
            }
            self.next += 1;
        }
        pll.advance_one();
        on_beat.or_else(|| {
            self.pulse.tick(abs_sample).map(|mut ev| {
                ev.sample_offset = i;
                ev
            })
        })
    }
}

// ----- Klapuri -------------------------------------------------------

/// Range BeatPLL accepts, mirrored from `aubio_tempo_tracker.rs` /
/// `beat_pll.rs`. Used by the Klapuri snap to fold the inferred
/// period into a sensible BPM range before handing it to the PLL.
const KLAPURI_MIN_BPM: f64 = 60.0;
const KLAPURI_MAX_BPM: f64 = 220.0;

pub struct KlapuriSource {
    pub tracker: KlapuriTracker,
    pub pulse: AubioPulseEmitter,
    sample_rate: u32,
    beats: [(u32, u64); MAX_EVENTS_PER_BLOCK],
    n_beats: usize,
    next: usize,
    last_beat_sample: Option<u64>,
}

impl KlapuriSource {
    pub fn new(sr: u32, pulse_rate: u32) -> Self {
        Self {
            tracker: KlapuriTracker::new(sr),
            pulse: AubioPulseEmitter::new(pulse_rate),
            sample_rate: sr,
            beats: [(0, 0); MAX_EVENTS_PER_BLOCK],
            n_beats: 0,
            next: 0,
            last_beat_sample: None,
        }
    }

    pub fn reset(&mut self) {
        self.tracker.reset();
        self.pulse.reset();
        self.n_beats = 0;
        self.next = 0;
        self.last_beat_sample = None;
    }

    pub fn process_block(&mut self, mono: &[f32], block_start_abs: u64) {
        self.n_beats = 0;
        self.next = 0;
        let beats = &mut self.beats;
        let n_beats = &mut self.n_beats;
        self.tracker
            .process_block(mono, block_start_abs, |offset, abs_sample| {
                if *n_beats < MAX_EVENTS_PER_BLOCK {
                    beats[*n_beats] = (offset, abs_sample);
                    *n_beats += 1;
                }
            });
    }

    /// Snap `pll` from a Klapuri beat. Period comes from the tracker's
    /// current locked tactus (in OSS frames × hop), with octave
    /// correction. Mirrors `AubioTempoTracker::snap_pll_at_beat`'s
    /// shape so the downstream PLL behaviour is consistent across
    /// tracking modes.
    fn snap_pll_at_beat(&mut self, pll: &mut BeatPll, beat_abs: u64) {
        let tau_oss = self.tracker.current_period_oss();
        if tau_oss > 0 {
            let raw_period = (tau_oss * self.tracker.hop()) as f64;
            pll.period_samples = octave_correct_period(raw_period, self.sample_rate as f64);
        } else if let Some(prev) = self.last_beat_sample {
            // Fall back to inter-beat interval if the tracker hasn't
            // exposed a locked period yet.
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

    pub fn tick(&mut self, i: u32, abs_sample: u64, pll: &mut BeatPll) -> Option<PulseEvent> {
        let mut on_beat: Option<PulseEvent> = None;
        while self.next < self.n_beats && self.beats[self.next].0 == i {
            let beat_abs = self.beats[self.next].1;
            self.snap_pll_at_beat(pll, beat_abs);
            if let Some(mut ev) = self.pulse.on_beat(beat_abs) {
                ev.sample_offset = i;
                on_beat = Some(ev);
            }
            self.next += 1;
        }
        pll.advance_one();
        on_beat.or_else(|| {
            self.pulse.tick(abs_sample).map(|mut ev| {
                ev.sample_offset = i;
                ev
            })
        })
    }
}

fn octave_correct_period(mut period: f64, sample_rate: f64) -> f64 {
    let min_period = sample_rate * 60.0 / KLAPURI_MAX_BPM;
    let max_period = sample_rate * 60.0 / KLAPURI_MIN_BPM;
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
