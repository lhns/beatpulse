// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! In-process Ableton Link integration test (gated behind feature
//! `link-integration`). See ADR-0015 and `docs/TESTING.md` §7.1.
//!
//! Two `AblLink` instances on the same machine become peers via loopback
//! discovery (no network needed). We construct one as the publisher
//! (used by BeatPulse's DSP pipeline) and one as the listener; feed the
//! pipeline a synthetic click track; assert that the listener observes
//! the published tempo.
//!
//! Run with: `cargo test --features link-integration --test link_listener -- --nocapture`

#![cfg(feature = "link-integration")]

use std::time::{Duration, Instant};

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::link::publisher::LinkPublisher;
use beatpulse::params::OnsetMethod;
use rusty_link::{AblLink, SessionState};

const SR: f64 = 44_100.0;
const BLOCK: usize = 512;

/// Generate a kick-drum click signal of `n_samples` at the given BPM.
fn make_click_track(bpm: f64, n_samples: usize) -> Vec<f32> {
    let mut buf = vec![0.0f32; n_samples];
    let click_len = (0.050 * SR as f32) as usize;
    let decay_tau = 0.020 * SR as f32;
    let beat_period = (SR * 60.0 / bpm) as usize;
    let mut t = (0.5 * SR) as usize;
    while t + click_len < n_samples {
        for i in 0..click_len {
            let phase = 2.0 * std::f32::consts::PI * 60.0 * (i as f32) / SR as f32;
            let env = (-(i as f32) / decay_tau).exp();
            buf[t + i] += 0.8 * env * phase.sin();
        }
        t += beat_period;
    }
    buf
}

/// Drive the publisher pipeline against `bpm` for `duration_secs`.
/// Returns the listener's observed tempo at the end.
fn run_with_listener(bpm: f64, duration_secs: f64) -> f64 {
    let listener = AblLink::new(120.0);
    listener.enable(true);

    let total = (duration_secs * SR) as usize;
    let signal = make_click_track(bpm, total);

    let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut pll = BeatPll::new(SR);
    let mut publisher = LinkPublisher::new(120.0);

    let block_dur = Duration::from_secs_f64(BLOCK as f64 / SR);
    let start = Instant::now();
    let mut absolute = 0u64;

    for chunk in signal.chunks(BLOCK) {
        let abs_at_block = absolute;
        tracker.process_block(chunk, |offset, _frac| {
            pll.on_onset((abs_at_block + offset as u64) as f64);
        });
        pll.advance(chunk.len() as u64);
        if pll.locked {
            publisher.publish_tempo(pll.current_bpm(), 0);
        }
        absolute += chunk.len() as u64;

        // Pace at real-time so Link gossip has a chance to propagate.
        let elapsed = start.elapsed();
        let target = block_dur * (absolute / BLOCK as u64) as u32;
        if target > elapsed {
            std::thread::sleep(target - elapsed);
        }
    }

    // Give Link's gossip thread a beat to deliver the final state.
    std::thread::sleep(Duration::from_millis(200));

    let mut state = SessionState::new();
    listener.capture_app_session_state(&mut state);
    state.tempo()
}

/// All BPMs in one test because parallel Link instances interfere
/// (multicast is process-global). Each sub-case constructs and drops its
/// own publisher to isolate session state, with a settle delay between.
#[test]
fn link_listener_tracks_published_tempo() {
    for &bpm in &[120.0_f64, 140.0, 100.0] {
        let observed = run_with_listener(bpm, 12.0);
        assert!(
            (observed - bpm).abs() < 1.5,
            "listener observed {observed:.2} BPM, expected ~{bpm}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}
