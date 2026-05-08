# ADR-0005: Reject simulated tap-tempo as a control mechanism

## Status

Accepted.

## Context

Daslight 5 supports four tempo sources: Audio analysis, Tap, MIDI Clock,
and Ableton Link. One way to drive Daslight from BeatPulse would be to
map a Daslight shortcut to "BPM tap" and have the plugin send that
shortcut once per beat (via MIDI or OSC). Daslight averages 4–7 taps to
derive BPM.

This is a lossy round-trip: BeatPulse computes BPM, throws it away by
emitting taps, Daslight rebuilds it from those taps. The averaging
window adds 4+ beats of latency to any tempo change, on top of BeatPulse's
own PLL lock time.

## Decision

Do not target Daslight's tap input. Use Ableton Link instead (see ADR-0007).

## Consequences

- The output stage carries actual tempo data, not synthetic tap events.
- Tempo changes propagate to Daslight in roughly one Link gossip interval
  (~25 ms) plus Daslight's own smoothing, instead of 4+ beats.
- For other DMX software that does *not* support Link, tap simulation
  remains a viable fallback path. Not implemented in v1.
