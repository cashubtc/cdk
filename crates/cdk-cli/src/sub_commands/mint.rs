use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use cdk::amount::SplitTarget;
use cdk::mint_url::MintUrl;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::{Wallet, WalletRepository, WalletSubscription};
use cdk::{Amount, StreamExt};
use cdk_common::nut00::KnownMethod;
use cdk_common::payjoin::{format_bip21_amount_from_sats, payjoin_v2_to_bip77_endpoint};
use cdk_common::NotificationPayload;
use clap::Args;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::timeout;

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
    /// Wait duration in seconds with no mint progress before giving up
    /// (0 = wait indefinitely; use this for onchain or slow payments)
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
                    "Quote: id={}, amount={}, expiry={}",
                    quote.id,
                    quote.amount.map_or("none".to_string(), |a| a.to_string()),
                    quote.expiry
                );

                println!("Please pay: {}", quote.request);

                quote
            }
            PaymentMethod::Known(KnownMethod::Onchain) => {
                let amount = sub_command_args.amount;
                let quote = wallet
                    .mint_quote(payment_method.clone(), amount.map(|a| a.into()), None, None)
                    .await?;

                println!("Quote: id={}, expiry={}", quote.id, quote.expiry);
                println!("Send sats to: {}", quote.request);
                if let Some(payjoin) = quote.payjoin.as_ref() {
                    let endpoint = payjoin_v2_to_bip77_endpoint(payjoin)?;
                    println!(
                        "Payjoin: {}",
                        build_payjoin_payment_uri(&quote.request, amount, &endpoint)
                    );
                }

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
                    "Quote: id={}, amount={}, expiry={}",
                    quote.id,
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

    // Spawn a background task that prints progress updates from the mint's
    // quote-state subscription while we wait for the `proof_stream` to yield
    // issued proofs. This is especially useful for onchain mints where block
    // confirmations can take many minutes and the user would otherwise see
    // nothing between printing the address and the first mint batch.
    let stop_progress = Arc::new(Notify::new());
    let progress_handle = spawn_progress_task(
        wallet.clone(),
        quote.id.clone(),
        payment_method.clone(),
        stop_progress.clone(),
    )
    .await;

    let mut amount_minted = Amount::ZERO;

    let mut proof_streams = wallet.proof_stream(quote, SplitTarget::default(), None);

    // `wait_duration` is an *idle* timeout: it bounds how long we wait without
    // any new proofs being issued, and resets each time a batch arrives. A
    // value of 0 disables it entirely (wait indefinitely), which is the right
    // choice for onchain or otherwise slow payments. We never `bail!` from
    // inside the loop so that the progress task below is always stopped cleanly.
    let wait_duration = sub_command_args.wait_duration;
    let mut timed_out = false;

    loop {
        let next = proof_streams.next();
        let event = if wait_duration == 0 {
            Ok(next.await)
        } else {
            timeout(Duration::from_secs(wait_duration), next).await
        };

        match event {
            // A batch of proofs was issued.
            Ok(Some(Ok(proofs))) => {
                let batch = proofs.total_amount()?;
                amount_minted += batch;
                println!("Minted {batch} {unit} (total: {amount_minted} {unit})");
            }
            // The stream surfaced an error.
            Ok(Some(Err(err))) => {
                tracing::error!("Proof streams ended with {:?}", err);
                break;
            }
            // The stream ended normally.
            Ok(None) => break,
            // No proofs arrived within `wait_duration`.
            Err(_elapsed) => {
                tracing::warn!("Timed out after {wait_duration}s waiting for mint proofs");
                println!("Timed out after {wait_duration}s waiting for the mint quote to be paid");
                timed_out = true;
                break;
            }
        }
    }

    // Stop and await the progress task so the CLI exits cleanly.
    stop_progress.notify_waiters();
    if let Some(handle) = progress_handle {
        let _ = handle.await;
    }

    if timed_out && amount_minted == Amount::ZERO {
        bail!("Timed out after {wait_duration}s waiting for the mint quote to be paid");
    }

    println!("Received {amount_minted} from mint {mint_url}");

    Ok(())
}

fn build_payjoin_payment_uri(
    address: &str,
    amount_sat: Option<u64>,
    bip77_endpoint: &str,
) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(amount_sat) = amount_sat {
        serializer.append_pair("amount", &format_bip21_amount_from_sats(amount_sat));
    }
    serializer.append_pair("pjos", "0");
    serializer.append_pair("pj", bip77_endpoint);

    format!(
        "BITCOIN:{}?{}",
        uppercase_qr_address(address),
        serializer.finish()
    )
}

fn uppercase_qr_address(address: &str) -> String {
    let lowercase = address.to_ascii_lowercase();
    if lowercase.starts_with("bc1")
        || lowercase.starts_with("tb1")
        || lowercase.starts_with("bcrt1")
    {
        return address.to_ascii_uppercase();
    }

    address.to_string()
}

