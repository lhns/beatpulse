// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Multi-band accent signal — Klapuri 2006 §III.
//!
//! Pipeline per analysis frame (one frame per `hop_size` audio
//! samples):
//!
//! ```text
//! windowed STFT → |·|² → log-compress → 4-band mel-warped sum →
//!                  DC removal (leaky-integrator subtract) →
//!                  half-wave-rectified differential → accent[4]
//! ```
//!
//! Output rate is the OSS rate = `sr / hop_size` (≈ 172 Hz at 44.1 kHz
//! / 256 hop). Accent values are non-negative; they spike around
//! perceptually-salient onsets in their respective frequency band.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

/// Number of accent **output** channels. Klapuri 2006 §III feeds the
/// comb-filter bank from 4 broad channels (low / low-mid / mid /
/// high), but each is a sum of finer-grained narrow bands that get
/// log-compressed and differentiated *individually* before the cross-
/// band sum. See [`N_NARROW_BANDS`] for the inner filterbank size.
pub const N_BANDS: usize = 4;

/// Number of **narrow** mel bands processed independently by the
/// per-band log-compression and differential stages. Each output
/// channel is the sum of `N_NARROW_BANDS / N_BANDS = 9` narrow bands
/// after differentiation. Paper-faithful design — collapsing to 4
/// channels *before* log+diff (the pass-≤11 approach) lets a single
/// loud transient in one frequency dominate the channel's accent and
/// drown out weaker periodic content, breaking the joint posterior's
/// expected evidence structure.
pub const N_NARROW_BANDS: usize = 36;
/// How many narrow bands fold into each output channel. Compile-time
/// constant — fails if `N_NARROW_BANDS` isn't a multiple of `N_BANDS`.
pub const NARROW_PER_BROAD: usize = N_NARROW_BANDS / N_BANDS;
const _: () = assert!(N_NARROW_BANDS == NARROW_PER_BROAD * N_BANDS);

/// Default STFT analysis parameters (matches Klapuri's "23 ms window,
/// 5.8 ms hop" at 44.1 kHz).
pub const FFT_SIZE: usize = 1024;
pub const HOP_SIZE: usize = 256;

/// μ-law compression strength (`log(1 + μ·p) / log(1 + μ)` on
/// power). 100 is the value Klapuri 2006 §III specifies for the
/// power-domain envelope.
const LOG_MU: f32 = 100.0;

/// Leaky-integrator coefficient for the DC-removal stage. α = 0.97 at
/// 172 Hz OSS rate cuts everything below ≈ 8 Hz, leaving the
/// onset-rate accent intact.
const DC_ALPHA: f32 = 0.97;

/// Weight of the half-wave-rectified differential vs. the sustained
/// log-power term in the accent composition. Klapuri 2006 §III gives
/// `accent = W·HWR(Δ log-power) + (1-W)·log-power` with `W ≈ 0.9`.
/// The sustained-energy term keeps the accent non-zero on tracks with
/// long sustained notes (waltz strings, tango bandoneon, sustained
/// chords) where pure spectral flux goes to ~0.
const ACCENT_W: f32 = 0.9;

/// Compute mel-warped band boundaries (in FFT-bin indices) for a
/// `fft_size`-point analysis at `sr`. Splits 0 .. sr/2 into
/// `N_NARROW_BANDS` equal-mel intervals. Narrow-band edition — used
/// for the per-band log/diff stage. Output channels group `NARROW_PER_BROAD`
/// adjacent narrow bands.
fn narrow_band_bins(sr: u32, fft_size: usize) -> [(usize, usize); N_NARROW_BANDS] {
    let nyquist = sr as f32 / 2.0;
    let mel = |f: f32| 2595.0 * (1.0 + f / 700.0).log10();
    let inv_mel = |m: f32| 700.0 * (10f32.powf(m / 2595.0) - 1.0);
    let mel_max = mel(nyquist);
    let mut out = [(0usize, 0usize); N_NARROW_BANDS];
    let bin_per_hz = fft_size as f32 / sr as f32;
    for (b, slot) in out.iter_mut().enumerate() {
        let m_lo = mel_max * b as f32 / N_NARROW_BANDS as f32;
        let m_hi = mel_max * (b + 1) as f32 / N_NARROW_BANDS as f32;
        let f_lo = inv_mel(m_lo);
        let f_hi = inv_mel(m_hi);
        let bin_lo = (f_lo * bin_per_hz) as usize;
        let bin_hi = ((f_hi * bin_per_hz) as usize).min(fft_size / 2 + 1);
        *slot = (bin_lo, bin_hi.max(bin_lo + 1));
    }
    out
}

