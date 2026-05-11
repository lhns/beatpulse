// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Klapuri diagnostic — read-only. Runs on 5 Ballroom tracks and
//! dumps intermediate values from each pipeline stage to track down
//! the real-audio failure mode. Plan §D.
//!
//! Re-implements the accent pipeline inline (rather than instrumenting
//! `src/dsp/klapuri/accent.rs` with debug hooks that would have to live
//! in production code). Stays in sync with the real impl by reading
//! the same `MultiBandAccent`'s constants (FFT_SIZE / HOP_SIZE /
//! N_BANDS).
//!
//! Run with:
//! `BEATPULSE_BALLROOM_DIR=... cargo test --release --features dataset-tests --test klapuri_diagnose -- --nocapture`

#![cfg(feature = "dataset-tests")]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use beatpulse::dsp::klapuri::accent::N_BANDS;
use beatpulse::dsp::klapuri::period_inference::PeriodInference;
use beatpulse::dsp::klapuri::resonators::{default_period_range, ResonatorBank};
use beatpulse::dsp::klapuri::KlapuriTracker;
use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

mod common;
use common::audio::decode_mono_44k1;

const TARGET_SR: u32 = 44_100;
const FFT_SIZE: usize = 1024;
const HOP_SIZE: usize = 256;
const LOG_MU: f32 = 100.0;
const DC_ALPHA: f32 = 0.97;
const ACCENT_W: f32 = 0.9;

/// Mirror of `src/dsp/klapuri/accent.rs::mel_band_bins`.
fn mel_band_bins(sr: u32, fft_size: usize) -> [(usize, usize); N_BANDS] {
    let nyquist = sr as f32 / 2.0;
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let inv_mel = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let mel_max = mel(nyquist);
    let mut out = [(0usize, 0usize); N_BANDS];
    let bin_per_hz = fft_size as f32 / sr as f32;
    for (b, slot) in out.iter_mut().enumerate() {
        let m_lo = mel_max * b as f32 / N_BANDS as f32;
        let m_hi = mel_max * (b + 1) as f32 / N_BANDS as f32;
        let f_lo = inv_mel(m_lo);
        let f_hi = inv_mel(m_hi);
        let bin_lo = (f_lo * bin_per_hz) as usize;
        let bin_hi = ((f_hi * bin_per_hz) as usize).min(fft_size / 2 + 1);
        *slot = (bin_lo, bin_hi.max(bin_lo + 1));
    }
    out
}

#[allow(dead_code)]
struct DiagAccent {
    sr: u32,
    band_bins: [(usize, usize); N_BANDS],
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    fft_in: Vec<f32>,
    fft_out: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    window_buf: Vec<f32>,
    pending_count: usize,
    pending: Vec<f32>,
    dc: [f32; N_BANDS],
    prev: [f32; N_BANDS],

    // Diagnostics
    per_band_power_sum: [f64; N_BANDS],
    per_band_power_max: [f32; N_BANDS],
    per_band_logp_sum: [f64; N_BANDS],
    per_band_dc_sum: [f64; N_BANDS],
    per_band_accent_sum: [f64; N_BANDS],
    per_band_accent_max: [f32; N_BANDS],
    per_band_accent_spikes: [usize; N_BANDS],
    // DC bin contribution to low-band power
    dc_bin_power_sum: f64,
    low_band_power_sum: f64,
    n_frames: usize,
}

