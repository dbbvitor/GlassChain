// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
use glasschain_core::{
    endorsement::Principal, InventoryUpdate, PurchaseConditions, PurchaseOrder, SmartContractDef,
    SupplyOffer, TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
};
use glasschain_identity::{CertChainVerifier, Identity, MspEndorsementProvider, Organization};
use glasschain_network::{Node, NodeEvent};
use glasschain_rpc::{AdminGate, GlasschainServer};
use glasschain_storage::RedbStorageProvider;
use glasschain_vm::WasmExecutionProvider;
use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};

/// Parse a decimal price string like "12.50" into minor currency units (cents).
fn parse_price(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if !frac.chars().all(|c| c.is_ascii_digit()) || frac.len() > 2 {
        return None;
    }
    let units: u64 = whole.parse().ok()?;
    let frac_units: u64 = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 10,
        2 => frac.parse::<u64>().ok()?,
        _ => return None,
    };
    units.checked_mul(100)?.checked_add(frac_units)
}

/// Print usage information.
/// Build a certificate verifier from the trust store at `path` — a PEM file
/// or a directory of `*.pem` (anchors/intermediates) and `*.crl` (ADR-013)
/// files. Shared by startup and the `reload-trust-store` REPL command
/// (zero-trust residual ZT-R3): both load the same way.
///
/// Returns the verifier plus `(files_loaded, crls_loaded)` for the startup
/// log line.
fn build_trust_store_verifier(
    org: &str,
    root_pem: &str,
    path: &str,
) -> Result<(CertChainVerifier, usize, usize), String> {
    let mut verifier = CertChainVerifier::from_pem(org, root_pem)
        .map_err(|e| format!("cannot build certificate verifier for `{org}`: {e}"))?;
    let load = |v: &mut CertChainVerifier, p: &std::path::Path| {
        v.add_federation_root_file(p).map_err(|e| e.to_string())
    };
    let load_crl = |v: &mut CertChainVerifier, p: &std::path::Path| {
        v.add_crl_file(p).map_err(|e| e.to_string())
    };
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => {
            let mut added = 0usize;
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(path)
                .map_err(|e| format!("cannot read trust store directory `{path}`: {e}"))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .is_some_and(|ext| ext == "pem" || ext == "crl")
                })
                .collect();
            files.sort();
            for file in &files {
                // `*.crl` files hold the organizations' signed CRLs (ADR-013);
                // `*.pem` files hold Root/intermediate CA certificates.
                let res = if file.extension().is_some_and(|ext| ext == "crl") {
                    load_crl(&mut verifier, file)
                } else {
                    load(&mut verifier, file)
                };
                res.map_err(|e| {
                    format!("cannot load trust anchor from `{}`: {e}", file.display())
                })?;
                added += 1;
            }
            let crls = verifier.crl_count();
            Ok((verifier, added, crls))
        }
        Ok(_) => {
            load(&mut verifier, std::path::Path::new(path))?;
            let crls = verifier.crl_count();
            Ok((verifier, 1, crls))
        }
        Err(e) => Err(format!("cannot load trust store at `{path}`: {e}")),
    }
}

/// Load-or-create the operator-owned identity file (ADR-018).
///
/// Returns `(organization, identity, freshly_created)`.
fn load_or_create_identity_file(
    path: &std::path::Path,
    org_name: &str,
    identity_name: &str,
) -> Result<(Organization, Arc<Identity>, bool), String> {
    if path.exists() {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read identity file `{}`: {e}", path.display()))?;
        let organization = Organization::import_json(&json)
            .map_err(|e| format!("identity file `{}` is corrupt: {e}", path.display()))?;
        let member = organization
            .get_member(identity_name)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "identity file `{}` holds organization `{}` but no member \
                     `{identity_name}`; fix --identity-node-id or re-provision",
                    path.display(),
                    organization.name
                )
            })?;
        if organization.name != org_name {
            return Err(format!(
                "--org '{org_name}' does not match the identity file's organization \
                 '{}'",
                organization.name
            ));
        }
        Ok((organization, Arc::new(member), false))
    } else {
        let mut organization = Organization::new(org_name)
            .map_err(|e| format!("cannot create organization `{org_name}`: {e}"))?;
        let identity = organization
            .issue_identity(identity_name)
            .map_err(|e| format!("cannot issue identity `{identity_name}`: {e}"))?
            .clone();
        let json = organization
            .export_json()
            .map_err(|e| format!("cannot serialize identity material: {e}"))?;
        std::fs::write(path, &json)
            .map_err(|e| format!("cannot write identity file `{}`: {e}", path.display()))?;
        // Owner read/write only — the file carries private keys.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok((organization, Arc::new(identity), true))
    }
}

fn usage() {
    eprintln!(
        r#"GlassChain Node

USAGE:
    glasschain-node [OPTIONS]

OPTIONS:
    --id <NODE_ID>          Node identifier (default: "node-1")
    --listen <ADDR>         Listen address (default: "0.0.0.0:8000")
    --peer <ADDR>           Seed peer address (repeatable)
    --difficulty <N>        PoW difficulty – number of leading zeros (default: 2)
    --storage-path <PATH>   Directory for persistent block storage (optional).
                            When provided, the chain is reloaded from disk on restart.
    --org <NAME>            Organization name for issuing an identity-backed TLS certificate.
    --identity-node-id <ID> Node ID to embed in the issued TLS identity certificate.
                            Defaults to the value passed to --id.
    --trust-store <PATH>    PEM file or directory of *.pem files holding the Root CA
                            certificates of the peer organizations to trust (ADR-011).
                            Requires --org. Without it, peer organizations are NOT
                            certificate-verified.
    --identity-file <PATH>  Operator-owned durable identity file (ADR-018): created
                            on first start, loaded after — the same identity key,
                            certificate and Root CA across restarts, so persisted
                            TOFU pins keep verifying. Requires --org. Without it the
                            identity is regenerated every start (dev behaviour).
    --rpc-addr <ADDR>       Address to bind the gRPC server (e.g. "0.0.0.0:50051").
                            When omitted, the gRPC server is not started.
    --help                  Show this help message

INTERACTIVE COMMANDS (after startup):
    supply   <seller> <product_id> <product_name> <qty> <price> <lead_days> <currency>
        Post a supply offer to the ledger.

    order <buyer> <seller> <product> <qty> <price> <currency>
        Post a manual purchase order.

    contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>
        Create a smart contract for automatic purchasing.

    inventory <owner> <product> <delta> <reason>
        Post an inventory update.

    asset <originator> <product_name> <gtin> <batch> <expiry> <serial> <qty> <event_type>
        Register a traceable asset (Phase 3). Displays the Metadata Trust Score.
        Use "-" for any optional field to leave it empty.

    chain
        Print the current chain summary.

    pending
        Print pending transactions.

    peers
        Print known peers.

    contracts    List all registered smart contracts.

    reload-trust-store <PATH>
        Re-read the federation trust store (PEM/CRL files) and hot-swap the
        certificate verifier (ADR-011/ADR-013, ZT-R3). Requires --org and the
        identity Root CA this node was started with.

    quit / exit
        Shut down the node.
"#
    );
}

