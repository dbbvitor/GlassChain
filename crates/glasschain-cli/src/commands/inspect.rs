//! `ledger-inspect` subcommand — query the `GlassChain` ledger state.
//!
//! This command documents the gRPC calls that *would* be issued against a live
//! node.  Actual network I/O is intentionally out of scope for this phase;
//! the output shows operators exactly what request would be sent so they can
//! validate flags before wiring up a running node.

use clap::Args;

/// Arguments for the `ledger-inspect` subcommand.
#[derive(Args, Debug)]
pub struct LedgerInspectArgs {
    /// gRPC server endpoint to query.
    #[arg(long, default_value = "http://localhost:9000")]
    pub endpoint: String,

    /// Block index to inspect.
    ///
    /// When provided, shows what a `LedgerService.GetBlock(index = N)` call
    /// would return from the remote node.  Omit to show chain status instead.
    #[arg(long)]
    pub block: Option<u64>,

    /// Asset GTIN (Global Trade Item Number) to query history for.
    ///
    /// When provided, shows what a `LedgerService.QueryAssetHistory(gtin = …)`
    /// call would return.  Can be combined with `--serial` to narrow the query.
    #[arg(long)]
    pub gtin: Option<String>,

    /// Asset serial number filter.
    ///
    /// When combined with `--gtin`, restricts the history query to a single
    /// serialised unit rather than the entire GTIN batch.
    #[arg(long)]
    pub serial: Option<String>,
}

/// Execute the `ledger-inspect` subcommand.
///
/// Prints a human-readable summary of the gRPC call that would be made
/// against the configured endpoint.  Priority order when multiple flags are
/// provided: `--block` > `--gtin` > (no flags → chain status).
///
/// # Errors
///
/// Currently infallible.  Future releases will return errors when the gRPC
/// call to the remote node fails.
pub fn run(args: &LedgerInspectArgs) {
    println!("GlassChain Ledger Inspector");
    println!("  Endpoint : {}", args.endpoint);
    println!();

    if let Some(index) = args.block {
        // ── Block query ────────────────────────────────────────────────────
        println!("  Action   : GetBlock");
        println!("  gRPC     : LedgerService.GetBlock {{ index: {index} }}");
        println!();
        println!("  Would send GetBlock(index={index}) to {}", args.endpoint);
        log::info!(
            "ledger-inspect: would query GetBlock(index={index}) at {}",
            args.endpoint,
        );
    } else if let Some(ref gtin) = args.gtin {
        // ── Asset history query ────────────────────────────────────────────
        println!("  Action   : QueryAssetHistory");
        println!("  GTIN     : {gtin}");
        if let Some(ref serial) = args.serial {
            println!("  Serial   : {serial}");
            println!(
                "  gRPC     : LedgerService.QueryAssetHistory {{ gtin: \"{gtin}\", serial_number: \"{serial}\" }}"
            );
            log::info!(
                "ledger-inspect: would query QueryAssetHistory(gtin={gtin}, serial_number={serial}) at {}",
                args.endpoint,
            );
        } else {
            println!("  gRPC     : LedgerService.QueryAssetHistory {{ gtin: \"{gtin}\" }}");
            log::info!(
                "ledger-inspect: would query QueryAssetHistory(gtin={gtin}) at {}",
                args.endpoint,
            );
        }
        println!();
        println!(
            "  Would send QueryAssetHistory(gtin={gtin}) to {}",
            args.endpoint,
        );
    } else {
        // ── Chain status (default) ─────────────────────────────────────────
        println!("  Action   : GetChainStatus");
        println!("  gRPC     : LedgerService.GetChainStatus {{}}");
        println!();
        println!("  Would send GetChainStatus to {}", args.endpoint);
        log::info!(
            "ledger-inspect: would query GetChainStatus at {}",
            args.endpoint,
        );
    }

    println!();
    println!("Tip: pass --block N, --gtin <GTIN>, or omit flags for chain status.");
}
