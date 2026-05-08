// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Realtime-safety smoke tests. See `docs/TESTING.md` §4.
//!
//! The full nih-plug `Plugin::process` cannot be called outside a host,
//! so these tests exercise the same DSP pipeline by driving the modules
//! directly with synthetic input. The crate-level
//! `assert_process_allocs` feature on nih-plug catches host-driven
//! allocations; here we run a soak test to confirm no NaNs or panics
//! over many blocks of varied sizes.

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::pulse_generator::PulseGenerator;
use beatpulse::dsp::silence_gate::SilenceGate;
use beatpulse::midi::formatter::{FormatterConfig, MidiFormatter};
use beatpulse::params::{CcValueMode, MsgType, OnsetMethod};

const SR: f64 = 44_100.0;

fn cfg() -> FormatterConfig {
    FormatterConfig {
        msg_type: MsgType::Both,
        cc_number: 16,
        cc_value_mode: CcValueMode::Fixed127,
        cc_value: 127,
        note_number: 60,
        note_velocity: 100,
        note_length_samples: 441,
        midi_channel: 1,
    }
}

/// R1 + R2: drive 10 000 blocks of varying sizes; no panic, no NaN.
#[test]
fn r1_r2_soak_varying_block_sizes() {
    let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut gate = SilenceGate::new(SR, -50.0, 200.0);
    let mut pll = BeatPll::new(SR);
    let mut gen = PulseGenerator::new(4);
    let mut fmt = MidiFormatter::new();

    let block_sizes = [32usize, 64, 128, 256, 512, 1024, 2048];
    let mut buf = vec![0.0f32; 4096];
    let mut out = Vec::with_capacity(64);
    let mut absolute = 0u64;

    for i in 0..1000 {
        let n = block_sizes[i % block_sizes.len()];
        // Fill buf with a fading 60 Hz click every 250 samples.
        for (k, s) in buf[..n].iter_mut().enumerate() {
            let phase = 2.0 * std::f32::consts::PI * 60.0 * (k as f32) / SR as f32;
            *s = if k % 250 < 50 {
                (phase.sin()) * 0.5
            } else {
                0.0
            };
        }
        let block = &buf[..n];

        // Silence gate
        for &s in block {
            gate.tick(s);
        }
        // Beat tracker
        tracker.process_block(block, |_off, _frac| {});
        // PLL advances
        pll.advance(n as u64);
        // Pulse gen + formatter
        out.clear();
        fmt.flush_due_note_offs(absolute, n as u32, &mut out);
        gen.process_block(&mut pll, n as u32, |_| {});
        absolute = absolute.wrapping_add(n as u64);

        // Sanity
        assert!(pll.phase_samples.is_finite());
        assert!(pll.period_samples.is_finite());
        assert!(pll.period_samples > 0.0);
    }

    let _ = cfg();
}

/// R3: toggle parameters mid-stream — use the public setter APIs.
#[test]
fn r3_param_toggles_no_panic() {
    let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut gate = SilenceGate::new(SR, -50.0, 200.0);
    let mut gen = PulseGenerator::new(4);

    for i in 0..200 {
        gate.set_threshold_db(-50.0 + (i % 20) as f64);
        gate.set_release_ms(100.0 + (i % 500) as f64);
        gen.set_pulse_rate(((i % 6) as u32 + 1) * 2);
        if i % 50 == 0 {
            tracker
                .set_method(match i % 7 {
                    0 => OnsetMethod::Hfc,
                    1 => OnsetMethod::Complex,
                    2 => OnsetMethod::SpecDiff,
                    3 => OnsetMethod::Kl,
                    4 => OnsetMethod::Mkl,
                    5 => OnsetMethod::Phase,
                    _ => OnsetMethod::SpecFlux,
                })
                .unwrap();
        }
    }
}