/// A parsed, validated REPL command. The stdin loop executes the payload while
/// parsing and validation live here so they can be unit-tested without a node.
#[derive(Debug, PartialEq, Eq)]
enum ReplCommand {
    Help,
    Supply {
        seller: String,
        product_id: String,
        product_name: String,
        qty: u64,
        price: u64,
        lead_days: u32,
        currency: String,
    },
    Order {
        buyer: String,
        seller: String,
        product: String,
        qty: u64,
        price: u64,
        currency: String,
    },
    Contract {
        contract_id: String,
        buyer: String,
        product: String,
        max_price: u64,
        min_qty: u64,
        max_qty: u64,
        max_lead: u32,
        currency: String,
    },
    Inventory {
        owner: String,
        product: String,
        delta: i64,
        reason: String,
    },
    Asset {
        originator: String,
        product_name: String,
        gtin: Option<String>,
        batch: Option<String>,
        expiry: Option<String>,
        serial: Option<String>,
        qty: u64,
        event_type: String,
    },
    Chain,
    Pending,
    Peers,
    Contracts,
    /// Reload the federation trust store (ZT-R3): re-read `--trust-store`
    /// files and hot-swap the verifier (peer paths take effect at the next
    /// Hello; the admin gate sees the new chain/CRLs immediately).
    ReloadTrustStore {
        path: String,
    },
    Quit,
}

/// Parse and validate one REPL line into a command. `Ok(None)` means the line
/// was blank; `Err(msg)` carries the exact message the REPL prints for a bad
/// command (usage lines and numeric-parse errors), preserving prior behavior.
#[allow(clippy::too_many_lines)]
fn parse_command(line: &str) -> Result<Option<ReplCommand>, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let Some(&cmd) = parts.first() else {
        return Ok(None);
    };

    match cmd {
        "help" | "?" => Ok(Some(ReplCommand::Help)),

        "supply" => {
            if parts.len() < 8 {
                return Err(
                    "Usage: supply <seller> <product_id> <product_name> <qty> <price> <lead_days> <currency>"
                        .to_owned(),
                );
            }
            let qty: u64 = if let Ok(v) = parts[4].parse() {
                v
            } else {
                return Err("Invalid quantity".to_owned());
            };
            let price: u64 = if let Some(v) = parse_price(parts[5]) {
                v
            } else {
                return Err("Invalid price (use decimal like 12.50)".to_owned());
            };
            let lead: u32 = if let Ok(v) = parts[6].parse() {
                v
            } else {
                return Err("Invalid lead_days".to_owned());
            };
            Ok(Some(ReplCommand::Supply {
                seller: parts[1].to_owned(),
                product_id: parts[2].to_owned(),
                product_name: parts[3].to_owned(),
                qty,
                price,
                lead_days: lead,
                currency: parts[7].to_owned(),
            }))
        }

        "order" => {
            if parts.len() < 7 {
                return Err(
                    "Usage: order <buyer> <seller> <product> <qty> <price> <currency>".to_owned(),
                );
            }
            let qty: u64 = if let Ok(v) = parts[4].parse() {
                v
            } else {
                return Err("Invalid quantity".to_owned());
            };
            let price: u64 = if let Some(v) = parse_price(parts[5]) {
                v
            } else {
                return Err("Invalid price".to_owned());
            };
            Ok(Some(ReplCommand::Order {
                buyer: parts[1].to_owned(),
                seller: parts[2].to_owned(),
                product: parts[3].to_owned(),
                qty,
                price,
                currency: parts[6].to_owned(),
            }))
        }

        "contract" => {
            if parts.len() < 9 {
                return Err(
                    "Usage: contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>"
                        .to_owned(),
                );
            }
            let max_price: u64 = if let Some(v) = parse_price(parts[4]) {
                v
            } else {
                return Err("Invalid max_price".to_owned());
            };
            let min_qty: u64 = if let Ok(v) = parts[5].parse() {
                v
            } else {
                return Err("Invalid min_qty".to_owned());
            };
            let max_qty: u64 = if let Ok(v) = parts[6].parse() {
                v
            } else {
                return Err("Invalid max_qty".to_owned());
            };
            let max_lead: u32 = if let Ok(v) = parts[7].parse() {
                v
            } else {
                return Err("Invalid max_lead".to_owned());
            };
            Ok(Some(ReplCommand::Contract {
                contract_id: parts[1].to_owned(),
                buyer: parts[2].to_owned(),
                product: parts[3].to_owned(),
                max_price,
                min_qty,
                max_qty,
                max_lead,
                currency: parts[8].to_owned(),
            }))
        }

        "inventory" => {
            if parts.len() < 5 {
                return Err("Usage: inventory <owner> <product> <delta> <reason>".to_owned());
            }
            let delta: i64 = if let Ok(v) = parts[3].parse() {
                v
            } else {
                return Err("Invalid delta".to_owned());
            };
            let reason = parts[4..].join(" ");
            Ok(Some(ReplCommand::Inventory {
                owner: parts[1].to_owned(),
                product: parts[2].to_owned(),
                delta,
                reason,
            }))
        }

        "asset" => {
            if parts.len() < 9 {
                return Err(
                    "Usage: asset <originator> <product_name> <gtin> <batch> <expiry> <serial> <qty> <event_type>"
                        .to_owned(),
                );
            }
            let qty: u64 = if let Ok(v) = parts[7].parse() {
                v
            } else {
                return Err("Invalid qty".to_owned());
            };
            let opt = |val: &str| {
                if val == "-" {
                    None
                } else {
                    Some(val.to_owned())
                }
            };
            Ok(Some(ReplCommand::Asset {
                originator: parts[1].to_owned(),
                product_name: parts[2].to_owned(),
                gtin: opt(parts[3]),
                batch: opt(parts[4]),
                expiry: opt(parts[5]),
                serial: opt(parts[6]),
                qty,
                event_type: parts[8].to_owned(),
            }))
        }

        "chain" => Ok(Some(ReplCommand::Chain)),
        "pending" => Ok(Some(ReplCommand::Pending)),
        "peers" => Ok(Some(ReplCommand::Peers)),
        "contracts" => Ok(Some(ReplCommand::Contracts)),
        "reload-trust-store" => {
            if parts.len() != 2 {
                return Err("Usage: reload-trust-store <PATH>".to_owned());
            }
            Ok(Some(ReplCommand::ReloadTrustStore {
                path: parts[1].to_owned(),
            }))
        }

        "quit" | "exit" => Ok(Some(ReplCommand::Quit)),

        other => Err(format!(
            "Unknown command: {other:?}. Type 'help' for usage."
        )),
    }
}

/// CLI arguments, in the order they appear in `usage()`. Extracted from
/// `main` so flag parsing is unit-testable without spawning the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    node_id: String,
    listen_addr: String,
    seed_peers: Vec<String>,
    difficulty: usize,
    storage_path: Option<String>,
    org_name: Option<String>,
    identity_node_id: Option<String>,
    trust_store: Option<String>,
    identity_file: Option<String>,
    rpc_addr: Option<String>,
}

impl CliArgs {
    fn defaults() -> Self {
        Self {
            node_id: "node-1".to_owned(),
            listen_addr: "0.0.0.0:8000".to_owned(),
            seed_peers: Vec::new(),
            difficulty: 2,
            storage_path: None,
            org_name: None,
            identity_node_id: None,
            trust_store: None,
            identity_file: None,
            rpc_addr: None,
        }
    }
}

