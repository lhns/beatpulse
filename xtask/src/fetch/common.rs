// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! Shared helpers for the dataset fetchers (`xtask/src/fetch/*.rs`).
//! Network, hashing, archive extraction, on-disk layout.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};

/// Repo-root local data directory. All datasets live under
/// `<repo>/tests/data/local/<name>/` (gitignored).
pub fn default_dest(name: &str) -> PathBuf {
    let xtask_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo = xtask_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or(xtask_dir);
    repo.join("tests/data/local").join(name)
}

/// Build a long-timeout blocking HTTP client with our user-agent.
pub fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent("beatpulse-xtask/1.0")
        .timeout(std::time::Duration::from_secs(600))
        .build()?)
}

/// Download `url` → `dest`, with a progress bar. Returns Ok on success;
/// bubbles up reqwest errors and HTTP 4xx/5xx as Err.
pub fn download(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> Result<()> {
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        bail!("GET {url} returned HTTP {}", resp.status());
    }
    let pb = match resp.content_length() {
        Some(len) => {
            let pb = ProgressBar::new(len);
            pb.set_style(
                ProgressStyle::with_template(
                    "{bar:40.cyan/blue} {bytes}/{total_bytes} ({eta}) {msg}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            pb.set_message("downloading");
            pb
        }
        None => {
            let pb = ProgressBar::new_spinner();
            pb.set_message("downloading (size unknown)");
            pb
        }
    };

    let mut out = fs::File::create(dest)?;
    let mut buf = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        total += n as u64;
        pb.set_position(total);
    }
    pb.finish_with_message(format!("downloaded → {}", dest.display()));
    Ok(())
}

/// Quiet variant — no progress bar; for many small files (per-track
/// downloads). Returns the size in bytes on success.
pub fn download_quiet(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> Result<u64> {
    let mut resp = client.get(url).send()?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let mut out = fs::File::create(dest)?;
    let n = io::copy(&mut resp, &mut out)?;
    Ok(n)
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut f, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn md5_of(path: &Path) -> Result<String> {
    // sha2 doesn't include MD5; use a tiny inline impl via the
    // `md-5` crate would add another dep. Read the file and hash via a
    // hand-rolled MD5 isn't worth it either. GiantSteps just needs to
    // *check* an MD5 from the .md5 sidecar files. Vendor the md5 crate
    // explicitly so we don't grow the optional-dep set further than
    // necessary.
    use ::md5::{Digest as _, Md5};
    let mut f = fs::File::open(path)?;
    let mut hasher = Md5::new();
    io::copy(&mut f, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn extract_tar_gz(src: &Path, dest: &Path) -> Result<()> {
    let f = fs::File::open(src)?;
    let gz = GzDecoder::new(f);
    let mut tar = tar::Archive::new(gz);
    tar.set_overwrite(true);
    tar.unpack(dest).context("untar")?;
    Ok(())
}

pub fn count_files_with_ext(dir: &Path, ext: &str) -> usize {
    fn walk(dir: &Path, ext: &str, out: &mut usize) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, ext, out);
            } else if p.extension().and_then(|s| s.to_str()) == Some(ext) {
                *out += 1;
            }
        }
    }
    let mut n = 0;
    walk(dir, ext, &mut n);
    n
}

pub fn print_env_hint(var: &str, audio_dir: &Path, test_name: &str) {
    let abs = fs::canonicalize(audio_dir).unwrap_or_else(|_| audio_dir.to_path_buf());
    let abs_str = abs.display().to_string();
    println!();
    if cfg!(windows) {
        println!("Set the env var (PowerShell): $env:{var}='{abs_str}'");
        println!("Set the env var (cmd):        set {var}={abs_str}");
    } else {
        println!("Set the env var (bash/zsh):   export {var}='{abs_str}'");
    }
    println!(
        "Then:                         cargo test --features dataset-tests --test {test_name} -- --nocapture"
    );
}
