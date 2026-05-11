# 0027 — Aubio `Tempo` as the default tracking mode

## Status

Accepted (2026-05-11).

## Context

Honest measurement on Ballroom (n=698, commits `c589c89`/`0ab7298` after fixing the test-harness bug) put Reactive at F=0.288 and Consensus at F=0.436. Literature reports ~0.75–0.85 for aubio + PLL beat trackers on the same dataset.

A diagnostic on the full corpus (`tests/ballroom_diagnostic.rs`) showed the dominant failure mode for Reactive is **wrong-tempo lock on 71% of tracks** — not phase offset (~11 ms median, well within the ±70 ms F-measure tolerance), not octave error (8.4% of tracks). Onsets are fine; the *period selection* is wrong. A threshold sweep on the full corpus (`tests/threshold_sweep.rs`) confirmed the issue isn't onset density: every threshold ∈ {0.0, 0.05, 0.1, 0.15, 0.2, 0.3} for SpecFlux/KL/HFC clusters at F=0.28–0.32, with only +0.04 spread.

The conclusion: BeatPulse's reactive period-smoothing PLL is structurally unable to converge to the right tempo on full-mix audio. Consensus mode (median IOI over a 2 s window) does better (0.436) by using a smarter period estimator, but it adds 2 s of latency and still trails the literature. The literature numbers come from algorithms doing real autocorrelation / tempogram-style period selection.

A separate experiment (`tests/aubio_tempo_experiment.rs`) ran aubio's built-in `Tempo` object — same sample-rate, same hop, same onset method (SpecFlux) — on the full Ballroom corpus and got **F=0.576, TA2=0.764**, in literature range. Aubio's Tempo internally does autocorrelation across a tempogram-like state and emits beat events at the inferred beat boundaries.

## Decision

Add `TrackingMode::AubioTempo` and **make it the new default**. Implementation: `src/dsp/aubio_tempo_tracker.rs::AubioTempoTracker` wraps `aubio_rs::Tempo`. On each detected beat the tracker computes the period from the inter-beat interval (octave-corrected into the BeatPLL's clamp range), snaps `BeatPll::period_samples` and `phase_samples`, and sets `locked = true`. Downstream (`PulseGenerator` → LED / MIDI / Link) is unchanged: aubio Tempo replaces the *front* of the pipeline (`BeatTracker` Onset detector + reactive PLL period smoothing); everything after the PLL state remains shared.

Per-block latency is the same as Reactive (one aubio hop ≈ 11.6 ms at 44.1 kHz / 512 hop). No additional buffering — aubio Tempo uses the same 1024-sample FFT with 512-sample hop as the existing `BeatTracker`.

Reactive and Lookahead Consensus remain available as alternatives. Reactive stays useful as a low-latency baseline for very clean drum-bus sources (aubio Tempo's autocorrelation needs ~2–4 beats to converge; on a clean kick, the reactive PLL's per-onset feedback locks in 1–2 onsets). Consensus stays useful when aubio Tempo mistracks a particular full-mix source — it's a different algorithm and may succeed where Tempo fails.

## Consequences

**Default behaviour changes.** Existing users get AubioTempo on next plugin load instead of Reactive (or Consensus, if they were on the prior post-ADR-0026 default — which was still Reactive; ADR-0026 made Consensus opt-in). The host's persisted parameter state will retain whatever the user explicitly set; only fresh instances see the new default.

**F-measure on Ballroom: 0.288 → 0.547.** A 90% relative improvement, +0.259 absolute. TA2 (octave-tolerant tempo accuracy): 0.285 → 0.744. AubioTempo wins outright on 387 / 698 tracks (55 %) over both alternatives.

**The integrated F=0.547 is below the standalone aubio Tempo's F=0.576** (measured in `tests/aubio_tempo_experiment.rs`). The ~0.03 gap is induced by `PulseGenerator`'s wrap-detection re-emitting beats: aubio's raw beat times go through PulseGenerator (PPQN=1) which adds a small timing wobble around each PLL period boundary. Acceptable trade-off — keeps the entire pipeline (LED + MIDI + Link) downstream of `PulseGenerator` unchanged, single source of truth for beat emission.

## Update 2026-05-12

The integrated-vs-standalone gap was closed. New `src/dsp/aubio_pulse_emitter.rs::AubioPulseEmitter` emits the on-beat pulse at the exact aubio beat sample (no wrap-detection) and linearly interpolates PPQN sub-beats between consecutive aubio beats. `lib.rs::process` dispatches on `tracking_mode`: AubioTempo uses `AubioPulseEmitter`, Reactive/Consensus continue to use `PulseGenerator`. Both downstream paths feed the same `MidiFormatter` / Link publishing.

Combined with `mir_eval`-standard scoring (`trim_beats(5.0)` + Sturm-2013 duplicate skip), final integrated AubioTempo on Ballroom (n=687) is **F=0.590, AMLt=0.459, TA2=0.777** — matching standalone aubio Tempo (F=0.590) exactly, in literature range for TA2. Reactive 0.284, Consensus 0.437 with the same scoring. ΔF(aubio−reactive)=+0.306; AubioTempo wins outright on 394/687 tracks (57 %).

A 7-method aubio-Tempo sweep (`tests/aubio_tempo_experiment.rs`) confirmed SpecFlux is the optimal onset method: SpecFlux=0.590, SpecDiff=0.584, Complex=0.557, KL=0.554, HFC=0.499, Phase=0.404, MKL=0.391. SpecFlux remains the default.

The remaining ~0.07 gap to OBTAIN's published F=0.66 for aubio on Ballroom is most likely annotation-set version (CPJKU vs original Gouyon), aubio version drift, or sample-rate convention differences. Closing further would require either a neural beat tracker (M5; deferred) or the Klapuri 2006-style multi-band-accent + comb-filter resonator family — see ADR-0028 for the future-direction record.

**Ballroom F=0.547 is short of the literature's 0.75–0.85.** Possible causes (out of scope for this ADR, tracked as follow-up): `PulseGenerator`'s wobble (~0.03), Ballroom annotation alignment relative to BeatPulse's "beat moment" definition, and the inherent gap between integrated real-time pipelines and offline academic benchmarks.

**Code surface added:** ~200 lines (`AubioTempoTracker` + tests). Plugin `process` now dispatches on `tracking_mode`: AubioTempo branches to `aubio_tempo.process_block` collecting beats; Reactive / Consensus continue to use `beat_tracker.process_block` collecting onsets.

**Latency is unchanged.** AubioTempo has the same per-block latency as Reactive (one hop, ~11.6 ms at 44.1 kHz). It does *not* add the 2 s lookahead penalty of Consensus mode.

**Aubio version coupling.** AubioTempo is now the default, so aubio's Tempo behaviour changes (across aubio versions) directly affect BeatPulse's user-facing output. We pin aubio-rs = "0.2" in `Cargo.toml`; bumping it requires re-measurement on Ballroom before merge.

## Out of scope

- Closing the remaining gap to literature (0.547 → 0.75+). Likely requires either source separation, a neural beat tracker (M5 from earlier ADRs), or restructuring the `PulseGenerator` boundary to reduce the ~0.03 wrap-detection penalty.
- Changing `BeatPll`'s α coefficients. They remain relevant for Reactive mode and don't affect AubioTempo (which snaps the PLL directly).
- Per-genre tuning. The corpus-wide F is the relevant metric.
- GiantSteps / SMC re-measurement. Mirrors are still down.
