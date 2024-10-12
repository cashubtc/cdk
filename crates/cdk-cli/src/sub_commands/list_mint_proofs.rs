use std::collections::BTreeMap;

use anyhow::Result;
use cdk::{
    mint_url::MintUrl,
    nuts::{CurrencyUnit, Proof},
    wallet::multi_mint_wallet::MultiMintWallet,
};

pub async fn proofs(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    list_proofs(multi_mint_wallet).await?;
    Ok(())
}

async fn list_proofs(
    multi_mint_wallet: &MultiMintWallet,
) -> Result<Vec<(MintUrl, (Vec<Proof>, CurrencyUnit))>> {
    let wallets_proofs: BTreeMap<MintUrl, (Vec<Proof>, CurrencyUnit)> =
        multi_mint_wallet.list_proofs().await?;

    let mut proofs_vec = Vec::with_capacity(wallets_proofs.len());

    for (i, (mint_url, proofs)) in wallets_proofs.iter().enumerate() {
        let mint_url = mint_url.clone();
        println!("{i}: {mint_url}");
        println!("|   Amount | Unit | Secret                                                           | DLEQ proof included");
        println!("|----------|------|------------------------------------------------------------------|--------------------");
        for proof in &proofs.0 {
            println!(
                "| {:8} | {:4} | {:64} | {}",
                proof.amount,
                proofs.1,
                proof.secret,
                proof.dleq.is_some()
            );
        }
        println!();
        proofs_vec.push((mint_url, proofs.clone()))
    }
    Ok(proofs_vec)
}
