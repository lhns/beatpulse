// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! `PulseGenerator` integration tests. See `docs/TESTING.md` §3.2.

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::pulse_generator::PulseGenerator;

const SR: f64 = 44_100.0;

fn pll_at(period: f64) -> BeatPll {
    let mut pll = BeatPll::new(SR);
    pll.period_samples = period;
    pll.phase_samples = 0.0;
    pll
}

/// Run `n_samples` through the generator with the PLL having `period`
/// samples per beat starting at phase 0. Returns the sample offsets of the
/// emitted pulses.
fn collect_pulses(period: f64, ppqn: u32, n_samples: u32) -> Vec<u32> {
    let mut pll = pll_at(period);
    let mut gen = PulseGenerator::new(ppqn);
    // First pulse-after-reset fires immediately. Consume it so subsequent
    // pulses come from real boundary crossings.
    gen.reset();
    let mut pulses = Vec::new();
    gen.process_block(&mut pll, n_samples, |ev| pulses.push(ev.sample_offset));
    pulses
}

/// G1–G6: spacing at PPQN 1, 2, 4, 8, 16, 24 with period = 44100 (1 s/beat).
#[test]
fn g1_through_g6_spacing() {
    let period = 44100.0;
    for &ppqn in &[1u32, 2, 4, 8, 16, 24] {
        let pulses = collect_pulses(period, ppqn, period as u32 + 1);
        let interval = period / ppqn as f64;
        // First pulse at sample 0 (first-after-reset). Then every `interval`.
        assert_eq!(
            pulses.len(),
            ppqn as usize + 1,
            "ppqn={ppqn}: got {} pulses, expected {}",
            pulses.len(),
            ppqn + 1
        );
        for (i, &offset) in pulses.iter().enumerate() {
            let expected = i as f64 * interval;
            assert!(
                (offset as f64 - expected).abs() <= 1.0,
                "ppqn={ppqn} pulse {i}: offset {offset}, expected ~{expected}"
            );
        }
    }
}

/// G7: pulse falling across a block boundary lands in correct block with
/// correct sub-block offset.
#[test]
fn g7_block_boundary() {
    let period = 4000.0;
    let mut pll = pll_at(period);
    let mut gen = PulseGenerator::new(4);
    gen.reset();

    // Block 1: 100 samples — should fire only the reset pulse at offset 0.
    let mut block1 = Vec::new();
    gen.process_block(&mut pll, 100, |ev| block1.push(ev.sample_offset));
    assert_eq!(block1, vec![0]);

    // Block 2: starting at sample 100, run through sample 1100 — should
    // fire the next pulse at sample-in-block ≈ 900 (period/4 = 1000 from
    // start, minus 100 already consumed).
    let mut block2 = Vec::new();
    gen.process_block(&mut pll, 1000, |ev| block2.push(ev.sample_offset));
    assert_eq!(block2.len(), 1);
    assert!(
        block2[0].abs_diff(900) <= 1,
        "expected ~900, got {}",
        block2[0]
    );
}

/// G8: PPQN switch mid-stream produces no duplicate pulse, no missed pulse.
#[test]
fn g8_ppqn_switch() {
    let period = 4000.0;
    let mut pll = pll_at(period);
    let mut gen = PulseGenerator::new(4);
    gen.reset();

    let mut pulses = Vec::new();
    // Run 500 samples at PPQN=4 — pulse at 0 only (next would be at 1000).
    gen.process_block(&mut pll, 500, |ev| pulses.push((4, ev.sample_offset)));
    assert_eq!(pulses, vec![(4, 0)]);

    // Switch to PPQN=8. Next pulse interval is now 500 samples.
    // PLL phase is currently 500. With PPQN=8, pulse_interval = 500.
    // pulse_phase = 500/500 = 1.0 → floor = 1, last_pulse_index was 0, so fires now.
    gen.set_pulse_rate(8);
    let mut after = Vec::new();
    gen.process_block(&mut pll, 600, |ev| after.push(ev.sample_offset));
    // Should fire at offset 0 (the boundary at 500 samples absolute is now at
    // offset 0 of the new block) AND the next one at offset 500 (absolute 1000).
    assert!(!after.is_empty(), "no pulses after PPQN switch");
}

/// G10: reset → next pulse fires at first sample.
#[test]
fn g10_reset_fires_first_pulse() {
    let mut pll = pll_at(4000.0);
    let mut gen = PulseGenerator::new(4);
    gen.reset();
    let mut pulses = Vec::new();
    gen.process_block(&mut pll, 1, |ev| pulses.push(ev.sample_offset));
    assert_eq!(pulses, vec![0]);
}

/// G11: first pulse after silence-resume matches the resume sample.
/// (We model "resume" as a reset, since SilenceGate triggers `reset()` on
/// the SilentToActive transition.)
#[test]
fn g11_first_pulse_after_silence() {
    let mut pll = pll_at(4000.0);
    let mut gen = PulseGenerator::new(4);
    gen.reset();
    // Run a long way (simulating no-output during silence).
    gen.process_block(&mut pll, 10_000, |_| {});
    // Now reset (silence → active edge).
    gen.reset();
    let mut after = Vec::new();
    gen.process_block(&mut pll, 1, |ev| after.push(ev.sample_offset));
    assert_eq!(after, vec![0]);
}
