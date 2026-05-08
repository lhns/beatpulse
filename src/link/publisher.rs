// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! `LinkPublisher` owns a `rusty_link::AblLink` and pushes tempo on each
//! `process` call using the realtime-safe `_audio_session_state` API.
//! See spec §6 and ADR-0007.
//
// TODO (spec §11 step 9): implement `LinkPublisher`.
