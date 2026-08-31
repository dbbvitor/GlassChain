//! Node-level endorsement enforcement tests (ADR-008 §4, ticket #45).
//!
//! Every scenario runs through the real commit path: submit → mine → assert
//! on the committed chain, the pending pool, and the derived world state. The
//! `endorsement` capability is activated at a future height, a policy set is
//! committed in-band via a `PolicyUpdate`, and an MSP endorsement provider is
//! attached to the node.
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use glasschain_core::{
    capability_hash, CanonicalRecord, CapabilityActivation, ContractExecution, CoreError,
    EndorserIdentity, ExecutionLimits, ExecutionProvider, ExecutionResult, PersistentWrite,
    PolicyExpression, PolicyUpdate, Principal, PurchaseConditions, RecordSignature, ScopedPolicies,
    ScopedTarget, SmartContractDef, Transaction, TransactionEndorsement, TransactionKind, WriteOp,
    WriteVisibility,
};
use glasschain_identity::{Identity, MspEndorsementProvider};
use glasschain_network::Node;
use std::collections::BTreeMap;
use std::sync::Arc;

const SUPPLY: &str = "supply";
const INVENTORY: &str = "inventory";

// ── Helpers ──────────────────────────────────────────────────────────────────

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

/// Deterministic write producer: every execution writes the same fixed
/// persistent writes, so scenarios can bind declared scopes to committed ones.
struct WritingExecutionProvider {
    writes: Vec<PersistentWrite>,
}

impl ExecutionProvider for WritingExecutionProvider {
    fn execute(
        &self,
        _contract_id: &str,
        _payload: &[u8],
        _limits: ExecutionLimits,
    ) -> Result<ExecutionResult, CoreError> {
        Ok(ExecutionResult {
            ephemeral: Vec::new(),
            writes: self.writes.clone(),
        })
    }

    fn name(&self) -> &'static str {
        "test-writer"
    }
}

fn write(channel: &str, contract: &str, key: &str) -> PersistentWrite {
    PersistentWrite {
        channel: channel.into(),
        contract: contract.into(),
        key: key.into(),
        op: WriteOp::Set(b"v".to_vec()),
        visibility: WriteVisibility::Public,
    }
}

fn target(channel: &str, contract: &str, keys: &[&str]) -> ScopedTarget {
    ScopedTarget {
        channel: channel.into(),
        contract: contract.into(),
        keys: keys.iter().map(|key| (*key).to_owned()).collect(),
        collection: None,
    }
}

fn pdc_target(channel: &str, contract: &str, keys: &[&str], collection: &str) -> ScopedTarget {
    ScopedTarget {
        collection: Some(collection.into()),
        ..target(channel, contract, keys)
    }
}

/// Sign `tx` with each identity under its claimed principal and attach the
/// signatures as one endorsement carrier over `target`.
fn endorsed(
    mut tx: Transaction,
    scope: ScopedTarget,
    signers: &[(&Identity, &str)],
) -> Transaction {
    let payload = TransactionEndorsement::payload(&tx).unwrap();
    tx.endorsements = vec![TransactionEndorsement {
        target: scope,
        signers: signers
            .iter()
            .map(|(identity, principal)| EndorserIdentity {
                claimed_principal: Principal::new(*principal),
                public_key: identity.public_key_bytes().to_vec(),
                signature: identity.sign_bytes(&payload),
            })
            .collect(),
    }];
    tx
}

/// A registered contract with a (fake) WASM payload: the write producer runs
/// for every execution of it.
fn writer_contract_tx() -> Transaction {
    Transaction::new(TransactionKind::ContractCreation(SmartContractDef {
        contract_id: "writer-contract".into(),
        buyer_id: "buyer-1".into(),
        product_id: "SKU-1".into(),
        conditions: PurchaseConditions {
            max_price_per_unit: 1,
            min_quantity: 1,
            max_quantity: 1,
            max_lead_time_days: 1,
            preferred_seller_id: None,
            currency: "USD".into(),
            auto_execute: false,
        },
        wasm_code_b64: Some(BASE64_STANDARD.encode(b"fake wasm")),
    }))
}

fn writer_execution_tx() -> Transaction {
    Transaction::new(TransactionKind::ContractExecution(ContractExecution {
        contract_id: "writer-contract".into(),
        purchase_order_tx_id: "po-1".into(),
        buyer_id: "buyer-1".into(),
        seller_id: "seller-1".into(),
        product_id: "SKU-1".into(),
        quantity: 1,
        total_price: 100,
        currency: "USD".into(),
    }))
}