/// Parse `--flag value` pairs. Unknown flags are ignored (forward
/// compatibility); a missing value keeps the current one.
fn parse_args(args: &[String]) -> CliArgs {
    let mut parsed = CliArgs::defaults();
    // A flag/value pair per iteration; a trailing flag without a value ends
    // the scan. No index arithmetic to mis-mutate into a non-terminating loop.
    let mut args = args.iter().skip(1);
    while let Some(flag) = args.next() {
        let Some(value) = args.next() else {
            break;
        };
        match flag.as_str() {
            "--id" => parsed.node_id.clone_from(value),
            "--listen" => parsed.listen_addr.clone_from(value),
            "--peer" => parsed.seed_peers.push(value.clone()),
            "--difficulty" => parsed.difficulty = value.parse().unwrap_or(2),
            "--storage-path" => parsed.storage_path = Some(value.clone()),
            "--org" => parsed.org_name = Some(value.clone()),
            "--identity-node-id" => parsed.identity_node_id = Some(value.clone()),
            "--trust-store" => parsed.trust_store = Some(value.clone()),
            "--identity-file" => parsed.identity_file = Some(value.clone()),
            "--rpc-addr" => parsed.rpc_addr = Some(value.clone()),
            _ => {}
        }
    }
    parsed
}

/// Log one node event in the REPL's `[event]` format. Extracted from the
/// spawned logger task so the arm coverage is unit-testable.
#[allow(clippy::too_many_lines)]
fn log_event(evt: &NodeEvent) {
    match evt {
        NodeEvent::TransactionAccepted(tx) => {
            log::info!("[event] Transaction accepted: {}", tx.id);
        }
        // The payload itself is never in the event stream (ADR-003).
        NodeEvent::PrivatePayloadReceived {
            collection,
            commitment,
        } => {
            log::info!(
                "[event] Private payload received: collection={collection} \
                 commitment={}",
                &commitment[..8]
            );
        }
        NodeEvent::BlockMined {
            index,
            hash,
            certificate: quorum,
        } => {
            let quorum_signers = quorum
                .signers_bitmap
                .iter()
                .map(|b| b.count_ones())
                .sum::<u32>();
            log::info!(
                "[event] Block mined: index={index} hash={} quorum_signers={quorum_signers}",
                &hash[..8],
            );
        }
        NodeEvent::BlockReceived {
            index,
            hash,
            certificate: quorum,
        } => {
            let quorum_signers = quorum
                .signers_bitmap
                .iter()
                .map(|b| b.count_ones())
                .sum::<u32>();
            log::info!(
                "[event] Block received from peer: index={index} hash={} quorum_signers={quorum_signers}",
                &hash[..8],
            );
        }
        NodeEvent::PeerConnected(addr) => {
            log::info!("[event] Peer connected: {addr}");
        }
        NodeEvent::EquivocationDetected { height, .. } => {
            log::warn!(
                "[event] EQUIVOCATION detected at height {height} - proof recorded for governance (ADR-009 section 4)"
            );
        }
        NodeEvent::PeerDisconnected(addr) => {
            log::info!("[event] Peer disconnected: {addr}");
        }
        NodeEvent::ContractExecuted {
            contract_id,
            quantity,
        } => {
            log::info!("[event] Contract {contract_id} auto-executed, qty={quantity}");
        }
        NodeEvent::AutonomousTransactionGenerated {
            trigger_id,
            transaction_id,
        } => {
            log::info!("[event] Watcher trigger {trigger_id} generated tx={transaction_id}");
        }
    }
}

/// REPL-scope identity material that `reload-trust-store` needs (ZT-R3).
struct ReplContext {
    org_name: Option<String>,
    org_root_pem: Option<String>,
    admin_gate: Option<AdminGate>,
}

