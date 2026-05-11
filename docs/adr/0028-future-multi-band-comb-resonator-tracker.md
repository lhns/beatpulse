# 0028 — Multi-band-accent + comb-filter resonator tracker (Klapuri family)

## Status

**Attempted (2026-05-NN). Rejected for now.** A from-paper implementation lives in `src/dsp/klapuri/` but is *not* wired into `BeatSource`; the standalone Ballroom benchmark fell well below the integration gate.

## Context

`AubioTempo` (ADR-0027) lifted Ballroom F-measure from 0.288 (Reactive) to 0.590 with TA2=0.777 — TA2 in literature range. The remaining ~0.07 gap to OBTAIN's published aubio F=0.66 is likely methodology drift (annotation-set version, aubio version, sample-rate convention). The bigger gap, ~0.20, to the F=0.75–0.85 cited for *other* trackers (Bock/madmom) is genuine algorithmic — aubio's Tempo (single-band onset + autocorrelation period selection) plateaus where the literature's leading non-neural trackers do not.

The next algorithmic step is the Klapuri/Eronen/Astola 2006 family (*Analysis of the meter of acoustic musical signals*, IEEE TASLP 14(1):342–355) — multi-band accent + comb-filter resonator bank with joint tatum/tactus/measure inference. That paper is the reference for the family of trackers that consistently outperform aubio in the literature.

## The Klapuri 2006 design (one paragraph)

Three-stage real-time tracker:

1. **Multi-band accent signal**: 4 sub-bands of mel-warped spectral flux, instead of aubio's single-band onset envelope. Handles bass-only and percussive-only material with different sensitivities — the dominant Ballroom failure mode (slow waltz vs fast samba) is a band-balance problem.
2. **Bank of comb-filter resonators**: ~50 resonators tuned to candidate periods (60–250 BPM). Each resonator integrates the multi-band accent through its own comb filter; resonator amplitude is the period's evidence.
3. **Joint tatum/tactus/measure inference**: a probabilistic prior over period ratios *jointly* infers all three pulse levels, suppressing octave errors (the failure mode that costs most F-measure on Ballroom). Phase comes from the comb filter's internal delay — no separate PLL.

Algorithmic latency is ~one tactus period (~0.5 s) for first lock; steady-state phase output is near-zero added latency. STFT-based, ~23 ms FFT window with ~5.8 ms hop at 44.1 kHz.

## Reference implementation

No public Klapuri-family reference implementation exists; an adoption requires implementing from the paper (~1-2 weeks of focused work).

## Decision

We attempted a from-paper implementation. Layout:

- `src/dsp/klapuri/accent.rs` — STFT (1024 / 256, Hann) → 4 mel-warped sub-bands → log-compressed magnitude → leaky-integrator DC removal → half-wave-rectified differential. 11 in-tree unit tests pass (band coverage, kick-train detection, broadband response).
- `src/dsp/klapuri/resonators.rs` — Bank of IIR comb filters covering 60-220 BPM (≈ 47-172 OSS frames at 172 Hz hop), per-period α tuned for half-energy at one period. Per-resonator energy via leaky integrator.
- `src/dsp/klapuri/period_inference.rs` — Joint posterior combining tactus energy + sub-harmonic (τ/2, τ/3) and super-harmonic (2τ, 3τ, 4τ) supports + log-Gaussian BPM prior at 120 BPM, σ=0.35 in log-BPM space.
- `src/dsp/klapuri/phase.rs` — Predictive beat emission. Periodic re-inference (every 64 OSS frames) updates τ; phase from the winning resonator's delay-line argmax. Public `KlapuriTracker::process_block` callback API.

Standalone benchmark `tests/klapuri_experiment.rs` on Ballroom (n=687, `trim_beats(5.0)` + Sturm 2013 dups skipped):

| Tracker     | F     | AMLt  | TA1   | TA2   |
|-------------|-------|-------|-------|-------|
| Klapuri (mine) | **0.277** | 0.080 | 0.044 | 0.045 |
| AubioTempo (ref) | 0.590 | 0.459 | 0.592 | 0.777 |
| ΔF          | **−0.313** | — | — | — |

Per the plan's Phase 5 gate (`F < 0.59 → don't integrate`), this is a **clear rejection**. TA2=4.5% is below the chance level for octave-tolerant tempo accuracy (a random period selector would land in the right family ≈ 10-20 % of the time on Ballroom), which means the implementation is doing something actively wrong, not just under-tuned.

**Likely causes (ranked):**

1. **Phase argmax is not the right phase signal.** Klapuri §IV-D uses the resonator output's delay-line state, but the precise definition (peak vs envelope, summed across channels vs per-channel) matters. My implementation takes `argmax_j (Σ_c |y_c[n−1−j]|)` which may not align with the paper's "phase such that the impulse-response template matches recent input".
2. **Period-inference scoring may need different weights.** Defaults (`w_tatum_half=0.6`, etc.) are guesses from the paper's discussion, not the paper's measured optimal. A grid search over weights on Ballroom would be the next step if revived.
3. **Accent stage normalization.** Klapuri uses a specific "spectral whitening" that I approximated with `log(1 + 100·power)`. Per-band normalization differences could starve some bands of dynamic range.
4. **Inference cadence.** I re-run every 64 OSS frames (~0.37 s); paper updates more often via dynamic-programming continuity. My implementation re-anchors only on >10% τ change, which may be too sticky.
5. **No measure-level (downbeat) inference.** The paper's joint inference includes the measure level; I only score tactus + tatum subharmonics. Missing measure could destabilise the joint posterior.

**Path forward if we revisit:**

- Inspect a few per-track failure modes (dump the inference scores + chosen τ for, say, 5 tracks). Determine whether period or phase is the dominant error.
- Consider porting from a battle-tested reference (no MIT Klapuri-family implementation found in the original research; mostly papers + closed-source products) — would need to be re-investigated.
- Or pivot to a different non-aubio algorithm (Stark/Plumbley CBSS via `michaelkrzyzaniak/Beat-and-Tempo-Tracking` MIT C, ~3-4k LoC port; or a small neural model).

The Klapuri code is left in place as a reference for any future revival; not exported beyond `pub mod klapuri;` in `src/dsp/mod.rs`. The `tests/klapuri_experiment.rs` standalone benchmark is preserved so a future improvement can be measured against the same reference numbers.

## References

- Klapuri, A.P., Eronen, A.J., and Astola, J.T. *Analysis of the meter of acoustic musical signals.* IEEE TASLP 14(1):342–355, 2006. https://www.iro.umontreal.ca/~pift6080/H09/documents/papers/klapuri_meter.pdf
