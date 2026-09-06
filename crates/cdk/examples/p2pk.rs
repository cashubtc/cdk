#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, SecretKey, SpendingConditions};
use cdk::wallet::advanced::{ProofQuery, ReceiveAdvancedOptions, SendAdvancedOptions};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::receive::ReceiveRequest;
use cdk::wallet::send::SendRequest;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;
use tracing_subscriber::EnvFilter;

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
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(100);

    // Create a new wallet
    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        localstore,
        seed,
    ))?;

    let session = wallet.request_mint(MintRequest::bolt11(amount)).await?;
    session.wait(Duration::from_secs(10)).await?;

    // Mint the received amount
    let proof_amounts: Vec<String> = wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await?
        .iter()
        .map(|proof| proof.proof.amount.to_string())
        .collect();
    println!("Minted nuts: [{}]", proof_amounts.join(", "));

    // Generate a secret key for spending conditions
    let secret = SecretKey::generate();

    // Create spending conditions using the generated public key
    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);

    // Get the total balance of the wallet
    let bal = wallet.balance().await?.available;
    println!("Total balance: {}", bal);

    let token_amount_to_send = Amount::from(10);

    // Send a token with the specified amount and spending conditions
    let plan = wallet
        .plan_send(
            SendRequest::new(token_amount_to_send).with_advanced(SendAdvancedOptions {
                conditions: Some(spending_conditions),
                ..Default::default()
            }),
        )
        .await?;

    let fee = plan.fee();

    println!("Fee: {}", fee);

    let token = plan.execute().await?.token;

    println!("Created token locked to pubkey: {}", secret.public_key());
    println!("{}", token);

    // Receive the token using the secret key
    let amount = wallet
        .receive(
            ReceiveRequest::new(token.to_string()).with_advanced(ReceiveAdvancedOptions {
                p2pk_signing_keys: vec![secret],
                ..Default::default()
            }),
        )
        .await?
        .amount;

    assert!(amount == token_amount_to_send);

    println!("Redeemed locked token worth: {}", u64::from(amount));

    Ok(())
}
