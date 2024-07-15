use anyhow::Result;
use cdk::wallet::MultiMintWallet;

pub async fn mint_pending(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    let amounts = multi_mint_wallet.check_all_mint_quotes(None).await?;

    for (unit, amount) in amounts {
        println!("Unit: {}, Amount: {}", unit, amount);
    }

    Ok(())
}
