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

use crate::dsp::beat_pll::BeatPll;
use crate::dsp::beat_tracker::BeatTracker;
use crate::dsp::pulse_generator::PulseGenerator;
use crate::dsp::silence_gate::{SilenceGate, Transition};
use crate::link::publisher::LinkPublisher;
use crate::midi::formatter::{FormatterConfig, MidiCommand, MidiFormatter};
use crate::params::{BeatpulseParams, OnsetMethod};
use crate::shared::{LinkStatus, SharedState};

/// All allocated DSP resources. Constructed in `initialize`, taken down in
/// `deactivate`.
struct DspState {
    sample_rate: f32,
    beat_tracker: BeatTracker,
    silence_gate: SilenceGate,
    pll: BeatPll,
    pulse_gen: PulseGenerator,
    midi_formatter: MidiFormatter,
    link: LinkPublisher,

    /// Mono scratch buffer big enough for any host block. Sized in
    /// `initialize` from `BufferConfig::max_buffer_size`.
    mono_scratch: Vec<f32>,

    /// Output queue for MIDI commands collected during `process`. Sized
    /// once with a generous upper bound — must not grow during `process`.
    midi_out: Vec<MidiCommand>,

    /// Absolute sample counter — used to schedule note-offs across block
    /// boundaries.
    absolute_sample: u64,

    /// State for `manual_resync` edge detection.
    last_resync_pressed: bool,

    /// Last applied onset method (so we recreate aubio only on change).
    last_method: OnsetMethod,
}

pub struct Beatpulse {
    params: Arc<BeatpulseParams>,
    shared: Arc<SharedState>,
    dsp: Option<DspState>,
}

