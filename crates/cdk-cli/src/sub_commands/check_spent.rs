use std::collections::HashMap;

use anyhow::Result;
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;

pub async fn check_spent(wallets: HashMap<UncheckedUrl, Wallet>) -> Result<()> {
    for wallet in wallets.values() {
        let amount = wallet.check_all_pending_proofs().await?;

        println!("Amount marked as spent: {}", amount);
    }

    Ok(())
}
