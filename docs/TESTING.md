# BeatPulse — Testing Reference

This document is the canonical list of everything BeatPulse must verify
before a release. It is kept in sync with the code: a new feature lands
with new test entries here, removed features have entries removed.

For the rationale behind testing-tool choices, see:
- [ADR-0013](adr/0013-pluginval-as-primary-validator.md) — pluginval as primary validator
- [ADR-0014](adr/0014-ballroom-dataset-for-beat-detection-eval.md) — Ballroom dataset
- [ADR-0015](adr/0015-link-integration-test-via-rusty-link-listener.md) — Link listener test

---

## 1. Scope and philosophy

**In scope:**

- DSP correctness (BeatTracker hop accumulation, BeatPLL convergence,
  PulseGenerator timing, SilenceGate state machine, MidiFormatter
  semantics).
- Real-time safety (no allocations / mutex acquisition / syscalls in the
  audio callback).
- VST3 / CLAP plugin conformance (parameter handling, state save/restore,
  speaker arrangements, threading).
- Ableton Link integration (publishing tempo, peer discovery,
  realtime-safe API path).
- MIDI output semantics (CC modes, Note + scheduled note-off,
  channel offsets, stuck-note prevention).
- End-to-end with the primary downstream consumer (Daslight 5).

**Explicitly out of scope:**

