//! Cross-validator BFT vote rounds on the wire (ADR-002 adoption gate,
//! ADR-009 on-chain validator registry, ADR-014 aggregation).
//!
//! Scenario: four validators derive their set from the **on-chain registry**
//! (world state under `governance/validator-registry/<name>`, written through
//! the contract seam), activate `bft_consensus`, and a round driver run by the
//! height's leader produces a real multi-signer BLS quorum certificate —
//! prevote → precommit — committed and replicated with the block.

#![cfg(feature = "bft")]

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use bls_signatures::{PrivateKey, Serialize as _};
use glasschain_core::{
    capability_hash, BftConsensusProvider, CapabilityActivation, ExecutionLimits,
    ExecutionProvider, ExecutionResult, PersistentWrite, RecordSignature, Transaction,
    TransactionKind, ValidatorInfo, WriteOp, WriteVisibility,
};
use glasschain_network::Node;
use std::sync::Arc;
use std::time::Duration;

const CHANNEL: &str = "governance";
const CONTRACT: &str = "validator-registry";
const VALIDATORS: usize = 4;

fn free_addr() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

/// Deterministic BLS validator keys; canonical order is the key index.
fn validator_keys() -> Vec<PrivateKey> {
    let mut seed = 0u8;
    (0..VALIDATORS)
        .map(|_| {
            seed += 1;
            PrivateKey::new([seed; 64])
        })
        .collect()
}

fn validators_with_pops(keys: &[PrivateKey]) -> Vec<ValidatorInfo> {
    keys.iter()
        .enumerate()
        .map(|(i, key)| {
            let public = key.public_key();
            ValidatorInfo {
                name: format!("validator-{i}"),
                public_key: public.as_bytes(),
                pop: key
                    .sign(format!(
                        "glasschain-bls-pop:{}",
                        hex::encode(public.as_bytes())
                    ))
                    .as_bytes(),
            }
        })
        .collect()
}

/// The registry descriptor stored in world state (what
/// `derive_validator_provider` parses).
fn registry_value(key: &PrivateKey) -> Vec<u8> {
    let public = key.public_key();
    let pop = key.sign(format!(
        "glasschain-bls-pop:{}",
        hex::encode(public.as_bytes())
    ));
    serde_json::json!({
        "public_key": BASE64_STANDARD.encode(public.as_bytes()),
        "pop": BASE64_STANDARD.encode(pop.as_bytes()),
    })
    .to_string()
    .into_bytes()
}

/// The test's stand-in for the governance contract that manages membership:
/// `execute_with_state` maps `commit:validator-registry:register-{i}` to that
/// validator's registry write (real deployments use a WASM governance
/// contract; the seam is identical).
struct RegistryProvider {
    keys: Vec<PrivateKey>,
}

impl ExecutionProvider for RegistryProvider {
    fn execute(
        &self,
        _contract_id: &str,
        _payload: &[u8],
        _limits: ExecutionLimits,
    ) -> Result<ExecutionResult, glasschain_core::CoreError> {
        Ok(ExecutionResult::default())
    }

    fn execute_with_state(
        &self,
        execution_id: &str,
        _payload: &[u8],
        _initial_state: std::collections::HashMap<String, Vec<u8>>,
        _limits: ExecutionLimits,
    ) -> Result<ExecutionResult, glasschain_core::CoreError> {
        let index: usize = execution_id
            .rsplit('-')
            .next()
            .and_then(|suffix| suffix.parse().ok())
            .ok_or_else(|| {
                glasschain_core::CoreError::InvalidTransaction(format!(
                    "registry execution id '{execution_id}' does not name a validator"
                ))
            })?;
        Ok(ExecutionResult {
            ephemeral: Vec::new(),
            writes: vec![registry_write(index, &self.keys)],
        })
    }

    fn name(&self) -> &'static str {
        "test-registry"
    }
}

fn registry_write(index: usize, keys: &[PrivateKey]) -> PersistentWrite {
    PersistentWrite {
        channel: CHANNEL.into(),
        contract: CONTRACT.into(),
        key: format!("validator-{index}"),
        op: WriteOp::Set(registry_value(&keys[index])),
        visibility: WriteVisibility::Public,
    }
}

