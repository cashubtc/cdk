use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk::error::Error;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{BaseHttpClient, HttpTransport, SendOptions, WalletBuilder};
use cdk::{Amount, StreamExt};
use cdk_common::mint_url::MintUrl;
use cdk_common::AuthToken;
use cdk_sqlite::wallet::memory;
use rand::random;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing_subscriber::EnvFilter;
use ureq::config::Config;
use ureq::Agent;
use url::Url;

#[derive(Debug, Clone)]
pub struct CustomHttp {
    agent: Agent,
}

impl Default for CustomHttp {
    fn default() -> Self {
        Self {
            agent: Agent::new_with_config(
                Config::builder()
                    .timeout_global(Some(Duration::from_secs(5)))
                    .no_delay(true)
                    .user_agent("Custom HTTP Transport")
                    .build(),
            ),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl HttpTransport for CustomHttp {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        panic!("Not supported");
    }

    fn with_proxy(
        &mut self,
        _proxy: Url,
        _host_matcher: Option<&str>,
        _accept_invalid_certs: bool,
    ) -> Result<(), Error> {
        panic!("Not supported");
    }

    async fn http_get<R>(&self, url: Url, _auth: Option<AuthToken>) -> Result<R, Error>
    where
        R: DeserializeOwned,
    {
        self.agent
            .get(url.as_str())
            .call()
            .map_err(|e| Error::HttpError(None, e.to_string()))?
            .body_mut()
            .read_json()
            .map_err(|e| Error::HttpError(None, e.to_string()))
    }

    /// HTTP Post request
    async fn http_post<P, R>(
        &self,
        url: Url,
        _auth_token: Option<AuthToken>,
        payload: &P,
    ) -> Result<R, Error>
    where
        P: Serialize + ?Sized + Send + Sync,
        R: DeserializeOwned,
    {
        self.agent
            .post(url.as_str())
            .send_json(payload)
            .map_err(|e| Error::HttpError(None, e.to_string()))?
            .body_mut()
            .read_json()
            .map_err(|e| Error::HttpError(None, e.to_string()))
    }
}

type CustomConnector = BaseHttpClient<CustomHttp>;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn,rustls=warn";

    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Initialize the memory store for the wallet
    let localstore = Arc::new(memory::empty().await?);

    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Define the mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let mint_url = MintUrl::from_str(mint_url)?;
    #[cfg(feature = "auth")]
    let http_client = CustomConnector::new(mint_url.clone(), None);

    #[cfg(not(feature = "auth"))]
    let http_client = CustomConnector::new(mint_url.clone());

    // Create a new wallet
    let wallet = WalletBuilder::new()
        .mint_url(mint_url)
        .unit(unit)
        .localstore(localstore)
        .seed(seed)
        .target_proof_count(3)
        .client(http_client)
        .build()?;

    let quotes = vec![
        wallet.mint_bolt12_quote(None, None).await?,
        wallet.mint_bolt12_quote(None, None).await?,
        wallet.mint_bolt12_quote(None, None).await?,
    ];

    let mut stream = wallet.mints_proof_stream(quotes, Default::default(), None);

    let stop = stream.get_cancel_token();

    let mut processed = 0;

    while let Some(proofs) = stream.next().await {
        let (mint_quote, proofs) = proofs?;

        // Mint the received amount
        let receive_amount = proofs.total_amount()?;
        tracing::info!("Received {} from mint {}", receive_amount, mint_quote.id);

        // Send a token with the specified amount
        let prepared_send = wallet.prepare_send(amount, SendOptions::default()).await?;
        let token = prepared_send.confirm(None).await?;
        tracing::info!("Token: {}", token);

        processed += 1;

        if processed == 3 {
            stop.cancel()
        }
    }

    tracing::info!("Stopped the loop after {} quotes being minted", processed);

    Ok(())
}
