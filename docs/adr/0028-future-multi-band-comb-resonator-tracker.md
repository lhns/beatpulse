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

## Update — debugging pass (post-rejection)

A second pass added 6 component-correctness unit tests (the existing smoke tests passed, hiding the bugs) and applied three fixes:

1. **Cross-correlation phase extraction.** Added `Resonator::phase_acc` — a per-phase smoothed |y| accumulator (γ=0.85, ~7-tick integration). `phase_of` now reads the accumulator's argmax instead of the raw `y_hist` argmax. This locks on the dominant *periodic* alignment, not the most recent loud transient.
2. **Extended `default_period_range` down to ~430 BPM-equivalent (τ=24).** The original range (47..172) couldn't supply sub-harmonic τ/2 evidence for any tactus τ ≤ 94 — i.e. for everything in the 120-220 BPM band, where most Ballroom tempos sit. The BPM prior keeps the new short-τ resonators from being chosen as the tactus; they exist only as supporting evidence for longer ones.
3. **Loosened the BPM prior** from σ=0.35 → 0.5 in log-BPM space (1σ now covers 73-200 BPM, was 85-170). Rationale: real Ballroom has slow waltzes (~80 BPM) and fast jives (~200 BPM) at the σ tail, and the joint sub/super-harmonic evidence (now usable thanks to fix #2) is the primary octave discriminator.

After all three fixes, the in-tree component tests all pass — including the end-to-end `klapuri_emits_within_70ms_of_truth_on_clean_120bpm_kick` (mean error 142 ms → ≤ 35 ms) and `klapuri_locks_to_correct_octave_on_120bpm` (110 BPM → within ±4% of 120). The component pipeline genuinely works on synthetic input.

**Re-measured on Ballroom (n=687, same scoring):**

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| Pre-fix (original) | 0.277 | 0.080 | 0.044 | 0.045 |
| Post-fix (debugging pass) | **0.318** | 0.043 | 0.017 | 0.031 |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

F crept up by +0.041 — but TA2 actually got *worse* (0.045 → 0.031). The fixes corrected the synthetic-input failure modes, but on real audio the tempo selection became more wrong, not less. Likely cause: the extended period range (B2) introduced spurious high-frequency resonator energy from real ballroom audio's hi-band content (hats, cymbals, hi-frequencies); these short-τ resonators now feed sub-harmonic evidence into the inference, but the evidence is *for the wrong tempo*. We traded a phase bug for a sub-harmonic-bias bug.

**Per the debugging plan's Phase C gate (F < 0.45 → stop), this stays Rejected.** ADR-0028 status remains Rejected; the implementation is preserved for any future revival, with the new measurement and the diagnostic notes above.

## Update — third debugging pass: per-τ integration-gain normalisation

A diagnostic harness (`tests/klapuri_diagnose.rs`) instrumented the accent + bank + inference pipeline and ran on 5 representative Ballroom tracks. Findings:

- DC-bin contribution to low-band power: **0.0 %** — DC contamination was *not* the bug.
- Per-band accent magnitudes: roughly balanced (max/min ~1.5×). Per-band imbalance was *not* the bug.
- **Top-5 raw resonator energies on every single track: τ=23, 24, 25, 26, 27** (the shortest periods in the bank) regardless of true tempo.
- **Inference winner on every single track: τ=69 (149.8 BPM)** regardless of true tempo (125, 171, 210, 133, 98).

The structural bug: for a comb filter `y[n] = α·y[n-τ] + (1-α)·x[n]` driven by broadband noise, the steady-state response is `E[y²] = (1-α)/(1+α)`. Short τ has smaller α (since `α = 0.5^(1/(2τ))`), so `(1-α)/(1+α)` is **larger** — short-τ resonators accumulate ~3.5× more energy on real broadband audio than long-τ ones for the same input statistics. On synthetic impulse trains the bank only fires resonators whose τ divides the impulse rate; on real continuous accent every resonator gets excited, and the short-τ ones dominate. The sub-harmonic inference then over-counts these inflated values as evidence for whatever tactus happens to have τ/2 or τ/3 near 23-27. Result: every track converges to the same locally-optimal tactus.

Fix landed in `src/dsp/klapuri/resonators.rs::total_energies`: divide each resonator's energy by `(1-α)/(1+α)` so resonators are comparable across τ. The comparison becomes "does this resonator have *anomalously* high energy relative to its baseline?", which is the right question for period inference.

Re-measured on Ballroom (n=687, same scoring):

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| Pre-debugging | 0.277 | 0.080 | 0.044 | 0.045 |
| Second pass (cross-corr + range + prior) | 0.318 | 0.043 | 0.017 | 0.031 |
| **Third pass (+ gain normalisation)** | **0.330** | 0.070 | 0.066 | 0.080 |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

TA1 (strict tempo) **quadrupled** (1.7 % → 6.6 %), TA2 **nearly tripled** (3.1 % → 8.0 %) — TA2 is now *above* the random-period chance level for the first time. F barely moved (+0.012) because correct tempo selection on a fraction of tracks doesn't help F when the phase is still drifting or the period is still wrong on most tracks. The bug-fix moved the algorithm from "uniformly wrong" toward "sometimes right", but the residual gap to AubioTempo's 0.59 / 0.78 is structural — the algorithm correctly selects the tempo on only ~8 % of tracks, vs aubio's ~78 %.

**Per the plan's Phase F gate (F < 0.45 → stop), this stays Rejected.** Three debugging passes, three real bugs fixed, but the algorithm still substantially underperforms aubio on Ballroom. Klapuri-from-paper appears to need significantly more work than a single implementation pass — either more bugs we haven't found, a structurally different accent pipeline, or the missing measure-level joint inference. The implementation in `src/dsp/klapuri/` (now with the per-τ normalisation fix) is preserved for any future fourth attempt; 19 component unit tests + the `klapuri_diagnose.rs` harness make further diagnosis straightforward.

**What would be needed to make Klapuri work for us:**

- Per-genre / per-track sub-harmonic weight calibration (the over-extended range needs a per-τ weight that decays for very short τ, instead of a flat `w_tatum_half`).
- Or: tighter per-band normalization in the accent stage so the high-frequency channels don't dominate the resonator bank.
- Or: implement the missing measure-level (downbeat) joint inference — Klapuri's full joint posterior over tatum/tactus/measure may be what suppresses the wrong-octave attractors.

Each of those is non-trivial. AubioTempo at F=0.59 / TA2=0.78 remains the production default. ADR-0027 stands.

## References

- Klapuri, A.P., Eronen, A.J., and Astola, J.T. *Analysis of the meter of acoustic musical signals.* IEEE TASLP 14(1):342–355, 2006. https://www.iro.umontreal.ca/~pift6080/H09/documents/papers/klapuri_meter.pdf
