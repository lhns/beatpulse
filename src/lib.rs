// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! BeatPulse — real-time beat detector VST3/CLAP plugin.
//!
//! See `BeatPulse-SPEC.md` for the full design and `docs/adr/` for the
//! architecture decisions behind it.

use std::num::NonZeroU32;
use std::sync::Arc;

use nih_plug::prelude::*;

pub mod dsp;
pub mod eval;
pub mod link;
pub mod midi;
pub mod params;
pub mod shared;
pub mod ui;

use crate::params::BeatpulseParams;
use crate::shared::SharedState;

pub struct Beatpulse {
    params: Arc<BeatpulseParams>,
    shared: Arc<SharedState>,
}

impl Default for Beatpulse {
    fn default() -> Self {
        Self {
            params: Arc::new(BeatpulseParams::default()),
            shared: Arc::new(SharedState::default()),
        }
    }
}

impl Plugin for Beatpulse {
    const NAME: &'static str = "BeatPulse";
    const VENDOR: &'static str = "lhns";
    const URL: &'static str = "https://github.com/lhns/beatpulse";
    const EMAIL: &'static str = "noreply@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[],
            aux_output_ports: &[],
            names: PortNames::const_default(),
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            aux_input_ports: &[],
            aux_output_ports: &[],
            names: PortNames::const_default(),
        },
    ];

    const MIDI_INPUT: MidiConfig = MidiConfig::None;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Step 1 (spec §11): no-op passthrough. The buffer is left untouched
        // (output == input by construction). DSP/MIDI/Link wiring lands in
        // step 7 (D1 in the project plan).
        let _ = (&self.params, &self.shared);
        ProcessStatus::Normal
    }

    fn deactivate(&mut self) {}
}

impl ClapPlugin for Beatpulse {
    const CLAP_ID: &'static str = "de.lhns.beatpulse";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Real-time beat detector with Ableton Link tempo broadcast");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Analyzer,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for Beatpulse {
    // Must be exactly 16 bytes. Stable identifier — do not change after release.
    const VST3_CLASS_ID: [u8; 16] = *b"BeatPulseLhnsv01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer, Vst3SubCategory::Tools];
}

nih_export_clap!(Beatpulse);
nih_export_vst3!(Beatpulse);
