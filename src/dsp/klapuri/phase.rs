// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Phase tracker + public `KlapuriTracker` — Klapuri 2006 §IV-D.
//!
//! Predictive beat emission. Periodically (every
//! `inference_interval` accent frames) run period inference + read
//! the winning resonator's phase. From `(τ, phase)` project forward:
//! the next beat sample is `(current_oss_frame − phase + τ) · hop_size`.
//! Subsequent beats are at multiples of `τ · hop_size`.

use crate::dsp::klapuri::accent::{MultiBandAccent, FFT_SIZE, HOP_SIZE, N_BANDS};
use crate::dsp::klapuri::downbeat::DownbeatTracker;
use crate::dsp::klapuri::period_inference::PeriodInference;
use crate::dsp::klapuri::resonators::{default_period_range, ResonatorBank};

const DEFAULT_INFERENCE_INTERVAL: u64 = 64;
const DEFAULT_WARMUP_FRAMES: u64 = 256;

/// Pole of the per-band accent-mean tracker. 0.99 at 172 Hz OSS ≈ 0.6 s
/// time constant — slow enough to leave the tempo periodicities
/// (typically ≥ 0.25 s) intact, fast enough to track gradual loudness
/// drift across a track.
const ACCENT_DC_ALPHA: f32 = 0.99;

/// Window size (in inference cycles) of the τ-median filter. At
/// `inference_interval = 64` OSS frames and 172 Hz OSS rate, 5 cycles
/// span ~1.86 s — long enough to suppress isolated octave flips,
/// short enough to track real tempo changes after ~0.7 s.
const TAU_HISTORY: usize = 5;

pub struct KlapuriTracker {
    sr: u32,
    hop: usize,

    accent: MultiBandAccent,
    bank: ResonatorBank,
    inference: PeriodInference,

    /// Downbeat tracker — observes the low-band accent at each
    /// emitted beat moment to identify the downbeat (beat-1-of-
    /// measure) phase. Independent of the joint posterior; runs at
    /// per-beat rather than per-inference granularity.
    downbeat: DownbeatTracker,
    /// Most-recent low-band accent value (band 0). Read by the
    /// downbeat tracker when a beat fires. Updated each accent
    /// frame.
    last_low_band_accent: f32,

    /// Per-band leaky-integrator running mean of accent, subtracted
    /// from each accent frame before feeding the bank. Removes the
    /// positive-mean DC component of HWR-only accent that would
    /// otherwise bias short-τ resonators after the
    /// `(1-α)/(1+α)` normalisation in `total_energies`.
    accent_dc: [f32; N_BANDS],

    /// Ring buffer of the last `TAU_HISTORY` inference winners. The
    /// median of this buffer is used as `tau_oss` instead of the raw
    /// per-cycle winner — single-cycle octave flip-flops between τ_t
    /// and 2τ_t (which break AMLt continuity and push TA1 off the
    /// strict 4 % tolerance) get rejected; sustained tempo changes
    /// still propagate after ≥ ⌈N/2⌉ confirming cycles.
    tau_history: [usize; TAU_HISTORY],
    /// Number of valid entries in `tau_history` (saturates at the
    /// buffer length).
    tau_history_count: usize,
    /// Write position in `tau_history`.
    tau_history_pos: usize,

    /// Total accent frames consumed since reset.
    oss_frames: u64,
    /// Most recently inferred tactus period (in OSS frames, integer).
    /// 0 ⇒ no lock yet. Used for `bank.phase_of(idx)` indexing and as
    /// the input to the τ-median filter.
    tau_oss: usize,
    /// Latest tactus period in **audio samples** as f64. Sub-frame
    /// resolution from parabolic-peak interpolation around the
    /// inference winner — prevents integer-τ quantisation drift
    /// (~0.5–1 ms/beat) which accumulates over ~30 s into AMLt
    /// failure on tracks whose true tempo falls between integer τ
    /// values. 0.0 ⇒ no lock yet.
    period_audio: f64,
    /// Latest predicted-next-beat in absolute audio-sample units,
    /// f64 for fractional accumulation. `None` until first lock.
    next_beat_abs: Option<f64>,
    /// Absolute sample of the start of the next OSS frame (= where
    /// the current pending hop will land in host time).
    next_oss_frame_abs: u64,
    /// How often to rerun period inference (every N accent frames).
    inference_interval: u64,
    /// Hold off on emitting beats until this many accent frames
    /// have been processed (warmup).
    warmup_frames: u64,
}

