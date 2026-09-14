//! Quorum-certificate verification cost (performance plan Step 4, ADR-015).
//!
//! Three variants of the IETF `PopScheme` same-message multisig check at quorum
//! sizes 101 and 201 (the quorums of the 150- and 300-validator designed
//! operating points):
//!
//! - `pairing_per_signer_*/<n>` — the superseded pure-Rust path: one
//!   multi-Miller loop with `n + 1` pairing terms (`bls12_381`);
//! - `blst_per_signer_*/<n>` — the same O(n) shape on the `blst` backend;
//! - `blst_sum_of_keys_*/<n>` — the shipped check: bilinearity collapses the
//!   signer keys into one G1 sum, so the verification is two pairing terms.
//!
//! Regression gate: the shipped sum-of-keys path must stay below the
//! per-signer pairing cost at every measured quorum.
#![allow(clippy::print_stderr)]

use bls12_381::{
    multi_miller_loop, G1Affine as PureG1, G2Affine as PureG2, G2Prepared as PureG2Prepared,
    Gt as PureGt,
};
use bls_signatures::{PrivateKey, Serialize};
use blstrs::{G1Affine, G1Projective, G2Affine, G2Projective, Gt};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use group::prime::PrimeCurveAffine as _;
use group::Group as _;
use pairing::{MillerLoopResult as _, MultiMillerLoop as _};

const MESSAGE: &[u8] = b"glasschain-bench-block-hash-fixture";

/// 300 fixed keys — the designed operating point's validator set.
#[allow(clippy::cast_possible_truncation)]
fn key(i: u32) -> PrivateKey {
    let mut ikm = [0u8; 32];
    ikm[0] = i as u8;
    ikm[31] = i as u8;
    PrivateKey::new(ikm)
}

/// Signers `0..quorum` of a 300-key roster aggregate one message.
struct Certificate {
    aggregate: G2Affine,
    signer_pks: Vec<G1Affine>,
    hash: G2Affine,
}

fn build_certificate(quorum: usize) -> Certificate {
    let keys: Vec<PrivateKey> = (0..300u32).map(key).collect();
    let signers = &keys[..quorum];
    let signatures: Vec<bls_signatures::Signature> =
        signers.iter().map(|k| k.sign(MESSAGE)).collect();
    let aggregate = bls_signatures::aggregate(&signatures).expect("aggregate");
    let aggregate: G2Projective = aggregate.into();
    let signer_pks: Vec<G1Affine> = signers
        .iter()
        .map(|k| {
            let bytes: [u8; 48] = k.public_key().as_bytes()[..48]
                .try_into()
                .expect("48-byte key");
            Option::from(G1Affine::from_compressed(&bytes)).expect("valid key")
        })
        .collect();
    let hash: G2Projective = bls_signatures::hash(MESSAGE);
    Certificate {
        aggregate: G2Affine::from(aggregate),
        signer_pks,
        hash: G2Affine::from(hash),
    }
}

/// The superseded O(quorum)-term check on the pure-Rust backend.
fn pairing_per_signer(cert: &Certificate) -> bool {
    let aggregate_bytes: [u8; 96] = cert.aggregate.to_compressed();
    let aggregate =
        <Option<PureG2>>::from(PureG2::from_compressed(&aggregate_bytes)).expect("valid sig");
    let g1_neg = -PureG1::generator();
    let hash_prepared = PureG2Prepared::from(
        <Option<PureG2>>::from(PureG2::from_compressed(&cert.hash.to_compressed()))
            .expect("valid hash"),
    );
    let signature_prepared = PureG2Prepared::from(aggregate);
    let signer_pks: Vec<PureG1> = cert
        .signer_pks
        .iter()
        .map(|pk| {
            <Option<PureG1>>::from(PureG1::from_compressed(&pk.to_compressed())).expect("valid key")
        })
        .collect();
    let mut terms: Vec<(&PureG1, &PureG2Prepared)> = Vec::with_capacity(cert.signer_pks.len() + 1);
    terms.push((&g1_neg, &signature_prepared));
    for pk in &signer_pks {
        terms.push((pk, &hash_prepared));
    }
    multi_miller_loop(&terms).final_exponentiation() == PureGt::identity()
}

/// The same O(quorum)-term shape on the blst backend.
fn blst_per_signer(cert: &Certificate) -> bool {
    let g1_neg = -G1Affine::generator();
    let signature_prepared = blstrs::G2Prepared::from(cert.aggregate);
    let hash_prepared = blstrs::G2Prepared::from(cert.hash);
    let mut terms: Vec<(&G1Affine, &blstrs::G2Prepared)> =
        Vec::with_capacity(cert.signer_pks.len() + 1);
    terms.push((&g1_neg, &signature_prepared));
    for pk in &cert.signer_pks {
        terms.push((pk, &hash_prepared));
    }
    blstrs::Bls12::multi_miller_loop(&terms).final_exponentiation() == Gt::identity()
}

/// The shipped check: signer keys collapse into one G1 sum (PoP-validated at
/// registration), so the verification is two pairing terms.
fn blst_sum_of_keys(cert: &Certificate) -> bool {
    let sum: G1Projective = cert
        .signer_pks
        .iter()
        .map(|pk| G1Projective::from(*pk))
        .sum();
    let lhs = blstrs::pairing(&-G1Affine::generator(), &cert.aggregate);
    let rhs = blstrs::pairing(&G1Affine::from(sum), &cert.hash);
    lhs + rhs == Gt::identity()
}

fn bench_verify(c: &mut Criterion) {
    for quorum in [usize::from(u16::try_from(101).unwrap()), 201] {
        let group_name = format!("quorum_{quorum}");
        let cert = build_certificate(quorum);

        for (name, verify) in [
            (
                "pairing_per_signer",
                &pairing_per_signer as &dyn Fn(&Certificate) -> bool,
            ),
            ("blst_per_signer", &blst_per_signer),
            ("blst_sum_of_keys", &blst_sum_of_keys),
        ] {
            assert!(
                verify(&cert),
                "{name} must verify before benchmarking it (quorum {quorum})"
            );
            let group_id = name;
            c.benchmark_group(&group_name)
                .throughput(Throughput::Elements(quorum as u64))
                .bench_function(group_id, |b| b.iter(|| verify(&cert)));
        }
        drop(cert);
    }
    // Keep BatchSize referenced: per-iteration setup is not needed because the
    // certificate is built once per group.
    let _ = BatchSize::SmallInput;
}

criterion_group!(benches, bench_verify);
criterion_main!(benches);
