use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::PaymentMethod;
use cdk::wallet::MultiMintWallet;
use cdk::{Amount, StreamExt};
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
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &MintSubCommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let description: Option<String> = sub_command_args.description.clone();

    let wallet = get_or_create_wallet(multi_mint_wallet, &mint_url).await?;

    let payment_method = PaymentMethod::from_str(&sub_command_args.method)?;

    let quote = match &sub_command_args.quote_id {
        None => match payment_method {
            PaymentMethod::Bolt11 => {
                let amount = sub_command_args
                    .amount
                    .ok_or(anyhow!("Amount must be defined"))?;
                let quote = wallet.mint_quote(Amount::from(amount), description).await?;

                println!("Quote: {quote:#?}");

                println!("Please pay: {}", quote.request);

                quote
            }
            PaymentMethod::Bolt12 => {
                let amount = sub_command_args.amount;
                println!("{:?}", sub_command_args.single_use);
                let quote = wallet
                    .mint_bolt12_quote(amount.map(|a| a.into()), description)
                    .await?;

                println!("Quote: {quote:#?}");

                println!("Please pay: {}", quote.request);

                quote
            }
            _ => {
                todo!()
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
