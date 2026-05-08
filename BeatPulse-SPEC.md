# BeatPulse — Real-time Beat Detector VST with Ableton Link Output

## 1. Overview

BeatPulse is a VST3 + CLAP audio effect plugin that detects beats in incoming
audio in real time and broadcasts the detected tempo to an Ableton Link
session. Its primary target consumer is **Daslight 5**, which supports
Ableton Link natively as a tempo source for BPM-driven scenes.

The plugin runs inside a DAW (target: FL Studio on Windows, but should work
in any VST3 or CLAP host) and reads audio from a single mixer channel —
typically a kick drum bus or a clean rhythmic source. It does **not** send
MIDI clock; major hosts ignore plugin-emitted clock messages, and Link is a
better protocol for the job anyway.

The plugin also emits the same beat events as MIDI notes/CCs into the host's
MIDI bus, for users who want to record or react to beat events inside the
DAW. MIDI output is independent of and parallel to the Link broadcast.

### Why Link, not OSC or MIDI Clock

Daslight 5 supports four tempo sources: Audio analysis, Tap, MIDI Clock, and
Ableton Link. Of these, Link is the only one that is bidirectional, jitter-
tolerant, and designed for cross-application tempo sharing on a LAN. OSC and
MIDI Clock would require either tap simulation (lossy round-trip) or
host-bypassing transport (architectural ugliness). Link sidesteps both.

The plugin joins the Link session on the local network and continuously
publishes the tempo derived by the beat tracker. Daslight, also a Link peer,
follows that tempo automatically. No mapping, no virtual MIDI ports, no
loopback required.

## 2. Goals and non-goals

### Goals

- Detect beats in real time from a mono or stereo audio input with low
  latency.
- Phase-lock a tempo estimate to the detected beats so the published tempo
  is smooth even when detection is noisy.
- Broadcast tempo to an Ableton Link session.
- Emit configurable MIDI events (CC, Note, or both) at a configurable
  pulse rate, in parallel to the Link broadcast.
- Run as a VST3 + CLAP plugin in FL Studio without bypassing the plugin
  contract.
- Survive silence and resume cleanly when audio returns.
- Be deterministic enough that downstream DMX cues do not visibly wobble.

### Non-goals

- MIDI beat clock output (host-incompatible; out of scope).
- OSC output (deferred; can be added later as another parallel output stage).
- rtpMIDI output (Link replaces the use case for this plugin).
- Polyphonic transcription, downbeat / bar detection, key detection.
- Offline analysis or batch processing.
- Plugin formats other than VST3 and CLAP (no AU, no AAX, no VST2).
  Standalone build is a useful side-effect of nih-plug but not a primary
  target.