impl DiagAccent {
    fn new(sr: u32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let fft_in = vec![0.0; FFT_SIZE];
        let fft_out = vec![Complex::new(0.0, 0.0); FFT_SIZE / 2 + 1];
        let fft_scratch = fft.make_scratch_vec();
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|n| {
                let x = std::f32::consts::PI * 2.0 * n as f32 / FFT_SIZE as f32;
                0.5 * (1.0 - x.cos())
            })
            .collect();
        Self {
            sr,
            band_bins: mel_band_bins(sr, FFT_SIZE),
            window,
            fft,
            fft_in,
            fft_out,
            fft_scratch,
            window_buf: vec![0.0; FFT_SIZE],
            pending_count: 0,
            pending: vec![0.0; HOP_SIZE],
            dc: [0.0; N_BANDS],
            prev: [0.0; N_BANDS],
            per_band_power_sum: [0.0; N_BANDS],
            per_band_power_max: [0.0; N_BANDS],
            per_band_logp_sum: [0.0; N_BANDS],
            per_band_dc_sum: [0.0; N_BANDS],
            per_band_accent_sum: [0.0; N_BANDS],
            per_band_accent_max: [0.0; N_BANDS],
            per_band_accent_spikes: [0; N_BANDS],
            dc_bin_power_sum: 0.0,
            low_band_power_sum: 0.0,
            n_frames: 0,
        }
    }

    fn process_block(&mut self, audio: &[f32]) -> Vec<[f32; N_BANDS]> {
        let mut out = Vec::new();
        for &s in audio {
            self.pending[self.pending_count] = s;
            self.pending_count += 1;
            if self.pending_count == HOP_SIZE {
                let n = FFT_SIZE - HOP_SIZE;
                self.window_buf.copy_within(HOP_SIZE.., 0);
                self.window_buf[n..].copy_from_slice(&self.pending);
                self.pending_count = 0;
                let frame = self.run_frame();
                out.push(frame);
            }
        }
        out
    }

    fn run_frame(&mut self) -> [f32; N_BANDS] {
        for i in 0..FFT_SIZE {
            self.fft_in[i] = self.window_buf[i] * self.window[i];
        }
        let _ = self.fft.process_with_scratch(
            &mut self.fft_in,
            &mut self.fft_out,
            &mut self.fft_scratch,
        );
        let mut accent = [0.0f32; N_BANDS];
        // DC-bin power tracking (low band's bin 0 contribution).
        let dc_bin_power = self.fft_out[0].norm_sqr();
        for (b, &(lo, hi)) in self.band_bins.iter().enumerate() {
            let mut power = 0.0f32;
            for c in &self.fft_out[lo..hi] {
                power += c.norm_sqr();
            }
            self.per_band_power_sum[b] += power as f64;
            if power > self.per_band_power_max[b] {
                self.per_band_power_max[b] = power;
            }
            let mu_norm = (1.0_f32 + LOG_MU).ln();
            let log_power = (1.0 + LOG_MU * power).ln() / mu_norm;
            self.per_band_logp_sum[b] += log_power as f64;
            self.dc[b] = DC_ALPHA * self.dc[b] + (1.0 - DC_ALPHA) * log_power;
            self.per_band_dc_sum[b] += self.dc[b] as f64;
            let after_dc = log_power - self.dc[b];
            let diff = after_dc - self.prev[b];
            self.prev[b] = after_dc;
            let hwr_diff = diff.max(0.0);
            let sustained = after_dc.max(0.0);
            let acc = ACCENT_W * hwr_diff + (1.0 - ACCENT_W) * sustained;
            accent[b] = acc;
            self.per_band_accent_sum[b] += acc as f64;
            if acc > self.per_band_accent_max[b] {
                self.per_band_accent_max[b] = acc;
            }
            if acc > 0.1 {
                self.per_band_accent_spikes[b] += 1;
            }
        }
        // Low-band DC contribution.
        if self.band_bins[0].0 == 0 {
            self.dc_bin_power_sum += dc_bin_power as f64;
            self.low_band_power_sum += self.per_band_power_sum[0]
                - self.per_band_power_sum[0].saturating_minus(self.per_band_power_sum[0]);
        }
        self.dc_bin_power_sum += dc_bin_power as f64;
        // Total low-band power for the ratio metric.
        let lo_power = {
            let (lo, hi) = self.band_bins[0];
            let mut s = 0.0f32;
            for c in &self.fft_out[lo..hi] {
                s += c.norm_sqr();
            }
            s
        };
        self.low_band_power_sum += lo_power as f64;

        self.n_frames += 1;
        accent
    }

    fn report(&self, track_name: &str) {
        println!("\n[diag {track_name}] n_frames={}", self.n_frames);
        let n = self.n_frames.max(1) as f64;
        println!(
            "  per-band power max:    L={:.2e}  LM={:.2e}  M={:.2e}  H={:.2e}",
            self.per_band_power_max[0],
            self.per_band_power_max[1],
            self.per_band_power_max[2],
            self.per_band_power_max[3]
        );
        println!(
            "  per-band power mean:   L={:.2e}  LM={:.2e}  M={:.2e}  H={:.2e}",
            self.per_band_power_sum[0] / n,
            self.per_band_power_sum[1] / n,
            self.per_band_power_sum[2] / n,
            self.per_band_power_sum[3] / n
        );
        println!(
            "  per-band logp mean:    L={:.3}   LM={:.3}   M={:.3}   H={:.3}",
            self.per_band_logp_sum[0] / n,
            self.per_band_logp_sum[1] / n,
            self.per_band_logp_sum[2] / n,
            self.per_band_logp_sum[3] / n
        );
        println!(
            "  per-band dc mean:      L={:.3}   LM={:.3}   M={:.3}   H={:.3}",
            self.per_band_dc_sum[0] / n,
            self.per_band_dc_sum[1] / n,
            self.per_band_dc_sum[2] / n,
            self.per_band_dc_sum[3] / n
        );
        println!(
            "  per-band accent max:   L={:.3}   LM={:.3}   M={:.3}   H={:.3}",
            self.per_band_accent_max[0],
            self.per_band_accent_max[1],
            self.per_band_accent_max[2],
            self.per_band_accent_max[3]
        );
        println!(
            "  per-band accent mean:  L={:.4}  LM={:.4}  M={:.4}  H={:.4}",
            self.per_band_accent_sum[0] / n,
            self.per_band_accent_sum[1] / n,
            self.per_band_accent_sum[2] / n,
            self.per_band_accent_sum[3] / n
        );
        println!(
            "  per-band accent spikes (>0.1): L={}  LM={}  M={}  H={}",
            self.per_band_accent_spikes[0],
            self.per_band_accent_spikes[1],
            self.per_band_accent_spikes[2],
            self.per_band_accent_spikes[3]
        );
        let dc_pct = if self.low_band_power_sum > 0.0 {
            100.0 * self.dc_bin_power_sum / self.low_band_power_sum
        } else {
            0.0
        };
        println!("  DC bin contribution to low-band power: {dc_pct:.1}%");
    }
}

