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
use bitcoin::bip32::DerivationPath;
use cashu::nut00::KnownMethod;
use cashu::util::unix_time;
use cashu::PaymentMethod;
use cdk::mint::{KeysetRotation, Mint, MintBuilder, MintMeltLimits};
use cdk::nuts::CurrencyUnit;
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
            PaymentMethod::Known(KnownMethod::Bolt11),
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

    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
        true,
        None,
    )
    .await
    .unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");

    let keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    assert_ne!(keyset_info.id, old_keyset_info.id);

    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
        true,
        None,
    )
    .await
    .unwrap();

    let active = mint.get_active_keysets();

    let active = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");

    let new_keyset_info = mint.get_keyset_info(active).expect("There is keyset");

    assert_ne!(new_keyset_info.id, keyset_info.id);
}

/// Test concurrent payment processing to verify race condition fix
///
/// This test simulates the real-world race condition where multiple concurrent
/// payment notifications arrive for the same payment_id. Before the fix, this
/// would cause "Payment ID already exists" errors. After the fix, all but one
/// should gracefully handle the duplicate and return a Duplicate error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_duplicate_payment_handling() {
    use cashu::PaymentMethod;
    use cdk::cdk_database::{MintDatabase, MintQuotesDatabase};
    use cdk::mint::MintQuote;
    use cdk::Amount;
    use cdk_common::payment::PaymentIdentifier;
    use tokio::task::JoinSet;

    // Create a test mint with in-memory database
    let mnemonic = Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = Arc::new(memory::empty().await.expect("valid db instance"));

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );

    let mut mint_builder = MintBuilder::new(database.clone());

    mint_builder = mint_builder
        .with_name("concurrent test mint".to_string())
        .with_description("testing concurrent payment handling".to_string());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();

    let mint = mint_builder
        .build_with_seed(database.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    // Create a mint quote
    let current_time = cdk::util::unix_time();
    let mint_quote = MintQuote::new(
        None,
        "concurrent_test_invoice".to_string(),
        CurrencyUnit::Sat,
        Some(Amount::from(1000).with_unit(CurrencyUnit::Sat)),
        current_time + 3600, // expires in 1 hour
        PaymentIdentifier::CustomId("test_lookup_id".to_string()),
        None,
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        Amount::ZERO.with_unit(CurrencyUnit::Sat),
        PaymentMethod::Known(KnownMethod::Bolt11),
        current_time,
        current_time,
        vec![],
        vec![],
        None, // extra_json
    );

    // Add the quote to the database
    {
        let mut tx = MintDatabase::begin_transaction(&*database).await.unwrap();
        tx.add_mint_quote(mint_quote.clone()).await.unwrap();
        tx.commit().await.unwrap();
    }

    // Simulate 10 concurrent payment notifications with the SAME payment_id
    let payment_id = "duplicate_payment_test_12345";
    let mut join_set = JoinSet::new();

    for i in 0..10 {
        let db_clone = database.clone();
        let quote_id = mint_quote.id.clone();
        let payment_id_clone = payment_id.to_string();

        join_set.spawn(async move {
            let mut tx = MintDatabase::begin_transaction(&*db_clone).await.unwrap();
            let mut quote_from_db = tx
                .get_mint_quote(&quote_id)
                .await
                .expect("no error")
                .expect("some value");

            let result = if let Err(err) = quote_from_db.add_payment(
                Amount::from(10).with_unit(CurrencyUnit::Sat),
                payment_id_clone,
                None,
            ) {
                Err(err)
            } else {
                tx.update_mint_quote(&mut quote_from_db)
                    .await
                    .map_err(cdk_common::Error::Database)
            };

            if result.is_ok() {
                tx.commit().await.unwrap();
            }

            (i, result)
        });
    }

    // Collect results
    let mut success_count = 0;
    let mut duplicate_errors = 0;
    let mut other_errors = Vec::new();

    while let Some(result) = join_set.join_next().await {
        let (task_id, db_result) = result.unwrap();
        match db_result {
            Ok(_) => success_count += 1,
            Err(e) => {
                let err_str = format!("{:?}", e);
                if err_str.contains("Duplicate") {
                    duplicate_errors += 1;
                } else {
                    other_errors.push((task_id, err_str));
                }
            }
        }
    }

    // Verify results
    assert_eq!(
        success_count, 1,
        "Exactly one task should successfully process the payment (got {})",
        success_count
    );
    assert!(
        other_errors.is_empty(),
        "No unexpected errors should occur. Got: {:?}",
        other_errors
    );
    assert_eq!(
        duplicate_errors, 9,
        "Nine tasks should receive Duplicate error (got {})",
        duplicate_errors
    );

    // Verify the quote was incremented exactly once
    let final_quote = MintQuotesDatabase::get_mint_quote(&*database, &mint_quote.id)
        .await
        .unwrap()
        .expect("Quote should exist");

    assert_eq!(
        final_quote.amount_paid(),
        Amount::from(10).with_unit(CurrencyUnit::Sat),
        "Quote amount should be incremented exactly once"
    );
    assert_eq!(
        final_quote.payments.len(),
        1,
        "Should have exactly one payment recorded"
    );
    assert_eq!(
        final_quote.payments[0].payment_id, payment_id,
        "Payment ID should match"
    );
}

