# ADR-0016: BeatPLL cold-starts by snapping to the first observed period

## Status

Accepted. Backfilled retroactively (decision made during initial unit-test
run in commit `aa3448b`).

## Context

The spec (§5.2) shows pseudocode that always smooths the period via
`period_samples = (1 - α_period) * period_samples + α_period * observed_period`,
with `period_samples` initialised to a default that maps to 120 BPM at
construction time.

The first `cargo test --test pll` run failed at 60 BPM and 220 BPM with
this scheme. With α_period = 0.09 and a 120 BPM cold start, convergence
to a target period that's a factor of 2 away takes ≈ 30 onsets to reach
within 6 % — and the spec-derived test asserted 1 % within 30 onsets.
At 60 BPM the test reported 62 BPM (6 % short of true).

The natural fix could be one of:

1. Make `α_period` larger (faster convergence, more sensitive to noise).
2. Increase the test's `n_onsets`.
3. Cold-start by snapping `period_samples = observed_period` on the first
   non-trivial onset (the second onset overall, since the first only
   establishes `last_onset_sample`).

(3) is the standard PLL acquisition-vs-tracking pattern: open-loop
acquisition until you have a usable estimate, then closed-loop tracking.
It also matches what a human listener would do: "I just heard two beats
0.5 s apart, that's 120 BPM, then I'll track from there."

## Decision

`BeatPll::on_onset` snaps `period_samples` to the first valid
`observed_period` (when `onsets_since_reset <= 1`), then smooths thereafter
using the configured `α_period`.

## Consequences

- Lock at 60–220 BPM completes in ≈ 8 onsets (the existing
  `LOCK_THRESHOLD`), not 80+. The unit tests pass with the spec's
  documented tolerances.
- A single noisy first observation can shove the PLL to a wrong tempo for
  one frame, but octave correction + the in-range bounds check catch the
  egregious cases, and the smoothing pulls it back within a few onsets.
- The `last_onset_sample = None` reset (silence-end, manual resync) also
  resets `onsets_since_reset = 0`, so re-acquisition snaps cleanly.
- Spec §5.2's pseudocode is, strictly speaking, deviated from. Recorded
  here so the deviation is intentional rather than drifted.
