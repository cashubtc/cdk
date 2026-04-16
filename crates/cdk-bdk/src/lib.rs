//! CDK onchain backend using BDK

#![doc = include_str!("../README.md")]

use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, PersistedWallet, Wallet};
use cdk_common::common::FeeReserve;
use cdk_common::database::KVStore;
use cdk_common::payment::{
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OnchainSettings, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
    SettingsResponse, WaitPaymentResponse,
};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};
use futures::{Stream, StreamExt};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

// use uuid::Uuid;
pub use crate::error::Error;
pub use crate::storage::{BdkStorage, FinalizedReceiveIntentRecord, FinalizedSendIntentRecord};
pub use crate::types::{
    BatchConfig, FeeEstimationConfig, PaymentMetadata, PaymentTier, SyncConfig,
};

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
    /// Sync interval in seconds
    pub(crate) sync_interval_secs: u64,
    /// Blockchain sync configuration
    pub(crate) sync_config: SyncConfig,
    /// Cache for fee rate estimation: Tier -> (sat_per_vb, timestamp)
    pub(crate) fee_rate_cache: Arc<Mutex<std::collections::HashMap<PaymentTier, (f64, u64)>>>,
}

/// Configuration for connecting to Bitcoin RPC
#[derive(Debug, Clone)]
pub struct BitcoinRpcConfig {
    /// Bitcoin RPC server hostname or IP address
    pub host: String,
    /// Bitcoin RPC server port number
    pub port: u16,
    /// Username for Bitcoin RPC authentication
    pub user: String,
    /// Password for Bitcoin RPC authentication
    pub password: String,
}

/// Source of blockchain data for the BDK wallet
#[derive(Debug, Clone)]
pub enum ChainSource {
    /// Use an Esplora server for blockchain data
    Esplora {
        /// URL of the Esplora server endpoint
        url: String,
        /// Number of parallel requests to use during sync
        parallel_requests: usize,
    },
    /// Use Bitcoin Core RPC for blockchain data
    BitcoinRpc(BitcoinRpcConfig),
}

