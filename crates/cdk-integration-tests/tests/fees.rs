//! Test calc fee

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::Wallet;
use cdk_integration_tests::{create_backends_fake_wallet, start_mint, wallet_mint, MINT_URL};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_mint_fee() -> Result<()> {
    tokio::spawn(async move {
        let ln_backends = create_backends_fake_wallet();

        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::Sat, (1, 32));

        start_mint(ln_backends, supported_units)
            .await
            .expect("Could not start mint")
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let mnemonic = Mnemonic::generate(12)?;

    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &mnemonic.to_seed_normalized(""),
        None,
    )?;

    let wallet = Arc::new(wallet);

    wallet_mint(
        Arc::clone(&wallet),
        10000.into(),
        SplitTarget::Value(1.into()),
    )
    .await
    .unwrap();
    println!("Minted");

    let proofs = wallet
        .localstore
        .get_proofs(Some(MintUrl::from_str(MINT_URL)?), None, None, None)
        .await?;

    let proofs: Vec<cdk::nuts::Proof> = proofs.into_iter().map(|p| p.proof).collect();

    let five_proofs = proofs[..5].to_vec();

    let fee = wallet.get_proofs_fee(&five_proofs).await?;

    // Check wallet gets fee calc correct
    assert_eq!(fee, 1.into());

    let _swap = wallet
        .swap(None, SplitTarget::Value(1.into()), five_proofs, None, false)
        .await?;

    let wallet_bal = wallet.total_balance().await?;

    // Check 1 sat was paid in fees for the swap
    assert_eq!(wallet_bal, 9999.into());

    let proofs = wallet
        .localstore
        .get_proofs(Some(MintUrl::from_str(MINT_URL)?), None, None, None)
        .await?;

    let proofs: Vec<cdk::nuts::Proof> = proofs.into_iter().map(|p| p.proof).collect();

    let thousand_proofs = proofs[..1001].to_vec();

    let fee = wallet.get_proofs_fee(&thousand_proofs).await?;

    // Check wallet gets fee calc correct
    assert_eq!(fee, 2.into());

    let _swap = wallet
        .swap(
            None,
            SplitTarget::Value(1.into()),
            thousand_proofs,
            None,
            false,
        )
        .await?;

    let wallet_bal = wallet.total_balance().await?;

    // Check 1 sat was paid in fees for the swap
    assert_eq!(wallet_bal, 9997.into());

    Ok(())
}
