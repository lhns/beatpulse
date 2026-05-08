# ADR-0025: Pulse firing requires monotonic forward progress

## Status

Accepted.

## Context

`PulseGenerator::check` decides whether to emit a pulse by comparing
its computed `absolute_index` (= `beat_counter × PPQN +
within_beat_index`) against the previously-emitted index. The original
implementation used `!=` — fire on any change.

With a frozen PLL the index advances monotonically, so this works. But
in normal operation `BeatPll::on_onset` can pull `phase_samples`
*backward* across a within-beat-index boundary as part of phase-locking.
That's the documented mechanism (ADR-0017's wrap-detection threshold
already protects against this at the *beat* boundary, but within-beat
boundaries weren't covered).

Walk-through with PPQN=4, period=44100, α_phase=0.165:

1. Phase advances naturally to ~11030 (just past first within-beat
   boundary at 11025). `within_beat_index = 1`,
   `absolute_index = N*4+1`. Pulse fires (correctly, `is_beat_boundary
   = false`).
2. An onset arrives at this sample. `pll.on_onset` computes
   `err = 11030`, applies `phase -= α_phase × 11030 ≈ 1820`. Phase
   ends up around 9210.
3. Next `observe_advance`: `within_beat_index = floor(9210 / 11025)
   = 0`, so `absolute_index = N*4+0`, which is **one less** than
   `last_pulse_index = N*4+1`.
4. Old `!=` check fires. The new event has `is_beat_boundary = true`
   (because within = 0), so the BEAT counter bumps and the UI BEAT
   LED blinks again — within the same beat. User sees this as
   occasional double-blinks.

ADR-0017 is the analogous fix at the beat boundary (the full wrap
from `phase ≈ period` back to `phase ≈ 0`); it doesn't help here
because within-beat-index changes don't trigger a wrap.

## Decision

Pulses fire only when `absolute_index > last_pulse_index`:

```rust
if absolute_index > self.last_pulse_index {
    self.last_pulse_index = absolute_index;
    Some(PulseEvent { ... })
}
```

Backward jumps in `absolute_index` (caused by PLL phase corrections)
are silent. The next *forward* progression past the previous
high-water mark fires the next real pulse.

This doesn't miss legitimate pulses: in steady-state the PLL phase
always eventually advances past where it was before (it's pulled by
the PLL toward the true beat, not held back); when phase next crosses
the next pulse boundary, `absolute_index` exceeds `last_pulse_index`
and fires.

The `i64::MIN` reset semantics still work: any small `absolute_index
> MIN` fires the first-after-reset pulse.

## Consequences

- BEAT LED no longer double-blinks on onsets that land just past
  within-beat boundaries.
- MIDI pulse cadence becomes strictly monotonic in time (no
  out-of-order events).
- Locked in by `tests/pulse_generator.rs::g14_backward_phase_correction_no_extra_pulse`,
  which mirrors the user-reported scenario (advance to just past a
  within-beat boundary, inject onset, assert no spurious fire).
- Pairs with ADR-0017: together, *wrap requires drop > period/2* and
  *firing requires absolute_index > last* make `PulseGenerator`
  robust against both backward beat-wraps and backward within-beat
  jumps caused by PLL phase corrections.
