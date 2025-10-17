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
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::hashes::Hash;
use cdk_common::amount::{to_unit, Amount};
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk_common::util::hex;
use futures::stream::StreamExt;
use futures::Stream;
use lightning_invoice::Bolt11Invoice;
use serde_json::Value;
use spark_wallet::{
    DefaultSigner, InvoiceDescription, KeySet, KeySetType, Network, SparkWallet, WalletBuilder,
    WalletEvent,
};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

pub mod config;
pub mod error;

#[cfg(test)]
mod tests;

pub use config::SparkConfig;
pub use error::Error;

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

        // Create signer from mnemonic
        let keyset = KeySet::from_mnemonic(
            KeySetType::Mainnet, // Will be overridden by network in wallet config
            mnemonic.to_string(),
            config.passphrase.clone(),
        )
        .map_err(|e| Error::Configuration(format!("Failed to create keyset: {}", e)))?;

        let signer = Arc::new(
            DefaultSigner::new(keyset)
                .map_err(|e| Error::Configuration(format!("Failed to create signer: {}", e)))?,
        );

        // Build wallet configuration
        let mut wallet_builder = WalletBuilder::new(config.network);

        // Set storage directory
        wallet_builder = wallet_builder.storage_dir(config.storage_dir.clone());

        // Set operator pool if provided
        if let Some(operator_config) = &config.operator_pool {
            wallet_builder = wallet_builder.operator_pool(operator_config.clone());
        }

        // Set service provider if provided
        if let Some(sp_config) = &config.service_provider {
            wallet_builder = wallet_builder.service_provider(sp_config.clone());
        }

        // Set reconnect interval
        wallet_builder = wallet_builder.reconnect_interval_seconds(config.reconnect_interval_seconds);

        // Set split secret threshold
        wallet_builder = wallet_builder.split_secret_threshold(config.split_secret_threshold);

        // Build the wallet
        let wallet = wallet_builder
            .build(signer)
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
        to_unit: &CurrencyUnit,
    ) -> Result<Amount, Error> {
        to_unit(amount, from_unit, to_unit)
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

        tokio::spawn(async move {
            let mut event_stream = wallet.subscribe_events().await;

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!("Event listener cancelled");
                        break;
                    }
                    Some(event) = event_stream.recv() => {
                        match event {
                            WalletEvent::IncomingPayment { payment } => {
                                info!("Received incoming payment event: {:?}", payment);

                                // Convert payment to WaitPaymentResponse
                                let payment_hash = match lightning_invoice::Bolt11Invoice::from_str(&payment.invoice) {
                                    Ok(invoice) => *invoice.payment_hash().as_ref(),
                                    Err(e) => {
                                        error!("Failed to parse invoice for payment hash: {}", e);
                                        continue;
                                    }
                                };

                                let amount_sat = payment.transfer_amount_sat.unwrap_or(0);

                                let response = WaitPaymentResponse {
                                    payment_identifier: PaymentIdentifier::PaymentHash(payment_hash),
                                    payment_amount: amount_sat.into(),
                                    unit: CurrencyUnit::Sat,
                                    payment_id: payment.id,
                                };

                                if let Err(e) = sender.send(response) {
                                    warn!("Failed to send payment notification: {}", e);
                                }
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

                info!("Created invoice with payment hash: {}", hex::encode(payment_hash));

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
                        let amount_sat = amountless.amount_msat / 1000;
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

                // Get total spent (amount + fees)
                let total_spent_sat = result.transfer.amount_sat;
                let total_spent = Self::sats_to_unit(total_spent_sat, unit)?;

                // Get payment preimage if available
                let payment_proof = result.lightning_payment
                    .and_then(|p| p.payment_preimage);

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

        // For now, return empty vec as Spark handles this through events
        // TODO: Query Spark for historical payment status
        Ok(vec![])
    }

    /// Check the status of an outgoing payment
    #[instrument(skip(self))]
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        info!("Checking outgoing payment status: {:?}", payment_identifier);

        // For now, return unknown status
        // TODO: Query Spark for payment status
        Ok(MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
            payment_proof: None,
            status: MeltQuoteState::Unknown,
            total_spent: Amount::ZERO,
            unit: CurrencyUnit::Sat,
        })
    }
}