/// Spawns a background task that prints human-readable progress updates for
/// the given mint quote. Returns `None` if the subscription could not be
/// created (e.g. for unsupported payment methods); in that case the main flow
/// continues silently as before.
async fn spawn_progress_task(
    wallet: Wallet,
    quote_id: String,
    payment_method: PaymentMethod,
    stop: Arc<Notify>,
) -> Option<tokio::task::JoinHandle<()>> {
    let subscription_filter = match payment_method {
        PaymentMethod::Known(KnownMethod::Bolt11) => {
            WalletSubscription::Bolt11MintQuoteState(vec![quote_id.clone()])
        }
        PaymentMethod::Known(KnownMethod::Bolt12) => {
            WalletSubscription::Bolt12MintQuoteState(vec![quote_id.clone()])
        }
        PaymentMethod::Known(KnownMethod::Onchain) => {
            WalletSubscription::MintQuoteOnchainState(vec![quote_id.clone()])
        }
        _ => return None,
    };

    let mut subscription = match wallet.subscribe(subscription_filter).await {
        Ok(sub) => sub,
        Err(err) => {
            tracing::warn!("Failed to subscribe to mint quote updates: {}", err);
            return None;
        }
    };

    Some(tokio::spawn(async move {
        // Track the last values we printed so we don't spam duplicates.
        let mut last_state: Option<String> = None;
        let mut last_amount_paid: Option<Amount> = None;
        let mut last_amount_issued: Option<Amount> = None;

        loop {
            tokio::select! {
                biased;
                _ = stop.notified() => break,
                maybe_event = subscription.recv() => {
                    let Some(event) = maybe_event else { break };
                    match event.into_inner() {
                        NotificationPayload::MintQuoteBolt11Response(info)
                            if info.quote == quote_id =>
                        {
                            let state_str = info.state.to_string();
                            if last_state.as_deref() != Some(state_str.as_str()) {
                                println!("Quote state: {}", state_str);
                                last_state = Some(state_str);
                            }
                        }
                        NotificationPayload::MintQuoteBolt12Response(info)
                            if info.quote == quote_id
                                && (last_amount_paid != Some(info.amount_paid)
                                    || last_amount_issued != Some(info.amount_issued)) =>
                        {
                            println!(
                                "Payment observed: amount_paid={}, amount_issued={}",
                                info.amount_paid, info.amount_issued
                            );
                            last_amount_paid = Some(info.amount_paid);
                            last_amount_issued = Some(info.amount_issued);
                        }
                        NotificationPayload::MintQuoteOnchainResponse(info)
                            if info.quote == quote_id
                                && (last_amount_paid != Some(info.amount_paid)
                                    || last_amount_issued != Some(info.amount_issued)) =>
                        {
                            if info.amount_paid == Amount::ZERO {
                                println!("Waiting for onchain payment...");
                            } else if info.amount_paid > info.amount_issued {
                                println!(
                                    "Payment confirmed: amount_paid={}, amount_issued={} (minting...)",
                                    info.amount_paid, info.amount_issued
                                );
                            } else {
                                println!(
                                    "Payment observed: amount_paid={}, amount_issued={}",
                                    info.amount_paid, info.amount_issued
                                );
                            }
                            last_amount_paid = Some(info.amount_paid);
                            last_amount_issued = Some(info.amount_issued);
                        }
                        _ => {}
                    }
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use cdk::nuts::nut31::PayjoinV2;
    use cdk_common::payjoin::payjoin_v2_from_bip77_endpoint;

    use super::*;

    #[test]
    fn payjoin_payment_uri_includes_bip77_pj_endpoint() {
        let payjoin = PayjoinV2::new(
            "https://payjo.in/ZGSLFXFUN7K72".to_string(),
            "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
            "Q2JJNCP7QRUVGUM64VHNMWFHLHH9NNF0NC29HUJKCDH3WNLNZCSEZ",
            1_720_547_781,
        )
        .expect("valid Payjoin keys");
        let endpoint = payjoin_v2_to_bip77_endpoint(&payjoin).expect("valid Payjoin v2 endpoint");
        let parsed = payjoin_v2_from_bip77_endpoint(&endpoint).expect("valid BIP77 endpoint");
        assert_eq!(parsed, payjoin);

        let uri = build_payjoin_payment_uri(
            "bcrt1q6d3a2w975yny0asuvd9a67ner4nks58ff0q8g4",
            Some(50_000),
            &endpoint,
        );

        assert!(uri.starts_with("BITCOIN:BCRT1Q6D3A2W975YNY0ASUVD9A67NER4NKS58FF0Q8G4?"));
        let query = uri
            .split_once('?')
            .map(|(_, query)| query)
            .expect("Payjoin payment URI must include query params");
        let params: std::collections::HashMap<_, _> = url::form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();

        assert_eq!(params.get("amount").map(String::as_str), Some("0.0005"));
        assert_eq!(params.get("pjos").map(String::as_str), Some("0"));
        let pj = params.get("pj").expect("pj param must be present");
        assert_eq!(pj, &endpoint);
        assert!(pj.contains("EX1C4UC6ES"));
        assert!(pj.contains("OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ"));
        assert!(pj.contains("RK1Q2JJNCP7QRUVGUM64VHNMWFHLHH9NNF0NC29HUJKCDH3WNLNZCSEZ"));
    }

    #[test]
    fn payjoin_payment_uri_allows_amountless_quote() {
        let uri = build_payjoin_payment_uri(
            "12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX",
            None,
            "HTTPS://EXAMPLE.COM/#OH1QYP",
        );

        assert_eq!(
            uri,
            "BITCOIN:12c6DSiU4Rq3P4ZxziKxzrL5LmMBrzjrJX?pjos=0&pj=HTTPS%3A%2F%2FEXAMPLE.COM%2F%23OH1QYP"
        );
    }
}
