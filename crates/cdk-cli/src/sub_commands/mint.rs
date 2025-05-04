use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload};
use cdk::wallet::{MultiMintWallet, WalletSubscription};
use cdk::Amount;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::utils::get_or_create_wallet;

#[derive(Args, Serialize, Deserialize)]
pub struct MintSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Amount
    amount: Option<u64>,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
    /// Quote description
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Quote Id
    #[arg(short, long)]
    quote_id: Option<String>,
}

pub async fn mint(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let description: Option<String> = sub_command_args.description.clone();

    let wallet = get_or_create_wallet(multi_mint_wallet, &mint_url, unit).await?;

    let quote_id = match &sub_command_args.quote_id {
        None => {
            let amount = sub_command_args
                .amount
                .ok_or(anyhow!("Amount must be defined"))?;
            let quote = wallet.mint_quote(Amount::from(amount), description).await?;

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
            quote.id
        }
        Some(quote_id) => quote_id.to_string(),
    };

    let proofs = wallet.mint(&quote_id, SplitTarget::default(), None).await?;

    let receive_amount = proofs.total_amount()?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
