use std::str::FromStr;
use std::sync::Arc;

use anyhow::{bail, Result};
use cdk::amount::{Amount, SplitTarget};
use cdk::dhke::construct_proofs;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::nut17::Params;
use cdk::nuts::{
    CurrencyUnit, Id, KeySet, MintBolt11Request, MintQuoteBolt11Request, MintQuoteState,
    NotificationPayload, PreMintSecrets, Proofs, State,
};
use cdk::wallet::client::{HttpClient, MintConnector};
use cdk::wallet::subscription::SubscriptionManager;
use cdk::wallet::WalletSubscription;
use cdk::Wallet;
use tokio::time::{timeout, Duration};

pub mod init_fake_wallet;
pub mod init_mint;
pub mod init_pure_tests;
pub mod init_regtest;

pub async fn wallet_mint(
    wallet: Arc<Wallet>,
    amount: Amount,
    split_target: SplitTarget,
    description: Option<String>,
) -> Result<()> {
    let quote = wallet.mint_quote(amount, description).await?;

    let mut subscription = wallet
        .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
            .id
            .clone()]))
        .await;

    while let Some(msg) = subscription.recv().await {
        if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
            if response.state == MintQuoteState::Paid {
                break;
            }
        }
    }

    let proofs = wallet.mint(&quote.id, split_target, None).await?;

    let receive_amount = proofs.total_amount()?;

    println!("Minted: {}", receive_amount);

    Ok(())
}

pub async fn mint_proofs(
    mint_url: &str,
    amount: Amount,
    keyset_id: Id,
    mint_keys: &KeySet,
    description: Option<String>,
) -> anyhow::Result<Proofs> {
    println!("Minting for ecash");
    println!();

    let wallet_client = HttpClient::new(MintUrl::from_str(mint_url)?);

    let request = MintQuoteBolt11Request {
        amount,
        unit: CurrencyUnit::Sat,
        description,
        pubkey: None,
    };

    let mint_quote = wallet_client.post_mint_quote(request).await?;

    println!("Please pay: {}", mint_quote.request);

    let subscription_client = SubscriptionManager::new(Arc::new(wallet_client.clone()));

    let mut subscription = subscription_client
        .subscribe(
            mint_url.parse()?,
            Params {
                filters: vec![mint_quote.quote.clone()],
                kind: cdk::nuts::nut17::Kind::Bolt11MintQuote,
                id: "sub".into(),
            },
        )
        .await;

    while let Some(msg) = subscription.recv().await {
        if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
            if response.state == MintQuoteState::Paid {
                break;
            }
        }
    }

    let premint_secrets = PreMintSecrets::random(keyset_id, amount, &SplitTarget::default())?;

    let request = MintBolt11Request {
        quote: mint_quote.quote,
        outputs: premint_secrets.blinded_messages(),
        signature: None,
    };

    let mint_response = wallet_client.post_mint(request).await?;

    let pre_swap_proofs = construct_proofs(
        mint_response.signatures,
        premint_secrets.rs(),
        premint_secrets.secrets(),
        &mint_keys.clone().keys,
    )?;

    Ok(pre_swap_proofs)
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

    // Wait for either the payment to complete or timeout
    match timeout(Duration::from_secs(timeout_secs), wait_future).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("Timeout waiting for mint quote to be paid")),
    }
}
