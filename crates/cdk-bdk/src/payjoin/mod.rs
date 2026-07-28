//! Payjoin support for the BDK on-chain backend.

mod cut_through;
mod persistence;
mod receive;
mod send;
mod validation;

use std::collections::HashSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Duration;

use bdk_wallet::bitcoin::{
    consensus, FeeRate, OutPoint, Script, Sequence, Transaction, TxIn, TxOut,
};
use bdk_wallet::KeychainKind;
use cdk_common::nuts::nut31::PayjoinV2;
use cdk_common::payjoin::{build_bip21_payjoin_uri, payjoin_v2_from_bip77_endpoint};
use cdk_common::payment::{Event, MakePaymentResponse, PaymentIdentifier, WaitPaymentResponse};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use self::persistence::RecordingSessionPersister;
use self::validation::{
    find_payment_outpoint, require_payjoin_send_payment_output, validate_payjoin_send_transaction,
};
use crate::error::Error;
use crate::send::batch_transaction::record::BatchOutputAssignment;
use crate::send::payment_intent::{state as intent_state, SendIntent};
use crate::send::staging::{StageableSendIntent, StagedBroadcastOutcome};
use crate::types::{PayjoinConfig, PaymentMetadata, PaymentTier};
use crate::util::parse_checked_address;
use crate::CdkBdk;

const PAYJOIN_RECEIVE_SESSION_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
/// How many sessions/intents a poller tick drives concurrently, so one slow
/// relay does not head-of-line-block every other session.
const PAYJOIN_POLL_CONCURRENCY: usize = 8;
const PAYJOIN_OHTTP_KEYS_CACHE_TTL_SECS: u64 = 10 * 60;
const PAYJOIN_OHTTP_KEYS_FETCH_TIMEOUT: Duration = Duration::from_secs(3);
const PAYJOIN_HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(35);
const PAYJOIN_RECEIVER_MAX_EFFECTIVE_FEE_RATE: FeeRate = FeeRate::ZERO;
/// Minimum fee rate enforced on a sender's original PSBT during the
/// broadcast-suitability check. On backends without `testmempoolaccept` (Esplora)
/// this floor is the primary anti-probing protection; on Bitcoin Core it is an
/// additional constraint on top of the full mempool-acceptance check.
const PAYJOIN_RECEIVER_MIN_ORIGINAL_FEE_RATE: FeeRate = FeeRate::from_sat_per_vb_u32(1);
#[cfg(test)]
const TEST_OHTTP_KEYS: &str = "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ";
#[cfg(test)]
static TEST_OHTTP_FETCH_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static TEST_OHTTP_FETCH_FAIL: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static TEST_OHTTP_FETCH_DELAY_MS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
static TEST_OHTTP_FETCH_CALLS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static TEST_OHTTP_FETCH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub(crate) async fn lock_test_ohttp_fetch() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_OHTTP_FETCH_TEST_LOCK.lock().await
}

