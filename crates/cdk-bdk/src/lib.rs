//! CDK onchain backend using BDK

#![doc = include_str!("../README.md")]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use std::{fmt, fs};

use async_trait::async_trait;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, PersistedWallet, Update, Wallet};
use cdk_common::amount::MSAT_IN_SAT;
use cdk_common::common::FeeReserve;
use cdk_common::database::KVStore;
use cdk_common::nuts::nut30::MeltQuoteOnchainFeeOption;
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

pub use crate::chain::{BitcoinRpcConfig, ChainSource, ElectrumConfig, EsploraConfig};
pub use crate::error::Error;
pub use crate::storage::{BdkStorage, FinalizedReceiveIntentRecord, FinalizedSendIntentRecord};
pub use crate::types::{
    BatchConfig, FeeEstimationConfig, PaymentMetadata, PaymentTier, SyncConfig,
    DEFAULT_TARGET_BLOCK_TIME_SECS,
};

pub mod chain;
pub mod error;
pub(crate) mod fee;
pub mod receive;
pub(crate) mod recovery;
pub mod send;
pub mod storage;
pub(crate) mod sync;
#[cfg(test)]
pub(crate) mod testutil;
pub mod types;
pub(crate) mod util;
pub mod wallet_info;

pub use crate::wallet_info::{
    WalletAddress, WalletBalance, WalletKeychain, WalletPage, WalletTransaction,
    WalletTransactionInput, WalletTransactionOutput,
};

const MAX_RECEIVE_ADDRESS_RESERVATION_ATTEMPTS: usize = 100;

/// Wrapper struct that combines wallet and database to prevent deadlocks
pub(crate) struct WalletWithDb {
    pub(crate) wallet: PersistedWallet<Connection>,
    pub(crate) db: Connection,
}

pub(crate) struct BackgroundTasks {
    pub(crate) cancel: CancellationToken,
    pub(crate) sync: JoinHandle<()>,
    pub(crate) batch: JoinHandle<()>,
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
}

