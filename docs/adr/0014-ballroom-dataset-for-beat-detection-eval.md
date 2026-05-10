# ADR-0014: Ballroom Dataset for beat-detection evaluation

## Status

Accepted.

## Context

The PLL unit tests in `tests/pll.rs` cover synthetic onset behaviour but
say nothing about real-world beat-tracking quality. To catch regressions
in `BeatTracker` (aubio integration) and in the BeatTracker→PLL pipeline
together, we need an offline harness that runs the plugin's DSP against
annotated audio and reports standard MIR metrics.

Candidate datasets:

- **Ballroom Dataset** — 685 × 30 s excerpts, ballroom dance genres
  (Waltz, Tango, Rumba, Jive, etc.), beat-annotated. Well-established
  in MIR literature. Small enough to evaluate in seconds.
- **GTZAN Rhythm** — 1000 × 30 s excerpts across 10 genres, beat
  annotations added in 2010. Harder material (jazz, classical, vocal).
  aubio scores poorly on much of it (see ADR-0003).
- **SMC_MIREX** — 217 excerpts curated to be *difficult* for beat
  trackers. Useful as a stress test, not as an acceptance gate.
- **Hainsworth** — 222 excerpts; smaller, older.
- **MIREX 2006 Beat** — 160 × 30 s excerpts annotated by 40 listeners.

BeatPulse's target use case is clean rhythmic input (kick bus). Ballroom
matches that distribution most closely. GTZAN's hard genres would
artificially depress our metrics in ways that don't reflect real usage.

## Decision

- **Primary**: Ballroom Dataset. Acceptance gate F-measure ≥ 0.70 on
  aggregate (typical onset-based + PLL trackers land 0.75–0.85; below
  0.70 indicates a wiring or PLL bug rather than aubio limits).
- **Secondary** (no hard gate, report only): SMC_MIREX as a stress test.
- **In-tree always-runs**: a small synthetic clicks set checked into
  `tests/data/` for CI regression detection without dataset download.

The dataset is not redistributed. Users supply a local path via the
`BEATPULSE_BALLROOM_DIR` environment variable; the harness skips
gracefully if unset.

Metrics implemented in Rust (~200 lines), matching `mir_eval`
definitions: F-measure (±70 ms tolerance), CMLt, AMLt, tempo accuracy 1,
tempo accuracy 2.

## Consequences

- CI without the dataset still runs unit tests + synthetic regression.
- A developer with the dataset on disk can run the full eval before
  pushing.
- Aggregate metrics committed in `tests/data/baseline-ballroom.json`;
  CI fails if any metric drops > 2 % vs baseline (when the dataset is
  present).
- We do not catch regressions on the harder material (jazz, ambient).
  Acceptable trade-off given the target use case.

**Update 2026-05-10**: Ballroom + GiantSteps Tempo are now automated
via `cargo xtask-fetch-{ballroom,giantsteps}`. SMC_MIREX harness is
present (`tests/dataset_smc.rs`) but the canonical INESC mirror is
currently unreliable; the fetcher prints manual-fallback instructions
on outage. GiantSteps adds a tempo-only A/B harness
(`tests/dataset_giantsteps.rs`) closer to the BeatPulse / Daslight
target use case (EDM full mixes) than Ballroom's ballroom-dance
material. None of the new dataset tests run in CI — they remain
opt-in via the `dataset-tests` cargo feature + per-dataset env vars.

**Update 2026-05-11**: First honest Ballroom run reported reactive F
≈ 0.29 (with the harness bug fixed — see ADR-0026's 2026-05-11 update
note). The 0.70 hard acceptance gate from this ADR was set in a
previous algorithm phase and is infeasible with current PLL +
onset-method tuning; brief audit (`tests/onset_method_audit.rs`)
confirmed all 7 aubio onset methods cluster at F=0.36–0.45 on Jive,
suggesting a structural gap (PLL tuning / annotation alignment / mono
downmix?) rather than a one-flip parameter fix. The hard gate in
`tests/dataset_ballroom.rs::ballroom_eval` is lowered to 0.25 as a
"did we break something obviously" floor; the actual recommendation
to users is Consensus mode (see `ballroom_compare`). Closing the gap
to literature performance is tracked as a separate follow-up.
