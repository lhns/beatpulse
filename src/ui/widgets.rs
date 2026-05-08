// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Custom widget helpers for BeatPulse's editor. Each takes a `ParamSetter`
//! and writes through it on user interaction. See ADR-0018 / ADR-0019.

use nih_plug::params::Param;
use nih_plug::prelude::{BoolParam, Enum, EnumParam, IntParam, IntRange, ParamSetter};
use nih_plug_egui::egui::{self, Color32, Response, Stroke, Ui};

/// Solid filled circle drawn via Painter — independent of the system
/// font's Unicode coverage (egui's default font doesn't include U+25CF).
/// `glow` adds a soft outer halo when bright (0.0..=1.0).
pub fn led(ui: &mut Ui, radius: f32, color: Color32, glow: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::Vec2::splat(radius * 2.0 + 6.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let center = rect.center();
    if glow > 0.05 {
        let alpha = (glow * 80.0) as u8;
        painter.circle_filled(
            center,
            radius * 1.8,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
        );
    }
    painter.circle_filled(center, radius, color);
    response
}

/// Dropdown for an `EnumParam<E>` showing the variant name in both the
/// closed and open states. Writes through the setter on selection.
pub fn enum_combo<E>(
    ui: &mut Ui,
    param: &EnumParam<E>,
    setter: &ParamSetter,
) -> Response
where
    E: Enum + PartialEq + Copy + 'static,
{
    let current = param.value();
    let variants = E::variants();
    let idx = current.to_index();
    let current_name: &'static str = variants.get(idx).copied().unwrap_or("?");
    let mut clicked_variant: Option<E> = None;
    let response = egui::ComboBox::from_id_salt(param.name())
        .selected_text(current_name)
        .width(160.0)
        .show_ui(ui, |ui| {
            for (i, name) in variants.iter().enumerate() {
                let variant = E::from_index(i);
                let is_selected = variant == current;
                if ui.selectable_label(is_selected, *name).clicked() && !is_selected {
                    clicked_variant = Some(variant);
                }
            }
        })
        .response;
    if let Some(v) = clicked_variant {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, v);
        setter.end_set_parameter(param);
    }
    response
}

/// Native egui `Checkbox` bound to a `BoolParam`.
pub fn bool_checkbox(
    ui: &mut Ui,
    param: &BoolParam,
    setter: &ParamSetter,
    label: &str,
) -> Response {
    let mut value = param.value();
    let response = ui.checkbox(&mut value, label);
    if response.changed() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, value);
        setter.end_set_parameter(param);
    }
    response
}

/// One-shot trigger button bound to a `BoolParam`. Writes `true` then
/// `false` so the audio thread's edge detector fires exactly once.
/// Styled prominently — primary action.
pub fn trigger_button(
    ui: &mut Ui,
    param: &BoolParam,
    setter: &ParamSetter,
    label: &str,
    accent: Color32,
) -> Response {
    let text = egui::RichText::new(label).strong().size(13.0);
    let response = ui.add(
        egui::Button::new(text)
            .min_size(egui::Vec2::new(140.0, 32.0))
            .corner_radius(6.0)
            .fill(Color32::from_rgb(35, 38, 44))
            .stroke(egui::Stroke::new(1.0, accent)),
    );
    if response.clicked() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, true);
        setter.end_set_parameter(param);
        setter.begin_set_parameter(param);
        setter.set_parameter(param, false);
        setter.end_set_parameter(param);
    }
    response
}

/// `egui::DragValue` bound to an `IntParam`. Click to type, drag to nudge.
pub fn int_drag(ui: &mut Ui, param: &IntParam, setter: &ParamSetter) -> Response {
    let mut value = param.value();
    let (min, max) = match param.range() {
        IntRange::Linear { min, max } => (min, max),
        IntRange::Reversed(inner) => match *inner {
            IntRange::Linear { min, max } => (min, max),
            IntRange::Reversed(_) => (0, 127),
        },
    };
    let response = ui.add(
        egui::DragValue::new(&mut value)
            .speed(1.0)
            .range(min..=max),
    );
    if response.changed() {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, value);
        setter.end_set_parameter(param);
    }
    response
}

/// Thin horizontal rule for section breaks (consistent colour, unlike
/// `ui.separator()`).
pub fn rule(ui: &mut Ui, color: Color32) {
    let height = 1.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width(), height + 8.0),
        egui::Sense::hover(),
    );
    let y = rect.center().y;
    ui.painter().line_segment(
        [
            egui::Pos2::new(rect.left() + 4.0, y),
            egui::Pos2::new(rect.right() - 4.0, y),
        ],
        Stroke::new(height, color),
    );
}