/// Coarse-grained band boundaries — group of `NARROW_PER_BROAD` narrow
/// bands per output channel. Returned by [`MultiBandAccent::band_bins`]
/// for downstream consumers that need to know the output channel's
/// frequency range (the lo of the first narrow band, hi of the last).
fn broad_band_bins(narrow: &[(usize, usize); N_NARROW_BANDS]) -> [(usize, usize); N_BANDS] {
    let mut out = [(0usize, 0usize); N_BANDS];
    for (b, slot) in out.iter_mut().enumerate() {
        let lo = narrow[b * NARROW_PER_BROAD].0;
        let hi = narrow[(b + 1) * NARROW_PER_BROAD - 1].1;
        *slot = (lo, hi);
    }
    out
}

pub struct MultiBandAccent {
    sr: u32,
    fft_size: usize,
    hop_size: usize,
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    fft_in: Vec<f32>,
    fft_out: Vec<Complex<f32>>,
    fft_scratch: Vec<Complex<f32>>,

    /// Sliding window of the most recent `fft_size` audio samples.
    window_buf: Vec<f32>,
    /// How many samples have entered `pending` since the last hop.
    pending_count: usize,
    /// Buffer for the next hop's worth of samples before they're
    /// committed to `window_buf`.
    pending: Vec<f32>,

    /// Output-channel band bins (one per broad channel, the union of
    /// its narrow members). Exposed via `band_bins()` for callers
    /// that just want the broad-band frequency layout.
    band_bins: [(usize, usize); N_BANDS],
    /// Narrow-band bins — the actual analysis granularity. Each
    /// narrow band gets its own log-compression + DC tracker +
    /// differential, then `NARROW_PER_BROAD` adjacent narrow bands
    /// sum into one output channel.
    narrow_bins: [(usize, usize); N_NARROW_BANDS],
    /// Per-narrow-band DC tracker.
    narrow_dc: [f32; N_NARROW_BANDS],
    /// Per-narrow-band previous compressed-and-DC-removed value
    /// (state for the HWR differential).
    narrow_prev: [f32; N_NARROW_BANDS],

    /// Most recently produced accent frames. Drained / inspected by
    /// the caller via `process_block`'s callback. We don't persist
    /// frames between blocks; the callback consumes them in-place.
    _phantom_frames: (),
}

impl MultiBandAccent {
    pub fn new(sr: u32) -> Self {
        Self::with_size(sr, FFT_SIZE, HOP_SIZE)
    }

