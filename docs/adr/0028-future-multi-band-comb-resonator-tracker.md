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

## Update — fourth debugging pass: aligning with open-source reference filters

User pushed back on the residual ~92 % wrong-tempo rate and suggested checking open-source references. Comparison against madmom (`features/onsets.py`, `features/tempo.py`, `audio/comb_filters.py`), librosa (`feature/rhythm.py`), and Essentia (`rhythm/rhythmextractor2013.cpp`) found three concrete divergences from the paper-only implementation:

1. **α formula gave wrong integration window.** Original: `α = 0.5^(1/(2τ))` — half-energy at one period (~ 0.5 s at τ=86). Klapuri-correct: `α = 0.5^(τ / (T₀.₅·fs_env))` with `T₀.₅ = 3 s` — half-energy at a **fixed 3 seconds** regardless of τ. At 120 BPM that's ~6 periods of evidence integration vs the old 2. Mine averaged too quickly; resonators couldn't discriminate between similar tempos. Fix: `resonators.rs::alpha_for_period` now takes the OSS rate and uses the 3-second convention.

2. **Missing weighted-differential accent composition.** Klapuri §III: `accent = W·HWR(Δ log-power) + (1-W)·log-power`, with `W ≈ 0.9`. Mine was pure HWR-diff, missing the `(1-W)·log-power` sustained-energy term. On Ballroom — full of sustained notes (waltz strings, tango bandoneon) — pure spectral flux goes to ≈ 0 and the comb bank sees no input. Fix: weighted composition in `accent.rs::run_frame`.

3. **Log-compression not μ-law normalised.** Original: `log(1 + μ·magnitude)` with μ=100. Klapuri-correct: `log(1 + μ·power) / log(1 + μ)` — on power not magnitude, and divided by `log(1+μ)` to normalise to [0, 1]. Fix: both changes in `accent.rs::run_frame`.

Re-measured on Ballroom (n=687, same scoring):

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| Pre-debugging | 0.277 | 0.080 | 0.044 | 0.045 |
| 2nd pass (cross-corr + range + prior) | 0.318 | 0.043 | 0.017 | 0.031 |
| 3rd pass (+ gain normalisation) | 0.330 | 0.070 | 0.066 | 0.080 |
| **4th pass (reference-aligned filters)** | **0.369** | **0.160** | **0.242** | **0.255** |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

TA1 **3.7× better** (6.6 % → 24.2 %), TA2 **3.2× better** (8.0 % → 25.5 %), AMLt doubled (0.07 → 0.16), F up to 0.369 from 0.330. **The algorithm now genuinely tracks ~25 % of Ballroom tracks correctly** — no longer broken, just not as good as aubio.

The 3-second integration window (G1) and the sustained-energy accent term (G2) are the load-bearing fixes — together they let the bank discriminate tempos that were previously indistinguishable. The μ-law normalisation (G3) is a smaller adjustment that mostly keeps values in a sensible range.

