// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Onset detection wrapping `aubio_rs::Onset`. See spec §5.1 and ADR-0003.
//
// TODO (spec §11 step 6): hop accumulator + aubio onset detector + sample-
// offset back-calculation.
