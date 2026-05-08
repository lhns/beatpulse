# ADR-0008: Choose Rust + nih-plug over C++ + JUCE

## Status

Accepted.

## Context

Two viable plugin development stacks were considered:

**C++ + JUCE**

- Industry standard, used by most commercial plugins.
- Mature, deeply documented, large hire-able talent pool.
- VST3, AU, AAX, VST2 from one codebase. CLAP via `clap-juce-extensions`.
- Polished UI component system.

**Rust + nih-plug**

- Younger framework (single primary maintainer), no crates.io release.
- VST3 and CLAP from one codebase. Standalone build.
- Memory safety guarantees particularly relevant in real-time audio code
  (no use-after-free → no stuck notes; no allocator-in-audio-thread bugs
  caught at compile time via patterns like `#[no_alloc]`).
- UI via egui or vizia — workable, less polished than JUCE.
- Smaller talent pool.

For *this specific project*:

- The user has Rust-adjacent background (Scala, network proxies, type-system
  comfort) and is more likely to enjoy maintaining Rust than C++.
- The plugin is small enough that JUCE's component-library advantage doesn't
  matter much.
- Audio plugins are exactly the domain where Rust's safety guarantees pay
  off — real-time, no GC, lifetime-checked buffer access.
- nih-plug's VST3 + CLAP coverage is sufficient; AU and AAX are not v1
  targets.

## Decision

Use **Rust + nih-plug**. UI via `nih_plug_egui`. DSP via `aubio-rs` and
custom Rust modules. Link via `rusty_link`.

## Consequences

- Faster iteration with cargo's tooling (testing, linting, formatting).
- Pinned-by-git-commit dependency on nih-plug; future updates may break
  the API. Pin a specific SHA in `Cargo.toml`.
- CMake dependency at build time (from `rusty_link`), so Windows users need
  the MSVC CMake toolchain installed.
- Slightly more friction integrating a C library (aubio) than in C++, but
  manageable; `aubio-rs` does the work.

## Considered alternative kept aside: hand-rolled onset detector

Both `aubio-rs` (LGPL-3 via aubio) and a hand-rolled spectral-flux onset
detector (~150 lines of `rustfft`) were considered. v1 keeps aubio for
faster time-to-working-prototype. v2 may replace it if the LGPL constraint
becomes inconvenient. Note: this only removes one of the two copyleft
constraints; Link itself is GPL-2-or-later.
