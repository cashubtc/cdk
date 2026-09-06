use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::advanced::{MintBatchClaimRequest, MintBatchRefreshRequest};
use cdk::wallet::mint::{MintQuoteId, MintState};
use cdk::wallet::WalletManager;
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
    wallet_manager: &WalletManager,
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

    let wallet = get_or_create_wallet(wallet_manager, &mint_url, unit).await?;

    let quote_ids: Vec<MintQuoteId> = sub_command_args
        .quote_id
        .iter()
        .cloned()
        .map(MintQuoteId::new)
        .collect();

    println!("Waiting for all batch quotes to be PAID...");
    let deadline = Instant::now() + Duration::from_secs(sub_command_args.wait_duration);

    loop {
        let statuses = wallet
            .advanced()
            .refresh_mint_batch(MintBatchRefreshRequest {
                quote_ids: quote_ids.clone(),
            })
            .await?;

        if statuses
            .iter()
            .any(|quote| quote.state == MintState::Issued)
        {
            bail!("One or more quotes are already ISSUED and cannot be batch minted");
        }

        if statuses.iter().all(|quote| quote.state == MintState::Paid) {
            break;
        }

        if Instant::now() >= deadline {
            let pending_quotes = statuses
                .iter()
                .filter(|quote| quote.state != MintState::Paid)
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

    let receipt = wallet
        .advanced()
        .claim_mint_batch(MintBatchClaimRequest {
            quote_ids,
            amount_split_target: SplitTarget::default(),
            conditions: None,
            external_keys: HashMap::new(),
        })
        .await?;

    println!(
        "Batch mint complete: received {} from mint {} in {} proofs",
        receipt.amount,
        mint_url,
        receipt.proofs.len()
    );

    Ok(())
}
