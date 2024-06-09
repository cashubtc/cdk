use anyhow::Result;
use cdk::wallet::Wallet;

pub async fn pending_mints(wallet: Wallet) -> Result<()> {
    let amount_claimed = wallet.check_all_mint_quotes().await?;

    println!("Amount minted: {amount_claimed}");
    Ok(())
}
