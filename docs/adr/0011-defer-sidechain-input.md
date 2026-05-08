# ADR-0011: Defer sidechain input to v2

## Status

Accepted, deferred.

## Context

A common workflow would be to run BeatPulse on the master mixer channel
and feed it a clean kick signal via a sidechain input, while the main
input is the full mix being played. nih-plug supports VST3 and CLAP
sidechain buses; FL Studio routes them.

For v1, the user can simply put BeatPulse on a dedicated kick mixer
channel that contains only the kick signal. This avoids the bus
configuration complexity for the initial version.

## Decision

v1 has a single mono/stereo audio input bus. Sidechain support is a v2
feature.

## Consequences

- Slightly less convenient routing in v1.
- Bus configuration code stays simple, reducing v1 scope.
