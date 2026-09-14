//! Wire codec cost at the current BLS shape (performance plan Step 1).
//!
//! Measures JSON encode/decode wall time and byte size for the `Message`
//! variants that dominate one consensus round at the 300-validator operating
//! point: `Block` (carrying a 200-signer sum-of-keys certificate),
//! `Proposal`, `Vote`, and `Precommit { block, prevote_certificate }`. The
//! sizes are printed once per build: the profile gate for any binary-encoding
//! decision — a swap is justified only when the recorded numbers say
//! encode/decode is a measured share of the round
//! (`docs/benchmarks/consensus-capacity.md`).
#![cfg(feature = "bft")]
#![allow(clippy::print_stderr)]

use bls_signatures::{PrivateKey, Serialize as _};
use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use glasschain_core::wire::SignatureAlgorithm;
use glasschain_core::{Block, QuorumCertificate, VotePhase};
use glasschain_network::protocol::Message;

const VALIDATORS: u64 = 300;
const QUORUM: u64 = 200;
const BLOCK_INDEX: u64 = 5741;
const BLOCK_HASH: &str = "000000000e6e21b476e427549a8ff0345c20112e1c5a1bcb12b02e28a0b3e62f";

#[allow(clippy::cast_possible_truncation)]
fn key(i: u64) -> PrivateKey {
    let mut ikm = [0u8; 32];
    ikm[..8].copy_from_slice(&i.to_be_bytes());
    ikm[8..16].copy_from_slice(&i.to_be_bytes());
    PrivateKey::new(ikm)
}

/// The round fixtures: a synthetic block, its 200-signer certificate from a
/// 300-key roster, one validator vote, and the pre-certificate twin the
/// Proposal carries. Shapes match the live wire; the aggregate is a genuine
/// BLS aggregate so the byte sizes are honest.
struct Fixtures {
    block: Message,
    vote: Message,
    proposal: Message,
    precommit: Message,
}

fn fixtures() -> Fixtures {
    let mut block = serde_json::from_str::<Block>(&format!(
        r#"{{
          "index": {BLOCK_INDEX},
          "timestamp": 1789000000,
          "transactions": [],
          "write_set": [],
          "previous_hash": "a7d81c40ecff4b7d30c7f74d04d2c02e5c01e31eeedbc9cd716f8b17ab6c9db3",
          "nonce": 812221,
          "hash": "{BLOCK_HASH}"
        }}"#
    ))
    .expect("fixture block");
    block.index = BLOCK_INDEX;
    let signatures: Vec<bls_signatures::Signature> = (0..QUORUM)
        .map(|i| key(i).sign(BLOCK_HASH.as_bytes()))
        .collect();
    let aggregate = bls_signatures::aggregate(&signatures).expect("aggregate");
    let mut bitmap = Vec::new();
    for i in 0..(VALIDATORS / 8) {
        bitmap.push(if i * 8 < QUORUM { u8::MAX } else { 0 });
    }
    let certificate = QuorumCertificate {
        block_index: BLOCK_INDEX,
        block_hash: BLOCK_HASH.to_owned(),
        signers_bitmap: bitmap,
        aggregate_signature: aggregate.as_bytes(),
        algorithm: SignatureAlgorithm::Bls12381,
    };
    let mut with_cert = block.clone();
    with_cert.certificate = Some(certificate.clone());
    let vote = Message::Vote(glasschain_core::BftVote::sign(
        "codec-profile",
        BLOCK_INDEX,
        0,
        VotePhase::Prevote,
        BLOCK_HASH,
        &key(1),
    ));
    let block_msg = Message::Block(with_cert.clone());
    let proposal = Message::Proposal {
        block: with_cert,
        round: 0,
    };
    let precommit = Message::Precommit {
        block,
        round: 0,
        prevote_certificate: certificate,
    };
    Fixtures {
        block: block_msg,
        vote,
        proposal,
        precommit,
    }
}

fn bench_wire_codec(c: &mut Criterion) {
    let fixtures = fixtures();
    let variants = [
        ("block", fixtures.block),
        ("vote", fixtures.vote),
        ("proposal", fixtures.proposal),
        ("precommit", fixtures.precommit),
    ];
    for (name, message) in variants {
        let encoded = serde_json::to_vec(&message).expect("serialize fixture");
        println!("wire size {name}: {} bytes", encoded.len());
        let mut group = c.benchmark_group(name);
        group.throughput(Throughput::Bytes(
            u64::try_from(encoded.len()).expect("wire sizes"),
        ));
        group.bench_function("encode", |b| {
            b.iter(|| serde_json::to_vec(&message).expect("encode"));
        });
        group.bench_function("decode", |b| {
            b.iter(|| serde_json::from_slice::<Message>(&encoded).expect("decode"));
        });
        group.finish();
    }
}

criterion_group!(benches, bench_wire_codec);
criterion_main!(benches);
