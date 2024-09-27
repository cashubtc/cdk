use std::{str::FromStr, sync::Arc};

use bitcoin::key::rand::random;
use cdk::{
    cdk_database::WalletMemoryDatabase,
    mint_url::MintUrl,
    nuts::CurrencyUnit,
    wallet::{MultiMintWallet, Wallet},
};
use cdk_nostr::zap::NutZapper;
use nostr_sdk::{Client, Keys, SecretKey};
use tokio::time::Duration;
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <relay_url> <mint_url> <nsec>", args[0]);
        std::process::exit(1);
    }

    let relay_url = Url::from_str(&args[1])?;
    let mint_url = MintUrl::from_str(&args[2])?;
    let nsec = SecretKey::from_str(&args[3])?;

    let keys = Keys::new(nsec);
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

    println!("Claiming zap events for {}...", keys.public_key());
    let mut stop = false;
    loop {
        let events = zapper.get_zap_events(None).await?;
        for event in events {
            println!("Claiming zap event: {}", event.id);
            let amount = zapper.claim_zap(event).await?;
            println!("Claimed {} sats", amount);
            stop = true;
        }
        if stop {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}
