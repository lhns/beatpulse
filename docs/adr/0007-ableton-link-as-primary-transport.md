# ADR-0007: Use Ableton Link as the primary cross-application transport

## Status

Accepted. Supersedes the implicit "OSC or rtpMIDI" assumption from earlier
in the design conversation.

## Context

After ruling out MIDI Clock (ADR-0001), tap simulation (ADR-0005), and OSC
for the Daslight use case (ADR-0006), the remaining candidates were:

- **rtpMIDI** — purpose-built for cross-machine MIDI, ships timestamps and
  retransmit. Would deliver MIDI Note/CC events to the DMX machine which
  Daslight maps as button shortcuts → still requires tap simulation.
- **Ableton Link** — designed exactly for cross-application tempo and phase
  sharing on a LAN. Daslight 5 is a first-class Link peer.
- **VBAN** — audio-streaming protocol with a MIDI subprotocol; no
  advantage over rtpMIDI here, narrower tooling ecosystem.

Link uniquely:

- Delivers tempo as semantic data, not as events to be averaged.
- Handles tempo changes in real time with sub-25 ms gossip latency.
- Is bidirectional — any peer can change tempo and others follow.
- Requires no port configuration or MIDI mapping on either end.
- Has an official, tested-and-validated reference implementation.

The Rust binding situation was checked: `rusty_link` wraps Ableton's
official `abl_link` C wrapper and is the right choice. A pure-Rust
alternative `ableton-link-rs` exists but self-describes as "early-stage"
and does not yet claim full interop with the official Link library.

## Decision

Make Ableton Link the primary output. Use `rusty_link` for the binding.
Use the realtime-safe `capture_audio_session_state` /
`commit_audio_session_state` API path from the audio thread.

MIDI output remains as a parallel output for in-DAW use (recording,
in-project automation, MIDI-only downstream consumers) and is independent
of Link.

## Consequences

- Daslight integration becomes trivial: enable Link on both sides, done.
- Both copyleft licenses (aubio LGPL-3, Link GPL-2-or-later) propagate;
  the project must be GPL-3 (see ADR-0009).
- `rusty_link` invokes CMake to build the bundled C++ Link sources at
  build time. README must document this dependency.
- Multiple plugin instances on the same network become Link peers fighting
  over session tempo. Must be documented; runtime detection deferred.
- Plugin gains a dependency on Ableton's network protocol design choices
  (multicast, no port config). Some restrictive networks block this.
