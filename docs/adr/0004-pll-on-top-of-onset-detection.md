# ADR-0004: Add a phase-locked loop on top of onset detection

## Status

Accepted.

## Context

Raw onset detection produces a sequence of timestamps. Naively, one could
either:

1. Emit a MIDI event at each detected onset, or
2. Compute BPM = 60 / mean(inter-onset interval) and emit at fixed rate.

Both are unacceptable for DMX use. (1) is jittery because onset detection
has frame-by-frame timing noise. (2) is laggy because mean-based BPM
estimation reacts slowly and doesn't account for phase.

What is actually needed is a *phase-locked loop*: maintain a continuously-
running estimate of period and phase, nudge both toward observations, and
*generate* output pulses from the prediction rather than reacting to
observations. In steady state this gives zero output latency and very low
jitter, regardless of the underlying detector's noise floor.

This is also exactly the technique Waveclock advertises ("tight phase
accuracy in relation to the input signal").

## Decision

Implement a `BeatPLL` component with separate smoothing constants for
period (`alpha_period`, ~0.05–0.15) and phase (`alpha_phase`, ~0.10–0.25).
Output pulses come from `PulseGenerator`, which advances phase per-sample
and fires events when phase crosses configured boundaries.

Include octave-correction logic in the PLL: if an observed inter-onset
interval is close to half or double the current period estimate, correct it
to avoid the well-known octave-error pathology of beat trackers.

## Consequences

- Steady-state output is sample-accurate and jitter-free regardless of
  onset detector noise.
- Initial lock takes 4–8 onsets (~2–4 seconds at typical tempos). The UI
  shows a LOCKED LED so the user knows when output is reliable.
- The PLL becomes the most testable and most-likely-to-be-subtly-wrong
  component. Dedicated unit tests are required.