// Provide a stub for the `saturating_minus` we accidentally call —
// not actually used at runtime; keeps the helper around in case we
// re-add the calculation later.
trait F64Sat {
    fn saturating_minus(self, _rhs: f64) -> f64;
}
impl F64Sat for f64 {
    fn saturating_minus(self, _rhs: f64) -> f64 {
        0.0
    }
}

fn audio_stats(audio: &[f32]) -> (f32, f32, usize, usize) {
    let mut peak = 0.0f32;
    let mut sum_sq = 0.0f64;
    let mut n_loud = 0usize;
    let mut n_very_loud = 0usize;
    for &s in audio {
        let a = s.abs();
        if a > peak {
            peak = a;
        }
        sum_sq += (s as f64) * (s as f64);
        if a > 0.1 {
            n_loud += 1;
        }
        if a > 0.5 {
            n_very_loud += 1;
        }
    }
    let rms = (sum_sq / audio.len().max(1) as f64).sqrt() as f32;
    (peak, rms, n_loud, n_very_loud)
}

fn load_beats(path: &Path) -> Option<Vec<f64>> {
    let s = fs::read_to_string(path).ok()?;
    let mut beats: Vec<f64> = s
        .lines()
        .filter_map(|l| l.split_whitespace().next().and_then(|t| t.parse().ok()))
        .collect();
    beats.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Some(beats)
}

fn tempo_from_beats(beats: &[f64]) -> f64 {
    if beats.len() < 3 {
        return 0.0;
    }
    let mut iois: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    iois.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = iois[iois.len() / 2];
    if med > 0.0 {
        60.0 / med
    } else {
        0.0
    }
}

/// Run a fresh KlapuriTracker over the audio; return the locked BPM
/// at the end.
fn klapuri_bpm(audio: &[f32]) -> f32 {
    let mut t = KlapuriTracker::new(TARGET_SR);
    let block = 512;
    let mut absolute = 0u64;
    for chunk in audio.chunks(block) {
        t.process_block(chunk, absolute, |_, _| {});
        absolute += chunk.len() as u64;
    }
    t.current_bpm()
}

