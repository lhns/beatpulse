# ADR-0010: Defer phase publishing on Link to a later version

## Status

Accepted, deferred.

## Context

Link sessions share both *tempo* and *phase* (beat position within a
quantum, typically 4 beats / one bar). Publishing phase would let
downstream peers align bar boundaries with our detection — useful for
DMX cues that should fire on bar 1 specifically.

Publishing phase requires *bar* (downbeat) detection, which is a harder
problem than beat detection. None of the candidate libraries do downbeat
detection in real time without significant additional latency. BeatNet
does it but is excluded by ADR-0003.

## Decision

v1 publishes tempo only. Phase is left at whatever the Link session has,
inherited from other peers or defaulted.

## Consequences

- Bar-aligned cues in Daslight cannot use BeatPulse's notion of "bar 1";
  Daslight's own bar counter rolls based on the published tempo.
- Adding phase publishing later is a localised change in `LinkPublisher`
  once a downbeat detector is in place. The architecture supports it.
