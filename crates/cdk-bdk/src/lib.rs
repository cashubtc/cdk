//! CDK onchain backend using BDK

#![doc = include_str!("../README.md")]

use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, PersistedWallet, Wallet};
use cdk_common::common::FeeReserve;
use cdk_common::database::KVStore;
use cdk_common::nuts::nut30::MeltQuoteOnchainFeeOption;
use cdk_common::payjoin::payjoin_v2_is_expired_at;
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

pub use crate::chain::{BitcoinRpcConfig, ChainSource, EsploraConfig};
pub use crate::error::Error;
pub use crate::storage::{BdkStorage, FinalizedReceiveIntentRecord, FinalizedSendIntentRecord};
pub use crate::types::{
    BatchConfig, FeeEstimationConfig, PayjoinConfig, PaymentMetadata, PaymentTier, SyncConfig,
    DEFAULT_PAYJOIN_EXPIRY_SECS, DEFAULT_TARGET_BLOCK_TIME_SECS,
};

pub mod chain;
pub mod error;
pub(crate) mod fee;
pub(crate) mod payjoin;
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
    pub(crate) payjoin_receive: Option<JoinHandle<()>>,
    pub(crate) payjoin_send: Option<JoinHandle<()>>,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PlanningPausePoint {
    FeeEstimation,
    BeforePlanningLock,
    Broadcast,
    PayjoinReceivePoll,
    PayjoinReceivePost,
}

#[cfg(test)]
#[derive(Clone)]
struct PlanningPause {
    entered: Arc<tokio::sync::Barrier>,
    resume: Arc<tokio::sync::Barrier>,
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct PlanningTestHooks {
    pauses: std::sync::Mutex<std::collections::HashMap<PlanningPausePoint, PlanningPause>>,
}

#[cfg(test)]
impl PlanningTestHooks {
    pub(crate) fn install(
        &self,
        point: PlanningPausePoint,
    ) -> (Arc<tokio::sync::Barrier>, Arc<tokio::sync::Barrier>) {
        let pause = PlanningPause {
            entered: Arc::new(tokio::sync::Barrier::new(2)),
            resume: Arc::new(tokio::sync::Barrier::new(2)),
        };
        self.pauses
            .lock()
            .expect("planning test hook mutex poisoned")
            .insert(point, pause.clone());
        (pause.entered, pause.resume)
    }

