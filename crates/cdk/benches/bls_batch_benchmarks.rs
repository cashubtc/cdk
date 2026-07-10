#![allow(missing_docs)]
#![allow(clippy::unwrap_used)]

use std::hint::black_box;

use cdk::nuts::nut01::{
    batch_verify_blind_signature_shares, trusted_dealer_keygen, verify_blind_signature_share,
    BlsBlindSignatureShare, BlsG1PublicKey, BlsPublicKeyShare, BlsSecretKey, BlsSignerId,
};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

struct MemberBatch {
    shares: Vec<BlsBlindSignatureShare>,
    public_shares: Vec<BlsPublicKeyShare>,
}

fn member_batches(
    threshold: usize,
    members: usize,
    output_count: usize,
) -> (Vec<BlsG1PublicKey>, Vec<MemberBatch>) {
    let signer_ids = (1..=members)
        .map(|id| BlsSignerId::new(id as u16).expect("non-zero signer id"))
        .collect::<Vec<_>>();
    let keysets = (0..4)
        .map(|keyset_index| {
            let coefficients = (0..threshold)
                .map(|coefficient_index| {
                    let seed = keyset_index * threshold + coefficient_index + 1;
                    BlsSecretKey::from_reduced_bytes(&[seed as u8; 32])
                })
                .collect::<Vec<_>>();
            trusted_dealer_keygen(&coefficients, &signer_ids).expect("threshold keyset")
        })
        .collect::<Vec<_>>();
    let blinded_messages = (0..output_count)
        .map(|index| {
            BlsG1PublicKey::hash_to_curve(format!("BLS batch benchmark output {index}").as_bytes())
        })
        .collect::<Vec<_>>();
    let batches = (0..threshold)
        .map(|member_index| MemberBatch {
            shares: blinded_messages
                .iter()
                .enumerate()
                .map(|(output_index, message)| {
                    keysets[output_index % keysets.len()].secret_shares()[member_index]
                        .blind_sign_share(message)
                })
                .collect(),
            public_shares: (0..output_count)
                .map(|output_index| {
                    keysets[output_index % keysets.len()].public_shares()[member_index]
                })
                .collect(),
        })
        .collect();

    (blinded_messages, batches)
}

fn bench_bls_blind_share_verification(c: &mut Criterion) {
    for (federation, threshold, members) in [("3-of-5", 3, 5), ("5-of-7", 5, 7)] {
        let mut group = c.benchmark_group(format!("bls-blind-shares/{federation}"));
        for output_count in [4, 14, 64] {
            let (blinded_messages, batches) = member_batches(threshold, members, output_count);
            for batch in &batches {
                assert!(batch_verify_blind_signature_shares(
                    &batch.shares,
                    &blinded_messages,
                    &batch.public_shares,
                )
                .expect("valid batch fixture"));
                for ((share, message), public_share) in batch
                    .shares
                    .iter()
                    .zip(&blinded_messages)
                    .zip(&batch.public_shares)
                {
                    assert!(verify_blind_signature_share(share, message, public_share)
                        .expect("valid scalar fixture"));
                }
            }

            group.bench_with_input(
                BenchmarkId::new("scalar", output_count),
                &output_count,
                |b, _| {
                    b.iter(|| {
                        for batch in &batches {
                            for ((share, message), public_share) in batch
                                .shares
                                .iter()
                                .zip(&blinded_messages)
                                .zip(&batch.public_shares)
                            {
                                black_box(
                                    verify_blind_signature_share(share, message, public_share)
                                        .expect("scalar verification"),
                                );
                            }
                        }
                    });
                },
            );
            group.bench_with_input(
                BenchmarkId::new("batch", output_count),
                &output_count,
                |b, _| {
                    b.iter(|| {
                        for batch in &batches {
                            black_box(
                                batch_verify_blind_signature_shares(
                                    &batch.shares,
                                    &blinded_messages,
                                    &batch.public_shares,
                                )
                                .expect("batch verification"),
                            );
                        }
                    });
                },
            );
        }
        group.finish();
    }
}

criterion_group!(benches, bench_bls_blind_share_verification);
criterion_main!(benches);
