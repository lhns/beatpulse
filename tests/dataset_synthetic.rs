// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Always-runs CI gate against in-memory synthetic click tracks at known
//! BPMs. See `docs/TESTING.md` §5 and ADR-0014.
//!
//! Audio is generated in-memory rather than committed to the repo so the
//! test remains hermetic and the repo stays small. The signal is a
//! kick-drum-like 60 Hz sine with exponential decay, identical to the one
//! in `tests/beat_tracker.rs`.

use beatpulse::dsp::beat_pll::BeatPll;
use beatpulse::dsp::beat_tracker::BeatTracker;
use beatpulse::dsp::pulse_generator::PulseGenerator;
use beatpulse::eval::{f_measure, tempo_accuracy_2, F_MEASURE_TOL, TEMPO_ACC_TOL};
use beatpulse::params::OnsetMethod;

const SR: f64 = 44_100.0;

fn make_clicks(total_samples: usize, click_samples: &[usize]) -> Vec<f32> {
    let mut buf = vec![0.0f32; total_samples];
    let click_len = (0.050 * SR as f32) as usize;
    let decay_tau = (0.020 * SR as f32) as f32;
    for &t in click_samples {
        for i in 0..click_len {
            let pos = t + i;
            if pos < total_samples {
                let phase =
                    2.0 * std::f32::consts::PI * 60.0 * (i as f32) / SR as f32;
                let env = (-(i as f32) / decay_tau).exp();
                buf[pos] += 0.8 * env * phase.sin();
            }
        }
    }
    buf
}

/// Run the BeatTracker→PLL pipeline against a click track at `bpm`.
/// Returns (estimated_beat_times_seconds, reference_beat_times_seconds).
fn run_at_bpm(bpm: f64, n_seconds: f64) -> (Vec<f64>, Vec<f64>) {
    let total = (n_seconds * SR) as usize;
    let beat_samples_f = SR * 60.0 / bpm;
    let mut click_samples = Vec::new();
    let mut t = 0.5 * SR;
    while (t as usize) + 1 < total {
        click_samples.push(t as usize);
        t += beat_samples_f;
    }
    let signal = make_clicks(total, &click_samples);
    let reference: Vec<f64> = click_samples.iter().map(|&s| s as f64 / SR).collect();

    let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut pll = BeatPll::new(SR);
    let mut estimated: Vec<f64> = Vec::new();
    let mut absolute = 0u64;

    let block = 512;
    for chunk in signal.chunks(block) {
        // Beat tracker emits onsets; we feed each onset to the PLL and
        // collect its time as the beat-time estimate.
        let abs_at_block = absolute;
        tracker.process_block(chunk, |offset, _frac| {
            let abs_sample = abs_at_block + offset as u64;
            pll.on_onset(abs_sample as f64);
            estimated.push(abs_sample as f64 / SR);
        });
        pll.advance(chunk.len() as u64);
        absolute += chunk.len() as u64;
    }

    (estimated, reference)
}

/// PulseGenerator emission rate must match `BPM/60 × PPQN` once the PLL
/// is locked, NOT the rate of incoming onsets. This is the user-visible
/// behaviour behind the PULSE LED — pre-fix, every detected onset triggered
/// a phantom pulse, making the LED flicker at onset rate; post-fix the
/// LED flashes at PPQN rate.
#[test]
fn pulse_rate_matches_ppqn_post_lock() {
    const BPM: f64 = 120.0;
    const PPQN: u32 = 4;
    const DURATION_S: f64 = 30.0;
    const BLOCK: u32 = 512;

    let total_samples = (DURATION_S * SR) as usize;
    let beat_period = SR * 60.0 / BPM;
    let mut click_samples = Vec::new();
    let mut t = 0.5 * SR;
    while (t as usize) + 1 < total_samples {
        click_samples.push(t as usize);
        t += beat_period;
    }
    let signal = make_clicks(total_samples, &click_samples);

    let mut tracker = BeatTracker::new(SR as u32, OnsetMethod::SpecFlux, 0.3).unwrap();
    let mut pll = BeatPll::new(SR);
    let mut gen = PulseGenerator::new(PPQN);
    gen.reset();

    // Sample at which the PLL first locks (= absolute sample of the
    // 8th onset, per LOCK_THRESHOLD in BeatPll).
    let mut lock_sample: Option<u64> = None;
    let mut absolute = 0u64;
    let mut pulses_after_lock = 0u32;

    for chunk in signal.chunks(BLOCK as usize) {
        for i in 0..chunk.len() {
            let abs = absolute + i as u64;
            // Beat tracker accumulates a hop at a time; emit the onset
            // and feed it to the PLL.
            // (We reuse process_block per-block below for simplicity.)
            let _ = abs;
        }
        let abs_at_block = absolute;
        let mut onsets_in_block: Vec<(u32, u64)> = Vec::new();
        tracker.process_block(chunk, |offset, _frac| {
            onsets_in_block.push((offset, abs_at_block + offset as u64));
        });
        let mut next_onset = 0usize;
        for i in 0..chunk.len() as u32 {
            while next_onset < onsets_in_block.len()
                && onsets_in_block[next_onset].0 == i
            {
                pll.on_onset(onsets_in_block[next_onset].1 as f64);
                if pll.locked && lock_sample.is_none() {
                    lock_sample = Some(onsets_in_block[next_onset].1);
                }
                next_onset += 1;
            }
            pll.advance_one();
            if let Some(_ev) = gen.observe_advance(&pll, i) {
                if lock_sample.is_some() {
                    pulses_after_lock += 1;
                }
            }
        }
        absolute += chunk.len() as u64;
    }

    let lock_at = lock_sample.expect("PLL never locked on a clean 120 BPM click track");
    let post_lock_seconds = (total_samples as u64 - lock_at) as f64 / SR;

    // Expected pulses ≈ duration × BPM/60 × PPQN.
    let expected = post_lock_seconds * BPM / 60.0 * PPQN as f64;
    let observed = pulses_after_lock as f64;
    let ratio = observed / expected;

    // Allow ±10 % — onsets that arrive slightly after lock can shift the
    // first counted pulse, and the test window is finite. Pre-fix this
    // test would observe ratio ≈ 1.5–2.0 because every onset added a
    // phantom pulse; post-fix it sits at ~1.0.
    assert!(
        ratio > 0.90 && ratio < 1.10,
        "post-lock pulse rate is wrong: observed {observed}, expected ~{expected:.0} \
         (ratio {ratio:.2}). Lock at sample {lock_at}, window {post_lock_seconds:.1}s."
    );
}

/// Per-BPM gates: F-measure ≥ 0.9, tempo accuracy 2 (octave-tolerant) true.
#[test]
fn synthetic_clicks_per_bpm_gates() {
    let bpms = [90.0_f64, 110.0, 120.0, 140.0, 160.0, 180.0];
    let mut aggregate_f = 0.0;
    for &bpm in &bpms {
        let (est, refr) = run_at_bpm(bpm, 30.0);
        let f = f_measure(&refr, &est, F_MEASURE_TOL);
        assert!(
            f >= 0.90,
            "bpm={bpm}: F-measure {f:.3} < 0.90 ({} ref, {} est)",
            refr.len(),
            est.len()
        );
        assert!(
            tempo_accuracy_2(&refr, &est, TEMPO_ACC_TOL),
            "bpm={bpm}: tempo accuracy 2 failed"
        );
        aggregate_f += f;
    }
    let aggregate = aggregate_f / bpms.len() as f64;
    assert!(
        aggregate >= 0.93,
        "aggregate F-measure {aggregate:.3} < 0.93"
    );
}
