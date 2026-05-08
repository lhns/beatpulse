// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Lock-free atomics shared between the audio thread and the UI thread.
//! See `BeatPulse-SPEC.md` §9.
//
// TODO: define `SharedState { current_bpm, locked, input_peak_db,
// link_peers, silence_active }` using `atomic_float::AtomicF64` and
// `std::sync::atomic::*`.
