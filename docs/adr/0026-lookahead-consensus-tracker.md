# 0026 — Lookahead-consensus tracker as opt-in alternative to per-onset PLL feedback

## Status

Accepted.

## Context

aubio's per-onset PLL feedback (the original `Reactive` path; ADR-0004) reacts immediately to every onset and works well on clean rhythmic input — kick busses, drum sub-mixes, click tracks. On full-mix audio it degrades visibly: the PLL chases sub-rhythms, vocal attacks, and harmonic onsets, producing a noisy BPM display and visible LED jitter. The Stability slider (ADR-0024) smooths the PLL after the fact but cannot fix bad onset *observations* — only their downstream effect.

Discussion in chat surfaced the lever: the user is willing to trade real-time responsiveness for a more confident tempo estimate, *if the latency is known and compensable*. The existing `latency_offset_ms` parameter (ADR-0023) already provides the compensation lever. That unlocks a fundamentally different tracker: buffer onsets over a 1–3 s window, derive a consensus tempo from their inter-onset-interval distribution, and commit that to the PLL periodically — rather than reacting to each onset. Standard MIR technique; same family as tempogram-based methods, simplified to median IOI for v1.

## Decision

Add an opt-in `TrackingMode::LookaheadConsensus` parameter alongside the existing per-onset behaviour (`TrackingMode::Reactive`, the default).

The consensus path:

1. `ConsensusTracker` (`src/dsp/consensus_tracker.rs`) buffers absolute-sample positions of detected onsets in a `VecDeque`.
2. Each block, prune onsets older than the configurable lookahead window (`lookahead_ms`, range 200–3000 ms, default 2000 ms).
3. Compute consecutive inter-onset intervals; octave-correct each into the PLL's `[min_period, max_period]` (60–220 BPM); drop residual outliers.
4. Take the **median** of the corrected IOIs as the consensus period. Median is robust to up to ~50 % outliers — typical for full-mix onset noise — without the parameter-tuning burden of a histogram or autocorrelation peak-pick.
5. Apply a 2-frame stability gate: only commit when two consecutive consensus computations agree within ±0.5 %. Prevents thrashing on transient onset density changes.
6. On commit: snap the PLL period directly (no α_period smoothing — consensus is already smoothed). Snap phase from the most recent buffered onset.

Per-onset PLL feedback is bypassed in this mode; every other downstream stage (PulseGenerator, MIDI emission, Link publishing, the per-sample phase advance) is unchanged. The branch lives entirely inside `Plugin::process` as a `match` on `params.tracking_mode.value()`.

## Consequences

- Default behaviour is unchanged: existing users see exactly the same output until they explicitly switch the tracking mode in the UI. `defaults_match_spec` continues to pass.
- The new mode adds approximately `lookahead_ms` of effective latency on top of the existing PLL latency. The user compensates via the existing `latency_offset_ms` slider; the documented rule of thumb is `≈ -lookahead_ms / 2`, refined by ear against the downstream chain.
- BPM display steadies on noisy / full-mix input. F-measure on full-mix material is expected to improve modestly (5–10 % in the analysis from chat); the heavier lift remains the neural-tracker option (M5) which is deferred.
- Tempo response is slower: a real tempo change now takes up to `2 × lookahead_ms` to commit (one window to fill, one to confirm). Acceptable for the DJ-set / DMX use case where tempo doesn't typically change abruptly.
- New code surface is small: ~200 lines incl. tests in `consensus_tracker.rs`, plus a `match` branch and a UI dropdown / slider.
- `latency_offset_ms` is *not* auto-coupled to `lookahead_ms`. Coupling parameters silently is a UX footgun when the user re-tunes the lookahead — the user calibrates manually.
- Why median, not autocorrelation: simpler to reason about, no additional tunables, robust on the typical 10–30 onset buffer sizes. If real-world use shows median fails on specific genres (heavy syncopation, long rests), revisit with autocorrelation in v1.x.
- Why snap, not smooth, on commit: the consensus value is itself a smoothed estimate. Running it through the α_period EMA again would slow tempo changes unnecessarily and defeat the lookahead's whole point.
- Why a 2-frame stability gate: prevents the tracker from committing every single block whenever the IOI buffer happens to differ slightly. Cold start consequently takes up to `2 × lookahead_ms` before the first commit; afterwards updates are near-instantaneous.

## Out of scope

Neural beat tracker (option 2 from chat) — separate, larger project (M5). Source separation upstream — explicitly skipped per chat. Tempogram / autocorrelation alternatives to median IOI — over-engineering for v1; revisit if median proves inadequate. Auto-setting `latency_offset_ms` based on `lookahead_ms` — silent parameter coupling rejected as a UX footgun.

## Update 2026-05-11

The first Ballroom A/B run (commit `c589c89`, 2026-05-10) reported reactive F=0.346, consensus F=0.536, ΔF=+0.190 with consensus better on 96 % of tracks. **Those numbers were inflated by a test-harness bug.** `tests/common/mod.rs::run_pipeline` used a naive wrap detector (`phase_samples < prev_phase`) that fired on every small backward phase correction `BeatPll::on_onset` makes. Reactive mode (which calls `on_onset` per detected onset) was therefore emitting a flood of false-positive "beats", many of which coincidentally landed near true beats and inflated F-measure recall; consensus mode (which doesn't call `on_onset`) was barely affected.

Fix: route the test harness through the production `PulseGenerator::new(1)` (PPQN=1 = beat boundaries only). It already implements the >50 %-period wrap test and the monotonic `last_pulse_index` guard, and is the same code that drives the production LED + MIDI output, so the test now measures what the user actually hears/sees.

Corrected Ballroom numbers (n=698, 2026-05-11): reactive F=0.288, consensus F=0.436, **ΔF=+0.148**, consensus better on 572/698 (82 %), reactive better on 114, tied on 12. The qualitative conclusion of this ADR is unchanged: consensus mode is a real improvement on full-mix audio and is the recommended setting for the Daslight DMX use case. Magnitude of the improvement is smaller than the buggy first measurement claimed, but still substantive and consistent across in-tree synthetic A/B (ΔF=+0.55 on 30 % spurious-onset clicks) and real audio (+0.15 on 698 Ballroom tracks).

Both modes' absolute F-measure on Ballroom (~0.29 / ~0.44) is well below the literature's ~0.75–0.85 for aubio + PLL trackers. A brief audit (`tests/onset_method_audit.rs`, Jive subset n=60) found all 7 aubio onset methods cluster at F=0.36–0.45 — the gap is not a one-flip parameter fix and is tracked as a follow-up. It does not change the consensus-vs-reactive recommendation.
