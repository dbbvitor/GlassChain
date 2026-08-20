//! `ledger-inspect` subcommand — query the `GlassChain` ledger state.
//!
//! This command documents the gRPC calls that *would* be issued against a live
//! node.  Actual network I/O is intentionally out of scope for this phase;
//! the output shows operators exactly what request would be sent so they can
//! validate flags before wiring up a running node.

use anyhow::Result;
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
/// Writes a human-readable summary of the gRPC call that would be made
/// against the configured endpoint to `out`.  Priority order when multiple
/// flags are provided: `--block` > `--gtin` > (no flags → chain status).
///
/// # Errors
///
/// Returns an error only if writing to `out` fails.
pub fn run(args: &LedgerInspectArgs, out: &mut dyn std::io::Write) -> Result<()> {
    writeln!(out, "GlassChain Ledger Inspector")?;
    writeln!(out, "  Endpoint : {}", args.endpoint)?;
    writeln!(out)?;

    if let Some(index) = args.block {
        // ── Block query ────────────────────────────────────────────────────
        writeln!(out, "  Action   : GetBlock")?;
        writeln!(
            out,
            "  gRPC     : LedgerService.GetBlock {{ index: {index} }}"
        )?;
        writeln!(out)?;
        writeln!(
            out,
            "  Would send GetBlock(index={index}) to {}",
            args.endpoint
        )?;
        log::info!(
            "ledger-inspect: would query GetBlock(index={index}) at {}",
            args.endpoint,
        );
    } else if let Some(ref gtin) = args.gtin {
        // ── Asset history query ────────────────────────────────────────────
        writeln!(out, "  Action   : QueryAssetHistory")?;
        writeln!(out, "  GTIN     : {gtin}")?;
        if let Some(ref serial) = args.serial {
            writeln!(out, "  Serial   : {serial}")?;
            writeln!(
                out,
                "  gRPC     : LedgerService.QueryAssetHistory {{ gtin: \"{gtin}\", serial_number: \"{serial}\" }}"
            )?;
            log::info!(
                "ledger-inspect: would query QueryAssetHistory(gtin={gtin}, serial_number={serial}) at {}",
                args.endpoint,
            );
        } else {
            writeln!(
                out,
                "  gRPC     : LedgerService.QueryAssetHistory {{ gtin: \"{gtin}\" }}"
            )?;
            log::info!(
                "ledger-inspect: would query QueryAssetHistory(gtin={gtin}) at {}",
                args.endpoint,
            );
        }
        writeln!(out)?;
        writeln!(
            out,
            "  Would send QueryAssetHistory(gtin={gtin}) to {}",
            args.endpoint,
        )?;
    } else {
        // ── Chain status (default) ─────────────────────────────────────────
        writeln!(out, "  Action   : GetChainStatus")?;
        writeln!(out, "  gRPC     : LedgerService.GetChainStatus {{}}")?;
        writeln!(out)?;
        writeln!(out, "  Would send GetChainStatus to {}", args.endpoint)?;
        log::info!(
            "ledger-inspect: would query GetChainStatus at {}",
            args.endpoint,
        );
    }

    writeln!(out)?;
    writeln!(
        out,
        "Tip: pass --block N, --gtin <GTIN>, or omit flags for chain status."
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_captured(args: &LedgerInspectArgs) -> String {
        let mut out = Vec::new();
        run(args, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn args(block: Option<u64>, gtin: Option<&str>, serial: Option<&str>) -> LedgerInspectArgs {
        LedgerInspectArgs {
            endpoint: "http://node-1:9000".into(),
            block,
            gtin: gtin.map(str::to_owned),
            serial: serial.map(str::to_owned),
        }
    }

    #[test]
    fn block_flag_takes_priority_over_gtin_and_serial() {
        let text = run_captured(&args(Some(7), Some("07891234567890"), Some("UNIT-42")));
        assert!(text.contains("  Action   : GetBlock"));
        assert!(text.contains("  gRPC     : LedgerService.GetBlock { index: 7 }"));
        assert!(text.contains("  Would send GetBlock(index=7) to http://node-1:9000"));
        assert!(!text.contains("QueryAssetHistory"));
        assert!(!text.contains("GetChainStatus"));
    }

    #[test]
    fn gtin_with_serial_narrows_history_query() {
        let text = run_captured(&args(None, Some("07891234567890"), Some("UNIT-42")));
        assert!(text.contains("  Action   : QueryAssetHistory"));
        assert!(text.contains("  GTIN     : 07891234567890"));
        assert!(text.contains("  Serial   : UNIT-42"));
        assert!(text.contains(
            "  gRPC     : LedgerService.QueryAssetHistory { gtin: \"07891234567890\", serial_number: \"UNIT-42\" }"
        ));
        assert!(!text.contains("GetBlock"));
    }

    #[test]
    fn gtin_without_serial_omits_serial() {
        let text = run_captured(&args(None, Some("07891234567890"), None));
        assert!(text.contains("  GTIN     : 07891234567890"));
        assert!(text
            .contains("  gRPC     : LedgerService.QueryAssetHistory { gtin: \"07891234567890\" }"));
        assert!(!text.contains("Serial"));
        assert!(!text.contains("GetBlock"));
    }

    #[test]
    fn no_flags_reports_chain_status() {
        let text = run_captured(&args(None, None, None));
        assert!(text.contains("  Action   : GetChainStatus"));
        assert!(text.contains("  gRPC     : LedgerService.GetChainStatus {}"));
        assert!(text.contains("  Would send GetChainStatus to http://node-1:9000"));
        assert!(!text.contains("GetBlock"));
        assert!(!text.contains("QueryAssetHistory"));
    }
}
