//! Mint Tests
//!
//! This file contains tests that focus on the mint's internal functionality without client interaction.
//! These tests verify the mint's behavior in isolation, such as keyset management, database operations,
//! and other mint-specific functionality that doesn't require wallet clients.
//!
//! Test Categories:
//! - Keyset rotation and management
//! - Database transaction handling
//! - Internal state transitions
//! - Fee calculation and enforcement
//! - Proof validation and state management

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk_fake_wallet::FakeWallet;
use cdk_sqlite::mint::memory;

pub const MINT_URL: &str = "http://127.0.0.1:8088";

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_correct_keyset() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = memory::empty().await.expect("valid db instance");

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );

    let localstore = Arc::new(database);
    let mut mint_builder = MintBuilder::new(localstore.clone());

    mint_builder = mint_builder
        .with_name("regtest mint".to_string())
        .with_description("regtest mint".to_string());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();
    // .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");
    let old_keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    mint.rotate_keyset(CurrencyUnit::Sat, 32, 0).await.unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");

    let keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    assert_ne!(keyset_info.id, old_keyset_info.id);

    mint.rotate_keyset(CurrencyUnit::Sat, 32, 0).await.unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");

    let new_keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    assert_ne!(new_keyset_info.id, keyset_info.id);
}
