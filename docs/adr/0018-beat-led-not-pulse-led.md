# ADR-0018: UI indicator LED reflects beats, not PPQN pulses

## Status

Accepted. Implemented in commit `e4f2f1a`.

## Context

The first version of the indicator LED bumped `SharedState::pulse_count`
on every PulseGenerator emission. At the default PPQN=4 and typical
tempos (120–140 BPM) that's 8–10 Hz — visually a rapid flicker.

User feedback: "should blink in the beat freq". They expect one flash per
beat (matching the BPM display), not one flash per PPQN-divided pulse.

Three options were considered:

1. Replace PULSE with BEAT (one flash per beat, regardless of PPQN).
2. Show two separate LEDs: BEAT and PULSE.
3. Lengthen the fade so adjacent flashes overlap into a continuous glow.

(2) is more informative but adds visual clutter. (3) hides the
behaviour rather than fixing the semantic mismatch. (1) matches the
mental model of the headline BPM display.

## Decision

The indicator is named **BEAT** and flashes once per detected beat
boundary. PulseGenerator now emits `PulseEvent { is_beat_boundary: bool }`
where `is_beat_boundary == (within_beat_index == 0)`. SharedState carries
two counters — `beat_count` (drives the LED) and `pulse_count` (kept for
potential future use; not consumed by the UI).

## Consequences

- LED cadence visually matches the BPM display (~2.2 Hz at 133 BPM).
- PPQN can still be set to 24 (for fine MIDI clock-equivalent output)
  without making the UI a strobe.
- One additional bool field on every `PulseEvent` and one additional
  atomic in `SharedState` — negligible cost.
- If users later say "I want to see pulses too," dropping a second LED
  alongside is cheap (the data is already stored).
