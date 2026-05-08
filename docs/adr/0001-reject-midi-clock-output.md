# ADR-0001: Don't ship MIDI Beat Clock from this plugin

## Status

Accepted, with caveats — see "Honesty note" below. The decision is "we
won't ship it from this plugin", not "MIDI Clock is bad."

## Context

The original ask was for a "BPM detector VST plugin which can output the
clock on MIDI." The straightforward reading is: a VST that detects tempo
from audio and emits MIDI Beat Clock messages (24 PPQN, 0xF8) so that
downstream gear syncs to it.

MIDI Beat Clock itself is fine — it's the standard hardware-sync transport
and works well between, e.g., a sequencer and a drum machine over a real
MIDI cable. The problem is *getting clock bytes out of a VST plugin into
something useful*.

The VST3 specification does not define MIDI Beat Clock as a host-routable
signal. Plugin MIDI output goes through `IEventList` (note on/off,
controllers, polypressure, etc.) — system real-time messages like 0xF8
do not have a defined slot there. CLAP's note-event model is similar.

**Community-knowledge claim** (not independently verified by us, recorded
honestly): major DAWs (Ableton Live, Logic Pro, Cubase, FL Studio) are
widely reported to ignore Beat Clock messages emitted from plugins, and
existing tools in this space (e.g. Teragon BeatCounter) document this as
the reason they cannot send clock. We have not run our own host
compatibility matrix to confirm this end-to-end.

The ways to actually deliver MIDI Clock from inside a plugin process are:

1. Open a system MIDI port from inside the plugin and write clock bytes
   directly, bypassing the host's MIDI bus. Works in principle, but
   violates the spirit of the VST contract; multiple plugin instances
   fight over the port; behaviour varies by host.
2. Run a standalone application alongside the DAW and route audio to it
   via a virtual audio cable. This is what Waveclock does. Works fine,
   but is not a plugin.

For our use case (drive Daslight 5), we don't need MIDI Clock at all —
Ableton Link is a better fit (see ADR-0007). So we don't need to fight
the format.

## Decision

This plugin does not emit MIDI Beat Clock. Beat events are emitted as
ordinary MIDI Note/CC messages (which hosts route normally) and tempo is
shared with downstream consumers via Ableton Link.

This is **not** a statement that MIDI Clock is the wrong choice in
general. If a future use case requires Clock, the most realistic paths
are:

- A separate standalone build of BeatPulse (nih-plug supports it) that
  opens a system MIDI port and writes clock bytes directly. Outside the
  VST contract, but a clean separation.
- A companion utility that consumes BeatPulse's Note/CC pulses (or its
  Link tempo) and produces Clock to a real or virtual MIDI port.

## Consequences

- Users who specifically need MIDI Clock for legacy gear must derive it
  externally.
- The plugin sidesteps a class of host-compatibility uncertainty.
- The architectural simplification enables Link as the primary
  transport, which is the right answer for the Daslight target anyway.

## Honesty note

The claim "major hosts ignore plugin-emitted Clock" is community
consensus and matches the documented behaviour of comparable tools, but
we have not run our own per-host test matrix to confirm it on current
DAW versions. If someone wants to revisit Clock output later, the right
first step is to actually test 0xF8 emission from a minimal nih-plug
test plugin in FL Studio / Live / Reaper / Cubase and record the
behaviour observed — not to take this ADR's claim on faith.
