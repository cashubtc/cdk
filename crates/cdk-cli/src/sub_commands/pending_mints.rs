use anyhow::Result;
use cdk::wallet::MultiMintWallet;
use cdk::Amount;

pub async fn mint_pending(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    let amounts = multi_mint_wallet.check_all_mint_quotes(None).await?;

    let amount = amounts.into_values().sum::<Amount>();

    println!("Amount minted: {amount}");
    Ok(())
}
