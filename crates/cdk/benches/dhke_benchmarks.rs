#![allow(missing_docs)]
use cdk::dhke;
use cdk::nuts::nut01::{PublicKey, SecretKey};
use cdk::util::hex;
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_dhke(c: &mut Criterion) {
    // *************************************************************
    // * PREPARE DATA FOR BENCHMARKS                           *
    // *************************************************************
    let message =
        hex::decode("d341ee4871f1f889041e63cf0d3823c713eea6aff01e80f1719f08f9e5be98f6").unwrap();
    let alice_sec: SecretKey =
        SecretKey::from_hex("99fce58439fc37412ab3468b73db0569322588f62fb3a49182d67e23d877824a")
            .unwrap();

    let blinded_key =
        PublicKey::from_hex("02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2")
            .unwrap();

    let r = SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
        .unwrap();
    let a =
        PublicKey::from_hex("020000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
    let bob_sec =
        SecretKey::from_hex("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap();
    let (blinded_message, _) =
        dhke::blind_message("test_message".as_bytes(), Some(bob_sec.clone())).unwrap();

    // *************************************************************
    // * RUN INDIVIDUAL STEPS                                  *
    // *************************************************************
    c.bench_function("hash_to_curve", |b| {
        b.iter(|| {
            dhke::hash_to_curve(&message.clone()).unwrap();
        })
    });

    c.bench_function("blind_message", |b| {
        b.iter(|| {
            dhke::blind_message(&message, Some(alice_sec.clone())).unwrap();
        })
    });

    c.bench_function("unblind_message", |b| {
        b.iter(|| {
            dhke::unblind_message(&blinded_key, &r, &a).unwrap();
        })
    });

    c.bench_function("sign_message", |b| {
        b.iter(|| {
            dhke::sign_message(&bob_sec.clone(), &blinded_message).unwrap();
        })
    });

    // *************************************************************
    // * RUN END TO END BDHKE                                 *
    // *************************************************************
    c.bench_function("End-to-End BDHKE", |b| {
        b.iter(|| {
            let (b, r) = dhke::blind_message(&message, Some(alice_sec.clone())).unwrap();

            // C_
            let signed = dhke::sign_message(&bob_sec, &b).unwrap();

            let unblinded = dhke::unblind_message(&signed, &r, &bob_sec.public_key()).unwrap();

            assert!(dhke::verify_message(&bob_sec, unblinded, &message).is_ok());
        })
    });
}

criterion_group!(benches, bench_dhke);
criterion_main!(benches);