// SAFETY: like other DSP wrappers, only the audio thread touches it.
unsafe impl Send for KlapuriTracker {}

impl KlapuriTracker {
    pub fn new(sr: u32) -> Self {
        let hop = HOP_SIZE;
        let accent = MultiBandAccent::with_size(sr, FFT_SIZE, hop);
        let periods = default_period_range(sr, hop);
        let bank = ResonatorBank::new(&periods, sr as f32 / hop as f32);
        let inference = PeriodInference::new(sr as f32 / hop as f32);
        let downbeat = DownbeatTracker::new();
        Self {
            sr,
            hop,
            accent,
            bank,
            inference,
            oss_frames: 0,
            tau_oss: 0,
            period_audio: 0.0,
            next_beat_abs: None,
            next_oss_frame_abs: 0,
            inference_interval: DEFAULT_INFERENCE_INTERVAL,
            warmup_frames: DEFAULT_WARMUP_FRAMES,
            accent_dc: [0.0; N_BANDS],
            tau_history: [0; TAU_HISTORY],
            tau_history_count: 0,
            tau_history_pos: 0,
            downbeat,
            last_low_band_accent: 0.0,
        }
    }

    pub fn sr(&self) -> u32 {
        self.sr
    }
    pub fn hop(&self) -> usize {
        self.hop
    }
    pub fn locked(&self) -> bool {
        self.next_beat_abs.is_some()
    }
    pub fn current_period_oss(&self) -> usize {
        self.tau_oss
    }
    /// Locked tactus period in audio samples, with sub-frame
    /// (fractional-τ) resolution. 0.0 until the tracker locks.
    pub fn current_period_audio(&self) -> f64 {
        self.period_audio
    }
    pub fn current_bpm(&self) -> f32 {
        if self.period_audio <= 0.0 {
            return 0.0;
        }
        60.0 * self.sr as f32 / self.period_audio as f32
    }

    /// Most-likely downbeat phase (beat-1-of-measure offset within a
    /// k-beat measure) for the given measure size. `None` until the
    /// downbeat tracker has accumulated enough beats. Per Klapuri
    /// 2006 §V — currently advisory; not yet wired into the joint
    /// posterior.
    pub fn downbeat_position(&self, k_measure: u8) -> Option<usize> {
        self.downbeat.downbeat_position(k_measure)
    }

    /// Confidence (0..1) of the downbeat estimate at the given
    /// `k_measure`. Higher = more concentrated per-position energy.
    pub fn downbeat_confidence(&self, k_measure: u8) -> f32 {
        self.downbeat.confidence(k_measure)
    }

    pub fn reset(&mut self) {
        self.accent.reset();
        self.bank.reset();
        self.inference.reset();
        self.downbeat.reset();
        self.oss_frames = 0;
        self.tau_oss = 0;
        self.period_audio = 0.0;
        self.next_beat_abs = None;
        self.next_oss_frame_abs = 0;
        self.accent_dc = [0.0; N_BANDS];
        self.tau_history = [0; TAU_HISTORY];
        self.tau_history_count = 0;
        self.tau_history_pos = 0;
        self.last_low_band_accent = 0.0;
    }

    /// Process one host block of mono audio. `block_start_abs` is the
    /// absolute sample of the first sample in the block. For each
    /// predicted beat that lands within this block, calls
    /// `on_beat(offset_in_block, abs_sample)`.
    pub fn process_block<F: FnMut(u32, u64)>(
        &mut self,
        audio: &[f32],
        block_start_abs: u64,
        mut on_beat: F,
    ) {
        // Track the host-time position of the next-completed accent
        // frame as we walk the block. Each completed frame consumes
        // `hop` audio samples; its host-time end-sample is wherever
        // we are in the block.
        let mut samples_into_block: usize = 0;
        let _hop = self.hop;

        // We need samples_consumed_since_last_frame to know when an
        // accent frame fires. MultiBandAccent's process_block fires
        // a callback at each hop boundary. We mirror the same loop
        // here instead of nesting state into the accent module.
        for &s in audio {
            self.accent_step(s, &mut samples_into_block, block_start_abs, &mut on_beat);
            samples_into_block += 1;
        }
    }