fn activation_tx(height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:bft_consensus:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: "bft_consensus".into(),
            version: 1,
            hash: capability_hash("bft_consensus", 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
                signer: "governance".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

fn plain_tx(id: &str) -> Transaction {
    Transaction::with_id(
        id.to_owned(),
        TransactionKind::InventoryUpdate(glasschain_core::InventoryUpdate {
            product_id: "BFT-SKU".into(),
            owner_id: "owner".into(),
            quantity_delta: 1,
            reason: "vote round test".into(),
        }),
    )
}

async fn poll_until(desc: &str, secs: u64, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if condition().await {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "condition never held: {desc}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn chain_len(node: &Node) -> usize {
    node.ledger_snapshot().await.chain.len()
}

/// The leader for `height`: round-robin over the canonical validator order
/// (ADR-009 — the same walk every node derives from the registry).
const fn leader_for(height: u64) -> usize {
    #[allow(clippy::cast_possible_truncation)]
    {
        height as usize % VALIDATORS
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn vote_rounds_produce_multi_signer_certificates_on_the_wire() {
    let _ = env_logger::try_init();
    let keys = validator_keys();
    let validators = validators_with_pops(&keys);

    // Four nodes; each provider holds the full set but a distinct local key.
    let nodes: Vec<Node> = (0..VALIDATORS)
        .map(|i| {
            let node = Node::new(format!("validator-{i}"), free_addr(), 1);
            node
        })
        .collect();
    let addrs: Vec<String> = nodes.iter().map(|n| n.listen_addr().to_owned()).collect();

    for (i, node) in nodes.iter().enumerate() {
        let provider =
            BftConsensusProvider::new(validators.clone(), keys[i]).expect("valid validators");
        node.set_bft_consensus(Arc::new(provider)).await;
        node.set_execution_provider(Arc::new(RegistryProvider { keys: keys.clone() }))
            .await;
    }

    // Mesh.
    nodes[0].start(vec![]).await.unwrap();
    // Full mesh: blocks are broadcast (never re-relayed), so the leader must
    // reach every validator directly.
    for i in 1..VALIDATORS {
        nodes[i].start(addrs[..i].to_vec()).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(400)).await;

    // ── Block 1 (PoW): register the validator set on-chain ─────────────────
    nodes[0]
        .submit_transaction(Transaction::new(TransactionKind::ContractCreation(
            glasschain_core::SmartContractDef {
                contract_id: CONTRACT.into(),
                buyer_id: "governance".into(),
                product_id: "registry".into(),
                conditions: glasschain_core::PurchaseConditions {
                    max_price_per_unit: 1,
                    min_quantity: 1,
                    max_quantity: 1,
                    max_lead_time_days: 1,
                    preferred_seller_id: None,
                    currency: "BRL".into(),
                    auto_execute: false,
                },
                wasm_code_b64: Some(BASE64_STANDARD.encode(b"fake wasm")),
            },
        )))
        .await
        .unwrap();
    for i in 0..VALIDATORS {
        nodes[0]
            .submit_transaction(Transaction::with_id(
                format!("register-{i}"),
                TransactionKind::ContractExecution(glasschain_core::ContractExecution {
                    contract_id: CONTRACT.into(),
                    purchase_order_tx_id: "po-1".into(),
                    buyer_id: "governance".into(),
                    seller_id: "seller-1".into(),
                    product_id: "registry".into(),
                    quantity: 1,
                    currency: "BRL".into(),
                    total_price: 1,
                }),
            ))
            .await
            .unwrap();
    }
    // Block 1's leader is validator-1 (1 % 4): only the leader's mine()
    // drives the round; every node may submit transactions. Give the
    // broadcast a moment to reach the leader's pool.
    assert_eq!(leader_for(1), 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    nodes[1].mine().await.unwrap();
    poll_until("all nodes hold the registry block", 8, || async {
        let mut all = true;
        for (i, n) in nodes.iter().enumerate() {
            let len = chain_len(n).await;
            if len < 2 {
                eprintln!("poll: node {i} at {len}");
                all = false;
            }
        }
        all
    })
    .await;

    // ── Block 2 (PoW): activate bft_consensus from height 3 onward ─────────
    // The activation is committed in block 2, so block 3 is the first
    // certificate-bearing block (ADR-010 height semantics).
    nodes[leader_for(2)]
        .submit_transaction(activation_tx(3))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    nodes[leader_for(2)].mine().await.unwrap();
    poll_until("all nodes hold the activation block", 10, || async {
        for n in &nodes {
            if chain_len(n).await < 3 {
                return false;
            }
        }
        true
    })
    .await;

    // Block 2 is the PoW activation block; block 3 is the first BFT block.
    for node in &nodes {
        let chain = node.ledger_snapshot().await.chain;
        assert_eq!(chain.len(), 3);
        assert!(
            chain[2].certificate.is_none(),
            "the activation block stays PoW"
        );
    }

    // ── Block 3: first BFT block — the vote round produces the QC ──────────
    let leader3 = leader_for(3);
    nodes[leader3]
        .submit_transaction(plain_tx("tx-after-activation"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    nodes[leader3].mine().await.unwrap();
    poll_until("all nodes hold block 3", 10, || async {
        for n in &nodes {
            if chain_len(n).await < 4 {
                return false;
            }
        }
        true
    })
    .await;
    for node in &nodes {
        let block = node.ledger_snapshot().await.chain.last().cloned().unwrap();
        let certificate = block.certificate.clone().expect("block 3 is BFT-attested");
        let signers = certificate
            .signers_bitmap
            .iter()
            .map(|b| b.count_ones())
            .sum::<u32>();
        assert!(
            signers as usize >= provider_quorum(),
            "block 3's certificate must carry a quorum bitmap, got {signers}"
        );
        // The leader's provider verifies the certificate cryptographically.
        let mut validators = validators_with_pops(&keys);
        validators.sort_by(|a, b| a.name.cmp(&b.name));
        let verifier = BftConsensusProvider::new(validators, keys[0]).unwrap();
        verifier
            .verify_certificate(&certificate, &block)
            .expect("the certificate must verify against the on-chain set");
    }
}

const fn provider_quorum() -> usize {
    VALIDATORS * 2 / 3 + 1
}
