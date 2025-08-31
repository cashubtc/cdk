//! CDK lightning backend for ldk-node

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::collections::HashMap;
use std::fs;
use std::ops::Deref;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bdk_bitcoind_rpc::bitcoincore_rpc::{Auth, Client, RpcApi};
use bdk_bitcoind_rpc::{Emitter, NO_EXPECTED_MEMPOOL_TXS};
use bdk_wallet::bitcoin::{Address, Network, OutPoint, Transaction};
use bdk_wallet::chain::ChainPosition;
use bdk_wallet::keys::bip39::Mnemonic;
use bdk_wallet::keys::{DerivableKey, ExtendedKey};
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::template::Bip84;
use bdk_wallet::{KeychainKind, PersistedWallet, Wallet};
use cdk_common::common::FeeReserve;
use cdk_common::payment::{self, *};
use cdk_common::{Amount, CurrencyUnit, MeltQuoteState};
use futures::{Stream, StreamExt};
use tokio::sync::{oneshot, Mutex};
use tokio::time::{interval, Duration};
use tokio_stream::wrappers::BroadcastStream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::error::Error;

mod error;

const NUM_CONFS: u32 = 3;

/// Unified command enum for all wallet operations
#[derive(Debug)]
enum CdkCommand {
    /// Process an immediate payout
    ProcessPayout {
        amount: Amount,
        max_fee: Amount,
        address: Address,
        response: oneshot::Sender<Result<(PaymentIdentifier, Amount), Error>>,
    },
    /// Broadcast a transaction
    BroadcastTransaction {
        tx: Transaction,
        response: oneshot::Sender<Result<(), Error>>,
    },
    /// Notify about an incoming payment
    NotifyPayment(WaitPaymentResponse),
    /// Shutdown the command processor
    Shutdown,
}

/// Wrapper struct that combines wallet and database to prevent deadlocks
struct WalletWithDb {
    wallet: PersistedWallet<Connection>,
    db: Connection,
}

impl WalletWithDb {
    fn new(wallet: PersistedWallet<Connection>, db: Connection) -> Self {
        Self { wallet, db }
    }

    fn persist(&mut self) -> Result<bool, bdk_wallet::rusqlite::Error> {
        self.wallet.persist(&mut self.db)
    }
}

/// BDK wallet
#[derive(Clone)]
pub struct CdkBdk {
    _fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    payment_sender: tokio::sync::broadcast::Sender<WaitPaymentResponse>,
    payment_receiver: Arc<tokio::sync::broadcast::Receiver<WaitPaymentResponse>>,
    events_cancel_token: CancellationToken,
    wallet_with_db: Arc<Mutex<WalletWithDb>>,
    chain_source: ChainSource,
    command_sender: tokio::sync::mpsc::Sender<CdkCommand>,
    command_receiver: Arc<Mutex<tokio::sync::mpsc::Receiver<CdkCommand>>>,
    pending_incoming_tx: Arc<Mutex<HashMap<OutPoint, WaitPaymentResponse>>>,
    pending_outgoing_tx: Arc<Mutex<HashMap<OutPoint, MakePaymentResponse>>>,
}

/// Configuration for connecting to Bitcoin RPC
///
/// Contains the necessary connection parameters for Bitcoin Core RPC interface.
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

/// Source of blockchain data for the Lightning node
///
/// Specifies how the node should connect to the Bitcoin network to retrieve
/// blockchain information and broadcast transactions.
#[derive(Debug, Clone)]
pub enum ChainSource {
    /// Use an Esplora server for blockchain data
    ///
    /// Contains the URL of the Esplora server endpoint
    Esplora(String),
    /// Use Bitcoin Core RPC for blockchain data
    ///
    /// Contains the configuration for connecting to Bitcoin Core
    BitcoinRpc(BitcoinRpcConfig),
}

