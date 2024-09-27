use std::{str::FromStr, sync::Arc};

use bitcoin::key::rand::random;
use cdk::{
    amount::{Amount, SplitTarget},
    cdk_database::WalletMemoryDatabase,
    mint_url::MintUrl,
    nuts::CurrencyUnit,
    wallet::{MultiMintWallet, Wallet},
};
use cdk_nostr::zap::NutZapper;
use nostr_sdk::{Client, Keys, PublicKey};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!("Usage: {} <relay_url> <mint_url> <npub> <amount>", args[0]);
        std::process::exit(1);
    }

    let relay_url = Url::from_str(&args[1])?;
    let mint_url = MintUrl::from_str(&args[2])?;
    let pubkey = PublicKey::from_str(&args[3])?;
    let amount = Amount::from(&args[4].parse::<u64>()?);

    let keys = Keys::generate();
    let client = Client::builder().signer(&keys).build();
    client.add_relay(&relay_url).await?;
    client.connect().await;

    let wallet = Wallet::new(
        &mint_url.to_string(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &random::<[u8; 32]>(),
        None,
    )?;
    let zapper = NutZapper::new(
        client,
        keys.secret_key().clone(),
        MultiMintWallet::new(vec![wallet.clone()]),
        None,
    );

    let mint_quote = wallet.mint_quote(amount, None).await?;
    println!("Request: {}", mint_quote.request);
    println!(
        "Press enter to mint {} {} at {} (quote id: {})...",
        mint_quote.amount, mint_quote.unit, mint_url, mint_quote.id
    );
    let _ = std::io::stdin().read_line(&mut String::new());
    let amount = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;
    println!("Minted {} sats", amount);

    let event_id = zapper
        .zap_from_mint(pubkey, mint_url, amount, CurrencyUnit::Sat)
        .await?;
    println!("Zap event id: {}", event_id);

    Ok(())
}
