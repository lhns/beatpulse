// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Klapuri 2006 multi-band beat tracker.
//!
//! From "Analysis of the meter of acoustic musical signals", Klapuri,
//! Eronen & Astola, IEEE TASLP 14(1):342–355, 2006. Three stages:
//!
//! 1. **Multi-band accent signal** ([`accent`]) — STFT → mel-warped
//!    sub-bands → log compression → DC removal → half-wave-rectified
//!    differential. 4 channels at the OSS rate (~172 Hz @ 44.1 kHz / 256
//!    hop).
//! 2. **Comb-filter resonator bank** ([`resonators`]) — bank of IIR comb
//!    filters tuned to candidate periods. Per-period energy is the
//!    period evidence.
//! 3. **Joint period inference + phase** ([`period_inference`],
//!    [`phase`]) — Bayesian-ish prior over period ratios couples
//!    tatum / tactus / measure (kills octave errors); phase from the
//!    winning resonator's delay-line state.
//!
//! Validation harnesses live in `tests/klapuri_*.rs`. The standalone
//! Ballroom benchmark (`tests/klapuri_experiment.rs`) is the
//! integration go/no-go gate per the plan.

pub mod accent;
pub mod joint_posterior;
pub mod period_inference;
pub mod phase;
pub mod resonators;

pub use phase::KlapuriTracker;