impl CdkBdk {
    /// Create a new CDK LDK Node instance
    ///
    /// # Arguments
    /// * `network` - Bitcoin network (mainnet, testnet, regtest, signet)
    /// * `chain_source` - Source of blockchain data (Esplora or Bitcoin RPC)
    /// * `storage_dir_path` - Directory path for node data storage
    /// * `fee_reserve` - Fee reserve configuration for payments
    ///
    /// # Returns
    /// A new `CdkBdk` instance ready to be started
    ///
    /// # Errors
    /// Returns an error if the LDK node builder fails to create the node
    pub fn new(
        mnemonic: Mnemonic,
        network: Network,
        chain_source: ChainSource,
        storage_dir_path: String,
        fee_reserve: FeeReserve,
    ) -> Result<Self, Error> {
        let storage_dir_path = PathBuf::from_str(&storage_dir_path).map_err(|_| Error::Path)?;
        let storage_dir_path = storage_dir_path.join("bdk_wallet");
        fs::create_dir_all(&storage_dir_path).unwrap();

        let mut db = Connection::open(storage_dir_path.join("bdk_wallet.sqlite"))?;

        let xkey: ExtendedKey = mnemonic.into_extended_key()?;
        // Get xprv from the extended key
        let xprv = xkey.into_xprv(network).ok_or(Error::Path)?;

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

        tracing::info!("Creating tokio channels for payment notifications and commands");
        let (payment_sender, payment_receiver) = tokio::sync::broadcast::channel(8);
        let (command_sender, command_receiver) = tokio::sync::mpsc::channel(10_000);

        Ok(Self {
            _fee_reserve: fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            payment_sender,
            payment_receiver: Arc::new(payment_receiver),
            events_cancel_token: CancellationToken::new(),
            wallet_with_db: Arc::new(Mutex::new(wallet_with_db)),
            chain_source,
            command_sender,
            command_receiver: Arc::new(Mutex::new(command_receiver)),
            pending_incoming_tx: Arc::new(Mutex::new(HashMap::new())),
            pending_outgoing_tx: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Unified command processor that handles all wallet operations
    async fn command_processor(&self) -> Result<(), Error> {
        let mut command_receiver = self.command_receiver.lock().await;

        while let Some(command) = command_receiver.recv().await {
            match command {
                CdkCommand::ProcessPayout {
                    amount,
                    max_fee,
                    address,
                    response,
                } => {
                    tracing::info!("Processing payout: {} sats to {}", amount, address);

                    let result = self.process_payout_internal(amount, max_fee, address).await;

                    if let Err(err) = response.send(result) {
                        tracing::error!("Failed to send payout response: {:?}", err);
                    }
                }
                CdkCommand::BroadcastTransaction { tx, response } => {
                    tracing::info!("Broadcasting transaction: {}", tx.compute_txid());

                    let result = self.broadcast_transaction_internal(tx).await;

                    if let Err(err) = response.send(result) {
                        tracing::error!("Failed to send broadcast response: {:?}", err);
                    }
                }
                CdkCommand::NotifyPayment(payment_response) => {
                    tracing::info!(
                        "Notifying payment: {:?}",
                        payment_response.payment_identifier
                    );

                    if let Err(err) = self.payment_sender.send(payment_response) {
                        tracing::error!("Failed to send payment notification: {}", err);
                    }
                }
                CdkCommand::Shutdown => {
                    tracing::info!("Command processor shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Internal payout processing method
    async fn process_payout_internal(
        &self,
        amount: Amount,
        max_fee: Amount,
        address: Address,
    ) -> Result<(PaymentIdentifier, Amount), Error> {
        let mut wallet_with_db = self.wallet_with_db.lock().await;

        let mut tx = wallet_with_db.wallet.build_tx();

        tx.add_recipient(
            address,
            bdk_wallet::bitcoin::Amount::from_sat(amount.into()),
        );

        let mut psbt = tx.finish().map_err(|e| Error::Wallet(e.to_string()))?;

        let fee = psbt.fee().map_err(|e| Error::Wallet(e.to_string()))?;

        if fee.to_sat() > max_fee.into() {
            return Err(Error::FeeTooHigh {
                fee: fee.to_sat(),
                max_fee: max_fee.into(),
            });
        }

        wallet_with_db.persist()?;

        if !wallet_with_db
            .wallet
            .sign(&mut psbt, Default::default())
            .map_err(|e| Error::Wallet(e.to_string()))?
        {
            return Err(Error::CouldNotSign);
        }

        let tx = psbt
            .extract_tx()
            .map_err(|e| Error::Wallet(e.to_string()))?;

        let txid = tx.compute_txid();

        // Use command channel for broadcasting
        let (broadcast_sender, broadcast_receiver) = oneshot::channel();
        let broadcast_command = CdkCommand::BroadcastTransaction {
            tx,
            response: broadcast_sender,
        };

        self.command_sender.send(broadcast_command).await?;

        // Wait for broadcast result (optional - could be fire-and-forget)
        if let Err(err) = broadcast_receiver.await {
            tracing::warn!("Broadcast operation failed: {:?}", err);
        }

        Ok((
            PaymentIdentifier::CustomId(txid.to_string()),
            amount + fee.to_sat().into(),
        ))
    }

    /// Internal transaction broadcasting method
    async fn broadcast_transaction_internal(&self, tx: Transaction) -> Result<(), Error> {
        // Placeholder for actual broadcasting implementation
        tracing::info!("Broadcasting transaction: {}", tx.compute_txid());
        // TODO: Implement actual broadcasting to network
        println!("Broadcasting transaction: {:?}", tx.compute_txid());
        Ok(())
    }

    async fn sync_wallet(&self) -> Result<(), Error> {
        match &self.chain_source {
            ChainSource::BitcoinRpc(rpc_config) => {
                let rpc_client: Client = Client::new(
                    "http://127.0.0.1:18443",
                    Auth::UserPass(rpc_config.user.clone(), rpc_config.password.clone()),
                )?;

                let blockchain_info = rpc_client.get_blockchain_info()?;
                println!(
                    "\nConnected to Bitcoin Core RPC.\nChain: {}\nLatest block: {} at height {}\n",
                    blockchain_info.chain, blockchain_info.best_block_hash, blockchain_info.blocks,
                );

                // Continue monitoring for new blocks
                let mut sync_interval = interval(Duration::from_secs(30)); // Check every 30 seconds

                println!("Starting continuous block monitoring...");
                loop {
                    tokio::select! {
                        // Cancel token arm
                        _ = self.events_cancel_token.cancelled() => {
                            tracing::info!("Wallet sync cancelled via cancel token");
                            self.command_sender.send(CdkCommand::Shutdown).await.ok();
                            break;
                        }

                        // Sync interval arm
                        _ = sync_interval.tick() => {
                            let mut found_blocks = vec![];


                            {


                            let mut wallet_with_db = self.wallet_with_db.lock().await;
                            let wallet_tip = wallet_with_db.wallet.latest_checkpoint();

                            let mut emitter = Emitter::new(
                                &rpc_client,
                                wallet_tip.clone(),
                                wallet_tip.height(),
                                NO_EXPECTED_MEMPOOL_TXS,
                            );

                            while let Some(block) = emitter.next_block()? {
                                found_blocks.push(block.block_height());

                                wallet_with_db
                                    .wallet
                                    .apply_block_connected_to(
                                        &block.block,
                                        block.block_height(),
                                        block.connected_to(),
                                    )
                                    .map_err(|e| Error::Wallet(e.to_string()))?;
                            }

                            if !found_blocks.is_empty() {
                                println!(); // New line after printing block heights
                                wallet_with_db.persist()?;
                                tracing::info!("Wallet synced with new blocks");
                            }

                            let checkpoint = wallet_with_db.wallet.latest_checkpoint();

                            tracing::info!("New block {} at height {}", checkpoint.block_id().hash, checkpoint.block_id().height);

                            }

                            for block in found_blocks {
                                self.process_block(block).await?;
                            }

                            self.check_pending_outgoing().await?;

                            self.check_pending_incoming().await?;

                        }
                    }
                }
            }
            _ => return Err(Error::UnsupportedOnchain),
        };

        Ok(())
    }

    async fn process_block(&self, block_height: u32) -> Result<(), Error> {
        let wallet_with_db = self.wallet_with_db.lock().await;

        let txs: Vec<Transaction> = wallet_with_db
            .wallet
            .transactions()
            .filter_map(|tx| match &tx.chain_position {
                ChainPosition::Confirmed { anchor, .. } => {
                    if anchor.block_id.height == block_height {
                        Some(tx.tx_node.tx.deref().clone())
                    } else {
                        None
                    }
                }
                ChainPosition::Unconfirmed { .. } => None,
            })
            .collect();

        drop(wallet_with_db);

        let mut pending_incoming = self.pending_incoming_tx.lock().await;

        for tx in txs {
            for (vout, out) in tx.output.iter().enumerate() {
                let outpoint = OutPoint::new(tx.compute_txid(), vout as u32);
                let wait_payment_response = WaitPaymentResponse {
                    payment_identifier: PaymentIdentifier::OnchainAddress(
                        // TODO what address type?
                        out.script_pubkey.to_p2wsh().to_string(),
                    ),
                    payment_amount: out.value.to_sat().into(),
                    unit: CurrencyUnit::Sat,
                    payment_id: outpoint.to_string(),
                };
                pending_incoming.insert(outpoint, wait_payment_response);
            }
        }

        Ok(())
    }

    async fn check_pending_outgoing(&self) -> Result<(), Error> {
        let mut pending_tx = self.pending_outgoing_tx.lock().await;
        let wallet_with_db = self.wallet_with_db.lock().await;

        let check_point = wallet_with_db.wallet.latest_checkpoint().height();

        let older_then = check_point - NUM_CONFS;

        let mut to_remove = vec![];

        for (outpoint, _make_payment_response) in pending_tx.iter() {
            if let Some(tx) = wallet_with_db.wallet.get_tx(outpoint.txid) {
                match &tx.chain_position {
                    ChainPosition::Confirmed { anchor, .. } => {
                        if anchor.block_id.height < older_then {
                            to_remove.push(*outpoint);
                            // TODO we need another channel to notifier of update
                        }
                    }
                    ChainPosition::Unconfirmed { .. } => (),
                }
            };
        }

        pending_tx.retain(|t, _| !to_remove.contains(t));

        Ok(())
    }

    async fn check_pending_incoming(&self) -> Result<(), Error> {
        let mut pending_tx = self.pending_incoming_tx.lock().await;
        let wallet_with_db = self.wallet_with_db.lock().await;

        let check_point = wallet_with_db.wallet.latest_checkpoint().height();

        let older_then = check_point - NUM_CONFS;

        let mut to_remove = vec![];

        for (outpoint, make_payment_response) in pending_tx.iter() {
            if let Some(tx) = wallet_with_db.wallet.get_tx(outpoint.txid) {
                match &tx.chain_position {
                    ChainPosition::Confirmed { anchor, .. } => {
                        if anchor.block_id.height < older_then {
                            to_remove.push(*outpoint);
                            if let Err(err) =
                                self.payment_sender.send(make_payment_response.clone())
                            {
                                tracing::error!("Could not send wait payment response: {}", err);
                            }
                        }
                    }
                    ChainPosition::Unconfirmed { .. } => (),
                }
            };
        }

        pending_tx.retain(|t, _| !to_remove.contains(t));

        Ok(())
    }
}

/// Mint payment trait
#[async_trait]
impl MintPayment for CdkBdk {
    type Err = payment::Error;

    /// Start the payment processor
    /// Starts the unified command processor and blockchain sync
    async fn start(&self) -> Result<(), Self::Err> {
        // Start the unified command processor
        let clone_self = self.clone();
        tokio::spawn(async move {
            if let Err(e) = clone_self.command_processor().await {
                tracing::error!("Command processor task failed: {}", e);
            }
        });

        // Start the wallet sync task
        let clone_self = self.clone();
        tokio::spawn(async move {
            if let Err(e) = clone_self.sync_wallet().await {
                tracing::error!("Sync wallet task failed: {}", e);
            }
        });

        Ok(())
    }

    /// Stop the payment processor
    /// Gracefully stops the LDK node and cancels all background tasks
    async fn stop(&self) -> Result<(), Self::Err> {
        Ok(())
    }

    /// Base Settings
    async fn get_settings(&self) -> Result<serde_json::Value, Self::Err> {
        let settings = Bolt11Settings {
            mpp: false,
            unit: CurrencyUnit::Sat,
            invoice_description: false,
            amountless: true,
            bolt12: false,
            onchain: true,
        };
        Ok(serde_json::to_value(settings)?)
    }

    /// Create a new invoice
    #[instrument(skip(self))]
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Onchain => {
                let mut wallet_with_db = self.wallet_with_db.lock().await;

                let address = wallet_with_db
                    .wallet
                    .reveal_next_address(KeychainKind::External);

                wallet_with_db.persist().map_err(Error::from)?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::OnchainAddress(
                        address.address.to_string(),
                    ),
                    request: address.address.to_string(),
                    expiry: None,
                })
            }
            IncomingPaymentOptions::Bolt11(_bolt11_options) => {
                Err(Error::UnsupportedOnchain.into())
            }
            IncomingPaymentOptions::Bolt12(_bolt12_options) => {
                Err(Error::UnsupportedOnchain.into())
            }
        }
    }

    /// Get payment quote
    /// Used to get fee and amount required for a payment request
    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Error::UnsupportedOnchain.into());
        }

        match options {
            OutgoingPaymentOptions::Onchain(onchain_options) => {
                let address = onchain_options.address;

                let mut wallet_with_db = self.wallet_with_db.lock().await;

                let mut tx = wallet_with_db.wallet.build_tx();

                tx.add_recipient(
                    address.script_pubkey(),
                    bdk_wallet::bitcoin::Amount::from_sat(onchain_options.amount.into()),
                );

                let psbt = tx.finish().map_err(|e| Error::Wallet(e.to_string()))?;

                let fee = psbt.fee().map_err(|e| Error::Wallet(e.to_string()))?;

                wallet_with_db.wallet.cancel_tx(&psbt.unsigned_tx);

                let fee = fee.to_sat();

                Ok(PaymentQuoteResponse {
                    request_lookup_id: None,
                    amount: onchain_options.amount,
                    fee: fee.into(),
                    unit: CurrencyUnit::Sat,
                    state: MeltQuoteState::Unpaid,
                })
            }
            OutgoingPaymentOptions::Bolt11(_bolt11_options) => {
                Err(Error::UnsupportedOnchain.into())
            }
            OutgoingPaymentOptions::Bolt12(_bolt12_options) => {
                Err(Error::UnsupportedOnchain.into())
            }
        }
    }

    /// Pay request
    #[instrument(skip(self, options))]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Error::UnsupportedOnchain.into());
        }

