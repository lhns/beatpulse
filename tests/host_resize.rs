// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! The editor advertises host-driven resize with our minimum size.
//! See ADR-0029.

use std::sync::Arc;

use beatpulse::params::BeatpulseParams;
use beatpulse::shared::SharedState;
use beatpulse::ui;
use nice_plug::editor::dpi::NativeSize;
use nice_plug::prelude::Editor;
use nice_plug_egui::RepaintNotifier;

#[test]
fn host_resize_clamps_to_min_size() {
    let editor = ui::editor(
        ui::default_state(),
        RepaintNotifier::new(),
        Arc::new(BeatpulseParams::default()),
        Arc::new(SharedState::default()),
    )
    .expect("editor");
    let hint = editor.resize_hint();
    assert!(hint.can_resize);

    let current = NativeSize::new(ui::WINDOW_W, ui::WINDOW_H);
    assert_eq!(
        hint.adjust_size(NativeSize::new(10, 10), current, 1.0),
        NativeSize::new(ui::MIN_W, ui::MIN_H)
    );
    let bigger = NativeSize::new(ui::WINDOW_W + 200, ui::WINDOW_H + 100);
    assert_eq!(hint.adjust_size(bigger, current, 1.0), bigger);
}
