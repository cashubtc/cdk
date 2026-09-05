use std::time::Duration;

use bdk_electrum::electrum_client::{Client, ConfigBuilder, ElectrumApi};
use bdk_electrum::BdkElectrumClient;
use bdk_wallet::bitcoin::Transaction;

use crate::chain::{BroadcastErrorKind, BroadcastFailure, BroadcastOutcome, ElectrumConfig};
use crate::error::Error;
use crate::BdkWalletEngine;

const ELECTRUM_TIMEOUT_SECS: u8 = 10;

type ElectrumClient = BdkElectrumClient<Client>;

fn new_electrum_client(url: &str) -> Result<ElectrumClient, bdk_electrum::electrum_client::Error> {
    let client_config = ConfigBuilder::new()
        .timeout(Some(Duration::from_secs(u64::from(ELECTRUM_TIMEOUT_SECS))))
        .build();
    let client = Client::from_config(url, client_config)?;
    Ok(BdkElectrumClient::new(client))
}

pub(crate) async fn sync_electrum_once(
    engine: &BdkWalletEngine,
    config: &ElectrumConfig,
) -> Result<(), Error> {
    let url = config.url.clone();
    let client = tokio::task::spawn_blocking(move || new_electrum_client(&url))
        .await
        .map_err(|error| Error::Electrum(error.to_string()))?
        .map_err(|error| Error::Electrum(error.to_string()))?;
    let request = {
        let wallet = engine.wallet_with_db.lock().await;
        client.populate_tx_cache(wallet.wallet.tx_graph().full_txs().map(|node| node.tx));
        wallet.wallet.start_sync_with_revealed_spks()
    };
    let batch_size = config.batch_size;
    let update = tokio::task::spawn_blocking(move || client.sync(request, batch_size, false))
        .await
        .map_err(|error| Error::Electrum(error.to_string()))?
        .map_err(|error| Error::Electrum(error.to_string()))?;
    let mut wallet = engine.wallet_with_db.lock().await;
    wallet
        .wallet
        .apply_update_events(update)
        .map_err(|error| Error::Wallet(error.to_string()))?;
    wallet.persist()?;
    Ok(())
}

pub(crate) fn classify_electrum_broadcast_error(message: &str) -> BroadcastErrorKind {
    let message = message.to_ascii_lowercase();

    if message.contains("dust")
        || message.contains("min relay")
        || message.contains("minrelay")
        || message.contains("mandatory-script-verify-flag-failed")
        || message.contains("non-mandatory-script-verify-flag")
        || message.contains("bad-txns")
        || message.contains("nonstandard")
        || message.contains("non-standard")
        || message.contains("insufficient fee")
        || message.contains("fee too low")
        || message.contains("mempool min fee")
        || message.contains("missing inputs")
        || message.contains("txn-mempool-conflict")
    {
        return BroadcastErrorKind::Rejected;
    }

    if message.contains("timeout")
        || message.contains("timed out")
        || message.contains("connection")
        || message.contains("connect")
        || message.contains("dns")
        || message.contains("broken pipe")
        || message.contains("refused")
        || message.contains("reset")
        || message.contains("temporarily unavailable")
    {
        return BroadcastErrorKind::Transient;
    }

    BroadcastErrorKind::Unknown
}

fn is_electrum_already_known(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("already in block chain")
        || message.contains("already in blockchain")
        || message.contains("already have transaction")
        || message.contains("txn-already-in-mempool")
        || message.contains("transaction already in mempool")
}

pub(crate) async fn broadcast_electrum(
    config: &ElectrumConfig,
    tx: Transaction,
) -> Result<BroadcastOutcome, BroadcastFailure> {
    let url = config.url.clone();

    tokio::task::spawn_blocking(move || {
        let client = new_electrum_client(&url).map_err(|error| {
            BroadcastFailure::new(BroadcastErrorKind::Transient, error.to_string())
        })?;

        tracing::info!(
            "Broadcasting transaction: {} via Electrum",
            tx.compute_txid()
        );

        match client.transaction_broadcast(&tx) {
            Ok(_) => Ok(BroadcastOutcome::Accepted),
            Err(error) => {
                let message = error.to_string();
                if is_electrum_already_known(&message) {
                    return Ok(BroadcastOutcome::AlreadyKnown);
                }

                Err(BroadcastFailure::new(
                    classify_electrum_broadcast_error(&message),
                    message,
                ))
            }
        }
    })
    .await
    .map_err(|error| {
        BroadcastFailure::new(
            BroadcastErrorKind::Transient,
            format!("Electrum broadcast task failed: {error}"),
        )
    })?
}

fn btc_per_kb_to_sat_per_vb(rate: f64) -> Option<f64> {
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }

    let rate = rate * 100_000.0;
    rate.is_finite().then_some(rate)
}

pub(crate) async fn fetch_fee_rate_electrum(
    config: &ElectrumConfig,
    target_blocks: u16,
) -> Result<f64, Error> {
    let url = config.url.clone();

    tokio::task::spawn_blocking(move || {
        let client =
            new_electrum_client(&url).map_err(|error| Error::Electrum(error.to_string()))?;
        let estimate = client
            .inner
            .estimate_fee(target_blocks as usize, None)
            .map_err(|error| Error::Electrum(error.to_string()))?;

        btc_per_kb_to_sat_per_vb(estimate).ok_or(Error::FeeEstimationUnavailable)
    })
    .await
    .map_err(|error| Error::FeeEstimationFailed(error.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_electrum_broadcast_errors() {
        assert_eq!(
            classify_electrum_broadcast_error("sendrawtransaction RPC error: missing inputs"),
            BroadcastErrorKind::Rejected
        );
        assert_eq!(
            classify_electrum_broadcast_error("connection timeout"),
            BroadcastErrorKind::Transient
        );
        assert_eq!(
            classify_electrum_broadcast_error("unexpected backend response"),
            BroadcastErrorKind::Unknown
        );
    }

    #[test]
    fn detects_already_known_transactions() {
        assert!(is_electrum_already_known("transaction already in mempool"));
        assert!(!is_electrum_already_known("missing inputs"));
    }

    #[test]
    fn converts_electrum_fee_rates() {
        assert_eq!(btc_per_kb_to_sat_per_vb(0.00001), Some(1.0));
        assert_eq!(btc_per_kb_to_sat_per_vb(-1.0), None);
        assert_eq!(btc_per_kb_to_sat_per_vb(0.0), None);
        assert_eq!(btc_per_kb_to_sat_per_vb(f64::NAN), None);
    }
}