/// Test that rotating a keyset with a final_expiry sets the expiry on the old keyset
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_rotate_keyset_with_expiry() {
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
        .with_name("expiry test mint".to_string())
        .with_description("testing keyset expiry".to_string());

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    let quote_ttl = QuoteTTL::new(10000, 10000);
    mint.set_quote_ttl(quote_ttl).await.unwrap();

    // Get the initial active keyset
    let active = mint.get_active_keysets();
    let active_id = active
        .get(&CurrencyUnit::Sat)
        .expect("There is a keyset for unit");
    let initial_keyset = mint.get_keyset_info(active_id).expect("There is keyset");
    assert!(initial_keyset.active);

    // Rotate with a past expiry timestamp
    let past_expiry = cdk::util::unix_time().saturating_sub(3600);
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
        true,
        Some(past_expiry),
    )
    .await
    .unwrap();

    // The old keyset should now be inactive
    let old_keyset = mint
        .get_keyset_info(&initial_keyset.id)
        .expect("Old keyset still exists");
    assert!(!old_keyset.active, "Old keyset should be inactive");

    // The new keyset should be active
    let active = mint.get_active_keysets();
    let new_active_id = active
        .get(&CurrencyUnit::Sat)
        .expect("There is an active keyset");
    assert_ne!(
        *new_active_id, initial_keyset.id,
        "New active keyset should differ from old"
    );

    // Rotate again without expiry
    mint.rotate_keyset(
        CurrencyUnit::Sat,
        cdk_integration_tests::standard_keyset_amounts(32),
        0,
        true,
        None,
    )
    .await
    .unwrap();

    // Should now have 3 keysets total
    let keysets = mint.keysets();
    assert!(
        keysets.keysets.len() >= 3,
        "Should have at least 3 keysets after two rotations, got {}",
        keysets.keysets.len()
    );

    // Exactly one should be active
    let active_count = keysets.keysets.iter().filter(|k| k.active).count();
    assert_eq!(active_count, 1, "Exactly one keyset should be active");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_builder_does_not_replace_all_expired_keysets() {
    use cdk_common::database::mint::KeysDatabase;

    let mnemonic = Mnemonic::generate(12).unwrap();
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let database = memory::empty().await.expect("valid db instance");
    let localstore = Arc::new(database);

    let fake_wallet1 = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );
    let mut builder1 = MintBuilder::new(localstore.clone());
    builder1 = builder1
        .with_name("test mint".to_string())
        .with_description("test mint".to_string());
    builder1
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet1),
        )
        .await
        .unwrap();
    let mint = builder1
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();
    mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
        .await
        .unwrap();

    assert_eq!(mint.keysets().keysets.len(), 1);

    let future_expiry = unix_time() + 1_000_000;
    let new_keyset = mint
        .rotate_keyset(
            CurrencyUnit::Sat,
            cdk_integration_tests::standard_keyset_amounts(32),
            0,
            true,
            Some(future_expiry),
        )
        .await
        .unwrap();

    let count_before_rebuild = mint.keysets().keysets.len();
    assert_eq!(count_before_rebuild, 2);

    // Mutate final_expiry to be in the past via localstore upsert.
    drop(mint);
    {
        let mut tx = KeysDatabase::begin_transaction(&*localstore).await.unwrap();
        let mut info = tx
            .get_keyset_infos()
            .await
            .unwrap()
            .into_iter()
            .find(|k| k.id == new_keyset.id)
            .expect("rotated keyset should be present in localstore");
        info.final_expiry = Some(unix_time().saturating_sub(1));
        tx.add_keyset_info(info).await.unwrap();
        tx.commit().await.unwrap();
    }

    let fake_wallet2 = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );
    let mut builder2 = MintBuilder::new(localstore.clone());
    builder2 = builder2
        .with_name("test mint".to_string())
        .with_description("test mint".to_string());
    builder2
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet2),
        )
        .await
        .unwrap();
    let mint2 = builder2
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    assert_eq!(mint2.keysets().keysets.len(), count_before_rebuild);
}

