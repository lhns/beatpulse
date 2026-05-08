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

pub mod dsp;
pub mod link;
pub mod midi;
pub mod params;
pub mod shared;
pub mod ui;

// TODO (spec §11 step 1): implement `Plugin` for `Beatpulse`, register it
// with `nih_export_vst3!` and `nih_export_clap!`, wire the no-op passthrough
// `process` body. Subsequent steps wire DSP, MIDI, and Link.
