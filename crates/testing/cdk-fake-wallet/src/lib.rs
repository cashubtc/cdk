//! CDK Fake LN Backend
//!
//! Used for testing where quotes are auto filled.
//!
//! The fake wallet now includes a secondary repayment system that continuously repays any-amount
//! invoices (amount = 0) at random intervals between 30 seconds and 3 minutes to simulate
//! real-world behavior where invoices might get multiple payments. Payments continue to be
//! processed until they are evicted from the queue when the queue reaches its maximum size
//! (default 100 items). This is in addition to the original immediate payment processing
//! which is maintained for all invoice types.

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use cdk_common::amount::{to_unit, Amount};
use cdk_common::common::FeeReserve;
use cdk_common::ensure_cdk;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, WaitPaymentResponse,
};
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use lightning::offers::offer::OfferBuilder;
use lightning_invoice::{Bolt11Invoice, Currency, InvoiceBuilder, PaymentSecret};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio::time;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uuid::Uuid;

pub mod error;

/// Default maximum size for the secondary repayment queue
const DEFAULT_REPAY_QUEUE_MAX_SIZE: usize = 100;

/// Secondary repayment queue manager for any-amount invoices
#[derive(Debug, Clone)]
struct SecondaryRepaymentQueue {
    queue: Arc<Mutex<VecDeque<PaymentIdentifier>>>,
    max_size: usize,
    sender: tokio::sync::mpsc::Sender<(PaymentIdentifier, Amount, String)>,
}

impl SecondaryRepaymentQueue {
    fn new(
        max_size: usize,
        sender: tokio::sync::mpsc::Sender<(PaymentIdentifier, Amount, String)>,
    ) -> Self {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let repayment_queue = Self {
            queue: queue.clone(),
            max_size,
            sender,
        };

        // Start the background secondary repayment processor
        repayment_queue.start_secondary_repayment_processor();

        repayment_queue
    }

    /// Add a payment to the secondary repayment queue
    async fn enqueue_for_repayment(&self, payment: PaymentIdentifier) {
        let mut queue = self.queue.lock().await;

        // If queue is at max capacity, remove the oldest item
        if queue.len() >= self.max_size {
            if let Some(dropped) = queue.pop_front() {
                tracing::debug!(
                    "Secondary repayment queue at capacity, dropping oldest payment: {:?}",
                    dropped
                );
            }
        }

        queue.push_back(payment);
        tracing::debug!(
            "Added payment to secondary repayment queue, current size: {}",
            queue.len()
        );
    }

    /// Start the background task that randomly processes secondary repayments from the queue
    fn start_secondary_repayment_processor(&self) {
        let queue = self.queue.clone();
        let sender = self.sender.clone();

        tokio::spawn(async move {
            use bitcoin::secp256k1::rand::rngs::OsRng;
            use bitcoin::secp256k1::rand::Rng;
            let mut rng = OsRng;

            loop {
                // Wait for a random interval between 30 seconds and 3 minutes (180 seconds)
                let delay_secs = rng.gen_range(1..=3);
                time::sleep(time::Duration::from_secs(delay_secs)).await;

                // Try to process a random payment from the queue without removing it
                let payment_to_process = {
                    let q = queue.lock().await;
                    if q.is_empty() {
                        None
                    } else {
                        // Pick a random index from the queue but don't remove it
                        let index = rng.gen_range(0..q.len());
                        q.get(index).cloned()
                    }
                };

                if let Some(payment) = payment_to_process {
                    // Generate a random amount for this secondary payment (same range as initial payment: 1-1000)
                    let random_amount: u64 = rng.gen_range(1..=1000);
                    let secondary_amount = Amount::from(random_amount);

                    // Generate a unique payment identifier for this secondary payment
                    // We'll create a new payment hash by appending a timestamp and random bytes
                    use bitcoin::hashes::{sha256, Hash};
                    let mut random_bytes = [0u8; 16];
                    rng.fill(&mut random_bytes);
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos() as u64;

                    // Create a unique hash combining the original payment identifier, timestamp, and random bytes
                    let mut hasher_input = Vec::new();
                    hasher_input.extend_from_slice(payment.to_string().as_bytes());
                    hasher_input.extend_from_slice(&timestamp.to_le_bytes());
                    hasher_input.extend_from_slice(&random_bytes);

                    let unique_hash = sha256::Hash::hash(&hasher_input);
                    let unique_payment_id = PaymentIdentifier::PaymentHash(*unique_hash.as_ref());

                    tracing::info!(
                        "Processing secondary repayment: original={:?}, new_id={:?}, amount={}",
                        payment,
                        unique_payment_id,
                        secondary_amount
                    );

                    // Send the payment notification using the original payment identifier
                    // The mint will process this through the normal payment stream
                    if let Err(e) = sender
                        .send((
                            payment.clone(),
                            secondary_amount,
                            unique_payment_id.to_string(),
                        ))
                        .await
                    {
                        tracing::error!(
                            "Failed to send secondary repayment notification for {:?}: {}",
                            unique_payment_id,
                            e
                        );
                    }
                }
            }
        });
    }
}