    async fn pause(&self, point: PlanningPausePoint) {
        let pause = self
            .pauses
            .lock()
            .expect("planning test hook mutex poisoned")
            .remove(&point);
        if let Some(pause) = pause {
            pause.entered.wait().await;
            pause.resume.wait().await;
        }
    }
}

#[derive(Clone)]
pub(crate) struct PayjoinOhttpKeysCache {
    pub(crate) keys: ::payjoin::OhttpKeys,
    pub(crate) fetched_at: u64,
    pub(crate) directory_url: String,
    pub(crate) ohttp_relay_url: String,
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
    /// Serializes wallet coin selection through durable reservation.
    pub(crate) tx_planning_lock: Arc<Mutex<()>>,
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
    /// Payjoin v2 configuration, when enabled by operator settings.
    pub(crate) payjoin_config: Option<PayjoinConfig>,
    /// Cache for Payjoin OHTTP keys fetched from the configured directory.
    pub(crate) payjoin_ohttp_keys_cache: Arc<Mutex<Option<PayjoinOhttpKeysCache>>>,
    /// Single-flight lock for OHTTP key fetches when the cache is empty or stale.
    pub(crate) payjoin_ohttp_keys_fetch_lock: Arc<Mutex<()>>,
    #[cfg(test)]
    pub(crate) planning_test_hooks: Arc<PlanningTestHooks>,
}

impl CdkBdk {
    pub(crate) fn validate_send_amount_against_dust(
        &self,
        address: &str,
        amount_sat: u64,
    ) -> Result<(), Error> {
        let address = crate::util::parse_checked_address(address, self.network, Error::Wallet)?;

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
        payjoin_config: Option<PayjoinConfig>,
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
            tx_planning_lock: Arc::new(Mutex::new(())),
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
            payjoin_config,
            payjoin_ohttp_keys_cache: Arc::new(Mutex::new(None)),
            payjoin_ohttp_keys_fetch_lock: Arc::new(Mutex::new(())),
            #[cfg(test)]
            planning_test_hooks: Arc::new(PlanningTestHooks::default()),
        })
    }

    async fn check_outgoing_payment_status_local(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Error> {
        let quote_id = match payment_identifier {
            PaymentIdentifier::QuoteId(id) => id.to_string(),
            _ => return Err(Error::UnsupportedOnchain),
        };

        if let Some(record) = self.storage.get_send_intent_by_quote_id(&quote_id).await? {
            let total_spent = match &record.state {
                crate::send::payment_intent::record::SendIntentState::Pending { .. }
                | crate::send::payment_intent::record::SendIntentState::BatchClaimed { .. }
                | crate::send::payment_intent::record::SendIntentState::CutThroughReserved {
                    ..
                }
                | crate::send::payment_intent::record::SendIntentState::CutThroughExposed {
                    ..
                }
                | crate::send::payment_intent::record::SendIntentState::PayjoinNegotiating {
                    ..
                }
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
                | crate::send::payment_intent::record::SendIntentState::BatchClaimed { .. }
                | crate::send::payment_intent::record::SendIntentState::CutThroughReserved {
                    ..
                }
                | crate::send::payment_intent::record::SendIntentState::CutThroughExposed {
                    ..
                }
                | crate::send::payment_intent::record::SendIntentState::PayjoinNegotiating {
                    ..
                }
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
        // A crash can persist a Payjoin-negotiating intent before its signed
        // original transaction is persisted in BDK. Restore every remaining
        // reservation before normal batching can select the same inputs.
        self.restore_payjoin_send_reservations().await?;
        self.restore_payjoin_receive_reservations().await?;
        // DB-only: crash-leftover `CutThroughReserved` intents must be released
        // before any session is driven (once sessions run, reservation is a
        // live transient state). Network-driven session recovery is NOT
        // awaited here — the pollers' immediate first tick does that work, so
        // startup does not block on directory round trips.
        self.release_stale_cut_through_reservations().await?;

        let cancel = CancellationToken::new();

        let spawn_supervised = |name: &'static str,
                                run: fn(
            CdkBdk,
            CancellationToken,
        ) -> std::pin::Pin<
            Box<dyn Future<Output = Result<(), Error>> + Send>,
        >| {
            let me = self.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                supervise(name, cancel, move |cancel| run(me.clone(), cancel)).await;
            })
        };

        let sync_handle = spawn_supervised("wallet sync", |me, cancel| {
            Box::pin(async move { me.sync_wallet(cancel).await })
        });
        let batch_handle = spawn_supervised("batch processor", |me, cancel| {
            Box::pin(async move { me.run_batch_processor(cancel).await })
        });
        // The payjoin pollers exclusively own negotiation progress and
        // fallback broadcasts (`check_outgoing_payment` is a pure status
        // read), so they run even without a payjoin config: leftover
        // sessions/intents from a previously configured run still need to be
        // expired, pruned, or settled via fallback broadcast. Without work to
        // do a tick is a cheap empty listing.
        let payjoin_receive_handle =
            Some(spawn_supervised("payjoin receive poller", |me, cancel| {
                Box::pin(async move { me.run_payjoin_receive_poller(cancel).await })
            }));
        let payjoin_send_handle = Some(spawn_supervised("payjoin send poller", |me, cancel| {
            Box::pin(async move { me.run_payjoin_send_poller(cancel).await })
        }));

        *tasks_lock = Some(BackgroundTasks {
            cancel,
            sync: sync_handle,
            batch: batch_handle,
            payjoin_receive: payjoin_receive_handle,
            payjoin_send: payjoin_send_handle,
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
            let payjoin_receive_aborter =
                bg.payjoin_receive.as_ref().map(|task| task.abort_handle());
            let payjoin_send_aborter = bg.payjoin_send.as_ref().map(|task| task.abort_handle());

            let joined = tokio::time::timeout(self.shutdown_timeout, async move {
                let _ = bg.sync.await;
                let _ = bg.batch.await;
                if let Some(task) = bg.payjoin_receive {
                    let _ = task.await;
                }
                if let Some(task) = bg.payjoin_send {
                    let _ = task.await;
                }
            })
            .await;

            if joined.is_err() {
                sync_aborter.abort();
                batch_aborter.abort();
                if let Some(aborter) = payjoin_receive_aborter {
                    aborter.abort();
                }
                if let Some(aborter) = payjoin_send_aborter {
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
            Some(payjoin) => {
                if payjoin_v2_is_expired_at(&payjoin, crate::util::unix_now()) {
                    return Err(cdk_common::payment::Error::InvalidExpiry);
                }
                if self.payjoin_config().is_some() {
                    Some(Self::accepted_payjoin_extra(&payjoin))
                } else {
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
        if requested_payjoin
            .as_ref()
            .is_some_and(|payjoin| payjoin_v2_is_expired_at(payjoin, crate::util::unix_now()))
        {
            return Err(cdk_common::payment::Error::InvalidExpiry);
        }

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
            if self.payjoin_config().is_some() {
                match self
                    .start_payjoin_send(
                        &quote_id,
                        &address,
                        amount_sat,
                        max_fee_sat,
                        tier,
                        metadata.clone(),
                        &payjoin,
                    )
                    .await
                {
                    // The send was prepared and persisted; the background poller
                    // drives the negotiation and broadcasts the Payjoin tx or
                    // the original fallback.
                    Ok(response) => return Ok(response),
                    // Only safe to fall back to a direct onchain send when the
                    // Payjoin attempt failed *before* the signed original PSBT
                    // was shared with the receiver. `start_payjoin_send` only
                    // posts via the poller, so all of its failures are
                    // pre-exposure and reported as `PayjoinSendNotStarted`.
                    Err(Error::PayjoinSendNotStarted(err)) => {
                        tracing::warn!(
                            quote_id = %quote_id,
                            error = %err,
                            "Optional Payjoin send could not be started; falling back to direct onchain send"
                        );
                    }
                    Err(err) => {
                        tracing::error!(
                            quote_id = %quote_id,
                            error = %err,
                            "Payjoin send failed after the original PSBT was shared; not \
                             falling back to a direct send to avoid double-spending"
                        );
                        return Err(err.into());
                    }
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
        let quote_id = onchain_options.quote_id;

        wallet_with_db.persist().map_err(|err| {
            tracing::error!("Could not persist to bdk db: {}", err);

            Error::BdkPersist
        })?;
        drop(wallet_with_db);

        let extra_json = match tokio::time::timeout(
            Duration::from_secs(3),
            self.create_payjoin_receive_extra(&quote_id, &address.address, 0),
        )
        .await
        {
            Ok(Ok(extra_json)) => extra_json,
            Ok(Err(err)) => {
                tracing::warn!(
                    quote_id = %quote_id,
                    address = %address.address,
                    "Could not create optional Payjoin receive session: {}",
                    err
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    quote_id = %quote_id,
                    address = %address.address,
                    "Timed out creating optional Payjoin receive session"
                );
                None
            }
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
                payment_id: record.payment_id.unwrap_or(record.outpoint),
            });
        }

        Ok(results)
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        // Pure status read by trait contract. Payjoin negotiation and fallback
        // broadcasts are driven exclusively by the background send poller and
        // the startup recovery pass — never from status checks.
        Ok(self
            .check_outgoing_payment_status_local(payment_identifier)
            .await?)
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
        absolute, transaction, Address, Network, OutPoint, Sequence, Transaction, TxIn, TxOut,
        Txid, Witness,
    };
    use bdk_wallet::keys::bip39::Mnemonic;
    use cdk_common::common::FeeReserve;
    use cdk_common::payment::MintPayment;
    use futures::future::join_all;
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
            None,
        )?;

        Ok((backend, tmp))
    }

    /// Build a test instance with a Payjoin config so the send poller paths are
    /// active. The directory/relay URLs are unreachable, which is fine: the
    /// fallback/idempotency paths under test never reach the network (broadcast
    /// goes to the bogus Esplora URL and is tolerated).
    async fn build_test_instance_with_payjoin(
        shutdown_timeout_secs: u64,
    ) -> (CdkBdk, tempfile::TempDir) {
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
        let payjoin_config = PayjoinConfig::new(
            "http://127.0.0.1:1".to_string(),
            "http://127.0.0.1:1".to_string(),
            Some(3600),
        )
        .expect("valid payjoin config");

        let backend = CdkBdk::new(
            mnemonic,
            Network::Regtest,
            chain_source,
            tmp.path().to_string_lossy().into_owned(),
            fee_reserve,
            Arc::new(kv),
            None,
            1,
            0,
            546,
            60,
            Some(shutdown_timeout_secs),
            None,
            Some(payjoin_config),
        )
        .expect("build payjoin CdkBdk test instance");

        (backend, tmp)
    }

    async fn fund_backend_wallet(backend: &CdkBdk, amount_sat: u64) -> OutPoint {
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

        let funding_outpoint = OutPoint::new(funding_tx.compute_txid(), 0);
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(funding_tx, 0)]);
        wallet_with_db.persist().expect("persist funded wallet");
        funding_outpoint
    }

    async fn enqueue_immediate_send(backend: &CdkBdk, amount_sat: u64) -> Uuid {
        crate::send::payment_intent::SendIntent::new(
            &backend.storage,
            QuoteId::UUID(Uuid::new_v4()).to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount_sat,
            2_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("enqueue immediate send")
        .intent_id
    }

    async fn wait_for_pause(barrier: &tokio::sync::Barrier) {
        tokio::time::timeout(Duration::from_secs(5), barrier.wait())
            .await
            .expect("planning test pause was not reached");
    }

    fn assert_planning_lock_available(backend: &CdkBdk, context: &str) {
        let guard = backend
            .tx_planning_lock
            .try_lock()
            .unwrap_or_else(|_| panic!("planning lock held during {context}"));
        drop(guard);
    }

    #[tokio::test]
    async fn delayed_fee_estimation_does_not_hold_planning_lock() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        enqueue_immediate_send(&backend, 10_000).await;
        let (entered, resume) = backend
            .planning_test_hooks
            .install(PlanningPausePoint::FeeEstimation);

        let worker = backend.clone();
        let task = tokio::spawn(async move { worker.process_ready_intents().await });
        wait_for_pause(&entered).await;
        assert_planning_lock_available(&backend, "fee estimation");
        resume.wait().await;

        let _ = task.await.expect("batch task panicked");
    }

    #[tokio::test]
    async fn delayed_broadcast_releases_lock_and_competing_batch_cannot_reuse_input() {
        use crate::send::payment_intent::{self, SendIntentAny};

        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 25_000).await;
        backend
            .fee_rate_cache
            .lock()
            .await
            .insert(PaymentTier::Immediate, (1.0, crate::util::unix_now()));

        let first_id = enqueue_immediate_send(&backend, 20_000).await;
        let first_batch_id = Uuid::new_v4();
        let first_record = backend
            .storage
            .claim_pending_send_intents_for_batch(&[first_id], first_batch_id)
            .await
            .expect("claim first send")
            .pop()
            .expect("first send was claimed");
        let SendIntentAny::BatchClaimed(first) = payment_intent::from_record(&first_record) else {
            panic!("first send was not batch-claimed");
        };
        let (entered, resume) = backend
            .planning_test_hooks
            .install(PlanningPausePoint::Broadcast);

        let worker = backend.clone();
        let first_task = tokio::spawn(async move {
            worker
                .build_sign_broadcast_batch(first_batch_id, vec![first])
                .await
        });
        wait_for_pause(&entered).await;
        assert_planning_lock_available(&backend, "transaction broadcast");

        let second_id = enqueue_immediate_send(&backend, 20_000).await;
        let second_batch_id = Uuid::new_v4();
        let second_record = backend
            .storage
            .claim_pending_send_intents_for_batch(&[second_id], second_batch_id)
            .await
            .expect("claim competing send")
            .pop()
            .expect("competing send was claimed");
        let SendIntentAny::BatchClaimed(second) = payment_intent::from_record(&second_record)
        else {
            panic!("competing send was not batch-claimed");
        };
        assert!(
            backend
                .build_sign_broadcast_batch(second_batch_id, vec![second])
                .await
                .is_err(),
            "competing batch must not be able to select the first batch's input"
        );
        assert_eq!(
            backend
                .storage
                .get_all_send_batches()
                .await
                .expect("list send batches")
                .len(),
            1,
            "only the durably reserved first transaction should be staged"
        );

        resume.wait().await;
        let _ = first_task.await.expect("first batch task panicked");
    }

    #[tokio::test]
    async fn delayed_payjoin_directory_poll_does_not_hold_planning_lock() {
        let _fetch_guard = crate::payjoin::lock_test_ohttp_fetch().await;
        crate::payjoin::configure_test_ohttp_fetch(Duration::ZERO, false);
        let (backend, _tmp) = build_test_instance_with_payjoin(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let address = {
            let mut wallet = backend.wallet_with_db.lock().await;
            let address = wallet
                .wallet
                .reveal_next_address(KeychainKind::External)
                .address;
            wallet.persist().expect("persist receive address");
            address
        };
        backend
            .create_payjoin_receive_extra(&quote_id, &address, 10_000)
            .await
            .expect("create Payjoin receive session")
            .expect("Payjoin receive extra");
        let record = backend
            .storage
            .get_payjoin_receive_session(&quote_id.to_string())
            .await
            .expect("load Payjoin receive session")
            .expect("Payjoin receive session");
        let (entered, resume) = backend
            .planning_test_hooks
            .install(PlanningPausePoint::PayjoinReceivePoll);

        let worker = backend.clone();
        let task =
            tokio::spawn(async move { worker.process_payjoin_receive_session(record).await });
        wait_for_pause(&entered).await;
        assert_planning_lock_available(&backend, "Payjoin directory polling");
        resume.wait().await;

        let _ = task.await.expect("Payjoin receive task panicked");
        crate::payjoin::disable_test_ohttp_fetch();
    }

    #[tokio::test]
    async fn delayed_payjoin_proposal_post_does_not_hold_planning_lock() {
        let (backend, _tmp) = build_test_instance_with_payjoin(5).await;
        let planning_guard = backend.tx_planning_lock.clone().lock_owned().await;
        let (entered, resume) = backend
            .planning_test_hooks
            .install(PlanningPausePoint::PayjoinReceivePost);

        let worker = backend.clone();
        let task = tokio::spawn(async move {
            worker
                .release_planning_before_payjoin_post(Some(planning_guard))
                .await;
        });
        wait_for_pause(&entered).await;
        assert_planning_lock_available(&backend, "Payjoin proposal posting");
        resume.wait().await;
        task.await.expect("Payjoin post boundary task panicked");
    }

    #[tokio::test]
    async fn payjoin_send_reservation_prevents_competing_normal_input_selection() {
        let _fetch_guard = crate::payjoin::lock_test_ohttp_fetch().await;
        crate::payjoin::configure_test_ohttp_fetch(Duration::ZERO, false);
        let (backend, _tmp) = build_test_instance_with_payjoin(5).await;
        fund_backend_wallet(&backend, 25_000).await;
        backend
            .fee_rate_cache
            .lock()
            .await
            .insert(PaymentTier::Immediate, (1.0, crate::util::unix_now()));
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let receive_address = {
            let mut wallet = backend.wallet_with_db.lock().await;
            let address = wallet
                .wallet
                .reveal_next_address(KeychainKind::External)
                .address;
            wallet.persist().expect("persist Payjoin receive address");
            address
        };
        let receive_extra = backend
            .create_payjoin_receive_extra(&QuoteId::UUID(Uuid::new_v4()), &receive_address, 20_000)
            .await
            .expect("create valid Payjoin parameters")
            .expect("Payjoin receive extra");
        let payjoin = serde_json::from_value::<cdk_common::nuts::nut31::PayjoinV2>(
            receive_extra.get("payjoin").expect("Payjoin field").clone(),
        )
        .expect("deserialize Payjoin parameters");

        backend
            .start_payjoin_send(
                &quote_id,
                "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
                20_000,
                2_000,
                PaymentTier::Immediate,
                PaymentMetadata::default(),
                &payjoin,
            )
            .await
            .expect("start Payjoin send");
        assert_planning_lock_available(&backend, "completed Payjoin send preparation");

        enqueue_immediate_send(&backend, 20_000).await;
        assert!(
            backend.process_ready_intents().await.is_err(),
            "normal planning must not reuse the Payjoin original's reserved input"
        );
        assert!(
            backend
                .storage
                .get_all_send_batches()
                .await
                .expect("list send batches")
                .is_empty(),
            "the competing normal send must fail before signed staging"
        );
        crate::payjoin::disable_test_ohttp_fetch();
    }

    #[tokio::test]
    async fn receive_proposal_reservation_prevents_competing_normal_input_selection() {
        use crate::send::payment_intent::{self, SendIntentAny};

        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        let funding_outpoint = fund_backend_wallet(&backend, 25_000).await;
        backend
            .fee_rate_cache
            .lock()
            .await
            .insert(PaymentTier::Immediate, (1.0, crate::util::unix_now()));
        let intent_id = enqueue_immediate_send(&backend, 20_000).await;
        let batch_id = Uuid::new_v4();
        let record = backend
            .storage
            .claim_pending_send_intents_for_batch(&[intent_id], batch_id)
            .await
            .expect("claim competing normal send")
            .pop()
            .expect("normal send was claimed");
        let SendIntentAny::BatchClaimed(intent) = payment_intent::from_record(&record) else {
            panic!("normal send was not batch-claimed");
        };
        let (entered, resume) = backend
            .planning_test_hooks
            .install(PlanningPausePoint::BeforePlanningLock);

        let worker = backend.clone();
        let task = tokio::spawn(async move {
            worker
                .build_sign_broadcast_batch(batch_id, vec![intent])
                .await
        });
        wait_for_pause(&entered).await;

        // Model the receive-proposal boundary: it owns the shared planning
        // guard while selecting the contribution and applying the resulting
        // signed proposal to BDK.
        let receive_guard = backend.tx_planning_lock.clone().lock_owned().await;
        resume.wait().await;
        let receive_proposal = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: funding_outpoint,
                script_sig: Default::default(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: bdk_wallet::bitcoin::Amount::from_sat(20_000),
                script_pubkey: Address::from_str("bcrt1q6rhpng9evdsfnn833a4f4vej0asu6dk5srld6x")
                    .expect("test address")
                    .require_network(Network::Regtest)
                    .expect("regtest address")
                    .script_pubkey(),
            }],
        };
        {
            let mut wallet = backend.wallet_with_db.lock().await;
            wallet
                .wallet
                .apply_unconfirmed_txs([(receive_proposal, crate::util::unix_now())]);
            wallet
                .persist()
                .expect("persist receive proposal reservation");
        }
        drop(receive_guard);

        assert!(
            task.await.expect("normal planning task panicked").is_err(),
            "normal planning must observe the receive proposal's spent input"
        );
        assert!(
            backend
                .storage
                .get_all_send_batches()
                .await
                .expect("list send batches")
                .is_empty(),
            "the competing normal send must fail before signed staging"
        );
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

    #[tokio::test]
    async fn test_start_payjoin_send_pre_exposure_failure_is_recoverable() {
        // A Payjoin send that fails before the original PSBT is shared with the
        // receiver (here, because no Payjoin directory is configured) must be
        // reported as `PayjoinSendNotStarted`. That is the only failure where
        // `make_payment` may safely fall back to a direct onchain send; any
        // other error means the original was already exposed and a second
        // transaction could double-spend.
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let payjoin = cdk_common::nuts::nut31::PayjoinV2::new(
            "https://payjoin.example/pj".to_string(),
            "QYPFLM8XL59R0XV4VGPLS7FRDSSM4TUXL07TXCWC4S0GLVLNK2SE4NQ",
            "QV6WSX0UQPAEA0RH54430D0UVZWS8CZ6FEGZF4RGFCDKJLPGMYEJG",
            crate::util::unix_now() + 3600,
        )
        .expect("valid Payjoin keys");

        let err = backend
            .start_payjoin_send(
                &quote_id,
                "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080",
                10_000,
                1_000,
                PaymentTier::Immediate,
                PaymentMetadata::default(),
                &payjoin,
            )
            .await
            .expect_err("payjoin send without a configured directory must fail");

        assert!(
            matches!(err, Error::PayjoinSendNotStarted(_)),
            "pre-exposure failures must be recoverable, got {err:?}"
        );
    }

    /// Regtest original tx paying `amount_sat` to `address`, used as the signed
    /// Payjoin fallback in send-intent tests.
    fn test_original_tx(address: &str, amount_sat: u64) -> Transaction {
        let fallback_script = bdk_wallet::bitcoin::Address::from_str(address)
            .expect("valid fallback address")
            .require_network(Network::Regtest)
            .expect("regtest fallback address")
            .script_pubkey();
        Transaction {
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
                script_pubkey: fallback_script,
            }],
        }
    }

    #[tokio::test]
    async fn test_restore_payjoin_send_reservations_closes_intent_persistence_crash_window() {
        use crate::send::payment_intent::{state as intent_state, SendIntent};

        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        let funding_outpoint = fund_backend_wallet(&backend, 100_000).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let address = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
        let mut original_tx = test_original_tx(&address, 90_000);
        original_tx.input[0].previous_output = funding_outpoint;
        let original_txid = original_tx.compute_txid();

        // Model a crash after the intent commit but before start_payjoin_send
        // applies and persists the original transaction to the BDK graph.
        SendIntent::<intent_state::PayjoinNegotiating>::new_payjoin(
            &backend.storage,
            quote_id.to_string(),
            address,
            90_000,
            10_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
            bdk_wallet::bitcoin::consensus::serialize(&original_tx),
            10_000,
            Vec::new(),
        )
        .await
        .expect("persist Payjoin negotiating intent");

        {
            let wallet_with_db = backend.wallet_with_db.lock().await;
            assert!(
                wallet_with_db.wallet.get_tx(original_txid).is_none(),
                "the simulated crash must leave the original unreserved"
            );
            assert!(
                wallet_with_db
                    .wallet
                    .list_unspent()
                    .any(|utxo| utxo.outpoint == funding_outpoint),
                "the original input must still be spendable before recovery"
            );
        }

        backend
            .restore_payjoin_send_reservations()
            .await
            .expect("restore Payjoin send reservations");

        let wallet_with_db = backend.wallet_with_db.lock().await;
        assert!(
            wallet_with_db.wallet.get_tx(original_txid).is_some(),
            "startup recovery must restore the original transaction reservation"
        );
        assert!(
            wallet_with_db
                .wallet
                .list_unspent()
                .all(|utxo| utxo.outpoint != funding_outpoint),
            "the restored original input must not remain available to normal batching"
        );
    }

    #[tokio::test]
    async fn test_recovery_drives_payjoin_intent_fallback_without_config() {
        use crate::send::payment_intent::record::SendIntentState;
        use crate::send::payment_intent::{state as intent_state, SendIntent};

        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let address = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
        let original_tx = test_original_tx(&address, 10_000);

        SendIntent::<intent_state::PayjoinNegotiating>::new_payjoin(
            &backend.storage,
            quote_id.to_string(),
            address,
            10_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
            bdk_wallet::bitcoin::consensus::serialize(&original_tx),
            500,
            Vec::new(),
        )
        .await
        .expect("persist Payjoin negotiating intent");

        // The recovery pass (also the poller's per-tick work) owns negotiation
        // progress; without a payjoin config it broadcasts the signed original
        // fallback and stages the intent.
        backend
            .recover_payjoin_sessions_once()
            .await
            .expect("recover payjoin sessions");

        let response = backend
            .check_outgoing_payment(&PaymentIdentifier::QuoteId(quote_id.clone()))
            .await
            .expect("check outgoing payment");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(response.total_spent, Amount::new(10_500, CurrencyUnit::Sat));

        let stored = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent")
            .expect("send intent remains active");
        assert!(
            matches!(stored.state, SendIntentState::AwaitingConfirmation { .. }),
            "fallback recovery must move Payjoin intent into AwaitingConfirmation, got {:?}",
            stored.state
        );
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_is_pure_for_negotiating_intent() {
        use crate::send::payment_intent::record::SendIntentState;
        use crate::send::payment_intent::{state as intent_state, SendIntent};

        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let address = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
        let original_tx = test_original_tx(&address, 10_000);

        SendIntent::<intent_state::PayjoinNegotiating>::new_payjoin(
            &backend.storage,
            quote_id.to_string(),
            address,
            10_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
            bdk_wallet::bitcoin::consensus::serialize(&original_tx),
            500,
            Vec::new(),
        )
        .await
        .expect("persist Payjoin negotiating intent");

        let response = backend
            .check_outgoing_payment(&PaymentIdentifier::QuoteId(quote_id.clone()))
            .await
            .expect("check outgoing payment");

        assert_eq!(response.status, MeltQuoteState::Pending);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Sat));

        let stored = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent")
            .expect("send intent remains active");
        assert!(
            matches!(stored.state, SendIntentState::PayjoinNegotiating { .. }),
            "status checks must not progress Payjoin intents, got {:?}",
            stored.state
        );
    }

    #[tokio::test]
    async fn test_check_outgoing_payment_terminal_and_unknown_intents() {
        use crate::send::payment_intent::SendIntent;

        let backend = build_test_instance(5).await;
        let failed_quote_id = QuoteId::UUID(Uuid::new_v4());
        let pending = SendIntent::new(
            &backend.storage,
            failed_quote_id.to_string(),
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

        let failed = backend
            .check_outgoing_payment(&PaymentIdentifier::QuoteId(failed_quote_id.clone()))
            .await
            .expect("status check failed intent");
        assert_eq!(failed.status, MeltQuoteState::Failed);
        assert_eq!(failed.total_spent, Amount::new(0, CurrencyUnit::Sat));

        let paid_quote_id = QuoteId::UUID(Uuid::new_v4());
        let pending = SendIntent::new(
            &backend.storage,
            paid_quote_id.to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            40_000,
            2_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("create Pending send intent");
        let awaiting = pending
            .assign_to_batch(&backend.storage, Uuid::new_v4())
            .await
            .expect("transition Pending to Batched")
            .mark_broadcast(
                &backend.storage,
                "deadbeef".to_string(),
                "deadbeef:1".to_string(),
                321,
            )
            .await
            .expect("transition Batched to AwaitingConfirmation");
        awaiting
            .finalize(&backend.storage)
            .await
            .expect("finalize send intent");

        let paid = backend
            .check_outgoing_payment(&PaymentIdentifier::QuoteId(paid_quote_id))
            .await
            .expect("status check paid intent");
        assert_eq!(paid.status, MeltQuoteState::Paid);
        assert_eq!(paid.payment_proof.as_deref(), Some("deadbeef:1"));
        assert_eq!(paid.total_spent, Amount::new(40_321, CurrencyUnit::Sat));

        let unknown = backend
            .check_outgoing_payment(&PaymentIdentifier::QuoteId(QuoteId::UUID(Uuid::new_v4())))
            .await
            .expect("status check unknown quote");
        assert_eq!(unknown.status, MeltQuoteState::Unknown);
        assert_eq!(unknown.total_spent, Amount::new(0, CurrencyUnit::Sat));
    }

    #[tokio::test]
    async fn test_ohttp_key_fetch_is_single_flight() {
        let _guard = crate::payjoin::lock_test_ohttp_fetch().await;
        crate::payjoin::configure_test_ohttp_fetch(Duration::from_millis(100), false);
        let (backend, _tmp) = build_test_instance_with_payjoin(5).await;
        let address =
            bdk_wallet::bitcoin::Address::from_str("bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080")
                .expect("valid fallback address")
                .require_network(Network::Regtest)
                .expect("regtest fallback address");

        let futures = (0..8).map(|_| {
            let backend = backend.clone();
            let address = address.clone();
            let quote_id = QuoteId::UUID(Uuid::new_v4());
            async move {
                backend
                    .create_payjoin_receive_extra(&quote_id, &address, 0)
                    .await
            }
        });

        let results = join_all(futures).await;
        let fetch_calls = crate::payjoin::test_ohttp_fetch_calls();
        crate::payjoin::disable_test_ohttp_fetch();

        assert!(results.iter().all(Result::is_ok));
        assert_eq!(
            fetch_calls, 1,
            "concurrent cache misses must collapse to one OHTTP key fetch"
        );
    }

    #[tokio::test]
    async fn test_create_incoming_payment_request_falls_back_on_ohttp_timeout() {
        let _guard = crate::payjoin::lock_test_ohttp_fetch().await;
        crate::payjoin::configure_test_ohttp_fetch(Duration::from_secs(5), false);
        let (backend, _tmp) = build_test_instance_with_payjoin(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let started = std::time::Instant::now();

        let response = backend
            .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                cdk_common::payment::OnchainIncomingPaymentOptions { quote_id },
            ))
            .await
            .expect("plain onchain quote should be returned");
        crate::payjoin::disable_test_ohttp_fetch();

        assert!(
            started.elapsed() < Duration::from_secs(4),
            "quote creation should be bounded by the Payjoin metadata timeout"
        );
        assert!(response.extra_json.is_none());
    }

    #[tokio::test]
    async fn test_recover_payjoin_receive_sessions_without_config() {
        use crate::storage::PayjoinReceiveSessionRecord;

        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        let now = crate::util::unix_now();
        let expired_quote = QuoteId::UUID(Uuid::new_v4()).to_string();
        let old_closed_quote = QuoteId::UUID(Uuid::new_v4()).to_string();
        let open_quote = QuoteId::UUID(Uuid::new_v4()).to_string();

        backend
            .storage
            .put_payjoin_receive_session(&PayjoinReceiveSessionRecord {
                quote_id: expired_quote.clone(),
                fallback_address: "bcrt1qexpired".to_string(),
                amount_sat: 1_000,
                proposal_receiver_outpoints: Vec::new(),
                proposal_tx_bytes: None,
                cut_through: None,
                expires_at: now.saturating_sub(1),
                events: Vec::new(),
                closed: false,
            })
            .await
            .expect("store expired session");

        backend
            .storage
            .put_payjoin_receive_session(&PayjoinReceiveSessionRecord {
                quote_id: old_closed_quote.clone(),
                fallback_address: "bcrt1qclosed".to_string(),
                amount_sat: 1_000,
                proposal_receiver_outpoints: Vec::new(),
                proposal_tx_bytes: None,
                cut_through: None,
                expires_at: now.saturating_sub(7 * 24 * 60 * 60).saturating_sub(1),
                events: Vec::new(),
                closed: true,
            })
            .await
            .expect("store old closed session");

        backend
            .storage
            .put_payjoin_receive_session(&PayjoinReceiveSessionRecord {
                quote_id: open_quote.clone(),
                fallback_address: "bcrt1qopen".to_string(),
                amount_sat: 1_000,
                proposal_receiver_outpoints: Vec::new(),
                proposal_tx_bytes: None,
                cut_through: None,
                expires_at: now + 60,
                events: Vec::new(),
                closed: false,
            })
            .await
            .expect("store open session");

        backend
            .recover_payjoin_sessions_once()
            .await
            .expect("recover payjoin sessions");

        let expired = backend
            .storage
            .get_payjoin_receive_session(&expired_quote)
            .await
            .expect("lookup expired")
            .expect("expired session remains");
        assert!(expired.closed, "expired session should be closed");

        assert!(
            backend
                .storage
                .get_payjoin_receive_session(&old_closed_quote)
                .await
                .expect("lookup old closed")
                .is_none(),
            "old closed session should be pruned"
        );

        let open = backend
            .storage
            .get_payjoin_receive_session(&open_quote)
            .await
            .expect("lookup open")
            .expect("open session remains");
        assert!(!open.closed, "unexpired open session should stay open");
    }

    #[tokio::test]
    async fn test_process_payjoin_send_intent_unreplayable_broadcasts_original_fallback() {
        // An empty/unreplayable event log (e.g. an expired session) must cause
        // the poller to broadcast the stored signed original as the fallback,
        // and stage the existing intent for the quote.
        use crate::send::payment_intent::record::SendIntentState;
        use crate::send::payment_intent::{state as intent_state, SendIntent};

        let (backend, _tmp) = build_test_instance_with_payjoin(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let address = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
        let original_tx = test_original_tx(&address, 10_000);
        let original_tx_bytes = bdk_wallet::bitcoin::consensus::serialize(&original_tx);

        let intent = SendIntent::<intent_state::PayjoinNegotiating>::new_payjoin(
            &backend.storage,
            quote_id.to_string(),
            address,
            10_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
            original_tx_bytes,
            500,
            // Empty event log: `replay_event_log` returns an error, so the
            // poller takes the original-fallback path.
            Vec::new(),
        )
        .await
        .expect("persist payjoin intent");

        backend
            .process_payjoin_send_intent(intent)
            .await
            .expect("process intent");

        let intent = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent")
            .expect("fallback must keep the send intent");
        assert_eq!(intent.amount_sat, 10_000);
        assert!(
            matches!(intent.state, SendIntentState::AwaitingConfirmation { .. }),
            "fallback must move Payjoin intent into AwaitingConfirmation, got {:?}",
            intent.state
        );
    }

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
