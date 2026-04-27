use glasschain_core::{
    InventoryUpdate, PurchaseConditions, PurchaseOrder, SmartContractDef, SupplyOffer,
    TraceableAsset, TraceableAssetRegistration, Transaction, TransactionKind,
};
use glasschain_identity::Organization;
use glasschain_network::{Node, NodeEvent};
use glasschain_storage::SledStorageProvider;
use glasschain_vm::WasmExecutionProvider;
use std::env;
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

    mine
        Mine a block with all pending transactions and wait for completion.

    mine-async
        Start mining in the background and return immediately.

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
            _ => {}
        }
        i += 1;
    }

    log::info!(
        "Starting GlassChain node id={node_id}  listen={listen_addr}  difficulty={difficulty}"
    );

    let identity = if let Some(ref org) = org_name {
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
            "Using identity-backed TLS certificate for node `{}` issued by organization `{}`",
            identity_name,
            org
        );
        Some(Arc::new(issued_identity))
    } else {
        None
    };

    // Build the node — optionally backed by persistent Sled storage.
    let node = if let Some(ref path) = storage_path {
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
    } else if let Some(identity) = identity {
        Arc::new(Node::new_with_identity(
            node_id.clone(),
            listen_addr.clone(),
            difficulty,
            identity,
        ))
    } else {
        Arc::new(Node::new(node_id.clone(), listen_addr.clone(), difficulty))
    };

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

    // Spawn event logger.
    let mut events = node.subscribe();
    tokio::spawn(async move {
        while let Ok(evt) = events.recv().await {
            match &evt {
                NodeEvent::TransactionAccepted(tx) => {
                    log::info!("[event] Transaction accepted: {}", tx.id);
                }
                NodeEvent::BlockMined { index, hash } => {
                    log::info!("[event] Block mined: index={index} hash={}", &hash[..8]);
                }
                NodeEvent::BlockReceived { index, hash } => {
                    log::info!(
                        "[event] Block received from peer: index={index} hash={}",
                        &hash[..8]
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

    println!("GlassChain node `{node_id}` is running on {listen_addr}");
    println!("Type 'help' for available commands.\n");

    // Interactive REPL.
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    loop {
        line.clear();
        print!("> ");
        use std::io::Write;
        let _ = std::io::stdout().flush();

        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            break; // EOF
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "help" | "?" => {
                usage();
            }

            "supply" => {
                if parts.len() < 8 {
                    eprintln!("Usage: supply <seller> <product_id> <product_name> <qty> <price> <lead_days> <currency>");
                    continue;
                }
                let qty: u64 = if let Ok(v) = parts[4].parse() {
                    v
                } else {
                    eprintln!("Invalid quantity");
                    continue;
                };
                let price: u64 = if let Some(v) = parse_price(parts[5]) {
                    v
                } else {
                    eprintln!("Invalid price (use decimal like 12.50)");
                    continue;
                };
                let lead: u32 = if let Ok(v) = parts[6].parse() {
                    v
                } else {
                    eprintln!("Invalid lead_days");
                    continue;
                };
                let tx = Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
                    product_id: parts[2].to_owned(),
                    product_name: parts[3].to_owned(),
                    seller_id: parts[1].to_owned(),
                    quantity_available: qty,
                    price_per_unit: price,
                    lead_time_days: lead,
                    currency: parts[7].to_owned(),
                }));
                match node.submit_transaction(tx).await {
                    Ok(()) => println!("Supply offer submitted."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "order" => {
                if parts.len() < 7 {
                    eprintln!("Usage: order <buyer> <seller> <product> <qty> <price> <currency>");
                    continue;
                }
                let qty: u64 = if let Ok(v) = parts[4].parse() {
                    v
                } else {
                    eprintln!("Invalid quantity");
                    continue;
                };
                let price: u64 = if let Some(v) = parse_price(parts[5]) {
                    v
                } else {
                    eprintln!("Invalid price");
                    continue;
                };
                let tx = Transaction::new(TransactionKind::PurchaseOrder(PurchaseOrder {
                    product_id: parts[3].to_owned(),
                    buyer_id: parts[1].to_owned(),
                    seller_id: parts[2].to_owned(),
                    quantity: qty,
                    agreed_price_per_unit: price,
                    currency: parts[6].to_owned(),
                    contract_id: None,
                }));
                match node.submit_transaction(tx).await {
                    Ok(()) => println!("Purchase order submitted."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "contract" => {
                if parts.len() < 9 {
                    eprintln!(
                        "Usage: contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>"
                    );
                    continue;
                }
                let max_price: u64 = if let Some(v) = parse_price(parts[4]) {
                    v
                } else {
                    eprintln!("Invalid max_price");
                    continue;
                };
                let min_qty: u64 = if let Ok(v) = parts[5].parse() {
                    v
                } else {
                    eprintln!("Invalid min_qty");
                    continue;
                };
                let max_qty: u64 = if let Ok(v) = parts[6].parse() {
                    v
                } else {
                    eprintln!("Invalid max_qty");
                    continue;
                };
                let max_lead: u32 = if let Ok(v) = parts[7].parse() {
                    v
                } else {
                    eprintln!("Invalid max_lead");
                    continue;
                };
                let tx = Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
                    contract_id: parts[1].to_owned(),
                    buyer_id: parts[2].to_owned(),
                    product_id: parts[3].to_owned(),
                    conditions: PurchaseConditions {
                        max_price_per_unit: max_price,
                        min_quantity: min_qty,
                        max_quantity: max_qty,
                        max_lead_time_days: max_lead,
                        preferred_seller_id: None,
                        currency: parts[8].to_owned(),
                        auto_execute: true,
                    },
                    wasm_code_b64: None,
                }));
                match node.submit_transaction(tx).await {
                    Ok(()) => println!("Smart contract created."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "inventory" => {
                if parts.len() < 5 {
                    eprintln!("Usage: inventory <owner> <product> <delta> <reason>");
                    continue;
                }
                let delta: i64 = if let Ok(v) = parts[3].parse() {
                    v
                } else {
                    eprintln!("Invalid delta");
                    continue;
                };
                let reason = parts[4..].join(" ");
                let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
                    owner_id: parts[1].to_owned(),
                    product_id: parts[2].to_owned(),
                    quantity_delta: delta,
                    reason,
                }));
                match node.submit_transaction(tx).await {
                    Ok(()) => println!("Inventory update submitted."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "asset" => {
                if parts.len() < 9 {
                    eprintln!(
                        "Usage: asset <originator> <product_name> <gtin> <batch> <expiry> <serial> <qty> <event_type>"
                    );
                    continue;
                }
                let qty: u64 = if let Ok(v) = parts[7].parse() {
                    v
                } else {
                    eprintln!("Invalid qty");
                    continue;
                };
                let opt = |s: &str| if s == "-" { None } else { Some(s.to_owned()) };
                let asset = TraceableAsset {
                    gtin: opt(parts[3]),
                    batch_number: opt(parts[4]),
                    expiry_date: opt(parts[5]),
                    serial_number: opt(parts[6]),
                    anvisa_registration: None,
                    manufacturer_id: None,
                    product_name: parts[2].to_owned(),
                    custodian_id: parts[1].to_owned(),
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
                        event_type: parts[8].to_owned(),
                        originator_id: parts[1].to_owned(),
                        purchase_order_ref: None,
                    },
                ));
                match node.submit_transaction(tx).await {
                    Ok(()) => println!("Asset registration submitted."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "mine" => match node.mine().await {
                Ok(()) => println!("Block mined."),
                Err(e) => eprintln!("Error mining: {e}"),
            },

            "mine-async" => {
                let node_ref = Arc::clone(&node);
                tokio::spawn(async move {
                    match node_ref.mine_async().await {
                        Ok(()) => println!("Block mined."),
                        Err(e) => eprintln!("Error mining: {e}"),
                    }
                });
                println!("Mining started in the background…");
            }

            "chain" => {
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

            "pending" => {
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
                    };
                    println!("  {} [{}]", tx.id, kind);
                }
            }

            "peers" => {
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

            "contracts" => {
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

            "quit" | "exit" => {
                println!("Shutting down.");
                break;
            }

            other => {
                eprintln!("Unknown command: {other:?}. Type 'help' for usage.");
            }
        }
    }
}