/// Fake Wallet
#[derive(Clone)]
pub struct FakeWallet {
    fee_reserve: FeeReserve,
    #[allow(clippy::type_complexity)]
    sender: tokio::sync::mpsc::Sender<(PaymentIdentifier, Amount, String)>,
    #[allow(clippy::type_complexity)]
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(PaymentIdentifier, Amount, String)>>>>,
    payment_states: Arc<Mutex<HashMap<String, MeltQuoteState>>>,
    failed_payment_check: Arc<Mutex<HashSet<String>>>,
    payment_delay: u64,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    incoming_payments: Arc<RwLock<HashMap<PaymentIdentifier, Vec<WaitPaymentResponse>>>>,
    unit: CurrencyUnit,
    secondary_repayment_queue: SecondaryRepaymentQueue,
}

impl FakeWallet {
    /// Create new [`FakeWallet`]
    pub fn new(
        fee_reserve: FeeReserve,
        payment_states: HashMap<String, MeltQuoteState>,
        fail_payment_check: HashSet<String>,
        payment_delay: u64,
        unit: CurrencyUnit,
    ) -> Self {
        Self::new_with_repay_queue_size(
            fee_reserve,
            payment_states,
            fail_payment_check,
            payment_delay,
            unit,
            DEFAULT_REPAY_QUEUE_MAX_SIZE,
        )
    }

    /// Create new [`FakeWallet`] with custom secondary repayment queue size
    pub fn new_with_repay_queue_size(
        fee_reserve: FeeReserve,
        payment_states: HashMap<String, MeltQuoteState>,
        fail_payment_check: HashSet<String>,
        payment_delay: u64,
        unit: CurrencyUnit,
        repay_queue_max_size: usize,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let incoming_payments = Arc::new(RwLock::new(HashMap::new()));

        let secondary_repayment_queue =
            SecondaryRepaymentQueue::new(repay_queue_max_size, sender.clone());

        Self {
            fee_reserve,
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
            payment_states: Arc::new(Mutex::new(payment_states)),
            failed_payment_check: Arc::new(Mutex::new(fail_payment_check)),
            payment_delay,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            incoming_payments,
            unit,
            secondary_repayment_queue,
        }
    }
}

/// Struct for signaling what methods should respond via invoice description
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct FakeInvoiceDescription {
    /// State to be returned from pay invoice state
    pub pay_invoice_state: MeltQuoteState,
    /// State to be returned by check payment state
    pub check_payment_state: MeltQuoteState,
    /// Should pay invoice error
    pub pay_err: bool,
    /// Should check failure
    pub check_err: bool,
}

impl Default for FakeInvoiceDescription {
    fn default() -> Self {
        Self {
            pay_invoice_state: MeltQuoteState::Paid,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        }
    }
}

#[async_trait]
impl MintPayment for FakeWallet {
    type Err = payment::Error;

    #[instrument(skip_all)]
    async fn get_settings(&self) -> Result<Value, Self::Err> {
        Ok(serde_json::to_value(Bolt11Settings {
            mpp: true,
            unit: self.unit.clone(),
            invoice_description: true,
            amountless: false,
            bolt12: true,
        })?)
    }

    #[instrument(skip_all)]
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    #[instrument(skip_all)]
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    #[instrument(skip_all)]
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitPaymentResponse> + Send>>, Self::Err> {
        tracing::info!("Starting stream for fake invoices");
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or(Error::NoReceiver)
            .unwrap();
        let unit = self.unit.clone();
        let receiver_stream = ReceiverStream::new(receiver);
        Ok(Box::pin(receiver_stream.map(
            move |(request_lookup_id, payment_amount, payment_id)| WaitPaymentResponse {
                payment_identifier: request_lookup_id.clone(),
                payment_amount,
                unit: unit.clone(),
                payment_id,
            },
        )))
    }

    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let (amount_msat, request_lookup_id) = match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                // If we have specific amount options, use those
                let amount_msat: u64 = if let Some(melt_options) = bolt11_options.melt_options {
                    let msats = match melt_options {
                        MeltOptions::Amountless { amountless } => {
                            let amount_msat = amountless.amount_msat;

                            if let Some(invoice_amount) =
                                bolt11_options.bolt11.amount_milli_satoshis()
                            {
                                ensure_cdk!(
                                    invoice_amount == u64::from(amount_msat),
                                    Error::UnknownInvoiceAmount.into()
                                );
                            }
                            amount_msat
                        }
                        MeltOptions::Mpp { mpp } => mpp.amount,
                    };

                    u64::from(msats)
                } else {
                    // Fall back to invoice amount
                    bolt11_options
                        .bolt11
                        .amount_milli_satoshis()
                        .ok_or(Error::UnknownInvoiceAmount)?
                };
                let payment_id =
                    PaymentIdentifier::PaymentHash(*bolt11_options.bolt11.payment_hash().as_ref());
                (amount_msat, Some(payment_id))
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let offer = bolt12_options.offer;

