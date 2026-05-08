// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Resilience tests: sample-rate changes, block-size extremes. See
//! `docs/TESTING.md` §3.5 (B5 sample-rate-change re-init) and §3.5 (B1
//! hop accumulation across block sizes — extended here to extremes).

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::pulse_generator::PulseGenerator;
use beatpulse::dsp::silence_gate::SilenceGate;
use beatpulse::params::OnsetMethod;

/// P1: sample-rate change is handled by reconstructing the BeatTracker
/// (aubio) and updating the PLL bounds. Verify both paths leave the DSP
/// in a sane, leak-free state.
#[test]
fn p1_sample_rate_change() {
    for &sr in &[44_100u32, 48_000, 96_000, 192_000, 22_050] {
        let _bt = BeatTracker::new(sr, OnsetMethod::SpecFlux, 0.3).unwrap();
        let pll = BeatPll::new(sr as f64);
        // PLL bounds must reflect the new SR.
        assert!(pll.min_period > 0.0);
        assert!(pll.max_period > pll.min_period);
        // 220 BPM → period >= min, 60 BPM → period <= max.
        let period_220 = sr as f64 * 60.0 / 220.0;
        let period_60 = sr as f64 * 60.0 / 60.0;
        assert!((pll.min_period - period_220).abs() < 1e-6);
        assert!((pll.max_period - period_60).abs() < 1e-6);

        let mut gate = SilenceGate::new(sr as f64, -50.0, 200.0);
        // 200 ms must produce ~0.2 * sr release samples; we don't expose
        // the field but we can verify by dropping silence and counting.
        for _ in 0..(sr as usize) {
            // 1 second
            gate.tick(0.0);
        }
        // Should have transitioned (started Active by default).
        assert!(matches!(
            gate.state(),
            beatpulse::dsp::silence_gate::GateState::Silent
        ));
    }
}

/// Successive `BeatTracker::new` + drop cycles should not leak. We can't
/// assert a memory bound from inside the test, but if aubio's Drop is
/// missing or wrong, repeated allocation will eventually OOM. 1000 cycles
/// is a cheap proxy.
#[test]
fn p1_repeated_construction_no_leak() {
    for _ in 0..1000 {
        let bt = BeatTracker::new(44_100, OnsetMethod::SpecFlux, 0.3).unwrap();
        drop(bt);
    }
}

/// P2: block sizes 1, 2, 8192, 16384 still produce expected behaviour.
/// We feed kick clicks at 120 BPM and verify the BeatTracker emits at
/// least one onset (it should emit ~5 over the test window) and the PLL
/// converges to within 5 % of 120 BPM.
#[test]
fn p2_block_size_extremes() {
    use std::f32::consts::PI;
    const SR: f64 = 44_100.0;

    let beat_period = (SR * 60.0 / 120.0) as usize;
    let total = beat_period * 8;
    let click_at: Vec<usize> = (1..7).map(|i| i * beat_period).collect();

    // Build the same kick-drum signal used in `tests/beat_tracker.rs`.
    let mut signal = vec![0.0f32; total];
    let click_len = (0.050 * SR as f32) as usize;
    let decay_tau = 0.020 * SR as f32;
    for &t in &click_at {
        for i in 0..click_len {
            let pos = t + i;
            if pos < total {
                let phase = 2.0 * PI * 60.0 * (i as f32) / SR as f32;
                let env = (-(i as f32) / decay_tau).exp();
                signal[pos] += 0.8 * env * phase.sin();
            }
        }
    }

    for &block in &[1usize, 2, 8192, 16384] {
        let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
        let mut pll = BeatPll::new(SR);
        let mut onsets = 0usize;
        let mut absolute = 0u64;
        for chunk in signal.chunks(block) {
            let abs_at_block = absolute;
            tracker.process_block(chunk, |offset, _frac| {
                onsets += 1;
                pll.on_onset((abs_at_block + offset as u64) as f64);
            });
            pll.advance(chunk.len() as u64);
            absolute += chunk.len() as u64;
        }
        // We expect at least 4 onsets out of 6 clicks, with PLL converged
        // to ~120 BPM (within 5 %).
        assert!(onsets >= 4, "block={block}: only {onsets} onsets detected");
        let bpm = pll.current_bpm();
        assert!(
            (bpm - 120.0).abs() / 120.0 < 0.05,
            "block={block}: bpm={bpm} outside 5 % of 120"
        );
    }
}

/// P2 (sub-case): PulseGenerator at extreme block sizes. We don't need
/// audio for this — drive the PLL at fixed period and check pulse spacing.
#[test]
fn p2_pulse_generator_block_extremes() {
    let period = 44100.0;
    for &block in &[1u32, 2, 8192, 16384] {
        let mut pll = BeatPll::new(44100.0);
        pll.period_samples = period;
        pll.phase_samples = 0.0;
        let mut gen = PulseGenerator::new(4);
        gen.reset();

        // Run for slightly more than one full period.
        let total = period as u32 + 1;
        let mut emitted = 0u32;
        let mut remaining = total;
        let mut absolute = 0u64;
        while remaining > 0 {
            let take = remaining.min(block);
            gen.process_block(&mut pll, take, |_| emitted += 1);
            remaining -= take;
            absolute += take as u64;
        }
        // 4 PPQN over one full period = 4 boundary crossings + 1 reset
        // pulse = 5.
        assert_eq!(
            emitted, 5,
            "block={block}: emitted {emitted}, expected 5 (over {} samples)",
            absolute
        );
    }
}