/// Build a mint over `localstore`, with a fake wallet for `unit` at `fee` and
/// `unit` optionally pinned to a fixed derivation path.
async fn build_mint(
    localstore: Arc<cdk_sqlite::mint::MintSqliteDatabase>,
    seed: &[u8],
    unit: CurrencyUnit,
    fee: u64,
    custom_path: Option<DerivationPath>,
    rotations: Vec<KeysetRotation>,
) -> Result<Mint, cdk::Error> {
    let fake_wallet = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        unit.clone(),
    );

    let mut builder = MintBuilder::new(localstore.clone());
    builder
        .add_payment_processor(
            unit.clone(),
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 5_000),
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();
    builder.set_unit_fee(&unit, fee).unwrap();

    if let Some(path) = custom_path {
        builder = builder.with_custom_derivation_paths(HashMap::from([(unit, path)]));
    }
    for rotation in rotations {
        builder = builder.with_keyset_rotation(rotation);
    }

    builder.build_with_seed(localstore, seed).await
}

fn custom_path() -> DerivationPath {
    "m/129372'/0'/7'".parse().expect("derivation path")
}

async fn keyset_count(localstore: &Arc<cdk_sqlite::mint::MintSqliteDatabase>) -> usize {
    use cdk_common::database::mint::KeysDatabase;

    let mut tx = KeysDatabase::begin_transaction(localstore.as_ref())
        .await
        .expect("keys transaction");
    let count = tx.get_keyset_infos().await.expect("keyset infos").len();
    tx.commit().await.expect("commit");
    count
}

async fn keyset_derivation_paths(
    localstore: &Arc<cdk_sqlite::mint::MintSqliteDatabase>,
) -> Vec<DerivationPath> {
    use cdk_common::database::mint::KeysDatabase;

    let mut tx = KeysDatabase::begin_transaction(localstore.as_ref())
        .await
        .expect("keys transaction");
    let paths = tx
        .get_keyset_infos()
        .await
        .expect("keyset infos")
        .into_iter()
        .map(|info| info.derivation_path)
        .collect();
    tx.commit().await.expect("commit");
    paths
}