                let amount_msat: u64 = if let Some(amount) = bolt12_options.melt_options {
                    amount.amount_msat().into()
                } else {
                    // Fall back to offer amount
                    let amount = offer.amount().ok_or(Error::UnknownInvoiceAmount)?;
                    match amount {
                        lightning::offers::offer::Amount::Bitcoin { amount_msats } => amount_msats,
                        _ => return Err(Error::UnknownInvoiceAmount.into()),
                    }
                };
                (amount_msat, None)
            }
        };

        let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let fee = max(relative_fee_reserve, absolute_fee_reserve);

        Ok(PaymentQuoteResponse {
            request_lookup_id,
            amount,
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
            unit: unit.clone(),
        })
    }

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let bolt11 = bolt11_options.bolt11;
                let payment_hash = bolt11.payment_hash().to_string();

                let description = bolt11.description().to_string();

                let status: Option<FakeInvoiceDescription> =
                    serde_json::from_str(&description).ok();

                let mut payment_states = self.payment_states.lock().await;
                let payment_status = status
                    .clone()
                    .map(|s| s.pay_invoice_state)
                    .unwrap_or(MeltQuoteState::Paid);

                let checkout_going_status = status
                    .clone()
                    .map(|s| s.check_payment_state)
                    .unwrap_or(MeltQuoteState::Paid);

                payment_states.insert(payment_hash.clone(), checkout_going_status);

                if let Some(description) = status {
                    if description.check_err {
                        let mut fail = self.failed_payment_check.lock().await;
                        fail.insert(payment_hash.clone());
                    }

                    ensure_cdk!(!description.pay_err, Error::UnknownInvoice.into());
                }

                let amount_msat: u64 = if let Some(melt_options) = bolt11_options.melt_options {
                    melt_options.amount_msat().into()
                } else {
                    // Fall back to invoice amount
                    bolt11
                        .amount_milli_satoshis()
                        .ok_or(Error::UnknownInvoiceAmount)?
                };

                let total_spent = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

                Ok(MakePaymentResponse {
                    payment_proof: Some("".to_string()),
                    payment_lookup_id: PaymentIdentifier::PaymentHash(
                        *bolt11.payment_hash().as_ref(),
                    ),
                    status: payment_status,
                    total_spent: total_spent + 1.into(),
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let bolt12 = bolt12_options.offer;
                let amount_msat: u64 = if let Some(amount) = bolt12_options.melt_options {
                    amount.amount_msat().into()
                } else {
                    // Fall back to offer amount
                    let amount = bolt12.amount().ok_or(Error::UnknownInvoiceAmount)?;
                    match amount {
                        lightning::offers::offer::Amount::Bitcoin { amount_msats } => amount_msats,
                        _ => return Err(Error::UnknownInvoiceAmount.into()),
                    }
                };

                let total_spent = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

                Ok(MakePaymentResponse {
                    payment_proof: Some("".to_string()),
                    payment_lookup_id: PaymentIdentifier::CustomId(Uuid::new_v4().to_string()),
                    status: MeltQuoteState::Paid,
                    total_spent: total_spent + 1.into(),
                    unit: unit.clone(),
                })
            }
        }
    }

    #[instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let (payment_hash, request, amount, expiry) = match options {
            IncomingPaymentOptions::Bolt12(bolt12_options) => {
                let description = bolt12_options.description.unwrap_or_default();
                let amount = bolt12_options.amount;
                let expiry = bolt12_options.unix_expiry;

                let secret_key = SecretKey::new(&mut bitcoin::secp256k1::rand::rngs::OsRng);
                let secp_ctx = Secp256k1::new();

                let offer_builder = OfferBuilder::new(secret_key.public_key(&secp_ctx))
                    .description(description.clone());

                let offer_builder = match amount {
                    Some(amount) => {
                        let amount_msat = to_unit(amount, unit, &CurrencyUnit::Msat)?;
                        offer_builder.amount_msats(amount_msat.into())
                    }
                    None => offer_builder,
                };

                let offer = offer_builder.build().unwrap();

                (
                    PaymentIdentifier::OfferId(offer.id().to_string()),
                    offer.to_string(),
                    amount.unwrap_or(Amount::ZERO),
                    expiry,
                )
            }
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let description = bolt11_options.description.unwrap_or_default();
                let amount = if unit == &CurrencyUnit::Sat {
                    to_unit(bolt11_options.amount, unit, &CurrencyUnit::Msat)
                        .unwrap_or(bolt11_options.amount * Amount::from(1000))
                } else {
                    bolt11_options.amount
                };
                let expiry = bolt11_options.unix_expiry;

                // Since this is fake we just use the amount no matter the unit to create an invoice
                let invoice = create_fake_invoice(amount.into(), description.clone());
                let payment_hash = invoice.payment_hash();

                (
                    PaymentIdentifier::PaymentHash(*payment_hash.as_ref()),
                    invoice.to_string(),
                    amount,
                    expiry,
                )
            }
        };

        // ALL invoices get immediate payment processing (original behavior)
        let sender = self.sender.clone();
        let duration = time::Duration::from_secs(self.payment_delay);
        let payment_hash_clone = payment_hash.clone();
        let incoming_payment = self.incoming_payments.clone();
        let unit_clone = self.unit.clone();

        let final_amount = if amount == Amount::ZERO {
            // For any-amount invoices, generate a random amount for the initial payment
            use bitcoin::secp256k1::rand::rngs::OsRng;
            use bitcoin::secp256k1::rand::Rng;
            let mut rng = OsRng;
            let random_amount: u64 = rng.gen_range(1..=1000);
            random_amount.into()
        } else {
            amount
        };

        // Schedule the immediate payment (original behavior maintained)
        tokio::spawn(async move {
            // Wait for the random delay to elapse
            time::sleep(duration).await;

            let response = WaitPaymentResponse {
                payment_identifier: payment_hash_clone.clone(),
                payment_amount: final_amount,
                unit: unit_clone,
                payment_id: payment_hash_clone.to_string(),
            };
            let mut incoming = incoming_payment.write().await;
            incoming
                .entry(payment_hash_clone.clone())
                .or_insert_with(Vec::new)
                .push(response.clone());

            // Send the message after waiting for the specified duration
            if sender
                .send((
                    payment_hash_clone.clone(),
                    final_amount,
                    payment_hash_clone.to_string(),
                ))
                .await
                .is_err()
            {
                tracing::error!("Failed to send label: {:?}", payment_hash_clone);
            }
        });

        // For any-amount invoices ONLY, also add to the secondary repayment queue
        if amount == Amount::ZERO {
            tracing::info!(
                "Adding any-amount invoice to secondary repayment queue: {:?}",
                payment_hash
            );

            self.secondary_repayment_queue
                .enqueue_for_repayment(payment_hash.clone())
                .await;
        }

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: payment_hash,
            request,
            expiry,
        })
    }

    #[instrument(skip_all)]
    async fn check_incoming_payment_status(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        Ok(self
            .incoming_payments
            .read()
            .await
            .get(request_lookup_id)
            .cloned()
            .unwrap_or(vec![]))
    }

    #[instrument(skip_all)]
    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        // For fake wallet if the state is not explicitly set default to paid
        let states = self.payment_states.lock().await;
        let status = states.get(&request_lookup_id.to_string()).cloned();

        let status = status.unwrap_or(MeltQuoteState::Paid);

        let fail_payments = self.failed_payment_check.lock().await;

        if fail_payments.contains(&request_lookup_id.to_string()) {
            return Err(payment::Error::InvoicePaymentPending);
        }

        Ok(MakePaymentResponse {
            payment_proof: Some("".to_string()),
            payment_lookup_id: request_lookup_id.clone(),
            status,
            total_spent: Amount::ZERO,
            unit: CurrencyUnit::Msat,
        })
    }
}

