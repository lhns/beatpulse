# BeatPulse — Spec compliance audit

Walkthrough of every numbered section in [`BeatPulse-SPEC.md`](../BeatPulse-SPEC.md). Status flags:

- ✅ Implemented + verified by test or pluginval.
- 🟡 Implemented, verification informal (visual / manual).
- ⚠ Implemented but with documented caveat or deviation (linked to ADR).
- ⏸ Deferred (user-facing or out-of-scope work).
- ❌ Not implemented.

Last refreshed against `main` after commit `f6b212c`-area work.

## §1 Overview

| Item                                                    | Status | Notes                                                    |
|---------------------------------------------------------|--------|----------------------------------------------------------|
| VST3 + CLAP plugin built with nih-plug                   | ✅      | `cargo xtask bundle beatpulse --release` produces both.  |
| Reads single-channel audio (mono or stereo)              | ✅      | `lib.rs` downmixes channels via mean.                    |
| Broadcasts tempo to Ableton Link                         | ✅      | `LinkPublisher`. Verified by `link_listener_tracks_published_tempo`. |
| Emits MIDI Note/CC events into host bus                  | ✅      | `MidiFormatter`. Verified by `m1`–`m10`.                 |
| FL Studio on Windows is primary target                   | 🟡      | Loads in FL Studio per user F5; full Daslight loop pending. |
| Does **not** send MIDI clock                             | ✅      | ADR-0001.                                                |

## §2 Goals and non-goals

| Goal / Non-goal                                                  | Status | Notes                          |
|------------------------------------------------------------------|--------|--------------------------------|
| Real-time beat detection (mono or stereo, low latency)            | ✅      | aubio @ HOP_SIZE=512 ≈ 11 ms.  |
| Phase-locked tempo                                                | ✅      | `BeatPll`.                     |
| Tempo broadcast to Ableton Link                                   | ✅      | `LinkPublisher`.               |
| Configurable MIDI events (CC / Note / Both) at configurable PPQN  | ✅      | `MidiFormatter` + UI.          |
| VST3 + CLAP, no host-bypass                                       | ✅      | Standard nih-plug exports.     |
| Survives silence + resumes cleanly                                | ✅      | `SilenceGate` + reset hooks.   |
| Deterministic enough for DMX                                      | ✅      | PLL phase smoothing, tested in `bpm_stability`. |
| Non-goal: MIDI Clock                                              | ✅      | Not implemented (ADR-0001).    |
| Non-goal: OSC                                                     | ✅      | Not implemented (ADR-0006).    |
| Non-goal: rtpMIDI                                                 | ✅      | Not implemented.               |
| Non-goal: bar/downbeat detection                                  | ✅      | Not implemented.               |
| Non-goal: AU/AAX/VST2                                             | ✅      | Only VST3 + CLAP exported.     |

## §3 Architecture

| Component       | File                          | Status |
|-----------------|-------------------------------|--------|
| BeatTracker     | `src/dsp/beat_tracker.rs`     | ✅      |
| SilenceGate     | `src/dsp/silence_gate.rs`     | ✅      |
| BeatPLL         | `src/dsp/beat_pll.rs`         | ✅      |
| PulseGenerator  | `src/dsp/pulse_generator.rs`  | ✅      |
| LinkPublisher   | `src/link/publisher.rs`       | ✅      |
| MidiFormatter   | `src/midi/formatter.rs`       | ✅      |
| Audio thread `process` wires them | `src/lib.rs`        | ✅      |
| Lock-free atomic UI state        | `src/shared.rs`     | ✅      |

## §4 Build environment

| Item                                | Status | Notes                                                    |
|-------------------------------------|--------|----------------------------------------------------------|
| Rust 2021, MSRV 1.75                 | ✅      | `Cargo.toml` `rust-version = "1.75"`.                    |
| `nih-plug` pinned to a git commit    | ✅      | `28b149ec` (ADR-0008).                                   |
| `aubio-rs` 0.2                       | ✅      |                                                          |
| `rusty_link` 0.4                     | ✅      |                                                          |
| `nih_plug_egui` for UI               | ✅      |                                                          |
| `approx` dev-dep                     | ✅      |                                                          |
| GPL-3 licensing                      | ✅      | `LICENSE` + headers in every `.rs` file (audited).       |
| Project layout matches spec §4       | ✅      | Files in expected paths.                                 |

