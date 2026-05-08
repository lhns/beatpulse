# ADR-0003: Use aubio for onset detection rather than madmom or BeatNet

## Status

Accepted.

## Context

Three candidate libraries were evaluated for the onset / beat detection
component:

| Library  | Algorithm vintage           | Real-time API | Latency    | License | Notes                          |
|----------|-----------------------------|---------------|------------|---------|--------------------------------|
| aubio    | classical DSP (2003-ish)    | Yes, native   | ~10–20 ms  | LGPL-3  | C library, mature              |
| madmom   | RNN + DBN (2016)            | Offline-first | High       | BSD-3   | Python, dormant since ~2022    |
| BeatNet  | CRNN + particle filter (2021) | Yes (online) | Medium     | MIT     | Python + PyTorch, modern       |

Published benchmarks (GTZAN F-measure, ±70 ms tolerance):

- madmom (offline DBN): ~88–90%
- BeatNet (online): ~75–80%
- BEAST (2024 SOTA online, not in scope): 80%
- aubio: not separately benchmarked on GTZAN; classical DSP baseline,
  generally considered substantially below madmom on accuracy but
  competitive on clean 4/4 electronic material.

Two key observations weighted the decision:

1. **The use case is kick-driven 4/4 for DMX cue generation**, not the
   full GTZAN distribution which includes jazz, classical, and vocal
   genres where aubio falls off most. On the actual target material the
   accuracy gap is much smaller than benchmark numbers suggest.
2. **Phase smoothness, not raw F-measure, is what matters for DMX.** None
   of the libraries optimise for this. Whichever library is chosen, a PLL
   layer on top is required to produce smooth pulse output. The PLL does
   most of the work that distinguishes a usable tool from a jittery one.

## Decision

Use **aubio** via the `aubio-rs` Rust binding. Use the raw `Onset` detector
(not `Tempo`) to give the PLL unfiltered observations.

## Consequences

- Lower CPU cost; runs comfortably in an audio callback.
- LGPL-3 license — propagates to the plugin; the plugin must therefore be
  GPL-3 (see ADR-0009).
- On hard material (jazz, ambient, complex polyrhythm) accuracy will be
  visibly worse than a neural tracker. Document this as a known limit.
- The PLL layer is a load-bearing component, not an afterthought.

## Honesty note on the original library comparison

The accuracy ratings in the initial library comparison ("good" / "very good"
/ "excellent") were not derived from a benchmark run; they were a synthesis
of qualitative literature consensus. Pushed back on, the actual numbers were
looked up and the ranking held — but the original presentation overstated
its rigor. Recorded here so future revisits don't re-mistake confidence for
measurement.