## 3. Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│                      BeatPulse VST3 / CLAP                          │
│                                                                     │
│   audio in ──┬──────────────────────────────────────► audio out     │
│              │                                                      │
│              ▼                                                      │
│      ┌──────────────┐    onsets    ┌─────────┐  period,phase        │
│      │ BeatTracker  │─────────────►│ BeatPLL │────────┬──────────┐  │
│      │  (aubio-rs)  │              └─────────┘        │          │  │
│      └──────────────┘                                 │          │  │
│              ▲                                        ▼          ▼  │
│              │                              ┌──────────────┐  ┌────┐│
│      ┌──────────────┐                       │PulseGenerator│  │Link││
│      │ SilenceGate  │                       └──────────────┘  │Pub ││
│      └──────────────┘                              │          └────┘│
│                                                    ▼            │   │
│                                          ┌─────────────────┐    │   │
│                       parameters ───────►│  MidiFormatter  │    │   │
│                                          └─────────────────┘    ▼   │
│                                                    │      Link UDP  │
│                                                    ▼                │
│                                                midi out             │
└────────────────────────────────────────────────────────────────────┘
```

Five components inside the plugin:

1. **BeatTracker** — wraps `aubio-rs`'s onset detector. Consumes audio,
   emits onset events with sample-accurate timestamps.
2. **SilenceGate** — gates onsets and pulse output during silent input.
3. **BeatPLL** — phase-locked loop. Maintains `period` (samples per beat)
   and `phase` (sample position within current beat). Onsets nudge both
   estimates toward the observed value.
4. **PulseGenerator** — per-sample comparator. When `phase` crosses a
   configured fractional boundary, fires a pulse event for MIDI output.
5. **LinkPublisher** — owns a `rusty_link::AblLink` instance. On each
   `process` call, captures the current Link session state, sets the tempo
   from the PLL's BPM, and commits.

The audio thread runs the DSP and writes MIDI. A separate background
thread (started by `rusty_link` internally) handles Link's network I/O.
Audio-thread → UI communication uses lock-free atomics published by the
PLL.

## 4. Build environment

- **Language**: Rust, edition 2021, MSRV 1.75.
- **Plugin framework**: [`nih-plug`](https://github.com/robbert-vdh/nih-plug),
  pinned to a recent commit (no crates.io release as of writing). Produces
  VST3 and CLAP from a single codebase.
- **DSP**: [`aubio-rs`](https://crates.io/crates/aubio-rs), thin Rust binding
  over the aubio C library.
- **Ableton Link**: [`rusty_link`](https://crates.io/crates/rusty_link),
  Rust wrapper over Ableton's official `abl_link` C wrapper. Pulled in as
  a git or crates.io dependency; requires CMake ≥ 3.14 at build time to
  compile the bundled C++ Link sources.
- **UI**: `nih_plug_egui` (egui-based, ships with nih-plug). Sufficient for
  a 480×320 utility plugin.
- **Testing**: standard `cargo test`, plus
  [`approx`](https://crates.io/crates/approx) for float comparisons.

### Licensing

Both `aubio-rs` (LGPL-3 via aubio) and `rusty_link` (GPL-2 via Ableton Link)
impose copyleft obligations. The whole project is therefore **GPL-3** to
satisfy both. License headers in every source file. README states this
prominently.

If a permissive future is desired:
- Replace aubio with a hand-rolled spectral-flux onset detector
  (~150 lines of `rustfft` + ring buffer). Removes the LGPL constraint.
- Ableton Link is **always** GPL-2-or-later. The only way to ship Link
  under permissive terms is a commercial license from Ableton. For
  personal use this is fine.

### Project layout

```
beatpulse/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE                                # GPL-3
├── src/
│   ├── lib.rs                             # nih-plug Plugin impl
│   ├── params.rs                          # parameter definitions
│   ├── dsp/
│   │   ├── mod.rs
│   │   ├── beat_tracker.rs                # aubio wrapper
│   │   ├── beat_pll.rs                    # phase-locked loop
│   │   ├── pulse_generator.rs             # phase comparator
│   │   └── silence_gate.rs
│   ├── midi/
│   │   ├── mod.rs
│   │   └── formatter.rs
│   ├── link/
│   │   ├── mod.rs
│   │   └── publisher.rs                   # rusty_link wrapper
│   ├── ui/
│   │   ├── mod.rs
│   │   ├── editor.rs                      # egui top-level
│   │   ├── bpm_display.rs
│   │   ├── lock_led.rs
│   │   └── level_meter.rs
│   └── shared.rs                          # atomics shared with UI
├── tests/
│   ├── pll.rs
│   ├── pulse_generator.rs
│   └── silence_gate.rs
└── xtask/                                 # bundling helpers (nih-plug convention)
    ├── Cargo.toml
    └── src/main.rs
```

### Build commands

```bash
git clone <repo>
cd beatpulse
cargo +stable build --release
cargo xtask bundle beatpulse --release     # produces VST3 + CLAP bundles
```

Bundles land in `target/bundled/`. Copy or symlink to:
- Windows VST3: `C:\Program Files\Common Files\VST3\`
- Windows CLAP: `C:\Program Files\Common Files\CLAP\`
- Linux VST3: `~/.vst3/`
- Linux CLAP: `~/.clap/`

A `cargo xtask install` target should automate this.

## 5. DSP details

### 5.1 BeatTracker

Use `aubio_rs::Onset`, not `aubio_rs::Tempo`. We want raw onsets so the PLL
has unfiltered observations.

- Method: `OnsetMode::SpecFlux` by default. Expose as an enum parameter:
  `Hfc`, `Complex`, `SpecDiff`, `Kl`, `Mkl`, `Phase`, `SpecFlux`.
- Buffer size: 1024.
- Hop size: 512. At 44.1 kHz this is ~11.6 ms granularity.
- Threshold: exposed via the `sensitivity` parameter, mapped 0..1 →
  0.1..1.0 internally (lower threshold = more sensitive).
- aubio's internal silence is set to -90 dB; we do our own silence gating
  in `SilenceGate` to control release behaviour.

aubio operates on hop-sized buffers, not the host's block size. The
`BeatTracker` accumulates host samples into an internal hop-sized ring
buffer and calls `Onset::do_result` whenever a hop is full. When aubio
reports an onset, we compute the sample position within the *current host
block* by back-calculating from samples consumed and pass that offset
upstream.

The audio is downmixed to mono before being fed to aubio (mean of all
channels).

**Allocation rules**: All aubio resources are allocated in
`Plugin::initialize`. `Plugin::process` only reads/writes existing buffers.

### 5.2 BeatPLL

State:

```rust
pub struct BeatPll {
    pub period_samples: f64,    // current estimate of samples per beat
    pub phase_samples:  f64,    // 0 ≤ phase < period_samples
    pub sample_rate:    f64,
    pub locked:         bool,
    pub onsets_since_reset: u32,
    pub last_onset_sample:  Option<f64>,

