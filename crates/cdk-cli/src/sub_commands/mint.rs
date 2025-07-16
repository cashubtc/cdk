use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload, PaymentMethod};
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
    /// Payment method
    #[arg(long, default_value = "bolt11")]
    method: String,
    /// Expiry
    #[arg(short, long)]
    expiry: Option<u64>,
    /// Expiry
    #[arg(short, long)]
    single_use: Option<bool>,
}

pub async fn mint(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let description: Option<String> = sub_command_args.description.clone();

    let wallet = get_or_create_wallet(multi_mint_wallet, &mint_url, unit).await?;

    let mut payment_method = PaymentMethod::from_str(&sub_command_args.method)?;

    let quote_id = match &sub_command_args.quote_id {
        None => match payment_method {
            PaymentMethod::Bolt11 => {
                let amount = sub_command_args
                    .amount
                    .ok_or(anyhow!("Amount must be defined"))?;
                let quote = wallet.mint_quote(Amount::from(amount), description).await?;

                println!("Quote: {quote:#?}");

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
            PaymentMethod::Bolt12 => {
                let amount = sub_command_args.amount;
                println!("{:?}", sub_command_args.single_use);
                let quote = wallet
                    .mint_bolt12_quote(amount.map(|a| a.into()), description)
                    .await?;

                println!("Quote: {quote:#?}");

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
            _ => {
                todo!()
            }
        },
        Some(quote_id) => {
            let quote = wallet
                .localstore
                .get_mint_quote(quote_id)
                .await?
                .ok_or(anyhow!("Unknown quote"))?;

            payment_method = quote.payment_method;
            quote_id.to_string()
        }
    };

    tracing::debug!("Attempting mint for: {}", payment_method);

    let proofs = match payment_method {
        PaymentMethod::Bolt11 => wallet.mint(&quote_id, SplitTarget::default(), None).await?,
        PaymentMethod::Bolt12 => {
            let response = wallet.mint_bolt12_quote_state(&quote_id).await?;

            let amount_mintable = response.amount_paid - response.amount_issued;

            if amount_mintable == Amount::ZERO {
                println!("Mint quote does not have amount that can be minted.");
                return Ok(());
            }

            wallet
                .mint_bolt12(
                    &quote_id,
                    Some(amount_mintable),
                    SplitTarget::default(),
                    None,
                )
                .await?
        }
        _ => {
            todo!()
        }
    };

    let receive_amount = proofs.total_amount()?;

    println!("Received {receive_amount} from mint {mint_url}");

    Ok(())
}
