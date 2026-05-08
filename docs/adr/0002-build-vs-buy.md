# ADR-0002: Build it ourselves rather than use existing tools

## Status

Accepted.

## Context

Existing audio-to-MIDI-Clock tools were surveyed:

- **Waveclock** (wavesum.net) — commercial, ~€60, closed source, abandoned-
  looking website, Klapuri-based algorithm. Does exactly what's needed.
- **Circular Logic inTime** — was $159, company defunct, abandonware demo
  may still run.
- **HoRNet SongKey MK4** — €30, standalone has MIDI Clock output, but BPM
  detection is unreliable per user reviews.
- **Teragon BeatCounter** — free, plugin-only, explicitly cannot send MIDI
  Clock.
- **Mixed In Key Live** — display-only, no MIDI output.

The free option (BeatCounter) does not meet the requirement. The cheap option
(SongKey MK4) is reportedly inaccurate. Waveclock is the only tool that does
the job well, and it costs money for a closed-source abandonware-risk tool.

The user explicitly asked for an open-source alternative. None exists at the
"finished application" level. The closest matches are research libraries
(aubio, madmom, BeatNet, librosa) and one promising-looking GitHub project
(SuperTimecodeConverter) which on closer inspection does not actually
include the audio-BPM-detection feature its README implied.

## Decision

Build a new plugin rather than buy Waveclock or use abandonware. Use existing
DSP libraries for the heavy lifting (onset detection) but write the
integration, phase-locked loop, and output stages from scratch.

## Consequences

- Significant implementation effort, but bounded — the plugin is small
  (~1500 LoC estimated).
- Full control over output format, license, and features.
- Reusable as a base for future related work (sidechain detection, OSC
  output, etc.).