/// Adding a custom derivation path to a unit that already has index-derived
/// keysets is a migration, not a re-derivation: the path has never produced keys,
/// so the rotation that applies the new fee lands on it. Only once it has a
/// keyset does it stop being able to produce new ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_builder_moves_a_unit_onto_an_unused_custom_derivation_path() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let seed = mnemonic.to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.expect("valid db instance"));

    let mint = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        0,
        None,
        Vec::new(),
    )
    .await
    .expect("the first build creates an index-derived keyset");
    drop(mint);

    assert_eq!(keyset_count(&localstore).await, 1);
    assert!(
        !keyset_derivation_paths(&localstore)
            .await
            .contains(&custom_path()),
        "the custom path has not been used yet"
    );

    let mint2 = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        100,
        Some(custom_path()),
        Vec::new(),
    )
    .await
    .expect("no keyset uses the custom path, so the fee change can be applied");

    let active = mint2
        .keysets()
        .keysets
        .into_iter()
        .find(|keyset| keyset.active && keyset.unit == CurrencyUnit::Sat)
        .expect("an active sat keyset");
    assert_eq!(active.input_fee_ppk, 100);
    drop(mint2);

    assert_eq!(keyset_count(&localstore).await, 2);
    assert!(
        keyset_derivation_paths(&localstore)
            .await
            .contains(&custom_path()),
        "the new keyset was derived from the custom path"
    );

    build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        200,
        Some(custom_path()),
        Vec::new(),
    )
    .await
    .expect_err("the custom path now has a keyset, so it cannot produce new keys");

    assert_eq!(
        keyset_count(&localstore).await,
        2,
        "the refused build wrote nothing"
    );
}

/// A unit pinned to a fixed derivation path keeps the keys derived from it, so
/// a fee change can only be applied by a rotation that cannot happen. The build
/// is refused rather than run half way.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_builder_refuses_fee_change_on_a_custom_derivation_path() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let seed = mnemonic.to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.expect("valid db instance"));

    let mint = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        0,
        Some(custom_path()),
        Vec::new(),
    )
    .await
    .expect("the first build creates the pinned keyset");
    drop(mint);

    let count_before = keyset_count(&localstore).await;
    assert_eq!(count_before, 1);

    let err = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        100,
        Some(custom_path()),
        Vec::new(),
    )
    .await
    .expect_err("a fee change on a pinned unit cannot be applied");

    let message = err.to_string();
    assert!(
        message.contains("sat") && message.contains("input fee"),
        "the error must name the unit and what changed: {message}"
    );
    assert_eq!(
        keyset_count(&localstore).await,
        count_before,
        "the build is refused before anything is written"
    );
}

/// The refusal is limited to what cannot be applied: an unchanged pinned unit
/// rebuilds and keeps its keyset.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_builder_rebuilds_an_unchanged_custom_derivation_path() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let seed = mnemonic.to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.expect("valid db instance"));

    let mint = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        0,
        Some(custom_path()),
        Vec::new(),
    )
    .await
    .expect("the first build creates the pinned keyset");
    let active = mint.keysets().keysets;
    drop(mint);

    let mint2 = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        0,
        Some(custom_path()),
        Vec::new(),
    )
    .await
    .expect("nothing changed, so nothing has to rotate");

    assert_eq!(mint2.keysets().keysets, active);
    assert_eq!(keyset_count(&localstore).await, 1);
}

