// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! egui-based plugin editor. See `BeatPulse-SPEC.md` §9.

pub mod widgets;

use std::sync::Arc;
use std::time::Instant;

use nih_plug::prelude::{Editor, ParamSetter};
use nih_plug_egui::egui::{self, Color32, RichText};
use nih_plug_egui::resizable_window::ResizableWindow;
use nih_plug_egui::{create_egui_editor, widgets as nih_widgets, EguiState};

use crate::params::{BeatpulseParams, MsgType};
use crate::shared::{LinkStatus, SharedState};

/// How long the beat-flash LED stays lit after each detected beat, in ms.
const BEAT_FLASH_MS: f32 = 120.0;

pub const WINDOW_W: u32 = 480;
pub const WINDOW_H: u32 = 540;
/// Minimum drag-to-resize dimensions (ResizableWindow's lower bound).
/// Below these, the level meter and section headers become hard to read.
pub const MIN_W: u32 = 420;
pub const MIN_H: u32 = 360;

// Theme colours.
const ACCENT: Color32 = Color32::from_rgb(80, 200, 230); // cyan
const LOCK_GREEN: Color32 = Color32::from_rgb(80, 220, 100);
const AMBER: Color32 = Color32::from_rgb(220, 180, 60);
const LED_DIM: Color32 = Color32::from_rgb(60, 60, 60);
const SECTION_HEADER: Color32 = Color32::from_rgb(170, 200, 215);
const RULE_COLOR: Color32 = Color32::from_rgb(50, 55, 62);

pub fn default_state() -> Arc<EguiState> {
    EguiState::from_size(WINDOW_W, WINDOW_H)
}

pub fn editor(
    egui_state: Arc<EguiState>,
    params: Arc<BeatpulseParams>,
    shared: Arc<SharedState>,
) -> Option<Box<dyn Editor>> {
    let editor_state_for_resize = egui_state.clone();
    create_egui_editor(
        egui_state,
        (),
        |ctx, _| apply_visuals(ctx),
        move |egui_ctx, setter, _state| {
            apply_visuals(egui_ctx);
            // ResizableWindow paints a drag-handle at the bottom-right
            // corner and writes the new size into EguiState (which is
            // persisted via #[persist]). See ADR-0020.
            ResizableWindow::new("beatpulse-window")
                .min_size(egui::Vec2::new(MIN_W as f32, MIN_H as f32))
                .show(egui_ctx, &editor_state_for_resize, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        build_panel(ui, &params, &shared, setter);
                    });
                });

            // Repaint at ~30 Hz so the BPM display, lock LED, peer count
            // and meter follow the audio thread.
            egui_ctx.request_repaint_after(std::time::Duration::from_millis(33));
        },
    )
}

fn apply_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = Color32::from_rgb(20, 22, 26);
    visuals.panel_fill = Color32::from_rgb(20, 22, 26);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(20, 22, 26);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(35, 38, 44);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(35, 38, 44);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(45, 50, 58);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(45, 50, 58);
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    ctx.set_visuals(visuals);
}

/// Options for tweaking `build_panel` rendering. Used by snapshot tests
/// to force certain sections open without simulating click events.
#[derive(Default, Clone, Copy)]
pub struct PanelOptions {
    pub force_midi_open: bool,
}

/// Build the editor's panel content. Extracted from `editor()` so it can
/// be driven by the snapshot test in `tests/ui_snapshot.rs`.
pub fn build_panel(
    ui: &mut egui::Ui,
    params: &BeatpulseParams,
    shared: &SharedState,
    setter: &ParamSetter,
) {
    build_panel_with(ui, params, shared, setter, PanelOptions::default());
}

/// Same as `build_panel` but takes options for testing.
pub fn build_panel_with(
    ui: &mut egui::Ui,
    params: &BeatpulseParams,
    shared: &SharedState,
    setter: &ParamSetter,
    options: PanelOptions,
) {
    // ResizableWindow strips the CentralPanel inner_margin in its
    // internal layout (it uses ui.clip_rect() for the content area).
    // Re-add explicit padding here so the production editor doesn't have
    // content butting up against the window edges.
    egui::Frame::NONE
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            build_panel_body(ui, params, shared, setter, options);
        });
}

