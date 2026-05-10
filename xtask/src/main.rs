// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

#[cfg(feature = "fetch-datasets")]
mod fetch;

fn main() -> nih_plug_xtask::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first().cloned() {
        // `cargo xtask fetch <dataset> ...`
        if first == "fetch" {
            args.remove(0);
            return run_fetch(&args);
        }
        // Back-compat: `cargo xtask fetch-<dataset> ...` is rewritten to
        // `fetch <dataset> ...`.
        if let Some(dataset) = first.strip_prefix("fetch-") {
            let dataset = dataset.to_string();
            args.remove(0);
            let mut new_args = vec![dataset];
            new_args.extend(args);
            return run_fetch(&new_args);
        }
    }
    nih_plug_xtask::main()
}

#[cfg(feature = "fetch-datasets")]
fn run_fetch(args: &[String]) -> nih_plug_xtask::Result<()> {
    fetch::run(args)
}

#[cfg(not(feature = "fetch-datasets"))]
fn run_fetch(_args: &[String]) -> nih_plug_xtask::Result<()> {
    Err(std::io::Error::other(
        "fetch subcommand requires the `fetch-datasets` feature. \
         Use `cargo xtask-fetch-{ballroom,giantsteps,smc} …` (see \
         .cargo/config.toml) instead of `cargo xtask fetch …`.",
    )
    .into())
}
