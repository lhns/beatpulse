// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Headless UI snapshot tests. See ADR-0019 and `docs/TESTING.md`.
//!
//! Renders the editor panel via `egui_kittest` (which uses wgpu under the
//! hood) and saves PNGs to `tests/data/output/` for visual review. The
//! test does not assert pixel equality — it just produces images and the
//! reviewer (human or LLM) inspects them.

use std::path::PathBuf;

use beatpulse::params::BeatpulseParams;
use beatpulse::shared::{LinkStatus, SharedState};
use beatpulse::ui::{build_panel, build_panel_with, PanelOptions, WINDOW_H, WINDOW_W};
use egui_kittest::Harness;
use nih_plug::context::gui::GuiContext;
use nih_plug::params::internals::ParamPtr;
use nih_plug::prelude::{ParamSetter, PluginApi};
use nih_plug::wrapper::state::PluginState;

/// Stub GuiContext for snapshots. Parameter writes are dropped on the
/// floor — we render the UI but don't propagate user edits anywhere.
struct StubGuiContext;

impl GuiContext for StubGuiContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Standalone
    }
    fn request_resize(&self) -> bool {
        false
    }
    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
    unsafe fn raw_set_parameter_normalized(&self, _param: ParamPtr, _normalized: f32) {}
    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
    fn get_state(&self) -> PluginState {
        PluginState {
            version: String::new(),
            params: Default::default(),
            fields: Default::default(),
        }
    }
    fn set_state(&self, _state: PluginState) {}
}

fn output_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/output");
    std::fs::create_dir_all(&dir).expect("could not create snapshot output dir");
    dir
}

fn save_png(image: &image::RgbaImage, name: &str) {
    let path = output_dir().join(format!("{name}.png"));
    image.save(&path).expect("failed to save snapshot PNG");
    eprintln!("snapshot saved: {}", path.display());
}

#[test]
fn snapshot_midi_expanded() {
    let params = BeatpulseParams::default();
    let shared = SharedState::default();
    shared.store_bpm(120.0);
    shared.store_locked(true);
    shared.store_peak_db(-12.0);
    shared.store_link_peers(2);
    shared.store_link_status(LinkStatus::Publishing);

    let stub = StubGuiContext;
    let setter = ParamSetter::new(&stub);

    let opts = PanelOptions {
        force_midi_open: true,
    };
    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(WINDOW_W as f32, WINDOW_H as f32 + 200.0))
        .wgpu()
        .build(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    build_panel_with(ui, &params, &shared, &setter, opts);
                });
            });
        });

    harness.run();
    let image = harness.render().expect("render failed");
    save_png(&image, "ui-midi-expanded");
}

#[test]
fn snapshot_resized_panel() {
    // Verify that growing the window gives content more room (level
    // meter and sliders widen) without scaling fonts. Per ADR-0020 /
    // ADR-0021.
    let params = BeatpulseParams::default();
    let shared = SharedState::default();
    shared.store_bpm(133.0);
    shared.store_locked(true);
    shared.store_peak_db(-9.5);
    shared.store_link_peers(3);
    shared.store_link_status(LinkStatus::Publishing);

    let stub = StubGuiContext;
    let setter = ParamSetter::new(&stub);

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(720.0, 800.0))
        .wgpu()
        .build(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    build_panel(ui, &params, &shared, &setter);
                });
            });
        });

    harness.run();
    let image = harness.render().expect("render failed");
    save_png(&image, "ui-resized");
}

#[test]
fn snapshot_unstable_bpm() {
    // Verify that `±σ` annotation appears next to the BPM when the
    // PLL is reporting noticeable variance.
    let params = BeatpulseParams::default();
    let shared = SharedState::default();
    shared.store_bpm(133.0);
    shared.store_bpm_std_dev(1.4);
    shared.store_locked(true);
    shared.store_peak_db(-9.5);
    shared.store_link_peers(2);
    shared.store_link_status(LinkStatus::Publishing);

    let stub = StubGuiContext;
    let setter = ParamSetter::new(&stub);

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(WINDOW_W as f32, WINDOW_H as f32))
        .wgpu()
        .build(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    build_panel(ui, &params, &shared, &setter);
                });
            });
        });

    harness.run();
    let image = harness.render().expect("render failed");
    save_png(&image, "ui-unstable-bpm");
}

#[test]
fn snapshot_default_panel() {
    let params = BeatpulseParams::default();
    let shared = SharedState::default();
    // Sample state: live plugin, locked, publishing.
    shared.store_bpm(120.0);
    shared.store_locked(true);
    shared.store_peak_db(-12.0);
    shared.store_link_peers(2);
    shared.store_link_status(LinkStatus::Publishing);

    let stub = StubGuiContext;
    let setter = ParamSetter::new(&stub);

    let mut harness = Harness::builder()
        .with_size(egui::Vec2::new(WINDOW_W as f32, WINDOW_H as f32))
        .wgpu()
        .build(|ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    build_panel(ui, &params, &shared, &setter);
                });
            });
        });

    harness.run();
    let image = harness.render().expect("render failed");
    save_png(&image, "ui-default");
}

// egui-kittest re-exports egui types via egui_kittest::egui? not always.
// Bring egui in scope via the same crate the build_panel uses, which is
// nih_plug_egui::egui. Importing nih_plug_egui directly here is the same
// concrete crate.
use nih_plug_egui::egui;
