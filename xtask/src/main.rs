// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 BeatPulse contributors
//
// This file is part of BeatPulse.
//
// BeatPulse is free software: you can redistribute it and/or modify it under
// the terms of the GNU General Public License version 3 as published by the
// Free Software Foundation.

#[cfg(feature = "fetch-ballroom")]
mod fetch_ballroom;

fn main() -> nih_plug_xtask::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("fetch-ballroom") {
        args.remove(0);
        return run_fetch_ballroom(&args);
    }
    nih_plug_xtask::main()
}

#[cfg(feature = "fetch-ballroom")]
fn run_fetch_ballroom(args: &[String]) -> nih_plug_xtask::Result<()> {
    fetch_ballroom::run(args)
}

#[cfg(not(feature = "fetch-ballroom"))]
fn run_fetch_ballroom(_args: &[String]) -> nih_plug_xtask::Result<()> {
    Err(std::io::Error::other(
        "fetch-ballroom requires the `fetch-ballroom` feature. \
         Use `cargo xtask-fetch-ballroom …` (see .cargo/config.toml) \
         instead of `cargo xtask fetch-ballroom …`.",
    )
    .into())
}