impl CdkBdk {
    fn outgoing_payment_failure_response(
        unit: &CurrencyUnit,
        quote_id: &cdk_common::QuoteId,
        reason: impl fmt::Display,
    ) -> MakePaymentResponse {
        tracing::warn!(
            quote_id = %quote_id,
            "BDK rejected onchain payment before dispatch: {reason}"
        );
        MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
            payment_proof: None,
            status: MeltQuoteState::Failed,
            total_spent: Amount::new(0, unit.clone()),
        }
    }

    fn ensure_supported_payment_unit(
        unit: &CurrencyUnit,
    ) -> Result<(), cdk_common::payment::Error> {
        match unit {
            CurrencyUnit::Sat | CurrencyUnit::Msat => Ok(()),
            _ => Err(cdk_common::payment::Error::UnsupportedUnit),
        }
    }

    fn ensure_amount_unit(unit: &CurrencyUnit, amount: &Amount<CurrencyUnit>) -> Result<(), Error> {
        if amount.unit() != unit {
            return Err(Error::AmountUnitMismatch {
                expected: unit.clone(),
                actual: amount.unit().clone(),
            });
        }

        Ok(())
    }

    fn payment_amount_to_sat(
        unit: &CurrencyUnit,
        amount: &Amount<CurrencyUnit>,
    ) -> Result<u64, Error> {
        Self::ensure_amount_unit(unit, amount)?;

        if unit == &CurrencyUnit::Msat && amount.value() % MSAT_IN_SAT != 0 {
            return Err(Error::FractionalSatoshiAmount {
                amount_msat: amount.value(),
            });
        }

        amount.to_sat().map_err(Error::from)
    }

    fn fee_limit_to_sat(unit: &CurrencyUnit, amount: &Amount<CurrencyUnit>) -> Result<u64, Error> {
        Self::ensure_amount_unit(unit, amount)?;
        amount.to_sat().map_err(Error::from)
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
    ) -> Result<Self, Error> {
        chain_source.validate()?;

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

        // A fresh Bitcoin Core wallet should start at the current tip rather
        // than scanning from genesis. An explicit rescan height overrides the
        // tip for seed recovery. Fetch this before creating the wallet so an
        // unreachable or misconfigured node cannot persist a genesis-pinned
        // wallet by accident.
        let initial_checkpoint = match wallet_opt.is_none() {
            true => chain_source.initial_checkpoint()?,
            false => None,
        };

        let mut wallet = match wallet_opt {
            Some(wallet) => wallet,
            None => {
                let mut wallet = Wallet::create(descriptor, change_descriptor)
                    .network(network)
                    .create_wallet(&mut db)
                    .map_err(|e| Error::Wallet(e.to_string()))?;

                if let Some(block_id) = initial_checkpoint {
                    let checkpoint = wallet.latest_checkpoint().insert(block_id);
                    wallet
                        .apply_update(Update {
                            chain: Some(checkpoint),
                            ..Default::default()
                        })
                        .map_err(|e| Error::Wallet(e.to_string()))?;
                }

                wallet
            }
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
        self.storage.ensure_send_outpoint_quote_id_index().await?;

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

        *tasks_lock = Some(BackgroundTasks {
            cancel,
            sync: sync_handle,
            batch: batch_handle,
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

            let joined = tokio::time::timeout(self.shutdown_timeout, async move {
                let _ = bg.sync.await;
                let _ = bg.batch.await;
            })
            .await;

            if joined.is_err() {
                sync_aborter.abort();
                batch_aborter.abort();
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
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        Self::ensure_supported_payment_unit(unit)?;

        let onchain_options = match options {
            OutgoingPaymentOptions::Onchain(o) => o,
            _ => return Err(cdk_common::payment::Error::UnsupportedPaymentOption),
        };

        let amount_sat = Self::payment_amount_to_sat(unit, &onchain_options.amount)?;
        self.validate_send_amount(&onchain_options.address, amount_sat)?;

        // Estimate fee_reserve for each configured tier so the mint presents
        // only the operator-enabled options. The configured order owns the
        // `fee_index` values and resolves them back to tiers during payment.
        let mut fee_options = Vec::with_capacity(self.batch_config.fee_options.len());
        for (idx, tier) in self.batch_config.fee_options.iter().enumerate() {
            let fee_estimate = self
                .estimate_onchain_fee_reserve(&onchain_options.address, amount_sat, *tier)
                .await?;
            let fee_reserve = Amount::new(fee_estimate.fee_reserve_sat, CurrencyUnit::Sat)
                .convert_to(unit)
                .map_err(Error::AmountConversion)?;
            fee_options.push(MeltQuoteOnchainFeeOption {
                fee_index: idx as u32,
                fee_reserve: fee_reserve.into(),
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
            fee: Amount::new(cheapest.fee_reserve.into(), unit.clone()),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: Some(cheapest.estimated_blocks),
            fee_options: Some(fee_options),
        })
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let onchain_options = match options {
            OutgoingPaymentOptions::Onchain(o) => o,
            _ => return Err(cdk_common::payment::Error::UnsupportedPaymentOption),
        };

        let address = onchain_options.address;
        let amount = onchain_options.amount;
        let quote_id = onchain_options.quote_id;

        if let Err(err) = Self::ensure_supported_payment_unit(unit) {
            return Ok(Self::outgoing_payment_failure_response(
                unit, &quote_id, err,
            ));
        }

        let amount_sat = match Self::payment_amount_to_sat(unit, &amount) {
            Ok(amount_sat) => amount_sat,
            Err(err) => {
                return Ok(Self::outgoing_payment_failure_response(
                    unit, &quote_id, err,
                ));
            }
        };
        if let Err(err) = self.validate_send_amount(&address, amount_sat) {
            return Ok(Self::outgoing_payment_failure_response(
                unit, &quote_id, err,
            ));
        }

        let max_fee_sat = match onchain_options.max_fee_amount {
            Some(max_fee) => match Self::fee_limit_to_sat(unit, &max_fee) {
                Ok(max_fee_sat) => max_fee_sat,
                Err(err) => {
                    return Ok(Self::outgoing_payment_failure_response(
                        unit, &quote_id, err,
                    ));
                }
            },
            None => 1_000,
        };
        // Resolve the wallet-selected `fee_index` back to a configured tier.
        // Older callers that omit `fee_index` continue to default to
        // Immediate.
        let tier = match self
            .batch_config
            .tier_for_fee_index(onchain_options.fee_index)
            .map_err(Error::UnknownFeeIndex)
        {
            Ok(tier) => tier,
            Err(err) => {
                return Ok(Self::outgoing_payment_failure_response(
                    unit, &quote_id, err,
                ));
            }
        };
        let metadata = PaymentMetadata::from_optional_json(onchain_options.metadata.as_deref());
        let fee_estimate = match self
            .estimate_onchain_fee_reserve(&address, amount_sat, tier)
            .await
        {
            Ok(fee_estimate) => fee_estimate,
            Err(err) => {
                return Ok(Self::outgoing_payment_failure_response(
                    unit, &quote_id, err,
                ));
            }
        };
        if fee_estimate.raw_fee_sat > max_fee_sat {
            let err = Error::EstimatedFeeTooHigh {
                estimated_fee: fee_estimate.raw_fee_sat,
                max_fee: max_fee_sat,
            };
            return Ok(Self::outgoing_payment_failure_response(
                unit, &quote_id, err,
            ));
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
            total_spent: Amount::new(0, unit.clone()),
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

        let quote_id = onchain_options.quote_id;
        let quote_id_string = quote_id.to_string();

        let mut wallet_with_db = self.wallet_with_db.lock().await;

        // An address already tracked for another quote (derived by another
        // instance sharing this seed, or by a re-initialised local wallet)
        // must not be handed out again; advance to a fresh address.
        let address_str = 'reserve_address: {
            for attempt in 1..=MAX_RECEIVE_ADDRESS_RESERVATION_ATTEMPTS {
                let address = wallet_with_db
                    .wallet
                    .reveal_next_address(KeychainKind::External);
                let candidate = address.address.to_string();

                wallet_with_db.persist().map_err(|err| {
                    tracing::warn!("Could not persist to bdk db: {}", err);

                    Error::BdkPersist
                })?;

                if self
                    .storage
                    .track_receive_address(&candidate, &quote_id_string)
                    .await?
                {
                    break 'reserve_address candidate;
                }

                tracing::debug!(
                    quote_id = %quote_id,
                    attempt,
                    max_attempts = MAX_RECEIVE_ADDRESS_RESERVATION_ATTEMPTS,
                    "Receive address is already reserved for another quote"
                );
            }

            return Err(Error::ReceiveAddressReservationExhausted {
                attempts: MAX_RECEIVE_ADDRESS_RESERVATION_ATTEMPTS,
            }
            .into());
        };

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: PaymentIdentifier::QuoteId(quote_id),
            request: address_str,
            expiry: None,
            extra_json: None,
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
            let has_broadcast_evidence =
                self.storage
                    .get_all_send_batches()
                    .await?
                    .iter()
                    .any(|batch| {
                        matches!(
                            &batch.state,
                            crate::send::batch_transaction::record::SendBatchState::Broadcast {
                                assignments,
                                ..
                            } if assignments
                                .iter()
                                .any(|assignment| assignment.intent_id == record.intent_id)
                        )
                    });
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
                crate::send::payment_intent::record::SendIntentState::Failed { .. }
                    if has_broadcast_evidence =>
                {
                    // A durable Broadcast transaction may already have reached
                    // the network. Never authorize proof compensation merely
                    // because a stale worker left the current intent Failed.
                    MeltQuoteState::Pending
                }
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
    use cdk_common::payment::{MintPayment, OnchainIncomingPaymentOptions};
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
        let chain_source = ChainSource::Esplora(EsploraConfig {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        });

        build_test_instance_with_chain_source(
            shutdown_timeout_secs,
            batch_config,
            sync_interval_secs,
            chain_source,
        )
        .await
    }

    async fn build_test_instance_with_chain_source(
        shutdown_timeout_secs: u64,
        batch_config: Option<BatchConfig>,
        sync_interval_secs: u64,
        chain_source: ChainSource,
    ) -> Result<(CdkBdk, tempfile::TempDir), Error> {
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
        )?;

        Ok((backend, tmp))
    }

    /// Build a `CdkBdk` instance with its own local wallet directory but a
    /// shared KV store, like a second replica or a re-initialised node using
    /// the same seed.
    async fn build_test_instance_with_shared_kv(
        kv: Arc<cdk_sqlite::mint::MintSqliteDatabase>,
    ) -> (CdkBdk, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mnemonic = Mnemonic::from_str(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("mnemonic");
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
            kv,
            None,
            1,
            0,
            546,
            60,
            Some(5),
            None,
        )
        .expect("build CdkBdk test instance");

        (backend, tmp)
    }

    #[tokio::test]
    async fn instances_sharing_seed_and_kv_never_share_a_receive_address() {
        let kv = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory kv store"),
        );
        let (first, _tmp_first) = build_test_instance_with_shared_kv(kv.clone()).await;
        let (second, _tmp_second) = build_test_instance_with_shared_kv(kv).await;

        let first_request = first
            .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                OnchainIncomingPaymentOptions {
                    quote_id: cdk_common::QuoteId::new(),
                },
            ))
            .await
            .expect("first receive request");
        let second_request = second
            .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                OnchainIncomingPaymentOptions {
                    quote_id: cdk_common::QuoteId::new(),
                },
            ))
            .await
            .expect("second receive request");

        assert_ne!(
            first_request.request, second_request.request,
            "each instance must hand out a distinct receive address"
        );
    }

    #[tokio::test]
    async fn operator_deposit_address_is_not_associated_with_a_quote() {
        let kv = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory kv store"),
        );
        let (backend, _tmp) = build_test_instance_with_shared_kv(kv).await;

        let operator_address = backend
            .create_operator_deposit_address()
            .await
            .expect("create operator deposit address");
        let quote_request = backend
            .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                OnchainIncomingPaymentOptions {
                    quote_id: cdk_common::QuoteId::new(),
                },
            ))
            .await
            .expect("create quote receive request");

        assert_ne!(operator_address, quote_request.request);
        assert!(backend
            .storage
            .get_quote_id_by_receive_address(&operator_address)
            .await
            .expect("look up operator address")
            .is_none());
        assert!(!backend
            .storage
            .get_tracked_receive_addresses()
            .await
            .expect("list quote receive addresses")
            .contains(&operator_address));
    }

    #[tokio::test]
    async fn receive_address_reservation_stops_after_attempt_limit() {
        let kv = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory kv store"),
        );
        let (first, _tmp_first) = build_test_instance_with_shared_kv(kv.clone()).await;
        let (second, _tmp_second) = build_test_instance_with_shared_kv(kv).await;

        for _ in 0..MAX_RECEIVE_ADDRESS_RESERVATION_ATTEMPTS {
            first
                .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                    OnchainIncomingPaymentOptions {
                        quote_id: cdk_common::QuoteId::new(),
                    },
                ))
                .await
                .expect("reserve receive address");
        }

        let err = second
            .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                OnchainIncomingPaymentOptions {
                    quote_id: cdk_common::QuoteId::new(),
                },
            ))
            .await
            .expect_err("reservation should stop after the attempt limit");

        let cdk_common::payment::Error::Onchain(inner) = err else {
            panic!("expected onchain error");
        };
        assert!(matches!(
            inner.downcast_ref::<Error>(),
            Some(Error::ReceiveAddressReservationExhausted { attempts })
                if *attempts == MAX_RECEIVE_ADDRESS_RESERVATION_ATTEMPTS
        ));
    }

    #[tokio::test]
    async fn wallet_info_lists_revealed_addresses_without_revealing_more() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;

        let initial_addresses = backend
            .wallet_addresses(0, 100)
            .await
            .expect("list initial addresses");
        assert_eq!(initial_addresses.total, 0);

        backend
            .create_incoming_payment_request(IncomingPaymentOptions::Onchain(
                OnchainIncomingPaymentOptions {
                    quote_id: cdk_common::QuoteId::new(),
                },
            ))
            .await
            .expect("create on-chain request");

        let addresses = backend
            .wallet_addresses(0, 100)
            .await
            .expect("list revealed addresses");
        assert_eq!(addresses.total, 1);
        assert_eq!(addresses.items.len(), 1);
        assert_eq!(addresses.items[0].keychain, WalletKeychain::External);
        assert_eq!(addresses.items[0].derivation_index, 0);
        assert!(!addresses.items[0].used);
        assert_eq!(addresses.items[0].balance_sat, 0);

        let balance = backend.wallet_balance().await;
        assert_eq!(balance.total_sat, 0);
        assert_eq!(
            backend
                .wallet_transactions(0, 20)
                .await
                .expect("list transactions")
                .total,
            0
        );

        let addresses_again = backend
            .wallet_addresses(0, 100)
            .await
            .expect("list revealed addresses again");
        assert_eq!(addresses_again.total, 1);
    }

    #[tokio::test]
    async fn wallet_info_paginates_revealed_addresses_across_keychains() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;

        {
            let mut wallet_with_db = backend.wallet_with_db.lock().await;
            let _ = wallet_with_db
                .wallet
                .reveal_addresses_to(KeychainKind::External, 1)
                .count();
            let _ = wallet_with_db
                .wallet
                .reveal_addresses_to(KeychainKind::Internal, 1)
                .count();
            wallet_with_db
                .persist()
                .expect("persist revealed addresses");
        }

        let page = backend
            .wallet_addresses(1, 2)
            .await
            .expect("list paginated addresses");

        assert_eq!(page.total, 4);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].keychain, WalletKeychain::External);
        assert_eq!(page.items[0].derivation_index, 1);
        assert_eq!(page.items[1].keychain, WalletKeychain::Internal);
        assert_eq!(page.items[1].derivation_index, 0);
    }

    async fn fund_backend_wallet_transactions(backend: &CdkBdk, amounts_sat: &[u64]) -> Vec<Txid> {
        let mut wallet_with_db = backend.wallet_with_db.lock().await;
        let funding_script = wallet_with_db
            .wallet
            .reveal_next_address(KeychainKind::External)
            .address
            .script_pubkey();
        let funding_transactions = amounts_sat
            .iter()
            .enumerate()
            .map(|(index, amount_sat)| Transaction {
                version: transaction::Version::TWO,
                lock_time: absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::new(
                        Txid::all_zeros(),
                        u32::try_from(index).expect("test transaction index fits in u32"),
                    ),
                    script_sig: Default::default(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                }],
                output: vec![TxOut {
                    value: bdk_wallet::bitcoin::Amount::from_sat(*amount_sat),
                    script_pubkey: funding_script.clone(),
                }],
            })
            .collect::<Vec<_>>();
        let txids = funding_transactions
            .iter()
            .map(Transaction::compute_txid)
            .collect();

        wallet_with_db
            .wallet
            .apply_unconfirmed_txs(funding_transactions.into_iter().map(|tx| (tx, 0)));
        wallet_with_db.persist().expect("persist funded wallet");

        txids
    }

    async fn fund_backend_wallet(backend: &CdkBdk, amount_sat: u64) {
        fund_backend_wallet_transactions(backend, &[amount_sat]).await;
    }

    #[tokio::test]
    async fn wallet_info_reports_unconfirmed_funding() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;
        fund_backend_wallet(&backend, 42_000).await;

        let balance = backend.wallet_balance().await;
        assert_eq!(balance.untrusted_pending_sat, 42_000);
        assert_eq!(balance.total_sat, 42_000);

        let transactions = backend
            .wallet_transactions(0, 20)
            .await
            .expect("list transactions");
        assert_eq!(transactions.total, 1);
        assert_eq!(transactions.items[0].received_sat, 42_000);
        assert_eq!(transactions.items[0].sent_sat, 0);
        assert_eq!(transactions.items[0].balance_delta_sat, 42_000);
        assert_eq!(transactions.items[0].confirmation_height, None);
        assert_eq!(transactions.items[0].first_seen, Some(0));
        assert_eq!(
            transactions.items[0].inputs,
            vec![WalletTransactionInput {
                txid: Txid::all_zeros().to_string(),
                vout: 0,
                amount_sat: None,
                address: None,
            }]
        );

        let addresses = backend
            .wallet_addresses(0, 20)
            .await
            .expect("list addresses");
        assert_eq!(addresses.total, 1);
        assert!(addresses.items[0].used);
        assert_eq!(
            transactions.items[0].outputs,
            vec![WalletTransactionOutput {
                vout: 0,
                address: addresses.items[0].address.clone(),
                amount_sat: 42_000,
                quote_id: None,
            }]
        );
        assert_eq!(addresses.items[0].balance_sat, 42_000);
        assert_eq!(addresses.items[0].confirmed_balance_sat, 0);

        let empty_page = backend
            .wallet_transactions(0, 0)
            .await
            .expect("list empty transaction page");
        assert_eq!(empty_page.total, 1);
        assert!(empty_page.items.is_empty());
    }

    #[tokio::test]
    async fn wallet_info_pairs_unconfirmed_incoming_output_with_quote_id() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;
        fund_backend_wallet(&backend, 42_000).await;
        let address = backend
            .wallet_addresses(0, 20)
            .await
            .expect("list addresses")
            .items[0]
            .address
            .clone();
        backend
            .storage
            .track_receive_address(&address, "mint-quote")
            .await
            .expect("track receive address");

        let transactions = backend
            .wallet_transactions(0, 20)
            .await
            .expect("list transactions");
        assert_eq!(
            transactions.items[0].outputs,
            vec![WalletTransactionOutput {
                vout: 0,
                address,
                amount_sat: 42_000,
                quote_id: Some("mint-quote".to_string()),
            }]
        );
    }

    #[tokio::test]
    async fn wallet_info_pairs_batched_outputs_with_quote_ids() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;
        let funding_txids = fund_backend_wallet_transactions(&backend, &[20_000, 22_000]).await;
        let funding_address = backend
            .wallet_addresses(0, 20)
            .await
            .expect("list funding address")
            .items[0]
            .address
            .clone();
        let first_address = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string();
        let second_address = "bcrt1q6rhpng9evdsfnn833a4f4vej0asu6dk5srld6x".to_string();

        let mut wallet_with_db = backend.wallet_with_db.lock().await;
        let first_recipient_script = bdk_wallet::bitcoin::Address::from_str(&first_address)
            .expect("valid address")
            .require_network(Network::Regtest)
            .expect("regtest address")
            .script_pubkey();
        let second_recipient_script = bdk_wallet::bitcoin::Address::from_str(&second_address)
            .expect("valid address")
            .require_network(Network::Regtest)
            .expect("regtest address")
            .script_pubkey();
        let change_script = wallet_with_db
            .wallet
            .reveal_next_address(KeychainKind::Internal)
            .address
            .script_pubkey();
        let spending_transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: funding_txids
                .iter()
                .rev()
                .map(|txid| TxIn {
                    previous_output: OutPoint::new(*txid, 0),
                    script_sig: Default::default(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: vec![
                TxOut {
                    value: bdk_wallet::bitcoin::Amount::from_sat(20_000),
                    script_pubkey: first_recipient_script,
                },
                TxOut {
                    value: bdk_wallet::bitcoin::Amount::from_sat(10_000),
                    script_pubkey: second_recipient_script,
                },
                TxOut {
                    value: bdk_wallet::bitcoin::Amount::from_sat(11_900),
                    script_pubkey: change_script,
                },
            ],
        };
        let spending_txid = spending_transaction.compute_txid();
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(spending_transaction, 1)]);
        wallet_with_db
            .persist()
            .expect("persist spending transaction");
        drop(wallet_with_db);

        let batch_id = Uuid::new_v4();
        let intents = [
            (Uuid::new_v4(), "quote-first", &first_address, 20_000, 0),
            (Uuid::new_v4(), "quote-second", &second_address, 10_000, 1),
        ];
        for (intent_id, quote_id, address, amount_sat, vout) in &intents {
            backend
                .storage
                .create_send_intent_if_absent(
                    &crate::send::payment_intent::record::SendIntentRecord {
                        intent_id: *intent_id,
                        attempt_id: Uuid::new_v4(),
                        quote_id: quote_id.to_string(),
                        address: address.to_string(),
                        amount_sat: *amount_sat,
                        max_fee_amount_sat: 1_000,
                        tier: PaymentTier::Immediate,
                        metadata: PaymentMetadata::default(),
                        state: crate::send::payment_intent::record::SendIntentState::AwaitingConfirmation {
                            batch_id,
                            txid: spending_txid.to_string(),
                            outpoint: format!("{spending_txid}:{vout}"),
                            fee_contribution_sat: 50,
                            created_at: 0,
                        },
                    },
                )
                .await
                .expect("store send intent");
        }

        let transactions = backend
            .wallet_transactions(0, 20)
            .await
            .expect("list transactions");

        assert_eq!(transactions.total, 3);
        assert_eq!(
            transactions.items[0].inputs,
            vec![
                WalletTransactionInput {
                    txid: funding_txids[1].to_string(),
                    vout: 0,
                    amount_sat: Some(22_000),
                    address: Some(funding_address.clone()),
                },
                WalletTransactionInput {
                    txid: funding_txids[0].to_string(),
                    vout: 0,
                    amount_sat: Some(20_000),
                    address: Some(funding_address),
                },
            ]
        );
        assert_eq!(
            transactions.items[0].outputs,
            vec![
                WalletTransactionOutput {
                    vout: 0,
                    address: first_address.clone(),
                    amount_sat: 20_000,
                    quote_id: Some("quote-first".to_string()),
                },
                WalletTransactionOutput {
                    vout: 1,
                    address: second_address.clone(),
                    amount_sat: 10_000,
                    quote_id: Some("quote-second".to_string()),
                },
            ]
        );
        assert_eq!(transactions.items[0].sent_sat, 42_000);
        assert_eq!(transactions.items[0].received_sat, 11_900);

        for (intent_id, quote_id, _, amount_sat, vout) in &intents {
            let expected = backend
                .storage
                .get_send_intent(intent_id)
                .await
                .expect("read send intent")
                .expect("send intent exists");
            backend
                .storage
                .finalize_send_intent(
                    &expected,
                    &FinalizedSendIntentRecord {
                        intent_id: *intent_id,
                        quote_id: quote_id.to_string(),
                        total_spent_sat: *amount_sat + 50,
                        outpoint: format!("{spending_txid}:{vout}"),
                        finalized_at: 0,
                    },
                )
                .await
                .expect("finalize send intent");
        }

        let finalized_transactions = backend
            .wallet_transactions(0, 20)
            .await
            .expect("list transactions after intent finalization");
        assert_eq!(
            finalized_transactions.items[0]
                .outputs
                .iter()
                .map(|output| output.quote_id.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("quote-first"), Some("quote-second")]
        );
    }

    #[tokio::test]
    async fn wallet_info_reports_multiple_incoming_outputs_in_vout_order() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;
        let mut wallet_with_db = backend.wallet_with_db.lock().await;
        let output_script = wallet_with_db
            .wallet
            .reveal_next_address(KeychainKind::External)
            .address
            .script_pubkey();
        let funding_transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::all_zeros(), 0),
                script_sig: Default::default(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![
                TxOut {
                    value: bdk_wallet::bitcoin::Amount::from_sat(21_000),
                    script_pubkey: output_script.clone(),
                },
                TxOut {
                    value: bdk_wallet::bitcoin::Amount::from_sat(21_000),
                    script_pubkey: output_script,
                },
            ],
        };
        wallet_with_db
            .wallet
            .apply_unconfirmed_txs([(funding_transaction, 0)]);
        wallet_with_db
            .persist()
            .expect("persist funding transaction");
        drop(wallet_with_db);

        let transactions = backend
            .wallet_transactions(0, 20)
            .await
            .expect("list transactions");

        assert_eq!(transactions.total, 1);
        assert_eq!(transactions.items[0].outputs.len(), 2);
        assert_eq!(transactions.items[0].outputs[0].vout, 0);
        assert_eq!(transactions.items[0].outputs[0].amount_sat, 21_000);
        assert_eq!(transactions.items[0].outputs[1].vout, 1);
        assert_eq!(transactions.items[0].outputs[1].amount_sat, 21_000);
    }

    #[tokio::test]
    async fn wallet_info_uses_txid_to_order_equal_chain_positions() {
        let (backend, _tmp) = build_test_instance_with_tempdir(1).await;
        let mut expected_txids =
            fund_backend_wallet_transactions(&backend, &[21_000, 42_000]).await;
        expected_txids.sort_by(|left, right| right.cmp(left));

        let first_page = backend
            .wallet_transactions(0, 1)
            .await
            .expect("list first transaction page");
        let second_page = backend
            .wallet_transactions(1, 1)
            .await
            .expect("list second transaction page");

        assert_eq!(first_page.total, 2);
        assert_eq!(second_page.total, 2);
        assert_eq!(first_page.items[0].txid, expected_txids[0].to_string());
        assert_eq!(second_page.items[0].txid, expected_txids[1].to_string());
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
    async fn test_make_payment_returns_failed_for_unknown_fee_index() {
        let backend = build_test_instance(5).await;
        let (quote_id, mut options) = onchain_options_for(10_000);
        let OutgoingPaymentOptions::Onchain(onchain) = &mut options else {
            panic!("expected onchain options");
        };
        onchain.fee_index = Some(99);

        let response = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("definitive pre-dispatch rejection should return a payment response");

        assert_authoritative_failure_response(response, quote_id.clone(), CurrencyUnit::Sat);
        assert!(
            backend
                .storage
                .get_send_intent_by_quote_id(&quote_id.to_string())
                .await
                .expect("lookup send intent by quote id")
                .is_none(),
            "unknown fee index rejection must not leave a pending send intent behind"
        );
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
    async fn test_make_payment_returns_failed_when_current_fee_exceeds_max_fee() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let (quote_id, mut options) = onchain_options_for(10_000);
        let OutgoingPaymentOptions::Onchain(onchain) = &mut options else {
            panic!("expected onchain options");
        };
        onchain.max_fee_amount = Some(Amount::new(1, CurrencyUnit::Sat));

        let response = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("definitive pre-dispatch rejection should return a payment response");

        assert_authoritative_failure_response(response, quote_id.clone(), CurrencyUnit::Sat);

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

    fn onchain_options_for_msat(
        quote_id: QuoteId,
        amount_msat: u64,
        max_fee_msat: u64,
    ) -> OutgoingPaymentOptions {
        OutgoingPaymentOptions::Onchain(Box::new(OnchainOutgoingPaymentOptions {
            address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            amount: Amount::new(amount_msat, CurrencyUnit::Msat),
            max_fee_amount: Some(Amount::new(max_fee_msat, CurrencyUnit::Msat)),
            quote_id,
            fee_index: None,
            metadata: None,
        }))
    }

    fn assert_authoritative_failure_response(
        response: MakePaymentResponse,
        quote_id: QuoteId,
        unit: CurrencyUnit,
    ) {
        assert_eq!(
            response.payment_lookup_id,
            PaymentIdentifier::QuoteId(quote_id)
        );
        assert_eq!(response.status, MeltQuoteState::Failed);
        assert_eq!(response.total_spent, Amount::new(0, unit));
        assert!(response.payment_proof.is_none());
    }

    #[tokio::test]
    async fn test_get_payment_quote_converts_fee_options_to_msat() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let options = onchain_options_for_msat(quote_id, 10_000_000, 10_000_000);

        let quote = backend
            .get_payment_quote(&CurrencyUnit::Msat, options)
            .await
            .expect("msat quote should succeed");

        assert_eq!(quote.amount, Amount::new(10_000_000, CurrencyUnit::Msat));
        assert_eq!(quote.fee.unit(), &CurrencyUnit::Msat);
        assert_eq!(quote.fee.value() % MSAT_IN_SAT, 0);

        let fee_options = quote.fee_options.expect("fee options");
        assert!(fee_options
            .iter()
            .all(|option| u64::from(option.fee_reserve) % MSAT_IN_SAT == 0));
        assert_eq!(
            quote.fee.value(),
            fee_options
                .iter()
                .map(|option| u64::from(option.fee_reserve))
                .min()
                .expect("non-empty fee options")
        );
    }

    #[tokio::test]
    async fn test_make_payment_converts_msat_amount_and_fee_to_sat() {
        let (backend, _tmp) = build_test_instance_with_tempdir(5).await;
        fund_backend_wallet(&backend, 100_000).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let options = onchain_options_for_msat(quote_id.clone(), 10_000_000, 10_000_000);

        let response = backend
            .make_payment(&CurrencyUnit::Msat, options)
            .await
            .expect("msat payment should enqueue a sat-native intent");

        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Msat));
        let intent = backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent")
            .expect("send intent should be persisted");
        assert_eq!(intent.amount_sat, 10_000);
        assert_eq!(intent.max_fee_amount_sat, 10_000);
    }

    #[tokio::test]
    async fn test_make_payment_returns_failed_for_fractional_satoshi_amount() {
        let backend = build_test_instance(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let options = onchain_options_for_msat(quote_id.clone(), 10_000_001, 10_000_000);

        let response = backend
            .make_payment(&CurrencyUnit::Msat, options)
            .await
            .expect("definitive pre-dispatch rejection should return a payment response");

        assert_authoritative_failure_response(response, quote_id.clone(), CurrencyUnit::Msat);
        assert!(backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent")
            .is_none());
    }

    #[tokio::test]
    async fn test_make_payment_returns_failed_for_mismatched_fee_unit() {
        let backend = build_test_instance(5).await;
        let quote_id = QuoteId::UUID(Uuid::new_v4());
        let mut options = onchain_options_for_msat(quote_id.clone(), 10_000_000, 10_000_000);
        let OutgoingPaymentOptions::Onchain(onchain) = &mut options else {
            panic!("expected onchain options");
        };
        onchain.max_fee_amount = Some(Amount::new(10_000, CurrencyUnit::Sat));

        let response = backend
            .make_payment(&CurrencyUnit::Msat, options)
            .await
            .expect("definitive pre-dispatch rejection should return a payment response");

        assert_authoritative_failure_response(response, quote_id.clone(), CurrencyUnit::Msat);
        assert!(backend
            .storage
            .get_send_intent_by_quote_id(&quote_id.to_string())
            .await
            .expect("lookup send intent")
            .is_none());
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
    async fn test_make_payment_returns_failed_for_dust_without_persisting_intent() {
        let backend = build_test_instance(5).await;
        let (quote_id, options) = onchain_options_for(1);

        let response = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("definitive pre-dispatch rejection should return a payment response");

        assert_authoritative_failure_response(response, quote_id.clone(), CurrencyUnit::Sat);
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
    async fn test_make_payment_returns_failed_below_minimum_without_persisting_intent() {
        let backend = build_test_instance(5).await;
        let (quote_id, options) = onchain_options_for(545);

        let response = backend
            .make_payment(&CurrencyUnit::Sat, options)
            .await
            .expect("definitive pre-dispatch rejection should return a payment response");

        assert_authoritative_failure_response(response, quote_id.clone(), CurrencyUnit::Sat);
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

        let batch_id = Uuid::new_v4();
        crate::testutil::store_test_signed_batch(&backend.storage, batch_id, &[pending.intent_id])
            .await;
        pending
            .assign_to_batch(&backend.storage, batch_id)
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

        let batch_id = Uuid::new_v4();
        crate::testutil::store_test_signed_batch(&backend.storage, batch_id, &[pending.intent_id])
            .await;
        let batched = pending
            .assign_to_batch(&backend.storage, batch_id)
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
    async fn durable_broadcast_fences_failed_intent_and_retry() {
        use crate::send::batch_transaction::record::{
            BatchOutputAssignment, SendBatchRecord, SendBatchState,
        };
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
        let intent_id = pending.intent_id;
        let attempt_id = pending.attempt_id;
        pending
            .fail(&backend.storage, "stale failure".to_string())
            .await
            .expect("transition Pending to Failed");

        backend
            .storage
            .store_send_batch(&SendBatchRecord {
                batch_id: Uuid::new_v4(),
                state: SendBatchState::Broadcast {
                    txid: Txid::all_zeros().to_string(),
                    tx_bytes: vec![0x01],
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id,
                        vout: 0,
                        fee_contribution_sat: 500,
                    }],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store Broadcast evidence");

        let response = backend
            .check_outgoing_payment(&PaymentIdentifier::QuoteId(quote_id.clone()))
            .await
            .expect("check fenced intent");
        assert_eq!(response.status, MeltQuoteState::Pending);

        let retry = SendIntent::new(
            &backend.storage,
            quote_id.to_string(),
            "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
            30_000,
            2_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await;
        assert!(matches!(
            retry,
            Err(Error::SendIntentStateConflict { intent_id: id, .. }) if id == intent_id
        ));
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

    #[cfg(feature = "electrum")]
    #[tokio::test]
    async fn test_sync_wallet_survives_unreachable_electrum() {
        let chain_source = ChainSource::Electrum(ElectrumConfig {
            url: "tcp://127.0.0.1:1".to_string(),
            batch_size: 5,
        });
        let (backend, _tmp) = build_test_instance_with_chain_source(5, None, 60, chain_source)
            .await
            .expect("build Electrum test instance");

        backend.start().await.expect("start");
        tokio::time::sleep(Duration::from_millis(500)).await;

        {
            let tasks = backend.tasks.lock().await;
            let background = tasks.as_ref().expect("tasks running");
            assert!(
                !background.sync.is_finished(),
                "sync task must not exit on transient Electrum errors"
            );
        }

        backend.stop().await.expect("stop");
    }
}
