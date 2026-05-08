# ADR-0024: Tempo stability is its own parameter, decoupled from Sensitivity

## Status

Accepted.

## Context

The original Sensitivity parameter (spec §7) controlled three things at
once via three lerps:

```rust
let aubio_threshold = lerp(0.1, 1.0, 1.0 - sensitivity);
let alpha_period    = lerp(0.03, 0.15, sensitivity);
let alpha_phase     = lerp(0.08, 0.25, sensitivity);
```

User reported the BPM display jitters on noisy / multi-layer audio.
The natural fix is more period smoothing (lower `α_period`), but
turning Sensitivity *down* to lower α also raises the aubio threshold
(fewer detected onsets) — opposite of what's wanted on noisy input.
And turning Sensitivity *up* to detect more onsets also raises α
(more jitter). The two goals fight each other when they share a knob.

The third coupled parameter (`α_phase`) controls *lock acquisition*
speed — how fast the predicted beat aligns with detected onsets. It
isn't related to long-term tempo smoothness, so leaving it on the
Sensitivity knob is fine.

## Decision

Split out `α_period` as its own `tempo_stability: FloatParam`
(range `0.0 ..= 1.0`, default `0.5`). Mapping:

```rust
let alpha_period = lerp(0.20, 0.02, tempo_stability);
```

- `tempo_stability = 0.0` → α = 0.20 (jittery, fast tempo-change response)
- `tempo_stability = 0.5` → α = 0.11 (close to historical default 0.09)
- `tempo_stability = 1.0` → α = 0.02 (very smooth, slow response)

Sensitivity continues to control aubio threshold + `α_phase`.

## Consequences

- One additional UI control ("Stability" slider in the Detection
  section, below Sensitivity).
- Existing presets / state files get `tempo_stability = 0.5` by
  default, which approximately matches the previous `α_period = 0.09`
  (new midpoint α = 0.11 — slightly less smoothing).
- User can dial up smoothness for noisy input without sacrificing
  onset sensitivity, and vice versa.
- The contract is locked in by `tests/bpm_stability.rs`:
  - `high_alpha_jitters_more_than_low_alpha` asserts σ(BPM) at α=0.20
    is at least 2× σ(BPM) at α=0.02 with the same input jitter.
  - `smooth_setting_keeps_bpm_within_tight_band` asserts σ(BPM) < 0.6
    BPM at α=0.02 with 15 ms onset jitter (realistic full-mix noise).

## Related

The same commit also adds a `bpm_std_dev` atomic in `SharedState`
(EMA-tracked in `BeatPll::on_onset`) so the UI can render `±σ` next
to the headline BPM display when σ > 0.5 — making "is this stable?"
a glance-check.
