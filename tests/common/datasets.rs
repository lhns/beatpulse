// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Dataset-specific helpers shared across `tests/dataset_*.rs`.

#![allow(dead_code)]

/// File stems of duplicate audio files in the Ballroom dataset, per
/// Sturm 2013 (*A simple method to determine if a music information
/// retrieval system is a 'horse'*). Removing them prevents the same
/// recording from contributing twice to aggregate metrics.
///
/// Documented at https://github.com/CPJKU/BallroomAnnotations and in
/// Sturm's appendix.
pub const BALLROOM_DUPLICATES: &[&str] = &[
    "Albums-Ballroom_Classics4-12",
    "Albums-Cafe_Paradiso-08",
    "Albums-Cafe_Paradiso-09",
    "Albums-Cafe_Paradiso-13",
    "Albums-Chrisanne1-13",
    "Albums-Chrisanne2-12",
    "Albums-Chrisanne3-04",
    "Albums-Fire-08",
    "Albums-Latin_Jam-11",
    "Albums-Latin_Jam-9",
    "Albums-Latin_Jam2-09",
    "Albums-Latin_Jam2-13",
    "Albums-Latin_Jam3-02",
];

/// Standard `min_t` for `beatpulse::eval::trim_beats` — matches
/// `mir_eval.beat.trim_beats` default. Apply to both reference and
/// estimate before scoring on Ballroom / SMC / GiantSteps.
pub const TRIM_BEATS_MIN_T: f64 = 5.0;

/// Returns true if `track_stem` is one of the known Ballroom duplicates
/// and should be skipped for scoring.
pub fn is_ballroom_duplicate(track_stem: &str) -> bool {
    BALLROOM_DUPLICATES.contains(&track_stem)
}