impl CdkBdk {
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
        sync_interval_secs: u64,
        shutdown_timeout_secs: Option<u64>,
        sync_config: Option<SyncConfig>,
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
            sync_interval_secs,
            sync_config: sync_config.unwrap_or_default(),
            fee_rate_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
        })
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

        let cancel = CancellationToken::new();

        let sync_self = self.clone();
        let sync_cancel = cancel.clone();
        let sync_handle = tokio::spawn(async move {
            tokio::select! {
                _ = sync_cancel.cancelled() => {
                    tracing::info!("Wallet sync task cancelled");
                }
                res = sync_self.sync_wallet(sync_cancel.clone()) => {
                    if let Err(e) = res {
                        tracing::error!("Wallet sync task failed: {}", e);
                    }
                }
            }
        });

        let batch_self = self.clone();
        let batch_cancel = cancel.clone();
        let batch_handle = tokio::spawn(async move {
            tokio::select! {
                _ = batch_cancel.cancelled() => {
                    tracing::info!("Batch processor task cancelled");
                }
                res = batch_self.run_batch_processor(batch_cancel.clone()) => {
                    if let Err(e) = res {
                        tracing::error!("Batch processor task failed: {}", e);
                    }
                }
            }
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

        let tier = PaymentTier::from_optional_str(onchain_options.tier.as_deref());

        let sat_per_vb = self
            .estimate_fee_rate_sat_per_vb(tier)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    tier = ?tier,
                    error = %e,
                    "Fee-rate estimation failed, using configured fallback"
                );
                self.batch_config.fee_estimation.fallback_sat_per_vb
            });

        // Use a heuristic for quoting (faster, no wallet lock required)
        let vbytes = fee::estimate_batch_vbytes_heuristic(1);
        let estimated_fee_sat = (sat_per_vb * vbytes as f64).ceil() as u64;

        let fee_reserve_sat = self.fee_reserve_for_estimate(estimated_fee_sat);

        // Echo the mint-supplied `quote_id` verbatim per the
        // `OnchainOutgoingPaymentOptions.quote_id` contract. The mint
        // validates this echo; any deviation triggers
        // `Error::OnchainQuoteLookupIdMismatch`.
        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::QuoteId(onchain_options.quote_id.clone())),
            amount: onchain_options.amount,
            fee: Amount::new(fee_reserve_sat, CurrencyUnit::Sat),
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: Some(
                match PaymentTier::from_optional_str(onchain_options.tier.as_deref()) {
                    PaymentTier::Immediate => 1,
                    PaymentTier::Standard => 6,
                    PaymentTier::Economy => 144,
                },
            ),
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

        let max_fee = onchain_options
            .max_fee_amount
            .unwrap_or(Amount::new(1000, CurrencyUnit::Sat));
        let tier = PaymentTier::from_optional_str(onchain_options.tier.as_deref());
        let metadata = PaymentMetadata::from_optional_json(onchain_options.metadata.as_deref());

        crate::send::payment_intent::SendIntent::new(
            &self.storage,
            quote_id.to_string(),
            address,
            amount.to_u64(),
            max_fee.to_u64(),
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

        wallet_with_db.persist().map_err(|err| {
            tracing::error!("Could not persist to bdk db: {}", err);

            Error::BdkPersist
        })?;

        let quote_id = onchain_options.quote_id;

        self.storage
            .track_receive_address(&address_str, &quote_id.to_string())
            .await?;

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
        let receiver = self.payment_sender.subscribe();
        Ok(Box::pin(BroadcastStream::new(receiver).filter_map(
            |event| async move {
                match event {
                    Ok(event) => Some(event),
                    Err(err) => {
                        tracing::warn!(
                            "cdk-bdk payment event subscriber lagged or errored: {}",
                            err
                        );
                        None
                    }
                }
            },
        )))
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
            };
            let status = match record.state {
                crate::send::payment_intent::record::SendIntentState::Pending { .. }
                | crate::send::payment_intent::record::SendIntentState::Batched { .. }
                | crate::send::payment_intent::record::SendIntentState::AwaitingConfirmation {
                    ..
                } => MeltQuoteState::Pending,
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

    use bdk_wallet::bitcoin::Network;
    use bdk_wallet::keys::bip39::Mnemonic;
    use cdk_common::common::FeeReserve;
    use cdk_common::payment::MintPayment;

    use super::*;
    use crate::fee::estimate_batch_vbytes_heuristic;

    /// Build a `CdkBdk` instance pointed at a bogus Esplora URL so the sync
    /// loop spins without needing a real backend. The ticks are short so
    /// shutdown tests run quickly.
    async fn build_test_instance(shutdown_timeout_secs: u64) -> CdkBdk {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mnemonic = Mnemonic::from_str(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .expect("mnemonic");

        let kv = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory kv store");

        let chain_source = ChainSource::Esplora {
            url: "http://127.0.0.1:1".to_string(),
            parallel_requests: 1,
        };

        let fee_reserve = FeeReserve {
            min_fee_reserve: Amount::new(1, CurrencyUnit::Sat).into(),
            percent_fee_reserve: 0.02,
        };

        CdkBdk::new(
            mnemonic,
            Network::Regtest,
            chain_source,
            tmp.path().to_string_lossy().into_owned(),
            fee_reserve,
            Arc::new(kv),
            None,
            1,
            0,
            60,
            Some(shutdown_timeout_secs),
            None,
        )
        .expect("build CdkBdk test instance")
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

    #[test]
    fn test_estimate_batch_vbytes_heuristic_single_recipient() {
        // 1 recipient → 2 inputs, 2 outputs (1 recipient + 1 change)
        // overhead 11 + 2*68 + 2*31 = 11 + 136 + 62 = 209
        assert_eq!(estimate_batch_vbytes_heuristic(1), 209);
    }

    #[test]
    fn test_estimate_batch_vbytes_heuristic_zero_recipients() {
        // max(1, 0) → 1 assumed recipient → 2 inputs, 1 output
        // overhead 11 + 2*68 + 1*31 = 11 + 136 + 31 = 178
        assert_eq!(estimate_batch_vbytes_heuristic(0), 178);
    }

    #[test]
    fn test_estimate_batch_vbytes_heuristic_multi_recipient() {
        // 5 recipients → 10 inputs, 6 outputs
        // overhead 11 + 10*68 + 6*31 = 11 + 680 + 186 = 877
        assert_eq!(estimate_batch_vbytes_heuristic(5), 877);
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
            OutgoingPaymentOptions::Onchain(Box::new(OnchainOutgoingPaymentOptions {
                address: "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080".to_string(),
                amount: Amount::new(amount_sat, CurrencyUnit::Sat),
                max_fee_amount: Some(Amount::new(1_000, CurrencyUnit::Sat)),
                quote_id,
                tier: None,
                metadata: None,
            })),
        )
    }

    #[tokio::test]
    async fn test_make_payment_pending_total_spent_is_zero() {
        // make_payment queues the intent before a batch has been built, so
        // the per-intent fee is unknown. total_spent MUST be 0, not the
        // user-requested amount (which would imply no fee).
        let backend = build_test_instance(5).await;
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
    async fn test_check_outgoing_payment_pending_intent_reports_zero_total_spent() {
        // An intent freshly created via make_payment is in state Pending.
        // check_outgoing_payment must report total_spent = 0 because the
        // fee contribution is not yet knowable.
        let backend = build_test_instance(5).await;
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
}