fn build_panel_body(
    ui: &mut egui::Ui,
    params: &BeatpulseParams,
    shared: &SharedState,
    setter: &ParamSetter,
    options: PanelOptions,
) {
    let egui_ctx = ui.ctx().clone();

    // Header
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("BeatPulse")
                .color(SECTION_HEADER)
                .strong()
                .size(20.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .color(Color32::from_rgb(110, 115, 120))
                    .size(11.0),
            );
        });
    });
    widgets::rule(ui, RULE_COLOR);

    // BPM hero row + LEDs + Link status
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{:.1}", shared.load_bpm()))
                .color(SECTION_HEADER)
                .monospace()
                .size(34.0),
        );
        ui.label(
            RichText::new("BPM")
                .color(Color32::from_rgb(140, 145, 150))
                .size(12.0)
                .strong(),
        );
        let sigma = shared.load_bpm_std_dev();
        if sigma > 0.5 {
            ui.label(
                RichText::new(format!("±{sigma:.1}"))
                    .color(Color32::from_rgb(200, 150, 80))
                    .size(13.0),
            );
        }
        ui.add_space(12.0);
        ui.vertical(|ui| {
            // LOCKED LED
            ui.horizontal(|ui| {
                let locked = shared.load_locked();
                let (color, glow) = if locked {
                    (LOCK_GREEN, 1.0)
                } else {
                    (LED_DIM, 0.0)
                };
                widgets::led(ui, 6.0, color, glow);
                ui.label(
                    RichText::new(if locked { "LOCKED" } else { "—" })
                        .color(if locked {
                            SECTION_HEADER
                        } else {
                            Color32::DARK_GRAY
                        })
                        .size(11.0),
                );
            });
            // BEAT LED
            ui.horizontal(|ui| {
                let beat_count = shared.load_beat_count();
                let id = egui::Id::new("beatpulse_beat_flash");
                let mut state: (u64, Option<Instant>) =
                    egui_ctx.data(|d| d.get_temp(id)).unwrap_or((0, None));
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
                let beat_color = Color32::from_rgb(lerp(60, 80), lerp(60, 200), lerp(60, 230));
                widgets::led(ui, 6.0, beat_color, brightness);
                ui.label(RichText::new("BEAT").color(SECTION_HEADER).size(11.0));
            });
        });
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(format!("Link peers {}", shared.load_link_peers()))
                    .color(SECTION_HEADER)
                    .size(11.0),
            );
            ui.horizontal(|ui| {
                let (text, color, glow) = match shared.load_link_status() {
                    LinkStatus::Off => ("OFF", LED_DIM, 0.0),
                    LinkStatus::Joined => ("JOINED", AMBER, 0.6),
                    LinkStatus::Publishing => ("PUBLISHING", LOCK_GREEN, 1.0),
                };
                widgets::led(ui, 5.0, color, glow);
                ui.label(RichText::new(text).color(color).size(11.0).strong());
            });
        });
    });
    widgets::rule(ui, RULE_COLOR);

    // Input level meter
    let peak_db = shared.load_peak_db();
    let normalized = ((peak_db + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
    section_header(ui, "Input level");
    ui.add(
        egui::widgets::ProgressBar::new(normalized)
            .text(format!("{peak_db:.1} dBFS"))
            .desired_width(ui.available_width())
            .corner_radius(2.0),
    );

    ui.add_space(6.0);
    section_header(ui, "Detection");
    // Reserve space for the label column (~110 px) AND the value-pill
    // that ParamSlider draws to the right of the slider itself (~70 px).
    let slider_w = (ui.available_width() - 200.0).max(160.0);
    egui::Grid::new("detection-grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label("Sensitivity");
            ui.add(
                nih_widgets::ParamSlider::for_param(&params.sensitivity, setter)
                    .with_width(slider_w),
            );
            ui.end_row();

            ui.label("Stability");
            ui.add(
                nih_widgets::ParamSlider::for_param(&params.tempo_stability, setter)
                    .with_width(slider_w),
            );
            ui.end_row();

            ui.label("Onset method");
            widgets::enum_combo(ui, &params.onset_method, setter);
            ui.end_row();

            ui.label("Silence threshold");
            ui.add(
                nih_widgets::ParamSlider::for_param(&params.silence_threshold, setter)
                    .with_width(slider_w),
            );
            ui.end_row();

            ui.label("Silence release");
            ui.add(
                nih_widgets::ParamSlider::for_param(&params.silence_release, setter)
                    .with_width(slider_w),
            );
            ui.end_row();
        });

    ui.add_space(6.0);
    section_header(ui, "Timing");
    egui::Grid::new("timing-grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label("Latency offset");
            ui.add(
                nih_widgets::ParamSlider::for_param(&params.latency_offset_ms, setter)
                    .with_width(slider_w),
            );
            ui.end_row();
        });

    ui.add_space(6.0);
    section_header(ui, "Outputs");
    ui.horizontal(|ui| {
        widgets::bool_checkbox(ui, &params.link_enabled, setter, "Ableton Link");
        ui.add_space(16.0);
        widgets::bool_checkbox(ui, &params.midi_enabled, setter, "MIDI");
    });

    ui.add_space(4.0);
    egui::CollapsingHeader::new(
        RichText::new("MIDI configuration")
            .color(SECTION_HEADER)
            .strong(),
    )
    .default_open(options.force_midi_open)
    .show(ui, |ui| {
        egui::Grid::new("midi-grid")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Type");
                widgets::enum_combo(ui, &params.msg_type, setter);
                ui.end_row();

                ui.label("Channel");
                widgets::int_drag(ui, &params.midi_channel, setter);
                ui.end_row();

                ui.label("Pulse rate");
                widgets::enum_combo(ui, &params.pulse_rate, setter);
                ui.end_row();
            });

        let msg_type = params.msg_type.value();
        let cc_enabled = matches!(msg_type, MsgType::Cc | MsgType::Both);
        let note_enabled = matches!(msg_type, MsgType::Note | MsgType::Both);

        ui.add_space(4.0);
        ui.add_enabled_ui(cc_enabled, |ui| {
            ui.label(
                RichText::new("CC")
                    .color(if cc_enabled {
                        SECTION_HEADER
                    } else {
                        Color32::DARK_GRAY
                    })
                    .strong(),
            );
            egui::Grid::new("cc-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("CC number");
                    widgets::int_drag(ui, &params.cc_number, setter);
                    ui.end_row();

                    ui.label("Value mode");
                    widgets::enum_combo(ui, &params.cc_value_mode, setter);
                    ui.end_row();

                    ui.label("Custom value");
                    widgets::int_drag(ui, &params.cc_value, setter);
                    ui.end_row();
                });
        });

        ui.add_space(4.0);
        ui.add_enabled_ui(note_enabled, |ui| {
            ui.label(
                RichText::new("Note")
                    .color(if note_enabled {
                        SECTION_HEADER
                    } else {
                        Color32::DARK_GRAY
                    })
                    .strong(),
            );
            egui::Grid::new("note-grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Note number");
                    widgets::int_drag(ui, &params.note_number, setter);
                    ui.end_row();

                    ui.label("Velocity");
                    widgets::int_drag(ui, &params.note_velocity, setter);
                    ui.end_row();

                    ui.label("Length (ms)");
                    ui.add(
                        nih_widgets::ParamSlider::for_param(&params.note_length_ms, setter)
                            .with_width(slider_w),
                    );
                    ui.end_row();
                });
        });
    });

    ui.add_space(8.0);
    widgets::rule(ui, RULE_COLOR);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        widgets::trigger_button(ui, &params.manual_resync, setter, "↻  RESYNC", ACCENT);
        ui.label(
            RichText::new("Force the PLL to re-acquire on the next onset.")
                .color(Color32::from_rgb(120, 125, 132))
                .size(11.0),
        );
    });
}

fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(3.0, 13.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 1.0, ACCENT);
        ui.label(
            RichText::new(text)
                .color(SECTION_HEADER)
                .strong()
                .size(13.0),
        );
    });
    ui.add_space(2.0);
}
