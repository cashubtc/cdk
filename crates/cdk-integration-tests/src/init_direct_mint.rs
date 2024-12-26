use std::collections::HashMap;
use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::cdk_database::mint_memory::MintMemoryDatabase;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::{CurrencyUnit, MintInfo, MintQuoteState, Nuts};
use cdk::types::QuoteTTL;
use cdk::{Amount, Mint, Wallet};
use rand::random;
use tokio::sync::Notify;

use crate::create_backends_fake_wallet;
use crate::direct_mint_connection::DirectMintConnection;

pub async fn create_and_start_test_mint() -> anyhow::Result<Arc<Mint>> {
    let fee: u64 = 0;
    let mut supported_units = HashMap::new();
    supported_units.insert(CurrencyUnit::Sat, (fee, 32));

    let nuts = Nuts::new()
        .nut07(true)
        .nut08(true)
        .nut09(true)
        .nut10(true)
        .nut11(true)
        .nut12(true)
        .nut14(true);

    let mint_info = MintInfo::new().nuts(nuts);

    let quote_ttl = QuoteTTL::new(10000, 10000);

    let mint_url = "http://aaa";

    let seed = random::<[u8; 32]>();
    let mint: Mint = Mint::new(
        mint_url,
        &seed,
        mint_info,
        quote_ttl,
        Arc::new(MintMemoryDatabase::default()),
        None,
        create_backends_fake_wallet(),
        supported_units,
        HashMap::new(),
        HashMap::new(),
    )
    .await?;

    let mint_arc = Arc::new(mint);

    let mint_arc_clone = Arc::clone(&mint_arc);
    let shutdown = Arc::new(Notify::new());
    tokio::spawn({
        let shutdown = Arc::clone(&shutdown);
        async move { mint_arc_clone.wait_for_paid_invoices(shutdown).await }
    });

    Ok(mint_arc)
}

pub fn get_mint_connector(mint: Arc<Mint>) -> DirectMintConnection {
    DirectMintConnection { mint }
}

pub fn create_test_wallet_for_mint(mint: Arc<Mint>) -> anyhow::Result<Arc<Wallet>> {
    let connector = get_mint_connector(mint);

    let seed = random::<[u8; 32]>();
    let mint_url = connector.mint.config.mint_url().to_string();
    let unit = CurrencyUnit::Sat;

    let localstore = WalletMemoryDatabase::default();
    let mut wallet = Wallet::new(&mint_url, unit, Arc::new(localstore), &seed, None, None)?;

    wallet.set_client(connector);

    Ok(Arc::new(wallet))
}

/// Creates a mint quote for the given amount and checks its state in a loop. Returns when
/// amount is minted.
pub async fn receive(wallet: Arc<Wallet>, amount: u64) -> anyhow::Result<Amount> {
    let desired_amount = Amount::from(amount);
    let quote = wallet.mint_quote(desired_amount, None).await?;

    loop {
        let status = wallet.mint_quote_state(&quote.id).await?;
        if status.state == MintQuoteState::Paid {
            break;
        }
    }

    wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .map_err(Into::into)
}
