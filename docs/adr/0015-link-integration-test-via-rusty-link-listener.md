# ADR-0015: In-process Link listener test on top of manual LinkHut procedure

## Status

Accepted.

## Context

Manually verifying Link integration with Ableton's `LinkHut` example app
catches obvious problems but is slow, requires a developer to drive it,
and doesn't run in CI. The publisher in `src/link/publisher.rs` is the
component most likely to silently regress: a typo swapping
`capture_audio_session_state` for the `_app_` variant would still
"work" but block the audio thread; a misordered tempo update would
publish stale BPM; an unset `Link::enable(true)` would emit nothing.

We want an automated check that catches all three classes of regression
without needing a DAW or a network.

## Decision

Add `tests/bin/link_listener.rs`, gated behind the `link-integration`
feature. It:

1. Constructs its own `rusty_link::AblLink` instance, enables it, joins
   the local Link session.
2. Spawns BeatPulse's DSP pipeline in a worker thread, fed a synthesised
   click track at 100, 120, and 140 BPM in sequence.
3. After 10 s per tempo, captures `state.tempo()` and asserts it is
   within 1 BPM of the injected value.

Link discovers loopback peers without network access, so this runs in
any CI environment.

The manual LinkHut procedure remains in `docs/TESTING.md` for cross-
implementation verification.

## Consequences

- Catches publisher regressions in CI.
- Adds a feature flag and a test binary; small maintenance cost.
- Does not exercise the DAW host's MIDI-bus emission, parameter UI, or
  network multicast. Those are covered by other tests + manual
  procedures.
