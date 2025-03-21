use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::MintDatabase;
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintBolt11Request,
    MintBolt11Response, MintInfo, MintQuoteBolt11Request, MintQuoteBolt11Response, PaymentMethod,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::util::unix_time;
use cdk::wallet::client::MintConnector;
use cdk::wallet::Wallet;
use cdk::{Amount, Error, Mint};
use cdk_fake_wallet::FakeWallet;
use tokio::sync::{Mutex, Notify};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::wait_for_mint_to_be_paid;

pub struct DirectMintConnection {
    pub mint: Arc<Mint>,
}

impl DirectMintConnection {
    pub fn new(mint: Arc<Mint>) -> Self {
        Self { mint }
    }
}

impl Debug for DirectMintConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DirectMintConnection",)
    }
}

/// Implements the generic [MintConnector] (i.e. use the interface that expects to communicate
/// to a generic mint, where we don't know that quote ID's are [Uuid]s) for [DirectMintConnection],
/// where we know we're dealing with a mint that uses [Uuid]s for quotes.
/// Convert the requests and responses between the [String] and [Uuid] variants as necessary.
#[async_trait]
impl MintConnector for DirectMintConnection {
    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        self.mint.pubkeys().await.map(|pks| pks.keysets)
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.mint
            .keyset(&keyset_id)
            .await
            .and_then(|res| res.ok_or(Error::UnknownKeySet))
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        self.mint.keysets().await
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .get_mint_bolt11_quote(request)
            .await
            .map(Into::into)
    }

    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
        self.mint
            .check_mint_quote(&quote_id_uuid)
            .await
            .map(Into::into)
    }

    async fn post_mint(
        &self,
        request: MintBolt11Request<String>,
    ) -> Result<MintBolt11Response, Error> {
        let request_uuid = request.try_into().unwrap();
        self.mint.process_mint_request(request_uuid).await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .get_melt_bolt11_quote(&request)
            .await
            .map(Into::into)
    }

    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
        self.mint
            .check_melt_quote(&quote_id_uuid)
            .await
            .map(Into::into)
    }

    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let request_uuid = request.try_into().unwrap();
        self.mint.melt_bolt11(&request_uuid).await.map(Into::into)
    }

    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        self.mint.process_swap_request(swap_request).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint.mint_info().await?.clone().time(unix_time()))
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.mint.check_state(&request).await
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.mint.restore(request).await
    }
}

pub fn setup_tracing() {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{}",
        default_filter, sqlx_filter, hyper_filter
    ));

    // Ok if successful, Err if already initialized
    // Allows us to setup tracing at the start of several parallel tests
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

pub async fn create_and_start_test_mint() -> Result<Arc<Mint>> {
    let mut mint_builder = MintBuilder::new();

    let database = cdk_sqlite::mint::memory::empty().await?;

    let localstore = Arc::new(database);
    mint_builder = mint_builder.with_localstore(localstore.clone());

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let ln_fake_backend = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        0,
    );

    mint_builder = mint_builder
        .add_ln_backend(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 1_000),
            Arc::new(ln_fake_backend),
        )
        .await?;

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("pure test mint".to_string())
        .with_description("pure test mint".to_string())
        .with_urls(vec!["https://aaa".to_string()])
        .with_seed(mnemonic.to_seed_normalized("").to_vec());

    localstore
        .set_mint_info(mint_builder.mint_info.clone())
        .await?;
    let quote_ttl = QuoteTTL::new(10000, 10000);
    localstore.set_quote_ttl(quote_ttl).await?;

    let mint = mint_builder.build().await?;

    let mint_arc = Arc::new(mint);

    let mint_arc_clone = Arc::clone(&mint_arc);
    let shutdown = Arc::new(Notify::new());
    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint_arc_clone.wait_for_paid_invoices(shutdown).await }
    });

    Ok(mint_arc)
}

async fn create_test_wallet_for_mint(mint: Arc<Mint>) -> Result<Wallet> {
    let connector = DirectMintConnection::new(mint.clone());

    let mint_info = mint.mint_info().await?;
    let mint_url = mint_info
        .urls
        .as_ref()
        .ok_or(anyhow!("Test mint URLs list is unset"))?
        .first()
        .ok_or(anyhow!("Test mint has empty URLs list"))?;

    let seed = Arc::new(Mnemonic::generate(12)?.to_seed_normalized(""));
    let unit = CurrencyUnit::Sat;
    let localstore = cdk_sqlite::wallet::memory::empty().await?;
    let mut wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    wallet.set_client(connector);

    Ok(wallet)
}

pub async fn create_test_wallet_arc_for_mint(mint: Arc<Mint>) -> Result<Arc<Wallet>> {
    create_test_wallet_for_mint(mint).await.map(Arc::new)
}

pub async fn create_test_wallet_arc_mut_for_mint(mint: Arc<Mint>) -> Result<Arc<Mutex<Wallet>>> {
    create_test_wallet_for_mint(mint)
        .await
        .map(Mutex::new)
        .map(Arc::new)
}

/// Creates a mint quote for the given amount and checks its state in a loop. Returns when
/// amount is minted.
pub async fn fund_wallet(wallet: Arc<Wallet>, amount: u64) -> Result<Amount> {
    let desired_amount = Amount::from(amount);
    let quote = wallet.mint_quote(desired_amount, None).await?;

    wait_for_mint_to_be_paid(&wallet, &quote.id, 60).await?;

    Ok(wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await?
        .total_amount()?)
}
