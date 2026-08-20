//! `contract-deploy` sub-command — deploy a smart contract to a `GlassChain` node.
//!
//! Builds a [`SmartContractDef`] from the provided CLI flags, wraps it in a
//! [`Transaction`], serialises the result to pretty-printed JSON, and either
//! prints the JSON (dry-run mode) or instructs the user to submit it to the
//! `LedgerService.SubmitTransaction` gRPC endpoint.

use anyhow::Result;
use clap::Args;
use glasschain_core::{PurchaseConditions, Transaction, TransactionKind};
use glasschain_sdk::GlasschainClient;

/// Arguments for the `contract-deploy` sub-command.
#[derive(Args, Debug)]
pub struct ContractDeployArgs {
    /// Unique contract identifier (e.g. `CONTRACT-PHARMA-001`).
    #[arg(long)]
    pub contract_id: String,

    /// Buyer organisation node ID that will own the contract.
    #[arg(long)]
    pub buyer_id: String,

    /// Product ID the contract targets (e.g. a SKU or GTIN).
    #[arg(long)]
    pub product_id: String,

    /// Maximum acceptable price per unit in minor currency units
    /// (e.g. centavos for BRL, cents for USD).
    #[arg(long)]
    pub max_price: u64,

    /// Minimum quantity the buyer wishes to order per execution.
    #[arg(long)]
    pub min_qty: u64,

    /// Maximum cumulative quantity across the full lifetime of the contract.
    /// The contract engine marks the contract as `Fulfilled` once this cap is
    /// reached.
    #[arg(long)]
    pub max_qty: u64,

    /// Maximum acceptable lead time in calendar days.
    #[arg(long)]
    pub max_lead_days: u32,

    /// ISO-4217 currency code used in the contract (e.g. `BRL`, `USD`).
    #[arg(long, default_value = "BRL")]
    pub currency: String,

    /// Print the transaction JSON to stdout without submitting it to a node.
    /// Useful for inspection and testing before deployment.
    #[arg(long)]
    pub dry_run: bool,
}

