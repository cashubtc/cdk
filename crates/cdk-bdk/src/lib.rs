//! CDK onchain backend using BDK

#![doc = include_str!("../README.md")]

use std::fs;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;

// use uuid::Uuid;
pub use crate::error::Error;
pub use crate::storage::{BdkStorage, FinalizedReceiveIntentRecord, FinalizedSendIntentRecord};
pub use crate::types::{BatchConfig, PaymentMetadata, PaymentTier};

pub mod error;
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
    pub(crate) events_cancel_token: CancellationToken,
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
            events_cancel_token: CancellationToken::new(),
            wallet_with_db: Arc::new(Mutex::new(wallet_with_db)),
            chain_source,
            storage: BdkStorage::new(kv_store),
            network,
            batch_config,
            batch_notify: Arc::new(Notify::new()),
            num_confs,
            min_receive_amount_sat,
        })
    }
}

#[async_trait]
impl MintPayment for CdkBdk {
    type Err = cdk_common::payment::Error;

    #[tracing::instrument(skip_all)]
    async fn start(&self) -> Result<(), Self::Err> {
        self.recover_receive_saga().await?;
        self.recover_send_saga().await?;

        let sync_self = self.clone();
        let sync_cancel = self.events_cancel_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = sync_cancel.cancelled() => {
                    tracing::info!("Wallet sync task cancelled");
                }
                res = sync_self.sync_wallet() => {
                    if let Err(e) = res {
                        tracing::error!("Wallet sync task failed: {}", e);
                    }
                }
            }
        });

        let batch_self = self.clone();
        let batch_cancel = self.events_cancel_token.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = batch_cancel.cancelled() => {
                    tracing::info!("Batch processor task cancelled");
                }
                res = batch_self.run_batch_processor() => {
                    if let Err(e) = res {
                        tracing::error!("Batch processor task failed: {}", e);
                    }
                }
            }
        });

        Ok(())
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        self.events_cancel_token.cancel();
        self.wait_invoice_cancel_token.cancel();
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

        let estimated_fee_sat = 1000; // Placeholder for actual fee estimation
        let fee_reserve_sat = self.fee_reserve_for_estimate(estimated_fee_sat);

        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::QuoteId(onchain_options.quote_id.clone())),
            amount: onchain_options.amount,
            fee: Amount::new(fee_reserve_sat, CurrencyUnit::Sat),
            state: MeltQuoteState::Unpaid,
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
            amount.clone().to_u64(),
            max_fee.to_u64(),
            tier,
            metadata,
        )
        .await?;

        if tier == PaymentTier::Immediate {
            self.batch_notify.notify_one();
        }

        Ok(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::QuoteId(quote_id),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: amount,
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
        Ok(Box::pin(
            BroadcastStream::new(receiver).filter_map(|event| async move { event.ok() }),
        ))
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
                total_spent: Amount::new(record.amount_sat, CurrencyUnit::Sat),
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
