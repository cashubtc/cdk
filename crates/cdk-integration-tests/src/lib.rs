use std::sync::Arc;

use anyhow::{bail, Result};
use cdk::amount::{Amount, SplitTarget};
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{MintQuoteState, NotificationPayload, State};
use cdk::wallet::WalletSubscription;
use cdk::Wallet;
use tokio::time::{sleep, timeout, Duration};

pub mod init_pure_tests;
pub mod init_regtest;

pub async fn wallet_mint(
    wallet: Arc<Wallet>,
    amount: Amount,
    split_target: SplitTarget,
    description: Option<String>,
) -> Result<()> {
    let quote = wallet.mint_quote(amount, description).await?;

    wait_for_mint_to_be_paid(&wallet, &quote.id, 60).await?;

    let proofs = wallet.mint(&quote.id, split_target, None).await?;

    let receive_amount = proofs.total_amount()?;

    println!("Minted: {}", receive_amount);

    Ok(())
}

// Get all pending from wallet and attempt to swap
// Will panic if there are no pending
// Will return Ok if swap fails as expected
pub async fn attempt_to_swap_pending(wallet: &Wallet) -> Result<()> {
    let pending = wallet
        .localstore
        .get_proofs(None, None, Some(vec![State::Pending]), None)
        .await?;

    assert!(!pending.is_empty());

    let swap = wallet
        .swap(
            None,
            SplitTarget::None,
            pending.into_iter().map(|p| p.proof).collect(),
            None,
            false,
        )
        .await;

    match swap {
        Ok(_swap) => {
            bail!("These proofs should be pending")
        }
        Err(err) => match err {
            cdk::error::Error::TokenPending => (),
            _ => {
                println!("{:?}", err);
                bail!("Wrong error")
            }
        },
    }

    Ok(())
}

pub async fn wait_for_mint_to_be_paid(
    wallet: &Wallet,
    mint_quote_id: &str,
    timeout_secs: u64,
) -> Result<()> {
    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![
            mint_quote_id.to_owned(),
        ]))
        .await;
    // Create the timeout future
    let wait_future = async {
        while let Some(msg) = subscription.recv().await {
            if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
                if response.state == MintQuoteState::Paid {
                    return Ok(());
                }
            }
        }
        Ok(())
    };

    let timeout_future = timeout(Duration::from_secs(timeout_secs), wait_future);

    let check_interval = Duration::from_secs(5);

    let periodic_task = async {
        loop {
            match wallet.mint_quote_state(mint_quote_id).await {
                Ok(result) => {
                    if result.state == MintQuoteState::Paid {
                        tracing::info!("mint quote paid via poll");
                        return Ok(());
                    }
                }
                Err(e) => {
                    tracing::error!("Could not check mint quote status: {:?}", e);
                }
            }
            sleep(check_interval).await;
        }
    };

    tokio::select! {
        result = timeout_future => {
            match result {
                Ok(payment_result) => payment_result,
                Err(_) => Err(anyhow::anyhow!("Timeout waiting for mint quote to be paid")),
            }
        }
        result = periodic_task => {
            result // Now propagates the result from periodic checks
        }
    }
}
