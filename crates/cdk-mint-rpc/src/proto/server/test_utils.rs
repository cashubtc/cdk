//! Shared mint setup for the management service tests.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bip39::Mnemonic;
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::types::QuoteTTL;
use cdk_common::nut00::KnownMethod;
use cdk_fake_wallet::FakeWallet;
use tokio::sync::Notify;

use super::{MintMutationGuard, MintMutationGuardError, MintRPCServer};

/// A well-formed quote id that no test mint has issued
pub(super) const UNKNOWN_QUOTE_ID: &str = "019820ab-cdef-7000-8000-000000000000";

pub(super) async fn create_test_rpc_server() -> MintRPCServer {
    create_test_rpc_server_with_payment_delay(2).await
}

/// Builds a test server whose fake payment backend waits `payment_delay`
/// seconds before reporting a quote paid
///
/// Tests that drive quote state themselves pass a delay long enough that
/// the backend never reports a payment of its own.
pub(super) async fn create_test_rpc_server_with_payment_delay(payment_delay: u64) -> MintRPCServer {
    let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());

    let mut mint_builder = MintBuilder::new(db.clone());

    let fee_reserve = cdk::types::FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_backend = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        payment_delay,
        CurrencyUnit::Sat,
    );

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Known(KnownMethod::Bolt11),
            MintMeltLimits::new(1, 10_000),
            Arc::new(fake_backend),
        )
        .await
        .unwrap();

    let mnemonic = Mnemonic::generate(12).unwrap();

    mint_builder = mint_builder
        .with_name("test mint".to_string())
        .with_description("test mint".to_string());

    let mint = mint_builder
        .build_with_seed(db.clone(), &mnemonic.to_seed_normalized(""))
        .await
        .unwrap();

    mint.set_quote_ttl(QuoteTTL::new(10000, 10000))
        .await
        .unwrap();

    mint.start().await.unwrap();

    MintRPCServer {
        socket_addr: "127.0.0.1:0".parse().unwrap(),
        mint: Arc::new(mint),
        mutation_guard: None,
        allow_mint_quote_payment_override: false,
        wallet_info_provider: None,
        shutdown: Arc::new(Notify::new()),
        handle: None,
    }
}

#[derive(Debug)]
pub(super) struct RejectingMutationGuard;

#[tonic::async_trait]
impl MintMutationGuard for RejectingMutationGuard {
    async fn check(&self) -> Result<(), MintMutationGuardError> {
        Err(MintMutationGuardError::FailedPrecondition(
            "configuration restart pending".to_owned(),
        ))
    }
}
