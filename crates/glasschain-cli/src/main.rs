// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! `glasschain` — command-line interface for the `GlassChain` distributed ledger.
//!
//! # Subcommands
//!
//! | Subcommand        | Description                                              |
//! |:------------------|:---------------------------------------------------------|
//! | `identity-gen`    | Generate a new node identity (standalone or org-issued). |
//! | `contract-deploy` | Deploy a smart contract to a `GlassChain` node.            |
//! | `ledger-inspect`  | Inspect the ledger state (blocks, assets, chain status). |
//! | `backup-scrub`    | Purge expired private payloads from a storage copy (ADR-017). |
//! | `channel-admin`   | Drive the ADR-017 channel-management RPCs as an admin principal. |
//!
//! # Usage
//!
//! ```text
//! glasschain --help
//! glasschain identity-gen --node-id my-node --org PharmaCorp
//! glasschain contract-deploy --contract-id C-001 --buyer-id buyer-1 \
//!     --product-id SKU-001 --max-price 5000 --min-qty 100 --max-qty 1000 \
//!     --max-lead-days 14 --dry-run
//! glasschain ledger-inspect --endpoint http://node1.example.com:9000 --gtin 07891234567890
//! ```

mod commands;

use clap::{Parser, Subcommand};

// ── CLI definition ─────────────────────────────────────────────────────────────

/// Top-level CLI struct parsed by `clap`.
#[derive(Parser, Debug)]
#[command(
    name = "glasschain",
    about = "GlassChain CLI — manage identities, deploy contracts, inspect the ledger",
    version = env!("CARGO_PKG_VERSION"),
    author,
)]
struct Cli {
    /// Log level filter passed to `env_logger`.
    ///
    /// Accepted values (case-insensitive): `error`, `warn`, `info`, `debug`, `trace`.
    #[arg(long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

// ── Subcommand enum ────────────────────────────────────────────────────────────

/// All subcommands supported by the `glasschain` binary.
#[derive(Subcommand, Debug)]
enum Commands {
    /// Generate a new node identity (with optional organisation Root CA).
    ///
    /// When `--org` is provided, a self-signed Root CA is created for that
    /// organisation and a member certificate is issued and embedded in the
    /// output JSON.
    IdentityGen(commands::identity::IdentityGenArgs),

    /// Deploy a smart contract to a `GlassChain` node.
    ///
    /// Builds a `SmartContractDef` transaction from the provided flags.
    /// Use `--dry-run` to inspect the JSON without submitting.
    ContractDeploy(commands::contract::ContractDeployArgs),

    /// Inspect the ledger state (blocks, assets, or chain status).
    ///
    /// Shows which gRPC call would be issued against the configured endpoint.
    LedgerInspect(commands::inspect::LedgerInspectArgs),

    /// Prune expired private payloads from a (copied) storage directory
    /// before archiving it (ADR-017). Run it on a *copy*: the live node's
    /// D5 sweep covers the running store, physical backups do not.
    BackupScrub(commands::backup_scrub::BackupScrubArgs),

    /// Administer channel membership over gRPC as a certificate-bound admin
    /// principal (ADR-017): create a collection, add or remove a member.
    ChannelAdmin(commands::channel_admin::ChannelAdminArgs),
}

// ── Entry point ────────────────────────────────────────────────────────────────

/// Entry point — initialises logging and dispatches to the chosen subcommand.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialise env_logger with the user-supplied level (or the default "info").
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&cli.log_level))
        .init();

    log::debug!("glasschain CLI starting — command: {:?}", cli.command);

    dispatch(cli.command, &mut std::io::stdout())
}

/// Route one parsed subcommand to its implementation. Separated from `main`
/// so the dispatch arms stay unit-testable without spawning the binary.
fn dispatch(command: Commands, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    match command {
        Commands::IdentityGen(args) => commands::identity::run(args, out)?,
        Commands::ContractDeploy(args) => commands::contract::run(args, out)?,
        Commands::LedgerInspect(args) => commands::inspect::run(&args, out)?,
        Commands::BackupScrub(args) => commands::backup_scrub::run(args, out)?,
        Commands::ChannelAdmin(args) => commands::channel_admin::run(args, out)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_runs_the_offline_subcommands() {
        let mut sink: Vec<u8> = Vec::new();

        // LedgerInspect with no server touched: prints the call it would make.
        let cli = Cli::parse_from([
            "glasschain",
            "ledger-inspect",
            "--endpoint",
            "http://127.0.0.1:1",
            "--gtin",
            "07891234567890",
        ]);
        dispatch(cli.command, &mut sink).expect("offline inspect");

        // Identity generation is fully offline.
        let cli = Cli::parse_from(["glasschain", "identity-gen", "--node-id", "cov-node"]);
        dispatch(cli.command, &mut sink).expect("offline identity-gen");

        // Contract deploy with --dry-run never touches the network.
        let cli = Cli::parse_from([
            "glasschain",
            "contract-deploy",
            "--contract-id",
            "C-001",
            "--buyer-id",
            "buyer-1",
            "--product-id",
            "SKU-001",
            "--max-price",
            "5000",
            "--min-qty",
            "100",
            "--max-qty",
            "1000",
            "--max-lead-days",
            "14",
            "--dry-run",
        ]);
        dispatch(cli.command, &mut sink).expect("offline contract deploy");

        assert!(!sink.is_empty(), "the subcommands must have printed");
    }
}
