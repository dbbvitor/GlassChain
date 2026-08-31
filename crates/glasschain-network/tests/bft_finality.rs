//! Node-level no-fork, final-at-commit scenario with the Tendermint-class BFT
//! implementation enabled (ticket #42, ADR-002 / ADR-010).
//!
//! The BFT engine is default-off: this file compiles only under the
//! `glasschain-network/bft` feature (CI's `--all-features` runs it).
//!
//! Scenario: a node with a BFT provider attached stays on dev/test `PoW` while
//! the `bft_consensus` capability is dormant; once the activation height is
//! reached (a `CapabilityActivation` record committed by block 1), subsequent
//! blocks carry a real, cryptographically verifiable quorum certificate and are
//! final at commit — single tip, strict chaining, no fork.
#![cfg(feature = "bft")]

use ed25519_dalek::SigningKey;
use glasschain_core::{
    capability_hash, BftConsensusProvider, CapabilityActivation, QuorumCertificate,
    RecordSignature, Transaction, TransactionKind, ValidatorInfo,
};
use glasschain_network::{Node, NodeEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;

/// A `bft_consensus` capability activation declared for `height`.
fn activation_tx(height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:bft_consensus:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: "bft_consensus".into(),
            version: 1,
            hash: capability_hash("bft_consensus", 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                signer: "org-gov".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

/// A provider over one validator (a 1-set is its own ⅔+ quorum).
fn single_validator_provider() -> BftConsensusProvider {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let validators = vec![ValidatorInfo {
        name: "validator-0".into(),
        public_key: signing_key.verifying_key().to_bytes(),
    }];
    BftConsensusProvider::new(validators, signing_key)
}

type BlockMinedEvent = (u64, String, QuorumCertificate);

/// Receive the next `BlockMined` event, skipping unrelated variants. Fails the
/// test instead of hanging (network tests bind real resources; never block).
async fn next_block_mined(events: &mut broadcast::Receiver<NodeEvent>) -> BlockMinedEvent {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
            Ok(Ok(NodeEvent::BlockMined {
                index,
                hash,
                certificate,
            })) => return (index, hash, certificate),
            Ok(Ok(_)) => {}
            Ok(Err(err)) => panic!("event stream failed: {err}"),
            Err(elapsed) => panic!("timed out waiting for BlockMined event: {elapsed}"),
        }
    }
}

#[tokio::test]
async fn bft_final_at_commit_no_fork() {
    let node = Node::new("bft-node", "127.0.0.1:0", 1);
    let mut events = node.subscribe();
    let provider = Arc::new(single_validator_provider());

    // BFT is enabled **before** the capability activates: the engine selection
    // must be capability-gated, not provider-gated (ADR-010 decision 4).
    node.set_bft_consensus(Arc::clone(&provider)).await;

    // Block 1 (height 1 < activation height 2): PoW attestation.
    node.submit_transaction(activation_tx(2)).await.unwrap();
    node.mine().await.unwrap();
    let (index, hash, certificate) = next_block_mined(&mut events).await;
    assert_eq!(index, 1);
    assert!(
        certificate.is_degenerate(),
        "dormant capability must stay PoW"
    );
    assert_eq!(certificate.block_hash, hash);

    let block1 = node
        .shared_ledger()
        .lock()
        .await
        .latest_block()
        .cloned()
        .expect("block 1 committed");
    assert_eq!(block1.index, 1);
    assert!(block1.has_valid_pow(1), "block 1 is PoW-mined");

    // Block 2 (height 2 == activation height): real quorum certificate.
    node.mine().await.unwrap();
    let (index, hash, certificate) = next_block_mined(&mut events).await;
    assert_eq!(index, 2);
    assert!(
        !certificate.is_degenerate(),
        "BFT commit carries attestations"
    );
    assert_eq!(certificate.block_hash, hash);

    let block2 = node
        .shared_ledger()
        .lock()
        .await
        .latest_block()
        .cloned()
        .expect("block 2 committed");
    assert_eq!(block2.index, 2);
    assert!(!block2.has_valid_pow(1), "BFT blocks are not PoW-mined");

    // Final at commit: the certificate attests exactly the committed block,
    // and every attestation is a real ed25519 signature over its hash by a
    // validator in the set (⅔+ distinct).
    certificate
        .validate(&block2)
        .expect("certificate matches block");
    provider
        .verify_certificate(&certificate, &block2)
        .expect("quorum certificate is cryptographically valid");

    // No fork: one tip, strict chaining across the engine swap.
    block2
        .chains_to(&block1)
        .expect("single tip across the swap");

    // Block 3 stays BFT and chains: finality is durable past the swap.
    node.mine().await.unwrap();
    let (index, _hash, certificate) = next_block_mined(&mut events).await;
    assert_eq!(index, 3);
    let block3 = node
        .shared_ledger()
        .lock()
        .await
        .latest_block()
        .cloned()
        .expect("block 3 committed");
    assert_eq!(block3.index, 3);
    assert!(block3.chains_to(&block2).is_ok());
    provider
        .verify_certificate(&certificate, &block3)
        .expect("block 3 is also final at commit");
}
