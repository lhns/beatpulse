// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors

//! `cargo xtask fetch <dataset>` — downloads open-licensed beat /
//! tempo evaluation datasets into a gitignored local directory so the
//! integration tests can run without manual file wrangling.
//!
//! The script does **not** redistribute the datasets (ADR-0014); it
//! merely automates the download. Users obtain the data under whatever
//! terms the upstream sources publish.

pub mod ballroom;
mod common;
pub mod giantsteps;
pub mod smc;

use anyhow::{bail, Result};

pub fn run(args: &[String]) -> Result<()> {
    let Some(dataset) = args.first() else {
        print_top_help();
        bail!("missing <dataset> argument");
    };
    let rest = &args[1..];
    match dataset.as_str() {
        "ballroom" => ballroom::run(rest),
        "giantsteps" => giantsteps::run(rest),
        "smc" => smc::run(rest),
        "--help" | "-h" => {
            print_top_help();
            Ok(())
        }
        other => bail!("unknown dataset {other:?}; pass --help for the list"),
    }
}

fn print_top_help() {
    println!(
        "Usage: cargo xtask fetch <dataset> [dataset-options]\n\n\
         Datasets:\n  \
         ballroom     Ballroom Dataset (685 × 30s WAVs, per-beat ground truth)\n  \
         giantsteps   GiantSteps Tempo (664 × 2-min EDM previews, tempo-only)\n  \
         smc          SMC_MIREX (217 × 40s adversarial clips, per-beat)\n\n\
         Each dataset accepts --dest, --force, --help. See e.g.\n  \
         cargo xtask fetch giantsteps --help"
    );
}
