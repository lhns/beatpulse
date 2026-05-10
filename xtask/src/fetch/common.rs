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

/// Download `url` → `dest` with a progress bar. Resume-aware: if the
/// destination already exists with `0 < size < expected_total`, sends
/// a `Range: bytes=<size>-` request and appends. If the file is
/// already complete (matches `Content-Length` from HEAD) returns
/// immediately. Falls back to a full re-download if the server doesn't
/// honour ranges (returns 200 instead of 206).
///
/// Bubbles up reqwest errors and HTTP 4xx/5xx as Err. The expected use
/// is to call this in a small retry loop (the network is the network).
pub fn download(client: &reqwest::blocking::Client, url: &str, dest: &Path) -> Result<()> {
    // 1. Probe expected size via HEAD. Some servers return
    // Content-Length: 0 or omit it on HEAD; treat that as "unknown"
    // rather than authoritative — *never* delete a cached file based
    // on an unknown total.
    let head = client.head(url).send()?;
    if !head.status().is_success() {
        bail!("HEAD {url} returned HTTP {}", head.status());
    }
    let expected_total = head.content_length().filter(|&n| n > 0);
    let accepts_range = head
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("bytes"))
        .unwrap_or(false);

    // 2. Decide where to start (resume vs fresh).
    let existing = fs::metadata(dest).ok().map(|m| m.len()).unwrap_or(0);
    let mut start = 0u64;
    if let Some(total) = expected_total {
        if existing == total {
            eprintln!(
                "[download] already complete: {} ({existing} bytes)",
                dest.display()
            );
            return Ok(());
        }
        if existing > total {
            eprintln!("[download] cached file too large ({existing} > {total}); starting over");
            let _ = fs::remove_file(dest);
        } else if existing > 0 && accepts_range {
            start = existing;
            eprintln!(
                "[download] resuming at byte {start}/{total} ({:.1}% done)",
                start as f64 / total as f64 * 100.0
            );
        } else if existing > 0 {
            eprintln!("[download] server doesn't advertise Accept-Ranges; restarting from 0");
            let _ = fs::remove_file(dest);
        }
    } else if existing > 0 && accepts_range {
        // Total unknown but cache exists. Try to resume optimistically.
        start = existing;
        eprintln!("[download] total unknown; optimistically resuming at byte {start}");
    }

    // 3. Issue GET (with Range if resuming).
    let mut req = client.get(url);
    if start > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={start}-"));
    }
    let mut resp = req.send()?;
    let status = resp.status();
    if !status.is_success() {
        bail!("GET {url} returned HTTP {status}");
    }
    // If we asked for a range but got 200 OK, the server ignored
    // Range — start over from byte 0.
    if start > 0 && status.as_u16() != 206 {
        eprintln!("[download] server returned {status} for Range request; restarting from 0");
        let _ = fs::remove_file(dest);
        start = 0;
    }

    // 4. Open destination in the right mode + set up progress bar.
    let total = expected_total.unwrap_or(0);
    let pb = if total > 0 {
        let pb = ProgressBar::new(total);
        pb.set_style(
            ProgressStyle::with_template("{bar:40.cyan/blue} {bytes}/{total_bytes} ({eta}) {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_bar()),
        );
        pb.set_position(start);
        pb.set_message(if start > 0 { "resuming" } else { "downloading" });
        pb
    } else {
        let pb = ProgressBar::new_spinner();
        pb.set_message("downloading (size unknown)");
        pb
    };

    let mut out = if start > 0 {
        fs::OpenOptions::new().append(true).open(dest)?
    } else {
        fs::File::create(dest)?
    };
    let mut buf = [0u8; 64 * 1024];
    let mut written = start;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        written += n as u64;
        pb.set_position(written);
    }
    pb.finish_with_message(format!("downloaded → {}", dest.display()));

    // 5. Final sanity check.
    if let Some(total) = expected_total {
        if written != total {
            bail!(
                "download truncated: wrote {written} bytes, expected {total}. \
                 Re-run to resume."
            );
        }
    }
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
