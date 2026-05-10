// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Ballroom Dataset fetcher (MTG ISMIR 2004 mirror + CPJKU annotations).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};

use super::common;

const AUDIO_URL: &str = "https://mtg.upf.edu/ismir2004/contest/tempoContest/data1.tar.gz";
// 1.35 GB (~698 × 30s 16-bit 44.1 kHz stereo WAVs). The mtg.upf.edu
// HTTP variant currently 403s — only HTTPS works (verified 2026-05-10).
const ANNOTATIONS_REPO: &str = "https://github.com/CPJKU/BallroomAnnotations";
const EXPECTED_WAV_COUNT: usize = 685;

pub fn run(args: &[String]) -> Result<()> {
    let mut dest: Option<PathBuf> = None;
    let mut force = false;
    let mut audio_url: Option<String> = None;
    let mut expected_sha256: Option<String> = None;

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
            "--sha256" => {
                expected_sha256 = Some(
                    iter.next()
                        .context("--sha256 requires a hex digest")?
                        .clone(),
                )
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => bail!("unknown argument {other:?}; pass --help"),
        }
    }

    let dest = dest.unwrap_or_else(|| common::default_dest("ballroom"));
    let cache = dest.join(".cache");
    let audio_dir = dest.join("audio");
    fs::create_dir_all(&cache).context("create cache dir")?;
    fs::create_dir_all(&audio_dir).context("create audio dir")?;

    let audio_url = audio_url.as_deref().unwrap_or(AUDIO_URL);
    let tarball = cache.join("data1.tar.gz");
    let client = common::http_client()?;

    if force {
        let _ = fs::remove_file(&tarball);
    }
    // download() is resume-aware: if a partial cache exists it sends a
    // Range request and appends; if it's already complete it returns
    // immediately.
    common::download(&client, audio_url, &tarball)
        .with_context(|| format!("downloading {audio_url} → {}", tarball.display()))?;

    if let Some(expected) = &expected_sha256 {
        let got = common::sha256_of(&tarball)?;
        if !got.eq_ignore_ascii_case(expected) {
            bail!(
                "tarball checksum mismatch: expected {expected}, got {got}. \
                 Pass --sha256 <hex> to lock in the new digest after manual \
                 verification, or --audio-url <alt> to use a different mirror."
            );
        }
        eprintln!("[ballroom] checksum verified");
    }

    let existing = common::count_files_with_ext(&audio_dir, "wav");
    if force || existing < EXPECTED_WAV_COUNT {
        eprintln!("[ballroom] extracting → {}", audio_dir.display());
        common::extract_tar_gz(&tarball, &audio_dir)?;
    } else {
        eprintln!(
            "[ballroom] audio already extracted ({existing} wavs at {})",
            audio_dir.display()
        );
    }

    let ann_dir = cache.join("BallroomAnnotations");
    if force && ann_dir.exists() {
        let _ = fs::remove_dir_all(&ann_dir);
    }
    if !ann_dir.exists() {
        eprintln!("[ballroom] cloning {ANNOTATIONS_REPO}");
        let status = Command::new("git")
            .args(["clone", "--depth", "1", ANNOTATIONS_REPO])
            .arg(&ann_dir)
            .status()
            .context("invoking git clone (is git installed?)")?;
        if !status.success() {
            bail!("git clone failed (exit {status:?})");
        }
    }

    let paired = pair_annotations(&ann_dir, &audio_dir)?;
    let wav_total = common::count_files_with_ext(&audio_dir, "wav");
    eprintln!("[ballroom] paired {paired} annotations / {wav_total} wavs");

    println!();
    println!("Done. {wav_total} wavs, {paired} beats files.");
    common::print_env_hint("BEATPULSE_BALLROOM_DIR", &audio_dir, "dataset_ballroom");
    Ok(())
}

fn print_help() {
    println!(
        "Usage: cargo xtask fetch ballroom [--dest <path>] [--force] \
         [--audio-url <url>] [--sha256 <hex>]\n\n\
         Audio:       {AUDIO_URL}\n\
         Annotations: {ANNOTATIONS_REPO}\n\n\
         Default dest: tests/data/local/ballroom/"
    );
}

/// CPJKU annotations are flat `<track>.beats` files whose stems match
/// the audio filenames. Copy each annotation next to its `.wav`.
fn pair_annotations(ann_dir: &Path, audio_dir: &Path) -> Result<usize> {
    let mut wavs: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
    fn collect(dir: &Path, out: &mut std::collections::HashMap<String, PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                collect(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.insert(stem.to_string(), p);
                }
            }
        }
    }
    collect(audio_dir, &mut wavs);
    if wavs.is_empty() {
        return Err(anyhow!(
            "no .wav files found under {}; extraction may have failed",
            audio_dir.display()
        ));
    }

    let mut paired = 0usize;
    let mut walked = 0usize;
    fn walk_ann(
        dir: &Path,
        paired: &mut usize,
        walked: &mut usize,
        wavs: &std::collections::HashMap<String, PathBuf>,
    ) -> Result<()> {
        let entries = fs::read_dir(dir)?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) == Some(".git") {
                    continue;
                }
                walk_ann(&p, paired, walked, wavs)?;
            } else if p.extension().and_then(|s| s.to_str()) == Some("beats") {
                *walked += 1;
                let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if let Some(wav) = wavs.get(stem) {
                    let dst = wav.with_extension("beats");
                    fs::copy(&p, &dst)?;
                    *paired += 1;
                } else {
                    eprintln!("[ballroom] no matching wav for annotation {}", p.display());
                }
            }
        }
        Ok(())
    }
    walk_ann(ann_dir, &mut paired, &mut walked, &wavs)?;
    if paired == 0 {
        bail!("no annotations matched any wav (walked {walked} .beats files)");
    }
    Ok(paired)
}
