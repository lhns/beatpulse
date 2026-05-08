# ADR-0013: pluginval as the primary plugin validator

## Status

Accepted.

## Context

VST3 / CLAP plugins benefit from automated conformance testing. Three
candidate tools were considered:

- **pluginval** (Tracktion, GPL-3) — single binary, headless, strictness
  levels 1–10, exercises parameter automation, state save/restore,
  randomized fuzzing, multi-threaded calls, multiple state restorations.
  Easy to integrate into CI: one command, exit code drives the gate.
- **Steinberg `validator`** (ships with the VST3 SDK) — official spec
  conformance check. More authoritative on the VST3 spec letter but
  requires building the SDK from source, has fewer fuzz/randomized
  tests, and is more cumbersome to wire into CI.
- **clap-validator** (free-audio org) — same idea as Steinberg's
  validator but for CLAP. The de-facto standard for CLAP conformance.

For day-to-day CI we want a fast, headless, no-build-prerequisite tool.
For pre-release we want the strictest possible spec check.

## Decision

- **pluginval** is the primary validator for the VST3 build, run on every
  CI invocation at strictness level 5, bumped to 8 before tagged releases.
- **Steinberg's validator** is a manual pre-release gate, run once per
  tagged release on a developer machine that has the VST3 SDK built.
- **clap-validator** is the primary validator for the CLAP build, run on
  every CI invocation.

## Consequences

- CI gates remain fast and require only a downloaded binary, not an SDK
  build.
- Releases pick up any spec issues pluginval misses.
- Adds GPL-3 tooling to the build pipeline; no impact on plugin license
  since pluginval is invoked as an external binary, not linked.