/// Create fake invoice
#[instrument]
pub fn create_fake_invoice(amount_msat: u64, description: String) -> Bolt11Invoice {
    let private_key = SecretKey::from_slice(
        &[
            0xe1, 0x26, 0xf6, 0x8f, 0x7e, 0xaf, 0xcc, 0x8b, 0x74, 0xf5, 0x4d, 0x26, 0x9f, 0xe2,
            0x06, 0xbe, 0x71, 0x50, 0x00, 0xf9, 0x4d, 0xac, 0x06, 0x7d, 0x1c, 0x04, 0xa8, 0xca,
            0x3b, 0x2d, 0xb7, 0x34,
        ][..],
    )
    .unwrap();

    use bitcoin::secp256k1::rand::rngs::OsRng;
    use bitcoin::secp256k1::rand::Rng;
    let mut rng = OsRng;
    let mut random_bytes = [0u8; 32];
    rng.fill(&mut random_bytes);

    let payment_hash = sha256::Hash::from_slice(&random_bytes).unwrap();
    let payment_secret = PaymentSecret([42u8; 32]);

    InvoiceBuilder::new(Currency::Bitcoin)
        .description(description)
        .payment_hash(payment_hash)
        .payment_secret(payment_secret)
        .amount_milli_satoshis(amount_msat)
        .current_timestamp()
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
        .unwrap()
}
