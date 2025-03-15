use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use cdk::wallet::Wallet;
use cdk_common::{Amount, CurrencyUnit, MintQuoteState};
use cdk_sqlite::wallet::memory;
use rand::Rng;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    // Mint URL and currency unit
    let mint_url = "http://127.0.0.1:3338";
    let unit = CurrencyUnit::Sat;

    // Initialize the memory store for the wallet
    let localstore = memory::empty().await?;

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit.clone(), Arc::new(localstore), &seed, None)?;

    println!("Bootstrapping to mint {}\n", mint_url);

    // Request bootstrap coins
    let bootstrap_coins = wallet.bootstrap(100, None).await?;

    println!(
        "bootstrap coins: {}\n",
        serde_json::to_string_pretty(&bootstrap_coins).unwrap()
    );

    println!("Minting 1337 sats...\n");

    // Mint Quote
    let mint_quote = wallet.mint_quote(Amount::from(1337), None).await?;
    let mut state = wallet.mint_quote_state(&mint_quote.id).await?;
    while state.state == MintQuoteState::Unpaid {
        sleep(Duration::new(3, 0));
        state = wallet.mint_quote_state(&mint_quote.id).await?;
    }

    // Mint
    let coins = wallet.kvac_mint(&mint_quote.id, Amount::from(1337)).await?;

    for coin in coins.iter() {
        println!("coin: {}\n", serde_json::to_string_pretty(&coin).unwrap());
    }

    // Send 19 sats
    println!("Sending 19 sats...\n");
    let (sent, kept) = wallet.kvac_send(Amount::from(19)).await?;

    // Check the state of the Minted coin: should be spent
    let states = wallet.check_coins_spent(coins).await?;

    println!(
        "checked states of minted kvac tokens after send:\n {:?}\n",
        states
    );

    println!("sent: {}\n", serde_json::to_string_pretty(&sent).unwrap());
    println!("kept: {}\n", serde_json::to_string_pretty(&kept).unwrap());

    // Receive 19 sats
    println!("Receiving 19 sats...\n");
    let received = wallet.kvac_receive_coins(vec![sent]).await?;

    println!(
        "received: {}\n",
        serde_json::to_string_pretty(&received).unwrap()
    );

    // Melt 986 sats
    println!("Melting 986 sats...\n");
    let invoice = String::from("lnbc9860n1pn6892cpp54np6ukttc43sev95wtd6mxr2rld7k5rfgcsz2xnw0a6hjmr6a6fsdqqcqzzsxqyz5vqsp528cg50helvnlwgzt9dwsr86ma6eh6czup4c4ge4rs3grrshhzshs9p4gqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqpqysgqqer3ddg2wzctu4emspeyngncnx06ne9rsltekd0ffnkmf69ax0estgh93jjyyvdlyh05mvng532tlj6phyzemf7evywuygu08a52augp09lm0f");
    let melt_quote = wallet.melt_quote(invoice, None).await?;

    println!("melt_quote: {:?}\n", melt_quote);

    let coins = wallet.kvac_melt(&melt_quote.id).await?;

    println!(
        "remaining: {}\n",
        serde_json::to_string_pretty(&coins).unwrap()
    );

    // Create a new wallet and try to restore
    let localstore1 = memory::empty().await?;
    let wallet1 = Wallet::new(mint_url, unit.clone(), Arc::new(localstore1), &seed, None)?;

    // Restore
    let restored_balances = wallet1.kvac_restore(100_000).await?;

    println!(
        "restored balances: {}\n",
        serde_json::to_string_pretty(&restored_balances).unwrap()
    );

    Ok(())
}
