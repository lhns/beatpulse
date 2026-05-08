// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Ableton Link tempo publisher. See `BeatPulse-SPEC.md` §6 and ADR-0007.

use rusty_link::{AblLink, SessionState};

/// Don't push tempo updates to Link when the BPM has moved by less than
/// this much since the last push. Avoids saturating the Link gossip with
/// sub-perceptual jitter from PLL micro-adjustments.
const COMMIT_THRESHOLD_BPM: f64 = 0.05;

pub struct LinkPublisher {
    link: AblLink,
    /// Reused on the audio thread; constructed once.
    session_state: SessionState,
    last_published_bpm: f64,
}

impl LinkPublisher {
    pub fn new(initial_bpm: f64) -> Self {
        let link = AblLink::new(initial_bpm);
        link.enable(true);
        link.enable_start_stop_sync(false); // We don't want transport events.
        Self {
            link,
            session_state: SessionState::new(),
            last_published_bpm: initial_bpm,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.link.enable(enabled);
    }

    pub fn is_enabled(&self) -> bool {
        self.link.is_enabled()
    }

    pub fn num_peers(&self) -> u64 {
        self.link.num_peers()
    }

    /// Publish the current tempo. No-op if the BPM hasn't changed
    /// significantly since the last call. Realtime-safe: uses
    /// `capture_audio_session_state` / `commit_audio_session_state`.
    pub fn publish_tempo(&mut self, current_bpm: f64) {
        if !self.link.is_enabled() {
            return;
        }
        if !current_bpm.is_finite() || current_bpm <= 0.0 {
            return;
        }
        if (current_bpm - self.last_published_bpm).abs() < COMMIT_THRESHOLD_BPM {
            return;
        }
        self.link.capture_audio_session_state(&mut self.session_state);
        let micros = self.link.clock_micros();
        self.session_state.set_tempo(current_bpm, micros);
        self.link.commit_audio_session_state(&self.session_state);
        self.last_published_bpm = current_bpm;
    }

    /// Read the current session tempo (e.g. for UI display).
    pub fn current_session_tempo(&mut self) -> f64 {
        self.link.capture_app_session_state(&mut self.session_state);
        self.session_state.tempo()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_and_drop() {
        let p = LinkPublisher::new(120.0);
        assert!(p.is_enabled());
        // num_peers may be > 0 if other Link peers are running on the
        // machine; just check it returns without panicking.
        let _ = p.num_peers();
    }

    #[test]
    fn publish_skips_subthreshold_changes() {
        let mut p = LinkPublisher::new(120.0);
        p.publish_tempo(120.0); // no change
        p.publish_tempo(120.04); // below threshold
        // Should still be 120.0 — Link's session state may have drifted from
        // peers, so we only assert that our local last_published_bpm hasn't
        // moved.
        assert_eq!(p.last_published_bpm, 120.0);
    }

    #[test]
    fn publish_accepts_supra_threshold_change() {
        let mut p = LinkPublisher::new(120.0);
        p.publish_tempo(124.5);
        assert_eq!(p.last_published_bpm, 124.5);
    }

    #[test]
    fn ignores_invalid_bpm() {
        let mut p = LinkPublisher::new(120.0);
        p.publish_tempo(f64::NAN);
        p.publish_tempo(0.0);
        p.publish_tempo(-30.0);
        assert_eq!(p.last_published_bpm, 120.0);
    }

    #[test]
    fn enable_disable_round_trip() {
        let p = LinkPublisher::new(120.0);
        assert!(p.is_enabled());
        p.set_enabled(false);
        assert!(!p.is_enabled());
        p.set_enabled(true);
        assert!(p.is_enabled());
    }
}
