# ADR-0006: Reject OSC as the v1 cross-application transport

## Status

Accepted, deferring OSC to a future version.

## Context

OSC was initially the planned cross-computer transport. It is well-supported
by DMX software (Resolume, QLC+, MagicQ, TouchDesigner, GrandMA), simple to
implement (`rosc` crate in Rust), and low-latency on a LAN.

The complication is that the user's actual target is Daslight 5. Daslight
speaks OSC but only for *mapping shortcuts to buttons* — not for receiving
structured tempo data. Driving Daslight's BPM via OSC would still require
tap simulation (see ADR-0005), which has its own problems.

Since Daslight supports Link natively as a tempo source, OSC's advantages
don't apply to this user's use case.

## Decision

Drop OSC from v1. Note in the spec that OSC can be added later as another
parallel output stage alongside Link and MIDI, with its own enable
parameter.

## Consequences

- Smaller v1 scope.
- Users with non-Daslight, non-Link targets (e.g. Resolume Arena pure-OSC
  setups) are not served by v1. They can either use the MIDI output path
  with a MIDI-to-OSC bridge, or wait for v2.
- The architecture leaves a clean slot for OSC re-introduction.
