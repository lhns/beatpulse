// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Plugin parameters. See `BeatPulse-SPEC.md` §7.

use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;

use crate::ui;

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum OnsetMethod {
    Hfc,
    Complex,
    SpecDiff,
    Kl,
    Mkl,
    Phase,
    #[name = "Spectral Flux"]
    SpecFlux,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum PulseRate {
    #[name = "1 PPQN"]
    Ppqn1,
    #[name = "2 PPQN"]
    Ppqn2,
    #[name = "4 PPQN"]
    Ppqn4,
    #[name = "8 PPQN"]
    Ppqn8,
    #[name = "16 PPQN"]
    Ppqn16,
    #[name = "24 PPQN"]
    Ppqn24,
}

impl PulseRate {
    pub fn as_u32(self) -> u32 {
        match self {
            PulseRate::Ppqn1 => 1,
            PulseRate::Ppqn2 => 2,
            PulseRate::Ppqn4 => 4,
            PulseRate::Ppqn8 => 8,
            PulseRate::Ppqn16 => 16,
            PulseRate::Ppqn24 => 24,
        }
    }
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum MsgType {
    Cc,
    Note,
    Both,
}

#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum CcValueMode {
    #[name = "Fixed 127"]
    Fixed127,
    #[name = "Fixed Custom"]
    FixedCustom,
    #[name = "Toggle 127/0"]
    Toggle127_0,
}

/// Tracking mode — selects how the audio feeds the PLL. See ADR-0026
/// (Lookahead Consensus), ADR-0027 (Aubio Tempo), and ADR-0028
/// (Klapuri 2006 multi-band tracker).
#[derive(Enum, PartialEq, Eq, Clone, Copy, Debug)]
pub enum TrackingMode {
    /// Aubio's `Tempo` object (autocorrelation-based beat tracker).
    /// Default — F=0.59 / AMLt=0.46 / TA2=0.78 on Ballroom. Best
    /// continuous-tracking metric (AMLt) of the available trackers,
    /// which matters for visible light cueing — fewer "loses lock
    /// mid-song" failures than Klapuri. Same per-block latency as
    /// Reactive (single hop ≈ 11.6 ms at 44.1 kHz). See ADR-0027.
    #[name = "Aubio Tempo"]
    AubioTempo,
    /// Per-onset PLL feedback against `aubio::Onset` events. Lowest
    /// latency, but reactive period smoothing locks onto the wrong
    /// tempo on most full-mix material (F=0.29 on Ballroom).
    /// Retained as a baseline / for very clean drum-bus sources.
    #[name = "Reactive"]
    Reactive,
    /// Buffer onsets over a window, derive consensus period from
    /// median IOI, snap PLL periodically. Higher latency (= the
    /// configured Lookahead window), F=0.44 on Ballroom. Useful when
    /// `AubioTempo` mistracks a particular full-mix source.
    #[name = "Lookahead Consensus"]
    LookaheadConsensus,
    /// Klapuri 2006 multi-band comb-resonator tracker. F=0.63 /
    /// AMLt=0.42 / TA2=0.79 on Ballroom — *beats aubio on F-measure
    /// and TA2 by +0.04 / +0.01* (tighter beat alignment when
    /// locked) but **trails on AMLt by 0.04** (loses lock more
    /// often mid-song). Try it when AubioTempo's beat placement
    /// feels slightly behind the music; stay on AubioTempo if you
    /// see lights desync mid-song. Higher CPU cost (STFT + 4-band
    /// accent + 150-resonator comb bank). Onset method / Sensitivity
    /// have no effect — Klapuri's accent stage is internal. See
    /// ADR-0028.
    #[name = "Klapuri"]
    Klapuri,
}

#[derive(Params)]
pub struct BeatpulseParams {
    /// Persisted egui editor state (window size, etc.).
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[id = "sens"]
    pub sensitivity: FloatParam,

    /// Decoupled period-smoothing control. 0 = jittery & fast (high α),
    /// 1 = smooth & slow (low α). Default 0.5 ≈ historical α_period.
    #[id = "stab"]
    pub tempo_stability: FloatParam,

    /// Output timing offset in milliseconds. Positive = pulses fire later
    /// than the predicted beat sample. Negative = earlier. Used to
    /// compensate for end-to-end audio→listener latency so DMX cues
    /// align with the perceived beat.
    #[id = "latMs"]
    pub latency_offset_ms: FloatParam,

    /// Selects per-onset PLL feedback (default) or buffered consensus.
    /// See ADR-0026.
    #[id = "trkMd"]
    pub tracking_mode: EnumParam<TrackingMode>,

    /// Sliding window for the consensus tracker. Effective only when
    /// `tracking_mode == LookaheadConsensus`. Adds approximately this
    /// much latency to BPM/beat output; compensate via
    /// `latency_offset_ms`.
    #[id = "lookMs"]
    pub lookahead_ms: FloatParam,

    #[id = "onset"]
    pub onset_method: EnumParam<OnsetMethod>,

    #[id = "silTh"]
    pub silence_threshold: FloatParam,

    #[id = "silRl"]
    pub silence_release: FloatParam,

    #[id = "pulse"]
    pub pulse_rate: EnumParam<PulseRate>,

    #[id = "linkOn"]
    pub link_enabled: BoolParam,

    #[id = "midiOn"]
    pub midi_enabled: BoolParam,

    #[id = "msgT"]
    pub msg_type: EnumParam<MsgType>,

    #[id = "ccNum"]
    pub cc_number: IntParam,

    #[id = "ccMode"]
    pub cc_value_mode: EnumParam<CcValueMode>,

    #[id = "ccVal"]
    pub cc_value: IntParam,

    #[id = "noteN"]
    pub note_number: IntParam,

    #[id = "noteV"]
    pub note_velocity: IntParam,

    #[id = "noteL"]
    pub note_length_ms: FloatParam,

    #[id = "chan"]
    pub midi_channel: IntParam,

    /// Manual resync: edge-triggered. The audio thread reads this each block;
    /// when it transitions false→true the PLL resets on the next onset.
    #[id = "resync"]
    pub manual_resync: BoolParam,
}

impl Default for BeatpulseParams {
    fn default() -> Self {
        Self {
            editor_state: ui::default_state(),
            sensitivity: FloatParam::new(
                "Sensitivity",
                0.5,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            tempo_stability: FloatParam::new(
                "Tempo stability",
                0.5,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            latency_offset_ms: FloatParam::new(
                "Latency offset",
                0.0,
                FloatRange::Linear {
                    min: -200.0,
                    max: 200.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(1.0),

            tracking_mode: EnumParam::new("Tracking Mode", TrackingMode::AubioTempo),

            lookahead_ms: FloatParam::new(
                "Lookahead",
                2000.0,
                FloatRange::Linear {
                    min: 200.0,
                    max: 3000.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(50.0),

            onset_method: EnumParam::new("Onset Method", OnsetMethod::SpecFlux),

            silence_threshold: FloatParam::new(
                "Silence Threshold",
                -50.0,
                FloatRange::Linear {
                    min: -90.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_step_size(1.0),

            silence_release: FloatParam::new(
                "Silence Release",
                200.0,
                FloatRange::Linear {
                    min: 50.0,
                    max: 2000.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(1.0),

            pulse_rate: EnumParam::new("Pulse Rate", PulseRate::Ppqn4),

            link_enabled: BoolParam::new("Link Enabled", true),
            midi_enabled: BoolParam::new("MIDI Enabled", true),

            msg_type: EnumParam::new("Message Type", MsgType::Cc),

            cc_number: IntParam::new("CC Number", 16, IntRange::Linear { min: 0, max: 127 }),

            cc_value_mode: EnumParam::new("CC Value Mode", CcValueMode::Fixed127),

            cc_value: IntParam::new("CC Value", 127, IntRange::Linear { min: 0, max: 127 }),

            note_number: IntParam::new("Note", 60, IntRange::Linear { min: 0, max: 127 }),

            note_velocity: IntParam::new(
                "Note Velocity",
                100,
                IntRange::Linear { min: 1, max: 127 },
            ),

            note_length_ms: FloatParam::new(
                "Note Length",
                10.0,
                FloatRange::Linear {
                    min: 1.0,
                    max: 100.0,
                },
            )
            .with_unit(" ms"),

            midi_channel: IntParam::new("MIDI Channel", 1, IntRange::Linear { min: 1, max: 16 }),

            manual_resync: BoolParam::new("Resync", false),
        }
    }
}

impl BeatpulseParams {
    /// Sensitivity-derived tunables. See spec §7.
    pub fn aubio_threshold(&self) -> f32 {
        let s = self.sensitivity.value();
        lerp(0.1, 1.0, 1.0 - s)
    }

    pub fn alpha_period(&self) -> f64 {
        // Decoupled from sensitivity (per ADR-0024). 0 = jittery (high α),
        // 1 = smooth (low α). The midpoint α=0.11 is close to the
        // historical default of 0.09 so existing presets behave similarly.
        let s = self.tempo_stability.value() as f64;
        lerp64(0.20, 0.02, s)
    }

    pub fn alpha_phase(&self) -> f64 {
        let s = self.sensitivity.value() as f64;
        lerp64(0.08, 0.25, s)
    }
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn lerp64(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn defaults_match_spec() {
        let p = BeatpulseParams::default();
        assert_eq!(p.sensitivity.value(), 0.5);
        assert_eq!(p.tempo_stability.value(), 0.5);
        assert_eq!(p.latency_offset_ms.value(), 0.0);
        assert_eq!(p.tracking_mode.value(), TrackingMode::AubioTempo);
        assert_eq!(p.lookahead_ms.value(), 2000.0);
        assert_eq!(p.onset_method.value(), OnsetMethod::SpecFlux);
        assert_eq!(p.silence_threshold.value(), -50.0);
        assert_eq!(p.silence_release.value(), 200.0);
        assert_eq!(p.pulse_rate.value(), PulseRate::Ppqn4);
        assert!(p.link_enabled.value());
        assert!(p.midi_enabled.value());
        assert_eq!(p.msg_type.value(), MsgType::Cc);
        assert_eq!(p.cc_number.value(), 16);
        assert_eq!(p.cc_value_mode.value(), CcValueMode::Fixed127);
        assert_eq!(p.cc_value.value(), 127);
        assert_eq!(p.note_number.value(), 60);
        assert_eq!(p.note_velocity.value(), 100);
        assert_eq!(p.note_length_ms.value(), 10.0);
        assert_eq!(p.midi_channel.value(), 1);
        assert!(!p.manual_resync.value());
    }

    /// Spec §7 sensitivity LUT must hit the documented endpoints.
    #[test]
    fn sensitivity_lut_endpoints() {
        let p = BeatpulseParams::default();

        // sensitivity = 0.5 (default)
        assert_relative_eq!(p.aubio_threshold(), 0.55, epsilon = 1e-4);
        assert_relative_eq!(p.alpha_phase(), 0.165, epsilon = 1e-4);
        // tempo_stability = 0.5 → α_period = midpoint of 0.20..0.02
        assert_relative_eq!(p.alpha_period(), 0.11, epsilon = 1e-4);
    }

    #[test]
    fn pulse_rate_numeric() {
        assert_eq!(PulseRate::Ppqn1.as_u32(), 1);
        assert_eq!(PulseRate::Ppqn24.as_u32(), 24);
    }
}
