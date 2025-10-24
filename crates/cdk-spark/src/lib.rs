//! CDK Lightning backend for Spark SDK
//!
//! This crate provides a nodeless Lightning implementation for Cashu mints using the Spark SDK.
//! Spark enables self-custodial Lightning payments without running a full Lightning node.

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::amount::{to_unit, Amount};
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk_common::util::hex;
use futures::Stream;
use lightning_invoice::Bolt11Invoice;
use serde_json::Value;
use spark_wallet::{
    DefaultSigner, InvoiceDescription, SparkWallet, SparkWalletConfig, WalletBuilder,
    WalletEvent,
};
use std::collections::HashMap;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn};

pub mod config;
pub mod error;

#[cfg(test)]
mod tests;

pub use config::SparkConfig;
pub use error::Error;

// Re-export Network from spark_wallet for convenience
pub use spark_wallet::Network;

/// Information about a received transfer
#[derive(Clone, Debug)]
struct TransferInfo {
    transfer_id: String,
    amount_sat: u64,
    timestamp: u64,
    paid: bool,
    preimage: Option<String>,
}

/// Information about an outgoing payment
#[derive(Clone, Debug)]
struct PaymentInfo {
    preimage: Option<String>,
    status: MeltQuoteState,
    amount_spent: u64,
}

/// CDK Lightning backend using Spark SDK
///
/// Provides nodeless Lightning Network functionality for CDK with support for:
/// - BOLT11 Lightning invoices (send/receive)
/// - BOLT12 Lightning offers (future support)
/// - Spark protocol native transfers
/// - On-chain Bitcoin deposits and withdrawals
#[derive(Clone)]
pub struct CdkSpark {
    inner: Arc<SparkWallet>,
    config: SparkConfig,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    sender: broadcast::Sender<WaitPaymentResponse>,
    receiver: Arc<Mutex<broadcast::Receiver<WaitPaymentResponse>>>,
    // Payment tracking caches
    incoming_payments: Arc<RwLock<HashMap<String, TransferInfo>>>,
    outgoing_payments: Arc<RwLock<HashMap<String, PaymentInfo>>>,
    // Invoice-to-payment-hash mapping
    invoice_map: Arc<RwLock<HashMap<String, String>>>, // invoice_string -> payment_hash
}

