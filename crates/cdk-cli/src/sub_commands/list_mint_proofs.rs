use std::collections::BTreeMap;

use anyhow::Result;
use cdk::{
    mint_url::MintUrl,
    nuts::{CurrencyUnit, Proof},
    wallet::multi_mint_wallet::MultiMintWallet,
};

pub async fn proofs(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    list_proofs(multi_mint_wallet, &CurrencyUnit::Sat).await?;
    Ok(())
}

async fn list_proofs(
    multi_mint_wallet: &MultiMintWallet,
    unit: &CurrencyUnit,
) -> Result<Vec<(MintUrl, Vec<Proof>)>> {
    let wallets_proofs: BTreeMap<MintUrl, Vec<Proof>> = multi_mint_wallet.list_proofs(unit).await?;

    let mut proofs_vec = Vec::with_capacity(wallets_proofs.len());

    for (i, (mint_url, proofs)) in wallets_proofs.iter().enumerate() {
        let mint_url = mint_url.clone();
        println!("{i}: {mint_url} {:#?}", proofs);
        proofs_vec.push((mint_url, proofs.clone()))
    }
    Ok(proofs_vec)
}
