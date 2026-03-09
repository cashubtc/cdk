use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::WalletRepository;
use clap::Args;
use tokio::time::sleep;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct MintBatchSubCommand {
    /// Mint URL
    mint_url: MintUrl,
    /// Quote IDs to mint in a single batch operation
    #[arg(long, required = true, action = clap::ArgAction::Append)]
    quote_id: Vec<String>,
    /// Wait duration in seconds for batch quote polling
    #[arg(long, default_value = "30")]
    wait_duration: u64,
}

pub async fn mint_batch(
    wallet_repository: &WalletRepository,
    sub_command_args: &MintBatchSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    if sub_command_args.quote_id.is_empty() {
        bail!("At least one --quote-id is required");
    }

    let mut seen_quote_ids = HashSet::new();
    for quote_id in &sub_command_args.quote_id {
        if !seen_quote_ids.insert(quote_id.clone()) {
            return Err(anyhow!("Duplicate quote id: {quote_id}"));
        }
    }

    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    let quote_ids: Vec<&str> = sub_command_args
        .quote_id
        .iter()
        .map(String::as_str)
        .collect();

    println!("Waiting for all batch quotes to be PAID...");
    let deadline = Instant::now() + Duration::from_secs(sub_command_args.wait_duration);

    loop {
        let statuses = wallet.batch_check_mint_quote_status(&quote_ids).await?;

        if statuses
            .iter()
            .any(|quote| matches!(quote.state, MintQuoteState::Issued))
        {
            bail!("One or more quotes are already ISSUED and cannot be batch minted");
        }

        if statuses
            .iter()
            .all(|quote| matches!(quote.state, MintQuoteState::Paid))
        {
            break;
        }

        if Instant::now() >= deadline {
            let pending_quotes = statuses
                .iter()
                .filter(|quote| !matches!(quote.state, MintQuoteState::Paid))
                .map(|quote| format!("{}:{}", quote.id, quote.state))
                .collect::<Vec<_>>()
                .join(", ");

            bail!(
                "Timed out waiting for paid quotes. Remaining: {}",
                pending_quotes
            );
        }

        sleep(Duration::from_millis(500)).await;
    }

    let proofs = wallet
        .batch_mint(&quote_ids, SplitTarget::default(), None, None)
        .await?;

    println!(
        "Batch mint complete: received {} from mint {} in {} proofs",
        proofs.total_amount()?,
        mint_url,
        proofs.len()
    );

    Ok(())
}
