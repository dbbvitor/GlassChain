// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 dbbvitor
//! The consensus capacity gate (ticket #48, ADR-010 §7): an in-process
//! compact-workload benchmark at 200 and 300 validators.
//!
//! Runs the actual `GlassChain` workload — anchored lot records, certification
//! attestations, and `state_commitment` batch anchors — against a star
//! topology (validators dial the mining leader) and reports:
//!
//! * per-block **leader commit latency** and **block size**,
//! * **certificate size** (the staged engine's attestation set — honest
//!   caveat: one local attestation, no cross-validator vote gossip exists to
//!   measure),
//! * **propagation fan-out** (time until 50% / 95% / 100% of validators hold
//!   the committed height),
//! * **pending-pool backpressure** (depth under sustained submission),
//! * **recovery** after an application-layer partition (a partitioned group
//!   joining late converges to the leader's chain),
//! * **private-data dissemination** measured separately from consensus.
//!
//! Mode: like `madsim_chaos.rs`, this file runs under the real Tokio runtime
//! by default and inside the madsim simulator with
//! `RUSTFLAGS="--cfg madsim"` (deterministic scheduling, seeded runs).
//!
//! Run (ignored by default — the full gate takes minutes):
//! ```bash
//! cargo test -p glasschain-network --test consensus_capacity -- --ignored --nocapture
//! ```
//!
//! Recorded evidence: `docs/benchmarks/consensus-capacity.md`.

use glasschain_core::{
    capability_hash, CanonicalRecord, CapabilityActivation, RecordSignature, Transaction,
    TransactionKind,
};
use glasschain_identity::{CertChainVerifier, Channel, ChannelConfig, Organization};
use glasschain_network::{Node, NodeEvent};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const COLLECTION: &str = "pricing";

#[path = "common/ports.rs"]
mod ports;

use ports::free_addr;

/// The `pdc` capability activation for the dissemination phase.
fn activation_tx(height: u64) -> Transaction {
    Transaction::with_id(
        format!("cap:pdc:{height}"),
        TransactionKind::CapabilityActivation(CapabilityActivation {
            capability_id: "pdc".into(),
            version: 1,
            hash: capability_hash("pdc", 1),
            activation_height: height,
            signatures: vec![RecordSignature {
                algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
                signer: "org-gov".into(),
                signature_bytes: vec![0x42],
            }],
        }),
    )
}

/// A deterministic anchored-lot record (the workload's custody anchor).
fn lot_record(seq: usize) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert("lot_id".to_owned(), Value::String(format!("LOT-{seq}")));
    payload.insert("product_id".to_owned(), Value::String("SKU-CAP".into()));
    payload.insert(
        "batch_number".to_owned(),
        Value::String(format!("B-CAP-{seq}")),
    );
    let mut lot = CanonicalRecord::new(1_700_000_000, "lot", payload, "org-maker");
    lot.record_id = format!("lot-cap-{seq}");
    lot.commitment = lot.commitment().ok();
    lot
}

/// A deterministic `state_commitment` batch anchor (ADR-010 §7 workload).
fn commitment_record(seq: usize) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert(
        "merkle_root".to_owned(),
        Value::String(format!("{seq:064x}")),
    );
    payload.insert(
        "counterparties".to_owned(),
        Value::Array(vec![
            Value::String("org-a".into()),
            Value::String("org-b".into()),
        ]),
    );
    let mut record = CanonicalRecord::new(1_700_000_000, "state_commitment", payload, "org-maker");
    record.record_id = format!("commitment-cap-{seq}");
    record.commitment = record.commitment().ok();
    record.signatures = (0..2)
        .map(|i| RecordSignature {
            algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
            signer: format!("org-{i}"),
            signature_bytes: vec![0x42],
        })
        .collect();
    record
}

/// A deterministic certification anchor referencing the round's lot.
fn certification_record(seq: usize) -> CanonicalRecord {
    let mut payload = BTreeMap::new();
    payload.insert(
        "lot_ref".to_owned(),
        Value::String(format!("lot-cap-{seq}")),
    );
    payload.insert("issuer".to_owned(), Value::String("org-maker".into()));
    payload.insert("scope".to_owned(), Value::String("capacity-run".into()));
    payload.insert("valid_from".to_owned(), Value::String("2026-09-01".into()));
    payload.insert("valid_to".to_owned(), Value::String("2027-09-01".into()));
    payload.insert("status".to_owned(), Value::String("valid".into()));
    let mut evidence = serde_json::Map::new();
    evidence.insert(
        "manifest_commitment".to_owned(),
        Value::String(format!("{seq:064x}")),
    );
    payload.insert("evidence_manifest".to_owned(), Value::Object(evidence));
    let mut attestation =
        CanonicalRecord::new(1_700_000_000, "quality_certification", payload, "org-maker");
    attestation.record_id = format!("cert-cap-{seq}");
    attestation.commitment = attestation.commitment().ok();
    attestation
}

fn signed(record: CanonicalRecord, issuer: &str) -> Transaction {
    let mut signed = record;
    signed.signatures.push(RecordSignature {
        algorithm: glasschain_core::wire::SignatureAlgorithm::Ed25519,
        signer: issuer.to_owned(),
        signature_bytes: vec![0x42],
    });
    Transaction::with_id(
        signed.record_id.clone(),
        TransactionKind::CanonicalRecord(signed),
    )
}

