use std::{str::FromStr, sync::Arc, time::Duration};

use bitcoin::key::rand::random;
use cdk::{
    amount::{Amount, SplitTarget},
    cdk_database::WalletMemoryDatabase,
    mint_url::MintUrl,
    nuts::CurrencyUnit,
    wallet::{MultiMintWallet, Wallet},
};
use cdk_nostr::nwc::{ConnectionBudget, NostrWalletConnect, WalletConnection};
use nostr_sdk::{nips::nip47::NostrWalletConnectURI, Keys};
use url::Url;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("Usage: {} <relay_url> <mint_url> <balance>", args[0]);
        std::process::exit(1);
    }

    let relay_url = Url::from_str(&args[1])?;
    let mint_url = MintUrl::from_str(&args[2])?;
    let amount = Amount::from(&args[3].parse::<u64>()?);

    let service_keys = Keys::generate();
    let connection_keys = Keys::generate();
    let connect_uri = NostrWalletConnectURI::new(
        service_keys.public_key(),
        relay_url,
        connection_keys.secret_key().clone(),
        None,
    );
    println!("Connect URI: {}", connect_uri);

    let wallet = Wallet::new(
        &mint_url.to_string(),
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &random::<[u8; 32]>(),
        None,
    )?;
    let mint_quote = wallet.mint_quote(amount, None).await?;
    println!("Request: {}", mint_quote.request);
    println!(
        "Press enter to mint {} {} at {} (quote id: {})...",
        mint_quote.amount, mint_quote.unit, mint_url, mint_quote.id
    );
    let _ = std::io::stdin().read_line(&mut String::new());
    let amount = wallet.mint(&mint_quote.id, SplitTarget::None, None).await?;
    println!("Minted {} sats", amount);

    let nwc = NostrWalletConnect::new(
        vec![WalletConnection::from_uri(
            connect_uri,
            ConnectionBudget {
                total_budget_msats: amount * 1000.into(),
                ..Default::default()
            },
        )],
        MultiMintWallet::new(vec![wallet]),
        service_keys.secret_key().clone(),
    );
    println!("Waiting for payment requests...");
    let payments = nwc
        .background_check_for_requests(Duration::from_secs(60))
        .await?;
    println!("Payments: {:?}", payments);

    Ok(())
}
