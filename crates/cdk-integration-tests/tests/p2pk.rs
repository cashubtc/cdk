use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bip39::Mnemonic;
use cdk::amount::SplitTarget;
use cdk::cdk_database::WalletMemoryDatabase;
use cdk::nuts::{CurrencyUnit, SecretKey, SpendingConditions};
use cdk::wallet::SendKind;
use cdk::{Amount, Wallet};
use cdk_integration_tests::{create_backends_fake_wallet, start_mint, wallet_mint, MINT_URL};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
pub async fn test_p2pk_swap() -> Result<()> {
    tokio::spawn(async move {
        let ln_backends = create_backends_fake_wallet();

        start_mint(ln_backends).await.expect("Could not start mint")
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mnemonic = Mnemonic::generate(12)?;

    let wallet = Wallet::new(
        MINT_URL,
        CurrencyUnit::Sat,
        Arc::new(WalletMemoryDatabase::default()),
        &mnemonic.to_seed_normalized(""),
        None,
    );

    let wallet = Arc::new(wallet);

    // Mint 100 sats for the wallet
    wallet_mint(Arc::clone(&wallet), 100.into()).await?;

    let secret = SecretKey::generate();

    let spending_conditions = SpendingConditions::new_p2pk(secret.public_key(), None);

    let amount = Amount::from(10);

    let token = wallet
        .send(
            amount,
            None,
            Some(spending_conditions),
            &SplitTarget::None,
            &SendKind::default(),
            false,
        )
        .await?;

    let attempt_amount = wallet
        .receive(&token.to_string(), SplitTarget::default(), &[], &[])
        .await;

    // This should fail since the token is not signed
    assert!(attempt_amount.is_err());

    let wrong_secret = SecretKey::generate();

    let received_amount = wallet
        .receive(
            &token.to_string(),
            SplitTarget::default(),
            &[wrong_secret],
            &[],
        )
        .await;

    assert!(received_amount.is_err());

    let received_amount = wallet
        .receive(&token.to_string(), SplitTarget::default(), &[secret], &[])
        .await
        .unwrap();

    assert_eq!(received_amount, amount);

    Ok(())
}
