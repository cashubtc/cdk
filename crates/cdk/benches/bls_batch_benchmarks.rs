#![allow(missing_docs)]
#![allow(clippy::unwrap_used)]

use std::hint::black_box;

use cdk::dhke::{batch_verify_bls_messages, verify_bls_message};
use cdk::nuts::nut01::{
    batch_verify_blind_signature_shares, trusted_dealer_keygen, verify_blind_signature_share,
    BlsBlindSignatureShare, BlsG1PublicKey, BlsPublicKeyShare, BlsSecretKey, BlsSignerId,
};
use cdk::nuts::{PublicKey, SecretKey};
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

struct ProofBatch {
    mint_pubkeys: Vec<PublicKey>,
    signatures: Vec<PublicKey>,
    messages: Vec<Vec<u8>>,
}

fn proof_batch(proof_count: usize) -> ProofBatch {
    let mint_secrets = (1..=4)
        .map(|seed| SecretKey::bls_from_reduced_bytes(&[seed; 32]))
        .collect::<Vec<_>>();
    let messages = (0..proof_count)
        .map(|index| format!("BLS proof batch benchmark secret {index}").into_bytes())
        .collect::<Vec<_>>();
    let mut mint_pubkeys = Vec::with_capacity(proof_count);
    let mut signatures = Vec::with_capacity(proof_count);
    for (index, message) in messages.iter().enumerate() {
        let mint_secret = &mint_secrets[index % mint_secrets.len()];
        mint_pubkeys.push(mint_secret.public_key());
        signatures.push(PublicKey::from(
            BlsG1PublicKey::hash_to_curve(message)
                .mul(mint_secret.as_bls().expect("BLS mint secret")),
        ));
    }

    ProofBatch {
        mint_pubkeys,
        signatures,
        messages,
    }
}

fn bench_bls_proof_verification(c: &mut Criterion) {
    let mut group = c.benchmark_group("bls-proof-messages");
    for proof_count in [1, 4, 8, 14, 64] {
        let batch = proof_batch(proof_count);
        let message_refs = batch.messages.iter().map(Vec::as_slice).collect::<Vec<_>>();
        for ((mint_pubkey, signature), message) in batch
            .mint_pubkeys
            .iter()
            .zip(&batch.signatures)
            .zip(&message_refs)
        {
            verify_bls_message(*mint_pubkey, *signature, message)
                .expect("valid scalar proof fixture");
        }
        batch_verify_bls_messages(&batch.mint_pubkeys, &batch.signatures, &message_refs)
            .expect("valid batch proof fixture");

        group.bench_with_input(
            BenchmarkId::new("scalar", proof_count),
            &proof_count,
            |b, _| {
                b.iter(|| {
                    for ((mint_pubkey, signature), message) in batch
                        .mint_pubkeys
                        .iter()
                        .zip(&batch.signatures)
                        .zip(&message_refs)
                    {
                        black_box(
                            verify_bls_message(*mint_pubkey, *signature, message)
                                .expect("scalar proof verification"),
                        );
                    }
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("batch", proof_count),
            &proof_count,
            |b, _| {
                b.iter(|| {
                    black_box(
                        batch_verify_bls_messages(
                            &batch.mint_pubkeys,
                            &batch.signatures,
                            &message_refs,
                        )
                        .expect("batch proof verification"),
                    );
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_bls_blind_share_verification,
    bench_bls_proof_verification
);
criterion_main!(benches);