/// A validator star: `validator_count` validators of which `connected` dial
/// the leader; the rest exist but are partitioned (they join at recovery
/// time). Staggered dial waves keep the leader's accept queue manageable.
///
/// Every node carries an org-issued identity and a fail-closed verifier: the
/// private-dissemination phase runs under the zero-trust gate (#86), which
/// refuses org-gated paths without verified certificates.
async fn build_star(validator_count: usize, connected: usize, difficulty: usize) -> ValidatorSet {
    let mut org = Organization::new("CapacityOrg").unwrap();
    let verifier = |org: &Organization| {
        let mut verifier = CertChainVerifier::from_org(org).unwrap();
        verifier.add_crl_pem(&org.crl_pem().unwrap()).unwrap();
        verifier
    };
    let leader_addr = free_addr();
    let leader_identity = org.issue_identity("leader").unwrap().clone();
    let leader = Arc::new(Node::new_with_identity(
        "leader",
        &leader_addr,
        difficulty,
        Arc::new(leader_identity),
    ));
    leader.set_cert_verifier(verifier(&org)).await;
    leader.start(vec![]).await.unwrap();

    let mut validators = Vec::with_capacity(validator_count);
    let mut wave: Vec<Arc<Node>> = Vec::new();
    for idx in 0..validator_count {
        let name = format!("validator-{idx}");
        let identity = org.issue_identity(name.clone()).unwrap().clone();
        let node = Arc::new(Node::new_with_identity(
            name,
            free_addr(),
            difficulty,
            Arc::new(identity),
        ));
        node.set_cert_verifier(verifier(&org)).await;
        if idx < connected {
            node.start(vec![leader_addr.clone()]).await.unwrap();
            wave.push(Arc::clone(&node));
            if wave.len() == 50 {
                tokio::time::sleep(Duration::from_millis(400)).await;
                wave.clear();
            }
        }
        validators.push(node);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    ValidatorSet {
        leader,
        leader_addr,
        validators,
        connected,
    }
}

struct ValidatorSet {
    leader: Arc<Node>,
    leader_addr: String,
    validators: Vec<Arc<Node>>,
    /// How many validators are currently connected (the rest are partitioned
    /// until `recover_partitioned` joins them).
    connected: usize,
}

/// One propagation measurement plus the attribution instrumentation
/// (Step 0, #62): `sweep_ms`/`ticks` expose the cost of the measurement
/// itself, `first_reached` separates the fastest peer from the thresholds.
#[derive(Default, Clone, Copy)]
struct PropagationSample {
    first_reached: Option<u128>,
    p50: Option<u128>,
    p95: Option<u128>,
    p100: Option<u128>,
    sweep_ms: u128,
    ticks: u32,
}

impl PropagationSample {
    const fn p100(&self) -> Option<u128> {
        self.p100
    }
}

impl ValidatorSet {
    /// Time until 50%/95%/100% of the validators hold at least `height`
    /// blocks (integer percent keeps the math off float casts).
    ///
    /// All three thresholds are measured from **one** start in **one** poll
    /// loop (#62 §5.4): the previous implementation ran three sequential
    /// polls, each with its own start time, so the 50% poll absorbed all the
    /// lock contention with ongoing commit work and the later thresholds
    /// measured an already-converged network — incoherent as a propagation
    /// measurement.
    async fn propagation_ms(&self, height: usize) -> PropagationSample {
        let start = Instant::now();
        // Fan-out is measured over the CONNECTED validators; the partitioned
        // group converges during recovery, not per-block.
        let sampled_count = (self.connected / (self.connected / 40).max(1)).min(self.connected);
        let want50 = (50 * sampled_count).div_ceil(100);
        let want95 = (95 * sampled_count).div_ceil(100);
        let want100 = sampled_count;
        let mut sample = PropagationSample::default();
        loop {
            // Attribution instrumentation (Step 0, #62): the full sweep over
            // `connected` nodes was measured growing per round and dominating
            // the thresholds (669 ms of 1751 ms at round 10, contending with
            // the 200-peer commit herd). A stride sample keeps the estimate
            // at O(sample) cost: the sweep is the instrument's overhead, not
            // part of the network being measured.
            let stride = (self.connected / 40).max(1);
            let sweep_start = Instant::now();
            let mut reached = 0;
            for idx in (0..self.connected).step_by(stride) {
                let node = &self.validators[idx];
                // Length read under the lock — ledger_snapshot would clone the
                // whole chain per poll, dominating the measurement.
                if node.shared_ledger().lock().await.chain.len() >= height {
                    reached += 1;
                }
            }
            sample.sweep_ms += sweep_start.elapsed().as_millis();
            sample.ticks += 1;
            let elapsed = start.elapsed().as_millis();
            if sample.first_reached.is_none() && reached >= 1 {
                sample.first_reached = Some(elapsed);
            }
            if reached >= want50 && sample.p50.is_none() {
                sample.p50 = Some(elapsed);
            }
            if reached >= want95 && sample.p95.is_none() {
                sample.p95 = Some(elapsed);
            }
            if reached >= want100 && sample.p100.is_none() {
                sample.p100 = Some(elapsed);
            }
            if sample.p100.is_some() || start.elapsed() > Duration::from_secs(90) {
                return sample;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Recover the partitioned validators: the unconnected dial the leader and
    /// converge to its chain. Returns (joined, convergence ms).
    async fn recover_partitioned(&mut self) -> (usize, Option<u128>) {
        let start = Instant::now();
        let mut joined = 0;
        for idx in self.connected..self.validators.len() {
            let node = &self.validators[idx];
            node.start(vec![self.leader_addr.clone()]).await.unwrap();
            joined += 1;
            if joined % 50 == 0 {
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
        self.connected = self.validators.len();
        let tip = self.leader.ledger_snapshot().await.chain.len();
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let mut caught_up = 0;
            for node in &self.validators {
                if node.ledger_snapshot().await.chain.len() >= tip {
                    caught_up += 1;
                }
            }
            if caught_up == self.validators.len() {
                return (joined, Some(start.elapsed().as_millis()));
            }
            if Instant::now() > deadline {
                return (joined, None);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

struct RoundMetrics {
    seq: usize,
    submit_ms: u128,
    mine_ms: u128,
    block_bytes: usize,
    tx_count: usize,
    attestation_set_bytes: usize,
    /// Pool depth after the round's submissions, before the mine — the
    /// backpressure the consensus layer absorbs.
    pool_depth_before_mine: usize,
    propagation_50: Option<u128>,
    propagation_95: Option<u128>,
    propagation_100: Option<u128>,
}

fn print_round(m: &RoundMetrics) {
    println!(
        "round {:>3}: submit {:>5} ms | mine {:>4} ms | block {:>6} B / {:>2} txs | attest. set {:>4} B | pool-before-mine {:>3} | fan-out 50% {:>4?} 95% {:>4?} 100% {:>4?} ms",
        m.seq, m.submit_ms, m.mine_ms, m.block_bytes, m.tx_count, m.attestation_set_bytes,
        m.pool_depth_before_mine, m.propagation_50, m.propagation_95, m.propagation_100,
    );
}

/// The `percent`th-percentile value (integer percent, ceil).
fn percentile<I: IntoIterator<Item = u128>>(values: I, percent: usize) -> u128 {
    let mut values: Vec<u128> = values.into_iter().collect();
    values.sort_unstable();
    let idx = (percent * values.len()).div_ceil(100).saturating_sub(1);
    values.get(idx).copied().unwrap_or(0)
}

fn print_summary(label: &str, rounds: &[RoundMetrics]) {
    let mine: Vec<u128> = rounds.iter().map(|r| r.mine_ms).collect();
    let sizes: Vec<usize> = rounds.iter().map(|r| r.block_bytes).collect();
    let attestation_sizes: Vec<usize> = rounds.iter().map(|r| r.attestation_set_bytes).collect();
    let pools: Vec<usize> = rounds.iter().map(|r| r.pool_depth_before_mine).collect();
    let p100 = rounds.iter().map(|r| r.propagation_100);
    println!(
        "SUMMARY[{label}]: rounds={} | mine p50={} p95={} ms | block bytes avg={} | attestation-set bytes avg={} | pool-before max={} | fan-out-100% median={:?} ms",
        rounds.len(),
        percentile(mine.clone(), 50),
        percentile(mine, 95),
        sizes.iter().sum::<usize>() / sizes.len().max(1),
        attestation_sizes.iter().sum::<usize>() / attestation_sizes.len().max(1),
        pools.iter().copied().max().unwrap_or(0),
        percentile(p100.into_iter().flatten(), 50),
    );
}

/// One sustained-load round: submit `txs_per_round` compact records, mine,
/// and report latency/size/pool/fan-out metrics.
async fn run_round(
    set: &ValidatorSet,
    events: &mut tokio::sync::mpsc::Receiver<NodeEvent>,
    seq: usize,
    txs_per_round: usize,
) -> RoundMetrics {
    let submit_start = Instant::now();
    for i in 0..txs_per_round {
        let record = match i % 3 {
            0 => lot_record(seq * 100 + i),
            1 => commitment_record(seq * 100 + i),
            _ => certification_record(seq * 100 + i),
        };
        set.leader
            .submit_transaction(signed(record, "org-maker"))
            .await
            .unwrap();
    }
    let submit_ms = submit_start.elapsed().as_millis();
    let pool_depth_before_mine = set
        .leader
        .ledger_snapshot()
        .await
        .pending_transactions
        .len();

    let mine_start = Instant::now();
    set.leader.mine().await.unwrap();
    let mine_ms = mine_start.elapsed().as_millis();

    // The staged engine's certificate: one local attestation per block (no
    // cross-validator vote rounds exist to measure — see the evidence doc).
    let attestation_set_bytes = loop {
        match tokio::time::timeout(Duration::from_secs(2), events.recv()).await {
            // The metric is the attestation SET (the vote-traffic proxy) —
            // serialized on its own, never the full certificate envelope.
            Ok(Some(NodeEvent::BlockMined {
                certificate: quorum,
                ..
            })) => {
                // The metric is the certificate itself (the vote-traffic
                // proxy) — constant-size under ADR-014 aggregation.
                break serde_json::to_vec(&quorum).map_or(0, |v| v.len());
            }
            Ok(Some(_)) => {}
            _ => break 0,
        }
    };

    let height = set.leader.ledger_snapshot().await.chain.len();
    let sample = set.propagation_ms(height).await;
    let (propagation_50, propagation_95, propagation_100) = (sample.p50, sample.p95, sample.p100);
    println!(
        "attribution: first-reached {:>4?} ms | sweep {} ms over {} ticks",
        sample.first_reached, sample.sweep_ms, sample.ticks
    );

    let last = set
        .leader
        .ledger_snapshot()
        .await
        .chain
        .last()
        .cloned()
        .unwrap();
    RoundMetrics {
        seq,
        submit_ms,
        mine_ms,
        block_bytes: serde_json::to_vec(&last).map_or(0, |v| v.len()),
        tx_count: last.transactions.len(),
        attestation_set_bytes,
        pool_depth_before_mine,
        propagation_50,
        propagation_95,
        propagation_100,
    }
}

/// The PDC dissemination phase: a member-only collection (every 10th
/// validator), one payload, and the time until every member holds it —
/// measured separately from the consensus rounds above.
async fn member_dissemination_phase(set: &ValidatorSet) {
    // Every participant holds the SAME collection config: membership is
    // network-wide state, and each member must see itself in the list.
    let mut member_orgs = vec!["leader".to_owned()];
    let mut member_positions = Vec::new();
    for (idx, _node) in set.validators.iter().enumerate() {
        if idx % 10 == 0 {
            member_orgs.push(format!("validator-{idx}"));
            member_positions.push(idx);
        }
    }
    let collection = || {
        Channel::new(ChannelConfig {
            name: COLLECTION.to_owned(),
            member_ids: member_orgs.clone(),
            description: "capacity-run collection".into(),
            endorsement_policy: None,
            retention_secs: 3600,
        })
    };
    set.leader.set_collections(vec![collection()]).await;
    for &idx in &member_positions {
        set.validators[idx]
            .set_collections(vec![collection()])
            .await;
    }

    let payload = b"capacity-run-private-payload".to_vec();
    let start = Instant::now();
    set.leader
        .submit_private_payload(COLLECTION, payload.clone())
        .await
        .unwrap();
    let commitment = glasschain_core::crypto::sha256(&payload);
    let deadline = Instant::now() + Duration::from_secs(30);
    let held = loop {
        let mut held = 0;
        for &idx in &member_positions {
            if set.validators[idx]
                .transient_payload(COLLECTION, &commitment)
                .await
                .is_some()
            {
                held += 1;
            }
        }
        if held == member_positions.len() || Instant::now() > deadline {
            break held;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    println!(
        "pdc dissemination: {}/{} members hold the payload in {:?} (leader held: {})",
        held,
        member_positions.len(),
        start.elapsed(),
        set.leader
            .transient_payload(COLLECTION, &commitment)
            .await
            .is_some()
    );
}

/// The capacity gate at `validator_count` validators: capability activation,
/// sustained compact workload, partition recovery, and separate PDC
/// dissemination.
async fn capacity_gate(validator_count: usize, txs_per_round: usize, rounds: usize) {
    println!(
        "=== capacity gate: {validator_count} validators, {txs_per_round} txs/round x {rounds} rounds ==="
    );
    let setup_start = Instant::now();
    // Application-layer partition: two thirds connected at first, the rest
    // join at recovery time.
    let connected = validator_count * 2 / 3;
    let mut set = build_star(validator_count, connected, 1).await;
    println!(
        "setup: {} validators created, {connected} connected in {:?}",
        set.validators.len(),
        setup_start.elapsed()
    );

    let (event_tx, mut events) = tokio::sync::mpsc::channel(64);
    let mut leader_events = set.leader.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = leader_events.recv().await {
            if event_tx.send(event).await.is_err() {
                break;
            }
        }
    });

    // The validators run dev/test PoW admission, so the leader stays on the
    // PoW path here: BFT-attested blocks are rejected at the peers'
    // `has_valid_pow` admission check (peer-path BFT admission is ADR-010
    // adoption-gate work — the certificate-size comparison is in the evidence
    // doc, measured leader-side).
    set.leader
        .submit_transaction(activation_tx(2))
        .await
        .unwrap();
    set.leader.mine().await.unwrap();
    let height = set.leader.ledger_snapshot().await.chain.len();
    set.propagation_ms(height)
        .await
        .p100()
        .expect("activation propagates to the connected validators");

    // ── Sustained compact workload ──────────────────────────────────────
    let mut all = Vec::new();
    for seq in 1..=rounds {
        let m = run_round(&set, &mut events, seq, txs_per_round).await;
        print_round(&m);
        all.push(m);
    }
    print_summary(&format!("{validator_count} validators"), &all);

    // ── Recovery after the application-layer partition ──────────────────
    let (joined, convergence) = set.recover_partitioned().await;
    println!("recovery: {joined} partitioned validators joined; convergence {convergence:?}");

    // ── Private-data dissemination (separate measurement) ───────────────
    member_dissemination_phase(&set).await;
}

/// The committed gate: 200 validators.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test(flavor = "multi_thread", worker_threads = 4))]
#[ignore = "capacity gate: minutes-long, run explicitly with --ignored --nocapture"]
async fn capacity_gate_200_validators() {
    capacity_gate(200, 20, 10).await;
}

/// The committed gate: 300 validators.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test(flavor = "multi_thread", worker_threads = 4))]
#[ignore = "capacity gate: minutes-long, run explicitly with --ignored --nocapture"]
async fn capacity_gate_300_validators() {
    capacity_gate(300, 20, 10).await;
}

/// A fast smoke check (not ignored): the harness works end-to-end at a small
/// validator count so the gate's plumbing cannot rot silently.
#[cfg_attr(madsim, madsim::test)]
#[cfg_attr(not(madsim), tokio::test(flavor = "multi_thread", worker_threads = 2))]
async fn capacity_harness_smoke() {
    let _ = env_logger::try_init();
    let mut set = build_star(6, 4, 1).await;
    set.leader
        .submit_transaction(activation_tx(2))
        .await
        .unwrap();
    set.leader.mine().await.unwrap();
    let (event_tx, mut events) = tokio::sync::mpsc::channel(64);
    let mut leader_events = set.leader.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = leader_events.recv().await {
            if event_tx.send(event).await.is_err() {
                break;
            }
        }
    });
    let m = run_round(&set, &mut events, 1, 6).await;
    assert_eq!(m.tx_count, 6, "the round commits the compact workload");
    assert!(
        m.attestation_set_bytes > 0,
        "every commit carries a certificate"
    );
    assert!(m.propagation_100.is_some(), "all validators converge");
    let (joined, convergence) = set.recover_partitioned().await;
    assert_eq!(joined, 2, "the partitioned validators join");
    assert!(convergence.is_some(), "recovery converges");
    member_dissemination_phase(&set).await;
}

#[cfg(feature = "bft")]
mod bft_finality_gate_section {
    use super::*;
    use crate::Node;
    #[cfg(feature = "bft")]
    use bls_signatures::{PrivateKey, Serialize as _};
    #[cfg(feature = "bft")]
    use glasschain_core::{BftConsensusProvider, ValidatorInfo};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    // ── BFT finality gate (Step 0's original goal — now unblocked) ──────────────

    /// Deterministic BLS keys for the validator set.
    fn bft_keys(count: usize) -> Vec<PrivateKey> {
        let mut seed = 0u8;
        (0..count)
            .map(|_| {
                seed += 1;
                PrivateKey::new([seed; 64])
            })
            .collect()
    }

    fn bft_validators(keys: &[PrivateKey]) -> Vec<ValidatorInfo> {
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

    async fn bft_poll_until(desc: &str, secs: u64, mut condition: impl AsyncFnMut() -> bool) {
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

    struct BftFinalityMetrics {
        seq: usize,
        /// Leader-side finality: `mine()` call duration (block build + vote
        /// round + local commit).
        leader_finality_ms: u128,
        /// Quorum size in the committed certificate.
        signers: usize,
        /// Time until every connected replica holds the block.
        replication_ms: Option<u128>,
        /// Leader-side per-phase split (Step 0): proposal / prevote / aggregate
        /// / precommit / aggregate, recorded by the round driver.
        phases: Option<glasschain_network::rounds::BftPhaseTimings>,
    }

    fn print_bft_round(m: &BftFinalityMetrics) {
        let phases = m.phases.unwrap_or_default();
        println!(
            "bft round {:>3}: leader finality {:>5} ms | signers {:>3} | replication {:>4?} ms | proposal {} ms, prevote {} ms, prevote-agg {} ms, precommit {} ms, precommit-agg {} ms",
            m.seq,
            m.leader_finality_ms,
            m.signers,
            m.replication_ms,
            phases.proposal_broadcast,
            phases.prevote,
            phases.prevote_aggregate,
            phases.precommit,
            phases.precommit_aggregate,
        );
    }

    fn print_bft_summary(label: &str, rounds: &[BftFinalityMetrics]) {
        let leader: Vec<u128> = rounds.iter().map(|r| r.leader_finality_ms).collect();
        let signers: Vec<usize> = rounds.iter().map(|r| r.signers).collect();
        let phase = |pick: fn(&glasschain_network::rounds::BftPhaseTimings) -> u128| {
            percentile(
                rounds.iter().filter_map(|r| r.phases.as_ref().map(pick)),
                50,
            )
        };
        println!(
            "BFT-SUMMARY[{label}]: rounds={} | finality p50={} p95={} p99={} ms | signers min={} | phases p50: proposal={} prevote={} prevote-agg={} precommit={} precommit-agg={} ms",
            rounds.len(),
            percentile(leader.clone(), 50),
            percentile(leader.clone(), 95),
            percentile(leader, 99),
            signers.iter().copied().min().unwrap_or(0),
            phase(|t| t.proposal_broadcast),
            phase(|t| t.prevote),
            phase(|t| t.prevote_aggregate),
            phase(|t| t.precommit),
            phase(|t| t.precommit_aggregate),
        );
    }

    /// The BFT finality gate: full-mesh validators over a shared static set
    /// (the on-chain registry's bootstrap fallback), `bft_consensus` activated,
    /// and the vote-round driver committing real multi-signer certificates.
    /// Measures leader-side finality latency (the Step 0 number) and replica
    /// replication lag.
    ///
    /// NOTE: run with a raised fd limit — the mesh holds ~2·n² sockets:
    /// `ulimit -n 65535 && cargo test ... --ignored --nocapture`.
    #[allow(clippy::too_many_lines)]
    async fn bft_finality_gate(validator_count: usize, txs_per_round: usize, rounds: usize) {
        let _ = env_logger::try_init();
        println!(
            "=== bft finality gate: {validator_count} validators, {txs_per_round} txs/round x {rounds} rounds ==="
        );
        eprintln!("env_logger initialized");
        let setup_start = Instant::now();
        let keys = bft_keys(validator_count);
        let validators = bft_validators(&keys);

        // Bind-first, dial-second: listeners bind immediately after their address
        // is reserved (no TOCTOU window for other dialers to steal the port);
        // the mesh dials through `connect_peer` afterwards, in waves.
        let mut nodes: Vec<Node> = Vec::with_capacity(validator_count);
        let mut addrs: Vec<String> = Vec::with_capacity(validator_count);
        for (i, key) in keys.iter().enumerate() {
            let addr = free_addr();
            let node = Node::new(format!("validator-{i}"), &addr, 1);
            let provider = BftConsensusProvider::new(validators.clone(), *key).expect("valid set");
            node.set_bft_consensus(Arc::new(provider)).await;
            // Execution provider for canonical-record evaluation.
            node.set_execution_provider(Arc::new(
                glasschain_vm::WasmExecutionProvider::new().unwrap(),
            ))
            .await;
            node.start(vec![]).await.expect("bind listener");
            nodes.push(node);
            addrs.push(addr);
        }

        // Full mesh in waves: proposals reach every validator directly (blocks
        // are broadcast, never re-relayed; votes answer point-to-point).
        let mut wave: usize = 0;
        for i in 1..addrs.len() {
            for dial in &addrs[..i] {
                nodes[i].connect_peer(dial);
                wave += 1;
            }
            if wave >= 500 {
                tokio::time::sleep(Duration::from_millis(400)).await;
                wave = 0;
            }
        }
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        println!(
            "mesh: {} validators fully connected in {:?}",
            validator_count,
            setup_start.elapsed()
        );

        // ── Block 1 (PoW): activate bft_consensus from height 2 ─────────────────
        nodes[0]
            .submit_transaction(activation_bft_tx(2))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        // Height 1's leader: 1 % n.
        let leader1 = 1 % validator_count;
        nodes[leader1].mine().await.unwrap();
        // Blocks diffuse through k-fanout relay waves (latency plan #1), so
        // a fixed sleep no longer guarantees every validator holds the
        // activation block: a validator still at genesis would mine a local
        // PoW fork instead of erroring as "not the round leader". Poll for
        // mesh-wide convergence before the first vote round.
        bft_poll_until(
            "activation block diffused to every validator",
            120,
            || async {
                for node in &nodes {
                    if node.ledger_snapshot().await.chain.len() < 2 {
                        return false;
                    }
                }
                true
            },
        )
        .await;
        // Height 2: the first vote round — its leader is 2 % n.
        nodes[leader1]
            .submit_transaction(canonical_record_tx(0))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut leader2 = 2 % validator_count;
        let start = Instant::now();
        for attempt in 0..4u32 {
            leader2 = (2 + attempt as usize) % validator_count;
            match nodes[leader2].mine().await {
                Ok(()) => break,
                Err(error) if error.to_string().contains("round leader") => {}
                Err(error) => panic!("first vote round failed: {error}"),
            }
        }
        println!(
            "first vote round (height 2, leader {leader2}): {:?}",
            start.elapsed()
        );

        // ── Sustained measured rounds ───────────────────────────────────────────
        let mut all = Vec::new();
        for seq in 1..=rounds {
            let height = seq + 2;
            let mut leader = height % validator_count;
            for i in 0..txs_per_round {
                nodes[leader]
                    .submit_transaction(canonical_record_tx((height * 100 + i) as u64))
                    .await
                    .unwrap();
            }
            // Wait for the previous block to replicate before driving the next
            // height: stale-tip nodes compute the wrong round leader.
            bft_poll_until(
                // Pure-Rust pairing throughput: every replica verifies the whole
                // certificate (a 201-term multi-miller loop at n=300) — the
                // replication lag IS the honest measurement (blst trigger).
                "replicas converged before the next round",
                600,
                || async {
                    for node in &nodes {
                        if node.ledger_snapshot().await.chain.len() < height {
                            return false;
                        }
                    }
                    true
                },
            )
            .await;

            // Drive the round; on a view change (the driver gave up and the
            // proposer rotated), retry with the next proposer.
            let mine_start = Instant::now();
            let mut leader_finality_ms = None;
            for attempt in 0..4u32 {
                leader = (height + attempt as usize) % validator_count;
                match nodes[leader].mine().await {
                    Ok(()) => {
                        leader_finality_ms = Some(mine_start.elapsed().as_millis());
                        break;
                    }
                    Err(error) if error.to_string().contains("round leader") => {}
                    Err(error) => panic!("round failed: {error}"),
                }
            }
            let leader_finality_ms =
                leader_finality_ms.expect("round did not commit within the view-change budget");

            let tip = nodes[leader].ledger_snapshot().await.chain.len();
            let cert = nodes[leader]
                .ledger_snapshot()
                .await
                .chain
                .last()
                .cloned()
                .unwrap()
                .certificate
                .expect("bft block carries a certificate");
            let signers = cert
                .signers_bitmap
                .iter()
                .map(|b| b.count_ones())
                .sum::<u32>() as usize;

            // Replication: time until every validator holds the committed block.
            let repl_start = Instant::now();
            bft_poll_until("replicas hold the block", 180, || async {
                for node in &nodes {
                    if node.ledger_snapshot().await.chain.len() < tip {
                        return false;
                    }
                }
                true
            })
            .await;
            let replication_ms = Some(repl_start.elapsed().as_millis());

            let m = BftFinalityMetrics {
                seq,
                leader_finality_ms,
                signers,
                replication_ms,
                phases: nodes[leader].last_round_phase_timings().await,
            };
            print_bft_round(&m);
            all.push(m);
        }
        print_bft_summary(&format!("{validator_count} validators (BFT)"), &all);
    }

    /// A `bft_consensus` capability activation.
    fn activation_bft_tx(height: u64) -> Transaction {
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

    /// A compact canonical record for the BFT workload (same shape as the `PoW`
    /// gate's compact set).
    fn canonical_record_tx(seq: u64) -> Transaction {
        #[allow(clippy::cast_possible_truncation)]
        let seq = seq as usize;
        signed(lot_record(seq), "org-maker")
    }

    #[cfg_attr(madsim, madsim::test)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "bft finality gate: run explicitly"]
    async fn bft_finality_gate_10_validators() {
        bft_finality_gate(10, 10, 10).await;
    }

    #[cfg_attr(madsim, madsim::test)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "bft finality gate: minutes-long, needs a raised fd limit (ulimit -n 65535); run explicitly"]
    async fn bft_finality_gate_100_validators() {
        bft_finality_gate(100, 10, 10).await;
    }

    #[cfg_attr(madsim, madsim::test)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "bft finality gate: heavier mesh (~80k sockets); raise the fd limit first"]
    async fn bft_finality_gate_200_validators() {
        bft_finality_gate(200, 10, 10).await;
    }

    #[cfg_attr(madsim, madsim::test)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "bft finality gate: heaviest mesh (~180k sockets); raise the fd limit first"]
    async fn bft_finality_gate_300_validators() {
        bft_finality_gate(300, 10, 10).await;
    }

    /// A lightweight workload transaction for the offered-load gate: no
    /// canonical/capability gate, so the saturation measurement isolates
    /// pool + round behavior, not record validation.
    fn load_tx(seq: u64) -> Transaction {
        Transaction::with_id(
            format!("load-{seq}"),
            TransactionKind::InventoryUpdate(glasschain_core::InventoryUpdate {
                product_id: "LOAD-SKU".into(),
                owner_id: "owner".into(),
                quantity_delta: 1,
                reason: "offered-load saturation".into(),
            }),
        )
    }

    async fn drive_saturation_round(
        nodes: &[Node],
        height: usize,
        burst: Option<(u64, usize)>,
    ) -> (
        u128,
        glasschain_network::PendingPoolStats,
        glasschain_network::PendingPoolStats,
        usize,
        usize,
    ) {
        #[allow(clippy::cast_possible_truncation)]
        let mut leader = height % nodes.len();
        let mut rejected = 0usize;
        if let Some((base, count)) = burst {
            for i in 0..count {
                if nodes[leader]
                    .submit_transaction(load_tx(base + u64::try_from(i).expect("burst fits")))
                    .await
                    .is_err()
                {
                    // Explicit backpressure (Step 6): the bounded pool
                    // rejected the submission — an operator-visible signal,
                    // not a silent queue drain.
                    rejected += 1;
                }
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let pool_before = nodes[leader].pending_pool_stats().await;
        let mine_start = Instant::now();
        let mut committed = false;
        for attempt in 0..4usize {
            leader = (height + attempt) % nodes.len();
            match nodes[leader].mine().await {
                Ok(()) => {
                    committed = true;
                    break;
                }
                Err(error) if error.to_string().contains("round leader") => {}
                Err(error) => panic!("round failed: {error}"),
            }
        }
        assert!(committed, "the round must commit");
        let pool_after = nodes[leader].pending_pool_stats().await;
        (
            mine_start.elapsed().as_millis(),
            pool_before,
            pool_after,
            leader,
            rejected,
        )
    }

    /// Step 6 offered-load saturation gate (100 validators, BFT): burst
    /// submissions between rounds while the leader keeps mining. Records —
    /// per the plan — pending count/bytes under offered load, backlog drain,
    /// and finality under load against the unloaded baseline. The failing
    /// budget this study looks for is pool depth/bytes growth and round
    /// latency, in that order.
    #[cfg_attr(madsim, madsim::test)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore = "offered-load saturation: minutes-long, needs a raised fd limit; run explicitly"]
    #[allow(clippy::too_many_lines)]
    async fn bft_offered_load_saturation_100_validators() {
        const VALIDATORS: usize = 100;
        const UNLOADED_ROUNDS: usize = 5;
        const LOADED_ROUNDS: usize = 8;
        // Burst size: 2 000 by default; the failing-budget probe overrides
        // it (e.g. `GLASSCHAIN_SATURATION_BURST=9000`) to push past the
        // 8 000-tx pool bound and measure the explicit rejections.
        let burst_size: usize = std::env::var("GLASSCHAIN_SATURATION_BURST")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(2_000);
        let _ = env_logger::try_init();
        println!(
            "=== bft offered-load saturation: {VALIDATORS} validators, burst {burst_size} txs/round ==="
        );
        let keys = bft_keys(VALIDATORS);
        let validators = bft_validators(&keys);
        let mut nodes: Vec<Node> = Vec::with_capacity(VALIDATORS);
        let mut addrs: Vec<String> = Vec::with_capacity(VALIDATORS);
        for (i, key) in keys.iter().enumerate() {
            let addr = free_addr();
            let node = Node::new(format!("validator-{i}"), &addr, 1);
            let provider = BftConsensusProvider::new(validators.clone(), *key).expect("valid set");
            node.set_bft_consensus(Arc::new(provider)).await;
            node.set_execution_provider(Arc::new(
                glasschain_vm::WasmExecutionProvider::new().unwrap(),
            ))
            .await;
            node.start(vec![]).await.expect("bind listener");
            nodes.push(node);
            addrs.push(addr);
        }
        let mut wave: usize = 0;
        for i in 1..addrs.len() {
            for dial in &addrs[..i] {
                nodes[i].connect_peer(dial);
                wave += 1;
            }
            if wave >= 500 {
                tokio::time::sleep(Duration::from_millis(400)).await;
                wave = 0;
            }
        }
        tokio::time::sleep(Duration::from_millis(1_500)).await;

        // Activation from height 2 (same bootstrap shape as the finality gate).
        nodes[0]
            .submit_transaction(activation_bft_tx(2))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let leader1 = 1 % VALIDATORS;
        nodes[leader1].mine().await.unwrap();
        bft_poll_until("activation diffused", 60, || async {
            for node in &nodes {
                if node.ledger_snapshot().await.chain.len() < 2 {
                    return false;
                }
            }
            true
        })
        .await;

        // Phase A — unloaded baseline.
        let mut unloaded = Vec::new();
        for seq in 1..=UNLOADED_ROUNDS {
            let height = seq + 1;
            bft_poll_until(
                "replicas converged before the unloaded round",
                120,
                || async {
                    for node in &nodes {
                        if node.ledger_snapshot().await.chain.len() < height {
                            return false;
                        }
                    }
                    true
                },
            )
            .await;
            let (finality, _, _, _, _) = drive_saturation_round(&nodes, height, None).await;
            unloaded.push(finality);
        }
        println!(
            "unloaded: rounds={} finality p50={} p99={} ms",
            unloaded.len(),
            percentile(unloaded.clone(), 50),
            percentile(unloaded.clone(), 99),
        );

        // Phase B — offered load: a burst of {BURST} txs per round.
        let mut loaded = Vec::new();
        let mut depth = Vec::new();
        let mut bytes = Vec::new();
        let mut drained = Vec::new();
        let mut seq: u64 = 0;
        for seq_round in 1..=LOADED_ROUNDS {
            let height = UNLOADED_ROUNDS + seq_round + 1;
            bft_poll_until(
                "replicas converged before the loaded round",
                120,
                || async {
                    for node in &nodes {
                        if node.ledger_snapshot().await.chain.len() < height {
                            return false;
                        }
                    }
                    true
                },
            )
            .await;
            seq += burst_size as u64;
            let (finality, before, after, _, rejected) =
                drive_saturation_round(&nodes, height, Some((seq, burst_size))).await;
            loaded.push(finality);
            depth.push(u128::try_from(before.count).expect("pool fits"));
            bytes.push(u128::try_from(before.bytes).expect("bytes fit"));
            drained.push(after.count);
            println!(
                "loaded round {seq_round}: finality {} ms | pool before {} txs / {} B | \
                 after {} txs | rejected {}",
                finality, before.count, before.bytes, after.count, rejected,
            );
        }
        println!(
            "SATURATION-SUMMARY: loaded finality p50={} p99={} ms | pool depth p50={} max={} |              pool bytes p50={} | drained-to-zero rounds={}/{}",
            percentile(loaded.clone(), 50),
            percentile(loaded.clone(), 99),
            percentile(depth.clone(), 50),
            depth.iter().copied().max().unwrap_or(0),
            percentile(bytes.clone(), 50),
            drained.iter().filter(|d| **d == 0).count(),
            LOADED_ROUNDS,
        );
        assert!(
            depth.iter().copied().max().unwrap_or(0)
                <= u128::try_from(glasschain_core::ledger::MAX_PENDING_TRANSACTIONS)
                    .expect("bound fits"),
            "the pool bound must hold under offered load"
        );
        if burst_size <= glasschain_core::ledger::MAX_BLOCK_TRANSACTIONS {
            assert!(
                drained.iter().all(|d| *d == 0),
                "bursts within the slice quota must drain fully each round"
            );
        } else {
            // Backpressure regime (Step 6 batching): the slice commits
            // MAX_BLOCK_TRANSACTIONS per round, the bound rejects the excess,
            // and the backlog persists — the pool bound is the designed
            // operator signal, not an emergency valve.
            assert!(
                drained.iter().all(|d| *d > 0),
                "bursts beyond the slice quota must leave a persistent backlog"
            );
        }
    }
}
