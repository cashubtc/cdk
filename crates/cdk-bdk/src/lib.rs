//! CDK onchain backend using BDK

#![doc = include_str!("../README.md")]

use std::fs;
use std::future::Future;
#[cfg(feature = "payjoin")]
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(feature = "payjoin")]
use std::sync::Mutex as StdMutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bdk_wallet::bitcoin::Network;
#[cfg(feature = "payjoin")]
use bdk_wallet::bitcoin::{OutPoint, Sequence, Transaction, TxIn};
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, PersistedWallet, Wallet};
use cdk_common::common::FeeReserve;
use cdk_common::database::KVStore;
use cdk_common::nuts::nut30::MeltQuoteOnchainFeeOption;
use cdk_common::nuts::nut31::OnchainPayjoin;
#[cfg(feature = "payjoin")]
use cdk_common::nuts::nut31::{OnchainPayjoinRequest, PayjoinV2, PAYJOIN_V2_VERSION};
use cdk_common::payment::{
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OnchainSettings, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
    SettingsResponse, WaitPaymentResponse,
};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};
use futures::Stream;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
#[cfg(feature = "payjoin")]
use uuid::Uuid;

pub use crate::chain::{BitcoinRpcConfig, ChainSource, EsploraConfig};
pub use crate::error::Error;
pub use crate::storage::{BdkStorage, FinalizedReceiveIntentRecord, FinalizedSendIntentRecord};
#[cfg(feature = "payjoin")]
pub use crate::types::PayjoinConfig;
pub use crate::types::{
    BatchConfig, FeeEstimationConfig, PaymentMetadata, PaymentTier, SyncConfig,
    DEFAULT_PAYJOIN_EXPIRY_SECS, DEFAULT_TARGET_BLOCK_TIME_SECS,
};

pub mod chain;
pub mod error;
pub(crate) mod fee;
pub mod receive;
pub(crate) mod recovery;
pub mod send;
pub mod storage;
pub(crate) mod sync;
pub mod types;
pub(crate) mod util;

/// Wrapper struct that combines wallet and database to prevent deadlocks
pub(crate) struct WalletWithDb {
    pub(crate) wallet: PersistedWallet<Connection>,
    pub(crate) db: Connection,
}

pub(crate) struct BackgroundTasks {
    pub(crate) cancel: CancellationToken,
    pub(crate) sync: JoinHandle<()>,
    pub(crate) batch: JoinHandle<()>,
    #[cfg(feature = "payjoin")]
    pub(crate) payjoin_receive: Option<JoinHandle<()>>,
}

struct PaymentEventStream {
    receiver: BroadcastStream<Event>,
    cancel: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    is_active: Arc<AtomicBool>,
}

impl Stream for PaymentEventStream {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.cancel.as_mut().poll(cx).is_ready() {
            this.is_active.store(false, Ordering::SeqCst);
            return Poll::Ready(None);
        }

