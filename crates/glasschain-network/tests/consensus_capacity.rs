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
use glasschain_identity::{Channel, ChannelConfig};
use glasschain_network::{Node, NodeEvent};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const COLLECTION: &str = "pricing";

fn free_addr() -> String {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

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
async fn build_star(validator_count: usize, connected: usize, difficulty: usize) -> ValidatorSet {
    let leader_addr = free_addr();
    let leader = Arc::new(Node::new("leader", &leader_addr, difficulty));
    leader.start(vec![]).await.unwrap();

    let mut validators = Vec::with_capacity(validator_count);
    let mut wave: Vec<Arc<Node>> = Vec::new();
    for idx in 0..validator_count {
        let node = Arc::new(Node::new(
            format!("validator-{idx}"),
            free_addr(),
            difficulty,
        ));
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
    async fn propagation_ms(&self, height: usize) -> (Option<u128>, Option<u128>, Option<u128>) {
        let start = Instant::now();
        // Fan-out is measured over the CONNECTED validators; the partitioned
        // group converges during recovery, not per-block.
        let want50 = (50 * self.connected).div_ceil(100);
        let want95 = (95 * self.connected).div_ceil(100);
        let want100 = self.connected;
        let mut reached_at: (Option<u128>, Option<u128>, Option<u128>) = (None, None, None);
        loop {
            let mut reached = 0;
            for node in &self.validators[..self.connected] {
                // Length read under the lock — ledger_snapshot would clone the
                // whole chain per poll, dominating the measurement.
                if node.shared_ledger().lock().await.chain.len() >= height {
                    reached += 1;
                }
            }
            let elapsed = start.elapsed().as_millis();
            if reached >= want50 && reached_at.0.is_none() {
                reached_at.0 = Some(elapsed);
            }
            if reached >= want95 && reached_at.1.is_none() {
                reached_at.1 = Some(elapsed);
            }
            if reached >= want100 && reached_at.2.is_none() {
                reached_at.2 = Some(elapsed);
            }
            if reached_at.2.is_some() || start.elapsed() > Duration::from_secs(90) {
                return reached_at;
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
                break serde_json::to_vec(&quorum.attestations).map_or(0, |v| v.len());
            }
            Ok(Some(_)) => {}
            _ => break 0,
        }
    };

    let height = set.leader.ledger_snapshot().await.chain.len();
    let (propagation_50, propagation_95, propagation_100) = set.propagation_ms(height).await;

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
        .2
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
