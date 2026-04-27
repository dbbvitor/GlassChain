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
/// it via [`GlasschainClient::build_smart_contract_tx`], and either prints the
/// JSON (when `--dry-run` is set) or shows the gRPC submission instructions.
///
/// # Errors
///
/// Returns an error if JSON serialisation fails (should be unreachable in
/// practice).
#[allow(clippy::needless_pass_by_value)] // clap gives us owned Args; consuming them is idiomatic
pub fn run(args: ContractDeployArgs) -> Result<()> {
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
        println!("=== Dry Run — Contract Deployment Transaction ===");
        println!();
        println!("{tx_json}");
        println!();
        println!("(dry-run: transaction was NOT submitted to any node)");
    } else {
        println!("=== Contract Deployment Transaction ===");
        println!();
        println!("{tx_json}");
        println!();
        println!("To deploy, submit the JSON above to:");
        println!("  gRPC service  : LedgerService");
        println!("  RPC method    : SubmitTransaction");
        println!();
        println!("Example (grpcurl):");
        println!(
            "  grpcurl -d '{{\"transaction_json\": ...}}' \
             <NODE_HOST>:<NODE_PORT> glasschain.LedgerService/SubmitTransaction"
        );
    }

    // Print a compact summary of the contract parameters.
    println!("--- Contract Summary ---");
    println!("  Contract ID  : {}", args.contract_id);
    println!("  Buyer        : {}", args.buyer_id);
    println!("  Product      : {}", args.product_id);
    println!(
        "  Price limit  : {} {} / unit",
        args.max_price, args.currency
    );
    println!(
        "  Quantity     : {} – {} units (lifetime cap)",
        args.min_qty, args.max_qty
    );
    println!("  Lead time    : ≤ {} days", args.max_lead_days);
    println!("  Auto-execute : true");

    // Validate that a well-formed Transaction was produced by round-tripping
    // the JSON (cheap, catches any edge-case serialisation bugs at runtime).
    let _: Transaction = serde_json::from_str(&tx_json)?;
    if let Ok(tx) = serde_json::from_str::<Transaction>(&tx_json) {
        if matches!(tx.kind, TransactionKind::ContractCreation(_)) {
            log::info!("Contract transaction validated — tx_id={}", tx.id);
        }
    }

    Ok(())
}