fn activation_tx(height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:endorsement:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: "endorsement".into(),
            version: 1,
            hash: capability_hash("endorsement", 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                signer: "governance".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

/// The base committed policy for `(supply, inventory)`: channel writes need
/// `org-a`, the `k1` key needs `org-b`, and the `pricing` collection needs
/// `auditor`.
fn base_policy() -> ScopedPolicies {
    ScopedPolicies {
        channel_default: PolicyExpression::signed_by("org-a"),
        contract_default: None,
        collection_policy: Some(PolicyExpression::signed_by("auditor")),
        key_policies: vec![("k1".to_owned(), PolicyExpression::signed_by("org-b"))],
    }
}

/// Submit a `PolicyUpdate` endorsed under the *current* effective policy for
/// its scope (`signers` must satisfy it).
fn policy_update_tx(
    channel: &str,
    contract: &str,
    policies: ScopedPolicies,
    signers: &[(&Identity, &str)],
) -> Transaction {
    let keys: Vec<String> = policies
        .key_policies
        .iter()
        .map(|(key, _)| key.clone())
        .collect();
    let tx = Transaction::new(TransactionKind::PolicyUpdate(PolicyUpdate {
        channel: channel.into(),
        contract: contract.into(),
        policies,
    }));
    let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    endorsed(tx, target(channel, contract, &key_refs), signers)
}

fn custody_tx() -> Transaction {
    let mut payload = BTreeMap::new();
    payload.insert(
        "shipment_ref".to_owned(),
        serde_json::Value::String("ship-1".to_owned()),
    );
    payload.insert(
        "receiver_id".to_owned(),
        serde_json::Value::String("org-b".to_owned()),
    );
    payload.insert(
        "received_at".to_owned(),
        serde_json::Value::String("2026-08-31".to_owned()),
    );
    let mut record = CanonicalRecord::new(0, "delivery_receipt", payload, "org-a");
    record.signatures.push(RecordSignature {
        signer: "org-a".into(),
        signature_bytes: vec![0x42],
    });
    Transaction::new(TransactionKind::CanonicalRecord(record))
}

fn recall_tx() -> Transaction {
    let mut payload = BTreeMap::new();
    payload.insert(
        "lot_ref".to_owned(),
        serde_json::Value::String("lot-1".to_owned()),
    );
    payload.insert(
        "reason".to_owned(),
        serde_json::Value::String("contamination".to_owned()),
    );
    payload.insert(
        "status".to_owned(),
        serde_json::Value::String("issued".to_owned()),
    );
    payload.insert(
        "issued_by".to_owned(),
        serde_json::Value::String("org-b".to_owned()),
    );
    let mut record = CanonicalRecord::new(0, "recall", payload, "org-a");
    record.signatures.push(RecordSignature {
        signer: "org-a".into(),
        signature_bytes: vec![0x42],
    });
    Transaction::new(TransactionKind::CanonicalRecord(record))
}

/// A node with the endorsement provider attached, the `endorsement` capability
/// active from height 2, and the base policy committed (block 2).
struct Harness {
    node: Node,
    gov: Identity,
    org_a: Identity,
    org_b: Identity,
    auditor: Identity,
}

async fn setup(writes: Vec<PersistentWrite>) -> Harness {
    let node = Node::new("endorse-node", free_addr(), 1);
    node.start(vec![]).await.unwrap();

    let gov = Identity::generate("gov");
    let org_a = Identity::generate("org-a");
    let org_b = Identity::generate("org-b");
    let auditor = Identity::generate("auditor");
    let mut msp = MspEndorsementProvider::new();
    msp.register_identity(&gov, Principal::new("network-governance"));
    msp.register_identity(&org_a, Principal::new("org-a"));
    msp.register_identity(&org_b, Principal::new("org-b"));
    msp.register_identity(&auditor, Principal::new("auditor"));
    node.set_endorsement_provider(Arc::new(msp)).await;
    node.set_execution_provider(Arc::new(WritingExecutionProvider { writes }))
        .await;

    node.submit_transaction(activation_tx(2)).await.unwrap();
    node.mine().await.unwrap();

    node.submit_transaction(policy_update_tx(
        SUPPLY,
        INVENTORY,
        base_policy(),
        &[(&gov, "network-governance")],
    ))
    .await
    .unwrap();
    node.mine().await.unwrap();
    Harness {
        node,
        gov,
        org_a,
        org_b,
        auditor,
    }
}

/// Register the write-producing contract and commit it.
async fn setup_writer(node: &Node) {
    node.submit_transaction(writer_contract_tx()).await.unwrap();
    node.mine().await.unwrap();
}

async fn chain_len(node: &Node) -> usize {
    node.ledger_snapshot().await.chain.len()
}

// ── Scenarios ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn failed_authorization_rejects_at_admission_with_no_state_change() {
    // The k1 key policy requires org-b; the carrier only carries org-a.
    let Harness { node, org_a, .. } = setup(vec![write(SUPPLY, INVENTORY, "k1")]).await;
    let before = chain_len(&node).await;

    let tx = endorsed(
        writer_execution_tx(),
        target(SUPPLY, INVENTORY, &["k1"]),
        &[(&org_a, "org-a")],
    );
    let error = node
        .submit_transaction(tx)
        .await
        .expect_err("unsatisfied key policy must reject at admission");
    assert!(error.to_string().contains("failed policy"), "{error}");

    assert_eq!(chain_len(&node).await, before, "nothing committed");
    assert!(
        node.world_state().await.is_empty(),
        "no partial state materialized"
    );
}

#[tokio::test]
async fn write_outside_declared_scope_rejects_block_and_restores_pool() {
    // The carrier authorizes k1, but the execution writes k2 — admission
    // cannot see the write, the commit path binds it and rejects the block.
    let Harness {
        node, org_a, org_b, ..
    } = setup(vec![write(SUPPLY, INVENTORY, "k2")]).await;
    setup_writer(&node).await;
    let before = chain_len(&node).await;

    let tx = endorsed(
        writer_execution_tx(),
        target(SUPPLY, INVENTORY, &["k1"]),
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    );
    node.submit_transaction(tx.clone()).await.unwrap();
    let error = node
        .mine()
        .await
        .expect_err("out-of-scope write must reject the whole block");
    assert!(
        error.to_string().contains("outside every declared"),
        "{error}"
    );

    assert_eq!(chain_len(&node).await, before, "block not committed");
    let ledger = node.ledger_snapshot().await;
    assert_eq!(
        ledger.pending_transactions.len(),
        1,
        "the transaction returns to the pending pool"
    );
    assert_eq!(ledger.pending_transactions[0].id, tx.id);
    assert!(
        node.world_state().await.is_empty(),
        "no partial state materialized"
    );
}

#[tokio::test]
async fn multi_key_transaction_satisfies_every_applicable_layer() {
    let Harness {
        node, org_a, org_b, ..
    } = setup(vec![
        write(SUPPLY, INVENTORY, "k1"),
        write(SUPPLY, INVENTORY, "k2"),
    ])
    .await;
    setup_writer(&node).await;

    // Channel layer (org-a) plus the k1 key layer (org-b); k2 has no policy.
    let tx = endorsed(
        writer_execution_tx(),
        target(SUPPLY, INVENTORY, &["k1", "k2"]),
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    );
    node.submit_transaction(tx).await.unwrap();
    node.mine().await.unwrap();

    let ledger = node.ledger_snapshot().await;
    let block = ledger.chain.last().unwrap();
    assert_eq!(block.write_set.len(), 2, "both keys committed");
    assert!(
        node.world_state()
            .await
            .contains_key("ws:supply:inventory:k1"),
        "state materialized after successful endorsement"
    );
}

#[tokio::test]
async fn distinct_signer_counting_holds_at_the_commit_path() {
    let Harness {
        node,
        gov,
        org_a,
        org_b,
        ..
    } = setup(vec![write(SUPPLY, "strict", "s1")]).await;
    setup_writer(&node).await;

    // Commit a 2-of-2 channel policy for the strict scope, authorized under
    // the fail-closed default by the network-governance principal.
    node.submit_transaction(policy_update_tx(
        SUPPLY,
        "strict",
        ScopedPolicies {
            channel_default: PolicyExpression::NOutOf {
                required: 2,
                rules: vec![
                    PolicyExpression::signed_by("org-a"),
                    PolicyExpression::signed_by("org-b"),
                ],
            },
            contract_default: None,
            collection_policy: None,
            key_policies: Vec::new(),
        },
        &[(&gov, "network-governance")],
    ))
    .await
    .unwrap();
    node.mine().await.unwrap();

    // org-a signing twice must not satisfy 2-of-2.
    let duplicate = endorsed(
        writer_execution_tx(),
        target(SUPPLY, "strict", &["s1"]),
        &[(&org_a, "org-a"), (&org_a, "org-a")],
    );
    let error = node
        .submit_transaction(duplicate)
        .await
        .expect_err("duplicate signatures must not inflate the count");
    assert!(error.to_string().contains("failed policy"), "{error}");

    // Two distinct organizations satisfy it and the write commits.
    let valid = endorsed(
        writer_execution_tx(),
        target(SUPPLY, "strict", &["s1"]),
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    );
    node.submit_transaction(valid).await.unwrap();
    node.mine().await.unwrap();
    assert!(
        node.world_state().await.contains_key("ws:supply:strict:s1"),
        "the write commits under distinct signatures"
    );
}

#[tokio::test]
async fn pdc_write_requires_collection_endorsement_not_just_membership() {
    let Harness {
        node,
        org_a,
        auditor,
        ..
    } = setup(vec![PersistentWrite {
        visibility: WriteVisibility::Pdc("pricing".into()),
        ..write(SUPPLY, INVENTORY, "price-key")
    }])
    .await;
    setup_writer(&node).await;

    // org-a satisfies the channel layer (a member-shaped principal) but the
    // collection endorsement policy requires `auditor`: membership alone is
    // never an endorsement (ADR-008 decision 1). The membership registry
    // itself lands with private-data dissemination (#46/#47).
    let member_only = endorsed(
        writer_execution_tx(),
        pdc_target(SUPPLY, INVENTORY, &["price-key"], "pricing"),
        &[(&org_a, "org-a")],
    );
    let error = node
        .submit_transaction(member_only)
        .await
        .expect_err("collection endorsement policy must be enforced");
    assert!(error.to_string().contains("failed policy"), "{error}");

    let endorsed_write = endorsed(
        writer_execution_tx(),
        pdc_target(SUPPLY, INVENTORY, &["price-key"], "pricing"),
        &[(&org_a, "org-a"), (&auditor, "auditor")],
    );
    node.submit_transaction(endorsed_write).await.unwrap();
    node.mine().await.unwrap();
    let ledger = node.ledger_snapshot().await;
    assert_eq!(
        ledger.chain.last().unwrap().write_set.len(),
        1,
        "the collection-endorsed PDC write commits"
    );
}

#[tokio::test]
async fn custody_handoff_requires_sender_and_receiving_custodian_2_of_2() {
    let Harness {
        node, org_a, org_b, ..
    } = setup(vec![]).await;
    let before = chain_len(&node).await;

    // Only the sender signs: the custody operation default (2-of-2) fails.
    let sender_only = endorsed(
        custody_tx(),
        target(SUPPLY, INVENTORY, &[]),
        &[(&org_a, "org-a")],
    );
    let error = node
        .submit_transaction(sender_only)
        .await
        .expect_err("custody handoff must require both custodians");
    assert!(error.to_string().contains("operation default"), "{error}");

    // Sender and receiving custodian both sign: the record commits.
    let both = endorsed(
        custody_tx(),
        target(SUPPLY, INVENTORY, &[]),
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    );
    node.submit_transaction(both).await.unwrap();
    node.mine().await.unwrap();
    assert_eq!(chain_len(&node).await, before + 1);
}

#[tokio::test]
async fn recall_transition_requires_the_multi_party_authority_signature() {
    // The v1 recall operation default: the issuing custodian (envelope
    // issuer) and the authorized authority (payload `issued_by`) must both
    // sign — 2-of-2 multi-party.
    let Harness {
        node, org_a, org_b, ..
    } = setup(vec![]).await;
    let before = chain_len(&node).await;

    let issuer_only = endorsed(
        recall_tx(),
        target(SUPPLY, INVENTORY, &[]),
        &[(&org_a, "org-a")],
    );
    let error = node
        .submit_transaction(issuer_only)
        .await
        .expect_err("recall must require the authorized authority");
    assert!(error.to_string().contains("operation default"), "{error}");
    assert_eq!(chain_len(&node).await, before, "nothing committed");

    let multi_party = endorsed(
        recall_tx(),
        target(SUPPLY, INVENTORY, &[]),
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    );
    node.submit_transaction(multi_party).await.unwrap();
    node.mine().await.unwrap();
    assert_eq!(chain_len(&node).await, before + 1);
}

#[tokio::test]
async fn policy_update_activates_only_after_its_block_commits() {
    let Harness {
        node, org_a, org_b, ..
    } = setup(vec![write(SUPPLY, INVENTORY, "k1")]).await;
    setup_writer(&node).await;

    // Re-key the channel default to org-b. The update itself is authorized
    // under the *current* policy: channel (org-a) + k1 key (org-b).
    node.submit_transaction(policy_update_tx(
        SUPPLY,
        INVENTORY,
        ScopedPolicies {
            channel_default: PolicyExpression::signed_by("org-b"),
            contract_default: None,
            collection_policy: None,
            key_policies: vec![("k1".to_owned(), PolicyExpression::signed_by("org-b"))],
        },
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    ))
    .await
    .unwrap();
    node.mine().await.unwrap();

    // Under the new policy org-b alone suffices…
    let new_signer = endorsed(
        writer_execution_tx(),
        target(SUPPLY, INVENTORY, &["k1"]),
        &[(&org_b, "org-b")],
    );
    node.submit_transaction(new_signer).await.unwrap();
    node.mine().await.unwrap();
    assert!(
        node.world_state()
            .await
            .contains_key("ws:supply:inventory:k1"),
        "the write commits under the new policy"
    );

    // …and the old principal no longer satisfies the channel layer.
    let old_signer = endorsed(
        writer_execution_tx(),
        target(SUPPLY, INVENTORY, &["k1"]),
        &[(&org_a, "org-a")],
    );
    let error = node
        .submit_transaction(old_signer)
        .await
        .expect_err("the retired policy must not authorize writes");
    assert!(error.to_string().contains("failed policy"), "{error}");
}

#[tokio::test]
async fn same_block_policy_change_and_write_conflict_is_rejected() {
    let Harness {
        node, org_a, org_b, ..
    } = setup(vec![write(SUPPLY, INVENTORY, "k1")]).await;
    setup_writer(&node).await;
    let before = chain_len(&node).await;

    // The update re-keys k1 and a later transaction in the same candidate
    // block writes k1: rejected whole, both transactions restored.
    node.submit_transaction(policy_update_tx(
        SUPPLY,
        INVENTORY,
        ScopedPolicies {
            channel_default: PolicyExpression::signed_by("org-b"),
            contract_default: None,
            collection_policy: None,
            key_policies: vec![("k1".to_owned(), PolicyExpression::signed_by("org-a"))],
        },
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    ))
    .await
    .unwrap();
    let writer = endorsed(
        writer_execution_tx(),
        target(SUPPLY, INVENTORY, &["k1"]),
        &[(&org_a, "org-a"), (&org_b, "org-b")],
    );
    node.submit_transaction(writer).await.unwrap();

    let error = node
        .mine()
        .await
        .expect_err("same-block policy update + write must be rejected");
    assert!(error.to_string().contains("same block"), "{error}");
    assert_eq!(chain_len(&node).await, before, "block not committed");
    assert_eq!(
        node.ledger_snapshot().await.pending_transactions.len(),
        2,
        "both transactions return to the pool"
    );
    assert!(
        node.world_state().await.is_empty(),
        "no partial state materialized"
    );
}

#[tokio::test]
async fn enforcement_stays_dormant_until_the_capability_activates() {
    // Provider attached, but the `endorsement` capability is never activated:
    // writes commit without carriers, exactly as before #45.
    let node = Node::new("dormant-node", free_addr(), 1);
    node.start(vec![]).await.unwrap();
    let identity = Identity::generate("org-a");
    let mut msp = MspEndorsementProvider::new();
    msp.register_identity(&identity, Principal::new("org-a"));
    node.set_endorsement_provider(Arc::new(msp)).await;
    node.set_execution_provider(Arc::new(WritingExecutionProvider {
        writes: vec![write(SUPPLY, INVENTORY, "k1")],
    }))
    .await;

    node.submit_transaction(writer_contract_tx()).await.unwrap();
    node.mine().await.unwrap();
    node.submit_transaction(writer_execution_tx())
        .await
        .unwrap();
    node.mine().await.unwrap();

    let ledger = node.ledger_snapshot().await;
    assert_eq!(
        ledger.chain.last().unwrap().write_set.len(),
        1,
        "the write commits without endorsement while the capability is inactive"
    );
}