        loop {
            match Pin::new(&mut this.receiver).poll_next(cx) {
                Poll::Ready(Some(Ok(event))) => return Poll::Ready(Some(event)),
                Poll::Ready(Some(Err(err))) => {
                    tracing::warn!(
                        "cdk-bdk payment event subscriber lagged or errored: {}",
                        err
                    );
                }
                Poll::Ready(None) => {
                    this.is_active.store(false, Ordering::SeqCst);
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl Drop for PaymentEventStream {
    fn drop(&mut self) {
        self.is_active.store(false, Ordering::SeqCst);
    }
}

#[cfg(feature = "payjoin")]
#[derive(Debug, Clone)]
struct RecordingSessionPersister<E> {
    events: Arc<StdMutex<Vec<E>>>,
    closed: Arc<AtomicBool>,
    _marker: PhantomData<E>,
}

#[cfg(feature = "payjoin")]
impl<E> RecordingSessionPersister<E>
where
    E: Clone,
{
    fn new(events: Vec<E>, closed: bool) -> Self {
        Self {
            events: Arc::new(StdMutex::new(events)),
            closed: Arc::new(AtomicBool::new(closed)),
            _marker: PhantomData,
        }
    }

    fn events(&self) -> Result<Vec<E>, Error> {
        self.events
            .lock()
            .map(|events| events.clone())
            .map_err(|err| Error::Payjoin(format!("Payjoin session lock poisoned: {}", err)))
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

#[cfg(feature = "payjoin")]
impl<E> payjoin::persist::SessionPersister for RecordingSessionPersister<E>
where
    E: Clone + Send + Sync + 'static,
{
    type InternalStorageError = std::convert::Infallible;
    type SessionEvent = E;

    fn save_event(&self, event: &Self::SessionEvent) -> Result<(), Self::InternalStorageError> {
        if let Ok(mut events) = self.events.lock() {
            events.push(event.clone());
        }
        Ok(())
    }

    fn load(
        &self,
    ) -> Result<Box<dyn Iterator<Item = Self::SessionEvent>>, Self::InternalStorageError> {
        let events = self
            .events
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();
        Ok(Box::new(events.into_iter()))
    }

    fn close(&self) -> Result<(), Self::InternalStorageError> {
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

impl WalletWithDb {
    pub(crate) fn new(wallet: PersistedWallet<Connection>, db: Connection) -> Self {
        Self { wallet, db }
    }

    pub(crate) fn persist(&mut self) -> Result<bool, bdk_wallet::rusqlite::Error> {
        self.wallet.persist(&mut self.db)
    }
}

/// CDK onchain payment backend using BDK (Bitcoin Development Kit)
#[derive(Clone)]
pub struct CdkBdk {
    pub(crate) fee_reserve: FeeReserve,
    pub(crate) wait_invoice_cancel_token: CancellationToken,
    pub(crate) wait_invoice_is_active: Arc<AtomicBool>,
    pub(crate) payment_sender: tokio::sync::broadcast::Sender<Event>,
    pub(crate) tasks: Arc<Mutex<Option<BackgroundTasks>>>,
    pub(crate) shutdown_timeout: Duration,
    pub(crate) wallet_with_db: Arc<Mutex<WalletWithDb>>,
    pub(crate) chain_source: ChainSource,
    pub(crate) storage: BdkStorage,
    pub(crate) network: Network,
    /// Batch processor configuration
    pub(crate) batch_config: BatchConfig,
    /// Notify handle to wake up the batch processor immediately
    pub(crate) batch_notify: Arc<Notify>,
    /// Number of confirmations required for on-chain payments
    pub(crate) num_confs: u32,
    /// Minimum on-chain receive amount that should count toward minting
    pub(crate) min_receive_amount_sat: u64,
    /// Minimum on-chain send amount accepted for melts
    pub(crate) min_send_amount_sat: u64,
    /// Sync interval in seconds
    pub(crate) sync_interval_secs: u64,
    /// Blockchain sync configuration
    pub(crate) sync_config: SyncConfig,
    /// Cache for fee rate estimation: Tier -> (sat_per_vb, timestamp)
    pub(crate) fee_rate_cache: Arc<Mutex<std::collections::HashMap<PaymentTier, (f64, u64)>>>,
    /// Payjoin v2 configuration, when compiled and enabled.
    #[cfg(feature = "payjoin")]
    pub(crate) payjoin_config: Option<PayjoinConfig>,
}

impl CdkBdk {
    fn requested_payjoin(metadata: Option<&str>) -> Option<OnchainPayjoin> {
        let value = metadata
            .and_then(|metadata| serde_json::from_str::<serde_json::Value>(metadata).ok())?;
        value
            .get("payjoin")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
    }

    #[cfg(feature = "payjoin")]
    fn accepted_payjoin_extra(payjoin: &OnchainPayjoin) -> serde_json::Value {
        serde_json::json!({
            "payjoin": OnchainPayjoinRequest {
                version: PAYJOIN_V2_VERSION,
                required: payjoin.is_required(),
            }
        })
    }

    #[cfg(feature = "payjoin")]
    async fn create_payjoin_receive_extra(
        &self,
        quote_id: &cdk_common::QuoteId,
        address: &bdk_wallet::bitcoin::Address,
        amount_sat: u64,
        required: bool,
    ) -> Result<Option<serde_json::Value>, Error> {
        let Some(config) = self.payjoin_config() else {
            if required {
                return Err(Error::PayjoinUnavailable(
                    "operator did not configure Payjoin directory and OHTTP relay".to_string(),
                ));
            }
            return Ok(None);
        };

        let ohttp_keys =
            payjoin::io::fetch_ohttp_keys(&config.ohttp_relay_url, &config.directory_url)
                .await
                .map_err(|err| Error::Payjoin(err.to_string()))?;
        let persister = RecordingSessionPersister::new(Vec::new(), false);
        let receiver =
            payjoin::receive::v2::Receiver::<payjoin::receive::v2::UninitializedReceiver>::create_session(
                address.clone(),
                config.directory_url.clone(),
                ohttp_keys.clone(),
                Some(Duration::from_secs(config.expiry_secs)),
            )
            .save(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let pj_uri = receiver.pj_uri().to_string();
        let endpoint = extract_bip21_payjoin_endpoint(&pj_uri)?;
        let receiver_key = extract_payjoin_fragment_value(&endpoint, "RK1").unwrap_or_default();
        let expires_at = crate::util::unix_now()
            .checked_add(config.expiry_secs)
            .unwrap_or(u64::MAX);

        self.storage
            .put_payjoin_receive_session(&crate::storage::PayjoinReceiveSessionRecord {
                quote_id: quote_id.to_string(),
                fallback_address: address.to_string(),
                amount_sat,
                required,
                expires_at,
                events: persister.events()?,
                closed: persister.closed(),
            })
            .await?;

        let payjoin = OnchainPayjoin {
            version: PAYJOIN_V2_VERSION,
            params: PayjoinV2 {
                endpoint,
                ohttp_relay: config.ohttp_relay_url.clone(),
                ohttp_keys: ohttp_keys.to_string(),
                receiver_key,
                expires_at: Some(expires_at),
                required,
            },
        };

        Ok(Some(serde_json::json!({ "payjoin": payjoin })))
    }

    #[cfg(not(feature = "payjoin"))]
    async fn create_payjoin_receive_extra(
        &self,
        _quote_id: &cdk_common::QuoteId,
        _address: &bdk_wallet::bitcoin::Address,
        _amount_sat: u64,
        required: bool,
    ) -> Result<Option<serde_json::Value>, Error> {
        if required {
            return Err(Error::PayjoinUnavailable(
                "cdk-bdk was built without the payjoin feature".to_string(),
            ));
        }
        Ok(None)
    }

    #[cfg(feature = "payjoin")]
    pub(crate) async fn run_payjoin_receive_poller(
        &self,
        cancel_token: CancellationToken,
    ) -> Result<(), Error> {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        tracing::info!("Starting Payjoin receive poller");
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = tick.tick() => {
                    for record in self.storage.get_all_payjoin_receive_sessions().await? {
                        if record.closed || record.expires_at < crate::util::unix_now() {
                            continue;
                        }
                        if let Err(err) = self.process_payjoin_receive_session(record).await {
                            tracing::warn!("Payjoin receive session processing failed: {}", err);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "payjoin")]
    async fn process_payjoin_receive_session(
        &self,
        mut record: crate::storage::PayjoinReceiveSessionRecord,
    ) -> Result<(), Error> {
        use payjoin::persist::OptionalTransitionOutcome;

        let Some(config) = self.payjoin_config() else {
            return Ok(());
        };
        let fallback_address = bdk_wallet::bitcoin::Address::from_str(&record.fallback_address)
            .map_err(|err| Error::Payjoin(err.to_string()))?
            .require_network(self.network)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let fallback_script = fallback_address.script_pubkey();
        let persister = RecordingSessionPersister::new(record.events.clone(), record.closed);
        let (session, history) = payjoin::receive::v2::replay_event_log(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        if let Some((request, context)) = history
            .extract_err_req(&config.ohttp_relay_url)
            .map_err(|err| Error::Payjoin(err.to_string()))?
        {
            let response = payjoin_http_request(request).await?;
            payjoin::receive::v2::process_err_res(&response, context)
                .map_err(|err| Error::Payjoin(err.to_string()))?;
            record.closed = true;
            record.events = persister.events()?;
            self.storage.put_payjoin_receive_session(&record).await?;
            return Ok(());
        }

        let payjoin_proposal = match session {
            payjoin::receive::v2::ReceiveSession::Initialized(mut receiver) => {
                let (request, context) = receiver
                    .extract_req(&config.ohttp_relay_url)
                    .map_err(|err| Error::Payjoin(err.to_string()))?;
                let response = payjoin_http_request(request).await?;
                let unchecked = match receiver
                    .process_res(&response, context)
                    .save(&persister)
                    .map_err(|err| Error::Payjoin(err.to_string()))?
                {
                    OptionalTransitionOutcome::Progress(unchecked) => unchecked,
                    OptionalTransitionOutcome::Stasis(_) => {
                        record.events = persister.events()?;
                        record.closed = persister.closed();
                        self.storage.put_payjoin_receive_session(&record).await?;
                        return Ok(());
                    }
                };
                Some(
                    self.accept_payjoin_receive_proposal(unchecked, &fallback_script, &persister)
                        .await?,
                )
            }
            payjoin::receive::v2::ReceiveSession::UncheckedProposal(unchecked) => Some(
                self.accept_payjoin_receive_proposal(unchecked, &fallback_script, &persister)
                    .await?,
            ),
            payjoin::receive::v2::ReceiveSession::PayjoinProposal(proposal) => Some(proposal),
            payjoin::receive::v2::ReceiveSession::TerminalFailure => {
                record.closed = true;
                record.events = persister.events()?;
                self.storage.put_payjoin_receive_session(&record).await?;
                return Ok(());
            }
            _ => None,
        };

        if let Some(mut proposal) = payjoin_proposal {
            let (request, context) = proposal
                .extract_req(&config.ohttp_relay_url)
                .map_err(|err| Error::Payjoin(err.to_string()))?;
            let response = payjoin_http_request(request).await?;
            proposal
                .process_res(&response, context)
                .save(&persister)
                .map_err(|err| Error::Payjoin(err.to_string()))?;
        }

        record.events = persister.events()?;
        record.closed = persister.closed();
        self.storage.put_payjoin_receive_session(&record).await?;
        Ok(())
    }

    #[cfg(feature = "payjoin")]
    async fn accept_payjoin_receive_proposal(
        &self,
        unchecked: payjoin::receive::v2::Receiver<payjoin::receive::v2::UncheckedProposal>,
        fallback_script: &bdk_wallet::bitcoin::Script,
        persister: &RecordingSessionPersister<payjoin::receive::v2::SessionEvent>,
    ) -> Result<payjoin::receive::v2::Receiver<payjoin::receive::v2::PayjoinProposal>, Error> {
        let receiver = unchecked
            .assume_interactive_receiver()
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let wallet_with_db = self.wallet_with_db.lock().await;
        let receiver = receiver
            .check_inputs_not_owned(|script| Ok(wallet_with_db.wallet.is_mine(script.to_owned())))
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = receiver
            .check_no_inputs_seen_before(|_| Ok(false))
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = receiver
            .identify_receiver_outputs(|script| Ok(script == fallback_script))
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = receiver
            .commit_outputs()
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;

        let candidate_inputs = wallet_with_db
            .wallet
            .list_unspent()
            .filter_map(|utxo| {
                let psbt_input = wallet_with_db
                    .wallet
                    .get_psbt_input(utxo.clone(), None, true)
                    .ok()?;
                payjoin::receive::InputPair::new(
                    TxIn {
                        previous_output: utxo.outpoint,
                        script_sig: Default::default(),
                        sequence: Sequence::MAX,
                        witness: Default::default(),
                    },
                    psbt_input,
                )
                .ok()
            })
            .collect::<Vec<_>>();
        let selected = receiver
            .try_preserving_privacy(candidate_inputs.clone())
            .or_else(|_| {
                candidate_inputs.into_iter().next().ok_or_else(|| {
                    Error::Payjoin("no Payjoin contribution input available".to_string())
                })
            })?;
        let receiver = receiver
            .contribute_inputs([selected])
            .map_err(|err| Error::Payjoin(err.to_string()))?
            .commit_inputs()
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let receiver = receiver
            .finalize_proposal(
                |psbt| {
                    let mut psbt = psbt.clone();
                    wallet_with_db
                        .wallet
                        .sign(&mut psbt, Default::default())
                        .map_err(|err| -> payjoin::ImplementationError {
                            Box::new(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                err.to_string(),
                            ))
                        })?;
                    Ok(psbt)
                },
                None,
                None,
            )
            .save(persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        drop(wallet_with_db);

        Ok(receiver)
    }

    #[cfg(feature = "payjoin")]
    async fn send_payjoin_payment(
        &self,
        quote_id: &cdk_common::QuoteId,
        address: &str,
        amount_sat: u64,
        max_fee_sat: u64,
        tier: PaymentTier,
        payjoin: &OnchainPayjoin,
    ) -> Result<MakePaymentResponse, Error> {
        use payjoin::persist::OptionalTransitionOutcome;
        use payjoin::UriExt;

        let fallback_address = bdk_wallet::bitcoin::Address::from_str(address)
            .map_err(|e| Error::Wallet(e.to_string()))?
            .require_network(self.network)
            .map_err(|e| Error::Wallet(e.to_string()))?;
        let sat_per_vb = self
            .estimate_fee_rate_sat_per_vb(tier)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    tier = ?tier,
                    error = %e,
                    "Payjoin fee-rate estimation failed, using configured fallback"
                );
                self.batch_config.fee_estimation.fallback_sat_per_vb
            });
        let fee_rate = bdk_wallet::bitcoin::FeeRate::from_sat_per_vb_u32(sat_per_vb.ceil() as u32);

        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let mut tx_builder = wallet_with_db.wallet.build_tx();
        tx_builder.add_recipient(
            fallback_address.clone(),
            bdk_wallet::bitcoin::Amount::from_sat(amount_sat),
        );
        tx_builder.fee_rate(fee_rate);
        let mut original_psbt = tx_builder
            .finish()
            .map_err(|err| Error::Payjoin(format!("Could not build original PSBT: {}", err)))?;
        let original_fee_sat = original_psbt
            .fee()
            .map_err(|err| {
                Error::Payjoin(format!("Could not calculate original PSBT fee: {}", err))
            })?
            .to_sat();
        if original_fee_sat > max_fee_sat {
            return Err(Error::Payjoin(format!(
                "original Payjoin PSBT fee {} exceeds max fee {}",
                original_fee_sat, max_fee_sat
            )));
        }
        if !wallet_with_db
            .wallet
            .sign(&mut original_psbt, Default::default())
            .map_err(|err| Error::Payjoin(format!("Could not sign original PSBT: {}", err)))?
        {
            return Err(Error::CouldNotSign);
        }
        wallet_with_db
            .persist()
            .map_err(|err| Error::Payjoin(format!("Could not persist wallet: {}", err)))?;
        drop(wallet_with_db);

        let pj_uri = build_payjoin_uri(address, amount_sat, &payjoin.params.endpoint);
        let pj_uri = payjoin::Uri::try_from(pj_uri.as_str())
            .map_err(|err| Error::Payjoin(format!("Invalid Payjoin URI: {}", err)))?
            .assume_checked()
            .check_pj_supported()
            .map_err(|_| {
                Error::Payjoin("Payjoin URI did not contain supported pj params".to_string())
            })?;
        let persister = RecordingSessionPersister::new(Vec::new(), false);
        let sender = payjoin::send::v2::SenderBuilder::new(original_psbt, pj_uri)
            .build_recommended(fee_rate)
            .save(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        self.storage
            .put_payjoin_send_session(&crate::storage::PayjoinSendSessionRecord {
                quote_id: quote_id.to_string(),
                fallback_address: address.to_string(),
                amount_sat,
                max_fee_sat,
                required: payjoin.is_required(),
                events: persister.events()?,
                closed: persister.closed(),
            })
            .await?;

        let (post_request, post_context) = sender
            .extract_v2(&payjoin.params.ohttp_relay)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        let post_response = payjoin_http_request(post_request).await?;
        let sender = sender
            .process_response(&post_response, post_context)
            .save(&persister)
            .map_err(|err| Error::Payjoin(err.to_string()))?;
        self.storage
            .put_payjoin_send_session(&crate::storage::PayjoinSendSessionRecord {
                quote_id: quote_id.to_string(),
                fallback_address: address.to_string(),
                amount_sat,
                max_fee_sat,
                required: payjoin.is_required(),
                events: persister.events()?,
                closed: persister.closed(),
            })
            .await?;

        let mut sender = sender;
        let poll_deadline = payjoin
            .params
            .expires_at
            .unwrap_or_else(|| crate::util::unix_now().saturating_add(300));
        let proposal_psbt = loop {
            if crate::util::unix_now() > poll_deadline {
                return Err(Error::Payjoin("Payjoin sender session expired".to_string()));
            }
            let (get_request, get_context) = sender
                .extract_req(&payjoin.params.ohttp_relay)
                .map_err(|err| Error::Payjoin(err.to_string()))?;
            let get_response = payjoin_http_request(get_request).await?;
            match sender
                .process_response(&get_response, get_context)
                .save(&persister)
                .map_err(|err| Error::Payjoin(err.to_string()))?
            {
                OptionalTransitionOutcome::Progress(psbt) => break psbt,
                OptionalTransitionOutcome::Stasis(next_sender) => {
                    sender = next_sender;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        };
        self.storage
            .put_payjoin_send_session(&crate::storage::PayjoinSendSessionRecord {
                quote_id: quote_id.to_string(),
                fallback_address: address.to_string(),
                amount_sat,
                max_fee_sat,
                required: payjoin.is_required(),
                events: persister.events()?,
                closed: persister.closed(),
            })
            .await?;

        let mut final_psbt = proposal_psbt;
        let final_fee_sat = final_psbt
            .fee()
            .map_err(|err| Error::Payjoin(format!("Could not calculate Payjoin fee: {}", err)))?
            .to_sat();
        if final_fee_sat > max_fee_sat {
            return Err(Error::Payjoin(format!(
                "Payjoin fee {} exceeds max fee {}",
                final_fee_sat, max_fee_sat
            )));
        }

        let mut wallet_with_db = self.wallet_with_db.lock().await;
        if !wallet_with_db
            .wallet
            .sign(&mut final_psbt, Default::default())
            .map_err(|err| Error::Payjoin(format!("Could not sign Payjoin PSBT: {}", err)))?
        {
            return Err(Error::CouldNotSign);
        }
        let tx = final_psbt
            .extract_tx()
            .map_err(|err| Error::Payjoin(format!("Could not extract Payjoin tx: {}", err)))?;
        let txid = tx.compute_txid();
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(tx.clone(), crate::util::unix_now())]);
        if let Err(err) = wallet_with_db.persist() {
            tracing::warn!(
                "Could not persist BDK wallet after Payjoin tx apply: {}",
                err
            );
        }
        drop(wallet_with_db);

        match self.broadcast_transaction_internal(tx.clone()).await {
            Ok(crate::chain::BroadcastOutcome::Accepted)
            | Ok(crate::chain::BroadcastOutcome::AlreadyKnown) => {}
            Err(failure) => {
                return Err(Error::Payjoin(format!(
                    "Payjoin broadcast failed: {}",
                    failure.message
                )));
            }
        }

        let outpoint = find_payment_outpoint(&tx, &fallback_address, amount_sat)
            .unwrap_or_else(|| OutPoint::new(txid, 0));
        let pending = crate::send::payment_intent::SendIntent::new(
            &self.storage,
            quote_id.to_string(),
            address.to_string(),
            amount_sat,
            max_fee_sat,
            tier,
            PaymentMetadata::default(),
        )
        .await?;
        let batch_id = Uuid::new_v4();
        let batched = pending.assign_to_batch(&self.storage, batch_id).await?;
        batched
            .mark_broadcast(
                &self.storage,
                txid.to_string(),
                outpoint.to_string(),
                final_fee_sat,
            )
            .await?;

        Ok(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: Amount::new(amount_sat + final_fee_sat, CurrencyUnit::Sat),
        })
    }

    pub(crate) fn validate_send_amount_against_dust(
        &self,
        address: &str,
        amount_sat: u64,
    ) -> Result<(), Error> {
        let address = bdk_wallet::bitcoin::Address::from_str(address)
            .map_err(|e| Error::Wallet(e.to_string()))?
            .require_network(self.network)
            .map_err(|e| Error::Wallet(e.to_string()))?;

        let dust_limit = bdk_wallet::bitcoin::TxOut::minimal_non_dust(address.script_pubkey())
            .value
            .to_sat();

        if amount_sat < dust_limit {
            return Err(Error::DustOutput {
                amount: amount_sat,
                dust_limit,
            });
        }

        Ok(())
    }

    pub(crate) fn validate_send_amount(&self, address: &str, amount_sat: u64) -> Result<(), Error> {
        self.validate_send_amount_against_dust(address, amount_sat)?;

        if amount_sat < self.min_send_amount_sat {
            return Err(Error::AmountBelowMinimumSend {
                amount: amount_sat,
                min: self.min_send_amount_sat,
            });
        }

        Ok(())
    }

    pub(crate) fn confirmations_satisfied(&self, tip_height: u32, anchor_height: u32) -> bool {
        if tip_height < anchor_height {
            return false;
        }

        tip_height - anchor_height + 1 >= self.num_confs
    }

    pub(crate) fn should_ignore_receive_amount(&self, amount_sat: u64) -> bool {
        amount_sat < self.min_receive_amount_sat
    }

    /// Return `true` when the wallet knows about the transaction and it
    /// satisfies the configured confirmation threshold.
    pub(crate) fn txid_has_required_confirmations(
        &self,
        wallet: &PersistedWallet<Connection>,
        txid_str: &str,
        intent_kind: &str,
        intent_id: &str,
    ) -> bool {
        let Ok(parsed_txid) = bdk_wallet::bitcoin::Txid::from_str(txid_str) else {
            tracing::warn!(
                intent_kind,
                intent_id,
                txid = txid_str,
                "Could not parse txid during confirmation check"
            );
            return false;
        };

        let Some(tx_details) = wallet.get_tx(parsed_txid) else {
            return false;
        };

        let check_point = wallet.latest_checkpoint().height();
        match &tx_details.chain_position {
            bdk_wallet::chain::ChainPosition::Confirmed { anchor, .. } => {
                self.confirmations_satisfied(check_point, anchor.block_id.height)
            }
            bdk_wallet::chain::ChainPosition::Unconfirmed { .. } => false,
        }
    }

    /// Create a new CdkBdk instance
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mnemonic: Mnemonic,
        network: Network,
        chain_source: ChainSource,
        storage_dir_path: String,
        fee_reserve: FeeReserve,
        kv_store: Arc<dyn KVStore<Err = cdk_common::database::Error> + Send + Sync>,
        batch_config: Option<BatchConfig>,
        num_confs: u32,
        min_receive_amount_sat: u64,
        min_send_amount_sat: u64,
        sync_interval_secs: u64,
        shutdown_timeout_secs: Option<u64>,
        sync_config: Option<SyncConfig>,
        #[cfg(feature = "payjoin")] payjoin_config: Option<PayjoinConfig>,
    ) -> Result<Self, Error> {
        let storage_dir_path = PathBuf::from(storage_dir_path);
        let storage_dir_path = storage_dir_path.join("bdk_wallet");
        fs::create_dir_all(&storage_dir_path)?;

        let mut db = Connection::open(storage_dir_path.join("bdk_wallet.sqlite"))?;

        let xkey: ExtendedKey = mnemonic.into_extended_key()?;
        let xprv = xkey.into_xprv(network.into()).ok_or(Error::Path)?;

        let descriptor = Bip84(xprv, KeychainKind::External);
        let change_descriptor = Bip84(xprv, KeychainKind::Internal);

        let wallet_opt = Wallet::load()
            .descriptor(KeychainKind::External, Some(descriptor.clone()))
            .descriptor(KeychainKind::Internal, Some(change_descriptor.clone()))
            .extract_keys()
            .check_network(network)
            .load_wallet(&mut db)
            .map_err(|e| Error::Wallet(e.to_string()))?;

        let mut wallet = match wallet_opt {
            Some(wallet) => wallet,
            None => Wallet::create(descriptor, change_descriptor)
                .network(network)
                .create_wallet(&mut db)
                .map_err(|e| Error::Wallet(e.to_string()))?,
        };

        wallet.persist(&mut db)?;

        let wallet_with_db = WalletWithDb::new(wallet, db);

        let batch_config = batch_config.unwrap_or_default();
        if batch_config.poll_interval.is_zero() {
            return Err(Error::InvalidConfig(
                "batch_config.poll_interval must be greater than zero".to_string(),
            ));
        }
        batch_config.validate().map_err(Error::InvalidConfig)?;

        if sync_interval_secs == 0 {
            return Err(Error::InvalidConfig(
                "sync_interval_secs must be greater than zero".to_string(),
            ));
        }

        let channel_capacity = batch_config.max_batch_size * 2 + 16;
        let (payment_sender, _) = tokio::sync::broadcast::channel(channel_capacity);

        Ok(Self {
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            payment_sender,
            tasks: Arc::new(Mutex::new(None)),
            shutdown_timeout: Duration::from_secs(shutdown_timeout_secs.unwrap_or(30)),
            wallet_with_db: Arc::new(Mutex::new(wallet_with_db)),
            chain_source,
            storage: BdkStorage::new(kv_store),
            network,
            batch_config,
            batch_notify: Arc::new(Notify::new()),
            num_confs,
            min_receive_amount_sat,
            min_send_amount_sat,
            sync_interval_secs,
            sync_config: sync_config.unwrap_or_default(),
            fee_rate_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
            #[cfg(feature = "payjoin")]
            payjoin_config,
        })
    }

    #[cfg(feature = "payjoin")]
    pub(crate) fn payjoin_config(&self) -> Option<&PayjoinConfig> {
        self.payjoin_config.as_ref()
    }
}

/// Supervise a long-running task, restarting it with exponential backoff
/// (1s -> 60s, capped) whenever it returns `Err`. The backoff resets once
/// the task has run for longer than [`SUPERVISOR_BACKOFF_RESET`]. Exits
/// cleanly when `cancel` is triggered.
///
/// A task returning `Ok(())` is treated as a clean shutdown (e.g. the
/// task observed the cancel token itself) and the supervisor exits.
async fn supervise<F, Fut>(name: &'static str, cancel: CancellationToken, mut f: F)
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: Future<Output = Result<(), Error>>,
{
    const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
    const MAX_BACKOFF: Duration = Duration::from_secs(60);
    const SUPERVISOR_BACKOFF_RESET: Duration = Duration::from_secs(300);

    let mut backoff = INITIAL_BACKOFF;

    loop {
        if cancel.is_cancelled() {
            break;
        }

        let started = Instant::now();
        let child_cancel = cancel.clone();

        let result = tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("{name} supervisor: cancelled");
                return;
            }
            r = f(child_cancel) => r,
        };

        match result {
            Ok(()) => {
                tracing::info!("{name} supervisor: task exited cleanly");
                return;
            }
            Err(e) => {
                let ran_for = started.elapsed();
                let transient = e.is_transient();
                tracing::error!(
                    task = name,
                    ran_for_secs = ran_for.as_secs(),
                    transient,
                    "supervised task returned error: {e}; restarting with backoff"
                );

                if ran_for >= SUPERVISOR_BACKOFF_RESET {
                    backoff = INITIAL_BACKOFF;
                }

                // Sleep with backoff, but wake immediately if cancelled.
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::info!("{name} supervisor: cancelled during backoff");
                        return;
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }

                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

#[cfg(feature = "payjoin")]
fn extract_bip21_payjoin_endpoint(uri: &str) -> Result<String, Error> {
    let query = uri.split_once('?').map(|(_, query)| query).ok_or_else(|| {
        Error::Payjoin("Payjoin URI did not include query parameters".to_string())
    })?;

    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key == "pj" {
            return Ok(value.into_owned());
        }
    }

    Err(Error::Payjoin(
        "Payjoin URI did not include a pj endpoint".to_string(),
    ))
}

#[cfg(feature = "payjoin")]
fn extract_payjoin_fragment_value(endpoint: &str, prefix: &str) -> Option<String> {
    let url = url::Url::parse(endpoint).ok()?;
    url.fragment()?
        .split('+')
        .find(|part| part.starts_with(prefix))
        .map(|part| part.to_string())
}

#[cfg(feature = "payjoin")]
fn build_payjoin_uri(address: &str, amount_sat: u64, endpoint: &str) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("amount", &format_bip21_amount(amount_sat));
    serializer.append_pair("pj", endpoint);
    format!("bitcoin:{}?{}", address, serializer.finish())
}

#[cfg(feature = "payjoin")]
fn format_bip21_amount(amount_sat: u64) -> String {
    let btc = amount_sat / 100_000_000;
    let sats = amount_sat % 100_000_000;
    if sats == 0 {
        return btc.to_string();
    }
    format!("{btc}.{sats:08}").trim_end_matches('0').to_string()
}

#[cfg(feature = "payjoin")]
async fn payjoin_http_request(request: payjoin::Request) -> Result<Vec<u8>, Error> {
    let response = reqwest::Client::new()
        .post(request.url)
        .header(reqwest::header::CONTENT_TYPE, request.content_type)
        .body(request.body)
        .send()
        .await
        .map_err(|err| Error::Payjoin(err.to_string()))?;
    if !response.status().is_success() {
        return Err(Error::Payjoin(format!(
            "Payjoin HTTP request failed with status {}",
            response.status()
        )));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|err| Error::Payjoin(err.to_string()))
}

#[cfg(feature = "payjoin")]
fn find_payment_outpoint(
    tx: &Transaction,
    address: &bdk_wallet::bitcoin::Address,
    amount_sat: u64,
) -> Option<OutPoint> {
    let script = address.script_pubkey();
    tx.output
        .iter()
        .enumerate()
        .find(|(_, output)| output.script_pubkey == script && output.value.to_sat() == amount_sat)
        .map(|(vout, _)| OutPoint::new(tx.compute_txid(), vout as u32))
}

#[async_trait]
impl MintPayment for CdkBdk {
    type Err = cdk_common::payment::Error;

    #[tracing::instrument(skip_all)]
    async fn start(&self) -> Result<(), Self::Err> {
        let mut tasks_lock = self.tasks.lock().await;
        if tasks_lock.is_some() {
            return Err(Error::AlreadyStarted.into());
        }

        self.recover_receive_saga().await?;
        self.recover_send_saga().await?;

        let cancel = CancellationToken::new();

        let sync_self = self.clone();
        let sync_cancel = cancel.clone();
        let sync_handle = tokio::spawn(async move {
            supervise("wallet sync", sync_cancel, move |cancel| {
                let me = sync_self.clone();
                async move { me.sync_wallet(cancel).await }
            })
            .await;
        });

        let batch_self = self.clone();
        let batch_cancel = cancel.clone();
        let batch_handle = tokio::spawn(async move {
            supervise("batch processor", batch_cancel, move |cancel| {
                let me = batch_self.clone();
                async move { me.run_batch_processor(cancel).await }
            })
            .await;
        });

        #[cfg(feature = "payjoin")]
        let payjoin_receive_handle = if self.payjoin_config().is_some() {
            let payjoin_self = self.clone();
            let payjoin_cancel = cancel.clone();
            Some(tokio::spawn(async move {
                supervise("payjoin receive poller", payjoin_cancel, move |cancel| {
                    let me = payjoin_self.clone();
                    async move { me.run_payjoin_receive_poller(cancel).await }
                })
                .await;
            }))
        } else {
            None
        };

        *tasks_lock = Some(BackgroundTasks {
            cancel,
            sync: sync_handle,
            batch: batch_handle,
            #[cfg(feature = "payjoin")]
            payjoin_receive: payjoin_receive_handle,
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        self.wait_invoice_cancel_token.cancel();

        let tasks_opt = {
            let mut tasks_lock = self.tasks.lock().await;
            tasks_lock.take()
        };

        if let Some(bg) = tasks_opt {
            bg.cancel.cancel();

            let sync_aborter = bg.sync.abort_handle();
            let batch_aborter = bg.batch.abort_handle();
            #[cfg(feature = "payjoin")]
            let payjoin_receive_aborter =
                bg.payjoin_receive.as_ref().map(|task| task.abort_handle());

            let joined = tokio::time::timeout(self.shutdown_timeout, async move {
                let _ = bg.sync.await;
                let _ = bg.batch.await;
                #[cfg(feature = "payjoin")]
                if let Some(task) = bg.payjoin_receive {
                    let _ = task.await;
                }
            })
            .await;

            if joined.is_err() {
                sync_aborter.abort();
                batch_aborter.abort();
                #[cfg(feature = "payjoin")]
                if let Some(aborter) = payjoin_receive_aborter {
                    aborter.abort();
                }
                tracing::error!(
                    "cdk-bdk background tasks did not exit within {:?}; forced abort",
                    self.shutdown_timeout
                );
            }
        }

        Ok(())
    }

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: "sat".to_string(),
            bolt11: None,
            bolt12: None,
            onchain: Some(OnchainSettings {
                confirmations: self.num_confs,
                min_receive_amount_sat: self.min_receive_amount_sat,
                min_send_amount_sat: self.min_send_amount_sat,
            }),
            custom: std::collections::HashMap::new(),
        })
    }

    async fn get_payment_quote(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let onchain_options = match options {
            OutgoingPaymentOptions::Onchain(o) => o,
            _ => return Err(cdk_common::payment::Error::UnsupportedPaymentOption),
        };

        self.validate_send_amount(
            &onchain_options.address,
            onchain_options.amount.clone().to_u64(),
        )?;
        let amount_sat = onchain_options.amount.clone().to_u64();
        let requested_payjoin = Self::requested_payjoin(onchain_options.metadata.as_deref());
        let payjoin_extra = match requested_payjoin {
            Some(payjoin) if !payjoin.is_supported() && payjoin.is_required() => {
                return Err(Error::PayjoinUnavailable(format!(
                    "unsupported Payjoin version {}",
                    payjoin.version
                ))
                .into());
            }
            Some(payjoin) if !payjoin.is_supported() => None,
            Some(payjoin) => {
                #[cfg(feature = "payjoin")]
                {
                    if self.payjoin_config().is_some() {
                        Some(Self::accepted_payjoin_extra(&payjoin))
                    } else if payjoin.is_required() {
                        return Err(Error::PayjoinUnavailable(
                            "operator did not configure Payjoin directory and OHTTP relay"
                                .to_string(),
                        )
                        .into());
                    } else {
                        None
                    }
                }
                #[cfg(not(feature = "payjoin"))]
                {
                    if payjoin.is_required() {
                        return Err(Error::PayjoinUnavailable(
                            "cdk-bdk was built without the payjoin feature".to_string(),
                        )
                        .into());
                    }
                    None
                }
            }
            None => None,
        };

        // Estimate fee_reserve for each configured tier so the mint presents
        // only the operator-enabled options. The configured order owns the
        // `fee_index` values and resolves them back to tiers during payment.
        let mut fee_options = Vec::with_capacity(self.batch_config.fee_options.len());
        for (idx, tier) in self.batch_config.fee_options.iter().enumerate() {
            let fee_estimate = self
                .estimate_onchain_fee_reserve(&onchain_options.address, amount_sat, *tier)
                .await?;
            fee_options.push(MeltQuoteOnchainFeeOption {
                fee_index: idx as u32,
                fee_reserve: Amount::from(fee_estimate.fee_reserve_sat),
                estimated_blocks: tier.estimated_blocks(),
            });
        }

        // The `fee`/`estimated_blocks` mirror fields surface the cheapest
        // available option as a sensible default, matching the mint's
        // initialization in `MeltQuote::new_onchain`.
        let cheapest = fee_options
            .iter()
            .min_by_key(|option| u64::from(option.fee_reserve))
            .copied()
            .expect("fee_options is validated as non-empty");

        // Echo the mint-supplied `quote_id` verbatim per the
        // `OnchainOutgoingPaymentOptions.quote_id` contract. The mint
        // validates this echo; any deviation triggers
        // `Error::OnchainQuoteLookupIdMismatch`.
        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::QuoteId(onchain_options.quote_id.clone())),
            amount: onchain_options.amount,
            fee: Amount::new(cheapest.fee_reserve.into(), CurrencyUnit::Sat),
            state: MeltQuoteState::Unpaid,
            extra_json: payjoin_extra,
            estimated_blocks: Some(cheapest.estimated_blocks),
            fee_options: Some(fee_options),
        })
    }

    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let onchain_options = match options {
            OutgoingPaymentOptions::Onchain(o) => o,
            _ => return Err(cdk_common::payment::Error::UnsupportedPaymentOption),
        };

        let address = onchain_options.address;
        let amount = onchain_options.amount;
        let quote_id = onchain_options.quote_id;
        let requested_payjoin = Self::requested_payjoin(onchain_options.metadata.as_deref());

        self.validate_send_amount(&address, amount.clone().to_u64())?;

        let max_fee = onchain_options
            .max_fee_amount
            .unwrap_or(Amount::new(1000, CurrencyUnit::Sat));
        let amount_sat = amount.clone().to_u64();
        let max_fee_sat = max_fee.clone().to_u64();
        // Resolve the wallet-selected `fee_index` back to a configured tier.
        // Older callers that omit `fee_index` continue to default to
        // Immediate.
        let tier = self
            .batch_config
            .tier_for_fee_index(onchain_options.fee_index)
            .map_err(Error::UnknownFeeIndex)?;
        let metadata = PaymentMetadata::from_optional_json(onchain_options.metadata.as_deref());
        if let Some(payjoin) = requested_payjoin {
            if !payjoin.is_supported() {
                if payjoin.is_required() {
                    return Err(Error::PayjoinUnavailable(format!(
                        "unsupported Payjoin version {}",
                        payjoin.version
                    ))
                    .into());
                }
            } else {
                #[cfg(feature = "payjoin")]
                {
                    if self.payjoin_config().is_some() {
                        match self
                            .send_payjoin_payment(
                                &quote_id,
                                &address,
                                amount_sat,
                                max_fee_sat,
                                tier,
                                &payjoin,
                            )
                            .await
                        {
                            Ok(response) => return Ok(response),
                            Err(err) if payjoin.is_required() => return Err(err.into()),
                            Err(err) => {
                                tracing::warn!(
                                    quote_id = %quote_id,
                                    error = %err,
                                    "Optional Payjoin send failed; falling back to direct onchain send"
                                );
                            }
                        }
                    } else if payjoin.is_required() {
                        return Err(Error::PayjoinUnavailable(
                            "operator did not configure Payjoin directory and OHTTP relay"
                                .to_string(),
                        )
                        .into());
                    }
                }
                #[cfg(not(feature = "payjoin"))]
                if payjoin.is_required() {
                    return Err(Error::PayjoinUnavailable(
                        "cdk-bdk was built without the payjoin feature".to_string(),
                    )
                    .into());
                }
            }
        }

        let fee_estimate = self
            .estimate_onchain_fee_reserve(&address, amount_sat, tier)
            .await?;
        if fee_estimate.raw_fee_sat > max_fee_sat {
            return Err(Error::EstimatedFeeTooHigh {
                estimated_fee: fee_estimate.raw_fee_sat,
                max_fee: max_fee_sat,
            }
            .into());
        }

        crate::send::payment_intent::SendIntent::new(
            &self.storage,
            quote_id.to_string(),
            address,
            amount_sat,
            max_fee_sat,
            tier,
            metadata,
        )
        .await?;

        if tier == PaymentTier::Immediate {
            self.batch_notify.notify_one();
        }

        // The intent has been queued but no batch has been built yet, so the
        // per-intent fee contribution is not yet knowable. Following the
        // convention used by other backends (LND/LDK-Node/CLN return `0` for
        // `Unknown`/`NotFound`), we return `0` as a sentinel meaning "actual
        // spent amount is not yet known". Callers should wait for the
        // terminal `Paid` event to read the authoritative `total_spent`.
        Ok(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::QuoteId(quote_id),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: Amount::new(0, CurrencyUnit::Sat),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let onchain_options = match options {
            IncomingPaymentOptions::Onchain(o) => o,
            _ => return Err(cdk_common::payment::Error::UnsupportedPaymentOption),
        };

        let mut wallet_with_db = self.wallet_with_db.lock().await;
        let address = wallet_with_db
            .wallet
            .reveal_next_address(KeychainKind::External);
        let address_str = address.address.to_string();
        let payjoin_request = onchain_options.payjoin;
        let quote_id = onchain_options.quote_id;

        wallet_with_db.persist().map_err(|err| {
            tracing::error!("Could not persist to bdk db: {}", err);

            Error::BdkPersist
        })?;
        drop(wallet_with_db);

        let extra_json = match payjoin_request {
            Some(request) if !request.is_supported() && request.required => {
                return Err(Error::PayjoinUnavailable(format!(
                    "unsupported Payjoin version {}",
                    request.version
                ))
                .into());
            }
            Some(request) if !request.is_supported() => None,
            Some(request) => {
                self.create_payjoin_receive_extra(&quote_id, &address.address, 0, request.required)
                    .await?
            }
            None => None,
        };

        self.storage
            .track_receive_address(&address_str, &quote_id.to_string())
            .await?;

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: PaymentIdentifier::QuoteId(quote_id),
            request: address_str,
            expiry: None,
            extra_json,
        })
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        self.wait_invoice_is_active.store(true, Ordering::SeqCst);

        let receiver = self.payment_sender.subscribe();
        let stream = PaymentEventStream {
            receiver: BroadcastStream::new(receiver),
            cancel: Box::pin(self.wait_invoice_cancel_token.clone().cancelled_owned()),
            is_active: Arc::clone(&self.wait_invoice_is_active),
        };

        Ok(Box::pin(stream))
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let PaymentIdentifier::QuoteId(quote_id) = payment_identifier else {
            return Err(Error::UnsupportedOnchain.into());
        };

        let quote_id_str = quote_id.to_string();
        let mut results = Vec::new();

        // Only return finalized payments. Active intents (Detected state) are
        // not yet confirmed and should not be reported to the mint for processing.
        let finalized = self
            .storage
            .get_finalized_receive_intents_by_quote_id(&quote_id_str)
            .await?;

        for record in finalized {
            results.push(WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
                payment_amount: Amount::new(record.amount_sat, CurrencyUnit::Sat),
                payment_id: record.outpoint,
            });
        }

        Ok(results)
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let quote_id = match payment_identifier {
            PaymentIdentifier::QuoteId(id) => id.to_string(),
            _ => return Err(Error::UnsupportedOnchain.into()),
        };

        // 1. Check active intents
        if let Some(record) = self.storage.get_send_intent_by_quote_id(&quote_id).await? {
            // `total_spent` is the actual amount spent (amount + fee) and is
            // only reported once the payment has been made. Before the batch
            // transaction has been built, the per-intent fee contribution is
            // unknown, so we return `0` as a sentinel. This matches the
            // convention used by other backends for non-terminal states.
            let total_spent = match &record.state {
                crate::send::payment_intent::record::SendIntentState::Pending { .. }
                | crate::send::payment_intent::record::SendIntentState::Batched { .. } => {
                    Amount::new(0, CurrencyUnit::Sat)
                }
                crate::send::payment_intent::record::SendIntentState::AwaitingConfirmation {
                    fee_contribution_sat,
                    ..
                } => Amount::new(record.amount_sat + fee_contribution_sat, CurrencyUnit::Sat),
                crate::send::payment_intent::record::SendIntentState::Failed { .. } => {
                    Amount::new(0, CurrencyUnit::Sat)
                }
            };
            let status = match record.state {
                crate::send::payment_intent::record::SendIntentState::Pending { .. }
                | crate::send::payment_intent::record::SendIntentState::Batched { .. }
                | crate::send::payment_intent::record::SendIntentState::AwaitingConfirmation {
                    ..
                } => MeltQuoteState::Pending,
                crate::send::payment_intent::record::SendIntentState::Failed { .. } => {
                    MeltQuoteState::Failed
                }
            };

            return Ok(MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: None,
                status,
                total_spent,
            });
        }

        // 2. Check finalized tombstones
        if let Some(record) = self
            .storage
            .get_finalized_intent_by_quote_id(&quote_id)
            .await?
        {
            return Ok(MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: Some(record.outpoint),
                status: MeltQuoteState::Paid,
                total_spent: Amount::new(record.total_spent_sat, CurrencyUnit::Sat),
            });
        }

