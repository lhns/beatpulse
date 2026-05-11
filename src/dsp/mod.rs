// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! DSP modules. See `BeatPulse-SPEC.md` §5.

pub mod aubio_pulse_emitter;
pub mod aubio_tempo_tracker;
pub mod beat_pll;
pub mod beat_source;
pub mod beat_tracker;
pub mod consensus_tracker;
pub mod pulse_generator;
pub mod silence_gate;
