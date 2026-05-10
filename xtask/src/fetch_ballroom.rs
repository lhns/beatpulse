// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! `cargo xtask fetch-ballroom` — downloads the canonical Ballroom
//! audio (MTG ISMIR 2004 mirror) and the CPJKU annotations into a
//! gitignored local directory so `cargo test --features dataset-tests
//! --test dataset_ballroom` can run with no manual file wrangling.
//!
//! The script does **not** redistribute the dataset (ADR-0014); it
//! merely automates the download. Users obtain the data under whatever
//! terms the upstream sources publish.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

const AUDIO_URL: &str = "http://mtg.upf.edu/ismir2004/contest/tempoContest/data1.tar.gz";
const ANNOTATIONS_REPO: &str = "https://github.com/CPJKU/BallroomAnnotations";
/// Expected number of WAVs in the canonical archive — sanity-check the
/// extraction.
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

    let dest = dest.unwrap_or_else(default_dest);
    let cache = dest.join(".cache");
    let audio_dir = dest.join("audio");
    fs::create_dir_all(&cache).context("create cache dir")?;
    fs::create_dir_all(&audio_dir).context("create audio dir")?;

    let audio_url = audio_url.as_deref().unwrap_or(AUDIO_URL);
    let tarball = cache.join("data1.tar.gz");

    // 1. Download (skip if cached + non-empty unless --force).
    if force || !tarball.exists() || fs::metadata(&tarball)?.len() < 1024 {
        download(audio_url, &tarball)
            .with_context(|| format!("downloading {audio_url} → {}", tarball.display()))?;
    } else {
        eprintln!("[fetch-ballroom] cached tarball at {}", tarball.display());
    }

    // 2. Verify checksum (if user supplied one).
    if let Some(expected) = &expected_sha256 {
        let got = sha256_of(&tarball)?;
        if !got.eq_ignore_ascii_case(expected) {
            bail!(
                "tarball checksum mismatch: expected {expected}, got {got}. \
                 The mirror may have replaced the file. Pass --sha256 <hex> \
                 to lock in the new digest after manual verification, or \
                 --audio-url <alt> to use a different mirror."
            );
        }
        eprintln!("[fetch-ballroom] checksum verified");
    }

    // 3. Extract (skip if EXPECTED_WAV_COUNT WAVs already present unless --force).
    let existing = count_wavs(&audio_dir);
    if force || existing < EXPECTED_WAV_COUNT {
        eprintln!("[fetch-ballroom] extracting → {}", audio_dir.display());
        extract_tar_gz(&tarball, &audio_dir)?;
    } else {
        eprintln!(
            "[fetch-ballroom] audio already extracted ({existing} wavs at {})",
            audio_dir.display()
        );
    }

    // 4. Fetch annotations.
    let ann_dir = cache.join("BallroomAnnotations");
    if force && ann_dir.exists() {
        let _ = fs::remove_dir_all(&ann_dir);
    }
    if !ann_dir.exists() {
        eprintln!("[fetch-ballroom] cloning {ANNOTATIONS_REPO}");
        let status = Command::new("git")
            .args(["clone", "--depth", "1", ANNOTATIONS_REPO])
            .arg(&ann_dir)
            .status()
            .context("invoking git clone (is git installed?)")?;
        if !status.success() {
            bail!("git clone failed (exit {status:?})");
        }
    }

    // 5. Pair .beats files next to their .wav files.
    let paired = pair_annotations(&ann_dir, &audio_dir)?;
    let wav_total = count_wavs(&audio_dir);
    eprintln!("[fetch-ballroom] paired {paired} annotations / {wav_total} wavs");

    // 6. Print the env-var hint.
    let abs = fs::canonicalize(&audio_dir).unwrap_or(audio_dir.clone());
    let abs_str = abs.display().to_string();
    println!();
    println!("Done. {wav_total} wavs, {paired} beats files.");
    if cfg!(windows) {
        println!("Set the env var (PowerShell): $env:BEATPULSE_BALLROOM_DIR='{abs_str}'");
        println!("Set the env var (cmd):        set BEATPULSE_BALLROOM_DIR={abs_str}");
    } else {
        println!("Set the env var (bash/zsh):   export BEATPULSE_BALLROOM_DIR='{abs_str}'");
    }
    println!(
        "Then:                         cargo test --features dataset-tests --test dataset_ballroom -- --nocapture"
    );
    Ok(())
}

fn print_help() {
    println!(
        "Usage: cargo xtask fetch-ballroom [--dest <path>] [--force] \
         [--audio-url <url>] [--sha256 <hex>]\n\n\
         Downloads the Ballroom Dataset (audio: {AUDIO_URL}) and the\n\
         CPJKU annotations ({ANNOTATIONS_REPO}) into a gitignored local\n\
         directory (default: tests/data/local/ballroom/) so the dataset\n\
         test can run.\n\n\
         Options:\n  \
         --dest <path>      destination directory (default: tests/data/local/ballroom)\n  \
         --force            re-download / re-extract even if cached\n  \
         --audio-url <url>  override the audio mirror\n  \
         --sha256 <hex>     verify tarball SHA-256 (recommended)"
    );
}

fn default_dest() -> PathBuf {
    let manifest = env_manifest_dir();
    manifest.join("tests/data/local/ballroom")
}

fn env_manifest_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR points to xtask/; bubble up one level to repo root.
    let xtask_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    xtask_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(xtask_dir)
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("beatpulse-xtask/1.0")
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        bail!(
            "GET {url} returned HTTP {} — try --audio-url <alt-mirror>",
            resp.status()
        );
    }
    let total = resp.content_length();
    let pb = if let Some(len) = total {
        let pb = ProgressBar::new(len);
        pb.set_style(
            ProgressStyle::with_template("{bar:40.cyan/blue} {bytes}/{total_bytes} ({eta}) {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
        pb.set_message("downloading");
        pb
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_message("downloading (size unknown)");
        pb
    };

    let mut out = fs::File::create(dest)?;
    let mut buf = [0u8; 64 * 1024];
    let mut total_read: u64 = 0;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total_read += n as u64;
        pb.set_position(total_read);
    }
    pb.finish_with_message(format!("downloaded → {}", dest.display()));
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut f, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_tar_gz(src: &Path, dest: &Path) -> Result<()> {
    let f = fs::File::open(src)?;
    let gz = GzDecoder::new(f);
    let mut tar = tar::Archive::new(gz);
    tar.set_overwrite(true);
    tar.unpack(dest).context("untar")?;
    Ok(())
}

fn count_wavs(dir: &Path) -> usize {
    fn walk(dir: &Path, out: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some("wav") {
                *out += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, &mut n);
    n
}

/// CPJKU annotations are flat `<track>.beats` files whose stems match
/// the audio filenames (Ballroom audio is itself flat — one big folder
/// per genre — but stems are unique). For each annotation, locate the
/// corresponding `.wav` (recursively) and copy the annotation next to
/// it. Returns the number of successfully paired files.
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
                    eprintln!(
                        "[fetch-ballroom] no matching wav for annotation {}",
                        p.display()
                    );
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
