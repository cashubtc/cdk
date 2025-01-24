use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::error::Error;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload, SecretKey, SpendingConditions};
use cdk::wallet::types::SendKind;
use cdk::wallet::{Wallet, WalletSubscription};
use cdk::Amount;
use rand::Rng;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let default_filter = "debug";

    let sqlx_filter = "sqlx=warn,hyper_util=warn,reqwest=warn,rustls=warn";

    let env_filter = EnvFilter::new(format!("{},{}", default_filter, sqlx_filter));

    // Parse input
    tracing_subscriber::fmt().with_env_filter(env_filter).init();

    // Initialize the memory store for the wallet
    let localstore = WalletMemoryDatabase::default();

    // Generate a random seed for the wallet
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    // Define the mint URL and currency unit
    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(50);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None)?;

    // Request a mint quote from the wallet
    let quote = wallet.mint_quote(amount, None).await?;

    println!("Minting nuts ...");

    // Subscribe to updates on the mint quote state
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await;

    // Wait for the mint quote to be paid
    while let Some(msg) = subscription.recv().await {
        if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
            if response.state == MintQuoteState::Paid {
                break;
            }
        }
    }

    // Mint the received amount
    let _receive_amount = wallet.mint(&quote.id, SplitTarget::default(), None).await?;

    // Generate a secret key for spending conditions
    let secret = SecretKey::generate();

    // Create spending conditions using the generated public key
    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);

    // Get the total balance of the wallet
    let bal = wallet.total_balance().await?;
    println!("{}", bal);

    // Send a token with the specified amount and spending conditions
    let token = wallet
        .send(
            10.into(),
            None,
            Some(spending_conditions),
            &SplitTarget::default(),
            &SendKind::default(),
            false,
        )
        .await?;

    println!("Created token locked to pubkey: {}", secret.public_key());
    println!("{}", token);

    // Receive the token using the secret key
    let amount = wallet
        .receive(&token.to_string(), SplitTarget::default(), &[secret], &[])
        .await?;

    println!("Redeemed locked token worth: {}", u64::from(amount));

    Ok(())
}
