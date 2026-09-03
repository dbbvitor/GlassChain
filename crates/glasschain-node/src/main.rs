use glasschain_core::{
    endorsement::Principal, InventoryUpdate, PurchaseConditions, PurchaseOrder, SmartContractDef,
    SupplyOffer, TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
};
use glasschain_identity::{CertChainVerifier, MspEndorsementProvider, Organization};
use glasschain_network::{Node, NodeEvent};
use glasschain_rpc::GlasschainServer;
use glasschain_storage::SledStorageProvider;
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
    --storage-path <PATH>   Directory for persistent Sled block storage (optional).
                            When provided, the chain is reloaded from disk on restart.
    --org <NAME>            Organization name for issuing an identity-backed TLS certificate.
    --identity-node-id <ID> Node ID to embed in the issued TLS identity certificate.
                            Defaults to the value passed to --id.
    --trust-store <PATH>    PEM file or directory of *.pem files holding the Root CA
                            certificates of the peer organizations to trust (ADR-011).
                            Requires --org. Without it, peer organizations are NOT
                            certificate-verified.
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
        "quit" | "exit" => Ok(Some(ReplCommand::Quit)),

        other => Err(format!(
            "Unknown command: {other:?}. Type 'help' for usage."
        )),
    }
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
    let mut node_id = "node-1".to_owned();
    let mut listen_addr = "0.0.0.0:8000".to_owned();
    let mut seed_peers: Vec<String> = Vec::new();
    let mut difficulty = 2usize;
    let mut storage_path: Option<String> = None;
    let mut org_name: Option<String> = None;
    let mut identity_node_id: Option<String> = None;
    let mut trust_store: Option<String> = None;
    let mut rpc_addr: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    node_id = v.clone();
                }
            }
            "--listen" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    listen_addr = v.clone();
                }
            }
            "--peer" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    seed_peers.push(v.clone());
                }
            }
            "--difficulty" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    difficulty = v.parse().unwrap_or(2);
                }
            }
            "--storage-path" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    storage_path = Some(v.clone());
                }
            }
            "--org" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    org_name = Some(v.clone());
                }
            }
            "--identity-node-id" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    identity_node_id = Some(v.clone());
                }
            }
            "--trust-store" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    trust_store = Some(v.clone());
                }
            }
            "--rpc-addr" => {
                i += 1;
                if let Some(v) = args.get(i) {
                    rpc_addr = Some(v.clone());
                }
            }
            _ => {}
        }
        i += 1;
    }

    log::info!(
        "Starting GlassChain node id={node_id}  listen={listen_addr}  difficulty={difficulty}"
    );

    let mut org_root_pem: Option<String> = None;
    let identity = org_name.as_ref().map(|org| {
        let identity_name = identity_node_id.clone().unwrap_or_else(|| node_id.clone());
        let mut organization = match Organization::new(org.clone()) {
            Ok(v) => v,
            Err(e) => {
                log::error!("Failed to create organization `{org}`: {e}");
                std::process::exit(1);
            }
        };
        let issued_identity = match organization.issue_identity(identity_name.clone()) {
            Ok(v) => v.clone(),
            Err(e) => {
                log::error!(
                    "Failed to issue identity `{identity_name}` from organization `{org}`: {e}"
                );
                std::process::exit(1);
            }
        };
        log::info!(
            "Using identity-backed TLS certificate for node `{identity_name}` issued by organization `{org}`"
        );
        org_root_pem = Some(organization.root_ca_cert_pem.clone());
        Arc::new(issued_identity)
    });

    // Build the node — optionally backed by persistent Sled storage.
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
            match SledStorageProvider::open(path) {
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
            let mut verifier = match CertChainVerifier::from_pem(org, root_pem) {
                Ok(v) => v,
                Err(e) => {
                    log::error!("Failed to build certificate verifier for `{org}`: {e}");
                    std::process::exit(1);
                }
            };
            let load = |v: &mut CertChainVerifier, p: &std::path::Path| {
                v.add_federation_root_file(p).map_err(|e| e.to_string())
            };
            let result: Result<usize, String> = match std::fs::metadata(path) {
                Ok(meta) if meta.is_dir() => {
                    let mut added = 0usize;
                    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(path) {
                        Ok(entries) => entries
                            .filter_map(|entry| entry.ok().map(|e| e.path()))
                            .filter(|p| p.extension().is_some_and(|ext| ext == "pem"))
                            .collect(),
                        Err(e) => {
                            log::error!("Failed to read trust store directory `{path}`: {e}");
                            std::process::exit(1);
                        }
                    };
                    files.sort();
                    for file in &files {
                        if let Err(e) = load(&mut verifier, file) {
                            log::error!(
                                "Failed to load trust anchor from `{}`: {e}",
                                file.display()
                            );
                            std::process::exit(1);
                        }
                        added += 1;
                    }
                    Ok(added)
                }
                Ok(_) => load(&mut verifier, std::path::Path::new(path)).map(|()| 1usize),
                Err(e) => Err(e.to_string()),
            };
            match result {
                Ok(files) => {
                    node.set_cert_verifier(verifier).await;
                    log::info!(
                        "Certificate verification enabled: own organization `{org}` plus {files} trust-store file(s) from `{path}` (peers from untrusted organizations are rejected on org-gated paths)"
                    );
                }
                Err(e) => {
                    log::error!("Failed to load trust store at `{path}`: {e}");
                    std::process::exit(1);
                }
            }
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
            match &evt {
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
                    let quorum_attestations = quorum.attestations.len();
                    log::info!(
                        "[event] Block mined: index={index} hash={} quorum_attestations={quorum_attestations}",
                        &hash[..8],
                    );
                }
                NodeEvent::BlockReceived {
                    index,
                    hash,
                    certificate: quorum,
                } => {
                    let quorum_attestations = quorum.attestations.len();
                    log::info!(
                        "[event] Block received from peer: index={index} hash={} quorum_attestations={quorum_attestations}",
                        &hash[..8],
                    );
                }
                NodeEvent::PeerConnected(addr) => {
                    log::info!("[event] Peer connected: {addr}");
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
                    log::info!(
                        "[event] Watcher trigger {trigger_id} generated tx={transaction_id}"
                    );
                }
            }
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

    // Interactive REPL.
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

            ReplCommand::Quit => {
                println!("Shutting down.");
                break;
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{parse_command, parse_price, ReplCommand};

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
}