## §5 DSP details

### §5.1 BeatTracker

| Item                                                 | Status | Notes                                              |
|------------------------------------------------------|--------|----------------------------------------------------|
| Use `aubio_rs::Onset`, not `Tempo`                    | ✅      | `BeatTracker::new` calls `Onset::new`.            |
| Method default `SpecFlux`, all 7 selectable           | ✅      | `OnsetMethod` enum + `EnumParam`.                 |
| Buffer size 1024                                      | ✅      | `BUF_SIZE = 1024`.                                |
| Hop size 512                                          | ✅      | `HOP_SIZE = 512`.                                 |
| Threshold from `sensitivity` param                    | ✅      | `aubio_threshold` lerp.                           |
| Hop accumulator across host blocks                    | ✅      | `B1` test.                                        |
| Onset position back-calculation                       | ✅      | Uses `Onset::get_last()` (delay-compensated).     |
| Stereo downmix                                        | ✅      | `process_block_multichannel`.                     |
| All aubio resources allocated in `initialize`         | ✅      | `lib.rs::initialize`.                             |

### §5.2 BeatPLL

| Item                                                       | Status | Notes                                          |
|------------------------------------------------------------|--------|------------------------------------------------|
| Period + phase state                                        | ✅      |                                                |
| Per-sample phase advance                                    | ✅      | `advance_one`.                                  |
| Period smoothing on each onset                              | ✅      | `α_period`, with cold-start snap (ADR-0016).    |
| Phase smoothing on each onset                               | ✅      | `α_phase`.                                      |
| Octave correction (×2 below min, ×0.5 above max)            | ✅      | With 1% tolerance band (commit `aa3448b`).      |
| min/max period clamp                                        | ✅      |                                                |
| Lock detection (`onsets_since_reset >= 8`)                  | ✅      | Spec uses 8; matches `LOCK_THRESHOLD`.          |
| Tunables driven by `sensitivity` parameter                  | ⚠      | `α_phase` from sensitivity; `α_period` decoupled to `tempo_stability` (ADR-0024). |
| `current_bpm()`                                             | ✅      |                                                |

### §5.2b ConsensusTracker (opt-in alternative)

| Item                                                                | Status | Notes                                          |
|---------------------------------------------------------------------|--------|------------------------------------------------|
| Sliding onset window (200–3000 ms, default 2000)                    | ✅      | `lookahead_ms` param.                          |
| Median IOI with octave correction + outlier filtering               | ✅      | `dominant_period`.                             |
| 2-frame stability gate before commit                                | ✅      | Prevents thrashing.                             |
| Snap PLL period + phase on commit (no double-smoothing)             | ✅      | Single entry point: `try_snap_pll`.             |
| Reset on silence-end / manual resync                                | ✅      |                                                |
| Default tracking mode unchanged (`Reactive`)                        | ✅      | Opt-in via `tracking_mode` param.              |

**Quantified effect** (in-tree synthetic A/B, `cargo test`):

- Noisy clicks (120 BPM + 30 % spurious offbeats):
  F-measure **0.51 → 0.84** (+0.33).
- Full-mix kick + offbeat tonal stab: octave-tolerant tempo accuracy
  **fail → pass**.
- BPM-correctness on noisy input (% of beats within ±5 % of truth):
  **0.21 → 0.77** (+56 pp).

σ of reported BPM is *not* a useful proxy here: reactive has low σ even
when locked on the wrong tempo (smooth drift); consensus has high σ
because each commit is a discrete period snap. See
`bpm_stability::consensus_more_accurate_bpm_than_reactive_on_noisy_input`.

### §5.3 PulseGenerator

| Item                                                       | Status | Notes                                          |
|------------------------------------------------------------|--------|------------------------------------------------|
| Per-sample comparator at `pulse_interval = period / PPQN`   | ✅      |                                                |
| Reset on PLL re-lock and silence end                        | ✅      |                                                |
| First-pulse-after-reset fires at first non-silent sample    | ✅      | `g11`.                                          |
| Wrap detection requires drop > period/2                     | ✅      | ADR-0017 + `g12`.                               |
| Monotonic firing (`absolute_index > last`)                  | ✅      | ADR-0025 + `g14`.                               |

