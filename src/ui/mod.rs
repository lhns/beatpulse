// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! egui-based plugin editor. See `BeatPulse-SPEC.md` §9.

use std::sync::Arc;
use std::time::Instant;

use nih_plug::prelude::Editor;
use nih_plug_egui::egui::{self, RichText};
use nih_plug_egui::{create_egui_editor, widgets, EguiState};

use crate::params::{BeatpulseParams, MsgType};
use crate::shared::{LinkStatus, SharedState};

/// How long the beat-flash LED stays lit after each detected beat, in ms.
/// 120 ms reads as a clean blink at typical tempos (60–220 BPM).
const BEAT_FLASH_MS: f32 = 120.0;

pub const WINDOW_W: u32 = 480;
pub const WINDOW_H: u32 = 540;

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(WINDOW_W, WINDOW_H)
}

pub fn editor(
    egui_state: Arc<EguiState>,
    params: Arc<BeatpulseParams>,
    shared: Arc<SharedState>,
) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        egui_state,
        (),
        |_, _| {},
        move |egui_ctx, setter, _state| {
            egui::CentralPanel::default().show(egui_ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("BeatPulse");
                ui.add_space(4.0);

                // Status row: lock LED + pulse LED + BPM + peer count
                ui.horizontal(|ui| {
                    let locked = shared.load_locked();
                    let lock_color = if locked {
                        egui::Color32::from_rgb(80, 220, 100)
                    } else {
                        egui::Color32::from_rgb(80, 80, 80)
                    };
                    let circle = "\u{25CF}"; // ●
                    ui.label(
                        RichText::new(circle)
                            .color(lock_color)
                            .size(20.0),
                    );
                    ui.label(if locked { "LOCKED" } else { "—" });

                    ui.separator();

                    // Beat-flash LED. Fires once per detected beat (i.e.
                    // pulses where `is_beat_boundary == true`),
                    // independent of PPQN. Persist last-seen count +
                    // last-flash time across frames via egui's temp data
                    // store.
                    let beat_count = shared.load_beat_count();
                    let id = egui::Id::new("beatpulse_beat_flash");
                    let mut state: (u64, Option<Instant>) = egui_ctx
                        .data(|d| d.get_temp(id))
                        .unwrap_or((0, None));
                    if beat_count != state.0 {
                        state.0 = beat_count;
                        state.1 = Some(Instant::now());
                    }
                    egui_ctx.data_mut(|d| d.insert_temp(id, state));

                    let elapsed_ms = state
                        .1
                        .map(|t| t.elapsed().as_secs_f32() * 1000.0)
                        .unwrap_or(f32::INFINITY);
                    let brightness = (1.0 - elapsed_ms / BEAT_FLASH_MS).clamp(0.0, 1.0);
                    let lerp = |dim: u8, bright: u8| {
                        (dim as f32 + (bright as f32 - dim as f32) * brightness) as u8
                    };
                    let beat_color =
                        egui::Color32::from_rgb(lerp(60, 80), lerp(60, 200), lerp(60, 230));
                    ui.label(
                        RichText::new(circle)
                            .color(beat_color)
                            .size(20.0),
                    );
                    ui.label("BEAT");

                    ui.separator();
                    ui.label(format!("BPM {:.1}", shared.load_bpm()));
                    ui.separator();
                    ui.label(format!("Link peers {}", shared.load_link_peers()));
                    let (link_text, link_color) = match shared.load_link_status() {
                        LinkStatus::Off => (
                            "OFF",
                            egui::Color32::from_rgb(110, 110, 110),
                        ),
                        LinkStatus::Joined => (
                            "JOINED",
                            egui::Color32::from_rgb(220, 180, 60),
                        ),
                        LinkStatus::Publishing => (
                            "PUBLISHING",
                            egui::Color32::from_rgb(80, 220, 100),
                        ),
                    };
                    ui.label(
                        RichText::new(format!("● {link_text}"))
                            .color(link_color),
                    );
                });

                ui.add_space(8.0);

                // Input level meter
                let peak_db = shared.load_peak_db();
                let normalized = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
                ui.label("Input level");
                ui.add(egui::widgets::ProgressBar::new(normalized).text(format!("{peak_db:.1} dBFS")));

                ui.add_space(8.0);
                ui.separator();

                ui.label("Sensitivity");
                ui.add(widgets::ParamSlider::for_param(&params.sensitivity, setter));

                ui.label("Onset method");
                ui.add(widgets::ParamSlider::for_param(&params.onset_method, setter));

                ui.add_space(4.0);
                ui.label("Silence threshold");
                ui.add(widgets::ParamSlider::for_param(&params.silence_threshold, setter));
                ui.label("Silence release");
                ui.add(widgets::ParamSlider::for_param(&params.silence_release, setter));

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Outputs:");
                    ui.add(widgets::ParamSlider::for_param(&params.link_enabled, setter));
                    ui.label("Link");
                    ui.add(widgets::ParamSlider::for_param(&params.midi_enabled, setter));
                    ui.label("MIDI");
                });

                ui.add_space(4.0);
                ui.collapsing("MIDI configuration", |ui| {
                    ui.label("Type");
                    ui.add(widgets::ParamSlider::for_param(&params.msg_type, setter));
                    ui.label("Channel");
                    ui.add(widgets::ParamSlider::for_param(&params.midi_channel, setter));
                    ui.label("Pulse rate");
                    ui.add(widgets::ParamSlider::for_param(&params.pulse_rate, setter));

                    let msg_type = params.msg_type.value();
                    let cc_enabled = matches!(msg_type, MsgType::Cc | MsgType::Both);
                    let note_enabled = matches!(msg_type, MsgType::Note | MsgType::Both);

                    ui.add_enabled_ui(cc_enabled, |ui| {
                        ui.label("CC number");
                        ui.add(widgets::ParamSlider::for_param(&params.cc_number, setter));
                        ui.label("CC value mode");
                        ui.add(widgets::ParamSlider::for_param(&params.cc_value_mode, setter));
                        ui.label("CC value (FixedCustom)");
                        ui.add(widgets::ParamSlider::for_param(&params.cc_value, setter));
                    });

                    ui.add_enabled_ui(note_enabled, |ui| {
                        ui.label("Note");
                        ui.add(widgets::ParamSlider::for_param(&params.note_number, setter));
                        ui.label("Velocity");
                        ui.add(widgets::ParamSlider::for_param(&params.note_velocity, setter));
                        ui.label("Length (ms)");
                        ui.add(widgets::ParamSlider::for_param(&params.note_length_ms, setter));
                    });
                });

                ui.add_space(4.0);
                ui.label("Resync");
                ui.add(widgets::ParamSlider::for_param(&params.manual_resync, setter));
                });
            });

            // Repaint at ~30 Hz so the BPM display, lock LED, peer count
            // and meter follow the audio thread.
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(33));
        },
    )
}
