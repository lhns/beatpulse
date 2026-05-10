// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! SMC_MIREX dataset fetcher.
//!
//! Per ADR-0014 the SMC_MIREX 2012 dataset is the explicit "hard cases"
//! stress test (217 × 40s clips chosen because beat-tracking fails on
//! them). The canonical mirror at INESC TEC has historically been
//! flaky: it may or may not be reachable on any given day.
//!
//! Strategy: try the canonical mirror; if it's unreachable, print a
//! clear "manual fallback" message and exit non-zero. With
//! `--skip-download`, just verify the layout of an already-populated
//! `audio/` directory.

use std::fs;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use super::common;

/// The canonical INESC TEC mirror as documented in the literature.
/// Currently (2026-05) this URL has been intermittent. Override via
/// `--audio-url` if a working mirror is found.
const CANONICAL_URL: &str =
    "http://smc.inesctec.pt/research/data-2/SMC_MIREX_Annotations_05_08_2014.zip";

pub fn run(args: &[String]) -> Result<()> {
    let mut dest: Option<PathBuf> = None;
    let mut force = false;
    let mut audio_url: Option<String> = None;
    let mut skip_download = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dest" => {
                dest = Some(PathBuf::from(
                    iter.next().context("--dest requires a path")?,
                ))
            }
            "--force" => force = true,
            "--audio-url" => {
                audio_url = Some(iter.next().context("--audio-url requires a URL")?.clone())
            }
            "--skip-download" => skip_download = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => bail!("unknown argument {other:?}; pass --help"),
        }
    }

    let dest = dest.unwrap_or_else(|| common::default_dest("smc"));
    let audio_dir = dest.join("audio");
    fs::create_dir_all(&audio_dir).context("create audio dir")?;

    if !skip_download {
        let url = audio_url.as_deref().unwrap_or(CANONICAL_URL);
        let cache = dest.join(".cache");
        fs::create_dir_all(&cache).context("create cache dir")?;
        let archive = cache.join("smc.zip");
        let client = common::http_client()?;

        if force || !archive.exists() {
            eprintln!("[smc] trying {url}");
            match common::download(&client, url, &archive) {
                Ok(()) => eprintln!("[smc] downloaded → {}", archive.display()),
                Err(e) => {
                    let _ = fs::remove_file(&archive);
                    print_manual_fallback(&audio_dir, &e.to_string());
                    bail!("SMC mirror unreachable");
                }
            }
        } else {
            eprintln!("[smc] cached archive at {}", archive.display());
        }
        // We don't auto-extract: the SMC distribution layout has
        // varied historically, and the exact zip structure depends on
        // which mirror you got it from. Print instructions instead.
        eprintln!(
            "[smc] archive saved at {}. Extract its WAV + .txt files \
             into {}, then re-run with --skip-download to verify the \
             layout.",
            archive.display(),
            audio_dir.display()
        );
        return Ok(());
    }

    // --skip-download: verify the layout.
    let n_wav = common::count_files_with_ext(&audio_dir, "wav");
    let n_txt = common::count_files_with_ext(&audio_dir, "txt");
    if n_wav == 0 {
        bail!(
            "no .wav files under {}. Place SMC audio + .txt annotations \
             there manually, then re-run with --skip-download.",
            audio_dir.display()
        );
    }
    println!();
    println!(
        "Layout OK. {n_wav} wavs, {n_txt} txt annotations under {}.",
        audio_dir.display()
    );
    common::print_env_hint("BEATPULSE_SMC_DIR", &audio_dir, "dataset_smc");
    Ok(())
}

fn print_manual_fallback(audio_dir: &std::path::Path, why: &str) {
    eprintln!();
    eprintln!("[smc] mirror error: {why}");
    eprintln!(
        "[smc] To use SMC_MIREX, obtain the dataset manually:\n  \
           - Email the SMC group (https://smc.inesctec.pt/) for the audio + annotations.\n  \
           - Place the WAV files and matching .txt beat annotations under:\n    \
             {}\n  \
           - Re-run this command with --skip-download to verify the layout.",
        audio_dir.display()
    );
}

fn print_help() {
    println!(
        "Usage: cargo xtask fetch smc [--dest <path>] [--force] \
         [--audio-url <url>] [--skip-download]\n\n\
         Canonical mirror: {CANONICAL_URL}\n\n\
         The SMC mirror is currently unreliable. Pass --skip-download \
         to verify the layout of audio you've placed manually.\n\n\
         Default dest: tests/data/local/smc/"
    );
}