    // Tunables, driven by sensitivity parameter
    pub alpha_period: f64,      // ~0.05 - 0.15
    pub alpha_phase:  f64,      // ~0.10 - 0.25

    // Hard limits, computed from sample_rate
    pub min_period: f64,        // = sample_rate * 60.0 / 220.0
    pub max_period: f64,        // = sample_rate * 60.0 / 60.0
}
```

Per-sample update (called inside `PulseGenerator`'s loop):

```rust
self.phase_samples += 1.0;
if self.phase_samples >= self.period_samples {
    self.phase_samples -= self.period_samples;
}
```

On each onset observation at absolute sample position `obs_sample`:

```rust
if let Some(last) = self.last_onset_sample {
    let mut observed_period = obs_sample - last;

    // Octave correction
    if observed_period < self.min_period {
        observed_period *= 2.0;
    } else if observed_period > self.max_period {
        observed_period *= 0.5;
    }

    if observed_period >= self.min_period
        && observed_period <= self.max_period
    {
        // Period smoothing
        self.period_samples = (1.0 - self.alpha_period) * self.period_samples
                            + self.alpha_period * observed_period;

        // Phase lock: an onset should land at phase = 0.
        let mut err = self.phase_samples;
        if err > self.period_samples * 0.5 {
            err -= self.period_samples;
        }
        self.phase_samples -= self.alpha_phase * err;
        if self.phase_samples < 0.0 {
            self.phase_samples += self.period_samples;
        }
    }
}

self.last_onset_sample = Some(obs_sample);
self.onsets_since_reset += 1;
self.locked = self.onsets_since_reset >= 8;
```

`current_bpm()` convenience: `60.0 * sample_rate / period_samples`.

### 5.3 PulseGenerator

Driven by `period_samples` and `phase_samples` and a configured `pulse_rate`
in pulses per quarter note (PPQN). Pulse interval is
`period_samples / pulse_rate as f64`.

In each `process` call, for each sample we advance phase and check whether
the running pulse counter has crossed an integer boundary:

```rust
let pulse_interval = self.period_samples / self.pulse_rate as f64;
let pulse_phase = self.phase_samples / pulse_interval;
let current_pulse_index = pulse_phase.floor() as i64;

