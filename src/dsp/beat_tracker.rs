// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

//! Onset detection wrapping `aubio_rs::Onset`. See `BeatPulse-SPEC.md` §5.1
//! and ADR-0003.
//!
//! aubio operates on hop-sized buffers. Host block sizes are arbitrary, so
//! `BeatTracker` accumulates host samples into an internal hop buffer and
//! calls `Onset::do_result` whenever a hop is full. When aubio reports an
//! onset, we use its internal frame counter to compute the sample position
//! within the current host block.

use aubio_rs::{Onset, OnsetMode};

use crate::params::OnsetMethod;

const BUF_SIZE: usize = 1024;
const HOP_SIZE: usize = 512;

pub struct BeatTracker {
    onset: Onset,
    sample_rate: u32,
    method: OnsetMethod,
    threshold: f32,

    accum: [f32; HOP_SIZE],
    accum_len: usize,

    /// Total samples that have been pushed into aubio's `do_` so far.
    /// Equivalent to aubio's internal frame counter.
    aubio_frames: u64,
}

// SAFETY: aubio C objects are not thread-safe, but the audio thread is the
// only thread that ever touches `BeatTracker` (per spec §12 — UI reads
// values via the SharedState atomics). The host frameworks (nih-plug)
// require `Plugin: Send` even though they ensure the audio callback is
// always invoked on the same thread. This impl makes that single-thread
// guarantee explicit.
unsafe impl Send for BeatTracker {}

impl BeatTracker {
    pub fn new(sample_rate: u32, method: OnsetMethod, threshold: f32) -> Result<Self, &'static str> {
        let onset = Onset::new(method_to_aubio(method), BUF_SIZE, HOP_SIZE, sample_rate)
            .map_err(|_| "aubio Onset::new failed")?;
        let mut tracker = Self {
            onset,
            sample_rate,
            method,
            threshold,
            accum: [0.0; HOP_SIZE],
            accum_len: 0,
            aubio_frames: 0,
        };
        tracker.set_threshold(threshold);
        Ok(tracker)
    }

    pub fn set_threshold(&mut self, threshold: f32) {
        self.threshold = threshold;
        self.onset.set_threshold(threshold);
    }

    pub fn set_method(&mut self, method: OnsetMethod) -> Result<(), &'static str> {
        if method == self.method {
            return Ok(());
        }
        // aubio doesn't support changing the detection function in-place;
        // recreate the Onset object.
        let new_onset = Onset::new(method_to_aubio(method), BUF_SIZE, HOP_SIZE, self.sample_rate)
            .map_err(|_| "aubio Onset::new failed")?;
        self.onset = new_onset;
        self.method = method;
        self.onset.set_threshold(self.threshold);
        // Frame counter resets implicitly because aubio's internal state
        // restarts. Reset our own tracking so onset positions stay coherent.
        self.aubio_frames = 0;
        self.accum_len = 0;
        Ok(())
    }

    pub fn hop_size(&self) -> usize {
        HOP_SIZE
    }

    pub fn buf_size(&self) -> usize {
        BUF_SIZE
    }

    /// Reset internal state. Called on sample-rate change or manual resync.
    pub fn reset(&mut self) {
        self.accum_len = 0;
        self.aubio_frames = 0;
    }

    /// Process a mono block of host samples. The callback fires once per
    /// detected onset with `(sample_offset_within_block, fractional_offset)`
    /// where `fractional_offset` is in [0.0, 1.0) representing aubio's
    /// sub-hop precision. The integer sample offset is sufficient for
    /// driving the PLL; the fractional part is exposed for callers that
    /// want to refine timing.
    ///
    /// Allocation-free: uses a fixed-size accumulator and aubio's
    /// pre-allocated buffers.
    pub fn process_block<F: FnMut(u32, f32)>(&mut self, block: &[f32], mut on_onset: F) {
        let mut i = 0usize;
        while i < block.len() {
            let space = HOP_SIZE - self.accum_len;
            let take = space.min(block.len() - i);
            self.accum[self.accum_len..self.accum_len + take]
                .copy_from_slice(&block[i..i + take]);
            self.accum_len += take;
            i += take;

            if self.accum_len == HOP_SIZE {
                // Position of the last sample in the current hop, expressed
                // as offset within the host block.
                let hop_end_block_offset = i; // = i after the copy
                self.consume_hop(hop_end_block_offset as u32, &mut on_onset);
            }
        }
    }

    fn consume_hop<F: FnMut(u32, f32)>(&mut self, hop_end_block_offset: u32, on_onset: &mut F) {
        let hop_start_aubio_frame = self.aubio_frames;
        if let Ok(result) = self.onset.do_result(&self.accum[..]) {
            self.aubio_frames += HOP_SIZE as u64;
            if result > 0.0 {
                // aubio's `get_last` returns the absolute sample position
                // (in its frame counter) of the most recent onset. Convert
                // to a block offset.
                let onset_aubio = self.onset.get_last() as u64;
                let onset_within_hop = onset_aubio.saturating_sub(hop_start_aubio_frame);
                // Onset position within this host block:
                //   block_offset = hop_end_block_offset - HOP_SIZE + onset_within_hop
                let onset_block_offset = (hop_end_block_offset as i64
                    - HOP_SIZE as i64
                    + onset_within_hop as i64)
                    .max(0) as u32;
                let frac = (result - result.floor()).clamp(0.0, 1.0);
                on_onset(onset_block_offset, frac);
            }
        } else {
            self.aubio_frames += HOP_SIZE as u64;
        }
        self.accum_len = 0;
    }

    /// Convenience: process a stereo (or multichannel) block by averaging
    /// channels into a temporary mono buffer, then calling
    /// [`Self::process_block`].
    pub fn process_block_multichannel<F: FnMut(u32, f32)>(
        &mut self,
        channels: &[&[f32]],
        scratch: &mut [f32],
        mut on_onset: F,
    ) {
        if channels.is_empty() {
            return;
        }
        let n = channels[0].len().min(scratch.len());
        let nch = channels.len() as f32;
        for i in 0..n {
            let mut sum = 0.0f32;
            for ch in channels {
                sum += ch[i];
            }
            scratch[i] = sum / nch;
        }
        self.process_block(&scratch[..n], |off, frac| on_onset(off, frac));
    }
}

fn method_to_aubio(m: OnsetMethod) -> OnsetMode {
    match m {
        OnsetMethod::Hfc => OnsetMode::Hfc,
        OnsetMethod::Complex => OnsetMode::Complex,
        OnsetMethod::SpecDiff => OnsetMode::SpecDiff,
        OnsetMethod::Kl => OnsetMode::Kl,
        OnsetMethod::Mkl => OnsetMode::Mkl,
        OnsetMethod::Phase => OnsetMode::Phase,
        OnsetMethod::SpecFlux => OnsetMode::SpecFlux,
    }
}
