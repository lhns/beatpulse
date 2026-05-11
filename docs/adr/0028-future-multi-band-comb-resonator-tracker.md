# 0028 — Future direction: multi-band-accent + comb-filter resonator tracker (Klapuri family)

## Status

Deferred (recorded 2026-05-12). Not planned for the current iteration; written so the option is documented when we revisit the F-measure ceiling.

## Context

`AubioTempo` (ADR-0027) lifted Ballroom F-measure from 0.288 (Reactive) to 0.590 with TA2=0.777 — TA2 in literature range. The remaining ~0.07 gap to OBTAIN's published aubio F=0.66 is likely methodology drift (annotation-set version, aubio version, sample-rate convention). The bigger gap, ~0.20, to the F=0.75–0.85 cited for *other* trackers (Bock/madmom) is genuine algorithmic — aubio's Tempo (single-band onset + autocorrelation period selection) plateaus where the literature's leading non-neural trackers do not.

User asked us to look at **waveclock** (https://wavesum.net/waveclock-audio-to-midi-clock.html), a closed-source commercial real-time MIDI-clock tracker. Wavesum's product page states it's based on Klapuri/Eronen/Astola 2006 (*Analysis of the meter of acoustic musical signals*, IEEE TASLP 14(1):342–355). That paper is the reference for the family of trackers that consistently outperform aubio in the literature.

## The Klapuri 2006 design (one paragraph)

Three-stage real-time tracker:

1. **Multi-band accent signal**: 4 sub-bands of mel-warped spectral flux, instead of aubio's single-band onset envelope. Handles bass-only and percussive-only material with different sensitivities — the dominant Ballroom failure mode (slow waltz vs fast samba) is a band-balance problem.
2. **Bank of comb-filter resonators**: ~50 resonators tuned to candidate periods (60–250 BPM). Each resonator integrates the multi-band accent through its own comb filter; resonator amplitude is the period's evidence.
3. **Joint tatum/tactus/measure inference**: a probabilistic prior over period ratios *jointly* infers all three pulse levels, suppressing octave errors (the failure mode that costs most F-measure on Ballroom). Phase comes from the comb filter's internal delay — no separate PLL.

Algorithmic latency is ~one tactus period (~0.5 s) for first lock; steady-state phase output is near-zero added latency. STFT-based, ~23 ms FFT window with ~5.8 ms hop at 44.1 kHz.

## Reference implementation

`michaelkrzyzaniak/Beat-and-Tempo-Tracking` ([GitHub](https://github.com/michaelkrzyzaniak/Beat-and-Tempo-Tracking)) is an MIT-licensed C implementation of the Klapuri family (~1.1k LoC, single file `src/BTT.c`). Adaptable in roughly a working day to Rust if/when we want a fourth tracking mode.

## Decision (deferred)

Do **not** implement now. Adopting it means:

- Replacing the entire front of the pipeline for the new mode (no aubio for it).
- A new module (`src/dsp/comb_resonator_tracker.rs` or similar) of substantial size.
- A fourth `TrackingMode` variant (we'd want to keep AubioTempo as the default until the new tracker is measured at parity or better).
- Re-measurement on Ballroom + the synthetic A/B tests + verification on actual DJ-rig material.

Estimated effort: 2–4 days of focused work. Worth doing only if:

- AubioTempo's F=0.59 / TA2=0.78 proves insufficient in real-world Daslight-DMX use (visible LED jitter on full mixes that aren't ballroom-style 4/4).
- The 0.07 OBTAIN gap turns out to be aubio version drift we can't close (i.e. aubio is at or near its ceiling on this material).

## References

- Klapuri, A.P., Eronen, A.J., and Astola, J.T. *Analysis of the meter of acoustic musical signals.* IEEE TASLP 14(1):342–355, 2006. https://www.iro.umontreal.ca/~pift6080/H09/documents/papers/klapuri_meter.pdf
- Krzyzaniak, M. *Beat-and-Tempo-Tracking* (BTT). MIT-licensed reference implementation. https://github.com/michaelkrzyzaniak/Beat-and-Tempo-Tracking
- Wavesum *waveclock*. Closed-source commercial product, cited as Klapuri-based. https://wavesum.net/waveclock-audio-to-midi-clock.html
