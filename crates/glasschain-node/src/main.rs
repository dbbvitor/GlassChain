use glasschain_core::{
    InventoryUpdate, PurchaseConditions, PurchaseOrder, SmartContractDef, SupplyOffer, Transaction,
    TransactionKind,
};
use glasschain_network::{Node, NodeEvent};
use std::env;
use tokio::io::{AsyncBufReadExt, BufReader};

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
    --help                  Show this help message

INTERACTIVE COMMANDS (after startup):
    supply <seller> <product> <qty> <price> <lead_days> <currency>
        Post a supply offer to the ledger.

    order <buyer> <seller> <product> <qty> <price> <currency>
        Post a manual purchase order.

    contract <contract_id> <buyer> <product> <max_price> <min_qty> <max_qty> <max_lead> <currency>
        Create a smart contract for automatic purchasing.

    inventory <owner> <product> <delta> <reason>
        Post an inventory update.

    mine
        Mine a block with all pending transactions.

    chain
        Print the current chain summary.

    pending
        Print pending transactions.

    peers
        Print known peers.

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
            _ => {}
        }
        i += 1;
    }

    log::info!("Starting GlassChain node id={node_id}  listen={listen_addr}  difficulty={difficulty}");

    let node = Node::new(node_id.clone(), listen_addr.clone(), difficulty);

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
                    log::info!(
                        "[event] Contract {contract_id} auto-executed, qty={quantity}"
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

        let parts: Vec<&str> = line.trim().split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }

        match parts[0] {
            "help" | "?" => {
                usage();
            }

            "supply" => {
                if parts.len() < 7 {
                    eprintln!("Usage: supply <seller> <product> <qty> <price> <lead_days> <currency>");
                    continue;
                }
                let qty: u64 = match parts[3].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid quantity"); continue; }
                };
                let price: f64 = match parts[4].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid price"); continue; }
                };
                let lead: u32 = match parts[5].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid lead_days"); continue; }
                };
                let tx = Transaction::new(TransactionKind::SupplyOffer(SupplyOffer {
                    product_id: parts[2].to_owned(),
                    product_name: parts[2].to_owned(),
                    seller_id: parts[1].to_owned(),
                    quantity_available: qty,
                    price_per_unit: price,
                    lead_time_days: lead,
                    currency: parts[6].to_owned(),
                }));
                match node.submit_transaction(tx).await {
                    Ok(_) => println!("Supply offer submitted."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "order" => {
                if parts.len() < 7 {
                    eprintln!("Usage: order <buyer> <seller> <product> <qty> <price> <currency>");
                    continue;
                }
                let qty: u64 = match parts[4].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid quantity"); continue; }
                };
                let price: f64 = match parts[5].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid price"); continue; }
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
                    Ok(_) => println!("Purchase order submitted."),
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
                let max_price: f64 = match parts[4].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid max_price"); continue; }
                };
                let min_qty: u64 = match parts[5].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid min_qty"); continue; }
                };
                let max_qty: u64 = match parts[6].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid max_qty"); continue; }
                };
                let max_lead: u32 = match parts[7].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid max_lead"); continue; }
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
                }));
                match node.submit_transaction(tx).await {
                    Ok(_) => println!("Smart contract created."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "inventory" => {
                if parts.len() < 5 {
                    eprintln!("Usage: inventory <owner> <product> <delta> <reason>");
                    continue;
                }
                let delta: i64 = match parts[3].parse() {
                    Ok(v) => v,
                    Err(_) => { eprintln!("Invalid delta"); continue; }
                };
                let reason = parts[4..].join(" ");
                let tx = Transaction::new(TransactionKind::InventoryUpdate(InventoryUpdate {
                    owner_id: parts[1].to_owned(),
                    product_id: parts[2].to_owned(),
                    quantity_delta: delta,
                    reason,
                }));
                match node.submit_transaction(tx).await {
                    Ok(_) => println!("Inventory update submitted."),
                    Err(e) => eprintln!("Error: {e}"),
                }
            }

            "mine" => {
                match node.mine().await {
                    Ok(_) => println!("Block mined."),
                    Err(e) => eprintln!("Error: {e}"),
                }
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
                println!("Pending transactions: {}", ledger.pending_transactions.len());
                for tx in &ledger.pending_transactions {
                    let kind = match &tx.kind {
                        TransactionKind::SupplyOffer(_) => "SupplyOffer",
                        TransactionKind::PurchaseOrder(_) => "PurchaseOrder",
                        TransactionKind::ContractCreation(_) => "ContractCreation",
                        TransactionKind::ContractExecution(_) => "ContractExecution",
                        TransactionKind::InventoryUpdate(_) => "InventoryUpdate",
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
