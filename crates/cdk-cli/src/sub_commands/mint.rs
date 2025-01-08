use std::str::FromStr;
use std::sync::Arc;

use anyhow::Result;
use cdk::amount::SplitTarget;
use cdk::cdk_database::{Error, WalletDatabase};
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload};
use cdk::wallet::types::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet, WalletSubscription};
use cdk::Amount;
use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Args, Serialize, Deserialize)]
pub struct MintSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Amount
    amount: u64,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Quote description
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

pub async fn mint(
    multi_mint_wallet: &MultiMintWallet,
    seed: &[u8],
    localstore: Arc<dyn WalletDatabase<Err = Error> + Sync + Send>,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let description: Option<String> = sub_command_args.description.clone();

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), unit.clone()))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            let wallet = Wallet::new(&mint_url.to_string(), unit, localstore, seed, None)?;

            multi_mint_wallet.add_wallet(wallet.clone()).await;
            wallet
        }
    };

    let quote = wallet
        .mint_quote(Amount::from(sub_command_args.amount), description)
        .await?;

    println!("Quote: {:#?}", quote);

    println!("Please pay: {}", quote.request);

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

    let proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;

    let receive_amount = proofs.total_amount()?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
