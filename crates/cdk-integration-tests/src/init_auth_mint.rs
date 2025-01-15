use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use bip39::Mnemonic;
use cdk::cdk_database::{self, MintAuthDatabase, MintDatabase};
use cdk::mint::{FeeReserve, MintBuilder, MintMeltLimits};
use cdk::nuts::{CurrencyUnit, Method, PaymentMethod, ProtectedEndpoint, RoutePath};
use cdk::wallet::Wallet;
use cdk::OidcClient;
use cdk_fake_wallet::FakeWallet;

use crate::init_mint::start_mint;

pub async fn start_fake_mint_with_auth<D, A>(
    addr: &str,
    port: u16,
    openid_discovery: String,
    database: D,
    auth_database: A,
) -> Result<()>
where
    D: MintDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
    A: MintAuthDatabase<Err = cdk_database::Error> + Send + Sync + 'static,
{
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_wallet = FakeWallet::new(fee_reserve, HashMap::default(), HashSet::default(), 0);

    let mut mint_builder = MintBuilder::new();

    mint_builder = mint_builder.with_localstore(Arc::new(database));

    mint_builder = mint_builder.add_ln_backend(
        CurrencyUnit::Sat,
        PaymentMethod::Bolt11,
        MintMeltLimits::new(1, 300),
        Arc::new(fake_wallet),
    );

    mint_builder = mint_builder.with_auth_localstore(Arc::new(auth_database));

    mint_builder = mint_builder.set_clear_auth_settings(
        openid_discovery,
        "cashu-client".to_string(),
        vec![ProtectedEndpoint::new(
            Method::Post,
            RoutePath::MintBlindAuth,
        )],
    );

    mint_builder = mint_builder.set_blind_auth_settings(
        50,
        vec![
            ProtectedEndpoint::new(Method::Post, RoutePath::MintQuoteBolt11),
            ProtectedEndpoint::new(Method::Post, RoutePath::MintBolt11),
            ProtectedEndpoint::new(Method::Get, RoutePath::MintQuoteBolt11),
            ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteBolt11),
            ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuoteBolt11),
            ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt11),
            ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate),
            ProtectedEndpoint::new(Method::Post, RoutePath::Restore),
        ],
    );

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("fake test mint".to_string())
        .with_mint_url(format!("http://{addr}:{port}"))
        .with_description("fake test mint".to_string())
        .with_quote_ttl(10000, 10000)
        .with_seed(mnemonic.to_seed_normalized("").to_vec());

    let mint = mint_builder.build().await?;

    start_mint(addr, port, mint).await?;

    Ok(())
}

pub async fn top_up_blind_auth_proofs(
    wallet: Arc<Wallet>,
    count: u64,
    oidc_user: &str,
    oidc_pass: &str,
) {
    let mint_info = wallet
        .get_mint_info()
        .await
        .expect("Mint info not found")
        .expect("Mint info not found");

    let openid_discovery = mint_info
        .nuts
        .nutxx
        .expect("Nutxx defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery);

    let access_token = oidc_client
        .get_access_token_with_user_password(oidc_user.to_string(), oidc_pass.to_string())
        .await
        .expect("Could not get cat");

    {
        let mut cat = wallet.cat.write().await;

        *cat = Some(access_token);
    }

    let _proofs = wallet
        .mint_blind_auth(count.into())
        .await
        .expect("could not mint blind auth");
}
