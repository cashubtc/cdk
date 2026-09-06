#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::error::Error;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::advanced::{MetadataSource, MintMetadataRequest};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
use cdk::{Amount, OidcClient};
use cdk_common::MintInfo;
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
    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        Arc::new(localstore),
        seed,
    ))?;

    let metadata = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest {
            source: MetadataSource::Refresh,
        })
        .await
        .expect("mint metadata");

    // Request a mint quote from the wallet
    let session = wallet.request_mint(MintRequest::bolt11(amount)).await?;

    println!("Minting nuts ... Quote ID: {}", session.id());

    // Getting the CAT token is not inscope of cdk and expected to be handled by the implemntor
    // We just use this helper fn with password auth for testing
    let access_token = get_access_token(&metadata.info).await;

    wallet.advanced().set_clear_auth_token(access_token).await?;

    wallet
        .advanced()
        .mint_blind_auth(10.into())
        .await
        .expect("Could not mint blind auth");

    let session = wallet.request_mint(MintRequest::bolt11(amount)).await?;
    let receipt = session.wait(Duration::from_secs(10)).await?;

    println!("Received: {}", receipt.amount);

    // Get the total balance of the wallet
    let balance = wallet.balance().await?.available;
    println!("Wallet balance: {}", balance);

    let send_plan = wallet.plan_send(SendRequest::new(10.into())).await?;
    let token = send_plan.execute().await?.token;

    println!("Created token: {}", token);

    let remaining_blind_auth = wallet.advanced().blind_auth_proofs().await?.len();

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
    let client_id = oidc_client
        .get_oidc_config()
        .await
        .expect("Failed to get OIDC config")
        .token_endpoint;

    // Create the request parameters
    let params = [
        ("grant_type", "password"),
        ("client_id", &client_id),
        ("username", TEST_USERNAME),
        ("password", TEST_PASSWORD),
    ];

    // Make the token request directly
    let client = cdk_common::HttpClient::new();
    let token_response: serde_json::Value = client
        .post_form(&token_url, &params)
        .await
        .expect("Failed to send token request");

    token_response["access_token"]
        .as_str()
        .expect("No access token in response")
        .to_string()
}
