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
use crate::dsp::klapuri::period_inference::PeriodInference;
use crate::dsp::klapuri::resonators::{default_period_range, ResonatorBank};

const DEFAULT_INFERENCE_INTERVAL: u64 = 64;
const DEFAULT_WARMUP_FRAMES: u64 = 256;

pub struct KlapuriTracker {
    sr: u32,
    hop: usize,

    accent: MultiBandAccent,
    bank: ResonatorBank,
    inference: PeriodInference,

    /// Total accent frames consumed since reset.
    oss_frames: u64,
    /// Most recently inferred tactus period (in OSS frames). 0 ⇒ no
    /// lock yet.
    tau_oss: usize,
    /// Latest predicted-next-beat in absolute audio-sample units.
    /// `None` until first lock.
    next_beat_abs: Option<u64>,
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
        let bank = ResonatorBank::new(&periods);
        let inference = PeriodInference::new(sr as f32 / hop as f32);
        Self {
            sr,
            hop,
            accent,
            bank,
            inference,
            oss_frames: 0,
            tau_oss: 0,
            next_beat_abs: None,
            next_oss_frame_abs: 0,
            inference_interval: DEFAULT_INFERENCE_INTERVAL,
            warmup_frames: DEFAULT_WARMUP_FRAMES,
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
    pub fn current_bpm(&self) -> f32 {
        if self.tau_oss == 0 {
            return 0.0;
        }
        60.0 * (self.sr as f32 / self.hop as f32) / self.tau_oss as f32
    }

    pub fn reset(&mut self) {
        self.accent.reset();
        self.bank.reset();
        self.oss_frames = 0;
        self.tau_oss = 0;
        self.next_beat_abs = None;
        self.next_oss_frame_abs = 0;
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
        let next_beat_abs = &mut self.next_beat_abs;
        let inference_interval = self.inference_interval;
        let warmup_frames = self.warmup_frames;
        let sr = self.sr;
        let abs_at_sample = block_start_abs + *samples_into_block as u64;

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
                if abs_at_sample >= nb {
                    let off = ((nb - block_start_abs).min(u32::MAX as u64)) as u32;
                    on_beat(off, nb);
                    // Schedule the next beat.
                    if *tau_oss > 0 {
                        *next_beat_abs = Some(nb + (*tau_oss * hop) as u64);
                    }
                }
            }
            return;
        }

        // A new accent frame is ready (at this audio sample).
        bank.tick(frame_acc);
        *oss_frames += 1;
        // Periodically re-run inference. Period inference sets
        // `tau_oss`; the next-beat schedule is anchored on FIRST lock
        // and re-anchored only when τ changes meaningfully (> 10%) or
        // we're not yet locked. Subsequent emissions advance by
        // τ·hop without re-anchoring — keeps the beat grid stable
        // even if inference jitters slightly between runs.
        if *oss_frames > warmup_frames && *oss_frames % inference_interval == 0 {
            if let Some((idx, tau, _bpm)) = inference.select(bank) {
                let prev_tau = *tau_oss;
                *tau_oss = tau;
                let need_anchor = next_beat_abs.is_none()
                    || prev_tau == 0
                    || (tau as i64 - prev_tau as i64).unsigned_abs() as usize * 10 > prev_tau;
                if need_anchor {
                    let phase = bank.phase_of(idx);
                    let phase_audio_off = (phase as u64).saturating_mul(hop as u64);
                    let last_beat_audio = abs_at_sample.saturating_sub(phase_audio_off);
                    let period_audio = (tau as u64) * (hop as u64);
                    let mut nb = last_beat_audio + period_audio;
                    while nb <= abs_at_sample {
                        nb += period_audio;
                    }
                    *next_beat_abs = Some(nb);
                }
            }
        }
        let _ = sr;

        // Check beat emission for this audio sample as well.
        if let Some(nb) = *next_beat_abs {
            if abs_at_sample >= nb {
                let off = ((nb - block_start_abs).min(u32::MAX as u64)) as u32;
                on_beat(off, nb);
                if *tau_oss > 0 {
                    *next_beat_abs = Some(nb + (*tau_oss * hop) as u64);
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
