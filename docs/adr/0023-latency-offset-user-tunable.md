# ADR-0023: Latency offset is user-tunable, not auto-detected

## Status

Accepted.

## Context

aubio's analysis delay (~`hop_size = 512` samples ≈ 11 ms at 44.1 kHz)
is already accounted for in `BeatTracker` via `Onset::get_last()`,
which subtracts the algorithm's internal delay from the reported onset
time. So the *detected onset position* is already correct relative to
the audio sample.

The piece that's *not* compensated is the **end-to-end audio path**:
host buffer → audio driver → speakers → listener ear. If the user's
setup has, say, 30 ms of output buffer latency, BeatPulse's Link
tempo and MIDI pulses fire "on time" relative to the audio sample,
but the listener perceives the audio 30 ms later — so DMX cues driven
off Link look slightly early relative to the perceived beat.

Auto-detecting this end-to-end latency is hard:

- VST3/CLAP transport metadata sometimes exposes the host's output
  buffer size, but coverage is patchy and varies per-host.
- The driver's buffer is rarely exposed.
- Speaker / room latency is invisible to software.

## Decision

Expose a single `latency_offset_ms: FloatParam` (range `-200.0 ..= 200.0` ms,
default `0.0`). Negative = pre-fire (compensate for audio output
latency); positive = post-fire (rare). User dials it in once for their
setup.

The offset is applied two ways:

1. **MIDI pulse emission** — each emitted `PulseEvent`'s `sample_offset`
   is shifted by `(offset_ms / 1000.0) * sample_rate`, clamped to
   `[0, n_samples - 1]` within the current block. For typical
   calibration values (±50 ms ≈ ±2200 samples) this fits comfortably
   in a 512-sample block; larger offsets get clamped to block edges.
2. **Link tempo timestamp** — the `clock_micros()` argument to
   `SessionState::set_tempo` is shifted by `offset_ms × 1000`. Link
   uses semantic timestamps not sample-accurate events, so the shift
   propagates cleanly to other peers.

Persisted with the rest of the parameter state per-instance via the
existing nih-plug parameter-state machinery.

## Consequences

- Calibration is a one-time activity per setup. If the host changes
  buffer size mid-session, user re-adjusts.
- We don't try to auto-detect (would require host probing and still
  wouldn't cover speaker/room latency). When/if `ProcessContext::transport()`
  reliably exposes buffer latency across hosts, an "auto" mode could
  be added on top — out of scope for v1.
- Within-block clamping for MIDI means very large offsets (> block
  size) will compress pulses to block edges. A future improvement
  would be a cross-block pending queue; deferred until needed.