#[cfg(test)]
pub(crate) fn configure_test_ohttp_fetch(delay: Duration, fail: bool) {
    TEST_OHTTP_FETCH_ENABLED.store(true, Ordering::SeqCst);
    TEST_OHTTP_FETCH_FAIL.store(fail, Ordering::SeqCst);
    TEST_OHTTP_FETCH_DELAY_MS.store(delay.as_millis() as u64, Ordering::SeqCst);
    TEST_OHTTP_FETCH_CALLS.store(0, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn disable_test_ohttp_fetch() {
    TEST_OHTTP_FETCH_ENABLED.store(false, Ordering::SeqCst);
    TEST_OHTTP_FETCH_FAIL.store(false, Ordering::SeqCst);
    TEST_OHTTP_FETCH_DELAY_MS.store(0, Ordering::SeqCst);
    TEST_OHTTP_FETCH_CALLS.store(0, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn test_ohttp_fetch_calls() -> usize {
    TEST_OHTTP_FETCH_CALLS.load(Ordering::SeqCst)
}

struct PreparedPayjoinSend {
    /// The signed original transaction, broadcastable as the Payjoin fallback.
    original_tx: Transaction,
    original_fee_sat: u64,
    persister: RecordingSessionPersister<::payjoin::send::v2::SessionEvent>,
    planning_guard: tokio::sync::OwnedMutexGuard<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PayjoinSendValidation {
    /// The single receiver-script output used for the melt payment proof.
    payment_outpoint: OutPoint,
    /// The mint wallet's net spend above the quoted receiver amount.
    fee_contribution_sat: u64,
}

struct PayjoinReceiveProposal {
    proposal: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::PayjoinProposal>,
    cut_through: Option<CutThroughReceiveProposal>,
    planning_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

#[derive(Clone)]
struct CutThroughProposal {
    reservation_id: Uuid,
    send_intent_id: Uuid,
    proposal_tx: Transaction,
    original_tx: Transaction,
    receive_outpoint: String,
    melt_outpoint: String,
    fee_contribution_sat: u64,
}

enum CutThroughReceiveProposal {
    Fresh(Box<CutThroughProposal>),
    Exposed,
}

async fn fetch_ohttp_keys_with_timeout(
    config: &PayjoinConfig,
) -> Result<::payjoin::OhttpKeys, Error> {
    tokio::time::timeout(PAYJOIN_OHTTP_KEYS_FETCH_TIMEOUT, fetch_ohttp_keys(config))
        .await
        .map_err(|_| {
            Error::Payjoin(format!(
                "Payjoin OHTTP key fetch timed out after {} seconds",
                PAYJOIN_OHTTP_KEYS_FETCH_TIMEOUT.as_secs()
            ))
        })?
}

async fn fetch_ohttp_keys(config: &PayjoinConfig) -> Result<::payjoin::OhttpKeys, Error> {
    #[cfg(test)]
    if let Some(result) = test_fetch_ohttp_keys(config).await {
        return result;
    }

    #[cfg(feature = "payjoin-local-https")]
    {
        if let Some(cert_der) = config.local_tls_cert_der.clone() {
            return ::payjoin::io::fetch_ohttp_keys_with_cert(
                &config.ohttp_relay_url,
                &config.directory_url,
                &cert_der,
            )
            .await
            .map_err(|err| Error::Payjoin(err.to_string()));
        }
    }

    ::payjoin::io::fetch_ohttp_keys(&config.ohttp_relay_url, &config.directory_url)
        .await
        .map_err(|err| Error::Payjoin(err.to_string()))
}

#[cfg(test)]
async fn test_fetch_ohttp_keys(
    _config: &PayjoinConfig,
) -> Option<Result<::payjoin::OhttpKeys, Error>> {
    if !TEST_OHTTP_FETCH_ENABLED.load(Ordering::SeqCst) {
        return None;
    }

    TEST_OHTTP_FETCH_CALLS.fetch_add(1, Ordering::SeqCst);
    let delay_ms = TEST_OHTTP_FETCH_DELAY_MS.load(Ordering::SeqCst);
    if delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }

    if TEST_OHTTP_FETCH_FAIL.load(Ordering::SeqCst) {
        return Some(Err(Error::Payjoin(
            "test OHTTP key fetch failure".to_string(),
        )));
    }

    let keys = TEST_OHTTP_KEYS
        .parse::<cdk_common::nuts::nut31::PayjoinOhttpKeys>()
        .map_err(|err| Error::Payjoin(err.to_string()))
        .and_then(|keys| {
            ::payjoin::OhttpKeys::try_from(keys.as_bytes().as_slice())
                .map_err(|err| Error::Payjoin(err.to_string()))
        });
    Some(keys)
}

fn payjoin_receive_session_state_name(
    session: &::payjoin::receive::v2::ReceiveSession,
) -> &'static str {
    match session {
        ::payjoin::receive::v2::ReceiveSession::Initialized(_) => "initialized",
        ::payjoin::receive::v2::ReceiveSession::UncheckedOriginalPayload(_) => {
            "unchecked_original_payload"
        }
        ::payjoin::receive::v2::ReceiveSession::MaybeInputsOwned(_) => "maybe_inputs_owned",
        ::payjoin::receive::v2::ReceiveSession::MaybeInputsSeen(_) => "maybe_inputs_seen",
        ::payjoin::receive::v2::ReceiveSession::OutputsUnknown(_) => "outputs_unknown",
        ::payjoin::receive::v2::ReceiveSession::WantsOutputs(_) => "wants_outputs",
        ::payjoin::receive::v2::ReceiveSession::WantsInputs(_) => "wants_inputs",
        ::payjoin::receive::v2::ReceiveSession::WantsFeeRange(_) => "wants_fee_range",
        ::payjoin::receive::v2::ReceiveSession::ProvisionalProposal(_) => "provisional_proposal",
        ::payjoin::receive::v2::ReceiveSession::PayjoinProposal(_) => "payjoin_proposal",
        ::payjoin::receive::v2::ReceiveSession::HasReplyableError(_) => "has_replyable_error",
        ::payjoin::receive::v2::ReceiveSession::Closed(_) => "closed",
        _ => "unknown",
    }
}

fn latest_payjoin_receive_replyable_error(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Option<serde_json::Value> {
    events.iter().rev().find_map(|event| match event {
        ::payjoin::receive::v2::SessionEvent::GotReplyableError(error) => Some(error.to_json()),
        _ => None,
    })
}

fn build_payjoin_uri(address: &str, amount_sat: u64, payjoin: &PayjoinV2) -> Result<String, Error> {
    build_bip21_payjoin_uri(address, Some(amount_sat), payjoin, None)
        .map_err(|err| Error::Payjoin(err.to_string()))
}

fn update_payjoin_receive_credit_cap(record: &mut crate::storage::PayjoinReceiveSessionRecord) {
    if let Some(amount_sat) = payjoin_original_receiver_output_amount_from_events(&record.events) {
        if record.amount_sat == 0 {
            tracing::debug!(
                quote_id = %record.quote_id,
                fallback_address = %record.fallback_address,
                previous_amount_sat = record.amount_sat,
                credit_cap_amount_sat = amount_sat,
                "Updated Payjoin receive credit cap from original PSBT receiver outputs"
            );
            record.amount_sat = amount_sat;
        } else if record.amount_sat != amount_sat {
            tracing::debug!(
                quote_id = %record.quote_id,
                fallback_address = %record.fallback_address,
                quoted_amount_sat = record.amount_sat,
                original_receiver_output_sat = amount_sat,
                "Keeping existing Payjoin receive credit cap"
            );
        }
    }
}

fn update_payjoin_receive_proposal_receiver_outpoints(
    record: &mut crate::storage::PayjoinReceiveSessionRecord,
    psbt: &bdk_wallet::bitcoin::Psbt,
    fallback_script: &Script,
) {
    let txid = psbt.unsigned_tx.compute_txid();
    let outpoints = psbt
        .unsigned_tx
        .output
        .iter()
        .enumerate()
        .filter(|(_, output)| output.script_pubkey.as_script() == fallback_script)
        .map(|(vout, _)| OutPoint::new(txid, vout as u32).to_string())
        .collect::<Vec<_>>();

    if outpoints.is_empty() {
        tracing::warn!(
            quote_id = %record.quote_id,
            fallback_address = %record.fallback_address,
            "Payjoin proposal has no receiver-script outpoints to record"
        );
        return;
    }

    if record.proposal_receiver_outpoints != outpoints {
        tracing::debug!(
            quote_id = %record.quote_id,
            fallback_address = %record.fallback_address,
            proposal_receiver_outpoint_count = outpoints.len(),
            "Updated Payjoin receive proposal receiver outpoints"
        );
        record.proposal_receiver_outpoints = outpoints;
    }
}

fn apply_zero_receiver_fee_range(
    receiver: ::payjoin::receive::v2::Receiver<::payjoin::receive::v2::WantsFeeRange>,
    persister: &RecordingSessionPersister<::payjoin::receive::v2::SessionEvent>,
) -> Result<::payjoin::receive::v2::Receiver<::payjoin::receive::v2::ProvisionalProposal>, Error> {
    receiver
        .apply_fee_range(None, Some(PAYJOIN_RECEIVER_MAX_EFFECTIVE_FEE_RATE))
        .save(persister)
        .map_err(|err| Error::Payjoin(err.to_string()))
}

fn ensure_payjoin_receiver_credit(
    psbt: &bdk_wallet::bitcoin::Psbt,
    fallback_script: &Script,
    minimum_amount_sat: u64,
) -> Result<(), Error> {
    let credited_amount_sat = payjoin_receiver_output_amount(psbt, fallback_script)?;
    if credited_amount_sat < minimum_amount_sat {
        return Err(Error::Payjoin(format!(
            "Payjoin proposal receiver output amount {} is below original amount {}",
            credited_amount_sat, minimum_amount_sat
        )));
    }

    Ok(())
}

fn payjoin_receiver_output_amount(
    psbt: &bdk_wallet::bitcoin::Psbt,
    fallback_script: &Script,
) -> Result<u64, Error> {
    psbt.unsigned_tx
        .output
        .iter()
        .filter(|output| output.script_pubkey.as_script() == fallback_script)
        .try_fold(0_u64, |amount_sat, output| {
            amount_sat
                .checked_add(output.value.to_sat())
                .ok_or_else(|| {
                    Error::Payjoin("Payjoin receiver output amount overflow".to_string())
                })
        })
}

fn payjoin_receiver_output_count_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Option<usize> {
    events.iter().rev().find_map(|event| match event {
        ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vouts) => Some(vouts.len()),
        _ => None,
    })
}

fn ensure_ordinary_payjoin_has_single_receiver_output(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Result<(), Error> {
    match payjoin_receiver_output_count_from_events(events) {
        Some(1) => Ok(()),
        Some(count) => Err(Error::Payjoin(format!(
            "Ordinary Payjoin requires exactly one receiver output, found {}",
            count
        ))),
        None => Err(Error::Payjoin(
            "Payjoin receiver output identification is missing".to_string(),
        )),
    }
}

/// The latest `RetrievedOriginalPayload` event in a receive session's log.
fn latest_original_payload(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Result<&::payjoin::receive::OriginalPayload, Error> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            ::payjoin::receive::v2::SessionEvent::RetrievedOriginalPayload { original, .. } => {
                Some(original)
            }
            _ => None,
        })
        .ok_or_else(|| Error::Payjoin("Payjoin original payload event missing".to_string()))
}

fn payjoin_original_input_outpoints_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Result<Vec<OutPoint>, Error> {
    let original = latest_original_payload(events)?;

    let mut outpoints = Vec::new();
    let mut collect_outpoint = |outpoint: &OutPoint| {
        outpoints.push(*outpoint);
        Ok(false)
    };
    original
        .check_no_inputs_seen_before(&mut collect_outpoint)
        .map_err(|err| Error::Payjoin(err.to_string()))?;

    Ok(outpoints)
}

/// Whether it is safe to drop a closed receive session's persisted credit cap.
///
/// A receiver-signed proposal never expires on-chain, and its receiver output
/// includes the mint's own contributed input value, so the cap in
/// `proposal_receiver_outpoints` must outlive any still-broadcastable proposal
/// — time alone is not sufficient. The cap is resolved once one of the
/// proposal outpoints was detected (the cap was applied when the receive
/// intent was created), or once an original receiver output was detected
/// instead: the proposal spends the same sender inputs as the original, so a
/// settled original means the proposal can never confirm.
async fn payjoin_receive_credit_cap_resolved(
    storage: &crate::storage::BdkStorage,
    network: bdk_wallet::bitcoin::Network,
    record: &crate::storage::PayjoinReceiveSessionRecord,
) -> Result<bool, Error> {
    if record.proposal_receiver_outpoints.is_empty() {
        return Ok(true);
    }
    for outpoint in &record.proposal_receiver_outpoints {
        if storage.has_receive_intent_for_outpoint(outpoint).await? {
            return Ok(true);
        }
    }

    let Ok(original_tx) = payjoin_original_tx_from_events(&record.events) else {
        return Ok(false);
    };
    let Ok(fallback_address) =
        parse_checked_address(&record.fallback_address, network, Error::Payjoin)
    else {
        return Ok(false);
    };
    let fallback_script = fallback_address.script_pubkey();
    let original_txid = original_tx.compute_txid();
    for (vout, output) in original_tx.output.iter().enumerate() {
        if output.script_pubkey != fallback_script {
            continue;
        }
        let outpoint = OutPoint::new(original_txid, vout as u32).to_string();
        if storage.has_receive_intent_for_outpoint(&outpoint).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn payjoin_original_tx_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Result<Transaction, Error> {
    let original = latest_original_payload(events)?;

    let original_tx = StdMutex::new(None);
    original
        .check_broadcast_suitability(None, |tx| {
            *original_tx.lock().map_err(|err| {
                ::payjoin::ImplementationError::new(std::io::Error::other(err.to_string()))
            })? = Some(tx.clone());
            Ok(true)
        })
        .map_err(|err| Error::Payjoin(err.to_string()))?;

    original_tx
        .into_inner()
        .map_err(|err| Error::Payjoin(format!("Payjoin original tx lock poisoned: {}", err)))?
        .ok_or_else(|| Error::Payjoin("Payjoin original tx missing".to_string()))
}

fn payjoin_original_receiver_output_amount_from_events(
    events: &[::payjoin::receive::v2::SessionEvent],
) -> Option<u64> {
    let mut receiver_vouts = None;
    let mut committed_outputs = None;

    for event in events {
        match event {
            ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vouts) => {
                receiver_vouts = Some(vouts.as_slice());
            }
            ::payjoin::receive::v2::SessionEvent::CommittedOutputs(outputs) => {
                committed_outputs = Some(outputs.as_slice());
            }
            _ => {}
        }
    }

    let receiver_vouts = receiver_vouts?;
    let committed_outputs = committed_outputs?;

    receiver_vouts.iter().try_fold(0_u64, |amount_sat, vout| {
        let output = committed_outputs.get(*vout)?;
        amount_sat.checked_add(output.value.to_sat())
    })
}

/// Shared HTTP client for directory/relay requests so connections are pooled
/// across polls instead of paying a TCP+TLS handshake per request.
static PAYJOIN_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

async fn payjoin_http_request(request: ::payjoin::Request) -> Result<Vec<u8>, Error> {
    let response = tokio::time::timeout(PAYJOIN_HTTP_REQUEST_TIMEOUT, async {
        PAYJOIN_HTTP_CLIENT
            .post(request.url)
            .header(reqwest::header::CONTENT_TYPE, request.content_type)
            .body(request.body)
            .send()
            .await
            .map_err(|err| Error::Payjoin(err.to_string()))
    })
    .await
    .map_err(|_| {
        Error::Payjoin(format!(
            "Payjoin HTTP request timed out after {} seconds",
            PAYJOIN_HTTP_REQUEST_TIMEOUT.as_secs()
        ))
    })??;
    if !response.status().is_success() {
        return Err(Error::Payjoin(format!(
            "Payjoin HTTP request failed with status {}",
            response.status()
        )));
    }
    tokio::time::timeout(PAYJOIN_HTTP_REQUEST_TIMEOUT, response.bytes())
        .await
        .map_err(|_| {
            Error::Payjoin(format!(
                "Payjoin HTTP response body timed out after {} seconds",
                PAYJOIN_HTTP_REQUEST_TIMEOUT.as_secs()
            ))
        })?
        .map(|bytes| bytes.to_vec())
        .map_err(|err| Error::Payjoin(err.to_string()))
}

#[cfg(test)]
mod tests;
