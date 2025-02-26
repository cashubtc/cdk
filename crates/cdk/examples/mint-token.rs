use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::error::Error;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload};
use cdk::wallet::types::SendKind;
use cdk::wallet::{Wallet, WalletSubscription};
use cdk::Amount;
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize the memory store for the wallet
    let localstore = WalletMemoryDatabase::default();

    // Generate a random seed for the wallet
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    // Define the mint URL and currency unit
    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None)?;

    // Request a mint quote from the wallet
    let quote = wallet.mint_quote(amount, None).await?;
    println!("Quote: {:#?}", quote);

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
    let proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;
    let receive_amount = proofs.total_amount()?;
    println!("Received {} from mint {}", receive_amount, mint_url);

    // Send a token with the specified amount
    let token = wallet
        .send(
            amount,
            None,
            None,
            &SplitTarget::default(),
            &SendKind::OnlineExact,
            false,
        )
        .await?;
    println!("Token:");
    println!("{}", token);

    Ok(())
}
