// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! CLI sub-command modules for the `glasschain` binary.
//!
//! Each module corresponds to one top-level subcommand and exposes:
//! - An `*Args` struct (derived from [`clap::Args`]) that owns the parsed flags.
//! - A `run(args)` (or `async run(args)`) function that implements the command logic.

pub mod backup_scrub;
pub mod channel_admin;
pub mod contract;
pub mod identity;
pub mod inspect;
