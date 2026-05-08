// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Beat-tracking evaluation metrics. Rust port of the metrics defined in
//! `mir_eval.beat`. See `docs/TESTING.md` §5.2 and ADR-0014.
//!
//! All inputs are beat times in seconds, sorted ascending. Reference (ground
//! truth) and estimate are passed separately.

/// Default ±70 ms tolerance window for F-measure.
pub const F_MEASURE_TOL: f64 = 0.070;

/// Tempo accuracy tolerance: ±4 % per `mir_eval`.
pub const TEMPO_ACC_TOL: f64 = 0.04;

/// Greedy bipartite match between reference and estimate beats within
/// `±tol_seconds`. Each reference beat matches at most one estimate beat,
/// and vice versa. Returns (true_positives, false_positives, false_negatives).
fn match_beats(reference: &[f64], estimate: &[f64], tol: f64) -> (usize, usize, usize) {
    let mut used_est = vec![false; estimate.len()];
    let mut tp = 0usize;
    let mut j = 0usize;
    for &r in reference {
        // Advance j to the first estimate within tol of r.
        while j < estimate.len() && estimate[j] < r - tol {
            j += 1;
        }
        // Find the closest unused estimate within [r - tol, r + tol].
        let mut best: Option<(usize, f64)> = None;
        let mut k = j;
        while k < estimate.len() && estimate[k] <= r + tol {
            if !used_est[k] {
                let d = (estimate[k] - r).abs();
                if best.map_or(true, |(_, bd)| d < bd) {
                    best = Some((k, d));
                }
            }
            k += 1;
        }
        if let Some((idx, _)) = best {
            used_est[idx] = true;
            tp += 1;
        }
    }
    let fn_ = reference.len() - tp;
    let fp = estimate.len() - tp;
    (tp, fp, fn_)
}

/// Beat-tracking F-measure with ±tol-second tolerance.
pub fn f_measure(reference: &[f64], estimate: &[f64], tol: f64) -> f64 {
    if reference.is_empty() || estimate.is_empty() {
        return 0.0;
    }
    let (tp, fp, fn_) = match_beats(reference, estimate, tol);
    if tp == 0 {
        return 0.0;
    }
    let precision = tp as f64 / (tp + fp) as f64;
    let recall = tp as f64 / (tp + fn_) as f64;
    2.0 * precision * recall / (precision + recall)
}

/// Estimate the tempo from beat timestamps using the median inter-beat
/// interval. Returns BPM. (`mir_eval` uses a more sophisticated histogram
/// approach; the median is a robust v1 approximation.)
pub fn tempo_from_beats(beats: &[f64]) -> f64 {
    if beats.len() < 2 {
        return 0.0;
    }
    let mut intervals: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = intervals[intervals.len() / 2];
    if median > 0.0 {
        60.0 / median
    } else {
        0.0
    }
}

/// Tempo accuracy 1: estimated tempo within `tol` (relative) of reference.
pub fn tempo_accuracy_1(reference: &[f64], estimate: &[f64], tol: f64) -> bool {
    let r = tempo_from_beats(reference);
    let e = tempo_from_beats(estimate);
    if r <= 0.0 || e <= 0.0 {
        return false;
    }
    ((e - r).abs() / r) <= tol
}

/// Tempo accuracy 2: like accuracy 1 but allowing a 2× / ½× octave error.
pub fn tempo_accuracy_2(reference: &[f64], estimate: &[f64], tol: f64) -> bool {
    let r = tempo_from_beats(reference);
    let e = tempo_from_beats(estimate);
    if r <= 0.0 || e <= 0.0 {
        return false;
    }
    let direct = ((e - r).abs() / r) <= tol;
    let double = ((e - 2.0 * r).abs() / (2.0 * r)) <= tol;
    let half = ((e - 0.5 * r).abs() / (0.5 * r)) <= tol;
    direct || double || half
}

/// Continuity-based metrics. Returns (CMLt, AMLt). CMLt requires correct
/// metrical level *and* phase. AMLt allows half/double tempo and phase
/// offsets of half a beat.
///
/// Implementation follows `mir_eval.beat.continuity`:
///   - A reference beat is "tracked" if the matching estimate is within
///     ±17.5 % of the inter-beat interval, AND the previous estimate was
///     also tracked (continuity).
///   - The metric is the longest correctly-tracked region as a fraction of
///     total reference beats.
pub fn continuity(reference: &[f64], estimate: &[f64]) -> (f64, f64) {
    if reference.len() < 2 || estimate.is_empty() {
        return (0.0, 0.0);
    }

    let cmlt = continuity_at_level(reference, estimate, 1.0, 0.0);

    // AMLt: try direct, half, double; with and without half-beat offset.
    let mut best = cmlt;
    for &factor in &[0.5_f64, 1.0, 2.0] {
        for &offset_frac in &[0.0_f64, 0.5] {
            let v = continuity_at_level(reference, estimate, factor, offset_frac);
            if v > best {
                best = v;
            }
        }
    }
    (cmlt, best)
}