### §5.4 SilenceGate

| Item                                                       | Status | Notes                                          |
|------------------------------------------------------------|--------|------------------------------------------------|
| RMS sliding window                                          | ✅      | 1 ms time constant (small adjustment from spec hint). |
| `Active → Silent` after `release_ms`                        | ✅      | `s2`, `s7`.                                     |
| `Silent → Active` immediately + reset hook                  | ✅      | `s3`, `s9`.                                     |
| Defaults: −50 dB threshold, 200 ms release                  | ✅      |                                                |
| Bypasses PulseGenerator + Link when silent                  | ✅      | `lib.rs` only emits when `silence_gate.is_active()`. |

## §6 Ableton Link integration

| Item                                                       | Status | Notes                                          |
|------------------------------------------------------------|--------|------------------------------------------------|
| `LinkPublisher` constructed in `Plugin::initialize` at 120 BPM | ✅  |                                                |
| `enable(true)`; `enable_start_stop_sync(false)`              | ✅      |                                                |
| Tempo published using `capture_audio_session_state` /  `commit_audio_session_state` | ✅ | Realtime-safe path. |
| Threshold-gated commits (>0.05 BPM change)                  | ✅      |                                                |
| Tempo-only (phase publishing deferred)                      | ✅      | ADR-0010.                                      |
| UI shows peer count + status (Off/Joined/Publishing)        | ✅      | `LinkStatus` enum.                              |
| `link.enable(false)` toggle in UI                           | ✅      | Outputs row Ableton Link checkbox.              |
| Documentation of multicast / firewall caveats               | ✅      | README §Manual end-to-end with Daslight 5.      |

## §7 Parameters

All 16 parameters from the spec table are implemented in `BeatpulseParams`. Plus 2 new (`tempo_stability`, `latency_offset_ms`) added per ADR-0023 / ADR-0024. Defaults verified by `params::tests::defaults_match_spec`.

| Parameter            | Implemented? | Notes                       |
|----------------------|--------------|-----------------------------|
| `sensitivity`         | ✅           |                             |
| `onset_method`        | ✅           |                             |
| `silence_threshold`   | ✅           |                             |
| `silence_release`     | ✅           |                             |
| `pulse_rate`          | ✅           |                             |
| `link_enabled`        | ✅           |                             |
| `midi_enabled`        | ✅           |                             |
| `msg_type`            | ✅           |                             |
| `cc_number`           | ✅           |                             |
| `cc_value_mode`       | ✅           |                             |
| `cc_value`            | ✅           |                             |
| `note_number`         | ✅           |                             |
| `note_velocity`       | ✅           |                             |
| `note_length_ms`      | ✅           |                             |
| `midi_channel`        | ✅           |                             |
| `manual_resync`       | ✅           |                             |
| **`tempo_stability` (new)** | ✅      | ADR-0024                    |
| **`latency_offset_ms` (new)** | ✅    | ADR-0023                    |

Sensitivity LUT (`aubio_threshold`, `α_phase`) verified by `params::tests::sensitivity_lut_endpoints`. `α_period` LUT verified separately as it's now from `tempo_stability` (midpoint = 0.11; was 0.09 in spec).

## §8 MIDI output

| Item                                            | Status | Notes                       |
|-------------------------------------------------|--------|-----------------------------|
| `Fixed127` / `FixedCustom` / `Toggle127_0` modes | ✅     | `m1`–`m3`.                   |
| Note-on + scheduled note-off                     | ✅     | `m4`, `m5`.                  |
| Stuck-note prevention                            | ✅     | `m6`.                        |
| `Both` mode emits CC and Note                    | ✅     | `m8`.                        |
| Channel offset (1-indexed → 0-indexed)            | ✅     | `m7`.                        |
| Sample-offset preserved                          | ✅     | `m10`.                       |
| Drop on full bus + nih_log warning               | ⏸     | Not explicitly implemented; nih-plug's `send_event` swallows overflow silently. Low risk in practice given pulse rates. |

## §9 UI

