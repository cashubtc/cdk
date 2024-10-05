use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use axum::Router;
use bip39::Mnemonic;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk::cdk_lightning::MintLightning;
use cdk::dhke::construct_proofs;
use cdk::mint::FeeReserve;
use cdk::nuts::{
    CurrencyUnit, Id, KeySet, MeltMethodSettings, MintInfo, MintMethodSettings, MintQuoteState,
    Nuts, PaymentMethod, PreMintSecrets, Proofs, State,
};
use cdk::types::{LnKey, QuoteTTL};
use cdk::wallet::client::HttpClient;
use cdk::{Mint, Wallet};
use cdk_fake_wallet::FakeWallet;
use init_regtest::{get_mint_addr, get_mint_port, get_mint_url};
use tokio::sync::Notify;
use tokio::time::sleep;
use tower_http::cors::CorsLayer;

pub mod init_fake_wallet;
pub mod init_regtest;

pub fn create_backends_fake_wallet(
) -> HashMap<LnKey, Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>> {
    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };
    let mut ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
    > = HashMap::new();
    let ln_key = LnKey::new(CurrencyUnit::Sat, PaymentMethod::Bolt11);

    let wallet = Arc::new(FakeWallet::new(
        fee_reserve.clone(),
        MintMethodSettings::default(),
        MeltMethodSettings::default(),
        HashMap::default(),
        HashSet::default(),
        0,
    ));

    ln_backends.insert(ln_key, wallet.clone());

    ln_backends
}

pub async fn start_mint(
    ln_backends: HashMap<
        LnKey,
        Arc<dyn MintLightning<Err = cdk::cdk_lightning::Error> + Sync + Send>,
    >,
    supported_units: HashMap<CurrencyUnit, (u64, u8)>,
) -> Result<()> {
    let nuts = Nuts::new()
        .nut07(true)
        .nut08(true)
        .nut09(true)
        .nut10(true)
        .nut11(true)
        .nut12(true)
        .nut14(true);

    let mint_info = MintInfo::new().nuts(nuts);

    let mnemonic = Mnemonic::generate(12)?;

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint = Mint::new(
        &get_mint_url(),
        &mnemonic.to_seed_normalized(""),
        mint_info,
        quote_ttl,
        Arc::new(MintMemoryDatabase::default()),
        ln_backends.clone(),
        supported_units,
    )
    .await?;
    let cache_time_to_live = 3600;
    let cache_time_to_idle = 3600;

    let mint_arc = Arc::new(mint);

    let v1_service = cdk_axum::create_mint_router(
        Arc::clone(&mint_arc),
        cache_time_to_live,
        cache_time_to_idle,
    )
    .await?;

    let mint_service = Router::new()
        .merge(v1_service)
        .layer(CorsLayer::permissive());

    let mint = Arc::clone(&mint_arc);

    let shutdown = Arc::new(Notify::new());

    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint.wait_for_paid_invoices(shutdown).await }
    });

    axum::Server::bind(
        &format!("{}:{}", get_mint_addr(), get_mint_port())
            .as_str()
            .parse()?,
    )
    .serve(mint_service.into_make_service())
    .await?;

    Ok(())
}

pub async fn wallet_mint(
    wallet: Arc<Wallet>,
    amount: Amount,
    split_target: SplitTarget,
    description: Option<String>,
) -> Result<()> {
    let quote = wallet.mint_quote(amount, description).await?;

    loop {
        let status = wallet.mint_quote_state(&quote.id).await?;

        if status.state == MintQuoteState::Paid {
            break;
        }
        println!("{:?}", status);

        sleep(Duration::from_secs(2)).await;
    }

    let receive_amount = wallet.mint(&quote.id, split_target, None).await?;

    println!("Minted: {}", receive_amount);

    Ok(())
}

pub async fn mint_proofs(
    mint_url: &str,
    amount: Amount,
    keyset_id: Id,
    mint_keys: &KeySet,
    description: Option<String>,
) -> anyhow::Result<Proofs> {
    println!("Minting for ecash");
    println!();

    let wallet_client = HttpClient::new();

    let mint_quote = wallet_client
        .post_mint_quote(mint_url.parse()?, 1.into(), CurrencyUnit::Sat, description)
        .await?;

    println!("Please pay: {}", mint_quote.request);

    loop {
        let status = wallet_client
            .get_mint_quote_status(mint_url.parse()?, &mint_quote.quote)
            .await?;

        if status.state == MintQuoteState::Paid {
            break;
        }
        println!("{:?}", status.state);

        sleep(Duration::from_secs(2)).await;
    }

    let premint_secrets = PreMintSecrets::random(keyset_id, amount, &SplitTarget::default())?;

    let mint_response = wallet_client
        .post_mint(
            mint_url.parse()?,
            &mint_quote.quote,
            premint_secrets.clone(),
        )
        .await?;

    let pre_swap_proofs = construct_proofs(
        mint_response.signatures,
        premint_secrets.rs(),
        premint_secrets.secrets(),
        &mint_keys.clone().keys,
    )?;

    Ok(pre_swap_proofs)
}

// Get all pending from wallet and attempt to swap
// Will panic if there are no pending
// Will return Ok if swap fails as expected
pub async fn attempt_to_swap_pending(wallet: &Wallet) -> Result<()> {
    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(!pending.is_empty());

    let swap = wallet
        .swap(
            None,
            SplitTarget::None,
            pending.into_iter().map(|p| p.proof).collect(),
            None,
            false,
        )
        .await;

    match swap {
        Ok(_swap) => {
            bail!("These proofs should be pending")
        }
        Err(err) => match err {
            cdk::error::Error::TokenPending => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error")
            }
        },
    }

    Ok(())
}
