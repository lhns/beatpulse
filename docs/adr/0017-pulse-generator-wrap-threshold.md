# ADR-0017: PulseGenerator wrap detection requires a drop greater than half a period

## Status

Accepted. Fix landed in commit `6e13820`.

## Context

`PulseGenerator` advances the PLL phase one sample at a time and emits a
pulse when the absolute pulse index changes. To track the absolute index
across beat boundaries it maintains an internal `beat_counter` that
increments on each natural phase wrap (phase grows past `period_samples`
and rolls back to 0).

The first implementation detected wrap by checking
`pll.phase_samples < self.last_phase`. This is the obvious correct
condition for an isolated PLL that only advances. But `BeatPll::on_onset`
*also* legitimately decreases `phase_samples` — that's how phase-locking
works: an onset arriving slightly after `phase = 0` pulls phase backward
toward zero so the next onset lands on phase 0.

User report on the BEAT LED: at a stable 133 BPM the LED flickered
"like crazy" — every detected onset (5–15/sec on a dense pop mix) was
producing a phantom beat-counter bump and an extra pulse, because the
naive wrap check fired on every backward correction.

The maximum legitimate backward correction is `α_phase × period/2`, and
with the spec's `α_phase ≤ 0.25` that's ≤ `period/8`. A real wrap drops
phase by ≈ `period`. There's a wide gap.

## Decision

Wrap detection requires the phase to drop by more than half a period:

```rust
if pll.phase_samples + pll.period_samples * 0.5 < self.last_phase {
    self.beat_counter += 1;
}
```

`period/2` was chosen because it's safely above the maximum legitimate
correction (`period/8` ≈ `0.125 * period`) and well below a true wrap
(`≈ period`).

## Consequences

- BEAT and PULSE LED behaviour is now stable; LED flash rate matches the
  PLL period rather than the onset rate.
- Pulse cadence in MIDI / Link is what the user expects (`BPM/60 × PPQN`),
  not inflated by per-onset phantom emissions.
- Two regression tests lock this in:
  - `tests/pulse_generator.rs::g12_onset_corrections_no_phantom_pulses`
    (control vs perturbed run).
  - `tests/dataset_synthetic.rs::pulse_rate_matches_ppqn_post_lock`
    (full pipeline against a click track).
- If a future change pushes `α_phase > 0.5` for any reason, this
  threshold becomes unsound — the unit tests above would still catch it,
  but it's worth knowing.