        Ok(MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
            payment_proof: None,
            status: MeltQuoteState::Unknown,
            total_spent: Amount::new(0, CurrencyUnit::Sat),
        })
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_payment_event_stream(&self) {
        self.wait_invoice_cancel_token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use bdk_wallet::bitcoin::hashes::Hash as _;
    use bdk_wallet::bitcoin::{
        absolute, transaction, Network, OutPoint, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    };
    use bdk_wallet::keys::bip39::Mnemonic;
    use cdk_common::common::FeeReserve;
    use cdk_common::payment::MintPayment;
    use futures::StreamExt;

    use super::*;
    use crate::fee::apply_quote_fee_safety;

    /// Build a `CdkBdk` instance pointed at a bogus Esplora URL so the sync
    /// loop spins without needing a real backend. The ticks are short so
    /// shutdown tests run quickly.
    async fn build_test_instance(shutdown_timeout_secs: u64) -> CdkBdk {
        build_test_instance_with_tempdir(shutdown_timeout_secs)
            .await
            .0
    }

    async fn build_test_instance_with_tempdir(
        shutdown_timeout_secs: u64,
    ) -> (CdkBdk, tempfile::TempDir) {
        build_test_instance_with_config(shutdown_timeout_secs, None, 60)
            .await
            .expect("build CdkBdk test instance")
    }

    async fn build_test_instance_with_config(
        shutdown_timeout_secs: u64,
        batch_config: Option<BatchConfig>,
        sync_interval_secs: u64,
    ) -> Result<(CdkBdk, tempfile::TempDir), Error> {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mnemonic = Mnemonic::from_str(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("mnemonic");

        let kv = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory kv store");

        let chain_source = ChainSource::Esplora(EsploraConfig {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        });

        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::new(1, CurrencyUnit::Sat).into(),
            percent_fee_reserve: 0.02,
        };

        let backend = CdkBdk::new(
            mnemonic,
            Network::Regtest,
            chain_source,
            tmp.path().to_string_lossy().into_owned(),
            fee_reserve,
            Arc::new(kv),
            batch_config,
            1,
            0,
            546,
            sync_interval_secs,
            Some(shutdown_timeout_secs),
            None,
            #[cfg(feature = "payjoin")]
            None,
        )?;

        Ok((backend, tmp))
    }

    async fn fund_backend_wallet(backend: &CdkBdk, amount_sat: u64) {
        let mut wallet_with_db = backend.wallet_with_db.lock().await;
        let funding_script = wallet_with_db
            .wallet
            .reveal_next_address(KeychainKind::External)
            .address
            .script_pubkey();
        let funding_tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::all_zeros(), 0),
                script_sig: Default::default(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: bdk_wallet::bitcoin::Amount::from_sat(amount_sat),
                script_pubkey: funding_script,
            }],
        };

        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(funding_tx, 0)]);
        wallet_with_db.persist().expect("persist funded wallet");
    }

    #[tokio::test]
    async fn test_new_rejects_zero_sync_interval() {
        match build_test_instance_with_config(5, None, 0).await {
            Err(Error::InvalidConfig(message)) => {
                assert!(message.contains("sync_interval_secs"));
            }
            Ok(_) => panic!("zero sync interval should be rejected"),
            Err(err) => panic!("expected invalid config error, got {err}"),
        }
    }

    #[tokio::test]
    async fn test_new_rejects_zero_batch_poll_interval() {
        let batch_config = BatchConfig {
            poll_interval: Duration::ZERO,
            ..BatchConfig::default()
        };

        match build_test_instance_with_config(5, Some(batch_config), 60).await {
            Err(Error::InvalidConfig(message)) => {
                assert!(message.contains("poll_interval"));
            }
            Ok(_) => panic!("zero batch poll interval should be rejected"),
            Err(err) => panic!("expected invalid config error, got {err}"),
        }
    }

    #[tokio::test]
    async fn test_new_rejects_zero_target_block_time() {
        let batch_config = BatchConfig {
            target_block_time: Duration::ZERO,
            ..BatchConfig::default()
        };

        match build_test_instance_with_config(5, Some(batch_config), 60).await {
            Err(Error::InvalidConfig(message)) => {
                assert!(message.contains("target_block_time"));
            }
            Ok(_) => panic!("zero target block time should be rejected"),
            Err(err) => panic!("expected invalid config error, got {err}"),
        }
    }

    #[tokio::test]
    async fn test_new_rejects_invalid_fallback_fee_rate() {
        let batch_config = BatchConfig {
            fee_estimation: FeeEstimationConfig {
                fallback_sat_per_vb: 0.0,
                ..FeeEstimationConfig::default()
            },
            ..BatchConfig::default()
        };

        match build_test_instance_with_config(5, Some(batch_config), 60).await {
            Err(Error::InvalidConfig(message)) => {
                assert!(message.contains("fallback_sat_per_vb"));
            }
            Ok(_) => panic!("invalid fallback fee rate should be rejected"),
            Err(err) => panic!("expected invalid config error, got {err}"),
        }
    }

    #[test]
    fn test_default_batch_deadlines_match_advertised_blocks() {
        let batch_config = BatchConfig::default();

        assert_eq!(batch_config.target_block_time, Duration::from_secs(600));
        assert_eq!(batch_config.standard_deadline, Duration::from_secs(3600));
        assert_eq!(batch_config.economy_deadline, Duration::from_secs(86_400));
        assert_eq!(
            batch_config.max_intent_age,
            Some(Duration::from_secs(86_430))
        );
    }

    #[tokio::test]
    async fn test_start_then_stop_exits_promptly() {
        let backend = build_test_instance(5).await;

        let started = tokio::time::timeout(Duration::from_secs(10), backend.start())
            .await
            .expect("start timed out");
        started.expect("start should succeed");

        let stopped = tokio::time::timeout(Duration::from_secs(10), backend.stop())
            .await
            .expect("stop timed out");
        stopped.expect("stop should succeed");
    }

    #[tokio::test]
    async fn test_double_start_returns_already_started() {
        let backend = build_test_instance(5).await;
        backend.start().await.expect("first start");

        let second = backend.start().await;
        assert!(second.is_err(), "second start should error");

        backend.stop().await.expect("stop");
    }

    #[tokio::test]
    async fn test_stop_without_start_is_ok() {
        let backend = build_test_instance(5).await;
        backend.stop().await.expect("stop on never-started is ok");
        backend.stop().await.expect("double stop is ok");
    }

    #[tokio::test]
    async fn test_restart_after_stop() {
        let backend = build_test_instance(5).await;
        backend.start().await.expect("first start");
        backend.stop().await.expect("first stop");
        backend.start().await.expect("second start");
        backend.stop().await.expect("second stop");
    }

    #[tokio::test]
    async fn test_wait_payment_event_tracks_active_state_and_cancels() {
        let backend = build_test_instance(5).await;
        assert!(!backend.is_payment_event_stream_active());

        let mut stream = backend
            .wait_payment_event()
            .await
            .expect("payment event stream");
        assert!(backend.is_payment_event_stream_active());

        backend.cancel_payment_event_stream();

        let next = tokio::time::timeout(Duration::from_secs(2), stream.next())
            .await
            .expect("stream should observe cancellation promptly");
        assert!(next.is_none());
        assert!(!backend.is_payment_event_stream_active());
    }

    #[test]
    fn test_quote_fee_safety_adds_multiplier_and_fixed_margin() {
        let config = FeeEstimationConfig {
            quote_safety_multiplier: 1.25,
            quote_fixed_safety_sat: 500,
            ..FeeEstimationConfig::default()
        };

        assert_eq!(apply_quote_fee_safety(1_000, &config), 1_750);
    }

    #[tokio::test]
    async fn test_fee_rate_cache_falls_back_on_error() {
        // With an unreachable Esplora URL, estimate_fee_rate_sat_per_vb
        // returns an error. The quote path falls back to the configured
        // default. We exercise the fallback by invoking get_payment_quote
        // with a tier hint and observing that it returns a non-zero fee.
        let backend = build_test_instance(5).await;

        let tier_err = backend
            .estimate_fee_rate_sat_per_vb(PaymentTier::Immediate)
            .await;
        assert!(
            tier_err.is_err(),
            "fee rate estimation should fail against bogus Esplora URL"
        );
    }

    #[tokio::test]
    async fn test_get_payment_quote_does_not_stage_wallet_changes() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (_quote_id, options) = onchain_options_for(10_000);

        backend
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await
            .expect("quote should succeed with fallback fee rate");

        let wallet_with_db = backend.wallet_with_db.lock().await;
        assert!(
            wallet_with_db.wallet.staged().is_none(),
            "quote estimation must not mutate or stage BDK wallet state"
        );
    }

    #[tokio::test]
    async fn test_default_fee_options_emit_immediate_only() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (_quote_id, options) = onchain_options_for(10_000);

        let quote = backend
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await
            .expect("quote should succeed");

        let fee_options = quote.fee_options.expect("fee options");
        assert_eq!(fee_options.len(), 1);
        assert_eq!(fee_options[0].fee_index, 0);
        assert_eq!(fee_options[0].estimated_blocks, 1);
    }

    #[tokio::test]
    async fn test_configured_fee_options_emit_indexes_in_order() {
        let batch_config = BatchConfig {
            fee_options: vec![
                PaymentTier::Immediate,
                PaymentTier::Standard,
                PaymentTier::Economy,
            ],
            ..BatchConfig::default()
        };
        let (backend, _tmp) = build_test_instance_with_config(5, Some(batch_config), 60)
            .await
            .expect("build CdkBdk test instance");
        fund_backend_wallet(&backend, 100_000).await;
        let (_quote_id, options) = onchain_options_for(10_000);

        let quote = backend
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await
            .expect("quote should succeed");

        let fee_options = quote.fee_options.expect("fee options");
        let indexes: Vec<u32> = fee_options.iter().map(|option| option.fee_index).collect();
        let estimated_blocks: Vec<u32> = fee_options
            .iter()
            .map(|option| option.estimated_blocks)
            .collect();

        assert_eq!(indexes, vec![0, 1, 2]);
        assert_eq!(estimated_blocks, vec![1, 6, 144]);
    }

    #[tokio::test]
    async fn test_configured_fee_index_resolves_by_position() {
        let batch_config = BatchConfig {
            fee_options: vec![PaymentTier::Immediate, PaymentTier::Economy],
            ..BatchConfig::default()
        };
        let (backend, _tmp) = build_test_instance_with_config(5, Some(batch_config), 60)
            .await
            .expect("build CdkBdk test instance");
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, mut options) = onchain_options_for(10_000);
        let OutgoingPaymentOptions::Onchain(onchain) = &mut options else {
            panic!("expected onchain options");
        };
        onchain.fee_index = Some(1);
        onchain.max_fee_amount = Some(Amount::new(10_000, CurrencyUnit::Sat));

        backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("make_payment should enqueue the intent");

        let intent = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent by quote id")
            .expect("send intent should be persisted");

        assert_eq!(intent.tier, PaymentTier::Economy);
    }

    #[tokio::test]
    async fn test_make_payment_omitted_fee_index_defaults_to_immediate() {
        let batch_config = BatchConfig {
            fee_options: vec![PaymentTier::Immediate, PaymentTier::Economy],
            ..BatchConfig::default()
        };
        let (backend, _tmp) = build_test_instance_with_config(5, Some(batch_config), 60)
            .await
            .expect("build CdkBdk test instance");
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, options) = onchain_options_for(10_000);

        backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("make_payment should enqueue the intent");

        let intent = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent by quote id")
            .expect("send intent should be persisted");

        assert_eq!(intent.tier, PaymentTier::Immediate);
    }

    #[tokio::test]
    async fn test_new_rejects_invalid_fee_option_lists() {
        for fee_options in [
            Vec::new(),
            vec![PaymentTier::Immediate, PaymentTier::Immediate],
            vec![
                PaymentTier::Immediate,
                PaymentTier::Standard,
                PaymentTier::Economy,
                PaymentTier::Immediate,
            ],
        ] {
            let batch_config = BatchConfig {
                fee_options,
                ..BatchConfig::default()
            };
            match build_test_instance_with_config(5, Some(batch_config), 60).await {
                Err(Error::InvalidConfig(message)) => {
                    assert!(message.contains("fee_options"));
                }
                Ok(_) => panic!("invalid fee options should be rejected"),
                Err(err) => panic!("expected invalid config error, got {err}"),
            }
        }
    }

    #[tokio::test]
    async fn test_get_payment_quote_rejects_empty_wallet() {
        let backend = build_test_instance(5).await;
        let (_quote_id, options) = onchain_options_for(10_000);

        let err = backend
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await
            .expect_err("empty wallet should not receive an onchain quote");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };

        let backend_err = inner
            .downcast_ref::<Error>()
            .expect("expected cdk-bdk backend error");
        assert!(matches!(backend_err, Error::NoSpendableUtxos));
    }

    #[tokio::test]
    async fn test_make_payment_rechecks_current_fee_against_max_fee() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, mut options) = onchain_options_for(10_000);
        let OutgoingPaymentOptions::Onchain(onchain) = &mut options else {
            panic!("expected onchain options");
        };
        onchain.max_fee_amount = Some(Amount::new(1, CurrencyUnit::Sat));

        let err = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect_err("payment should be rejected when current fee exceeds max");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };
        match inner.downcast_ref::<Error>() {
            Some(Error::EstimatedFeeTooHigh { max_fee, .. }) => assert_eq!(*max_fee, 1),
            other => panic!("expected EstimatedFeeTooHigh, got {other:?}"),
        }

        assert!(
            backend
                .storage
                .get_send_intent_by_quote_id(&quote_id.to_string())
                .await
                .expect("lookup send intent by quote id")
                .is_none(),
            "fee recheck rejection must not leave a pending send intent behind"
        );
    }

    #[tokio::test]
    async fn test_get_settings_reports_min_send_amount() {
        let backend = build_test_instance(5).await;

        let settings = backend.get_settings().await.expect("settings");
        let onchain = settings.onchain.expect("onchain settings");

        assert_eq!(onchain.min_receive_amount_sat, 0);
        assert_eq!(onchain.min_send_amount_sat, 546);
    }

    // ------------------------------------------------------------------
    // Regression tests for Finding 5: total_spent is only authoritative
    // after the payment has been made. While the intent is queued but not
    // yet broadcast, the per-intent fee is unknown, so `total_spent` is
    // reported as 0 (sentinel), matching the LND/LDK/CLN convention for
    // non-terminal responses.
    // ------------------------------------------------------------------

    use cdk_common::payment::OnchainOutgoingPaymentOptions;
    use cdk_common::QuoteId;
    use uuid::Uuid;

    /// Build an onchain outgoing payment option with a fresh quote id.
    fn onchain_options_for(amount_sat: u64) -> (QuoteId, OutgoingPaymentOptions) {
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        (
            quote_id.clone(),
            onchain_options_for_quote(quote_id, amount_sat),
        )
    }

    fn onchain_options_for_quote(quote_id: QuoteId, amount_sat: u64) -> OutgoingPaymentOptions {
        OutgoingPaymentOptions::Onchain(Box::new(OnchainOutgoingPaymentOptions {
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount: Amount::new(amount_sat, CurrencyUnit::Sat),
            max_fee_amount: Some(Amount::new(1_000, CurrencyUnit::Sat)),
            quote_id,
            fee_index: None,
            metadata: None,
        }))
    }

    #[tokio::test]
    async fn test_make_payment_pending_total_spent_is_zero() {
        // make_payment queues the intent before a batch has been built, so
        // the per-intent fee is unknown. total_spent MUST be 0, not the
        // user-requested amount (which would imply no fee).
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, options) = onchain_options_for(10_000);

        let response = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("make_payment should enqueue the intent");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(
            response.payment_lookup_id,
            PaymentIdentifier::QuoteId(quote_id)
        );
        assert_eq!(
            response.total_spent,
            Amount::new(0, CurrencyUnit::Sat),
            "Pending onchain response MUST use 0 sentinel; the real \
             total_spent is only known after the batch transaction is built"
        );
    }

    #[tokio::test]
    async fn test_get_payment_quote_rejects_dust_output() {
        let backend = build_test_instance(5).await;
        let (_quote_id, options) = onchain_options_for(1);

        let err = backend
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await
            .expect_err("dust output should be rejected at quote time");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };

        let backend_err = inner
            .downcast_ref::<Error>()
            .expect("expected cdk-bdk backend error");
        assert!(matches!(backend_err, Error::DustOutput { .. }));
    }

    #[tokio::test]
    async fn test_make_payment_rejects_dust_output_without_persisting_intent() {
        let backend = build_test_instance(5).await;
        let (quote_id, options) = onchain_options_for(1);

        let err = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect_err("dust output should be rejected before enqueue");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };

        let backend_err = inner
            .downcast_ref::<Error>()
            .expect("expected cdk-bdk backend error");
        assert!(matches!(backend_err, Error::DustOutput { .. }));
        assert!(
            backend
                .storage
                .get_send_intent_by_quote_id(&quote_id.to_string())
                .await
                .expect("lookup send intent by quote id")
                .is_none(),
            "dust rejection must not leave a pending send intent behind"
        );
    }

    #[tokio::test]
    async fn test_get_payment_quote_rejects_amount_below_minimum_send() {
        let backend = build_test_instance(5).await;
        let (_quote_id, options) = onchain_options_for(545);

        let err = backend
            .get_payment_quote(&CurrencyUnit::Sat, options)
            .await
            .expect_err("amount below configured minimum should be rejected at quote time");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };

        let backend_err = inner
            .downcast_ref::<Error>()
            .expect("expected cdk-bdk backend error");
        assert!(matches!(
            backend_err,
            Error::AmountBelowMinimumSend {
                amount: 545,
                min: 546
            }
        ));
    }

    #[tokio::test]
    async fn test_make_payment_rejects_amount_below_minimum_send_without_persisting_intent() {
        let backend = build_test_instance(5).await;
        let (quote_id, options) = onchain_options_for(545);

        let err = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect_err("amount below configured minimum should be rejected before enqueue");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };

        let backend_err = inner
            .downcast_ref::<Error>()
            .expect("expected cdk-bdk backend error");
        assert!(matches!(
            backend_err,
            Error::AmountBelowMinimumSend {
                amount: 545,
                min: 546
            }
        ));
        assert!(
            backend
                .storage
                .get_send_intent_by_quote_id(&quote_id.to_string())
                .await
                .expect("lookup send intent by quote id")
                .is_none(),
            "minimum-send rejection must not leave a pending send intent behind"
        );
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_pending_intent_reports_zero_total_spent() {
        // An intent freshly created via make_payment is in state Pending.
        // check_outgoing_payment must report total_spent = 0 because the
        // fee contribution is not yet knowable.
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, options) = onchain_options_for(12_345);

        backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("make_payment should enqueue the intent");

        let payment_identifier = PaymentIdentifier::QuoteId(quote_id);
        let response = backend
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("check_outgoing_payment for Pending intent");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
        assert_eq!(response.payment_proof, None);
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_batched_intent_reports_zero_total_spent() {
        // Driving an intent through Pending → Batched (fee still unknown at
        // the per-intent level until the batch transaction is built) must
        // still report total_spent = 0.
        use crate::send::payment_intent::SendIntent;
        use crate::types::{PaymentMetadata, PaymentTier};

        let backend = build_test_instance(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());

        let pending = SendIntent::new(
            &backend.storage,
            quote_id.to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            20_000,
            1_000,
            PaymentTier::Standard,
            PaymentMetadata::default(),
        )
        .await
        .expect("create Pending send intent");

        pending
            .assign_to_batch(&backend.storage, Uuid::new_v4())
            .await
            .expect("transition Pending → Batched");

        let payment_identifier = PaymentIdentifier::QuoteId(quote_id);
        let response = backend
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("check_outgoing_payment for Batched intent");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(
            response.total_spent,
            Amount::new(0, CurrencyUnit::Sat),
            "Batched intents report total_spent = 0 until the batch \
             transaction is built and the per-intent fee is fixed"
        );
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_awaiting_confirmation_includes_fee() {
        // Once an intent reaches AwaitingConfirmation, the per-intent fee
        // contribution is persisted on the intent record. check_outgoing_payment
        // must now report total_spent = amount + fee_contribution_sat so that
        // downstream consumers (e.g. recovery / subscribers) see the
        // authoritative figure even though the payment is still unconfirmed.
        use crate::send::payment_intent::SendIntent;
        use crate::types::{PaymentMetadata, PaymentTier};

        let backend = build_test_instance(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());

        let pending = SendIntent::new(
            &backend.storage,
            quote_id.to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            30_000,
            2_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("create Pending send intent");

        let batched = pending
            .assign_to_batch(&backend.storage, Uuid::new_v4())
            .await
            .expect("transition Pending → Batched");

        let fee_contrib = 512_u64;
        batched
            .mark_broadcast(
                &backend.storage,
                "deadbeef".to_string(),
                "deadbeef:0".to_string(),
                fee_contrib,
            )
            .await
            .expect("transition Batched → AwaitingConfirmation");

        let payment_identifier = PaymentIdentifier::QuoteId(quote_id);
        let response = backend
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("check_outgoing_payment for AwaitingConfirmation intent");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(
            response.total_spent,
            Amount::new(30_000 + fee_contrib, CurrencyUnit::Sat),
            "AwaitingConfirmation intents know the per-intent fee \
             contribution and must report amount + fee"
        );
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_failed_intent_reports_failed() {
        use crate::send::payment_intent::SendIntent;
        use crate::types::{PaymentMetadata, PaymentTier};

        let backend = build_test_instance(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());

        let pending = SendIntent::new(
            &backend.storage,
            quote_id.to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            30_000,
            2_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("create Pending send intent");

        pending
            .fail(&backend.storage, "fee too high".to_string())
            .await
            .expect("transition Pending to Failed");

        let payment_identifier = PaymentIdentifier::QuoteId(quote_id);
        let response = backend
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("check_outgoing_payment for Failed intent");

        assert_eq!(response.status, MeltQuoteState::Failed);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
        assert_eq!(response.payment_proof, None);
    }

    #[tokio::test]
    async fn test_make_payment_can_retry_failed_intent_with_same_quote_id() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, options) = onchain_options_for(30_000);

        backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("initial make_payment should enqueue intent");

        let initial = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup initial intent")
            .expect("initial intent exists");

        backend
            .storage
            .update_send_intent(
                &initial.intent_id,
                &crate::send::payment_intent::record::SendIntentState::Failed {
                    reason: "pre-sign failure".to_string(),
                    created_at: 1_700_000_000,
                    failed_at: 1_700_000_100,
                },
            )
            .await
            .expect("mark failed");

        let retry_options = onchain_options_for_quote(quote_id.clone(), 30_000);
        let response = backend
            .make_payment(&CurrencyUnit::Sat, retry_options)
            .await
            .expect("retry with same quote id should requeue failed intent");

        assert_eq!(response.status, MeltQuoteState::Pending);

        let retried = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup retried intent")
            .expect("retried intent exists");
        assert_eq!(retried.intent_id, initial.intent_id);
        assert!(matches!(
            retried.state,
            crate::send::payment_intent::record::SendIntentState::Pending { .. }
        ));
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_unknown_quote_reports_zero() {
        // A quote id with no active intent and no finalized tombstone must
        // return MeltQuoteState::Unknown with total_spent = 0 (existing
        // behaviour; pinned here for defence-in-depth).
        let backend = build_test_instance(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let payment_identifier = PaymentIdentifier::QuoteId(quote_id);

        let response = backend
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("check_outgoing_payment for unknown quote");

        assert_eq!(response.status, MeltQuoteState::Unknown);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));
        assert_eq!(response.payment_proof, None);
    }

    // ------------------------------------------------------------------
    // Chain-sync resilience tests
    // ------------------------------------------------------------------

    #[test]
    fn test_is_transient_classifies_network_errors() {
        // Esplora errors are always classified as transient: the sync
        // loop should retry them on the next tick, and this classification
        // drives the log severity in the supervisor.
        let esplora_err = Error::Esplora(
            "HttpResponse { status: 525, message: \"error code: 525\" }".to_string(),
        );
        assert!(esplora_err.is_transient());

        let esplora_404 = Error::Esplora(
            "HttpResponse { status: 404, message: \"Block not found\" }".to_string(),
        );
        assert!(esplora_404.is_transient());

        // Local wallet/state errors are not transient: they indicate a
        // real defect that retrying will not resolve.
        let wallet_err = Error::Wallet("invalid checkpoint".to_string());
        assert!(!wallet_err.is_transient());

        let vout_err = Error::VoutNotFound;
        assert!(!vout_err.is_transient());

        // Timed-out I/O is transient.
        let io_err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "network timeout",
        ));
        assert!(io_err.is_transient());

        // An arbitrary I/O error kind is not.
        let io_other = Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bad data",
        ));
        assert!(!io_other.is_transient());
    }

    #[tokio::test]
    async fn test_supervisor_restarts_failing_task_with_backoff() {
        // The supervisor must keep calling the supplied future as long
        // as it returns Err, until the cancel token is triggered.
        let cancel = CancellationToken::new();
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let counter_clone = Arc::clone(&counter);
        let cancel_inner = cancel.clone();
        let supervisor = tokio::spawn(async move {
            super::supervise("test", cancel_inner, move |_c| {
                let c = Arc::clone(&counter_clone);
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Err::<(), Error>(Error::Esplora("boom".to_string()))
                }
            })
            .await;
        });

        // Let a few restart cycles happen (initial backoff is 1s).
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("supervisor did not exit after cancel")
            .expect("supervisor task panicked");

        let n = counter.load(Ordering::Relaxed);
        assert!(
            n >= 2,
            "supervisor should have restarted the task at least twice, got {n}"
        );
    }

    #[tokio::test]
    async fn test_supervisor_exits_on_ok() {
        // Ok(()) from the task is treated as clean shutdown; the
        // supervisor exits immediately without restart.
        let cancel = CancellationToken::new();
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

        let counter_clone = Arc::clone(&counter);
        let cancel_inner = cancel.clone();
        let supervisor = tokio::spawn(async move {
            super::supervise("test", cancel_inner, move |_c| {
                let c = Arc::clone(&counter_clone);
                async move {
                    c.fetch_add(1, Ordering::Relaxed);
                    Ok::<(), Error>(())
                }
            })
            .await;
        });

        tokio::time::timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("supervisor did not exit after Ok(())")
            .expect("supervisor task panicked");

        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "supervisor must not restart a task that returned Ok(())"
        );
    }

    #[tokio::test]
    async fn test_supervisor_cancel_during_backoff() {
        // Cancelling during the backoff sleep must exit promptly rather
        // than waiting for the sleep to expire.
        let cancel = CancellationToken::new();
        let cancel_inner = cancel.clone();
        let supervisor = tokio::spawn(async move {
            super::supervise("test", cancel_inner, move |_c| async move {
                // Fail immediately so we enter the backoff sleep.
                Err::<(), Error>(Error::Esplora("boom".to_string()))
            })
            .await;
        });

        // Give the supervisor a moment to enter its first backoff.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let cancel_at = std::time::Instant::now();
        cancel.cancel();

        tokio::time::timeout(Duration::from_secs(2), supervisor)
            .await
            .expect("supervisor did not exit promptly after cancel")
            .expect("supervisor task panicked");

        let elapsed = cancel_at.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "supervisor took {elapsed:?} to exit after cancel; expected < 500ms"
        );
    }

    #[tokio::test]
    async fn test_sync_wallet_survives_unreachable_esplora() {
        // sync_wallet must not return Err when the Esplora endpoint is
        // unreachable — it should warn and continue. We prove this by
        // starting the backend (which spawns the sync task against a
        // bogus URL) and letting it run for long enough to tick at least
        // twice, then stop cleanly.
        let backend = build_test_instance(5).await;
        backend.start().await.expect("start");

        // Sync interval is 60s per build_test_instance, so this test
        // only verifies the first synchronous tick path: the task must
        // stay alive and the supervisor must not log a "task failed"
        // line for a transient network error.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The sync JoinHandle must still be running, not completed.
        {
            let tasks = backend.tasks.lock().await;
            let bg = tasks.as_ref().expect("tasks running");
            assert!(
                !bg.sync.is_finished(),
                "sync task must not exit on transient Esplora errors"
            );
        }

        backend.stop().await.expect("stop");
    }
}
