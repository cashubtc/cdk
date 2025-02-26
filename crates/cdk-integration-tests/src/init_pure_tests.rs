use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk::cdk_database::{MintDatabase, WalletMemoryDatabase};
use cdk::mint::{FeeReserve, MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    AuthToken, CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysetResponse,
    MeltBolt11Request, MeltQuoteBolt11Request, MeltQuoteBolt11Response, MintAuthRequest,
    MintBolt11Request, MintBolt11Response, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, PaymentMethod, RestoreRequest, RestoreResponse, SwapRequest,
    SwapResponse, nut21::{Method, ProtectedEndpoint, RoutePath},
};
use cdk::types::QuoteTTL;
use cdk::util::unix_time;
use cdk::wallet::{MintConnector, Wallet};
use cdk::{Amount, Error, Mint};
use cdk_fake_wallet::FakeWallet;
use tokio::sync::Notify;
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
    async fn get_mint_keys(&self, _auth_token: Option<AuthToken>) -> Result<Vec<KeySet>, Error> {
        self.mint.pubkeys().await.map(|pks| pks.keysets)
    }

    async fn get_mint_keyset(
        &self,
        keyset_id: Id,
        _auth_token: Option<AuthToken>,
    ) -> Result<KeySet, Error> {
        self.mint
            .keyset(&keyset_id)
            .await
            .and_then(|res| res.ok_or(Error::UnknownKeySet))
    }

    async fn get_mint_keysets(
        &self,
        _auth_token: Option<AuthToken>,
    ) -> Result<KeysetResponse, Error> {
        self.mint.keysets().await
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
        _auth_token: Option<AuthToken>,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .get_mint_bolt11_quote(request)
            .await
            .map(Into::into)
    }

    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
        auth_token: Option<AuthToken>,
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
        auth_token: Option<AuthToken>,
    ) -> Result<MintBolt11Response, Error> {
        self.mint
            .verify_auth(
                auth_token,
                &ProtectedEndpoint::new(Method::Post, RoutePath::MintBolt11),
            )
            .await?;
        let request_uuid = request.try_into().unwrap();
        self.mint.process_mint_request(request_uuid).await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
        auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .verify_auth(
                auth_token,
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuoteBolt11),
            )
            .await?;
        self.mint
            .get_melt_bolt11_quote(&request)
            .await
            .map(Into::into)
    }

    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
        auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let quote_id_uuid = Uuid::from_str(quote_id).unwrap();
        self.mint
            .check_melt_quote(auth_token, &quote_id_uuid)
            .await
            .map(Into::into)
    }

    async fn post_melt(
        &self,
        request: MeltBolt11Request<String>,
        auth_token: Option<AuthToken>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .verify_auth(
                auth_token,
                &ProtectedEndpoint::new(Method::Post, RoutePath::MeltBolt11),
            )
            .await?;
        let request_uuid = request.try_into().unwrap();
        self.mint.melt_bolt11(&request_uuid).await.map(Into::into)
    }

    async fn post_swap(
        &self,
        swap_request: SwapRequest,
        auth_token: Option<AuthToken>,
    ) -> Result<SwapResponse, Error> {
        self.mint
            .verify_auth(
                auth_token,
                &ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
            )
            .await?;
        self.mint.process_swap_request(swap_request).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint.mint_info().await?.clone().time(unix_time()))
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
        auth_token: Option<AuthToken>,
    ) -> Result<CheckStateResponse, Error> {
        self.mint
            .verify_auth(
                auth_token,
                &ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate),
            )
            .await?;
        self.mint.check_state(&request).await
    }

    async fn post_restore(
        &self,
        request: RestoreRequest,
        auth_token: Option<AuthToken>,
    ) -> Result<RestoreResponse, Error> {
        self.mint
            .verify_auth(
                auth_token,
                &ProtectedEndpoint::new(Method::Post, RoutePath::Restore),
            )
            .await?;
        self.mint.restore(request).await
    }

    /// Get Blind Auth keys
    async fn get_mint_blind_auth_keys(&self) -> Result<Vec<KeySet>, Error> {
        Ok(self.mint.auth_pubkeys().await?.keysets)
    }

    /// Get Blind Auth Keyset
    async fn get_mint_blind_auth_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        Ok(self
            .mint
            .keyset(&keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?)
    }

    /// Get Blind Auth keysets
    async fn get_mint_blind_auth_keysets(&self) -> Result<KeysetResponse, Error> {
        self.mint.auth_keysets().await
    }

    /// Post mint blind auth
    async fn post_mint_blind_auth(
        &self,
        request: MintAuthRequest,
        auth_token: AuthToken,
    ) -> Result<MintBolt11Response, Error> {
        self.mint.mint_blind_auth(auth_token, request).await
    }
}

pub async fn create_and_start_test_mint() -> anyhow::Result<Arc<Mint>> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!(
        "{},{},{}",
        default_filter, sqlx_filter, hyper_filter
    ));

    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    let mut mint_builder = MintBuilder::new();

    let database = MintMemoryDatabase::default();

    let localstore = Arc::new(database);
    mint_builder = mint_builder.with_localstore(localstore.clone());

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let ln_fake_backend = Arc::new(FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        0,
    ));

    mint_builder = mint_builder.add_ln_backend(
        CurrencyUnit::Sat,
        PaymentMethod::Bolt11,
        MintMeltLimits::new(1, 1_000),
        ln_fake_backend,
    );

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("pure test mint".to_string())
        .with_description("pure test mint".to_string())
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

pub fn create_test_wallet_for_mint(mint: Arc<Mint>) -> anyhow::Result<Arc<Wallet>> {
    let connector = DirectMintConnection::new(mint);

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let mint_url = "http://aa".to_string();
    let unit = CurrencyUnit::Sat;
    let localstore = WalletMemoryDatabase::default();
    let mut wallet = Wallet::new(&mint_url, unit, Arc::new(localstore), &seed, None, None)?;

    wallet.set_client(connector);

    Ok(Arc::new(wallet))
}

/// Creates a mint quote for the given amount and checks its state in a loop. Returns when
/// amount is minted.
pub async fn fund_wallet(wallet: Arc<Wallet>, amount: u64) -> anyhow::Result<Amount> {
    let desired_amount = Amount::from(amount);
    let quote = wallet.mint_quote(desired_amount, None).await?;

    wait_for_mint_to_be_paid(&wallet, &quote.id, 60).await?;

    Ok(wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await?
        .total_amount()?)
}
