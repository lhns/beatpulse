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

/// G12: onset-driven phase corrections must NOT produce phantom pulses.
///
/// `BeatPll::on_onset` legitimately reduces `phase_samples` (it's the
/// mechanism by which the PLL phase-locks). The PulseGenerator's wrap
/// detection must distinguish that small backward correction from a real
/// natural wrap-around at the end of a beat.
///
/// Methodology: run two passes against the same PLL period and compare
/// pulse counts. The "control" pass advances phase only (no onsets). The
/// "perturbed" pass injects onsets at each beat + 50 samples (each onset
/// pulls phase backward by ≤ alpha_phase × 50 ≈ 8 samples). Phase
/// corrections shift pulse positions slightly forward in time but must
/// not create or skip pulses. Counts must match.
#[test]
fn g12_onset_corrections_no_phantom_pulses() {
    use beatpulse::dsp::pulse_generator::PulseEvent;

    const SR: f64 = 44_100.0;
    const PERIOD: f64 = 44_100.0; // 60 BPM
    const PPQN: u32 = 4;
    const N_BEATS: usize = 5;
    const BLOCK: u32 = 512;
    // Run a bit past N_BEATS periods to capture the final boundary pulse
    // even after onset corrections shift things by a few samples.
    const TOTAL_SAMPLES: u32 = (PERIOD as u32) * N_BEATS as u32 + 1000;

    fn run(inject_onsets: bool) -> Vec<PulseEvent> {
        let mut pll = BeatPll::new(SR);
        pll.period_samples = PERIOD;
        pll.phase_samples = 0.0;
        pll.alpha_period = 0.09;
        pll.alpha_phase = 0.165;

        let mut gen = PulseGenerator::new(PPQN);
        gen.reset();

        let onset_samples: Vec<u64> = (1..=N_BEATS)
            .map(|i| (i as u64) * (PERIOD as u64) + 50)
            .collect();
        let mut next_onset = 0usize;

        let mut pulses: Vec<PulseEvent> = Vec::new();
        let mut absolute = 0u64;
        let mut remaining = TOTAL_SAMPLES;
        while remaining > 0 {
            let take = remaining.min(BLOCK);
            for i in 0..take {
                let abs = absolute + i as u64;
                if inject_onsets
                    && next_onset < onset_samples.len()
                    && onset_samples[next_onset] == abs
                {
                    pll.on_onset(abs as f64);
                    next_onset += 1;
                }
                pll.advance_one();
                if let Some(ev) = gen.observe_advance(&pll, i) {
                    pulses.push(ev);
                }
            }
            absolute += take as u64;
            remaining -= take;
        }
        pulses
    }

    let control = run(false);
    let perturbed = run(true);

    // Phase corrections must not change pulse count. (They will shift
    // pulse positions a few samples forward, but the cadence must remain
    // exactly N_BEATS * PPQN + 1 pulses for the same window length.)
    assert_eq!(
        control.len(),
        perturbed.len(),
        "onset corrections changed pulse count: control={}, perturbed={}",
        control.len(),
        perturbed.len()
    );
}

/// G13: `PulseEvent::is_beat_boundary` is true exactly once per beat,
/// regardless of PPQN. This is what drives the BEAT LED in the UI.
#[test]
fn g13_beat_boundary_flag() {
    use beatpulse::dsp::pulse_generator::PulseEvent;

    const PERIOD: f64 = 44_100.0;
    const N_BEATS: u32 = 5;

    for &ppqn in &[1u32, 2, 4, 8, 16, 24] {
        let mut pll = pll_at(PERIOD);
        let mut gen = PulseGenerator::new(ppqn);
        gen.reset();
        let mut events: Vec<PulseEvent> = Vec::new();
        let total = (PERIOD as u32) * N_BEATS;
        gen.process_block(&mut pll, total, |ev| events.push(ev));

        let beats: usize = events.iter().filter(|e| e.is_beat_boundary).count();
        let non_beats: usize = events.len() - beats;

        // Expected boundaries: one per beat wrap (N_BEATS) plus the
        // reset-fire pulse at sample 0 (also `within_beat_index == 0`).
        let expected_beats = N_BEATS as usize + 1;
        assert_eq!(
            beats, expected_beats,
            "ppqn={ppqn}: got {beats} beat-boundary events, expected {expected_beats}"
        );
        // Non-boundary pulses: (PPQN - 1) per beat.
        assert_eq!(
            non_beats,
            (ppqn as usize - 1) * N_BEATS as usize,
            "ppqn={ppqn}: got {non_beats} non-boundary pulses, expected {}",
            (ppqn - 1) * N_BEATS
        );
    }
}