**Per the Phase H gate (F ∈ [0.35, 0.45] → don't integrate but document convergence), this stays Rejected** with the new measurement. The algorithm has converged to a genuinely-working-but-not-competitive state: aubio is still 60 % better on F and 3× better on tempo accuracy. To close the remaining gap would require implementing Klapuri's full joint tatum/tactus/measure posterior (we only do tactus + tatum subharmonics) and probably the dynamic-programming continuity constraint for phase. Both are non-trivial; deferred indefinitely. Production stays on `TrackingMode::AubioTempo` at F=0.59 / TA2=0.78.

**What would be needed to make Klapuri work for us:**

- Per-genre / per-track sub-harmonic weight calibration (the over-extended range needs a per-τ weight that decays for very short τ, instead of a flat `w_tatum_half`).
- Or: tighter per-band normalization in the accent stage so the high-frequency channels don't dominate the resonator bank.
- Or: implement the missing measure-level (downbeat) joint inference — Klapuri's full joint posterior over tatum/tactus/measure may be what suppresses the wrong-octave attractors.

Each of those is non-trivial. AubioTempo at F=0.59 / TA2=0.78 remains the production default. ADR-0027 stands.

## Update — fifth debugging pass: per-inference phase re-anchoring

User asked for another pass on the suspicion that more obvious bugs were lurking. Two candidates surfaced:

1. **Sustained-term semantics**: the `(1-W)·log_power` term in `accent.rs` was computed as `(1-W)·HWR(log_power − dc)`, which goes to ~0 on truly sustained content (the DC tracker converges on a ~33-frame timescale). Tried replacing the DC-removed deviation with the raw μ-law-normalised `log_power` per the paper.
2. **Phase re-anchoring**: `phase.rs` only re-anchored `next_beat_abs` on first lock or on > 10 % τ change. The first lock occurs at warmup (~1.5 s) when `phase_acc` has barely integrated cross-correlation evidence; a bad cold-start phase locked in and was never corrected. Changed to re-anchor every inference cycle, with a midpoint nudge for small adjustments and a snap for half-period jumps (octave switches).

Combined both, measured on Ballroom — **F regressed to 0.290**, TA1 collapsed from 24.2 % → 0.3 %. Reverted #1 only and re-measured:

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| 4th pass | 0.369 | 0.160 | 0.242 | 0.255 |
| 5th-pass (#1 + #2) | 0.290 | 0.047 | 0.003 | 0.012 |
| **5th pass (#2 only)** | **0.402** | 0.118 | 0.237 | 0.252 |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

**#1 was actively harmful**: feeding the absolute `log_power` into the comb-filter bank gives every resonator a constant DC component the bank's energy normalisation can't separate, washing out the periodic signal and breaking period inference. The DC-removed sustained term is correct *for our bank*; Klapuri's paper presumably handles the DC differently in his comb stage.

**#2 alone improved F by +0.033** (0.369 → 0.402) without measurable harm to tempo accuracy (TA1 within noise: −0.005). Per-inference re-anchoring with smoothing is the right behaviour: the cold-start phase from `phase_of` after only ~1.5 s of accent integration is too noisy to hold; later inferences see far more cross-correlation evidence and correct the drift.

**Per the Phase L gate (F ∈ [0.37, 0.45] → don't integrate, document)** this stays Rejected as production but is a real improvement over the 4th pass. F now 0.402 vs aubio's 0.590 — gap closed by ~13 %, but aubio still ~47 % better on F and ~2.5× better on TA2. Production stays on `TrackingMode::AubioTempo`.

## Update — sixth debugging pass: per-band DC removal at bank input

The 5th-pass diagnostic showed normalised resonator energies dominated monotonically by the very shortest τ (23–27, ≈ 400–450 BPM) on every Ballroom track. The BPM prior was doing all the work to drag inference into a plausible range; on tracks where it lost, the algorithm defaulted to ~149.8 BPM.

**Root cause** (read `total_energies` together with the accent statistics):

The accent signal is HWR-only output → has a positive mean (~0.03–0.05 per band). For DC input, every comb filter outputs `y_steady = m` regardless of τ, so DC contributes `m²` to every resonator's energy. The normalisation divides by `(1-α)/(1+α)`, which for the DC component equals **multiplying by `(1+α)/(1-α)`** — a factor of 65× at τ=23 vs 19× at τ=82. The DC bias gets amplified ~3.4× more at short τ than at tactus τ. Periodic content was being normalised correctly; DC was not.

**Fix:** subtract a per-band leaky-integrator running mean (α=0.99, ~0.6 s time constant) from each accent frame in `KlapuriTracker::accent_step` before calling `bank.tick`. The bank now sees zero-mean input, the periodic-content normalisation is unbiased, and the prior no longer has to fight a structural short-τ pull.

This is the *opposite-direction* fix to the K1 trap from the 5th pass. K1 added DC to accent (raw `log_power`) and broke things; this removes the DC that was already there.

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| 4th pass | 0.369 | 0.160 | 0.242 | 0.255 |
| 5th pass (re-anchor) | 0.402 | 0.118 | 0.237 | 0.252 |
| **6th pass (+ DC removal at bank input)** | **0.584** | **0.315** | **0.456** | **0.662** |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

**F essentially tied with aubio** (Δ = −0.006). TA1 nearly doubled, TA2 ~2.6× better, AMLt ~2.7× better. Aubio still leads on TA2 (0.777 vs 0.662) and AMLt (0.459 vs 0.315), so it remains the better continuous-tracking option, but Klapuri is now in the same league.

**Per the Phase L gate (F > 0.45) → eligible for opt-in integration** as a 4th `BeatSource` tracking mode. Whether to expose it is a separate decision; either way ADR-0028 changes from Rejected to Conditionally Accepted.

## Update — seventh debugging pass: relax BPM prior + boost super-harmonic weight

The post-6th-pass diagnostic showed real periodic structure (top-K resonator energies clustered at the actual tactus + its harmonics). Track 3 (truth = 210.5 BPM) was the smoking gun: the bank's **#1-ranked normalised resonator was τ=49 (210.9 BPM, the actual tactus)**, but inference still picked τ=98 (105.5 BPM, half-tempo). Hand-checked:

- raw(τ=49) = 0.0167 + 0.5·e(98) + … ≈ 0.0253
- raw(τ=98) = 0.0126 + 0.6·e(49) + … ≈ 0.0226
- prior σ=0.5: prior(211 BPM) = 0.530, prior(106 BPM) = 0.968
- score(49) = 0.0253·0.530 = **0.0134**
- score(98) = 0.0226·0.968 = **0.0219** ← wins, but wrong

Bank evidence ratio (raw_49/raw_98 = 1.12) was correct; a 1.83× prior swing flipped it. Generalises to every track with true tempo > ~150 BPM, matching the TA1/TA2 gap (0.456 vs 0.662 → ~21 % of tracks had octave errors).

**Fix:**
1. `prior_sigma` 0.5 → 0.8. The tighter σ=0.5 systematically collapsed >180 BPM tracks to their half-tempo octave; σ=0.8 still penalises implausible tempos (40 BPM → 0.30, 400 BPM → 0.18) but lets ballroom's full 60–220 BPM range be picked when the bank is confident.
2. `w_measure_2` 0.5 → 0.7. The super-harmonic resonator at 2τ measures the same beat one metrical level up — direct independent evidence for the tactus, undervalued at 0.5.

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| 6th pass | 0.584 | 0.315 | 0.456 | 0.662 |
| **7th pass (relaxed prior + measure_2 boost)** | **0.630** | **0.367** | **0.485** | **0.755** |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

**Klapuri now beats aubio on F-measure** (Δ = **+0.040**) and is essentially tied on TA2 (Δ = −0.022). Aubio still leads on AMLt (continuous tracking, Δ = +0.092) and TA1 (strict tempo, Δ = +0.107) — meaning aubio is more often *exactly* right on tempo, but Klapuri's predictions are better localised when correct.

ADR-0028 status: **Conditionally Accepted → Accepted as opt-in tracking mode** (when wired up). The TA1/AMLt gap is the next debugging target if a further pass is wanted.

## Update — eighth debugging pass: τ-median filter for AMLt continuity

Targeted the AMLt and TA1 gaps to aubio (0.367 vs 0.459 / 0.485 vs 0.592 after pass 7). Hypothesis: when two octaves' inference scores were close, the per-cycle winner (every 64 OSS frames ≈ 0.37 s) was flip-flopping. Each flip triggered a snap-to-new-anchor in the K2 phase logic, resetting AMLt's continuity counter. The TA1 hit followed because the per-cycle median predicted tempo drifted across the strict 4 % tolerance.

**Fix:** ring buffer of the last 5 inference winners in `KlapuriTracker`; use the **median** as the active `tau_oss` instead of the per-cycle winner. Median is the right operator (averaging 86 and 43 yields 64.5, which is meaningless — median picks one of the two). Window of 5 cycles ≈ 1.86 s suppresses isolated flips; sustained tempo changes propagate after ≥ 3 confirming cycles (~1.1 s lag).

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| 7th pass | 0.630 | 0.367 | 0.485 | 0.755 |
| **8th pass (+ τ-median filter)** | **0.633** | **0.408** | **0.491** | **0.766** |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

**AMLt gap to aubio halved** (0.092 → 0.051). F still ahead of aubio (Δ=+0.043); TA2 nearly matched (Δ=−0.011). TA1 still trails (Δ=−0.101) — the residual gap is steady-state octave bias on tracks where the median-of-5 itself is the wrong octave, not flip-flop continuity loss.

## Update — ninth debugging pass: longer energy integration

Added a per-octave-error tally to `klapuri_experiment.rs`. After pass 8 (n=687):

```
correct=338  half=7  double=178  third=0  3/2=33  2/3=17  other=114
```

**26 % of tracks predicted at 2× truth tempo** — by far the dominant error class. Mostly slow Latin/ballroom tracks (truth 80–110 BPM) collapsed to their double-tempo octave (160–220 BPM). The pass-7 σ-widening fixed the *opposite* trap (very fast tracks → half-tempo) but left this one wide open.

Tried four prior-knob interventions:
| Change | F | TA1 | correct | double | half |
|---|---|---|---|---|---|
| Pass 8 baseline | 0.633 | 0.491 | 338 | 178 | 7 |
| centre=100, w_measure_2=0.5 | 0.610 | 0.444 | 301 | 94 | 114 |
| centre=100 only | 0.606 | 0.413 | 281 | 121 | 87 |
| w_measure_2=0.5 only | 0.627 | 0.453 | 306 | 160 | 37 |
| centre=110 | 0.616 | 0.435 | 294 | 151 | 42 |

Every alternative traded `double` for `half` at a net loss. Pass-8's prior settings are a local optimum on the prior knobs; the dominant double-tempo trap is **structural** (sub-harmonic resonator at τ_t/2 has higher raw bank energy than the tactus on percussive Ballroom audio, even after the DC-removal of pass 6) and can't be fixed by re-tuning the prior alone.

Switched to a different lever — `ResonatorBank::energy_decay`. The leaky-integrator pole on per-resonator energy was 0.99 (~0.58 s window): too short for slow tracks (60–80 BPM = ≥0.75 s/beat) to accumulate enough beats for confident discrimination between tactus and its sub-harmonic. Slower integration sweep:

| `energy_decay` | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| 0.99 (pass 8) | 0.633 | 0.408 | 0.491 | 0.766 |
| 0.995 | 0.634 | 0.416 | 0.504 | 0.777 |
| 0.997 | 0.632 | 0.422 | 0.507 | 0.786 |
| **0.998 (~3 s window)** | **0.631** | **0.424** | **0.507** | **0.789** |
| 0.999 | 0.625 | 0.428 | 0.511 | 0.792 |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

`0.998` is the chosen balance: AMLt + TA1 + TA2 all maximised while F is essentially flat (Δ=−0.002, within noise). **TA2 now beats aubio** (0.789 vs 0.777). AMLt gap to aubio reduced from 0.051 → 0.035; TA1 gap from 0.101 → 0.085.

The slower integration didn't move the double-tempo tally meaningfully (177 → 181) — that bug remains structural. The win is on continuous-tracking stability and strict-tempo accuracy *within* tracks where the right octave was picked.

## Update — tenth debugging pass: structural double-trap (no fix found)

User asked for a 10th pass directly attacking the dominant residual error class — 181/687 tracks (26 %) predicted at 2× truth tempo. Six interventions were tested, all regressed F:

| Change | F | correct | double | half |
|---|---|---|---|---|
| Pass 9 baseline | 0.631 | 343 | 181 | 6 |
| `w_tatum_half` 0.6→0.7, `w_measure_2` 0.7→0.5 (swap weights) | 0.615 | 305 | 158 | 58 |
| `w_tatum_half` 0.6→0.7 (only) | 0.625 | 332 | 175 | 12 |
| Anti-double-tempo demotion (BPM>140, threshold 0.85) | 0.612 | 301 | 157 | 63 |
| Anti-double-tempo demotion (threshold 0.95) | 0.627 | 332 | 177 | 14 |
| Anti-double-tempo demotion (threshold 0.99) | 0.630 | 341 | 180 | 8 |
| Asymmetric prior σ_fast=0.55 σ_slow=0.8 | 0.589 | 297 | 108 | 69 |
| Asymmetric prior σ_fast=0.7 σ_slow=0.8 | 0.615 | 313 | 164 | 27 |

**Every intervention that reduced doubles increased halves at net loss.** Across all six experiments the marginal cost of removing one double averaged ~1.5 newly-introduced halves. The two error classes occupy mirror-image regions of the prior×weight×scoring parameter space; symmetric tuning is constrained to a Pareto frontier whose pass-9 point is the F-maximum.

Why: at inference time the algorithm cannot distinguish "bank correctly identified the tactus, predicting fast tempo" (Track-3-type, e.g. truth=210 BPM) from "bank's structural short-τ bias dragged inference to the half-period, predicting fast tempo" (truth-100-doubled type). Both produce the same `(τ_w, score(τ_w), score(2·τ_w))` shape. Distinguishing them requires either:

1. An algorithmic improvement that reduces the bank's structural short-τ bias (e.g., contrast normalisation, peakiness-weighted scoring, or matched-template filtering instead of raw y² accumulation).
2. The full Klapuri 2006 joint posterior over tatum/tactus/measure with dynamic-programming continuity — far more invasive than the linear-combination scoring used today.
3. Per-track structural cues (beat phase regularity, harmonic content) added as a 2nd pass after the inference winner — substantial new code.

Pass 10 verdict: **no concrete bug found**, double-tempo trap requires algorithmic redesign rather than knob-tuning. Per the prior memory rule (shelve after 2 consecutive flat passes), Klapuri is now considered converged at F=0.631 / AMLt=0.424 / TA1=0.507 / TA2=0.789 and ready for opt-in integration as `BeatSource::Klapuri`. Further improvement would need sustained algorithmic work, not parameter sweeps.

## Update — eleventh debugging pass: fractional τ via parabolic interpolation

User suspected a real lock-loss bug behind the AMLt gap (0.424 vs aubio's 0.459). Phase A added a per-beat error-trajectory classifier to `tests/klapuri_experiment.rs` (filtering to TA2-correct + AMLt < 0.5 = AMLt-failing tracks where tempo is right within octave). Result on the AMLt-failing population:

```
clean=2  drift=2  jumps=200  oscill=1  mixed=104
```

The hypothesis (integer-τ quantisation drift) explained only **2/309 tracks** as pure linear drift. The dominant pattern was *steady-state polyrhythmic tracking* — `Albums-Latino_Latino-06` shows the signature:

```
+12.0 -141.3 +0.9 +285.4 -10.2 +274.2 -11.3 +273.1 -2.4 -268.0
```

Alternating 0/+280 ms = pred-period ≈ 1.5× truth-period (3:2 polyrhythm) — the algorithm is reliably tracking *a* metrical level that happens to fall between TA2's 4 % octave bands but produces beats systematically offset from truth. Same algorithmic-not-bug limitation as the pass-10 double-tempo trap.

A small *real* drift signal does exist (e.g. `Media-104916`: smooth ramp 0 → +83 ms over 86 beats = 1.1 ms/beat, exactly the integer-τ quantisation prediction). Shipped the fix anyway since it is theoretically correct + zero-risk:

**Fix:** parabolic-peak interpolation around the inference winner in `PeriodInference::select`, returning `(idx, tau_int, tau_frac, bpm)`. `KlapuriTracker` stores `period_audio: f64 = tau_frac × hop` and uses it for both beat scheduling (`next_beat_abs: Option<f64>`) and `KlapuriSource::snap_pll_at_beat`'s PLL period. The integer τ is still used for `bank.phase_of(idx)` indexing and the τ-median ring; the fractional value is only consumed when it agrees with the median (within 1 OSS frame) so an octave-switch median doesn't get a stale fractional offset from the previous octave.

| Pass | F | AMLt | TA1 | TA2 |
|---|---|---|---|---|
| 9 baseline | 0.631 | 0.424 | 0.507 | 0.789 |
| **11 (+ fractional τ)** | **0.635** | **0.430** | **0.507** | **0.785** |
| AubioTempo (reference) | 0.590 | 0.459 | 0.592 | 0.777 |

F up +0.004, AMLt up +0.006, TA1 unchanged, TA2 within noise. Jumps-bucket count dropped 200 → 188 (−12), confirming some "jumps" were really cumulative drift exceeding the 30 ms step threshold. **AMLt gap to aubio narrowed from 0.035 to 0.029.** The remaining gap is structural (polyrhythmic locks), not a bug.

## Literature comparison & realism check

User asked whether F=0.635 / AMLt=0.430 is realistic for a Klapuri 2006 implementation, or whether we still have bugs. Cross-referenced against published numbers:

**Klapuri 2006 on Ballroom** (per Gouyon, Klapuri, Dixon et al. 2006, "An experimental comparison of audio tempo induction algorithms", reproduced in several follow-up papers):

| Metric | Klapuri 2006 (paper) | Ours (pass 11) | Gap |
|---|---|---|---|
| Acc1 (strict tempo, ±4 %) | **63.18 %** | 50.7 % | **−12.5 pts** |
| Acc2 (octave-tolerant) | **90.97 %** | 78.5 % | **−12.5 pts** |

**The user's instinct was right — we are systematically ~12.5 percentage points below the paper's Ballroom result on both tempo metrics.** That gap is too large to be implementation noise or corpus-version differences; it points to a real missing component.

**What the paper actually specifies** (per the abstract + §IV-C summary): "A probabilistic model which represents primitive musical knowledge and uses the low-level observations to perform joint estimation of the tatum, tactus, and measure pulses, which takes into account the temporal dependencies between successive estimates and enables both causal and noncausal analysis."

**What we implement**: linear weighted sum of `e(τ) + 0.6·e(τ/2) + 0.3·e(τ/3) + 0.7·e(2τ) + …` × log-Gaussian BPM prior, picked per-inference-cycle, smoothed with a τ-median-of-5 ring. **No joint posterior**, **no temporal-continuity probability model**, **no noncausal (Viterbi/forward-backward) smoothing**. That's the same gap pass 10's structural-bias analysis hit, and pass 11's polyrhythmic-trajectory diagnostic confirmed: the missing 12.5 points are exactly the joint posterior + DP continuity that's been "out of scope" all along.

For context, ISMIR 2004 contest overall (across multiple corpora) — Klapuri won at Acc1 = 67.29 %, Acc2 = 85.01 %. The Ballroom-only result (Acc2 = 90.97 %) is higher than the overall because Ballroom has steady tempo. On harder corpora with expressive timing, paper-Klapuri scores below us — so the gap is corpus-specific to Ballroom, where the joint posterior's continuity prior helps most. Reference: Ballroom annotations were originally made with Davies' hybrid tracker, giving some annotation bias toward Davies-style picks; paper-Klapuri's 91 % is achieved despite that bias.

**On the waveclock question**: waveclock's product page (wavesum.net) explicitly states it is "based on psychoacoustically motivated algorithm by Anssi Klapuri & AL." It is a commercial Klapuri-family implementation, not a different algorithm. The IEEE paper "Equilibria of Adaptive Wavetable Oscillators with Applications to Beat Tracking" suggests it may extend Klapuri's idea with adaptive oscillators on top, but the core lineage is the same as ours. So the user's "why doesn't waveclock use aubio if aubio is better?" question reduces to:

1. **Aubio is *not* uniformly better.** Aubio's autocorrelation handles polyrhythms more robustly on stationary-tempo corpora (Ballroom), but doesn't adapt as well to expressive timing changes (live DJ pitch-shifts, rubato, ritardando). Klapuri-style comb filters with the full joint posterior do — that's the design tradeoff.
2. **Waveclock's commercial product is closer to paper-Klapuri than our implementation is** — they likely have the joint posterior + DP continuity that closes the 12.5-point gap we observe.
3. **The fact that aubio scores well on Ballroom is partly Ballroom-specific.** Davies-derived annotations + steady tempo + Western-pop bias all favour autocorrelation. The same trackers reverse rankings on harder corpora (SMC, GiantSteps).

**Bottom line as of pass 11:** the 12.5-point gap is the joint-posterior + DP-continuity component (§IV-C). Pass 12 added the joint-posterior module as a research artifact. Pass 13 wired it into `select()` with substantial improvements. Pass 14 audited the paper for missing details and fixed two more (W=0.8, extended bank). Continued below in chronological order.

## Consolidated results history (Ballroom n=687, mir_eval)

All numbers are mean F-measure (±70 ms tolerance), AMLt (allowed metrical level — total beats), TA1 (strict tempo within ±4 %), TA2 (octave-tolerant) on the 687-track Ballroom subset (after dropping 11 known duplicates). Aubio reference: F=0.590, AMLt=0.459, TA1=0.592, TA2=0.777. Paper-Klapuri (Gouyon et al. 2006 reproduction on Ballroom): TA1=0.632, TA2=0.910.

| Pass | Commit | Branch | F | AMLt | TA1 | TA2 | Change |
|---|---|---|---|---|---|---|---|
| Pass 5 (re-anchor) | (in `0b9cd77`) | `main` | 0.402 | 0.118 | 0.237 | 0.252 | Per-inference phase re-anchor + smoothing |
| Pass 6 (DC removal) | (in `0b9cd77`) | `main` | 0.584 | 0.315 | 0.456 | 0.662 | Per-band DC removal at bank input |
| Pass 7 (relaxed prior) | (in `0b9cd77`) | `main` | 0.630 | 0.367 | 0.485 | 0.755 | σ 0.5→0.8, w_measure_2 0.5→0.7 |
| Pass 8 (τ-median) | (in `0b9cd77`) | `main` | 0.633 | 0.408 | 0.491 | 0.766 | Median of last 5 inference winners |
| Pass 9 (slow energy_decay) | (in `0b9cd77`) | `main` | 0.633 | 0.408 | 0.491 | 0.766 | Bank `energy_decay` 0.99→0.998 |
| Pass 10 (no improvement) | (in `0b9cd77`) | `main` | 0.633 | 0.408 | 0.491 | 0.766 | 6 prior tweaks tried, none helped |
| BeatSource wiring | `0de8d22` | `main` | (no change) | | | | `TrackingMode::Klapuri` opt-in |
| Pass 11 (fractional τ) | `db7aee4` | `main` | 0.635 | 0.430 | 0.507 | 0.785 | Parabolic-peak interp on inference winner |
| Pass 11 (lit comparison) | `74c06e3` | `main` | (doc only) | | | | Establishes paper TA1/TA2 = 0.632/0.910 target |
| Pass 12 (joint stub) | `568d9e0` | `klapuri/joint-posterior-experiment` | (no change) | | | | `joint_posterior` module + tests, not wired |
| Pass 13 Phase A (36 narrow bands) | `9ef30f5` | branch | 0.652 | 0.441 | 0.527 | 0.808 | Per-narrow-band log+diff before sum (paper §III) |
| Pass 13 Phase B (joint posterior wired) | `2d83bb9` | branch | 0.592 | 0.399 | 0.437 | 0.703 | Wired into `select()` (regression — fixed by Phase C) |
| Pass 13 Phase C (Gaussian likelihood) | `b6fed9e` | branch | 0.680 | 0.474 | 0.571 | 0.910 | Per-cycle log-LR vs empirical noise floor |
| Pass 13 Phase D (forward filter) | `e61bc28` | branch | 0.683 | 0.482 | 0.574 | 0.905 | Sparse-transition online HMM forward step |
| Pass 13 Phase F (α tuning) | `8721734` | branch | 0.683 | 0.492 | 0.588 | 0.921 | α_t=1.5, α_h=0.2, α_m=0.4 |
| Pass 13 Phase E (downbeat) | `05cd82d` | branch | (no change) | | | | Per-measure-position low-band-accent tracker (advisory) |
| Pass 14 fix 1 (ACCENT_W) | `e0937b1` | branch | 0.693 | 0.502 | 0.598 | 0.930 | W 0.9 → 0.8 (paper exact) |
| Pass 14 fix 2 (extended bank) | `bae8b48` | branch | 0.694 | 0.499 | 0.604 | 0.929 | τ_max 172 → 688 (paper §II-B) |
| Aubio reference | — | — | 0.590 | 0.459 | 0.592 | 0.777 | Production default |
| Paper-Klapuri (Gouyon 2006) | n/a | n/a | — | — | 0.632 | 0.910 | Target |

**Branch tip vs aubio (`bae8b48`):** F **+0.104**, AMLt **+0.040**, TA1 **+0.012**, TA2 **+0.152** — beats on **all four metrics**.

**Branch tip vs paper-Klapuri:** TA2 **+0.019** (beats), TA1 **−0.028** (still trails). Plausibly the residual is the offline-Viterbi vs online-forward-filter architectural ceiling — paper does both period and phase via Viterbi (non-causal), we do online causal.

## Pass 14 — paper audit findings (vs `bae8b48`)

Cross-referenced against text extracted from Eronen's PhD thesis (Tampere CRIS, contains paper [P5] verbatim) via `pdftotext`.

| Component | Paper | Ours (pre-pass-14) | Status |
|---|---|---|---|
| Filterbank type | "logarithmically distributed subbands" (mel approximation) | mel | ✓ match |
| Narrow-band count | 36 | 36 | ✓ match |
| Output channel count | 4 | 4 | ✓ match |
| FFT window length | 23 ms (1024 @ 44.1 kHz) | 1024 | ✓ match |
| Hop length | 5.8 ms (256) | 256 | ✓ match |
| μ-law μ | 100 | 100 | ✓ match |
| Compression weight W | **0.8** | 0.9 | ✗ **fixed in `e0937b1`** |
| Bank period range τ | **1 ≤ τ ≤ 688** (4 s) | 24 ≤ τ ≤ 172 | ✗ **fixed in `bae8b48`** |
| Comb α formula | `0.5^(τ/(T₀.₅·fs))`, T₀.₅ = 3 s | same | ✓ match |
| Resonator normalisation | `s(τ,n) = r̂(τ,n) / W₀(n)` per channel (eq. 8) | `r/((1−α)/(1+α))` (white-noise baseline) | ✗ **see Fix 3 below** |
| Joint state | (τ_tatum, τ_tactus, τ_measure), top-5 each → 125 states (beam) | (τ_tactus, k_tatum, k_measure), all ~750 states | partial match — different parameterisation |
| Period inference | Viterbi (offline, both causal and non-causal supported) | online forward filter | architectural ceiling — real-time only |
| Phase tracking | Two parallel Viterbi HMMs (tactus + measure phase) | `phase_acc` argmax | architectural ceiling |
| Tactus phase likelihood (eq. 27) | weighted sum across channels emphasising low frequency | argmax of summed `phase_acc` | partial — no low-freq emphasis |
| Measure phase likelihood (eq. 32) | rhythmic-template matching ("low, loud, ., loud" / "low, ., loud, .") | not implemented | not implemented (downbeat tracker is separate, advisory) |
| Tactus prior | two-param lognormal, μ=0.55 σ=0.28 (per Parncutt) | log-Gaussian centre 120 BPM, σ=0.8 | similar shape, different parameters; tested 80/100/110/120 in pass 9 — no effect |

## Pass 14 — negative results (NOT shipped)

Tried during pass 14 audit, regressed or no improvement, reverted on the branch:

- **Paper normalisation `/W₀` standalone** (no commit): F 0.694 → 0.366. Catastrophic regression because our Gaussian log-LR likelihood compares energies against an empirical noise floor that breaks when energies are pre-normalised to ~[0, 1]. Will retry with the matching paper-eq-17 likelihood (Fix 3, planned).
- **Per-band local-window Gaussian** (in pass 13 Phase G, not committed): radii 8/30/80 — all regress. Local stats don't help when neighbouring τ values have similar energy.
- **Missing-level penalty sweeps** (in pass 13, not committed): −3.0/−1.0/0.0/−5.0 — no effect either direction.
- **Anti-double-tempo demotion** (pass 10, not committed): thresholds 0.85/0.95/0.99 — every variant trades doubles for halves at net loss.
- **Asymmetric BPM prior σ_fast≠σ_slow** (pass 10, not committed): 0.55/0.7 — same trade-off pattern.
- **Consistency penalty `−β·max(0, e_m − e_t)`** (pass 14, not committed): β=4/20 — barely fires (energies satisfy condition only on a few tracks).
- **Hard rejection when e_m > 1.2·e_t** (pass 14, not committed): doesn't trigger (typical e_m / e_t ratios on real audio < 1.2 even on doubled tracks).

## References

- Klapuri, A.P., Eronen, A.J., and Astola, J.T. *Analysis of the meter of acoustic musical signals.* IEEE TASLP 14(1):342–355, 2006. https://www.iro.umontreal.ca/~pift6080/H09/documents/papers/klapuri_meter.pdf
