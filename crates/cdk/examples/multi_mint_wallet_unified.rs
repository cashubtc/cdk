//! Example demonstrating the new unified MultiMintWallet interface
//!
//! This example shows how the improved MultiMintWallet API makes it easier to use
//! by providing direct mint, melt, send, and receive functions similar to the single Wallet.

use cdk::nuts::CurrencyUnit;
use cdk::wallet::{MultiMintWallet, SendOptions};
use cdk::Amount;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Assume we have a configured MultiMintWallet
    // let multi_wallet = MultiMintWallet::new(...);

    // Example 1: Get total balance across all wallets for a unit
    // No need to manually iterate through wallets
    // let total_sats = multi_wallet.total_balance(&CurrencyUnit::Sat).await?;
    // println!("Total balance across all SAT wallets: {}", total_sats);

    // Example 2: Send tokens with automatic wallet selection
    // The wallet with the best balance and fees is automatically selected
    // let token = multi_wallet.send(
    //     Amount::from(1000),
    //     &CurrencyUnit::Sat,
    //     SendOptions::default(),
    // ).await?;
    // println!("Token sent: {}", token);

    // Example 3: Pay an invoice with automatic wallet selection
    // The wallet with the best route and lowest fees is automatically selected
    // let invoice = "lnbc...";
    // let result = multi_wallet.melt(
    //     invoice,
    //     &CurrencyUnit::Sat,
    //     None,  // MeltOptions
    //     Some(Amount::from(10)), // max_fee
    // ).await?;
    // println!("Invoice paid: {:?}", result);

    // Example 4: Consolidate proofs across wallets
    // Optimizes proof distribution by consolidating small proofs into larger ones
    // let consolidated = multi_wallet.consolidate(&CurrencyUnit::Sat).await?;
    // println!("Consolidated {} sats worth of proofs", consolidated);

    // Example 5: Swap proofs with automatic wallet selection
    // Automatically finds a wallet with proofs to swap
    // let swapped = multi_wallet.swap(
    //     &CurrencyUnit::Sat,
    //     Some(Amount::from(500)),
    //     None, // SpendingConditions
    // ).await?;
    // println!("Swapped proofs: {:?}", swapped);

    // Example 6: For specific wallet operations, you can still use the explicit methods
    // let wallet_key = WalletKey::new(mint_url, unit);
    // let token = multi_wallet.send_from_wallet(
    //     &wallet_key,
    //     Amount::from(1000),
    //     SendOptions::default(),
    // ).await?;

    // Example 7: Using the new builder patterns for complex operations
    // let token = multi_wallet
    //     .send_builder(Amount::from(1000), CurrencyUnit::Sat)
    //     .include_fee(true)
    //     .max_fee(Amount::from(10))
    //     .send()
    //     .await?;

    // let result = multi_wallet
    //     .melt_builder("lnbc...".to_string(), CurrencyUnit::Sat)
    //     .enable_mpp(true)
    //     .max_fee(Amount::from(20))
    //     .pay()
    //     .await?;

    println!("Example completed - uncomment code to test with real wallet");
    Ok(())
}
