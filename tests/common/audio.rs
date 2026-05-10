// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Multi-format audio decode for the dataset harnesses.
//!
//! Returns 44.1 kHz mono `f32` (the only sample rate the existing test
//! pipeline supports). Files at other sample rates are rejected — we
//! don't bundle a resampler, since GiantSteps is already 44.1 kHz LOFI
//! and Ballroom / SMC ship as 44.1 kHz WAV.

use std::path::Path;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::sample::Sample;

const TARGET_SR: u32 = 44_100;

/// Decode the audio file at `path` to mono f32 at 44.1 kHz. Returns
/// `None` (and prints a message) if the file's sample rate doesn't
/// match — we don't resample.
pub fn decode_mono_44k1(path: &Path) -> Option<Vec<f32>> {
    let file = std::fs::File::open(path).ok()?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
        hint.with_extension(ext);
    }

    let probe = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut format = probe.format;
    let track = format.default_track()?.clone();
    let codec_params = track.codec_params;
    let sample_rate = codec_params.sample_rate?;
    if sample_rate != TARGET_SR {
        eprintln!(
            "skipping {} (sample rate {sample_rate} != {TARGET_SR})",
            path.display()
        );
        return None;
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .ok()?;

    let mut mono = Vec::<f32>::new();
    let track_id = track.id;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => append_mono(&decoded, &mut mono),
            Err(SymphoniaError::DecodeError(_)) => continue, // skip bad packets
            Err(_) => break,
        }
    }
    Some(mono)
}

fn append_mono(decoded: &AudioBufferRef<'_>, out: &mut Vec<f32>) {
    use symphonia::core::audio::AudioBufferRef::*;
    match decoded {
        F32(buf) => downmix(buf, out, |s| s),
        F64(buf) => downmix(buf, out, |s| s as f32),
        S16(buf) => downmix(buf, out, |s| s as f32 / i16::MAX as f32),
        S24(buf) => downmix(buf, out, |s| s.inner() as f32 / 8_388_608.0),
        S32(buf) => downmix(buf, out, |s| s as f32 / i32::MAX as f32),
        S8(buf) => downmix(buf, out, |s| s as f32 / i8::MAX as f32),
        U8(buf) => downmix(buf, out, |s| (s as f32 - 128.0) / 128.0),
        U16(buf) => downmix(buf, out, |s| (s as f32 - 32_768.0) / 32_768.0),
        U24(buf) => downmix(buf, out, |s| (s.inner() as f32 - 8_388_608.0) / 8_388_608.0),
        U32(buf) => downmix(buf, out, |s| (s as f32 / u32::MAX as f32) * 2.0 - 1.0),
    }
}

fn downmix<S: Sample + Copy, F: Fn(S) -> f32>(
    buf: &symphonia::core::audio::AudioBuffer<S>,
    out: &mut Vec<f32>,
    to_f32: F,
) {
    let n_ch = buf.spec().channels.count();
    let n_frames = buf.frames();
    out.reserve(n_frames);
    if n_ch == 1 {
        for &s in &buf.chan(0)[..n_frames] {
            out.push(to_f32(s));
        }
        return;
    }
    let inv = 1.0 / n_ch as f32;
    for i in 0..n_frames {
        let mut sum = 0.0f32;
        for c in 0..n_ch {
            sum += to_f32(buf.chan(c)[i]);
        }
        out.push(sum * inv);
    }
}