/// Execute one parsed REPL command against the running node.
///
/// Returns `false` when the command was `Quit` (the REPL loop must stop).
/// Extracted from `main` so every arm is unit-testable against a real node.
#[allow(clippy::too_many_lines)]
async fn execute_repl_command(node: &Node, cmd: ReplCommand, ctx: &ReplContext) -> bool {
    match cmd {
        ReplCommand::Help => usage(),
        ReplCommand::Supply {
            seller,
            product_id,
            product_name,
            qty,
            price,
            lead_days,
            currency,
        } => {
            let tx = Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
                product_id,
                product_name,
                seller_id: seller,
                quantity_available: qty,
                price_per_unit: price,
                lead_time_days: lead_days,
                currency,
            }));
            match node.submit_transaction(tx).await {
                Ok(()) => println!("Supply offer submitted."),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        ReplCommand::Order {
            buyer,
            seller,
            product,
            qty,
            price,
            currency,
        } => {
            let tx = Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
                product_id: product,
                buyer_id: buyer,
                seller_id: seller,
                quantity: qty,
                agreed_price_per_unit: price,
                currency,
                contract_id: None,
            }));
            match node.submit_transaction(tx).await {
                Ok(()) => println!("Purchase order submitted."),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        ReplCommand::Contract {
            contract_id,
            buyer,
            product,
            max_price,
            min_qty,
            max_qty,
            max_lead,
            currency,
        } => {
            let tx = Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
                contract_id,
                buyer_id: buyer,
                product_id: product,
                conditions: PurchaseConditions {
                    max_price_per_unit: max_price,
                    min_quantity: min_qty,
                    max_quantity: max_qty,
                    max_lead_time_days: max_lead,
                    preferred_seller_id: None,
                    currency,
                    auto_execute: true,
                },
                wasm_code_b64: None,
            }));
            match node.submit_transaction(tx).await {
                Ok(()) => println!("Smart contract created."),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        ReplCommand::Inventory {
            owner,
            product,
            delta,
            reason,
        } => {
            let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
                owner_id: owner,
                product_id: product,
                quantity_delta: delta,
                reason,
            }));
            match node.submit_transaction(tx).await {
                Ok(()) => println!("Inventory update submitted."),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        ReplCommand::Asset {
            originator,
            product_name,
            gtin,
            batch,
            expiry,
            serial,
            qty,
            event_type,
        } => {
            let asset = TraceableAsset {
                gtin,
                batch_number: batch,
                expiry_date: expiry,
                serial_number: serial,
                anvisa_registration: None,
                manufacturer_id: None,
                product_name,
                custodian_id: originator.clone(),
                country_of_origin: None,
                storage_temp_celsius: None,
                quantity: qty,
            };
            let score = glasschain_core::MetadataTrustScore::compute(&asset);
            println!(
                "Metadata Trust Score: {} (fee multiplier: {:.0}%)",
                score,
                score.fee_multiplier() * 100.0
            );
            let tx = Transaction::new(TransactionKind::AssetRegistration(
                TraceableAssetRegistration {
                    asset,
                    event_type,
                    originator_id: originator,
                    purchase_order_ref: None,
                },
            ));
            match node.submit_transaction(tx).await {
                Ok(()) => println!("Asset registration submitted."),
                Err(e) => eprintln!("Error: {e}"),
            }
        }
        ReplCommand::Chain => {
            let ledger = node.ledger_snapshot().await;
            println!("Chain length: {} blocks", ledger.chain.len());
            for block in &ledger.chain {
                println!(
                    "  [{:>4}] {} | txns={} | prev={}…",
                    block.index,
                    &block.hash[..12],
                    block.transactions.len(),
                    &block.previous_hash[..8.min(block.previous_hash.len())]
                );
            }
        }
        ReplCommand::Pending => {
            let ledger = node.ledger_snapshot().await;
            println!(
                "Pending transactions: {}",
                ledger.pending_transactions.len()
            );
            for tx in &ledger.pending_transactions {
                let kind = match &tx.kind {
                    TransactionKind::SupplyOffer(_) => "SupplyOffer",
                    TransactionKind::PurchaseOrder(_) => "PurchaseOrder",
                    TransactionKind::ContractCreation(_) => "ContractCreation",
                    TransactionKind::ContractExecution(_) => "ContractExecution",
                    TransactionKind::InventoryUpdate(_) => "InventoryUpdate",
                    TransactionKind::AssetRegistration(_) => "AssetRegistration",
                    TransactionKind::CanonicalRecord(_) => "CanonicalRecord",
                    TransactionKind::CapabilityActivation(_) => "CapabilityActivation",
                    TransactionKind::PolicyUpdate(_) => "PolicyUpdate",
                };
                println!("  {} [{}]", tx.id, kind);
            }
        }
        ReplCommand::Peers => {
            let peers = node.known_peers().await;
            if peers.is_empty() {
                println!("No connected peers.");
            } else {
                println!("Known peers ({}):", peers.len());
                for p in peers {
                    println!("  {p}");
                }
            }
        }
        ReplCommand::Contracts => {
            let summaries = node.contract_summaries().await;
            if summaries.is_empty() {
                println!("No contracts registered.");
            } else {
                println!("Contracts ({}):", summaries.len());
                for s in &summaries {
                    println!(
                        "  [{}] buyer={} product={} status={} purchased={}/{}",
                        s.id,
                        s.buyer_id,
                        s.product_id,
                        s.status,
                        s.quantity_purchased,
                        s.max_quantity
                    );
                }
            }
        }
        ReplCommand::ReloadTrustStore { path } => {
            match (ctx.org_name.as_deref(), ctx.org_root_pem.as_deref()) {
                (Some(org), Some(root_pem)) => {
                    match build_trust_store_verifier(org, root_pem, &path) {
                        Ok((verifier, files, crls)) => {
                            node.set_cert_verifier(verifier.clone()).await;
                            // The admin gate shares the swap (ZT-R3):
                            // it verifies against the new chain/CRLs
                            // from the next authorization on.
                            if let Some(gate) = ctx.admin_gate.as_ref() {
                                gate.update_verifier(Arc::new(verifier));
                            }
                            println!(
                                "Trust store reloaded from {path}: {files} file(s), {crls} CRL(s) — the next Hello verifies against it."
                            );
                        }
                        Err(e) => eprintln!("Error: {e}"),
                    }
                }
                _ => {
                    eprintln!(
                        "reload-trust-store requires --org (there is no own Root CA to verify against)"
                    );
                }
            }
        }
        ReplCommand::Quit => {
            println!("Shutting down.");
            return false;
        }
    }
    true
}

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        usage();
        return;
    }

    // Parse CLI arguments.
    let args = parse_args(&args);
    let CliArgs {
        node_id,
        listen_addr,
        seed_peers,
        difficulty,
        storage_path,
        org_name,
        identity_node_id,
        trust_store,
        identity_file,
        rpc_addr,
    } = args;

    log::info!(
        "Starting GlassChain node id={node_id}  listen={listen_addr}  difficulty={difficulty}"
    );

    let mut org_root_pem: Option<String> = None;
    let mut ocsp_staple: Option<Vec<u8>> = None;
    // ADR-017 operator RBAC: the gRPC channel-management gate, installed when
    // the certificate verifier is. `None` ⇒ admin RPCs fail closed.
    // Survives the gRPC spawn so `reload-trust-store` can hot-swap it (ZT-R3).
    let mut admin_gate_handle: Option<AdminGate> = None;
    // ── Identity custody (ADR-018) ───────────────────────────────────────────
    // Without `--identity-file` the organization is regenerated every start
    // (dev behaviour: every restart is a new key). With `--identity-file` the
    // node owns a durable identity file: created on first start, loaded after
    // — the same identity key, certificate and root CA re-presented, so
    // persisted TOFU pins keep verifying across restarts.

    let identity = org_name.as_ref().map(|org| {
        let identity_name = identity_node_id.clone().unwrap_or_else(|| node_id.clone());
        let (organization, issued_identity) = identity_file.as_deref().map_or_else(
            || {
                log::warn!(
                    "No --identity-file: organization `{org}` and identity `{identity_name}` are \
                     regenerated every start (new key, new certificate, new Root CA) — persisted \
                     TOFU pins on peers will refuse this node until an operator removes them \
                     (start with --identity-file <PATH> for durable custody, ADR-018)"
                );
                let mut organization = Organization::new(org.clone()).unwrap_or_else(|e| {
                    log::error!("Failed to create organization `{org}`: {e}");
                    std::process::exit(1);
                });
                let issued_identity = organization
                    .issue_identity(identity_name.clone())
                    .unwrap_or_else(|e| {
                        log::error!(
                            "Failed to issue identity `{identity_name}` from organization `{org}`: {e}"
                        );
                        std::process::exit(1);
                    })
                    .clone();
                (organization, Arc::new(issued_identity))
            },
            |file| {
                let (organization, identity, created) =
                    load_or_create_identity_file(std::path::Path::new(file), org, &identity_name)
                        .unwrap_or_else(|e| {
                            log::error!("{e}");
                            std::process::exit(1);
                        });
                if created {
                    log::info!(
                        "Durable identity material created at `{file}` (0600) — the node re-presents the same identity key and Root CA across restarts (ADR-018)"
                    );
                } else {
                    log::info!(
                        "Loaded durable identity material for node `{identity_name}` from `{file}` (ADR-018)"
                    );
                }
                (organization, identity)
            },
        );
        log::info!(
            "Using identity-backed TLS certificate for node `{identity_name}` issued by organization `{}`",
            organization.name
        );
        org_root_pem = Some(organization.root_ca_cert_pem.clone());
        // ADR-017: mint this node's OCSP staple — the issuer-signed live
        // revocation status of its own certificate, stapled on every Hello.
        match organization.ocsp_response_der(&identity_name) {
            Ok(staple) => {
                ocsp_staple = Some(staple);
                log::info!(
                    "OCSP staple minted for node `{identity_name}` (verified locally by peers; no responder network queries)"
                );
            }
            Err(_) => log::warn!(
                "OCSP staple minting failed; peers fall back to the CRL path"
            ),
        }
        issued_identity
    });

    // Build the node — optionally backed by persistent redb storage.
    let node = storage_path.as_ref().map_or_else(
        || {
            identity.clone().map_or_else(
                || Arc::new(Node::new(node_id.clone(), listen_addr.clone(), difficulty)),
                |identity| {
                    Arc::new(Node::new_with_identity(
                        node_id.clone(),
                        listen_addr.clone(),
                        difficulty,
                        identity,
                    ))
                },
            )
        },
        |path| {
            log::info!("Using persistent storage at {path}");
            match RedbStorageProvider::open(path) {
                Ok(storage) => {
                    if let Some(identity) = identity.clone() {
                        Arc::new(Node::new_with_storage_and_identity(
                            node_id.clone(),
                            listen_addr.clone(),
                            difficulty,
                            Arc::new(storage),
                            identity,
                        ))
                    } else {
                        Arc::new(Node::new_with_storage(
                            node_id.clone(),
                            listen_addr.clone(),
                            difficulty,
                            Arc::new(storage),
                        ))
                    }
                }
                Err(e) => {
                    log::error!("Failed to open storage at {path}: {e}");
                    std::process::exit(1);
                }
            }
        },
    );

    // Attach the WASM execution provider so contracts with wasm_code_b64 payloads
    // are evaluated through the Wasmtime sandbox.
    match WasmExecutionProvider::new() {
        Ok(executor) => {
            node.set_execution_provider(Arc::new(executor)).await;
            log::info!("WASM execution provider enabled");
        }
        Err(e) => {
            log::warn!("WASM execution provider unavailable: {e}");
        }
    }

    // Staple the minted OCSP response on every Hello (ADR-017).
    if let Some(staple) = ocsp_staple {
        node.set_ocsp_staple(staple).await;
    }

    // Attach the MSP endorsement provider when the node has an organizational
    // identity. Attaching it is necessary but not sufficient: enforcement also
    // requires the `endorsement` capability to be active at the candidate
    // height, which is activated in-band via a committed CapabilityActivation
    // record (ADR-008). Without a provider every endorsement gate
    // short-circuits.
    match (org_name.as_ref(), identity.as_ref()) {
        (Some(org), Some(identity)) => {
            let mut msp = MspEndorsementProvider::new();
            msp.register_identity(identity, Principal::new(org.clone()));
            node.set_endorsement_provider(Arc::new(msp)).await;
            log::info!(
                "MSP endorsement provider enabled for organization `{org}` (enforcement begins once the `endorsement` capability is active at the candidate height)"
            );
        }
        _ => {
            log::warn!(
                "No endorsement provider configured: endorsement enforcement is disabled (start with --org to attach one)"
            );
        }
    }

    // Install the federation certificate verifier (ADR-011). Without it, the
    // private-payload path fails open to the self-asserted `Hello` org — that
    // must be an operator-visible decision, not a silent default.
    match (
        org_name.as_deref(),
        org_root_pem.as_deref(),
        trust_store.as_deref(),
    ) {
        (Some(org), Some(root_pem), Some(path)) => {
            let (verifier, files, crls) = match build_trust_store_verifier(org, root_pem, path) {
                Ok(result) => result,
                Err(e) => {
                    log::error!("{e}");
                    std::process::exit(1);
                }
            };
            // ADR-013 fail-closed: without CRLs every peer verification
            // rejects, so make the omission loud at startup.
            if verifier.federation_anchor_count() > 0 && verifier.crl_count() == 0 {
                log::warn!(
                    "Trust store has federation anchors but no CRLs: every peer \
                     verification will fail until '*.crl' files are added"
                );
            }
            log::info!(
                "Certificate verification enabled: own organization `{org}` plus {files} trust-store file(s) from `{path}` — {crls} CRL(s) loaded; revocation is fail-closed (peers whose issuing CA has no current CRL are rejected)"
            );
            node.set_cert_verifier(verifier.clone()).await;
            admin_gate_handle = Some(AdminGate::new(Arc::new(verifier)));
        }
        (Some(_), Some(_), None) => {
            log::warn!(
                "No federation trust store configured: peer organizations are NOT certificate-verified and the private-payload path trusts the self-asserted org (start with --trust-store <PATH> to enable verification)"
            );
        }
        (None, _, Some(_)) => {
            log::error!(
                "--trust-store requires --org: there is no organization Root CA to verify against"
            );
            std::process::exit(1);
        }
        _ => {}
    }

    // Spawn event logger.
    let mut events = node.subscribe();
    tokio::spawn(async move {
        while let Ok(evt) = events.recv().await {
            log_event(&evt);
        }
    });

    if let Err(e) = node.start(seed_peers).await {
        log::error!("Failed to start node: {e}");
        std::process::exit(1);
    }

    // ── Optional gRPC server ───────────────────────────────────────────────
    if let Some(ref addr_str) = rpc_addr {
        let rpc_node = Arc::clone(&node);
        match addr_str.parse::<SocketAddr>() {
            Ok(addr) => {
                let server = GlasschainServer::new(rpc_node);
                // ADR-017: the gate is installed only with a configured
                // verifier; channel-management RPCs fail closed without one.
                let server = match admin_gate_handle.as_ref() {
                    Some(gate) => server.with_admin_gate(gate.clone()),
                    None => server,
                };
                tokio::spawn(async move {
                    if let Err(e) = server.serve(addr).await {
                        log::error!("gRPC server error: {e}");
                    }
                });
                log::info!("gRPC server started on {addr}");
            }
            Err(e) => {
                log::warn!("Invalid --rpc-addr {addr_str:?}: {e} — gRPC server not started");
            }
        }
    }

    println!("GlassChain node `{node_id}` is running on {listen_addr}");
    println!("Type 'help' for available commands.\n");

    // REPL-scope copies of the identity material the `reload-trust-store`
    // command needs (ZT-R3).
    let ctx = ReplContext {
        org_name: org_name.clone(),
        org_root_pem: org_root_pem.clone(),
        admin_gate: admin_gate_handle,
    };
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        print!("> ");
        let _ = std::io::stdout().flush();

        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            break; // EOF
        }

        let cmd = match parse_command(&line) {
            Ok(Some(cmd)) => cmd,
            Ok(None) => continue,
            Err(msg) => {
                eprintln!("{msg}");
                continue;
            }
        };

        if !execute_repl_command(&node, cmd, &ctx).await {
            break;
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        build_trust_store_verifier, execute_repl_command, load_or_create_identity_file, log_event,
        parse_args, parse_command, parse_price, CliArgs, ReplCommand, ReplContext,
    };
    use glasschain_identity::Organization;
    use glasschain_network::Node;
    use std::sync::Arc;

    /// Same held-socket allocation as the network suite's `free_addr`
    /// (probe→drop→rebind is a Windows CI flake — see
    /// `glasschain-network/tests/common/ports.rs`).
    fn free_addr() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("bound address").to_string();
        glasschain_network::stash_prebound_listener(&addr, listener);
        addr
    }

    async fn repl_node(id: &str) -> (Node, String) {
        let addr = free_addr();
        let node = Node::new(id, &addr, 1);
        node.start(vec![]).await.unwrap();
        (node, addr)
    }

    /// ZT-R3: the shared loader produces a verifier that (a) loads `*.pem`
    /// anchors and `*.crl` files from a directory, (b) accepts a single-file
    /// store, and (c) rejects a missing path. Reload is just "call it again".
    #[test]
    fn build_trust_store_verifier_loads_dir_file_and_reports() {
        let mut org = Organization::new("PharmaCorp").unwrap();
        let peer_org = Organization::new("MedCorp").unwrap();
        let dir = std::env::temp_dir().join(format!(
            "glasschain-trust-store-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("peer-root.pem"), &peer_org.root_ca_cert_pem).unwrap();
        std::fs::write(dir.join("peer-root.crl"), peer_org.crl_pem().unwrap()).unwrap();

        // Directory store: one anchor file + one CRL file.
        let (verifier, files, crls) =
            build_trust_store_verifier("PharmaCorp", &org.root_ca_cert_pem, dir.to_str().unwrap())
                .unwrap();
        assert_eq!(files, 2);
        assert_eq!(crls, 1);
        assert_eq!(verifier.federation_anchor_count(), 1);
        // A member of the *own* org still verifies through the reloaded
        // verifier (rotation must not lose the own anchor).
        let identity = org.issue_identity("node-a").unwrap().clone();
        let mut with_own_crl = verifier;
        with_own_crl.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
        assert!(with_own_crl
            .verify_cert_pem(identity.certificate_pem.as_ref().unwrap())
            .is_ok());

        // Single-file store.
        let single = dir.join("single.pem");
        std::fs::write(&single, &peer_org.root_ca_cert_pem).unwrap();
        let (_, files, crls) = build_trust_store_verifier(
            "PharmaCorp",
            &org.root_ca_cert_pem,
            single.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(files, 1);
        assert_eq!(crls, 0);

        // Missing path.
        assert!(build_trust_store_verifier(
            "PharmaCorp",
            &org.root_ca_cert_pem,
            dir.join("nope").to_str().unwrap()
        )
        .is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_price_accepts_whole_number() {
        assert_eq!(parse_price("12"), Some(1200));
        assert_eq!(parse_price("0"), Some(0));
        // Leading/trailing whitespace is trimmed.
        assert_eq!(parse_price(" 7 "), Some(700));
    }

    #[test]
    fn parse_price_accepts_one_digit_fraction() {
        assert_eq!(parse_price("12.5"), Some(1250));
        assert_eq!(parse_price("0.1"), Some(10));
    }

    #[test]
    fn parse_price_accepts_two_digit_fraction() {
        assert_eq!(parse_price("12.50"), Some(1250));
        assert_eq!(parse_price("9.99"), Some(999));
        assert_eq!(parse_price("0.05"), Some(5));
    }

    #[test]
    fn parse_price_rejects_three_digit_fraction() {
        assert_eq!(parse_price("12.500"), None);
        assert_eq!(parse_price("1.234"), None);
    }

    #[test]
    fn parse_price_rejects_negative() {
        assert_eq!(parse_price("-12.50"), None);
        assert_eq!(parse_price("-1"), None);
    }

    #[test]
    fn parse_price_rejects_empty_or_garbage() {
        assert_eq!(parse_price(""), None);
        assert_eq!(parse_price("   "), None);
        assert_eq!(parse_price("abc"), None);
        assert_eq!(parse_price("12.x"), None);
        assert_eq!(parse_price("1.2.3"), None);
        assert_eq!(parse_price(".5"), None);
        assert_eq!(parse_price("+1"), None);
    }

    #[test]
    fn parse_price_rejects_overflow() {
        // `u64::MAX` in minor units is the largest accepted value.
        assert_eq!(parse_price("184467440737095516.15"), Some(u64::MAX));
        // `whole * 100` exceeds `u64::MAX` (checked_mul).
        assert_eq!(parse_price("184467440737095517"), None);
        // `whole * 100 + frac` exceeds `u64::MAX` (checked_add).
        assert_eq!(parse_price("184467440737095516.16"), None);
        // The whole part itself exceeds `u64::MAX` (parse fails).
        assert_eq!(parse_price("18446744073709551616"), None);
    }

    #[test]
    fn parse_command_blank_line_is_none() {
        assert_eq!(parse_command(""), Ok(None));
        assert_eq!(parse_command("   \n"), Ok(None));
        assert_eq!(parse_command("\t \n"), Ok(None));
    }

    #[test]
    fn parse_command_help() {
        assert_eq!(parse_command("help"), Ok(Some(ReplCommand::Help)));
        assert_eq!(parse_command("?"), Ok(Some(ReplCommand::Help)));
        // Extra tokens are ignored, matching the inline `parts[0]` match.
        assert_eq!(parse_command("help extra"), Ok(Some(ReplCommand::Help)));
    }

    #[test]
    fn parse_command_supply() {
        assert_eq!(
            parse_command("supply acme p1 Widget 100 12.50 3 USD"),
            Ok(Some(ReplCommand::Supply {
                seller: "acme".to_owned(),
                product_id: "p1".to_owned(),
                product_name: "Widget".to_owned(),
                qty: 100,
                price: 1250,
                lead_days: 3,
                currency: "USD".to_owned(),
            }))
        );
    }

    #[test]
    fn parse_command_order() {
        assert_eq!(
            parse_command("order buyer seller p1 5 9.99 USD"),
            Ok(Some(ReplCommand::Order {
                buyer: "buyer".to_owned(),
                seller: "seller".to_owned(),
                product: "p1".to_owned(),
                qty: 5,
                price: 999,
                currency: "USD".to_owned(),
            }))
        );
    }

    #[test]
    fn parse_command_contract() {
        assert_eq!(
            parse_command("contract c1 buyer p1 12.50 10 50 7 USD"),
            Ok(Some(ReplCommand::Contract {
                contract_id: "c1".to_owned(),
                buyer: "buyer".to_owned(),
                product: "p1".to_owned(),
                max_price: 1250,
                min_qty: 10,
                max_qty: 50,
                max_lead: 7,
                currency: "USD".to_owned(),
            }))
        );
    }

    #[test]
    fn parse_command_inventory_joins_reason_words() {
        let cmd = parse_command("inventory acme p1 -5 damaged in transit")
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            ReplCommand::Inventory {
                owner: "acme".to_owned(),
                product: "p1".to_owned(),
                delta: -5,
                reason: "damaged in transit".to_owned(),
            }
        );
    }

    #[test]
    fn parse_command_asset_dash_placeholder() {
        let cmd = parse_command("asset acme Widget - - - - 5 RECEIVED")
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            ReplCommand::Asset {
                originator: "acme".to_owned(),
                product_name: "Widget".to_owned(),
                gtin: None,
                batch: None,
                expiry: None,
                serial: None,
                qty: 5,
                event_type: "RECEIVED".to_owned(),
            }
        );
    }

    #[test]
    fn parse_command_asset_keeps_non_dash_values() {
        let cmd = parse_command("asset acme Widget 789012345678 42 2026-01-01 SN-1 5 RECEIVED")
            .unwrap()
            .unwrap();
        assert_eq!(
            cmd,
            ReplCommand::Asset {
                originator: "acme".to_owned(),
                product_name: "Widget".to_owned(),
                gtin: Some("789012345678".to_owned()),
                batch: Some("42".to_owned()),
                expiry: Some("2026-01-01".to_owned()),
                serial: Some("SN-1".to_owned()),
                qty: 5,
                event_type: "RECEIVED".to_owned(),
            }
        );
    }

    #[test]
    fn parse_command_simple_commands() {
        for (line, cmd) in [
            ("chain", ReplCommand::Chain),
            ("pending", ReplCommand::Pending),
            ("peers", ReplCommand::Peers),
            ("contracts", ReplCommand::Contracts),
        ] {
            assert_eq!(parse_command(line), Ok(Some(cmd)));
        }
    }

    #[test]
    fn parse_command_quit() {
        assert_eq!(parse_command("quit"), Ok(Some(ReplCommand::Quit)));
        assert_eq!(parse_command("exit"), Ok(Some(ReplCommand::Quit)));
    }

    #[test]
    fn parse_command_unknown() {
        assert_eq!(
            parse_command("frobnicate x"),
            Err("Unknown command: \"frobnicate\". Type 'help' for usage.".to_owned())
        );
    }

    #[test]
    fn parse_command_arity_errors() {
        assert_eq!(
            parse_command("supply acme p1 Widget 100 12.50"),
            Err("Usage: supply <seller> <product_id> <product_name> <qty> <price> <lead_days> <currency>".to_owned())
        );
        assert_eq!(
            parse_command("order buyer seller p1 5"),
            Err("Usage: order <buyer> <seller> <product> <qty> <price> <currency>".to_owned())
        );
        assert_eq!(
            parse_command("contract c1 buyer p1 12.50"),
            Err("Usage: contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>".to_owned())
        );
        assert_eq!(
            parse_command("inventory acme p1"),
            Err("Usage: inventory <owner> <product> <delta> <reason>".to_owned())
        );
        assert_eq!(
            parse_command("asset acme Widget"),
            Err("Usage: asset <originator> <product_name> <gtin> <batch> <expiry> <serial> <qty> <event_type>".to_owned())
        );
    }

    #[test]
    fn parse_command_numeric_errors() {
        for (line, msg) in [
            ("supply acme p1 Widget abc 12.50 3 USD", "Invalid quantity"),
            (
                "supply acme p1 Widget 100 banana 3 USD",
                "Invalid price (use decimal like 12.50)",
            ),
            ("supply acme p1 Widget 100 12.50 x USD", "Invalid lead_days"),
            ("order buyer seller p1 abc 9.99 USD", "Invalid quantity"),
            ("order buyer seller p1 5 banana USD", "Invalid price"),
            (
                "contract c1 buyer p1 banana 10 50 7 USD",
                "Invalid max_price",
            ),
            ("contract c1 buyer p1 12.50 x 50 7 USD", "Invalid min_qty"),
            ("contract c1 buyer p1 12.50 10 x 7 USD", "Invalid max_qty"),
            ("contract c1 buyer p1 12.50 10 50 x USD", "Invalid max_lead"),
            ("inventory acme p1 abc damaged", "Invalid delta"),
            ("asset acme Widget 789 42 2026 1 x RECEIVED", "Invalid qty"),
        ] {
            assert_eq!(parse_command(line), Err(msg.to_owned()));
        }
    }

    #[test]
    fn parse_args_defaults_without_flags() {
        assert_eq!(parse_args(&["bin".to_owned()]), CliArgs::defaults());
    }

    #[test]
    fn parse_args_reads_every_flag() {
        let args = [
            "bin".to_owned(),
            "--id".to_owned(),
            "node-9".to_owned(),
            "--listen".to_owned(),
            "0.0.0.0:9000".to_owned(),
            "--peer".to_owned(),
            "127.0.0.1:8000".to_owned(),
            "--peer".to_owned(),
            "127.0.0.1:8001".to_owned(),
            "--difficulty".to_owned(),
            "4".to_owned(),
            "--storage-path".to_owned(),
            "/tmp/glasschain".to_owned(),
            "--org".to_owned(),
            "PharmaCorp".to_owned(),
            "--identity-node-id".to_owned(),
            "cert-node".to_owned(),
            "--trust-store".to_owned(),
            "/etc/trust".to_owned(),
            "--identity-file".to_owned(),
            "/var/lib/id.json".to_owned(),
            "--rpc-addr".to_owned(),
            "0.0.0.0:50051".to_owned(),
        ];
        let parsed = parse_args(&args);
        assert_eq!(parsed.node_id, "node-9");
        assert_eq!(parsed.listen_addr, "0.0.0.0:9000");
        assert_eq!(parsed.seed_peers, vec!["127.0.0.1:8000", "127.0.0.1:8001"]);
        assert_eq!(parsed.difficulty, 4);
        assert_eq!(parsed.storage_path.as_deref(), Some("/tmp/glasschain"));
        assert_eq!(parsed.org_name.as_deref(), Some("PharmaCorp"));
        assert_eq!(parsed.identity_node_id.as_deref(), Some("cert-node"));
        assert_eq!(parsed.trust_store.as_deref(), Some("/etc/trust"));
        assert_eq!(parsed.identity_file.as_deref(), Some("/var/lib/id.json"));
        assert_eq!(parsed.rpc_addr.as_deref(), Some("0.0.0.0:50051"));
    }

    #[test]
    fn parse_args_ignores_unknown_flags_and_bad_difficulty() {
        // Unknown flags are skipped with their values; an unparsable
        // difficulty falls back to the default 2.
        let args = [
            "bin".to_owned(),
            "--wat".to_owned(),
            "x".to_owned(),
            "--difficulty".to_owned(),
            "not-a-number".to_owned(),
            "--id".to_owned(),
            "n".to_owned(),
        ];
        let parsed = parse_args(&args);
        assert_eq!(parsed.node_id, "n");
        assert_eq!(parsed.difficulty, 2);
    }

    #[test]
    fn parse_args_tolerates_trailing_flag_without_value() {
        let parsed = parse_args(&["bin".to_owned(), "--listen".to_owned()]);
        assert_eq!(parsed.listen_addr, "0.0.0.0:8000");
    }

    fn no_ctx() -> ReplContext {
        ReplContext {
            org_name: None,
            org_root_pem: None,
            admin_gate: None,
        }
    }

    #[tokio::test]
    async fn repl_submits_every_transaction_kind() {
        let (node, _) = repl_node("repl-tx").await;
        let ctx = no_ctx();

        let supply = parse_command("supply acme p1 Widget 100 12.50 3 USD")
            .unwrap()
            .unwrap();
        let order = parse_command("order buyer acme p1 5 9.99 USD")
            .unwrap()
            .unwrap();
        let contract = parse_command("contract c1 buyer p1 12.50 10 50 7 USD")
            .unwrap()
            .unwrap();
        let inventory = parse_command("inventory acme p1 -5 damaged")
            .unwrap()
            .unwrap();
        let asset = parse_command("asset acme Widget 789012345678 42 2026-01-01 SN-1 5 RECEIVED")
            .unwrap()
            .unwrap();

        assert!(execute_repl_command(&node, supply, &ctx).await);
        assert!(execute_repl_command(&node, order, &ctx).await);
        assert!(execute_repl_command(&node, contract, &ctx).await);
        assert!(execute_repl_command(&node, inventory, &ctx).await);
        assert!(execute_repl_command(&node, asset, &ctx).await);

        let ledger = node.ledger_snapshot().await;
        let kinds: Vec<&str> = ledger
            .pending_transactions
            .iter()
            .map(|tx| match &tx.kind {
                glasschain_core::TransactionKind::SupplyOffer(_) => "supply",
                glasschain_core::TransactionKind::PurchaseOrder(_) => "order",
                glasschain_core::TransactionKind::ContractCreation(_) => "contract",
                glasschain_core::TransactionKind::InventoryUpdate(_) => "inventory",
                glasschain_core::TransactionKind::AssetRegistration(_) => "asset",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["supply", "order", "contract", "inventory", "asset"]
        );
    }

    #[tokio::test]
    async fn repl_chain_shows_blocks_after_mining() {
        let (node, _) = repl_node("repl-chain").await;
        let ctx = no_ctx();

        assert!(execute_repl_command(&node, ReplCommand::Chain, &ctx).await);
        node.mine().await.unwrap();
        assert!(execute_repl_command(&node, ReplCommand::Chain, &ctx).await);

        let ledger = node.ledger_snapshot().await;
        assert_eq!(ledger.chain.len(), 2);
    }

    #[tokio::test]
    async fn repl_pending_and_peers_and_contracts() {
        let (node, addr_a) = repl_node("repl-lists").await;
        let ctx = no_ctx();

        // Pending: empty, then with one transaction.
        assert!(execute_repl_command(&node, ReplCommand::Pending, &ctx).await);
        let tx = parse_command("order buyer acme p1 5 9.99 USD")
            .unwrap()
            .unwrap();
        execute_repl_command(&node, tx, &ctx).await;
        assert!(execute_repl_command(&node, ReplCommand::Pending, &ctx).await);
        assert_eq!(node.ledger_snapshot().await.pending_transactions.len(), 1);

        // Peers: empty first (the "No connected peers." arm).
        assert!(execute_repl_command(&node, ReplCommand::Peers, &ctx).await);

        // Contracts: empty, then one registered.
        assert!(execute_repl_command(&node, ReplCommand::Contracts, &ctx).await);
        let contract = parse_command("contract c1 buyer p1 12.50 10 50 7 USD")
            .unwrap()
            .unwrap();
        execute_repl_command(&node, contract, &ctx).await;
        assert!(execute_repl_command(&node, ReplCommand::Contracts, &ctx).await);
        assert_eq!(node.contract_summaries().await.len(), 1);

        // Peers again, now with a real connection.
        let peer_addr = free_addr();
        let peer = Node::new("repl-peer", &peer_addr, 1);
        peer.start(vec![addr_a.clone()]).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert!(execute_repl_command(&node, ReplCommand::Peers, &ctx).await);
        assert!(!node.known_peers().await.is_empty());
    }

    #[tokio::test]
    async fn repl_help_and_quit() {
        let (node, _) = repl_node("repl-help").await;
        assert!(execute_repl_command(&node, ReplCommand::Help, &no_ctx()).await);
        assert!(!execute_repl_command(&node, ReplCommand::Quit, &no_ctx()).await);
    }

    #[tokio::test]
    async fn repl_reload_trust_store_requires_org() {
        let (node, _) = repl_node("repl-reload").await;
        let reload = parse_command("reload-trust-store /tmp/somewhere")
            .unwrap()
            .unwrap();
        // Without --org there is no own Root CA to verify against: the
        // command errors but the REPL continues.
        assert!(execute_repl_command(&node, reload, &no_ctx()).await);
    }

    #[tokio::test]
    async fn repl_reload_trust_store_swaps_verifier_and_gate() {
        let (node, _) = repl_node("repl-reload-ok").await;

        let org = Organization::new("PharmaCorp").unwrap();
        let peer_org = Organization::new("MedCorp").unwrap();
        let dir = std::env::temp_dir().join(format!(
            "glasschain-repl-reload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("peer-root.pem"), &peer_org.root_ca_cert_pem).unwrap();
        std::fs::write(dir.join("peer-root.crl"), peer_org.crl_pem().unwrap()).unwrap();
        // Single-file store for the initial admin-gate verifier.
        let single = dir.join("single.pem");
        std::fs::write(&single, &peer_org.root_ca_cert_pem).unwrap();

        let (gate_verifier, _, _) = build_trust_store_verifier(
            "PharmaCorp",
            &org.root_ca_cert_pem,
            single.to_str().unwrap(),
        )
        .unwrap();
        let ctx = ReplContext {
            org_name: Some("PharmaCorp".to_owned()),
            org_root_pem: Some(org.root_ca_cert_pem.clone()),
            admin_gate: Some(glasschain_rpc::AdminGate::new(Arc::new(gate_verifier))),
        };

        assert!(
            node.cert_verifier().await.is_none(),
            "the REPL node starts without a verifier"
        );
        // A bad path hits the `Err` branch and the REPL keeps going, without
        // installing anything.
        let reload = parse_command("reload-trust-store /tmp/bad-path")
            .unwrap()
            .unwrap();
        assert!(execute_repl_command(&node, reload, &ctx).await);
        assert!(
            node.cert_verifier().await.is_none(),
            "a failed reload must not install a verifier"
        );

        // The directory store reload succeeds: files and CRLs counted,
        // verifier and admin gate swapped.
        let reload = parse_command(&format!("reload-trust-store {}", dir.to_str().unwrap()))
            .unwrap()
            .unwrap();
        assert!(execute_repl_command(&node, reload, &ctx).await);
        assert!(
            node.cert_verifier().await.is_some(),
            "the matching (org, root) arm must install the reloaded verifier"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_event_covers_every_variant_without_panicking() {
        use glasschain_core::{
            Block, InventoryUpdate, QuorumCertificate, Transaction, TransactionKind,
        };
        use glasschain_network::NodeEvent;
        let cert_block = Block::new(1, vec![], "0".to_owned());
        let events = vec![
            NodeEvent::TransactionAccepted(Transaction::new(TransactionKind::InventoryUpdate(
                InventoryUpdate {
                    product_id: "p1".to_owned(),
                    owner_id: "o1".to_owned(),
                    quantity_delta: 1,
                    reason: "r".to_owned(),
                },
            ))),
            NodeEvent::PeerConnected("127.0.0.1:1".to_owned()),
            NodeEvent::PeerDisconnected("127.0.0.1:1".to_owned()),
            NodeEvent::EquivocationDetected {
                height: 3,
                public_key: vec![1, 2, 3],
            },
            NodeEvent::ContractExecuted {
                contract_id: "c1".to_owned(),
                quantity: 5,
            },
            NodeEvent::AutonomousTransactionGenerated {
                trigger_id: "t1".to_owned(),
                transaction_id: "tx1".to_owned(),
            },
            NodeEvent::PrivatePayloadReceived {
                collection: "orders".to_owned(),
                commitment: "abcdef12rest".to_owned(),
            },
            NodeEvent::BlockMined {
                index: 1,
                hash: "abc123deadbeef99".to_owned(),
                certificate: QuorumCertificate::pow(&cert_block),
            },
            NodeEvent::BlockReceived {
                index: 2,
                hash: "abc123deadbeef88".to_owned(),
                certificate: QuorumCertificate::pow(&cert_block),
            },
        ];
        for evt in &events {
            log_event(evt);
        }
    }
    #[test]
    fn identity_file_create_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "glasschain-identity-file-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("org.json");

        // First start creates the file (0600) and issues the identity.
        let (organization, identity, created) =
            load_or_create_identity_file(&dir.join("org.json"), "PharmaCorp", "node-a").unwrap();
        assert!(created);
        assert_eq!(organization.name, "PharmaCorp");
        assert!(identity.certificate_pem.is_some());
        assert!(path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "the custody file is owner-only");
        }

        // A restart re-presents the same key.
        let (reloaded, same_identity, created_again) =
            load_or_create_identity_file(&path, "PharmaCorp", "node-a").unwrap();
        assert!(!created_again);
        assert_eq!(reloaded.name, "PharmaCorp");
        assert_eq!(
            same_identity.public_key_bytes(),
            identity.public_key_bytes()
        );
        let _ = &organization;

        // A file holding another organization's material is refused.
        let mut foreign = Organization::new("MedCorp").unwrap();
        foreign.issue_identity("med-node").unwrap();
        let foreign_path = dir.join("foreign.json");
        std::fs::write(&foreign_path, foreign.export_json().unwrap()).unwrap();
        let Err(foreign_err) =
            load_or_create_identity_file(&foreign_path, "PharmaCorp", "med-node")
        else {
            panic!("a foreign custody file must be refused");
        };
        assert!(foreign_err.contains("does not match"), "{foreign_err}");

        // An identity file without the requested member is refused.
        let Err(ghost_err) = load_or_create_identity_file(&path, "PharmaCorp", "ghost") else {
            panic!("a missing member must be refused");
        };
        assert!(ghost_err.contains("no member"), "{ghost_err}");

        // Corrupt material fails closed.
        let corrupt = dir.join("corrupt.json");
        std::fs::write(&corrupt, "{not json").unwrap();
        let Err(corrupt_err) = load_or_create_identity_file(&corrupt, "PharmaCorp", "node-a")
        else {
            panic!("corrupt material must be refused");
        };
        assert!(corrupt_err.contains("corrupt"), "{corrupt_err}");

        let _ = identity;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_command_reload_requires_exactly_one_path() {
        assert_eq!(
            parse_command("reload-trust-store"),
            Err("Usage: reload-trust-store <PATH>".to_owned())
        );
        assert_eq!(
            parse_command("reload-trust-store a b"),
            Err("Usage: reload-trust-store <PATH>".to_owned())
        );
    }
}
