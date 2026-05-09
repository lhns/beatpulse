// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! CPU profile harness for the DSP pipeline. Reports ns/sample over a
//! soak run; not a hard gate, but useful as a baseline + regression
//! signal. Gated behind feature `cpu-bench` because it runs longer than
//! a normal unit test.
//!
//! Run with: `cargo test --features cpu-bench --test cpu_profile -- --nocapture`

#![cfg(feature = "cpu-bench")]

use std::time::Instant;

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::pulse_generator::PulseGenerator;
use beatpulse::dsp::silence_gate::SilenceGate;
use beatpulse::midi::formatter::{FormatterConfig, MidiFormatter};
use beatpulse::params::{CcValueMode, MsgType, OnsetMethod};

const SR: f64 = 44_100.0;
const BLOCK: u32 = 512;
const N_BLOCKS: u32 = 10_000;

fn make_signal(n_samples: usize, bpm: f64) -> Vec<f32> {
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

#[test]
fn cpu_profile_full_pipeline() {
    let total_samples = (N_BLOCKS * BLOCK) as usize;
    let signal = make_signal(total_samples, 120.0);

    let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut gate = SilenceGate::new(SR, -50.0, 200.0);
    let mut pll = BeatPll::new(SR);
    pll.set_alphas(0.11, 0.165);
    let mut gen = PulseGenerator::new(4);
    let mut fmt = MidiFormatter::new();

    let cfg = FormatterConfig {
        msg_type: MsgType::Both,
        cc_number: 16,
        cc_value_mode: CcValueMode::Fixed127,
        cc_value: 127,
        note_number: 60,
        note_velocity: 100,
        note_length_samples: 441,
        midi_channel: 1,
    };

    let mut midi_out = Vec::with_capacity(BLOCK as usize * 3);
    let mut absolute = 0u64;

    let start = Instant::now();
    for chunk in signal.chunks(BLOCK as usize) {
        let abs_at_block = absolute;
        for s in chunk {
            gate.tick(*s);
        }
        let mut onsets: Vec<(u32, u64)> = Vec::with_capacity(4);
        tracker.process_block(chunk, |offset, _frac| {
            onsets.push((offset, abs_at_block + offset as u64));
        });
        midi_out.clear();
        fmt.flush_due_note_offs(abs_at_block, chunk.len() as u32, &mut midi_out);

        let mut next_onset = 0usize;
        for i in 0..chunk.len() as u32 {
            while next_onset < onsets.len() && onsets[next_onset].0 == i {
                pll.on_onset(onsets[next_onset].1 as f64);
                next_onset += 1;
            }
            pll.advance_one();
            if let Some(ev) = gen.observe_advance(&pll, i) {
                fmt.on_pulse(ev, abs_at_block, &cfg, &mut midi_out);
            }
        }
        absolute += chunk.len() as u64;
    }
    let elapsed = start.elapsed();

    let total_samples_f = (N_BLOCKS * BLOCK) as f64;
    let ns_per_sample = elapsed.as_nanos() as f64 / total_samples_f;
    let real_time_seconds = total_samples_f / SR;
    let speedup = real_time_seconds / elapsed.as_secs_f64();

    eprintln!("=== BeatPulse CPU profile ===");
    eprintln!("Blocks: {N_BLOCKS} × {BLOCK} samples = {total_samples_f:.0} samples");
    eprintln!("Wall time: {:?}", elapsed);
    eprintln!("ns/sample: {ns_per_sample:.1}");
    eprintln!("Speedup vs real-time: {speedup:.1}×");

    // Loose sanity gate: must be at least 100× real-time. The actual
    // budget is much tighter than this but a tighter assertion would
    // be machine-dependent. This catches catastrophic regressions
    // (e.g. accidental allocation in the hot loop).
    assert!(
        speedup > 100.0,
        "DSP pipeline ran at only {speedup:.1}× real-time; expected > 100×"
    );
}