    pub fn with_size(sr: u32, fft_size: usize, hop_size: usize) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let fft_in = vec![0.0; fft_size];
        let fft_out = vec![Complex::new(0.0, 0.0); fft_size / 2 + 1];
        let fft_scratch = fft.make_scratch_vec();
        // Hann window.
        let window: Vec<f32> = (0..fft_size)
            .map(|n| {
                let x = std::f32::consts::PI * 2.0 * n as f32 / fft_size as f32;
                0.5 * (1.0 - x.cos())
            })
            .collect();
        Self {
            sr,
            fft_size,
            hop_size,
            window,
            fft,
            fft_in,
            fft_out,
            fft_scratch,
            window_buf: vec![0.0; fft_size],
            pending_count: 0,
            pending: vec![0.0; hop_size],
            band_bins: {
                let narrow = narrow_band_bins(sr, fft_size);
                broad_band_bins(&narrow)
            },
            narrow_bins: narrow_band_bins(sr, fft_size),
            narrow_dc: [0.0; N_NARROW_BANDS],
            narrow_prev: [0.0; N_NARROW_BANDS],
            _phantom_frames: (),
        }
    }

    pub fn sr(&self) -> u32 {
        self.sr
    }
    pub fn hop_size(&self) -> usize {
        self.hop_size
    }
    pub fn fft_size(&self) -> usize {
        self.fft_size
    }
    /// OSS rate in Hz.
    pub fn oss_rate(&self) -> f32 {
        self.sr as f32 / self.hop_size as f32
    }
    pub fn band_bins(&self) -> &[(usize, usize); N_BANDS] {
        &self.band_bins
    }

    pub fn reset(&mut self) {
        self.window_buf.fill(0.0);
        self.pending.fill(0.0);
        self.pending_count = 0;
        self.narrow_dc = [0.0; N_NARROW_BANDS];
        self.narrow_prev = [0.0; N_NARROW_BANDS];
    }

    /// Push `audio` samples through the accent pipeline; for each
    /// completed analysis frame (one per `hop_size` samples after the
    /// window has filled), invoke `on_frame([accent_low, low_mid, mid,
    /// high])`.
    ///
    /// Allocation-free.
    pub fn process_block<F: FnMut([f32; N_BANDS])>(&mut self, audio: &[f32], mut on_frame: F) {
        for &s in audio {
            self.pending[self.pending_count] = s;
            self.pending_count += 1;
            if self.pending_count == self.hop_size {
                // Slide window_buf left by hop_size, append pending.
                let n = self.fft_size - self.hop_size;
                self.window_buf.copy_within(self.hop_size.., 0);
                self.window_buf[n..].copy_from_slice(&self.pending);
                self.pending_count = 0;
                let frame = self.run_frame();
                on_frame(frame);
            }
        }
    }

    fn run_frame(&mut self) -> [f32; N_BANDS] {
        // Apply window into fft_in.
        for i in 0..self.fft_size {
            self.fft_in[i] = self.window_buf[i] * self.window[i];
        }
        // Forward FFT.
        let _ = self.fft.process_with_scratch(
            &mut self.fft_in,
            &mut self.fft_out,
            &mut self.fft_scratch,
        );

        // Paper-faithful per-narrow-band stage (Klapuri 2006 §III).
        // Each of the 36 narrow bands runs its own log-compression →
        // DC removal → HWR differential, BEFORE summing into broad
        // channels. This keeps a single loud transient in one
        // frequency from drowning out concurrent periodic content in
        // neighbouring bands (the cross-band sum after per-band
        // nonlinearities preserves more independent evidence than the
        // pre-sum nonlinearity used in passes ≤ 11).
        let mu_norm = (1.0_f32 + LOG_MU).ln();
        let mut narrow_hwr_diff = [0.0f32; N_NARROW_BANDS];
        let mut narrow_sustained = [0.0f32; N_NARROW_BANDS];
        for (b, &(lo, hi)) in self.narrow_bins.iter().enumerate() {
            let mut power = 0.0f32;
            for c in &self.fft_out[lo..hi] {
                power += c.norm_sqr();
            }
            // μ-law: log(1 + μ·p) / log(1 + μ). On power, not
            // magnitude (Klapuri 2006 §III).
            let log_power = (1.0 + LOG_MU * power).ln() / mu_norm;
            // Per-narrow-band DC tracker (leaky integrator).
            self.narrow_dc[b] = DC_ALPHA * self.narrow_dc[b] + (1.0 - DC_ALPHA) * log_power;
            let after_dc = log_power - self.narrow_dc[b];
            let diff = after_dc - self.narrow_prev[b];
            self.narrow_prev[b] = after_dc;
            narrow_hwr_diff[b] = diff.max(0.0);
            narrow_sustained[b] = after_dc.max(0.0);
        }

        // Sum NARROW_PER_BROAD adjacent narrow bands into each output
        // channel, then apply the W / (1-W) weighting on the SUM.
        // Output channel order matches `band_bins`.
        let mut accent = [0.0f32; N_BANDS];
        for (out_b, slot) in accent.iter_mut().enumerate() {
            let start = out_b * NARROW_PER_BROAD;
            let end = start + NARROW_PER_BROAD;
            let mut hwr_sum = 0.0f32;
            let mut sus_sum = 0.0f32;
            for k in start..end {
                hwr_sum += narrow_hwr_diff[k];
                sus_sum += narrow_sustained[k];
            }
            // No 1/N scaling — the sum of 9 narrow-band HWR-diffs
            // is naturally larger than the single broadband HWR-diff
            // it replaces. Downstream stages (per-band DC removal in
            // `KlapuriTracker::accent_step`, the bank's `(1-α)/(1+α)`
            // normalisation, and the inference's BPM prior) are all
            // scale-invariant in the relevant sense — the ratios
            // between τ values' bank energies are what drives
            // selection.
            *slot = ACCENT_W * hwr_sum + (1.0 - ACCENT_W) * sus_sum;
        }
        accent
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

    /// Sanity check: 4-band split at 44.1 kHz with default fft_size
    /// produces bins covering the full range monotonically.
    #[test]
    fn mel_band_bins_cover_full_range() {
        let narrow = narrow_band_bins(44_100, FFT_SIZE);
        assert_eq!(narrow[0].0, 0);
        for b in 1..N_NARROW_BANDS {
            // Narrow bands are monotonically increasing.
            assert!(narrow[b].0 >= narrow[b - 1].1.saturating_sub(1));
        }
        assert!(narrow[N_NARROW_BANDS - 1].1 <= FFT_SIZE / 2 + 1);

        // Broad bins (the group-sums) also cover the full range.
        let broad = broad_band_bins(&narrow);
        assert_eq!(broad[0].0, 0);
        for b in 1..N_BANDS {
            assert!(broad[b].0 >= broad[b - 1].1.saturating_sub(1));
        }
        assert!(broad[N_BANDS - 1].1 <= FFT_SIZE / 2 + 1);
    }

    /// Feed a 120 BPM kick track; every band should produce positive
    /// spikes (the kick's attack is broadband, which is exactly what
    /// makes accent detection useful — multiple channels all see
    /// transients). The looser shape we care about: spike rate is on
    /// the order of beats × small-N (a few hop frames around each
    /// beat), not "every frame is a spike".
    #[test]
    fn kick_train_produces_per_beat_spikes() {
        let sr = 44_100u32;
        let beat_period = (sr as f32 * 60.0 / 120.0) as usize; // 22050
        let n = beat_period * 60 + 1024; // ~30 s, 60 beats expected
        let audio = synth_kick(sr, n, beat_period);
        let mut accent = MultiBandAccent::new(sr);
        let mut spike_counts = [0usize; N_BANDS];
        let mut total_frames = 0usize;
        // Threshold scaled to the μ-law-normalised accent range
        // [0, 1] (post-G3). Pick a small but non-trivial value: 0.01.
        accent.process_block(&audio, |frame| {
            total_frames += 1;
            for (b, &v) in frame.iter().enumerate() {
                if v > 0.01 {
                    spike_counts[b] += 1;
                }
            }
        });
        // Each band should fire at least a handful of spikes (one per
        // beat at minimum, plus HWR-diff neighbours and the sustained
        // term keeping non-zero accent), well below "every frame".
        for (b, &n) in spike_counts.iter().enumerate() {
            assert!(
                (30..total_frames).contains(&n),
                "band {b}: spike count {n} out of expected range (total frames {total_frames})"
            );
        }
    }

    /// White-ish noise should excite all bands roughly equally and
    /// the HWR-diff should still produce nonzero output (DC tracker
    /// adapts).
    #[test]
    fn noise_excites_all_bands_after_warmup() {
        let sr = 44_100u32;
        let n = 44_100; // 1 s
        let mut seed: u32 = 0xDEADBEEF;
        let audio: Vec<f32> = (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(48271) % 2_147_483_647;
                (seed as f32 / 2_147_483_647.0) * 2.0 - 1.0
            })
            .collect();
        let mut accent = MultiBandAccent::new(sr);
        let mut nonzero = [0usize; N_BANDS];
        let mut frames = 0;
        accent.process_block(&audio, |frame| {
            frames += 1;
            // Skip the first few frames while DC tracker warms up.
            if frames < 20 {
                return;
            }
            for (b, &v) in frame.iter().enumerate() {
                if v > 0.0 {
                    nonzero[b] += 1;
                }
            }
        });
        for (b, &n) in nonzero.iter().enumerate() {
            assert!(n > 10, "band {b} produced no positive accent");
        }
    }
}