impl Default for Beatpulse {
    fn default() -> Self {
        Self {
            params: Arc::new(BeatpulseParams::default()),
            shared: Arc::new(SharedState::default()),
            dsp: None,
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

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        ui::editor(
            self.params.editor_state.clone(),
            self.params.clone(),
            self.shared.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate;
        let max_block = buffer_config.max_buffer_size as usize;

        let onset_method = self.params.onset_method.value();
        let beat_tracker = match BeatTracker::new(
            sr as u32,
            onset_method,
            self.params.aubio_threshold(),
        ) {
            Ok(t) => t,
            Err(e) => {
                nih_log!("BeatTracker init failed: {e}");
                return false;
            }
        };

        let silence_gate = SilenceGate::new(
            sr as f64,
            self.params.silence_threshold.value() as f64,
            self.params.silence_release.value() as f64,
        );

        let mut pll = BeatPll::new(sr as f64);
        pll.set_alphas(self.params.alpha_period(), self.params.alpha_phase());

        let pulse_gen = PulseGenerator::new(self.params.pulse_rate.value().as_u32());
        let midi_formatter = MidiFormatter::new();
        let link = LinkPublisher::new(120.0);

        // Generous capacity: one MIDI event per sample at PPQN=24 is wildly
        // beyond reality, but bounded. Reserve up-front so process never
        // allocates.
        let midi_out = Vec::with_capacity(max_block * 3);

        self.dsp = Some(DspState {
            sample_rate: sr,
            beat_tracker,
            silence_gate,
            pll,
            pulse_gen,
            midi_formatter,
            link,
            mono_scratch: vec![0.0; max_block.max(1)],
            midi_out,
            absolute_sample: 0,
            last_resync_pressed: false,
            last_method: onset_method,
        });

        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let n_samples = buffer.samples();
        if n_samples == 0 {
            return ProcessStatus::Normal;
        }

        let params = self.params.clone();
        let shared = self.shared.clone();
        let Some(dsp) = self.dsp.as_mut() else {
            return ProcessStatus::Normal;
        };

        // Audio is left untouched (passthrough). We run analysis off the input.

        // 1. Update DSP tunables that may have moved since the last block.
        update_dsp_from_params(dsp, &params);

        // 2. Manual-resync edge detection.
        let resync_now = params.manual_resync.value();
        if resync_now && !dsp.last_resync_pressed {
            dsp.pll.reset();
            dsp.pulse_gen.reset();
            dsp.midi_formatter.reset();
        }
        dsp.last_resync_pressed = resync_now;

        // 3. Downmix to mono into the preallocated scratch buffer. From here
        // on we use disjoint field borrows of `dsp` (no borrow of `dsp` as a
        // whole) so the borrow on `mono_scratch` doesn't block them.
        let channels = buffer.as_slice_immutable();
        let n_ch = channels.len().max(1) as f32;
        let mono = &mut dsp.mono_scratch[..n_samples];
        for s in mono.iter_mut() {
            *s = 0.0;
        }
        for ch in channels {
            for (i, &s) in ch.iter().enumerate().take(n_samples) {
                mono[i] += s;
            }
        }
        let inv_nch = 1.0 / n_ch;
        for s in mono.iter_mut() {
            *s *= inv_nch;
        }

        // 4. Silence gate: process the block, capture transitions.
        let block_start = dsp.absolute_sample;
        let mut peak = 0.0f32;
        for (i, &s) in mono.iter().enumerate() {
            let mag = s.abs();
            if mag > peak {
                peak = mag;
            }
            let t = dsp.silence_gate.tick(s);
            if let Transition::SilentToActive = t {
                dsp.pll.reset();
                dsp.pulse_gen.reset();
                dsp.midi_formatter.reset();
                let _ = i;
            }
        }
        let active = dsp.silence_gate.is_active();

        // 5. Beat tracker: feed the same mono block, collect onsets.
        // Each onset advances the PLL via on_onset(); but we also need to
        // run the per-sample phase / PulseGenerator over the same block.
        // Strategy: advance the PLL phase + emit pulses for the whole
        // block; for each onset, retroactively call on_onset() at its
        // sample position.
        //
        // Onset positions are reported via callback; we collect into a
        // small fixed array on the stack rather than allocating.
        const MAX_ONSETS_PER_BLOCK: usize = 16;
        let mut onsets: [(u32, u64); MAX_ONSETS_PER_BLOCK] = [(0, 0); MAX_ONSETS_PER_BLOCK];
        let mut n_onsets = 0usize;
        if active {
            let abs = dsp.absolute_sample;
            dsp.beat_tracker.process_block(mono, |offset, _frac| {
                if n_onsets < MAX_ONSETS_PER_BLOCK {
                    onsets[n_onsets] = (offset, abs + offset as u64);
                    n_onsets += 1;
                }
            });
        }

        // 6. Drive the PLL + PulseGenerator over the block, applying onsets
        // at their sample positions and emitting pulses.
        dsp.midi_out.clear();
        // First flush any due note-offs that were scheduled in previous
        // blocks.
        dsp.midi_formatter
            .flush_due_note_offs(block_start, n_samples as u32, &mut dsp.midi_out);

        if active {
            let cfg = formatter_config(&params, dsp.sample_rate);
            let latency_offset_samples =
                (params.latency_offset_ms.value() / 1000.0 * dsp.sample_rate) as i32;
            let mut next_onset = 0usize;
            for i in 0..n_samples as u32 {
                // Apply any onsets landing at this sample, BEFORE advancing.
                while next_onset < n_onsets && onsets[next_onset].0 == i {
                    dsp.pll.on_onset(onsets[next_onset].1 as f64);
                    next_onset += 1;
                }
                // Advance PLL one sample and check for a pulse.
                dsp.pll.advance_one();
                if let Some(mut ev) = dsp.pulse_gen.observe_advance(&dsp.pll, i) {
                    // Drive the UI flash indicators. Independent of output
                    // gating — the LEDs reflect detection.
                    shared.bump_pulse();
                    if ev.is_beat_boundary {
                        shared.bump_beat();
                    }
                    // Apply user latency offset: shift the pulse's
                    // sample_offset, clamped to within this block. For
                    // typical offsets (±50 ms ≈ ±2200 samples) this
                    // covers the calibration use case; larger offsets
                    // get clamped to block edges. See ADR-0023.
                    let shifted =
                        (ev.sample_offset as i32 + latency_offset_samples)
                            .clamp(0, n_samples as i32 - 1) as u32;
                    ev.sample_offset = shifted;
                    if params.midi_enabled.value() {
                        dsp.midi_formatter
                            .on_pulse(ev, block_start, &cfg, &mut dsp.midi_out);
                    }
                }
            }
            // Any onsets at sample==n_samples (rare): apply at end of block.
            while next_onset < n_onsets {
                dsp.pll.on_onset(onsets[next_onset].1 as f64);
                next_onset += 1;
            }
        } else {
            // Silent: still advance PLL phase so it doesn't desync.
            dsp.pll.advance(n_samples as u64);
        }

        // 7. Emit MIDI events to the host bus.
        for cmd in dsp.midi_out.drain(..) {
            let event = midi_command_to_event(cmd);
            context.send_event(event);
        }

        // 8. Publish tempo to Link if locked + active + enabled.
        let link_param = params.link_enabled.value();
        let latency_offset_micros =
            (params.latency_offset_ms.value() * 1000.0) as i64;
        if active && dsp.pll.locked && link_param {
            dsp.link
                .publish_tempo(dsp.pll.current_bpm(), latency_offset_micros);
        } else if !link_param && dsp.link.is_enabled() {
            dsp.link.set_enabled(false);
        } else if link_param && !dsp.link.is_enabled() {
            dsp.link.set_enabled(true);
        }
        let link_status = if !link_param {
            LinkStatus::Off
        } else if active && dsp.pll.locked {
            LinkStatus::Publishing
        } else {
            LinkStatus::Joined
        };
        shared.store_link_status(link_status);

        // 9. Update shared state for the UI.
        let peak_db = if peak > 1e-6 {
            20.0 * (peak as f64).log10()
        } else {
            -120.0
        };
        shared.store_bpm(dsp.pll.current_bpm());
        shared.store_bpm_std_dev(dsp.pll.bpm_std_dev());
        shared.store_locked(dsp.pll.locked);
        shared.store_peak_db(peak_db);
        shared.store_link_peers(dsp.link.num_peers() as usize);
        shared.store_silence_active(!active);

        dsp.absolute_sample = dsp.absolute_sample.wrapping_add(n_samples as u64);

        ProcessStatus::Normal
    }

    fn deactivate(&mut self) {
        self.dsp = None;
    }
}

fn formatter_config(params: &BeatpulseParams, sample_rate: f32) -> FormatterConfig {
    FormatterConfig {
        msg_type: params.msg_type.value(),
        cc_number: params.cc_number.value() as u8,
        cc_value_mode: params.cc_value_mode.value(),
        cc_value: params.cc_value.value() as u8,
        note_number: params.note_number.value() as u8,
        note_velocity: params.note_velocity.value() as u8,
        note_length_samples: ((params.note_length_ms.value() / 1000.0) * sample_rate)
            .max(1.0) as u32,
        midi_channel: params.midi_channel.value() as u8,
    }
}

fn update_dsp_from_params(dsp: &mut DspState, params: &BeatpulseParams) {
    dsp.silence_gate
        .set_threshold_db(params.silence_threshold.value() as f64);
    dsp.silence_gate
        .set_release_ms(params.silence_release.value() as f64);
    dsp.pll
        .set_alphas(params.alpha_period(), params.alpha_phase());
    dsp.pulse_gen
        .set_pulse_rate(params.pulse_rate.value().as_u32());
    dsp.beat_tracker.set_threshold(params.aubio_threshold());

    let onset_method = params.onset_method.value();
    if onset_method != dsp.last_method {
        if dsp.beat_tracker.set_method(onset_method).is_ok() {
            dsp.last_method = onset_method;
        }
    }
}

fn midi_command_to_event(cmd: MidiCommand) -> NoteEvent<()> {
    match cmd {
        MidiCommand::Cc {
            sample_offset,
            channel,
            cc_number,
            value,
        } => NoteEvent::MidiCC {
            timing: sample_offset,
            channel,
            cc: cc_number,
            value: value as f32 / 127.0,
        },
        MidiCommand::NoteOn {
            sample_offset,
            channel,
            note,
            velocity,
        } => NoteEvent::NoteOn {
            timing: sample_offset,
            voice_id: None,
            channel,
            note,
            velocity: velocity as f32 / 127.0,
        },
        MidiCommand::NoteOff {
            sample_offset,
            channel,
            note,
        } => NoteEvent::NoteOff {
            timing: sample_offset,
            voice_id: None,
            channel,
            note,
            velocity: 0.0,
        },
    }
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
    const VST3_CLASS_ID: [u8; 16] = *b"BeatPulseLhnsv01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer, Vst3SubCategory::Tools];
}

nih_export_clap!(Beatpulse);
nih_export_vst3!(Beatpulse);
