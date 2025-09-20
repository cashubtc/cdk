use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::{env, fs};

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use bip39::Mnemonic;
use cashu::quote_id::QuoteId;
use cashu::{MeltQuoteBolt12Request, MintQuoteBolt12Request, MintQuoteBolt12Response};
use cdk::amount::SplitTarget;
use cdk::cdk_database::{self, WalletDatabase};
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysetResponse,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, MintInfo, MintQuoteBolt11Request,
    MintQuoteBolt11Response, MintRequest, MintResponse, PaymentMethod, RestoreRequest,
    RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::util::unix_time;
use cdk::wallet::{AuthWallet, MintConnector, Wallet, WalletBuilder};
use cdk::{Amount, Error, Mint, StreamExt};
use cdk_fake_wallet::FakeWallet;
use tokio::sync::RwLock;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

pub struct DirectMintConnection {
    pub mint: Mint,
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl DirectMintConnection {
    pub fn new(mint: Mint) -> Self {
        Self {
            mint,
            auth_wallet: Arc::new(RwLock::new(None)),
        }
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
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        panic!("Not implemented");
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        Ok(self.mint.pubkeys().keysets)
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.mint.keyset(&keyset_id).ok_or(Error::UnknownKeySet)
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        Ok(self.mint.keysets())
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .get_mint_quote(request.into())
            .await
            .map(Into::into)
    }

    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .check_mint_quote(&QuoteId::from_str(quote_id)?)
            .await
            .map(Into::into)
    }

    async fn post_mint(&self, request: MintRequest<String>) -> Result<MintResponse, Error> {
        let request_id: MintRequest<QuoteId> = request.try_into().unwrap();
        self.mint.process_mint_request(request_id).await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .get_melt_quote(request.into())
            .await
            .map(Into::into)
    }

    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .check_melt_quote(&QuoteId::from_str(quote_id)?)
            .await
            .map(Into::into)
    }

    async fn post_melt(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let request_uuid = request.try_into().unwrap();
        self.mint.melt(&request_uuid).await.map(Into::into)
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

    /// Get the auth wallet for the client
    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        self.auth_wallet.read().await.clone()
    }

    /// Set auth wallet on client
    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        let mut auth_wallet = self.auth_wallet.write().await;

        *auth_wallet = wallet;
    }

    async fn post_mint_bolt12_quote(
        &self,
        request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let res: MintQuoteBolt12Response<QuoteId> =
            self.mint.get_mint_quote(request.into()).await?.try_into()?;
        Ok(res.into())
    }

    async fn get_mint_quote_bolt12_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let quote: MintQuoteBolt12Response<QuoteId> = self
            .mint
            .check_mint_quote(&QuoteId::from_str(quote_id)?)
            .await?
            .try_into()?;

        Ok(quote.into())
    }

    /// Melt Quote [NUT-23]
    async fn post_melt_bolt12_quote(
        &self,
        request: MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .get_melt_quote(request.into())
            .await
            .map(Into::into)
    }
    /// Melt Quote Status [NUT-23]
    async fn get_melt_bolt12_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .check_melt_quote(&QuoteId::from_str(quote_id)?)
            .await
            .map(Into::into)
    }
    /// Melt [NUT-23]
    async fn post_melt_bolt12(
        &self,
        _request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        // Implementation to be added later
        Err(Error::UnsupportedPaymentMethod)
    }
}

pub fn setup_tracing() {
    let default_filter = "debug";

    let h2_filter = "h2=warn";
    let hyper_filter = "hyper=warn";

    let env_filter = EnvFilter::new(format!("{default_filter},{h2_filter},{hyper_filter}"));

    // Ok if successful, Err if already initialized
    // Allows us to setup tracing at the start of several parallel tests
    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .try_init();
}

