use std::collections::HashMap;

use anyhow::Result;
use cdk::wallet::Wallet;
use cdk::{Amount, UncheckedUrl};

pub async fn mint_pending(wallets: HashMap<UncheckedUrl, Wallet>) -> Result<()> {
    let mut amount_claimed = Amount::ZERO;
    for wallet in wallets.values() {
        let claimed = wallet.check_all_mint_quotes().await?;
        amount_claimed += claimed;
    }

    println!("Amount minted: {amount_claimed}");
    Ok(())
}
