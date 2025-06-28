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

    let fake_wallet = FakeWallet::new(fee_reserve, HashMap::default(), HashSet::default(), 0);

    let mut mint_builder = MintBuilder::new();

    mint_builder = mint_builder
        .with_localstore(Arc::new(database))
        .with_keystore(Arc::new(key_store));

    mint_builder = mint_builder
        .add_ln_backend(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 300),
            Arc::new(fake_wallet),
        )
        .await?;

    mint_builder =
        mint_builder.set_clear_auth_settings(openid_discovery, "cashu-client".to_string());

    mint_builder = mint_builder.set_blind_auth_settings(50);

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

    let mut tx = auth_database.begin_transaction().await?;

    tx.add_protected_endpoints(blind_auth_endpoints).await?;

    let mut clear_auth_endpoint = HashMap::new();
    clear_auth_endpoint.insert(
        ProtectedEndpoint::new(Method::Post, RoutePath::MintBlindAuth),
        AuthRequired::Clear,
    );

    tx.add_protected_endpoints(clear_auth_endpoint).await?;

    tx.commit().await?;

    mint_builder = mint_builder.with_auth_localstore(Arc::new(auth_database));

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_description("fake test mint".to_string())
        .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let _mint = mint_builder.build().await?;

    todo!("Need to start this a cdk mintd keeping as ref for now");
}

pub async fn top_up_blind_auth_proofs(auth_wallet: Arc<AuthWallet>, count: u64) {
    let _proofs = auth_wallet
        .mint_blind_auth(count.into())
        .await
        .expect("could not mint blind auth");
}