/// Walk the bank+inference manually for finer-grained diagnostics:
/// after feeding all accent frames, what's the top-5 raw + scored?
fn dump_inference_state(audio: &[f32]) {
    let sr = TARGET_SR;
    let oss_rate = sr as f32 / HOP_SIZE as f32;
    let periods = default_period_range(sr, HOP_SIZE);
    let mut bank = ResonatorBank::new(&periods, sr as f32 / HOP_SIZE as f32);
    let mut inf = PeriodInference::new(oss_rate);
    let mut diag = DiagAccent::new(sr);

    // Feed accent → (per-band DC removal) → bank in one pass.
    // Mirrors `KlapuriTracker::accent_step`'s DC-removal so the
    // diagnostic reflects what the real tracker sees.
    let frames = diag.process_block(audio);
    let mut accent_dc = [0.0f32; N_BANDS];
    const ACCENT_DC_ALPHA: f32 = 0.99;
    for f in &frames {
        let mut zm = [0.0f32; N_BANDS];
        for b in 0..N_BANDS {
            accent_dc[b] = ACCENT_DC_ALPHA * accent_dc[b] + (1.0 - ACCENT_DC_ALPHA) * f[b];
            zm[b] = f[b] - accent_dc[b];
        }
        bank.tick(zm);
    }

    // Top-5 normalised resonator energies.
    let energies = bank.total_energies().to_vec();
    let mut idx: Vec<usize> = (0..energies.len()).collect();
    idx.sort_by(|&a, &b| energies[b].partial_cmp(&energies[a]).unwrap());
    println!("  top-5 normalised resonator energies (post DC-removal):");
    for &i in idx.iter().take(5) {
        let tau = periods[i];
        let bpm = 60.0 * oss_rate / tau as f32;
        println!("    τ={tau:3}  BPM={bpm:6.1}  energy={:.4e}", energies[i]);
    }

    if let Some((winner_idx, winner_tau, winner_bpm)) = inf.select(&mut bank) {
        println!("  inference winner: τ={winner_tau}  BPM={winner_bpm:.1}  (idx {winner_idx})");
    }
}

#[test]
fn klapuri_diagnose_ballroom() {
    let Ok(dir) = env::var("BEATPULSE_BALLROOM_DIR") else {
        eprintln!("BEATPULSE_BALLROOM_DIR not set — skipping");
        return;
    };
    let dir = PathBuf::from(dir);
    // Pick one .wav from each top-level genre subdir (Ballroom layout
    // has genre folders like Jive/, Waltz/, Tango/, etc).
    let genre_dirs = match fs::read_dir(dir.join("BallroomData")) {
        Ok(e) => e
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.path())
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };
    let mut picks: Vec<PathBuf> = Vec::new();
    for g in &genre_dirs {
        if let Ok(entries) = fs::read_dir(g) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                    picks.push(p);
                    break;
                }
            }
        }
        if picks.len() >= 5 {
            break;
        }
    }
    if picks.is_empty() {
        eprintln!("no Ballroom genre dirs found under {}", dir.display());
        return;
    }

    println!(
        "\n=== Klapuri diagnostic over {} representative tracks ===",
        picks.len()
    );

    for wav in &picks {
        let name = wav.file_stem().unwrap().to_string_lossy().into_owned();
        let beats_path = wav.with_extension("beats");
        let Some(audio) = decode_mono_44k1(wav) else {
            eprintln!("[diag] decode failed: {}", wav.display());
            continue;
        };
        let truth_bpm = load_beats(&beats_path)
            .map(|b| tempo_from_beats(&b))
            .unwrap_or(0.0);

        // 1. Audio scale.
        let (peak, rms, n_loud, n_very_loud) = audio_stats(&audio);
        println!(
            "\n--- {name} ---\n  audio: n={}  peak={peak:.3}  rms={rms:.3}  >0.1={n_loud}  >0.5={n_very_loud}",
            audio.len()
        );

        // 2. Accent diagnostics.
        let mut diag = DiagAccent::new(TARGET_SR);
        let _frames = diag.process_block(&audio);
        diag.report(&name);

        // 3+4. Inference state (rebuilds accent again, by design —
        // small repeat cost for self-contained reporting).
        dump_inference_state(&audio);

        // 5. Klapuri vs truth tempo.
        let k_bpm = klapuri_bpm(&audio);
        println!("  truth_bpm={truth_bpm:.1}  klapuri_bpm={k_bpm:.1}");
    }
}
