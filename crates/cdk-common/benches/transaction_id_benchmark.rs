use cashu::nuts::nut01::SecretKey;
use cashu::PublicKey;
use cdk_common::wallet::TransactionId;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

fn generate_public_keys(count: usize) -> Vec<PublicKey> {
    (0..count)
        .map(|_| SecretKey::generate().public_key())
        .collect()
}

fn bench_transaction_id(c: &mut Criterion) {
    let mut group = c.benchmark_group("TransactionId::new");

    let sizes = vec![1, 10, 50, 100, 500];

    for size in sizes {
        let public_keys = generate_public_keys(size);

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| TransactionId::new(public_keys.clone()));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_transaction_id);
criterion_main!(benches);
