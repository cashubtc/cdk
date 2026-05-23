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
use cdk_common::payjoin::{
    format_bip21_amount_from_sats, payjoin_v2_from_bip77_endpoint, payjoin_v2_to_bip77_endpoint,
};
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
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("amount", &format_bip21_amount_from_sats(amount_sat));
    serializer.append_pair("pj", &build_payjoin_endpoint(payjoin)?);
    Ok(format!("bitcoin:{}?{}", address, serializer.finish()))
}

fn build_payjoin_endpoint(payjoin: &PayjoinV2) -> Result<String, Error> {
    // The payjoin sender expects a BIP21/BIP77 `pj` URI. Cashu uses Unix
    // timestamp; BIP77 URI fragments use encoded `EX1`, so rebuild it only at
    // this library boundary.
    payjoin_v2_to_bip77_endpoint(payjoin).map_err(|err| Error::Payjoin(err.to_string()))
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
mod tests {
    use std::str::FromStr;

    use bdk_wallet::bitcoin::absolute::LockTime;
    use bdk_wallet::bitcoin::{
        transaction, Amount as BitcoinAmount, Network, Psbt, ScriptBuf, TxOut, Txid,
    };
    use bdk_wallet::keys::bip39::Mnemonic;
    use cdk_common::common::FeeReserve;

    use super::*;

    async fn build_test_backend() -> (CdkBdk, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mnemonic = Mnemonic::from_str(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("mnemonic");
        let kv = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory kv store");
        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::new(1, CurrencyUnit::Sat).into(),
            percent_fee_reserve: 0.02,
        };
        let backend = CdkBdk::new(
            mnemonic,
            Network::Regtest,
            crate::ChainSource::Esplora(crate::EsploraConfig {
                url: "http://127.0.0.1:1".to_string(),
                parallel_requests: 1,
            }),
            tmp.path().to_string_lossy().into_owned(),
            fee_reserve,
            Arc::new(kv),
            None,
            1,
            0,
            546,
            60,
            Some(1),
            None,
            None,
        )
        .expect("build CdkBdk test instance");

        (backend, tmp)
    }

    fn test_psbt_with_outputs(outputs: Vec<TxOut>) -> Psbt {
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(
                    Txid::from_str(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    )
                    .expect("valid txid"),
                    0,
                ),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            }],
            output: outputs,
        };
        Psbt::from_unsigned_tx(tx).expect("valid test psbt")
    }

    #[test]
    fn amountless_payjoin_receive_session_cap_comes_from_original_receiver_outputs() {
        let events = vec![
            ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![1]),
            ::payjoin::receive::v2::SessionEvent::CommittedOutputs(vec![
                TxOut {
                    value: BitcoinAmount::from_sat(8_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: BitcoinAmount::from_sat(3_000),
                    script_pubkey: ScriptBuf::new(),
                },
            ]),
        ];
        let mut record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 0,
            proposal_receiver_outpoints: Vec::new(),
            proposal_tx_bytes: None,
            cut_through: None,
            expires_at: 1_700_000_000,
            events,
            closed: false,
        };

        update_payjoin_receive_credit_cap(&mut record);

        assert_eq!(record.amount_sat, 3_000);
    }

    #[test]
    fn payjoin_receive_session_records_proposal_receiver_outpoints() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(8_000),
                script_pubkey: other_script,
            },
            TxOut {
                value: BitcoinAmount::from_sat(3_000),
                script_pubkey: fallback_script.clone(),
            },
        ]);
        let expected_outpoint = OutPoint::new(psbt.unsigned_tx.compute_txid(), 1).to_string();
        let mut record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 3_000,
            proposal_receiver_outpoints: Vec::new(),
            proposal_tx_bytes: None,
            cut_through: None,
            expires_at: 1_700_000_000,
            events: Vec::new(),
            closed: false,
        };

        update_payjoin_receive_proposal_receiver_outpoints(&mut record, &psbt, &fallback_script);

        assert_eq!(record.proposal_receiver_outpoints, vec![expected_outpoint]);
    }

    #[test]
    fn payjoin_receiver_credit_sums_final_receiver_outputs() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(2_000),
                script_pubkey: fallback_script.clone(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(9_000),
                script_pubkey: other_script,
            },
            TxOut {
                value: BitcoinAmount::from_sat(3_000),
                script_pubkey: fallback_script.clone(),
            },
        ]);

        assert_eq!(
            payjoin_receiver_output_amount(&psbt, &fallback_script).expect("sum outputs"),
            5_000
        );
    }

    #[test]
    fn payjoin_receiver_credit_accepts_unreduced_receiver_output() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(5_000),
            script_pubkey: fallback_script.clone(),
        }]);

        ensure_payjoin_receiver_credit(&psbt, &fallback_script, 5_000)
            .expect("sender-funded payjoin keeps receiver output whole");
    }

    #[test]
    fn payjoin_receiver_credit_rejects_reduced_receiver_output() {
        let fallback_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(4_999),
            script_pubkey: fallback_script.clone(),
        }]);

        let err = ensure_payjoin_receiver_credit(&psbt, &fallback_script, 5_000)
            .expect_err("receiver output below original amount must be rejected");

        assert!(err.to_string().contains("below original amount"));
    }

    #[test]
    fn payjoin_send_payment_output_accepts_exact_output() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(9_000),
                script_pubkey: other_script,
            },
            TxOut {
                value: BitcoinAmount::from_sat(10_000),
                script_pubkey: payment_script.clone(),
            },
        ]);

        let outpoint =
            require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
                .expect("payment output is present");

        assert_eq!(outpoint.vout, 1);
    }

    #[test]
    fn payjoin_send_payment_output_accepts_larger_output() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(12_000),
            script_pubkey: payment_script.clone(),
        }]);

        let outpoint =
            require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
                .expect("larger payment output is present");

        assert_eq!(outpoint.vout, 0);
    }

    #[test]
    fn payjoin_send_payment_output_rejects_smaller_single_output() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let other_script = ScriptBuf::from_bytes(vec![0x6a]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(9_999),
                script_pubkey: payment_script.clone(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(10_000),
                script_pubkey: other_script,
            },
        ]);

        let err = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
            .expect_err("altered payment output must be rejected");

        assert!(err.to_string().contains("missing payment output"));
    }

    #[test]
    fn payjoin_send_payment_output_rejects_split_only_outputs() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![
            TxOut {
                value: BitcoinAmount::from_sat(6_000),
                script_pubkey: payment_script.clone(),
            },
            TxOut {
                value: BitcoinAmount::from_sat(4_000),
                script_pubkey: payment_script.clone(),
            },
        ]);

        let err = require_payjoin_send_payment_output(&psbt.unsigned_tx, &payment_script, 10_000)
            .expect_err("split-only receiver outputs are unsupported");

        assert!(err.to_string().contains("missing payment output"));
    }

    #[test]
    fn payjoin_send_validation_accepts_net_spend_within_cap() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        }]);

        let validation = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            12_000,
            1_000,
        )
        .expect("net spend at cap is accepted");

        assert_eq!(validation.fee_contribution_sat, 1_000);
    }

    #[test]
    fn payjoin_send_validation_accepts_larger_receiver_output_with_local_fee_cap() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(12_000),
            script_pubkey: payment_script.clone(),
        }]);

        let validation = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            20_000,
            9_500,
        )
        .expect("receiver-funded larger output is accepted when mint spend is capped");

        assert_eq!(validation.fee_contribution_sat, 500);
    }

    #[test]
    fn payjoin_send_validation_rejects_net_spend_above_cap() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        }]);

        let err = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            12_001,
            1_000,
        )
        .expect_err("net spend above amount plus max fee is rejected");

        assert!(err.to_string().contains("exceeding cap"));
    }

    #[test]
    fn payjoin_send_validation_rejects_net_spend_below_payment_amount() {
        let payment_script = ScriptBuf::from_bytes(vec![0x51]);
        let psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(10_000),
            script_pubkey: payment_script.clone(),
        }]);

        let err = validate_payjoin_send_transaction(
            &psbt.unsigned_tx,
            &payment_script,
            10_000,
            1_000,
            9_999,
            0,
        )
        .expect_err("mint net spend below quote cannot produce fee contribution");

        assert!(err.to_string().contains("below payment amount"));
    }

    #[test]
    fn payjoin_original_receiver_output_amount_sums_all_receiver_outputs() {
        let events = vec![
            ::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0, 2]),
            ::payjoin::receive::v2::SessionEvent::CommittedOutputs(vec![
                TxOut {
                    value: BitcoinAmount::from_sat(21_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: BitcoinAmount::from_sat(99_000),
                    script_pubkey: ScriptBuf::new(),
                },
                TxOut {
                    value: BitcoinAmount::from_sat(34_000),
                    script_pubkey: ScriptBuf::new(),
                },
            ]),
        ];

        assert_eq!(
            payjoin_original_receiver_output_amount_from_events(&events),
            Some(55_000)
        );
    }

    #[test]
    fn payjoin_receive_amount_missing_events_returns_none() {
        let events = vec![::payjoin::receive::v2::SessionEvent::IdentifiedReceiverOutputs(vec![0])];

        assert_eq!(
            payjoin_original_receiver_output_amount_from_events(&events),
            None
        );
    }

    #[test]
    fn payjoin_original_input_outpoints_come_from_retrieved_payload_event() {
        let first_outpoint = OutPoint::new(
            Txid::from_str("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .expect("valid txid"),
            0,
        );
        let second_outpoint = OutPoint::new(
            Txid::from_str("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .expect("valid txid"),
            1,
        );
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![
                TxIn {
                    previous_output: first_outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Default::default(),
                },
                TxIn {
                    previous_output: second_outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Default::default(),
                },
            ],
            output: vec![TxOut {
                value: BitcoinAmount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let psbt = Psbt::from_unsigned_tx(tx).expect("valid unsigned psbt");
        let event = serde_json::json!({
            "RetrievedOriginalPayload": {
                "original": {
                    "psbt": psbt,
                    "params": {
                        "v": 2,
                        "output_substitution": "Enabled",
                        "additional_fee_contribution": null,
                        "min_fee_rate": 250
                    }
                },
                "reply_key": null
            }
        });
        let event = serde_json::from_value(event).expect("deserialize Payjoin session event");

        assert_eq!(
            payjoin_original_input_outpoints_from_events(&[event])
                .expect("extract original input outpoints"),
            vec![first_outpoint, second_outpoint]
        );
    }

    #[test]
    fn payjoin_receive_session_expiry_is_strictly_in_the_past() {
        let record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 1_000,
            proposal_receiver_outpoints: Vec::new(),
            proposal_tx_bytes: None,
            cut_through: None,
            expires_at: 100,
            events: Vec::new(),
            closed: false,
        };

        assert!(!record.is_expired(100));
        assert!(record.is_expired(101));
    }

    #[test]
    fn payjoin_receive_session_prunes_closed_records_after_retention() {
        let record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address: "bcrt1qfallback".to_string(),
            amount_sat: 1_000,
            proposal_receiver_outpoints: Vec::new(),
            proposal_tx_bytes: None,
            cut_through: None,
            expires_at: 100,
            events: Vec::new(),
            closed: true,
        };
        let retention_edge = 100 + PAYJOIN_RECEIVE_SESSION_RETENTION_SECS;

        assert!(!record.should_prune(retention_edge, PAYJOIN_RECEIVE_SESSION_RETENTION_SECS));
        assert!(record.should_prune(retention_edge + 1, PAYJOIN_RECEIVE_SESSION_RETENTION_SECS));
    }

    #[tokio::test]
    async fn payjoin_receive_credit_cap_outlives_unresolved_proposal() {
        use bdk_wallet::bitcoin::hashes::Hash;
        use bdk_wallet::bitcoin::{Address, WPubkeyHash};

        use crate::receive::receive_intent::record::{ReceiveIntentRecord, ReceiveIntentState};

        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db");
        let storage = crate::storage::BdkStorage::new(Arc::new(db));

        let fallback_script = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([7u8; 20]));
        let fallback_address = Address::from_script(&fallback_script, Network::Regtest)
            .expect("valid script")
            .to_string();

        let original_tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(
                    Txid::from_str(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    )
                    .expect("valid txid"),
                    0,
                ),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Default::default(),
            }],
            output: vec![TxOut {
                value: BitcoinAmount::from_sat(1_000),
                script_pubkey: fallback_script,
            }],
        };
        let original_txid = original_tx.compute_txid();
        let mut psbt = Psbt::from_unsigned_tx(original_tx).expect("valid unsigned psbt");
        // check_broadcast_suitability computes the psbt fee rate, which needs
        // input UTXO data.
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: BitcoinAmount::from_sat(2_000),
            script_pubkey: ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([9u8; 20])),
        });
        let event = serde_json::json!({
            "RetrievedOriginalPayload": {
                "original": {
                    "psbt": psbt,
                    "params": {
                        "v": 2,
                        "output_substitution": "Enabled",
                        "additional_fee_contribution": null,
                        "min_fee_rate": 250
                    }
                },
                "reply_key": null
            }
        });
        let events = vec![serde_json::from_value(event).expect("deserialize session event")];

        let proposal_outpoint =
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc:0".to_string();
        let record = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: "quote-1".to_string(),
            fallback_address,
            amount_sat: 1_000,
            proposal_receiver_outpoints: vec![proposal_outpoint.clone()],
            proposal_tx_bytes: None,
            cut_through: None,
            expires_at: 100,
            events,
            closed: true,
        };

        // A session that never signed a proposal has no cap worth keeping.
        let mut no_proposal = record.clone();
        no_proposal.proposal_receiver_outpoints.clear();
        assert!(
            payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &no_proposal)
                .await
                .expect("resolve")
        );

        // The signed proposal is still broadcastable: keep the cap.
        assert!(
            !payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &record)
                .await
                .expect("resolve")
        );

        // Once a proposal receiver outpoint was detected the cap was applied.
        let detect = |outpoint: String, txid: String| ReceiveIntentRecord {
            intent_id: Uuid::new_v4(),
            quote_id: "quote-1".to_string(),
            state: ReceiveIntentState::Detected {
                address: record.fallback_address.clone(),
                txid,
                outpoint,
                amount_sat: 1_000,
                block_height: 1,
                created_at: 0,
            },
        };
        let proposal_txid = proposal_outpoint
            .split_once(':')
            .expect("outpoint format")
            .0
            .to_string();
        storage
            .create_receive_intent_if_absent(&detect(proposal_outpoint.clone(), proposal_txid))
            .await
            .expect("create intent");
        assert!(
            payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &record)
                .await
                .expect("resolve")
        );

        // A settled original also resolves the cap: the proposal conflicts
        // with it on the sender inputs and can never confirm.
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory db");
        let storage = crate::storage::BdkStorage::new(Arc::new(db));
        storage
            .create_receive_intent_if_absent(&detect(
                OutPoint::new(original_txid, 0).to_string(),
                original_txid.to_string(),
            ))
            .await
            .expect("create intent");
        assert!(
            payjoin_receive_credit_cap_resolved(&storage, Network::Regtest, &record)
                .await
                .expect("resolve")
        );
    }

    #[tokio::test]
    async fn cut_through_receive_reuses_reserved_intent_before_exposure() {
        let (backend, _tmp) = build_test_backend().await;
        let receive_quote_id = "receive-quote".to_string();
        let send_intent_id = Uuid::new_v4();
        let intent = crate::send::payment_intent::record::SendIntentRecord {
            intent_id: send_intent_id,
            quote_id: "send-quote".to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 40_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: crate::send::payment_intent::record::SendIntentState::Pending {
                created_at: 1_700_000_000,
            },
        };
        backend
            .storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store pending intent");
        let reservation_id = Uuid::new_v4();
        backend
            .storage
            .reserve_pending_send_intent_for_cut_through(
                &send_intent_id,
                reservation_id,
                &receive_quote_id,
                50_000,
            )
            .await
            .expect("reserve intent")
            .expect("reservation");

        let existing = backend
            .reserved_cut_through_candidate(&receive_quote_id, 50_000)
            .await
            .expect("load existing reservation");

        let (intent_record, reused_reservation_id) = existing.expect("reusable reservation");
        assert_eq!(reused_reservation_id, reservation_id);
        assert_eq!(intent_record.intent_id, send_intent_id);
        assert!(backend
            .storage
            .get_pending_send_intents()
            .await
            .expect("pending intents")
            .is_empty());
    }

    #[tokio::test]
    async fn cut_through_receive_abandons_mismatched_reserved_intent_before_fallback() {
        let (backend, _tmp) = build_test_backend().await;
        let receive_quote_id = "receive-quote".to_string();
        let send_intent_id = Uuid::new_v4();
        let intent = crate::send::payment_intent::record::SendIntentRecord {
            intent_id: send_intent_id,
            quote_id: "send-quote".to_string(),
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat: 40_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: crate::send::payment_intent::record::SendIntentState::Pending {
                created_at: 1_700_000_000,
            },
        };
        backend
            .storage
            .create_send_intent_if_absent(&intent)
            .await
            .expect("store pending intent");
        let reservation_id = Uuid::new_v4();
        backend
            .storage
            .reserve_pending_send_intent_for_cut_through(
                &send_intent_id,
                reservation_id,
                &receive_quote_id,
                60_000,
            )
            .await
            .expect("reserve intent")
            .expect("reservation");

        let existing = backend
            .reserved_cut_through_candidate(&receive_quote_id, 50_000)
            .await
            .expect("load existing reservation");

        assert!(existing.is_none());
        assert!(matches!(
            backend
                .storage
                .get_send_intent(&send_intent_id)
                .await
                .expect("load intent")
                .expect("intent")
                .state,
            crate::send::payment_intent::record::SendIntentState::Pending { .. }
        ));
    }

    #[tokio::test]
    async fn payjoin_proposal_replay_detects_exposed_cut_through_settlement() {
        let (backend, _tmp) = build_test_backend().await;
        let quote_id = "receive-quote".to_string();
        let mut proposal_psbt = test_psbt_with_outputs(vec![TxOut {
            value: BitcoinAmount::from_sat(40_000),
            script_pubkey: ScriptBuf::new(),
        }]);
        proposal_psbt.inputs[0].witness_utxo = Some(TxOut {
            value: BitcoinAmount::from_sat(41_000),
            script_pubkey: ScriptBuf::new(),
        });
        proposal_psbt.inputs[0].final_script_witness = Some(Default::default());
        let proposal_tx = proposal_psbt
            .clone()
            .extract_tx()
            .expect("test proposal extracts");
        let session = crate::storage::PayjoinReceiveSessionRecord {
            quote_id: quote_id.clone(),
            fallback_address: "bcrt1qaddr".to_string(),
            amount_sat: 50_000,
            proposal_receiver_outpoints: Vec::new(),
            proposal_tx_bytes: Some(consensus::serialize(&proposal_tx)),
            cut_through: Some(crate::storage::PayjoinCutThroughProgress::Active {
                reservation_id: Uuid::new_v4(),
                send_intent_id: Uuid::new_v4(),
                proposal_txid: proposal_tx.compute_txid().to_string(),
            }),
            expires_at: 1_700_000_001,
            events: Vec::new(),
            closed: false,
        };
        backend
            .storage
            .put_payjoin_receive_session(&session)
            .await
            .expect("store exposed settlement");

        assert!(backend
            .exposed_cut_through_for_proposal(&quote_id, &proposal_psbt)
            .await
            .expect("lookup exposed settlement"));
        assert!(!backend
            .exposed_cut_through_for_proposal("other-quote", &proposal_psbt)
            .await
            .expect("lookup unrelated quote"));
    }

    #[test]
    fn builds_payjoin_endpoint_from_normalized_fields() {
        let payjoin = PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
            "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
            1_720_547_781,
        )
        .expect("valid Payjoin keys");

        assert_eq!(
            build_payjoin_endpoint(&payjoin).expect("endpoint builds"),
            "https://payjoin.example/pj#EX1C4UC6ES-OH1QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ-RK1QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG"
        );
    }
}
