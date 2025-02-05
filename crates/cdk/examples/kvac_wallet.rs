use cdk::cdk_database::WalletMemoryDatabase;
use cdk::wallet::Wallet;
use cdk_common::{Amount, CurrencyUnit, MintQuoteState};
use rand::Rng;
use std::{sync::Arc, thread::sleep, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    // Mint URL and currency unit
    let mint_url = "http://127.0.0.1:3338";
    let unit = CurrencyUnit::Sat;

    // Initialize the memory store
    let localstore = WalletMemoryDatabase::default();

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None)?;

    // Request bootstrap coins
    let bootstrap_coins = wallet.bootstrap(2, None).await?;

    println!(
        "bootstrap coins: {}",
        serde_json::to_string_pretty(&bootstrap_coins).unwrap()
    );

    // Mint Quote
    let mint_quote = wallet.mint_quote(Amount::from(15), None).await?;
    let mut state = wallet.mint_quote_state(&mint_quote.id).await?;
    while state.state == MintQuoteState::Unpaid {
        sleep(Duration::new(3, 0));
        state = wallet.mint_quote_state(&mint_quote.id).await?;
    }

    // Send
    let coins = wallet.kvac_mint(&mint_quote.id, Amount::from(15)).await?;

    for coin in coins {
        println!(
            "coin: {}",
            serde_json::to_string_pretty(&coin).unwrap(),
        );
    }

    Ok(())
}
