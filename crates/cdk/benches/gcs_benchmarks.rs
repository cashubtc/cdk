use cdk::gcs::GCSFilter;
use criterion::{criterion_group, criterion_main, Criterion};
use rand::prelude::*;

fn bench_create_gcs_1_mill(c: &mut Criterion) {
    let num_items_arr = [100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000]; // 1 million entries
    let item_size = 33; // 33 bytes per entry
    let mut rng = rand::rng();

    for num_items in num_items_arr {
        let mut items = Vec::with_capacity(num_items);
        for _ in 0..num_items {
            let mut item = vec![0u8; item_size];
            rng.fill_bytes(&mut item);
            items.push(item);
        }

        let p = 19;
        let m = 784931;

        c.bench_function(
            format!("GCSFilter::create {} entries", num_items).as_str(),
            |b| {
                b.iter(|| {
                    let _ = GCSFilter::create(&items, p, m).unwrap();
                })
            },
        );
    }
}

criterion_group!(benches, bench_create_gcs_1_mill);
criterion_main!(benches);
