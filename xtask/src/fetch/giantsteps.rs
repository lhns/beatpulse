// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! GiantSteps Tempo Dataset fetcher.
//!
//! Audio: `https://www.cp.jku.at/datasets/giantsteps/backup/<id>.LOFI.mp3`
//! (JKU's stable mirror; the original Beatport CDN is the backup).
//!
//! Annotations & MD5 sidecars:
//! `https://github.com/GiantSteps/giantsteps-tempo-dataset` →
//! - `md5/<id>.LOFI.mp3.md5` — single hex digest line.
//! - `annotations_v2/tempo/<id>.LOFI.bpm` — single integer BPM line.
//!
//! Best-effort URL handling (per user choice): each track is downloaded
//! independently; on HTTP error / timeout / MD5 mismatch we log + skip
//! and keep going, then report a final `<ok>/<total>` summary. A few
//! rotted URLs are expected.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

use super::common;

const PRIMARY_BASE: &str = "https://www.cp.jku.at/datasets/giantsteps/backup/";
const BACKUP_BASE: &str = "http://geo-samples.beatport.com/lofi/";
const REPO_URL: &str = "https://github.com/GiantSteps/giantsteps-tempo-dataset";

pub fn run(args: &[String]) -> Result<()> {
    let mut dest: Option<PathBuf> = None;
    let mut force = false;
    let mut limit: Option<usize> = None;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dest" => {
                dest = Some(PathBuf::from(
                    iter.next().context("--dest requires a path")?,
                ))
            }
            "--force" => force = true,
            "--limit" => {
                limit = Some(
                    iter.next()
                        .context("--limit requires a number")?
                        .parse()
                        .context("--limit value is not a number")?,
                )
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => bail!("unknown argument {other:?}; pass --help"),
        }
    }

    let dest = dest.unwrap_or_else(|| common::default_dest("giantsteps"));
    let cache = dest.join(".cache");
    let audio_dir = dest.join("audio");
    fs::create_dir_all(&cache).context("create cache dir")?;
    fs::create_dir_all(&audio_dir).context("create audio dir")?;

    // 1. Clone (or refresh) the dataset repo for md5s + annotations.
    let repo_dir = cache.join("giantsteps-tempo-dataset");
    if force && repo_dir.exists() {
        let _ = fs::remove_dir_all(&repo_dir);
    }
    if !repo_dir.exists() {
        eprintln!("[giantsteps] cloning {REPO_URL}");
        let status = Command::new("git")
            .args(["clone", "--depth", "1", REPO_URL])
            .arg(&repo_dir)
            .status()
            .context("invoking git clone (is git installed?)")?;
        if !status.success() {
            bail!("git clone failed (exit {status:?})");
        }
    }

    // 2. Enumerate <id>.LOFI.mp3.md5 files.
    let md5_dir = repo_dir.join("md5");
    let ann_dir = repo_dir.join("annotations_v2/tempo");
    let mut entries: Vec<PathBuf> = fs::read_dir(&md5_dir)
        .with_context(|| format!("reading {}", md5_dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("md5"))
        .collect();
    entries.sort();
    if let Some(n) = limit {
        entries.truncate(n);
    }
    let total = entries.len();
    if total == 0 {
        bail!("no .md5 sidecar files found under {}", md5_dir.display());
    }

    // 3. Per-track download + MD5 verify + annotation copy.
    let client = common::http_client()?;
    let mut ok = 0usize;
    let mut skipped_dl = 0usize;
    let mut skipped_md5 = 0usize;
    let mut skipped_no_ann = 0usize;
    let pb = indicatif::ProgressBar::new(total as u64);
    pb.set_style(
        indicatif::ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} ok={msg}")
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar()),
    );

    for md5_path in &entries {
        // <id>.LOFI.mp3.md5 → strip ".md5"
        let md5_stem = md5_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let mp3_name = md5_stem.trim_end_matches(".md5");
        let mp3_dest = audio_dir.join(mp3_name);
        let bpm_src = ann_dir.join(format!("{}.bpm", mp3_name.trim_end_matches(".mp3")));
        let bpm_dest = mp3_dest.with_extension("bpm");

        let expected_md5 = fs::read_to_string(md5_path)
            .ok()
            .map(|s| s.trim().to_lowercase())
            .unwrap_or_default();

        // Skip if already complete.
        if !force
            && mp3_dest.exists()
            && bpm_dest.exists()
            && common::md5_of(&mp3_dest)
                .map(|h| h == expected_md5)
                .unwrap_or(false)
        {
            ok += 1;
            pb.set_message(ok.to_string());
            pb.inc(1);
            continue;
        }

        // Download primary, fall back to backup.
        let primary_url = format!("{PRIMARY_BASE}{mp3_name}");
        let backup_url = format!("{BACKUP_BASE}{mp3_name}");
        let dl_result = common::download_quiet(&client, &primary_url, &mp3_dest)
            .or_else(|_| common::download_quiet(&client, &backup_url, &mp3_dest));
        if let Err(e) = dl_result {
            eprintln!("[giantsteps] skip {mp3_name}: download failed ({e})");
            let _ = fs::remove_file(&mp3_dest);
            skipped_dl += 1;
            pb.inc(1);
            continue;
        }

        if !expected_md5.is_empty() {
            let got = common::md5_of(&mp3_dest)?;
            if got != expected_md5 {
                eprintln!(
                    "[giantsteps] skip {mp3_name}: md5 mismatch (expected {expected_md5}, got {got})"
                );
                let _ = fs::remove_file(&mp3_dest);
                skipped_md5 += 1;
                pb.inc(1);
                continue;
            }
        }

        if bpm_src.exists() {
            fs::copy(&bpm_src, &bpm_dest)?;
        } else {
            eprintln!(
                "[giantsteps] skip {mp3_name}: no annotation at {}",
                bpm_src.display()
            );
            let _ = fs::remove_file(&mp3_dest);
            skipped_no_ann += 1;
            pb.inc(1);
            continue;
        }

        ok += 1;
        pb.set_message(ok.to_string());
        pb.inc(1);
    }
    pb.finish();

    // 4. Optional metadata file (genre groupings).
    let meta_src = repo_dir.join("metadata/giantsteps-tempo_metadata.json");
    let meta_dest = dest.join("metadata.json");
    if meta_src.exists() {
        let _ = fs::copy(&meta_src, &meta_dest);
    }

    println!();
    println!(
        "Done. {ok}/{total} tracks (skipped: {skipped_dl} download, \
         {skipped_md5} md5, {skipped_no_ann} no-annotation)."
    );
    common::print_env_hint("BEATPULSE_GIANTSTEPS_DIR", &audio_dir, "dataset_giantsteps");
    Ok(())
}

fn print_help() {
    println!(
        "Usage: cargo xtask fetch giantsteps [--dest <path>] [--force] [--limit <N>]\n\n\
         Audio:  {PRIMARY_BASE}<id>.LOFI.mp3 (backup: {BACKUP_BASE})\n\
         Repo:   {REPO_URL} (md5/ + annotations_v2/tempo/)\n\n\
         Default dest: tests/data/local/giantsteps/\n\
         --limit N: only fetch the first N tracks (useful for smoke-testing)."
    );
}