- aubio's internal correctness (treated as a trusted upstream).
- DAW host bugs (we report them, we don't work around them blindly).
- Network reliability beyond LAN.
- Sample rates outside 44.1–192 kHz.

---

## 2. Test pyramid

```
                     ┌──────────────────────────┐
                     │ Manual E2E (Daslight 5)  │     §12
                     └──────────────────────────┘
                  ┌──────────────────────────────────┐
                  │ System: Link listener + LinkHut  │  §7
                  └──────────────────────────────────┘
              ┌──────────────────────────────────────────┐
              │ Conformance: pluginval / clap-validator  │  §6
              └──────────────────────────────────────────┘
          ┌──────────────────────────────────────────────────┐
          │ Integration: dataset eval, end-to-end DSP        │  §5
          └──────────────────────────────────────────────────┘
      ┌────────────────────────────────────────────────────────────┐
      │ Unit: per-module DSP, MidiFormatter, params, RT safety     │  §3 §4 §9
      └────────────────────────────────────────────────────────────┘
```

---

## 3. Per-module unit tests

Files live in `tests/`. Each test file maps to one source module.

### 3.1 `tests/pll.rs` — `BeatPLL`

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| P1  | Convergence at 60 BPM: period within 0.1 % in ≤ 20 onsets.                     |
| P2  | Convergence at 100 BPM (same).                                                 |
| P3  | Convergence at 120 BPM (per spec §10).                                         |
| P4  | Convergence at 140 BPM (same).                                                 |
| P5  | Convergence at 180 BPM (same).                                                 |
| P6  | Convergence at 220 BPM — at the upper clamp; verify no overshoot.              |
| P7  | Phase lock from 0.5-period offset: phase at onset time within `0.02·period` of 0 within 10 onsets. |
| P8  | Phase lock from 0.25-period offset (same).                                     |
| P9  | Phase lock from 0.75-period offset (same).                                     |
| P10 | Octave correction up: feed 240 BPM with PLL at 120; tracks 240 within 20 onsets without flip-flop. |
| P11 | Octave correction down: feed 60 BPM with PLL at 120; tracks 60 within 20 onsets. |
| P12 | Tempo step change: 120 BPM × 30 onsets, then 140 BPM × 30 onsets; within 1 % by onset 50. |
| P13 | Recovery after `locked = false` reset; first 8 onsets re-lock as if cold-started. |
| P14 | min_period clamp: feed onsets at 250 BPM (above max 220); period stays clamped. |
| P15 | max_period clamp: feed onsets at 50 BPM (below min 60); period stays clamped. |
| P16 | Numerical stability: simulate 10⁶ samples with no onsets; no drift, no NaN.    |
| P17 | Single onset: no division by zero, `last_onset_sample` set, `locked == false`. |

### 3.2 `tests/pulse_generator.rs` — `PulseGenerator`

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| G1  | Spacing at PPQN=1: pulses at 0, period, 2·period (±1 sample).                  |
| G2  | Spacing at PPQN=2.                                                             |
| G3  | Spacing at PPQN=4 (per spec §10).                                              |
| G4  | Spacing at PPQN=8.                                                             |
| G5  | Spacing at PPQN=16.                                                            |
| G6  | Spacing at PPQN=24.                                                            |
| G7  | Block-boundary pulse lands in correct block with correct sub-block offset.    |
| G8  | PPQN switch mid-stream: no duplicate, no missed pulse at the transition.       |
| G9  | Period change mid-block (PLL update mid-process): pulse spacing adapts.        |
| G10 | `last_pulse_index` reset → next pulse fires at first non-silent sample.        |
| G11 | First-pulse-after-silence timing matches re-entry sample.                      |

### 3.3 `tests/silence_gate.rs` — `SilenceGate`

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| S1  | Audio above threshold → state Active.                                          |
| S2  | Drop to silence: state stays Active for `release_ms`, then Silent.             |
| S3  | Audio returns from Silent → immediate Active + reset hook fires once.          |
| S4  | Threshold transition at -60 dB.                                                |
| S5  | Threshold transition at -50 dB (default).                                      |
| S6  | Threshold transition at -40 dB.                                                |
| S7  | Release timing accuracy ±1 ms over 50–2000 ms range.                           |
| S8  | RMS window correctness across arbitrary block boundaries.                      |
| S9  | Reset hook fires exactly once per silent→active edge (no spurious extras).     |

### 3.4 `tests/midi_formatter.rs` — `MidiFormatter`

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| M1  | CC `Fixed127`: every pulse → CC value 127 with correct sample offset.          |
| M2  | CC `FixedCustom`: every pulse → CC `cc_value`.                                 |
| M3  | CC `Toggle127_0`: alternates 127 / 0 / 127 / 0…                                |
| M4  | Note mode: note-on at pulse, note-off at pulse + `note_length_ms`.             |
| M5  | Note-off scheduled across block boundary lands in the correct future block.    |
| M6  | Stuck-note prevention: new pulse before pending note-off → old note-off flushed at new pulse offset. |
| M7  | Channel offset: emitted MIDI uses `midi_channel - 1` (1→0, 16→15).             |
| M8  | `Both` mode emits CC and Note simultaneously at every pulse.                   |
| M9  | Velocity, note number, CC number all reflect parameter values.                 |
| M10 | Sample-offset preserved through formatter (no rounding to block start).        |

### 3.5 `tests/beat_tracker.rs` — `BeatTracker`

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| B1  | Hop accumulation across host block sizes 32, 64, 128, 256, 512, 1024, 2048.    |
| B2  | Onset sample-offset back-calculation correct relative to host block start.     |
| B3  | Mono input passes through unchanged.                                           |
| B4  | Stereo input correctly downmixed (mean of channels) before aubio.              |
| B5  | Sample-rate change re-initialises aubio resources without leak.                |
| B6  | aubio threshold parameter map: `sensitivity = 0.0 / 0.5 / 1.0` produce expected aubio thresholds. |
| B7  | All seven `OnsetMode` enum values selectable and produce onsets.               |

---

## 4. Real-time safety tests

`tests/realtime_safety.rs`. nih-plug's `assert_process_allocs` feature
panics on heap allocation in `process`. Tests:

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| R1  | Drive `Plugin::process` for 10 000 blocks at 512 samples; no panic.            |
| R2  | Vary block size every iteration (random in 32–2048); no allocation.            |
| R3  | Toggle every parameter mid-stream; no allocation in the audio thread.          |
| R4  | Feed denormal-rich input (very small floats); CPU time stays bounded → FTZ/DAZ active. |
| R5  | No mutex acquisition: instrument with `parking_lot`'s deadlock detection or a thread-local guard. |

---

## 5. Beat-detection accuracy on real audio

### 5.1 Datasets

| Dataset            | Role                | Gate                          | Distribution            |
|--------------------|---------------------|-------------------------------|-------------------------|
| Synthetic clicks   | Always-runs in CI   | Hard: F-measure ≥ 0.95        | In-tree, `tests/data/synthetic/` |
| Ballroom           | Primary acceptance  | Hard: F-measure ≥ 0.70 aggregate, no individual track < 0.50 | External, `BEATPULSE_BALLROOM_DIR` (fetcher: `cargo xtask-fetch-ballroom`) |
| GiantSteps Tempo   | EDM A/B (consensus) | Soft: report ΔTA1/ΔTA2        | External, `BEATPULSE_GIANTSTEPS_DIR` (fetcher: `cargo xtask-fetch-giantsteps`) |
| SMC_MIREX          | Hard-cases stress   | Soft: report only             | External, `BEATPULSE_SMC_DIR` (mirror flaky; manual fallback documented) |

### 5.2 Metrics (Rust impl in `tests/eval/metrics.rs`)

Match `mir_eval` semantics:

- **F-measure** with ±70 ms tolerance window.
- **CMLt** — correct metrical level (continuity-required).
- **AMLt** — allowed metrical level (octave-tolerant).
- **Tempo accuracy 1** — within 4 % of true tempo.
- **Tempo accuracy 2** — within 4 % allowing an octave error.

### 5.3 Harness

The Ballroom dataset isn't openly redistributable; fetch it locally with
the helper xtask, then run the test:

```bash
cargo xtask-fetch-ballroom        # downloads + extracts + pairs annotations
$env:BEATPULSE_BALLROOM_DIR = "<repo>/tests/data/local/ballroom/audio"  # PowerShell
# or:
export BEATPULSE_BALLROOM_DIR="$(pwd)/tests/data/local/ballroom/audio"  # bash/zsh
cargo test --features dataset-tests -- --nocapture
```

`cargo xtask-fetch-ballroom` is feature-gated so the deps it needs
(reqwest, flate2, tar, sha2, md-5, indicatif) don't enter the regular
`cargo xtask bundle …` build path. See `xtask/Cargo.toml` (the
`fetch-datasets` cargo feature) and the aliases in `.cargo/config.toml`.
It downloads the MTG ISMIR 2004 audio mirror and clones the CPJKU
annotation repo, pairing `.beats` next to each `.wav` under
`tests/data/local/ballroom/audio/` (gitignored).

**GiantSteps Tempo** (664 × 2-min EDM previews — exact genre match for
the Daslight DMX use case):

```powershell
cargo xtask-fetch-giantsteps
$env:BEATPULSE_GIANTSTEPS_DIR = "<repo>/tests/data/local/giantsteps/audio"
cargo test --features dataset-tests --test dataset_giantsteps -- --nocapture
```

Audio is per-track (one MP3 per HTTP request) from JKU's stable mirror
with the Beatport CDN as fallback. **Best-effort**: any track whose URL
has rotted is skipped + logged; the test runs against whatever survived.
Final summary reports `<ok>/<total>` and skip reasons. Tempo-only
ground truth (`<id>.LOFI.bpm` = single integer) → harness scores
**TA1** (within ±4 %) and **TA2** (octave-tolerant), no F-measure.

**SMC_MIREX** (217 × 40s adversarial clips):

```powershell
cargo xtask-fetch-smc            # tries the canonical INESC mirror
# If the mirror is down (currently common), follow the printed
# manual-fallback instructions: place wav + .txt files under
# tests/data/local/smc/audio/, then:
cargo xtask-fetch-smc --skip-download   # verifies the layout
$env:BEATPULSE_SMC_DIR = "<repo>/tests/data/local/smc/audio"
cargo test --features dataset-tests --test dataset_smc -- --nocapture
```

The harness:

1. Decodes each WAV to f32 PCM at 44.1 kHz mono via `hound`.
2. Drives `BeatTracker` + `BeatPll` with synthetic block sizes; collects
   beat sample positions.
3. Reads ground-truth `.beats` files; computes per-track metrics.
4. Writes per-track CSV to `tests/data/output/ballroom-<commit>.csv` and
   aggregate JSON to `tests/data/output/ballroom-<commit>.json`.
5. Compares aggregate against `tests/data/baseline-ballroom.json`; fails
   if any metric drops by > 2 %.

### 5.3a Tracking-mode A/B tests (consensus quantification)

ADR-0026 added an opt-in `LookaheadConsensus` tracking mode. Three
hermetic in-tree tests quantify its effect vs Reactive (no external
data needed; run in default `cargo test`):

- `dataset_synthetic::consensus_beats_reactive_on_noisy_clicks` — 120
  BPM clicks + 30 % spurious offbeat clutter. Asserts
  `consensus_F > reactive_F + 0.05`. Typical observed Δ ≈ +0.30.
- `dataset_synthetic::consensus_holds_tempo_on_full_mix_pattern` — kick
  on the beat + 440 Hz tonal stab on every offbeat. Asserts consensus
  passes octave-tolerant tempo accuracy; reactive typically fails.
- `bpm_stability::consensus_more_accurate_bpm_than_reactive_on_noisy_input`
  — % of post-warmup beats where reported BPM is within ±5 % of truth.
  Asserts consensus accuracy > reactive accuracy + 10 pp. **Note:** σ
  of BPM is reported as a diagnostic but not asserted — it's a
  misleading metric for this mode (reactive often has low σ while
  locked on the *wrong* tempo; consensus has high σ because of step-
  snap commits).

The Ballroom comparison test (`dataset_ballroom::ballroom_compare`,
gated on `dataset-tests` feature + `BEATPULSE_BALLROOM_DIR`) writes
side-by-side per-track + aggregate metrics for both modes to
`tests/data/output/ballroom-compare.{csv,json}`. No hard regression
gate — the goal is to *measure* the per-track Δ. Inspect the JSON's
`delta_f_mean` and `n_consensus_better` / `n_reactive_better`
fields after each run.

### 5.4 Updating the baseline

After an intentional algorithm change, re-run the harness and commit
the new baseline JSON in the same PR with a note in the commit body.

---

## 6. VST3 / CLAP conformance

### 6.1 pluginval (VST3) — every CI run

Local invocation (developer machine with display):

```bash
pluginval --strictness-level 5 --validate-in-process \
          --validate target/bundled/beatpulse.vst3
```

CI invocation (headless GitHub Actions runners):

```bash
pluginval --strictness-level 5 --validate-in-process --skip-gui-tests \
          --validate target/bundled/beatpulse.vst3
```

The CI invocation passes `--skip-gui-tests` because GitHub Actions
runners have no display server / GL context — opening the egui editor
crashes with SIGSEGV on Linux (GLX returns null) and an `Option::unwrap`
panic in `baseview/src/gl/win.rs:227` on Windows. The skipped Editor
test is a "does it draw without crashing" check; we cover that path
via the `egui_kittest` snapshot tests in default `cargo test`. All
other pluginval tests — audio processing, parameter state, automation,
bus config, threading — still run.

Exit code 0 = pass. Strictness 5 covers parameter automation, state
save/restore, basic threading, and call coverage. Bumped to 8 before
tagged releases (parameter fuzz, multi-state restore — slower).

### 6.2 Steinberg validator — pre-release

Manual gate, run by a developer with the VST3 SDK built locally:

```
validator target/bundled/BeatPulse.vst3
```

Catches stricter spec issues around speaker arrangements and parameter-
change ordering.

### 6.3 clap-validator (CLAP) — every CI run

```bash
clap-validator validate target/bundled/BeatPulse.clap
```

### 6.4 Smoke-load matrix (manual, per release)

| Host                        | Format | Notes                                             |
|-----------------------------|--------|---------------------------------------------------|
| FL Studio 21 (Windows)      | VST3   | Primary host; full feature exercise.              |
| Reaper (Windows + Linux)    | VST3 + CLAP | Free, scriptable; good for regression sweeps. |
| Bitwig demo                 | CLAP   | Reference CLAP host.                              |
| Ableton Live trial          | VST3   | Cross-check Link interop with the canonical Link host. |

Document host-specific quirks as they're discovered, in `docs/HOSTS.md`.

---

## 7. Ableton Link integration

### 7.1 Automated: `tests/bin/link_listener.rs`

Feature-gated:

```bash
cargo test --features link-integration
```

Steps:

1. Construct a separate `rusty_link::AblLink`, enable it, join the local
   Link session.
2. Spawn BeatPulse's DSP pipeline in a worker thread.
3. Feed a generated 100 BPM click → wait 10 s → assert published tempo
   within 1 BPM.
4. Repeat for 120 BPM and 140 BPM.
5. Step-change test: 120 → 140 mid-stream, assert convergence within 10 s.

### 7.2 Manual cross-check with LinkHut

1. Build Ableton's `LinkHut` example (from the official Link C++ repo).
2. Launch LinkHut on the same machine.
3. Load BeatPulse in FL Studio.
4. Confirm: BeatPulse's UI shows `link_peers = 1`; LinkHut shows
   "BeatPulse" peer.
5. Feed 120 BPM kick; LinkHut tempo display tracks BeatPulse's BPM
   within 1 BPM after lock.
6. Toggle BeatPulse's `Link enable` off → LinkHut peer count drops.

### 7.3 Multi-instance behaviour (manual)

Verify ADR-0012: load two BeatPulse instances both with Link enabled,
feeding different tempos. Confirm visible session-tempo thrash. Confirm
disabling Link on one instance restores stable behaviour.

### 7.4 Network conditions matrix (manual)

| Scenario                                     | Expected                                |
|----------------------------------------------|-----------------------------------------|
| Same subnet, multicast allowed               | Peers discover within ~1 s.             |
| Different subnets                            | No peer discovery (documented limit).   |
| Wi-Fi multicast disabled                     | No peer discovery; document workaround. |
| Windows Defender "Public network" profile    | Blocked; document firewall rule.        |
| VPN client active                            | Often blocked; document.                |

---

## 8. MIDI output

### 8.1 Automated

Covered by `tests/midi_formatter.rs` (§3.4).

### 8.2 Manual via loopMIDI + MIDI-OX

1. Install [loopMIDI](https://www.tobias-erichsen.de/software/loopmidi.html)
   and [MIDI-OX](http://www.midiox.com/).
2. Create a virtual MIDI port `BeatPulse-Out` in loopMIDI.
3. Route BeatPulse's MIDI output to `BeatPulse-Out` in FL Studio.
4. Open the port in MIDI-OX.
5. Verify per parameter combination:
   - CC#, Note#, channel, velocity match parameter values.
   - Pulse rate (events per detected quarter note) matches PPQN setting.
   - Note length matches `note_length_ms` ±2 ms.
   - Rapid PPQN switches produce no stuck notes.
   - Silence → MIDI output stops; resume → first pulse fires correctly.

---

## 9. Parameter behaviour

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| Q1  | Save preset → load preset: every parameter byte-identical.                     |
| Q2  | Parameter automation via host: pluginval fuzz covers most; manual sweep `sensitivity` while audio plays. |
| Q3  | Sensitivity LUT at 0.0 / 0.5 / 1.0 matches spec §7 (aubio threshold, alpha_period, alpha_phase). |
| Q4  | `manual_resync` trigger resets PLL state on the next process call.             |
| Q5  | All BoolParam toggles take effect within one process block.                    |
| Q6  | EnumParam changes don't allocate or leak.                                      |
| Q7  | IntParam clamps to its declared range.                                         |

---

## 10. Audio sanity (passthrough)

The plugin must be a perfect passthrough on its main audio bus.

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| A1  | Input == output sample-for-sample (±1 ULP) at 44.1 kHz, block 512.             |
| A2  | Same at 48, 96, 192 kHz.                                                       |
| A3  | Same across block sizes 32, 64, 128, 256, 512, 1024, 2048.                     |
| A4  | Stereo channels independent (no cross-talk).                                   |

---

## 11. Performance

| ID  | Case                                                                           |
|-----|--------------------------------------------------------------------------------|
| F1  | CPU < 5 % of one core at 44.1 kHz / 512 block on the reference machine. (Document machine: CPU model, OS.) |
| F2  | No allocator activity in `process` (covered by §4 R1–R3).                      |
| F3  | Memory bounded: 1 hour of `process` calls under Valgrind (Linux build) shows no growth. |
| F4  | Link enabled vs disabled CPU delta < 0.5 % (Link's network thread does not bleed into audio). |

---

## 12. Manual end-to-end with Daslight 5

Procedure mirrors spec §10 with explicit pass criteria.

1. Load BeatPulse on an FL Studio mixer channel with a 120 BPM kick loop.
2. In Daslight 5: Settings → Audio → enable Ableton Link.
3. **Pass:** within 4–8 beats, BeatPulse's `LOCKED` LED is on and
   Daslight's BPM display reads 119–121.
4. Stop the kick. **Pass:** Daslight's tempo holds at the last published
   value (Link does not "release" tempo).
5. Restart with a 140 BPM kick. **Pass:** Daslight converges to 139–141
   within 10 seconds.
6. Trigger a Daslight scene mapped to "on bar". **Pass:** scene fires on
   detected beats with no visible wobble (subjective).

---

## 13. CI workflow

`.github/workflows/ci.yml`:

| Trigger             | Job                          | Steps                                             |
|---------------------|------------------------------|---------------------------------------------------|
| PR                  | `unit`                       | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` |
| PR                  | `bundle-validate` (Windows)  | `cargo xtask bundle ... --release`, pluginval s5, clap-validator |
| PR                  | `bundle-validate` (Linux)    | Same as Windows.                                  |
| PR                  | `link-integration`           | `cargo test --features link-integration`          |
| Push to main        | All of the above + Coverage  | Codecov upload.                                   |
| Tag `v*`            | `release`                    | All of the above at strictness 8 + create GitHub release with bundles. |

Datasets are **not** downloaded in CI (size + license). The `dataset-tests`
feature is run by developers locally before bumping the baseline JSON.

---

## 14. Out-of-test scope (explicit)

- aubio internal correctness.
- DAW host bugs (reported upstream, not patched around silently).
- Network reliability beyond LAN multicast.
- Behaviour at sample rates outside 44.1–192 kHz.
- Plugin formats other than VST3 + CLAP (per spec §2).
