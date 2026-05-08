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

use nih_plug::prelude::Editor;
use nih_plug_egui::egui::{self, RichText};
use nih_plug_egui::{create_egui_editor, widgets, EguiState};

use crate::params::{BeatpulseParams, MsgType};
use crate::shared::SharedState;

pub const WINDOW_W: u32 = 480;
pub const WINDOW_H: u32 = 360;

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
                ui.heading("BeatPulse");
                ui.add_space(4.0);

                // Status row: lock LED + BPM + peer count
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
                    ui.label(format!("BPM {:.1}", shared.load_bpm()));
                    ui.separator();
                    ui.label(format!("Link peers {}", shared.load_link_peers()));
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

            // Repaint at ~30 Hz so the BPM display, lock LED, peer count
            // and meter follow the audio thread.
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(33));
        },
    )
}
