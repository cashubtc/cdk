use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cashu::{AuthRequired, Method, ProtectedEndpoint, RoutePath};
use cdk::cdk_database::{self, MintAuthDatabase, MintDatabase, MintKeysDatabase};
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::types::FeeReserve;
use cdk::wallet::AuthWallet;
use cdk_fake_wallet::FakeWallet;

pub async fn start_fake_mint_with_auth<D, A, K>(
    _addr: &str,
    _port: u16,
    openid_discovery: String,
    database: D,
    auth_database: A,
    key_store: K,
) -> Result<()>
where
    D: MintDatabase<cdk_database::Error> + Send + Sync + 'static,
    A: MintAuthDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
    K: MintKeysDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
{
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_wallet = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );

    let mut mint_builder = MintBuilder::new(Arc::new(database));

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 300),
            Arc::new(fake_wallet),
        )
        .await?;

    let auth_database = Arc::new(auth_database);

    mint_builder = mint_builder.with_auth(
        auth_database.clone(),
        openid_discovery,
        "cashu-client".to_string(),
        vec![],
    );

    let blind_auth_endpoints = vec![
        ProtectedEndpoint::new(Method::Post, RoutePath::MintQuoteBolt11),
        ProtectedEndpoint::new(Method::Post, RoutePath::MintBolt11),
        ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11),
        ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteBolt11),
        ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuoteBolt11),
        ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt11),
        ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
        ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate),
        ProtectedEndpoint::new(Method::Post, RoutePath::Restore),
    ];

    let blind_auth_endpoints =
        blind_auth_endpoints
            .into_iter()
            .fold(HashMap::new(), |mut acc, e| {
                acc.insert(e, AuthRequired::Blind);
                acc
            });

    mint_builder = mint_builder.with_blind_auth(50, blind_auth_endpoints.keys().cloned().collect());

    let mut tx = auth_database.begin_transaction().await?;

    tx.add_protected_endpoints(blind_auth_endpoints).await?;

    let mut clear_auth_endpoint = HashMap::new();
    clear_auth_endpoint.insert(
        ProtectedEndpoint::new(Method::Post, RoutePath::MintBlindAuth),
        AuthRequired::Clear,
    );

    tx.add_protected_endpoints(clear_auth_endpoint).await?;

    tx.commit().await?;

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder.with_description("fake test mint".to_string());

    let _mint = mint_builder
        .build_with_seed(Arc::new(key_store), &mnemonic.to_seed_normalized(""))
        .await?;

    todo!("Need to start this a cdk mintd keeping as ref for now");
}

pub async fn top_up_blind_auth_proofs(auth_wallet: Arc<AuthWallet>, count: u64) {
    let _proofs = auth_wallet
        .mint_blind_auth(count.into())
        .await
        .expect("could not mint blind auth");
}