fn continuity_at_level(reference: &[f64], estimate: &[f64], factor: f64, offset_frac: f64) -> f64 {
    if reference.len() < 2 || estimate.len() < 2 {
        return 0.0;
    }
    let mean_ibi = (reference[reference.len() - 1] - reference[0]) / (reference.len() - 1) as f64;
    let phase_offset = mean_ibi * offset_frac;
    let tol_frac = 0.175;

    // Synthesise the expected reference beat sequence at the given
    // tempo factor and phase offset.
    let n_expected = ((reference.len() as f64) * factor).round() as usize;
    if n_expected < 2 {
        return 0.0;
    }
    let new_ibi = mean_ibi / factor;
    let expected: Vec<f64> = (0..n_expected)
        .map(|i| reference[0] + phase_offset + i as f64 * new_ibi)
        .collect();

    // Index of the closest estimate to each expected beat.
    let nearest_idx: Vec<usize> = expected
        .iter()
        .map(|&exp| {
            let mut best = 0usize;
            let mut best_d = f64::INFINITY;
            for (j, &e) in estimate.iter().enumerate() {
                let d = (e - exp).abs();
                if d < best_d {
                    best_d = d;
                    best = j;
                }
            }
            best
        })
        .collect();

    // Continuity: each expected beat is "tracked" iff
    //   (a) the closest estimate is within tol_frac * new_ibi, AND
    //   (b) the inter-estimate interval at that index is within tol_frac
    //       of new_ibi (i.e. estimate is at the right tempo locally).
    // The metric is the longest contiguous run of tracked beats.
    let mut max_run = 0usize;
    let mut run = 0usize;
    for (i, &exp) in expected.iter().enumerate() {
        let j = nearest_idx[i];
        let phase_ok = (estimate[j] - exp).abs() <= tol_frac * new_ibi;
        let tempo_ok = if j > 0 {
            ((estimate[j] - estimate[j - 1]) - new_ibi).abs() <= tol_frac * new_ibi
        } else if j + 1 < estimate.len() {
            ((estimate[j + 1] - estimate[j]) - new_ibi).abs() <= tol_frac * new_ibi
        } else {
            false
        };
        if phase_ok && tempo_ok {
            run += 1;
            if run > max_run {
                max_run = run;
            }
        } else {
            run = 0;
        }
    }
    max_run as f64 / expected.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn beats_at(bpm: f64, n: usize, start: f64) -> Vec<f64> {
        let dt = 60.0 / bpm;
        (0..n).map(|i| start + i as f64 * dt).collect()
    }

    #[test]
    fn f_measure_perfect_match() {
        let r = beats_at(120.0, 30, 0.5);
        assert_relative_eq!(f_measure(&r, &r, F_MEASURE_TOL), 1.0, epsilon = 1e-9);
    }

    #[test]
    fn f_measure_no_overlap() {
        let r = beats_at(120.0, 30, 0.5);
        let e = beats_at(120.0, 30, 50.0);
        assert_eq!(f_measure(&r, &e, F_MEASURE_TOL), 0.0);
    }

    #[test]
    fn f_measure_half_match() {
        let r = beats_at(120.0, 30, 0.5);
        // Estimate every other reference beat.
        let e: Vec<f64> = r.iter().step_by(2).copied().collect();
        let f = f_measure(&r, &e, F_MEASURE_TOL);
        // 15 TP, 0 FP, 15 FN → P=1, R=0.5, F=2/3
        assert_relative_eq!(f, 2.0 / 3.0, epsilon = 1e-9);
    }

    #[test]
    fn f_measure_within_tolerance() {
        let r = beats_at(120.0, 30, 0.5);
        // Shift estimate by 50 ms — within 70 ms tolerance.
        let e: Vec<f64> = r.iter().map(|t| t + 0.050).collect();
        let f = f_measure(&r, &e, F_MEASURE_TOL);
        assert_relative_eq!(f, 1.0, epsilon = 1e-9);
    }

    #[test]
    fn f_measure_outside_tolerance() {
        let r = beats_at(120.0, 30, 0.5);
        // Shift by 100 ms — outside 70 ms tolerance.
        let e: Vec<f64> = r.iter().map(|t| t + 0.100).collect();
        assert_eq!(f_measure(&r, &e, F_MEASURE_TOL), 0.0);
    }

    #[test]
    fn tempo_estimate_120() {
        let r = beats_at(120.0, 30, 0.5);
        assert_relative_eq!(tempo_from_beats(&r), 120.0, epsilon = 1e-6);
    }

    #[test]
    fn tempo_accuracy_1_within_tol() {
        let r = beats_at(120.0, 30, 0.0);
        let e = beats_at(122.0, 30, 0.0); // 1.7 % off
        assert!(tempo_accuracy_1(&r, &e, TEMPO_ACC_TOL));
    }

    #[test]
    fn tempo_accuracy_1_outside_tol() {
        let r = beats_at(120.0, 30, 0.0);
        let e = beats_at(140.0, 30, 0.0); // 16.7 % off
        assert!(!tempo_accuracy_1(&r, &e, TEMPO_ACC_TOL));
    }

    #[test]
    fn tempo_accuracy_2_octave_error_ok() {
        let r = beats_at(120.0, 30, 0.0);
        let e = beats_at(240.0, 60, 0.0);
        assert!(tempo_accuracy_2(&r, &e, TEMPO_ACC_TOL));
        let e2 = beats_at(60.0, 15, 0.0);
        assert!(tempo_accuracy_2(&r, &e2, TEMPO_ACC_TOL));
    }

    #[test]
    fn continuity_perfect_match() {
        let r = beats_at(120.0, 30, 0.5);
        let (cmlt, amlt) = continuity(&r, &r);
        assert_relative_eq!(cmlt, 1.0, epsilon = 1e-6);
        assert_relative_eq!(amlt, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn continuity_octave_error_amlt_only() {
        let r = beats_at(120.0, 30, 0.5);
        let e = beats_at(240.0, 60, 0.5);
        let (cmlt, amlt) = continuity(&r, &e);
        // CMLt should be low (wrong metrical level), AMLt should be high.
        assert!(cmlt < 0.5, "cmlt={cmlt} expected < 0.5");
        assert!(amlt > 0.8, "amlt={amlt} expected > 0.8");
    }
}
