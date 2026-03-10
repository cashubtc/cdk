use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::WalletRepository;
use cdk::{Amount, StreamExt};
use cdk_common::nut00::KnownMethod;
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::utils::get_or_create_wallet;

#[derive(Args, Serialize, Deserialize)]
pub struct MintSubCommand {
    /// Mint url
    mint_url: MintUrl,
    /// Amount
    amount: Option<u64>,
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
    /// Wait duration in seconds for mint quote polling
    #[arg(long, default_value = "30")]
    wait_duration: u64,
}

pub async fn mint(
    wallet_repository: &WalletRepository,
    sub_command_args: &MintSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let description: Option<String> = sub_command_args.description.clone();

    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    let payment_method = PaymentMethod::from_str(&sub_command_args.method)?;

    let quote = match &sub_command_args.quote_id {
        None => match payment_method {
            PaymentMethod::Known(KnownMethod::Bolt11) => {
                let amount = sub_command_args
                    .amount
                    .ok_or(anyhow!("Amount must be defined"))?;
                let quote = wallet
                    .mint_quote(
                        PaymentMethod::BOLT11,
                        Some(Amount::from(amount)),
                        description,
                        None,
                    )
                    .await?;

                println!(
                    "Quote: id={}, state={}, amount={}, expiry={}",
                    quote.id,
                    quote.state,
                    quote.amount.map_or("none".to_string(), |a| a.to_string()),
                    quote.expiry
                );

                println!("Please pay: {}", quote.request);

                quote
            }
            PaymentMethod::Known(KnownMethod::Bolt12) => {
                let amount = sub_command_args.amount;
                println!(
                    "Single use: {}",
                    sub_command_args
                        .single_use
                        .map_or("none".to_string(), |b| b.to_string())
                );
                let quote = wallet
                    .mint_quote(
                        payment_method.clone(),
                        amount.map(|a| a.into()),
                        description,
                        None,
                    )
                    .await?;

                println!(
                    "Quote: id={}, state={}, amount={}, expiry={}",
                    quote.id,
                    quote.state,
                    quote.amount.map_or("none".to_string(), |a| a.to_string()),
                    quote.expiry
                );

                println!("Please pay: {}", quote.request);

                quote
            }
            _ => {
                let amount = sub_command_args.amount;
                println!(
                    "Single use: {}",
                    sub_command_args
                        .single_use
                        .map_or("none".to_string(), |b| b.to_string())
                );
                let quote = wallet
                    .mint_quote(payment_method.clone(), amount.map(|a| a.into()), None, None)
                    .await?;

                println!(
                    "Quote: id={}, state={}, amount={}, expiry={}",
                    quote.id,
                    quote.state,
                    quote.amount.map_or("none".to_string(), |a| a.to_string()),
                    quote.expiry
                );

                println!("Please pay: {}", quote.request);

                quote
            }
        },
        Some(quote_id) => wallet
            .localstore
            .get_mint_quote(quote_id)
            .await?
            .ok_or(anyhow!("Unknown quote"))?,
    };

    tracing::debug!("Attempting mint for: {}", payment_method);

    let mut amount_minted = Amount::ZERO;

    let mut proof_streams = wallet.proof_stream(quote, SplitTarget::default(), None);

    while let Some(proofs) = proof_streams.next().await {
        let proofs = match proofs {
            Ok(proofs) => proofs,
            Err(err) => {
                tracing::error!("Proof streams ended with {:?}", err);
                break;
            }
        };
        amount_minted += proofs.total_amount()?;
    }

    println!("Received {amount_minted} from mint {mint_url}");

    Ok(())
}