    /// Push one audio sample. If this completes a hop, fire the
    /// accent → bank → (periodic inference) → beat-emission cycle.
    fn accent_step<F: FnMut(u32, u64)>(
        &mut self,
        s: f32,
        samples_into_block: &mut usize,
        block_start_abs: u64,
        on_beat: &mut F,
    ) {
        // Feed one sample to the accent module. The accent module
        // invokes its frame callback when a hop completes.
        let bank = &mut self.bank;
        let inference = &mut self.inference;
        let hop = self.hop;
        let oss_frames = &mut self.oss_frames;
        let tau_oss = &mut self.tau_oss;
        let period_audio_state = &mut self.period_audio;
        let next_beat_abs = &mut self.next_beat_abs;
        let inference_interval = self.inference_interval;
        let warmup_frames = self.warmup_frames;
        let sr = self.sr;
        let abs_at_sample = block_start_abs + *samples_into_block as u64;
        let abs_at_sample_f = abs_at_sample as f64;

        let mut frame_fired = false;
        let mut frame_acc = [0.0f32; N_BANDS];
        self.accent.process_block(&[s], |frame| {
            // process_block fires at most once for a 1-sample input.
            frame_fired = true;
            frame_acc = frame;
        });
        if !frame_fired {
            // Within-hop: just check whether a predicted beat lands
            // at this audio sample.
            if let Some(nb) = *next_beat_abs {
                if abs_at_sample_f >= nb {
                    let nb_u = nb.round() as u64;
                    let off = (nb_u.saturating_sub(block_start_abs).min(u32::MAX as u64)) as u32;
                    // Feed the downbeat tracker with the most recent
                    // low-band accent value — kicks dominate the low
                    // band, downbeats usually have the loudest kick.
                    self.downbeat.observe_beat(self.last_low_band_accent);
                    on_beat(off, nb_u);
                    // Schedule the next beat.
                    if *period_audio_state > 0.0 {
                        *next_beat_abs = Some(nb + *period_audio_state);
                    }
                }
            }
            return;
        }

        // A new accent frame is ready (at this audio sample). Subtract
        // the per-band running mean before feeding the bank — HWR-only
        // accent has a positive DC component that, after the bank's
        // (1-α)/(1+α) normalisation, biases short-τ resonators by
        // ((1+α)/(1-α)), a ≥ 3× factor in our period range.
        let mut zero_mean = [0.0f32; N_BANDS];
        for b in 0..N_BANDS {
            self.accent_dc[b] =
                ACCENT_DC_ALPHA * self.accent_dc[b] + (1.0 - ACCENT_DC_ALPHA) * frame_acc[b];
            zero_mean[b] = frame_acc[b] - self.accent_dc[b];
        }
        // Cache the low-band accent for the downbeat tracker (consumed
        // when a beat fires within this hop, below).
        self.last_low_band_accent = frame_acc[0];
        bank.tick(zero_mean);
        *oss_frames += 1;
        // Periodically re-run inference. Period inference sets
        // `tau_oss`; the next-beat schedule is re-anchored on EVERY
        // inference cycle from the current resonator phase (each call
        // integrates more cross-correlation evidence than the cold
        // start). Small phase corrections are smoothed by averaging
        // halfway between the prior prediction and the new one;
        // half-period jumps snap to the new anchor (octave switch).
        if *oss_frames > warmup_frames && *oss_frames % inference_interval == 0 {
            if let Some((_raw_idx, raw_tau, raw_tau_frac, _raw_bpm)) = inference.select(bank) {
                // Push the per-cycle winner into the τ-median ring.
                self.tau_history[self.tau_history_pos] = raw_tau;
                self.tau_history_pos = (self.tau_history_pos + 1) % TAU_HISTORY;
                if self.tau_history_count < TAU_HISTORY {
                    self.tau_history_count += 1;
                }
                // Median over valid entries — robust to single-cycle
                // octave flip-flops.
                let n = self.tau_history_count;
                let mut buf = [0usize; TAU_HISTORY];
                buf[..n].copy_from_slice(&self.tau_history[..n]);
                buf[..n].sort_unstable();
                let tau = buf[n / 2];
                let idx = bank.periods().binary_search(&tau).unwrap_or(_raw_idx);
                *tau_oss = tau;
                // Use the parabolic-interpolated fractional τ for
                // period when it agrees with the median (within one
                // OSS frame), else fall back to the median integer τ
                // — the fractional value is only meaningful when it
                // describes the same octave the median selected.
                let period_oss_f = if (raw_tau_frac - tau as f32).abs() < 1.0 {
                    raw_tau_frac as f64
                } else {
                    tau as f64
                };
                *period_audio_state = period_oss_f * hop as f64;
                let phase = bank.phase_of(idx);
                let phase_audio_off = phase as f64 * hop as f64;
                let last_beat_audio = abs_at_sample_f - phase_audio_off;
                let mut nb = last_beat_audio + *period_audio_state;
                while nb <= abs_at_sample_f {
                    nb += *period_audio_state;
                }
                if let Some(prev_nb) = *next_beat_abs {
                    let diff_abs = (prev_nb - nb).abs();
                    if diff_abs > 0.5 * *period_audio_state {
                        // Big jump (octave switch) — accept new anchor.
                        *next_beat_abs = Some(nb);
                    } else {
                        // Small adjustment — nudge halfway to avoid
                        // sudden phase jumps on stable tracks.
                        *next_beat_abs = Some(0.5 * (prev_nb + nb));
                    }
                } else {
                    *next_beat_abs = Some(nb);
                }
            }
        }
        let _ = sr;

        // Check beat emission for this audio sample as well.
        if let Some(nb) = *next_beat_abs {
            if abs_at_sample_f >= nb {
                let nb_u = nb.round() as u64;
                let off = (nb_u.saturating_sub(block_start_abs).min(u32::MAX as u64)) as u32;
                self.downbeat.observe_beat(self.last_low_band_accent);
                on_beat(off, nb_u);
                if *period_audio_state > 0.0 {
                    *next_beat_abs = Some(nb + *period_audio_state);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_kick(sr: u32, n_samples: usize, beat_period_samples: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; n_samples];
        let click_len = (0.050 * sr as f32) as usize;
        let decay_tau = 0.020 * sr as f32;
        let mut t = 0;
        while t + click_len < n_samples {
            for i in 0..click_len {
                let phase = 2.0 * std::f32::consts::PI * 60.0 * (i as f32) / sr as f32;
                let env = (-(i as f32) / decay_tau).exp();
                buf[t + i] += 0.8 * env * phase.sin();
            }
            t += beat_period_samples;
        }
        buf
    }

    /// 120 BPM kick track → KlapuriTracker locks and emits beats at
    /// roughly the kick positions.
    #[test]
    fn klapuri_tracks_120bpm_kick() {
        let sr = 44_100u32;
        let beat = (sr as f32 * 60.0 / 120.0) as usize; // 22050
        let n = beat * 60 + 1024;
        let audio = synth_kick(sr, n, beat);
        let mut tracker = KlapuriTracker::new(sr);
        let mut beats: Vec<u64> = Vec::new();
        let block = 512;
        let mut absolute = 0u64;
        for chunk in audio.chunks(block) {
            tracker.process_block(chunk, absolute, |_off, abs| {
                beats.push(abs);
            });
            absolute += chunk.len() as u64;
        }
        assert!(
            tracker.locked(),
            "tracker should lock on a 30 s 120 BPM kick track"
        );
        let bpm = tracker.current_bpm();
        eprintln!(
            "klapuri 120 BPM kick: locked BPM={bpm:.1}, emitted {} beats over 30 s",
            beats.len()
        );
        // BPM should match 120 within ±15 % at the tactus or a
        // metrical level (½, 1, 2, 3, 4). Phase 5's full Ballroom
        // benchmark is the real evaluation; this unit test is just a
        // smoke check that the pipeline produces a locked tempo in
        // the right ballpark.
        let candidates = [120.0_f32, 60.0, 240.0, 360.0, 480.0, 40.0, 30.0];
        let ok_bpm = candidates.iter().any(|c| (bpm - c).abs() < c * 0.15);
        assert!(
            ok_bpm,
            "expected 120 BPM (or metrical level ±15%), got {bpm:.1}"
        );
        // Beat count should be consistent with the locked BPM: roughly
        // (bpm / 60) × 30 s ± 10 (allowing for warm-up).
        let expected_beats = (bpm / 60.0 * 30.0) as usize;
        assert!(
            (beats.len() as i64 - expected_beats as i64).abs() <= 15,
            "beat count {} inconsistent with locked BPM={bpm:.1} (expected ~{expected_beats})",
            beats.len()
        );
    }

    // ------------------------------------------------------------------
    // Phase A4 — end-to-end synthetic regression added during the
    // Klapuri debugging pass (plan §A4). Currently FAILS (the phase
    // bug dominates); should pass after the fixes land.
    // ------------------------------------------------------------------

    /// Mean absolute error between predicted and nearest-truth beat
    /// times should be below the F-measure tolerance after warmup.
    /// Without this guarantee the F-measure on Ballroom can't break
    /// out of the 0.3 floor.
    #[test]
    fn klapuri_emits_within_70ms_of_truth_on_clean_120bpm_kick() {
        let sr = 44_100u32;
        let beat = (sr as f32 * 60.0 / 120.0) as usize; // 22050
        let n = beat * 60 + 1024; // ~30 s
        let audio = synth_kick(sr, n, beat);
        let mut tracker = KlapuriTracker::new(sr);
        let mut predicted: Vec<u64> = Vec::new();
        let block = 512;
        let mut absolute = 0u64;
        for chunk in audio.chunks(block) {
            tracker.process_block(chunk, absolute, |_off, abs| {
                predicted.push(abs);
            });
            absolute += chunk.len() as u64;
        }
        // Truth beat times: every `beat` samples starting at t=0.
        let truth: Vec<u64> = (0..)
            .map(|i| (i * beat) as u64)
            .take_while(|&t| t < n as u64)
            .collect();
        // Drop the first 5 s of predictions (warmup).
        let warmup = (5.0 * sr as f64) as u64;
        let post: Vec<u64> = predicted.iter().copied().filter(|&p| p > warmup).collect();
        assert!(
            post.len() >= 20,
            "expected ≥ 20 post-warmup beats, got {} (total predicted: {})",
            post.len(),
            predicted.len()
        );
        let total_err: f64 = post
            .iter()
            .map(|&p| {
                let nearest = truth
                    .iter()
                    .min_by_key(|&&t| (t as i64 - p as i64).unsigned_abs())
                    .copied()
                    .unwrap_or(0);
                ((p as i64 - nearest as i64).unsigned_abs() as f64) / sr as f64
            })
            .sum();
        let mean_err_s = total_err / post.len() as f64;
        let mean_err_ms = mean_err_s * 1000.0;
        assert!(
            mean_err_ms < 35.0,
            "mean abs error {mean_err_ms:.1} ms > 35 ms — phase tracking is broken \
             (predicted post-warmup: {} beats over 30 s)",
            post.len()
        );
    }

    /// After 30 s of clean 120 BPM kicks, the locked BPM should be
    /// within ±4 % of 60 / 120 / 240. Tighter than the existing test
    /// (±15 %) — a regression check on the period inference.
    #[test]
    fn klapuri_locks_to_correct_octave_on_120bpm() {
        let sr = 44_100u32;
        let beat = (sr as f32 * 60.0 / 120.0) as usize;
        let n = beat * 60 + 1024;
        let audio = synth_kick(sr, n, beat);
        let mut tracker = KlapuriTracker::new(sr);
        let block = 512;
        let mut absolute = 0u64;
        for chunk in audio.chunks(block) {
            tracker.process_block(chunk, absolute, |_, _| {});
            absolute += chunk.len() as u64;
        }
        let bpm = tracker.current_bpm();
        let candidates = [120.0_f32, 60.0, 240.0];
        let ok = candidates.iter().any(|c| (bpm - c).abs() < c * 0.04);
        assert!(ok, "expected 120 BPM (or 60 / 240) ±4 %, got {bpm:.1}");
    }
}