/// Execute the `contract-deploy` command.
///
/// Constructs a [`glasschain_core::SmartContractDef`] from `args`, serialises
/// it via [`GlasschainClient::build_smart_contract_tx`], and either writes the
/// JSON to `out` (when `--dry-run` is set) or shows the gRPC submission
/// instructions.
///
/// # Errors
///
/// Returns an error if JSON serialisation fails (should be unreachable in
/// practice) or if writing to `out` fails.
#[allow(clippy::needless_pass_by_value)] // clap gives us owned Args; consuming them is idiomatic
pub fn run(args: ContractDeployArgs, out: &mut dyn std::io::Write) -> Result<()> {
    log::info!(
        "contract-deploy: contract_id={}, buyer={}, product={}, dry_run={}",
        args.contract_id,
        args.buyer_id,
        args.product_id,
        args.dry_run,
    );

    let conditions = PurchaseConditions {
        max_price_per_unit: args.max_price,
        min_quantity: args.min_qty,
        max_quantity: args.max_qty,
        max_lead_time_days: args.max_lead_days,
        preferred_seller_id: None,
        currency: args.currency.clone(),
        auto_execute: true,
    };

    let tx_json = GlasschainClient::build_smart_contract_tx(
        &args.contract_id,
        &args.buyer_id,
        &args.product_id,
        conditions,
    )?;

    if args.dry_run {
        writeln!(out, "=== Dry Run — Contract Deployment Transaction ===")?;
        writeln!(out)?;
        writeln!(out, "{tx_json}")?;
        writeln!(out)?;
        writeln!(out, "(dry-run: transaction was NOT submitted to any node)")?;
    } else {
        writeln!(out, "=== Contract Deployment Transaction ===")?;
        writeln!(out)?;
        writeln!(out, "{tx_json}")?;
        writeln!(out)?;
        writeln!(out, "To deploy, submit the JSON above to:")?;
        writeln!(out, "  gRPC service  : LedgerService")?;
        writeln!(out, "  RPC method    : SubmitTransaction")?;
        writeln!(out)?;
        writeln!(out, "Example (grpcurl):")?;
        writeln!(
            out,
            "  grpcurl -d '{{\"transaction_json\": ...}}' \
             <NODE_HOST>:<NODE_PORT> glasschain.v1.LedgerService/SubmitTransaction"
        )?;
    }

    // Print a compact summary of the contract parameters.
    writeln!(out, "--- Contract Summary ---")?;
    writeln!(out, "  Contract ID  : {}", args.contract_id)?;
    writeln!(out, "  Buyer        : {}", args.buyer_id)?;
    writeln!(out, "  Product      : {}", args.product_id)?;
    writeln!(
        out,
        "  Price limit  : {} {} / unit",
        args.max_price, args.currency
    )?;
    writeln!(
        out,
        "  Quantity     : {} – {} units (lifetime cap)",
        args.min_qty, args.max_qty
    )?;
    writeln!(out, "  Lead time    : ≤ {} days", args.max_lead_days)?;
    writeln!(out, "  Auto-execute : true")?;

    // Validate that a well-formed Transaction was produced by round-tripping
    // the JSON (cheap, catches any edge-case serialisation bugs at runtime).
    let tx: Transaction = serde_json::from_str(&tx_json)?;
    if matches!(&tx.kind, TransactionKind::ContractCreation(_)) {
        log::info!("Contract transaction validated — tx_id={}", tx.id);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(dry_run: bool) -> ContractDeployArgs {
        ContractDeployArgs {
            contract_id: "CONTRACT-TEST-001".into(),
            buyer_id: "buyer-1".into(),
            product_id: "SKU-001".into(),
            max_price: 5_000,
            min_qty: 100,
            max_qty: 1_000,
            max_lead_days: 14,
            currency: "BRL".into(),
            dry_run,
        }
    }

    fn run_captured(args: ContractDeployArgs) -> String {
        let mut out = Vec::new();
        run(args, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    /// Extract the pretty-printed transaction JSON that sits between the
    /// closing `===` of the header line and the first blank line after it.
    fn emitted_tx_json(text: &str) -> String {
        text.split("===")
            .nth(2)
            .unwrap()
            .trim_start()
            .split("\n\n")
            .next()
            .unwrap()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn dry_run_prints_tx_json_without_submit_instructions() {
        let text = run_captured(args(true));

        // Dry-run branch: no submission instructions.
        assert!(text.contains("=== Dry Run — Contract Deployment Transaction ==="));
        assert!(text.contains("(dry-run: transaction was NOT submitted to any node)"));
        assert!(!text.contains("SubmitTransaction"));
        assert!(!text.contains("grpcurl"));

        // PurchaseConditions assembly reflected in the human summary.
        assert!(text.contains("  Contract ID  : CONTRACT-TEST-001"));
        assert!(text.contains("  Buyer        : buyer-1"));
        assert!(text.contains("  Product      : SKU-001"));
        assert!(text.contains("  Price limit  : 5000 BRL / unit"));
        assert!(text.contains("  Quantity     : 100 – 1000 units (lifetime cap)"));
        assert!(text.contains("  Lead time    : ≤ 14 days"));
        assert!(text.contains("  Auto-execute : true"));
    }

    #[test]
    fn submit_mode_prints_grpc_submit_instructions() {
        let text = run_captured(args(false));

        // Submit branch.
        assert!(text.contains("=== Contract Deployment Transaction ==="));
        assert!(text.contains("To deploy, submit the JSON above to:"));
        assert!(text.contains("  gRPC service  : LedgerService"));
        assert!(text.contains("  RPC method    : SubmitTransaction"));
        assert!(text.contains("glasschain.v1.LedgerService/SubmitTransaction"));
        assert!(!text.contains("(dry-run: transaction was NOT submitted to any node)"));
    }

    #[test]
    fn emitted_transaction_round_trips_to_contract_creation() {
        let text = run_captured(args(true));
        let tx: Transaction = serde_json::from_str(&emitted_tx_json(&text)).unwrap();

        match tx.kind {
            TransactionKind::ContractCreation(def) => {
                assert_eq!(def.contract_id, "CONTRACT-TEST-001");
                assert_eq!(def.buyer_id, "buyer-1");
                assert_eq!(def.product_id, "SKU-001");
                assert!(def.wasm_code_b64.is_none());
                let conditions = def.conditions;
                assert_eq!(conditions.max_price_per_unit, 5_000);
                assert_eq!(conditions.min_quantity, 100);
                assert_eq!(conditions.max_quantity, 1_000);
                assert_eq!(conditions.max_lead_time_days, 14);
                assert_eq!(conditions.currency, "BRL");
                assert!(conditions.auto_execute);
                assert!(conditions.preferred_seller_id.is_none());
            }
            other => panic!("expected ContractCreation, got {other:?}"),
        }
    }
}
