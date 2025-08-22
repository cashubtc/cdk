use std::sync::Arc;
use std::time::Duration;

use cdk::error::Error;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{SendOptions, Wallet};
use cdk::{Amount, OidcClient};
use cdk_common::amount::SplitTarget;
use cdk_common::{MintInfo, ProofsMethods};
use cdk_sqlite::wallet::memory;
use rand::Rng;
use tracing_subscriber::EnvFilter;

const TEST_USERNAME: &str = "cdk-test";
const TEST_PASSWORD: &str = "cdkpassword";

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Set up logging
    let default_filter = "debug";
    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn,rustls=warn";
    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Initialize the memory store for the wallet
    let localstore = memory::empty().await?;

    // Generate a random seed for the wallet
    let seed = rand::rng().random::<[u8; 64]>();

    // Define the mint URL and currency unit
    let mint_url = "http://127.0.0.1:8085";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(50);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    let mint_info = wallet
        .fetch_mint_info()
        .await
        .expect("mint info")
        .expect("could not get mint info");

    // Request a mint quote from the wallet
    let quote = wallet.mint_quote(amount, None).await;

    println!("Minting nuts ... {:?}", quote);

    // Getting the CAT token is not inscope of cdk and expected to be handled by the implemntor
    // We just use this helper fn with password auth for testing
    let access_token = get_access_token(&mint_info).await;

    wallet.set_cat(access_token).await.unwrap();

    wallet
        .mint_blind_auth(10.into())
        .await
        .expect("Could not mint blind auth");

    let quote = wallet.mint_quote(amount, None).await.unwrap();
    let proofs = wallet
        .wait_and_mint_quote(quote, SplitTarget::default(), None, Duration::from_secs(10))
        .await
        .unwrap();

    println!("Received: {}", proofs.total_amount()?);

    // Get the total balance of the wallet
    let balance = wallet.total_balance().await?;
    println!("Wallet balance: {}", balance);

    let prepared_send = wallet
        .prepare_send(10.into(), SendOptions::default())
        .await?;
    let token = prepared_send.confirm(None).await?;

    println!("Created token: {}", token);

    let remaining_blind_auth = wallet.get_unspent_auth_proofs().await?.len();

    // We started with 10 blind tokens we expect 8 ath this point
    // 1 is used for the mint quote + 1 used for the mint
    // The swap is not expected to use one as it will be offline or we have "/swap" as an unprotected endpoint in the mint config
    assert_eq!(remaining_blind_auth, 8);

    println!("Remaining blind auth: {}", remaining_blind_auth);

    Ok(())
}

async fn get_access_token(mint_info: &MintInfo) -> String {
    let openid_discovery = mint_info
        .nuts
        .nut21
        .clone()
        .expect("Nut21 defined")
        .openid_discovery;

    let oidc_client = OidcClient::new(openid_discovery, None);

    // Get the token endpoint from the OIDC configuration
    let token_url = oidc_client
        .get_oidc_config()
        .await
        .expect("Failed to get OIDC config")
        .token_endpoint;

    // Create the request parameters
    let params = [
        ("grant_type", "password"),
        ("client_id", "cashu-client"),
        ("username", TEST_USERNAME),
        ("password", TEST_PASSWORD),
    ];

    // Make the token request directly
    let client = reqwest::Client::new();
    let response = client
        .post(token_url)
        .form(&params)
        .send()
        .await
        .expect("Failed to send token request");

    let token_response: serde_json::Value = response
        .json()
        .await
        .expect("Failed to parse token response");

    token_response["access_token"]
        .as_str()
        .expect("No access token in response")
        .to_string()
}