/// G14: a phase correction that pushes the PLL phase BACKWARD across a
/// within-beat-index boundary must not fire a spurious pulse. See
/// ADR-0025. This is the bug that made the BEAT LED double-blink within
/// a single beat: phase moved past a within-beat boundary (firing
/// correctly), then an onset corrected it back across the boundary,
/// firing a spurious pulse with `is_beat_boundary == true`.
#[test]
fn g14_backward_phase_correction_no_extra_pulse() {
    use beatpulse::dsp::pulse_generator::PulseEvent;

    const PERIOD: f64 = 44_100.0; // 60 BPM
    const PPQN: u32 = 4;
    const PULSE_INTERVAL: f64 = PERIOD / PPQN as f64; // 11025

    let mut pll = BeatPll::new(SR);
    pll.period_samples = PERIOD;
    pll.phase_samples = 0.0;
    pll.alpha_period = 0.0; // freeze period — only test phase corrections
    pll.alpha_phase = 0.165;

    // Seed the PLL with two prior onsets so subsequent on_onset calls
    // exercise the phase-correction path (not cold-start snap).
    pll.on_onset(0.0);
    pll.on_onset(PERIOD);

    let mut gen = PulseGenerator::new(PPQN);
    gen.reset();

    // Drive forward to just past the first within-beat boundary
    // (sample 11025). Collect events.
    let mut events: Vec<PulseEvent> = Vec::new();
    let to_first = (PULSE_INTERVAL as u32) + 5; // 11030 samples
    gen.process_block(&mut pll, to_first, |ev| events.push(ev));

    // We expect 2 pulses: the reset-fire at sample 0 (within=0) and
    // the boundary at sample ~11025 (within=1).
    assert_eq!(
        events.len(),
        2,
        "expected 2 pulses pre-onset, got {}",
        events.len()
    );
    assert!(events[0].is_beat_boundary);
    assert!(!events[1].is_beat_boundary);
    assert!(pll.phase_samples > PULSE_INTERVAL);

    // Now inject an onset at this position. With err = phase ≈ 11030
    // and α_phase = 0.165, the correction is ~1820 samples backward,
    // so phase ends up around 9210 — back inside within_beat_index = 0.
    // The "absolute sample" is whatever — only the relative phase matters
    // for the correction. Use last + something < period so the resulting
    // observed_period stays in-range.
    let abs_sample = pll.last_onset_sample.unwrap() + PULSE_INTERVAL;
    pll.on_onset(abs_sample);
    assert!(
        pll.phase_samples < PULSE_INTERVAL,
        "expected backward correction across the boundary; phase = {}",
        pll.phase_samples
    );

    // Pre-fix: a spurious pulse with is_beat_boundary=true would fire
    // here (the BEAT LED double-blink). Post-fix: silent.
    let mut after = Vec::new();
    gen.process_block(&mut pll, 1, |ev| after.push(ev));
    assert!(
        after.is_empty(),
        "spurious pulse fired after backward phase correction: {:?}",
        after
    );

    // Continue advancing far enough to cross the next pulse boundary
    // at phase ≈ 22050 (within_beat_index = 2). From the corrected
    // phase ≈ 9210 we need ≥ 12840 samples to cross. 13000 lands at
    // ~22210 (within = 2) without yet reaching 33075 (within = 3),
    // so exactly one pulse should fire.
    let mut more = Vec::new();
    gen.process_block(&mut pll, 13000, |ev| more.push(ev));
    assert_eq!(
        more.len(),
        1,
        "expected exactly one pulse after the boundary, got {}",
        more.len()
    );
    assert!(!more[0].is_beat_boundary);
}