if current_pulse_index != self.last_pulse_index {
    pulses.push(PulseEvent { sample_offset });
    self.last_pulse_index = current_pulse_index;
}
```

`last_pulse_index` is reset to `i64::MIN` whenever the PLL re-locks or
silence ends, so the first pulse after restart fires at the first
non-silent sample.

### 5.4 SilenceGate

Tracks input RMS over a sliding window. State machine:

- `Active` → `Silent` after `release_ms` of audio below `threshold_db`.
- `Silent` → `Active` immediately on first sample above threshold; PLL
  is reset (`locked = false`, `onsets_since_reset = 0`,
  `last_onset_sample = None`) and `PulseGenerator::last_pulse_index` is
  reset on this transition.

When `Silent`, `PulseGenerator` is bypassed (no MIDI events emitted) and
`LinkPublisher` stops updating tempo. The BeatTracker still runs so the
PLL can lock as soon as audio returns.

Defaults: threshold -50 dB, release 200 ms.

## 6. Ableton Link integration

### 6.1 Lifecycle

`LinkPublisher` is constructed in `Plugin::initialize` with an initial
tempo of 120 BPM:

```rust
let link = AblLink::new(120.0);
link.enable(true);
link.enable_start_stop_sync(false);   // we don't want transport events
```

It is dropped in `Plugin::deactivate`. Link's network thread is owned by
the C++ object and shuts down cleanly on drop.

### 6.2 Tempo publishing

In each `process` call, after the PLL has been advanced for the block, the
publisher reads the PLL's current BPM and updates the Link session state
**only if** the value has changed by more than a small threshold (e.g.
0.05 BPM). This avoids hammering Link with redundant commits.

```rust
let current_bpm = pll.current_bpm();
if (current_bpm - self.last_published_bpm).abs() > 0.05
    && silence_gate.is_active()
    && pll.locked
{
    let mut state = self.link.capture_audio_session_state();
    let micros = self.link.clock_micros();
    state.set_tempo(current_bpm, micros);
    self.link.commit_audio_session_state(state);
    self.last_published_bpm = current_bpm;
}
```

Note: use `capture_audio_session_state` / `commit_audio_session_state`
(not the `app` variants) because we are calling from the audio thread.
These are the realtime-safe paths in `abl_link`.

### 6.3 Phase publishing (deferred)

Link sessions also share *phase* — beat position within a quantum
(typically 4 beats / one bar). v1 ships **tempo only** because phase
publishing requires bar detection (not just beat detection), which is out
of scope. If downbeat detection is added in a future version, re-enable
phase publishing here.

### 6.4 Link UI elements

The plugin's UI shows:

- Number of currently connected Link peers (read via `link.num_peers()`).
- A "Link enabled" toggle (writes `link.enable(bool)`).
- The currently published tempo (mirror of PLL BPM).

### 6.5 Network requirements

- Link uses UDP multicast on the local network.
- Both machines (FL Studio PC, Daslight PC) must be on the same subnet.
- Link does not require any port configuration from the user.
- Some VPN clients and "private network" firewall profiles block multicast;
  if the user reports zero peers, point them at the Windows Defender
  firewall rules first.

## 7. Parameters

All parameters use `nih_plug::params::*`. Stored on the `BeatpulseParams`
struct.

| ID                  | Type       | Range / values                                  | Default | Notes                                  |
|---------------------|------------|-------------------------------------------------|---------|----------------------------------------|
| `sensitivity`       | FloatParam | 0.0 – 1.0                                       | 0.5     | Drives aubio threshold + PLL alphas    |
| `onset_method`      | EnumParam  | Hfc, Complex, SpecDiff, Kl, Mkl, Phase, SpecFlux| SpecFlux| aubio onset method                     |
| `silence_threshold` | FloatParam | -90 .. 0 dB                                     | -50     |                                        |
| `silence_release`   | FloatParam | 50 .. 2000 ms                                   | 200     |                                        |
| `pulse_rate`        | EnumParam  | Ppqn1, Ppqn2, Ppqn4, Ppqn8, Ppqn16, Ppqn24      | Ppqn4   | Pulses per detected quarter note       |
| `link_enabled`      | BoolParam  | true/false                                      | true    | Master enable for Link broadcast       |
| `midi_enabled`      | BoolParam  | true/false                                      | true    | Master enable for MIDI output          |
| `msg_type`          | EnumParam  | Cc, Note, Both                                  | Cc      |                                        |
| `cc_number`         | IntParam   | 0 – 127                                         | 16      |                                        |
| `cc_value_mode`     | EnumParam  | Fixed127, FixedCustom, Toggle127_0              | Fixed127|                                        |
| `cc_value`          | IntParam   | 0 – 127                                         | 127     | Used when mode = FixedCustom           |
| `note_number`       | IntParam   | 0 – 127                                         | 60      |                                        |
| `note_velocity`     | IntParam   | 1 – 127                                         | 100     |                                        |
| `note_length_ms`    | FloatParam | 1 – 100 ms                                      | 10      | Time between note-on and note-off      |
| `midi_channel`      | IntParam   | 1 – 16                                          | 1       |                                        |
| `manual_resync`     | trigger    | —                                               | —       | Resets PLL on next onset               |

Sensitivity mapping (one knob, three things behind it):

```rust
let aubio_threshold = lerp(0.1, 1.0, 1.0 - sensitivity);
let alpha_period    = lerp(0.03, 0.15, sensitivity);
let alpha_phase     = lerp(0.08, 0.25, sensitivity);
```

Link is the *primary* output. MIDI is a parallel output. Either can be
disabled independently via the `*_enabled` BoolParams.

## 8. MIDI output

`MidiFormatter` converts pulse events from `PulseGenerator` into MIDI
messages and emits them via `nih_plug::midi::NoteEvent` into the buffer's
event queue with the correct sample offset.

### CC mode

- `Fixed127`: every pulse emits one `NoteEvent::MidiCC` with value 127.
- `FixedCustom`: every pulse emits one `NoteEvent::MidiCC` with value =
  `cc_value`.
- `Toggle127_0`: alternates 127 / 0 / 127 / 0 …

### Note mode

Note-on at pulse time, note-off scheduled `note_length_ms` later. The
note-off is queued in a small ring buffer keyed by absolute sample
position; each `process` call flushes any note-offs whose scheduled time
falls within the current block (with correct sub-block offset).

### Both mode

CC and Note emitted simultaneously at every pulse.

### Channel

All output messages use `midi_channel - 1` (since MIDI channels are
0-indexed in the nih-plug API).

### Edge cases

- If a Note pulse fires before the previous Note's note-off has been sent
  (e.g. very short period or long `note_length_ms`), the old note-off is
  flushed at the new pulse's sample offset to avoid stuck notes.
- If the host's MIDI bus is at capacity, drop the event silently and log
  via `nih_log!` in debug builds.

## 9. UI

`nih_plug_egui`-based, fixed size 480×320 px.

Layout (roughly):

```
┌─────────────────────────────────────────────────────────────┐
│  BeatPulse                                                  │
│                                                             │
│  ┌─────────────┐   ┌──────────────────────────────────────┐ │
│  │             │   │  BPM         124.3                   │ │
│  │  ●  LOCKED  │   │  Link peers  2                       │ │
│  │             │   │  Pulse rate  4 PPQN                  │ │
│  └─────────────┘   └──────────────────────────────────────┘ │
│                                                             │
│  Input level   ▓▓▓▓▓▓▓▓▓░░░░░░░░░                           │
│                                                             │
│  Sensitivity   [───────●─────────]   Slow ◄──► Fast         │
│  Onset method  [ Spectral Flux ▼ ]                          │
│                                                             │
│  Silence       Threshold [-50 dB]   Release [200 ms]        │
│                                                             │
│  Outputs       [✓] Ableton Link    [✓] MIDI                 │
│                                                             │
│  MIDI          Type  [ CC ▼ ]   Channel [ 1 ▼ ]             │
│                CC#   [ 16 ]     Value mode [ Fixed 127 ▼ ]  │
│                Note  [ 60 ]     Velocity [100]  Length [10ms]│
│                Pulse rate [ 4 PPQN ▼ ]                      │
│                                                             │
│  [ Resync ]                                                 │
└─────────────────────────────────────────────────────────────┘
```

The UI thread polls atomics from `shared.rs` at ~30 Hz:

```rust
pub struct SharedState {
    pub current_bpm:    AtomicF64,
    pub locked:         AtomicBool,
    pub input_peak_db:  AtomicF64,
    pub link_peers:     AtomicUsize,
    pub silence_active: AtomicBool,
}
```

Atomic floats via `atomic_float::AtomicF64` (small dep, MIT-licensed).

Disable irrelevant controls based on `msg_type`: CC controls disabled when
type=Note, Note controls disabled when type=CC.

## 10. Tests

Standard `cargo test`. The DSP modules are pure Rust without nih-plug
dependencies in their type signatures, so they're testable directly.

### `tests/pll.rs`

- **Convergence on synthetic onsets**: feed onsets at exactly 120 BPM and
  check `period_samples` converges to within 0.1% of the expected value
  in fewer than 20 onsets.
- **Phase lock**: feed onsets at 120 BPM with the first observation
  offset by half a beat. After 10 onsets, `phase_samples` at onset times
  should be within `0.02 * period_samples` of 0.
- **Octave correction**: feed onsets at 240 BPM with the PLL initialised
  to 120 BPM. Verify it tracks 240 correctly without flip-flopping.
- **Tempo change**: feed 120 BPM for 30 onsets, then 140 BPM for 30
  onsets. Verify catchup to within 1% by onset 50.

### `tests/pulse_generator.rs`

- **Pulse spacing**: with a fixed `period_samples` and `phase_samples = 0`,
  pulses at PPQN=4 should land at sample positions
  `period/4, 2·period/4, 3·period/4, period`, ±1 sample.
- **Block boundary**: pulses falling across a block boundary land in the
  correct block with the correct sub-block offset.
- **PPQN switch**: changing PPQN mid-stream produces no duplicate pulses
  and no missed pulses at the transition.

### `tests/silence_gate.rs`

- Audio above threshold → state is `Active`.
- Drop to silence → state stays `Active` for `release_ms`, then
  transitions to `Silent`.
- Audio returns → state immediately `Active`, reset hook fires.

### Manual integration test

Not automatable, but document in `README.md`:

1. Load BeatPulse on an FL Studio mixer channel with a 120 BPM kick loop.
2. Open Daslight 5 on the same machine. Settings → Audio → enable
   Ableton Link.
3. Verify Daslight's BPM display follows BeatPulse's BPM display within
   one second of pressing play.
4. Stop the kick loop. Daslight's tempo should hold at the last
   published value (Link does not "release" tempo).
5. Restart with a 140 BPM loop. Daslight should converge to ~140 within
   ~10 seconds (PLL lock + Daslight smoothing).

## 11. Implementation order

1. `cargo new --lib`, set up `Cargo.toml` with nih-plug, build a no-op
   passthrough plugin that loads in FL Studio.
2. `params.rs` with all parameters; verify they save/load via the host.
3. `silence_gate.rs` with tests.
4. `beat_pll.rs` with tests, driven by synthetic onset inputs.
5. `pulse_generator.rs` with tests.
6. `beat_tracker.rs` (aubio integration). Verify by logging onset times
   to stderr with a known-good audio file.
7. Wire the four DSP components together in `lib.rs::process`.
8. `midi/formatter.rs` and MIDI output. Test by routing MIDI from
   BeatPulse to a MIDI monitor in FL Studio.
9. `link/publisher.rs`. Test by running Ableton's `LinkHut` example app
   on the same machine and watching peer count + tempo.
10. UI polish in `ui/` (lock LED, BPM display, level meter, peer count).
11. End-to-end test with Daslight 5: 120 BPM kick → BeatPulse → Link →
    Daslight scene synced.

Each step should compile and run before moving to the next.

## 12. Known pitfalls

- **aubio thread safety**: aubio C objects are not thread-safe. Only the
  audio thread touches them. UI thread reads display values via atomics.
- **Allocation in audio thread**: forbidden. All aubio resources are
  allocated in `Plugin::initialize`. `process` only reads/writes existing
  buffers.
- **rusty_link audio-thread API**: use `capture_audio_session_state` and
  `commit_audio_session_state` from the audio thread — these are the
  realtime-safe paths. The `_app_` variants take a mutex and can block.
- **Sample-rate changes**: `Plugin::initialize` is called again. Tear down
  and rebuild aubio objects, recompute `min_period` / `max_period`.
- **Block size changes**: aubio's hop size is fixed regardless. The
  internal accumulator handles arbitrary host block sizes.
- **Denormals**: nih-plug enables FTZ/DAZ on the audio thread by default.
  Verify this is still true; if not, set them manually.
- **Initial lock**: do not publish tempo to Link or emit MIDI pulses
  before `BeatPll::locked == true`. Show the LOCKED LED off until then.
  ~4–8 beats of silence on start; this is correct behaviour. The Link
  session will hold whatever tempo the previous peer last set.
- **Multiple plugin instances on Link**: each instance opens a Link
  session and becomes a peer. Two instances with different detected
  tempos will *fight* over the session tempo, with last-write-wins
  semantics. Document this — only run one BeatPulse instance with
  `link_enabled = true` per session.
- **CMake at build time**: `rusty_link` invokes CMake to build the Link
  C++ sources. Document this in README. On Linux also need
  `build-essential`, on Windows the MSVC CMake toolchain.
- **GPL-3 reminder**: every source file gets a GPL-3 header.

## 13. Out-of-scope items the implementer may notice

- **Sidechain input**: nih-plug supports VST3/CLAP sidechain buses. v2
  feature; v1 reads the main input bus only.
- **Tempo follower for the host**: it would be possible to have the
  plugin set the host's tempo. We are explicitly **not** doing that.
- **OSC output**: explicitly deferred. If added later, slot it in as a
  third parallel output stage alongside Link and MIDI, with its own
  enable BoolParam.
- **Phase publishing on Link**: requires bar detection; deferred.
- **Multiple simultaneous MIDI output streams** (e.g. CC at 4 PPQN AND
  Note at 1 PPQN). v2 feature.