pub async fn create_and_start_test_mint() -> Result<Mint> {
    // Read environment variable to determine database type
    let db_type = env::var("CDK_TEST_DB_TYPE").expect("Database type set");

    let localstore = match db_type.to_lowercase().as_str() {
        "memory" => Arc::new(cdk_sqlite::mint::memory::empty().await?),
        _ => {
            // Create a temporary directory for SQLite database
            let temp_dir = create_temp_dir("cdk-test-sqlite-mint")?;
            let path = temp_dir.join("mint.db").to_str().unwrap().to_string();
            Arc::new(
                cdk_sqlite::MintSqliteDatabase::new(path.as_str())
                    .await
                    .expect("Could not create sqlite db"),
            )
        }
    };

    let mut mint_builder = MintBuilder::new(localstore.clone());

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let ln_fake_backend = FakeWallet::new(
        fee_reserve.clone(),
        HashMap::default(),
        HashSet::default(),
        2,
        CurrencyUnit::Sat,
    );

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 10_000),
            Arc::new(ln_fake_backend),
        )
        .await?;

    let mnemonic = Mnemonic::generate(12)?;

    mint_builder = mint_builder
        .with_name("pure test mint".to_string())
        .with_description("pure test mint".to_string())
        .with_urls(vec!["https://aaa".to_string()]);

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint = mint_builder
        .build_with_seed(localstore.clone(), &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(quote_ttl).await?;

    mint.start().await?;

    Ok(mint)
}

pub async fn create_test_wallet_for_mint(mint: Mint) -> Result<Wallet> {
    let connector = DirectMintConnection::new(mint.clone());

    let mint_info = mint.mint_info().await?;
    let mint_url = mint_info
        .urls
        .as_ref()
        .ok_or(anyhow!("Test mint URLs list is unset"))?
        .first()
        .ok_or(anyhow!("Test mint has empty URLs list"))?;

    let seed = Mnemonic::generate(12)?.to_seed_normalized("");
    let unit = CurrencyUnit::Sat;

    // Read environment variable to determine database type
    let db_type = env::var("CDK_TEST_DB_TYPE").expect("Database type set");

    let localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync> =
        match db_type.to_lowercase().as_str() {
            "sqlite" => {
                // Create a temporary directory for SQLite database
                let temp_dir = create_temp_dir("cdk-test-sqlite-wallet")?;
                let path = temp_dir.join("wallet.db").to_str().unwrap().to_string();
                let database = cdk_sqlite::WalletSqliteDatabase::new(path.as_str())
                    .await
                    .expect("Could not create sqlite db");
                Arc::new(database)
            }
            "redb" => {
                // Create a temporary directory for ReDB database
                let temp_dir = create_temp_dir("cdk-test-redb-wallet")?;
                let path = temp_dir.join("wallet.redb");
                let database = cdk_redb::WalletRedbDatabase::new(&path)
                    .expect("Could not create redb mint database");
                Arc::new(database)
            }
            "memory" => {
                let database = cdk_sqlite::wallet::memory::empty().await?;
                Arc::new(database)
            }
            _ => {
                bail!("Db type not set")
            }
        };

    let wallet = WalletBuilder::new()
        .mint_url(mint_url.parse().unwrap())
        .unit(unit)
        .localstore(localstore)
        .seed(seed)
        .client(connector)
        .build()?;

    Ok(wallet)
}

/// Creates a mint quote for the given amount and checks its state in a loop. Returns when
/// amount is minted.
/// Creates a temporary directory with a unique name based on the prefix
fn create_temp_dir(prefix: &str) -> Result<PathBuf> {
    let temp_dir = env::temp_dir();
    let unique_dir = temp_dir.join(format!("{}-{}", prefix, Uuid::new_v4()));
    fs::create_dir_all(&unique_dir)?;
    Ok(unique_dir)
}

pub async fn fund_wallet(
    wallet: Wallet,
    amount: u64,
    split_target: Option<SplitTarget>,
) -> Result<Amount> {
    let desired_amount = Amount::from(amount);
    let quote = wallet.mint_quote(desired_amount, None).await?;

    Ok(wallet
        .proof_stream(quote, split_target.unwrap_or_default(), None)
        .next()
        .await
        .expect("proofs")?
        .total_amount()?)
}
