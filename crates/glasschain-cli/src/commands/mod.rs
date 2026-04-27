//! CLI sub-command modules for the `glasschain` binary.
//!
//! Each module corresponds to one top-level subcommand and exposes:
//! - An `*Args` struct (derived from [`clap::Args`]) that owns the parsed flags.
//! - A `run(args)` (or `async run(args)`) function that implements the command logic.

pub mod contract;
pub mod identity;
pub mod inspect;