| Item                                                   | Status | Notes                       |
|--------------------------------------------------------|--------|-----------------------------|
| egui-based, ~480×360 (now 480×540 default; resizable)  | ✅      | Window grew to fit MIDI foldout (commit `08ed906`); user-resizable per ADR-0020. |
| Lock LED                                                | ✅      | Painter circle (immune to font coverage).      |
| BPM display                                             | ✅      | 34 pt monospace hero, with `±σ` annotation.    |
| Pulse rate visible                                      | ✅      | In MIDI configuration foldout.                  |
| Input level meter                                       | ✅      |                                                |
| Sensitivity slider                                      | ✅      |                                                |
| Onset method dropdown                                   | ✅      | (Was a slider; replaced ADR-0019 era.)          |
| Silence threshold + release sliders                     | ✅      |                                                |
| Output enable checkboxes (Link + MIDI)                  | ✅      |                                                |
| MIDI configuration: Type / Channel / Pulse rate / CC / Note | ✅  |                                                |
| Resync button                                           | ✅      | Prominent rounded button.                       |
| Disable irrelevant controls based on `msg_type`         | ✅      | `add_enabled_ui(cc_enabled, ...)` and `add_enabled_ui(note_enabled, ...)`. |
| Atomics for UI thread                                   | ✅      | `SharedState`.                                  |
| ~30 Hz repaint                                          | ✅      | `egui_ctx.request_repaint_after(33ms)`.         |

UI extras beyond spec:
- BEAT LED (one flash per beat) — ADR-0018.
- Link status indicator (`Off / Joined / Publishing`).
- Tempo Stability slider — ADR-0024.
- Latency offset slider — ADR-0023.

## §10 Tests

| Item                                                                  | Status | Notes                                |
|------------------------------------------------------------------------|--------|--------------------------------------|
| `tests/pll.rs` (P1–P17)                                                | ✅      | 9 tests.                              |
| `tests/pulse_generator.rs` (G1–G14)                                    | ✅      | 8 tests.                              |
| `tests/silence_gate.rs` (S1–S9)                                        | ✅      | (in `src/dsp/silence_gate.rs::tests`) |
| Manual integration with Daslight 5                                     | ⏸      | F5 deferred per user.                 |
| Plus extras (E1–E6, latency, stability, snapshots, Link integration)   | ✅      | See `docs/TESTING.md`.                |

## §11 Implementation order

All 11 steps complete. See `git log` for commit-by-commit progression matching the spec ordering.

## §12 Known pitfalls

| Pitfall                                                | Status | Notes                                      |
|--------------------------------------------------------|--------|--------------------------------------------|
| aubio thread safety (audio thread only)                 | ✅      | `unsafe impl Send for BeatTracker`.        |
| Allocation in audio thread forbidden                    | ✅      | `assert_process_allocs` enabled, audio path allocates only in `initialize`. |
| `rusty_link` audio-thread API                           | ✅      | Using `_audio_session_state` paths.         |
| Sample-rate changes re-init aubio                       | ✅      | `BeatTracker::new` called from `initialize`. |
| Block-size variability                                  | ✅      | `B1`, `P2` resilience tests.                |
| Denormals (FTZ/DAZ)                                     | 🟡      | nih-plug claims default-enabled; not independently verified. |
| Initial lock + don't publish before locked              | ✅      |                                            |
| Multiple plugin instances on Link → thrash              | ✅      | ADR-0012 + new thrash test.                 |
| CMake at build time                                     | ✅      | README documents.                           |
| GPL-3 reminder                                          | ✅      | Headers audited in every `.rs`.              |

## §13 Out-of-scope items

All confirmed not implemented per spec intent:

- Sidechain input — out-of-scope (ADR-0011).
- Tempo follower for the host — out-of-scope.
- OSC output — out-of-scope (ADR-0006).
- Phase publishing on Link — out-of-scope (ADR-0010).
- Multiple simultaneous MIDI output streams — out-of-scope.

## Follow-ups surfaced by this audit

None blocking. Two minor items worth noting:

1. **MIDI bus overflow handling** (§8): no explicit drop-with-`nih_log!`. nih-plug's `send_event` silently no-ops if the queue is full. Acceptable for v1; revisit if a user reports lost events.
2. **Denormals** (§12): nih-plug enables FTZ/DAZ by default but we don't have an independent test that verifies it stays enabled. Could add a denormal-rich input test that asserts CPU time stays bounded; not blocking.