        match options {
            OutgoingPaymentOptions::Onchain(outgoing) => {
                let (response_sender, response_receiver) = oneshot::channel();

                let command = CdkCommand::ProcessPayout {
                    amount: outgoing.amount,
                    max_fee: outgoing.max_fee_amount.unwrap_or_default(),
                    address: outgoing.address,
                    response: response_sender,
                };

                self.command_sender
                    .send(command)
                    .await
                    .map_err(Error::from)?;

                let result = response_receiver.await.map_err(Error::from)?;
                let (ident, total_amount) = result.map_err(Error::from)?;

                Ok(MakePaymentResponse {
                    payment_lookup_id: ident,
                    unit: CurrencyUnit::Sat,
                    payment_proof: None,
                    status: MeltQuoteState::Pending,
                    total_spent: total_amount,
                })
            }
            OutgoingPaymentOptions::Bolt11(_bolt11_options) => {
                Err(Error::UnsupportedOnchain.into())
            }

            OutgoingPaymentOptions::Bolt12(_bolt12_options) => {
                Err(Error::UnsupportedOnchain.into())
            }
        }
    }

    /// Listen for invoices to be paid to the mint
    /// Returns a stream of request_lookup_id once invoices are paid
    #[instrument(skip(self))]
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitPaymentResponse> + Send>>, Self::Err> {
        tracing::info!("Starting stream for invoices - wait_any_incoming_payment called");

        // Set active flag to indicate stream is active
        self.wait_invoice_is_active.store(true, Ordering::SeqCst);
        tracing::debug!("wait_invoice_is_active set to true");

        let receiver = self.payment_receiver.clone();

        tracing::info!("Receiver obtained successfully, creating response stream");

        // Transform the String stream into a WaitPaymentResponse stream
        let response_stream = BroadcastStream::new(receiver.resubscribe());

        // Map the stream to handle BroadcastStreamRecvError
        let response_stream = response_stream.filter_map(|result| async move {
            match result {
                Ok(payment) => Some(payment),
                Err(err) => {
                    tracing::warn!("Error in broadcast stream: {}", err);
                    None
                }
            }
        });

        // Create a combined stream that also handles cancellation
        let cancel_token = self.wait_invoice_cancel_token.clone();
        let is_active = self.wait_invoice_is_active.clone();

        let stream = Box::pin(response_stream);

        // Set up a task to clean up when the stream is dropped
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            tracing::info!("wait_invoice stream cancelled");
            is_active.store(false, Ordering::SeqCst);
        });

        tracing::info!("wait_any_incoming_payment returning stream");
        Ok(stream)
    }

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    /// Check the status of an incoming payment
    async fn check_incoming_payment_status(
        &self,
        _payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        todo!()
    }

    /// Check the status of an outgoing payment
    async fn check_outgoing_payment(
        &self,
        _request_lookup_id: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        todo!()
    }
}

impl Drop for CdkBdk {
    fn drop(&mut self) {
        tracing::info!("Drop called on CdkLdkNode");
        self.wait_invoice_cancel_token.cancel();
        tracing::debug!("Cancelled wait_invoice token in drop");
    }
}