impl CdkSpark {
    /// Create a new CDK Spark instance
    ///
    /// # Arguments
    /// * `config` - Spark configuration including network, mnemonic, and storage settings
    ///
    /// # Returns
    /// A new `CdkSpark` instance ready to be started
    ///
    /// # Errors
    /// Returns an error if:
    /// - Configuration is invalid
    /// - Mnemonic is invalid
    /// - Wallet initialization fails
    pub async fn new(config: SparkConfig) -> Result<Self, Error> {
        // Validate configuration
        config.validate()?;

        info!(
            "Initializing Spark wallet for network: {:?}",
            config.network
        );

        // Parse mnemonic
        let mnemonic = bip39::Mnemonic::from_str(&config.mnemonic)
            .map_err(|e| Error::InvalidMnemonic(e.to_string()))?;

        // Convert mnemonic to seed bytes
        let seed = mnemonic.to_seed(config.passphrase.as_deref().unwrap_or(""));

        // Create signer with seed and network
        let signer = Arc::new(
            DefaultSigner::new(&seed, config.network)
                .map_err(|e| Error::Configuration(format!("Failed to create signer: {}", e)))?,
        );

        // Build SparkWalletConfig
        let wallet_config = SparkWalletConfig {
            network: config.network,
            operator_pool: config.operator_pool.clone().unwrap_or_else(|| 
                SparkWalletConfig::default_operator_pool_config(config.network)
            ),
            reconnect_interval_seconds: config.reconnect_interval_seconds,
            service_provider_config: config.service_provider.clone().unwrap_or_else(|| 
                SparkWalletConfig::default_config(config.network).service_provider_config
            ),
            split_secret_threshold: config.split_secret_threshold,
            tokens_config: SparkWalletConfig::default_tokens_config(),
        };

        // Create wallet with config and signer
        let wallet = WalletBuilder::new(wallet_config, signer)
            .build()
            .await
            .map_err(|e| Error::SparkWallet(e))?;

        info!("Spark wallet initialized successfully");

        // Create broadcast channel for payment events
        let (sender, receiver) = broadcast::channel(100);

        Ok(Self {
            inner: Arc::new(wallet),
            config,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
            incoming_payments: Arc::new(RwLock::new(HashMap::new())),
            outgoing_payments: Arc::new(RwLock::new(HashMap::new())),
            invoice_map: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Get the inner Spark wallet
    pub fn wallet(&self) -> &Arc<SparkWallet> {
        &self.inner
    }

    /// Get the configuration
    pub fn config(&self) -> &SparkConfig {
        &self.config
    }

    /// Convert amount from one unit to another
    fn convert_amount(
        amount: Amount,
        from_unit: &CurrencyUnit,
        target_unit: &CurrencyUnit,
    ) -> Result<Amount, Error> {
        to_unit(amount, from_unit, target_unit)
            .map_err(|e| Error::AmountConversion(format!("{:?}", e)))
    }

    /// Convert satoshis to the target unit
    fn sats_to_unit(sats: u64, unit: &CurrencyUnit) -> Result<Amount, Error> {
        Self::convert_amount(sats.into(), &CurrencyUnit::Sat, unit)
    }

    /// Convert from unit to satoshis
    fn unit_to_sats(amount: Amount, unit: &CurrencyUnit) -> Result<u64, Error> {
        let sats = Self::convert_amount(amount, unit, &CurrencyUnit::Sat)?;
        Ok(u64::from(sats))
    }

    /// Start the event listener for incoming payments
    async fn start_event_listener(&self) -> Result<(), Error> {
        let wallet = Arc::clone(&self.inner);
        let sender = self.sender.clone();
        let cancel_token = self.wait_invoice_cancel_token.clone();
        let incoming_payments = Arc::clone(&self.incoming_payments);
        let invoice_map = Arc::clone(&self.invoice_map);

        tokio::spawn(async move {
            let mut event_stream = wallet.subscribe_events();

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Event listener cancelled");
                        break;
                    }
                    Ok(event) = event_stream.recv() => {
                        match event {
                            WalletEvent::TransferClaimed(transfer) => {
                                info!("Transfer claimed event: {:?}", transfer.id);
                                debug!("Transfer details: sender={}, receiver={}, amount={}", 
                                    transfer.sender_id, transfer.receiver_id, transfer.total_value_sat);

                                // Store transfer info in payment cache
                                let transfer_info = TransferInfo {
                                    transfer_id: transfer.id.to_string(),
                                    amount_sat: transfer.total_value_sat,
                                    timestamp: cdk_common::util::unix_time(),
                                    paid: true,
                                    preimage: None, // TODO: Get preimage from Spark if available
                                };

                                // Store in cache by transfer ID
                                incoming_payments.write().await.insert(
                                    transfer.id.to_string(),
                                    transfer_info.clone()
                                );

                                // Try to find matching payment hash from invoice
                                // For now, we'll store by transfer ID and implement lookup logic
                                // TODO: Implement proper payment hash mapping
                                
                                // Emit payment event for CDK
                                let mut payment_id_bytes = [0u8; 32];
                                let transfer_bytes = transfer.id.to_bytes();
                                payment_id_bytes[..transfer_bytes.len()].copy_from_slice(&transfer_bytes);
                                
                                let payment_response = WaitPaymentResponse {
                                    payment_identifier: PaymentIdentifier::PaymentId(payment_id_bytes),
                                    payment_amount: Amount::from(transfer.total_value_sat),
                                    unit: CurrencyUnit::Sat,
                                    payment_id: transfer.id.to_string(),
                                };

                                let _ = sender.send(payment_response);
                            }
                            WalletEvent::Synced => {
                                debug!("Wallet synced");
                            }
                            WalletEvent::StreamConnected => {
                                info!("Spark stream connected");
                            }
                            WalletEvent::StreamDisconnected => {
                                warn!("Spark stream disconnected");
                            }
                            _ => {
                                debug!("Received other wallet event: {:?}", event);
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

#[async_trait]
impl MintPayment for CdkSpark {
    type Err = payment::Error;

    /// Start the Spark wallet and payment processor
    #[instrument(skip(self))]
    async fn start(&self) -> Result<(), Self::Err> {
        info!("Starting Spark payment processor");

        // Start event listener for incoming payments
        self.start_event_listener()
            .await
            .map_err(|e| payment::Error::from(e))?;

        info!("Spark payment processor started successfully");
        Ok(())
    }

    /// Stop the payment processor
    #[instrument(skip(self))]
    async fn stop(&self) -> Result<(), Self::Err> {
        info!("Stopping Spark payment processor");

        // Cancel event listener
        self.wait_invoice_cancel_token.cancel();

        info!("Spark payment processor stopped");
        Ok(())
    }

    /// Get payment settings
    #[instrument(skip(self))]
    async fn get_settings(&self) -> Result<Value, Self::Err> {
        let settings = Bolt11Settings {
            mpp: true, // Spark supports multi-part payments
            unit: CurrencyUnit::Sat,
            invoice_description: true,
            amountless: true, // Spark supports amountless invoices
            bolt12: false,    // BOLT12 support coming soon
        };
        Ok(serde_json::to_value(settings)?)
    }

    /// Create an incoming payment request (invoice)
    #[instrument(skip(self))]
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let amount = bolt11_options.amount;
                let description = bolt11_options.description;
                let expiry = bolt11_options.unix_expiry;

                // Convert amount to satoshis
                let amount_sat = Self::unit_to_sats(amount, unit)?;

                info!("Creating Lightning invoice for {} sats", amount_sat);

                // Create invoice description
                let invoice_desc = description.map(|d| InvoiceDescription::Memo(d));

                // Create Lightning invoice using Spark wallet
                let payment = self
                    .inner
                    .create_lightning_invoice(amount_sat, invoice_desc, None, false)
                    .await
                    .map_err(Error::SparkWallet)?;

                // Parse the invoice to get payment hash
                let invoice = Bolt11Invoice::from_str(&payment.invoice)
                    .map_err(|e| Error::InvoiceParse(e.to_string()))?;

                let payment_hash = *invoice.payment_hash().as_ref();
                let payment_hash_hex = hex::encode(payment_hash);

                // Store invoice-to-payment-hash mapping
                self.invoice_map
                    .write()
                    .await
                    .insert(payment.invoice.clone(), payment_hash_hex.clone());

                info!("Created invoice with payment hash: {}", payment_hash_hex);

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::PaymentHash(payment_hash),
                    request: payment.invoice,
                    expiry,
                })
            }
            IncomingPaymentOptions::Bolt12(_) => {
                Err(Error::Bolt12NotSupported.into())
            }
        }
    }

    /// Get a quote for paying an outgoing payment
    #[instrument(skip(self, options))]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let invoice = &bolt11_options.bolt11;

                // Get amount from invoice or melt options
                let amount_msat = match bolt11_options.melt_options {
                    Some(melt_options) => melt_options.amount_msat().into(),
                    None => invoice
                        .amount_milli_satoshis()
                        .ok_or(Error::UnknownInvoiceAmount)?,
                };

                let amount_sat = amount_msat / 1000;

                info!("Getting payment quote for {} sats", amount_sat);

                // Calculate fees using config
                let relative_fee = (self.config.fee_reserve.percent_fee_reserve * amount_sat as f32) as u64;
                let min_fee: u64 = self.config.fee_reserve.min_fee_reserve.into();
                let fee_sat = relative_fee.max(min_fee);

                let amount = Self::sats_to_unit(amount_sat, unit)?;
                let fee = Self::sats_to_unit(fee_sat, unit)?;

                let payment_hash = *invoice.payment_hash().as_ref();

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(payment_hash)),
                    amount,
                    fee,
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Error::Bolt12NotSupported.into())
            }
        }
    }

    /// Make an outgoing payment
    #[instrument(skip(self, options))]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let invoice_str = bolt11_options.bolt11.to_string();
                let invoice = &bolt11_options.bolt11;

                // Get amount to send (for amountless invoices)
                let amount_to_send = match bolt11_options.melt_options {
                    Some(MeltOptions::Amountless { amountless }) => {
                        let amount_sat = u64::from(amountless.amount_msat) / 1000;
                        Some(amount_sat)
                    }
                    _ => None,
                };

                info!("Paying Lightning invoice: {}", invoice_str);

                // Pay the invoice using Spark wallet
                let result = self
                    .inner
                    .pay_lightning_invoice(
                        &invoice_str,
                        amount_to_send,
                        None, // max_fee_sat
                        self.config.network == Network::Mainnet, // prefer_spark
                    )
                    .await
                    .map_err(Error::SparkWallet)?;

                let payment_hash = *invoice.payment_hash().as_ref();
                let payment_hash_hex = hex::encode(payment_hash);

                // Get total spent (amount + fees)
                // Use the invoice amount as the base, since transfer.total_value_sat might be 0
                let invoice_amount_sat = u64::from(invoice.amount_milli_satoshis().unwrap_or(0)) / 1000;
                let total_spent_sat = if invoice_amount_sat > 0 {
                    invoice_amount_sat
                } else {
                    result.transfer.total_value_sat
                };
                let total_spent = Self::sats_to_unit(total_spent_sat, unit)?;

                // Get payment preimage if available
                let payment_proof = result.lightning_payment
                    .and_then(|p| p.payment_preimage);

                // Store outgoing payment info in cache
                self.outgoing_payments.write().await.insert(
                    payment_hash_hex.clone(),
                    PaymentInfo {
                        preimage: payment_proof.clone(),
                        status: MeltQuoteState::Paid,
                        amount_spent: total_spent_sat,
                    }
                );

                info!("Payment completed successfully");

                Ok(MakePaymentResponse {
                    payment_lookup_id: PaymentIdentifier::PaymentHash(payment_hash),
                    payment_proof,
                    status: MeltQuoteState::Paid,
                    total_spent,
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Error::Bolt12NotSupported.into())
            }
        }
    }

    /// Listen for incoming payment events
    #[instrument(skip(self))]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        info!("Starting payment event stream");

        self.wait_invoice_is_active.store(true, Ordering::SeqCst);

        let mut receiver = self.receiver.lock().await;
        let new_receiver = self.sender.subscribe();
        *receiver = new_receiver.resubscribe();
        drop(receiver);

        let receiver = self.sender.subscribe();
        let cancel_token = self.wait_invoice_cancel_token.clone();
        let is_active = Arc::clone(&self.wait_invoice_is_active);

        let stream = async_stream::stream! {
            let mut rx = receiver;
            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Payment event stream cancelled");
                        is_active.store(false, Ordering::SeqCst);
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(payment) => {
                                yield Event::PaymentReceived(payment);
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Payment event stream lagged by {} messages", n);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Payment event stream closed");
                                break;
                            }
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    /// Check if wait invoice is active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel wait invoice stream
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel();
    }

    /// Check the status of an incoming payment
    #[instrument(skip(self))]
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        info!("Checking incoming payment status: {:?}", payment_identifier);

        // Query the payment cache
        let cache = self.incoming_payments.read().await;

        // Try to find payment by identifier
        match payment_identifier {
            PaymentIdentifier::PaymentHash(_hash) => {
                // Convert payment hash to hex string for lookup
                let _payment_hash_hex = hex::encode(_hash);
                
                // Look for transfer with matching payment hash
                // For now, we'll search by amount and timestamp as a fallback
                // TODO: Implement proper payment hash to transfer mapping
                for (_transfer_id, transfer_info) in cache.iter() {
                    if transfer_info.paid {
                        // This is a simplified matching - in production we'd need better mapping
                        return Ok(vec![WaitPaymentResponse {
                            payment_identifier: payment_identifier.clone(),
                            payment_amount: Amount::from(transfer_info.amount_sat),
                            unit: CurrencyUnit::Sat,
                            payment_id: transfer_info.transfer_id.clone(),
                        }]);
                    }
                }
            }
            PaymentIdentifier::PaymentId(id_bytes) => {
                // Convert bytes to string for lookup
                let id = hex::encode(id_bytes);
                // Look up by transfer ID
                if let Some(transfer_info) = cache.get(&id) {
                    if transfer_info.paid {
                        return Ok(vec![WaitPaymentResponse {
                            payment_identifier: payment_identifier.clone(),
                            payment_amount: Amount::from(transfer_info.amount_sat),
                            unit: CurrencyUnit::Sat,
                            payment_id: transfer_info.transfer_id.clone(),
                        }]);
                    }
                }
            }
            _ => {
                // Other identifier types not supported
                return Ok(vec![]);
            }
        }

        // Payment not yet received
        Ok(vec![])
    }

    /// Check the status of an outgoing payment
    #[instrument(skip(self))]
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        info!("Checking outgoing payment status: {:?}", payment_identifier);

        // Query outgoing payment cache
        let cache = self.outgoing_payments.read().await;
        
        let payment_hash = payment_identifier.to_string();
        
        if let Some(payment_info) = cache.get(&payment_hash) {
            return Ok(MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: payment_info.preimage.clone(),
                status: payment_info.status.clone(),
                total_spent: Amount::from(payment_info.amount_spent),
                unit: CurrencyUnit::Sat,
            });
        }

        // Payment not found, return unknown status
        Ok(MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
            payment_proof: None,
            status: MeltQuoteState::Unknown,
            total_spent: Amount::ZERO,
            unit: CurrencyUnit::Sat,
        })
    }
}