/// The check runs over every planned rotation before the first one is written,
/// so a unit that could have rotated does not rotate when another cannot.
///
/// The pinned unit is `usd`, which sorts after `sat`: a build that rotated as it
/// planned would already have rotated `sat` by the time it reached the unit it
/// has to refuse.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_builder_rotates_nothing_when_one_unit_cannot_rotate() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let seed = mnemonic.to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.expect("valid db instance"));

    let fake_wallet = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Sat,
    );
    let fake_wallet_usd = FakeWallet::new(
        FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 1.0,
        },
        HashMap::default(),
        HashSet::default(),
        0,
        CurrencyUnit::Usd,
    );

    let two_units = |fee: u64| {
        let sat = fake_wallet.clone();
        let usd = fake_wallet_usd.clone();
        let localstore = localstore.clone();
        async move {
            let mut builder = MintBuilder::new(localstore.clone());
            builder
                .add_payment_processor(
                    CurrencyUnit::Sat,
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    MintMeltLimits::new(1, 5_000),
                    Arc::new(sat),
                )
                .await
                .unwrap();
            builder
                .add_payment_processor(
                    CurrencyUnit::Usd,
                    PaymentMethod::Known(KnownMethod::Bolt11),
                    MintMeltLimits::new(1, 5_000),
                    Arc::new(usd),
                )
                .await
                .unwrap();
            builder.set_unit_fee(&CurrencyUnit::Sat, fee).unwrap();
            builder.set_unit_fee(&CurrencyUnit::Usd, fee).unwrap();
            builder
                .with_custom_derivation_paths(HashMap::from([(CurrencyUnit::Usd, custom_path())]))
        }
    };

    let mint = two_units(0)
        .await
        .build_with_seed(localstore.clone(), &seed)
        .await
        .expect("the first build creates both keysets");
    drop(mint);

    let count_before = keyset_count(&localstore).await;
    assert_eq!(count_before, 2);

    two_units(100)
        .await
        .build_with_seed(localstore.clone(), &seed)
        .await
        .expect_err("the pinned unit cannot take the new fee");

    assert_eq!(
        keyset_count(&localstore).await,
        count_before,
        "the unpinned unit must not rotate when the build is refused"
    );
}

/// A configured rotation whose keyset already exists has nothing left to do, so
/// restarts stop issuing another keyset for the same entry. The expiry offset
/// differs between the two builds, since a real config recomputes it on boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn test_builder_applies_a_configured_rotation_once() {
    let mnemonic = Mnemonic::generate(12).unwrap();
    let seed = mnemonic.to_seed_normalized("");
    let localstore = Arc::new(memory::empty().await.expect("valid db instance"));

    let amounts = cdk_integration_tests::standard_keyset_amounts(32);
    let rotations = |expiry_offset: u64| {
        vec![
            KeysetRotation {
                unit: CurrencyUnit::Sat,
                amounts: amounts.clone(),
                input_fee_ppk: 0,
                use_keyset_v2: false,
                final_expiry: Some(unix_time().saturating_sub(expiry_offset)),
            },
            KeysetRotation {
                unit: CurrencyUnit::Sat,
                amounts: amounts.clone(),
                input_fee_ppk: 0,
                use_keyset_v2: true,
                final_expiry: None,
            },
        ]
    };

    let mint = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        0,
        None,
        rotations(3600),
    )
    .await
    .expect("the first build applies both rotations");
    assert_active_keyset_is_unexpired(&mint);
    drop(mint);

    let count_before = keyset_count(&localstore).await;

    let mint2 = build_mint(
        localstore.clone(),
        &seed,
        CurrencyUnit::Sat,
        0,
        None,
        rotations(1800),
    )
    .await
    .expect("the second build finds both rotations already applied");

    assert_eq!(
        keyset_count(&localstore).await,
        count_before,
        "a restart must not issue another keyset per configured rotation"
    );
    assert_active_keyset_is_unexpired(&mint2);
}

/// The configured rotations end on an unexpired keyset, so that is what the
/// unit must be left active on. The expired entry comes first and rotating is
/// what makes a keyset active, so a build that stopped early would leave the
/// mint active on an expired keyset.
fn assert_active_keyset_is_unexpired(mint: &Mint) {
    let active: Vec<_> = mint
        .keysets()
        .keysets
        .into_iter()
        .filter(|keyset| keyset.active && keyset.unit == CurrencyUnit::Sat)
        .collect();

    assert_eq!(active.len(), 1, "sat must have exactly one active keyset");
    assert_eq!(
        active[0].final_expiry, None,
        "the active keyset must be the one the configured rotations end on"
    );
}
