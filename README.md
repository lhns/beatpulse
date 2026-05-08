# BeatPulse

Real-time beat detector VST3 + CLAP plugin that broadcasts the detected
tempo to an [Ableton Link](https://www.ableton.com/en/link/) session and
emits parallel MIDI Note/CC pulses.

Primary downstream target: **Daslight 5** (DMX) via Link tempo source.
Primary host: **FL Studio** on Windows. Should work in any VST3 or CLAP
host.

## Status

Pre-alpha. See `BeatPulse-SPEC.md` for the design and `docs/adr/` for
architecture decisions.

## License

**GPL-3.0-or-later.** Forced by the combination of:

- `aubio` (LGPL-3) — onset detection
- Ableton Link (GPL-2-or-later) — tempo broadcast

See `LICENSE` and `docs/adr/0009-gpl3-licensing.md`.

## Build

Prerequisites:

- Rust ≥ 1.75 (`rustup install stable`)
- CMake ≥ 3.14 (used by `rusty_link` to build Ableton Link's bundled C++)
- A C/C++ toolchain (MSVC on Windows, gcc/clang on Linux)

```bash
git clone https://github.com/lhns/beatpulse.git
cd beatpulse
cargo build --release
cargo xtask bundle beatpulse --release
```

Bundles land in `target/bundled/`. Install paths:

| OS      | VST3                                       | CLAP                                       |
|---------|--------------------------------------------|--------------------------------------------|
| Windows | `C:\Program Files\Common Files\VST3\`      | `C:\Program Files\Common Files\CLAP\`      |
| Linux   | `~/.vst3/`                                  | `~/.clap/`                                  |
| macOS   | `~/Library/Audio/Plug-Ins/VST3/`            | `~/Library/Audio/Plug-Ins/CLAP/`            |

## Test

```bash
cargo test                                # unit tests
cargo test --features dataset-tests       # Ballroom F-measure (set BEATPULSE_BALLROOM_DIR)
cargo test --features link-integration    # in-process Link listener test
```

Plugin conformance:

```bash
pluginval --strictness-level 5 --validate target/bundled/BeatPulse.vst3
clap-validator validate target/bundled/BeatPulse.clap
```

Full testing reference: `docs/TESTING.md`.

## Manual end-to-end with Daslight 5

1. Load BeatPulse on an FL Studio mixer channel with a 120 BPM kick loop.
2. In Daslight 5: Settings → Audio → enable Ableton Link.
3. BeatPulse's `LOCKED` LED lights after ~4–8 beats. Daslight's BPM
   display follows BeatPulse within ~1 second.
4. Stop the kick. Daslight's tempo holds at the last published value.
5. Restart with a different BPM. Daslight converges within ~10 s.

If Daslight reports zero peers:

- Confirm both machines are on the same subnet.
- Allow multicast through Windows Defender (private network profile).
- Disable VPN clients that block multicast.

## Architecture

See `BeatPulse-SPEC.md` and `docs/adr/`.
