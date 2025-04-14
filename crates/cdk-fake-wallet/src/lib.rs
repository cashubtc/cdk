//! CDK Fake LN Backend
//!
//! Used for testing where quotes are auto filled

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::rand::{thread_rng, Rng};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use cdk::amount::{to_unit, Amount};
use cdk::cdk_payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, PaymentIdentifier, PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState, MintQuoteState, PaymentMethod};
use cdk::types::FeeReserve;
use cdk::{ensure_cdk, mint, MeltPaymentRequest};
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use lightning::offers::offer::OfferBuilder;
use lightning_invoice::{Bolt11Invoice, Currency, InvoiceBuilder, PaymentSecret};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::time;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::instrument;

pub mod error;

/// Fake Wallet
#[derive(Clone)]
pub struct FakeWallet {
    fee_reserve: FeeReserve,
    #[allow(clippy::type_complexity)]
    sender: tokio::sync::mpsc::Sender<(PaymentIdentifier, Amount)>,
    #[allow(clippy::type_complexity)]
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<(PaymentIdentifier, Amount)>>>>,
    payment_states: Arc<Mutex<HashMap<String, MeltQuoteState>>>,
    failed_payment_check: Arc<Mutex<HashSet<String>>>,
    payment_delay: u64,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl FakeWallet {
    /// Create new [`FakeWallet`]
    pub fn new(
        fee_reserve: FeeReserve,
        payment_states: HashMap<String, MeltQuoteState>,
        fail_payment_check: HashSet<String>,
        payment_delay: u64,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);

        Self {
            fee_reserve,
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
            payment_states: Arc::new(Mutex::new(payment_states)),
            failed_payment_check: Arc::new(Mutex::new(fail_payment_check)),
            payment_delay,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
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
    type Err = cdk_payment::Error;

    #[instrument(skip_all)]
    async fn get_settings(&self) -> Result<Value, Self::Err> {
        Ok(serde_json::to_value(Bolt11Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
            invoice_description: true,
            amountless: false,
        })?)
    }

    #[instrument(skip_all)]
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    #[instrument(skip_all)]
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_is_active.store(false, Ordering::SeqCst);
        self.wait_invoice_cancel_token.cancel()
    }

    #[instrument(skip_all)]
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitPaymentResponse> + Send>>, Self::Err> {
        tracing::info!("Starting stream for fake invoices");
        self.wait_invoice_is_active.store(true, Ordering::SeqCst);

        let receiver = self.receiver.lock().await.take().ok_or(Error::NoReceiver)?;
        let receiver_stream = ReceiverStream::new(receiver);

        Ok(Box::pin(receiver_stream.map(
            |(request_lookup_id, payment_amount)| WaitPaymentResponse {
                payment_identifier: request_lookup_id.clone(),
                payment_amount,
                unit: CurrencyUnit::Sat,
                payment_id: request_lookup_id.to_string(),
            },
        )))
    }

    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        request: &str,
        unit: &CurrencyUnit,
        options: Option<MeltOptions>,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let bolt11 = Bolt11Invoice::from_str(request)?;

        let amount_msat = match options {
            Some(amount) => amount.amount_msat(),
            None => bolt11
                .amount_milli_satoshis()
                .ok_or(Error::UnknownInvoiceAmount)?
                .into(),
        };

        let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let fee = max(relative_fee_reserve, absolute_fee_reserve);

        Ok(PaymentQuoteResponse {
            request_lookup_id: bolt11.payment_hash().to_string(),
            amount,
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
            options: None,
        })
    }

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        melt_quote: mint::MeltQuote,
        _partial_msats: Option<Amount>,
        _max_fee_msats: Option<Amount>,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match melt_quote.payment_method {
            PaymentMethod::Bolt11 => {
                let payment_request = melt_quote.request;

                let bolt11 = match payment_request {
                    MeltPaymentRequest::Bolt11 { bolt11 } => bolt11,
                    _ => {
                        return Err(cdk_payment::Error::UnsupportedUnit);
                    }
                };

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

                Ok(MakePaymentResponse {
                    payment_proof: Some("".to_string()),
                    payment_lookup_id: payment_hash,
                    status: payment_status,
                    total_spent: melt_quote.amount + 1.into(),
                    unit: melt_quote.unit,
                })
            }
            PaymentMethod::Bolt12 => {
                let bolt12 = &match melt_quote.request {
                    MeltPaymentRequest::Bolt11 { .. } => return Err(Error::WrongRequestType.into()),
                    MeltPaymentRequest::Bolt12 { offer, invoice: _ } => offer,
                };
                Ok(MakePaymentResponse {
                    payment_proof: Some("".to_string()),
                    payment_lookup_id: bolt12.to_string(),
                    status: MeltQuoteState::Paid,
                    total_spent: melt_quote.amount,
                    unit: melt_quote.unit,
                })
            }
            PaymentMethod::Custom(_) => {
                panic!("Unssuported method");
            }
        }
    }

    #[instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let (payment_hash, request) = if payment_method == &PaymentMethod::Bolt12 {
            let secret_key = bitcoin::secp256k1::SecretKey::new(&mut rand::thread_rng());

            let secp_ctx = Secp256k1::new();

            let offer_builder =
                OfferBuilder::new(secret_key.public_key(&secp_ctx)).description(description);

            let amount = Some(amount);

            let offer_builder = match amount {
                Some(amount) => {
                    let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;
                    offer_builder.amount_msats(amount.into())
                }
                None => offer_builder,
            };

            let offer = offer_builder.build().unwrap();

            (
                PaymentIdentifier::OfferId(offer.id().to_string()),
                offer.to_string(),
            )
        } else {
            // Since this is fake we just use the amount no matter the unit to create an invoice
            let amount_msat = amount;

            let invoice = create_fake_invoice(amount_msat.into(), description);

            let payment_hash = invoice.payment_hash();

            (
                PaymentIdentifier::PaymentHash(*payment_hash),
                invoice.to_string(),
            )
        };

        let sender = self.sender.clone();
        let duration = time::Duration::from_secs(self.payment_delay);

        let amount = if amount == Amount::ZERO {
            let mut rng = rand::thread_rng();

            // Generate a random number between 1 and 1000 (inclusive)
            let random_number: u64 = rng.gen_range(1..=1000);
            random_number.into()
        } else {
            amount
        };

        let payment_hash_clone = payment_hash.clone();

        tokio::spawn(async move {
            // Wait for the random delay to elapse
            time::sleep(duration).await;

            // Send the message after waiting for the specified duration
            if sender
                .send((payment_hash_clone.clone(), amount))
                .await
                .is_err()
            {
                tracing::error!("Failed to send label: {:?}", payment_hash_clone);
            }
        });

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: payment_hash,
            request,
            expiry: None,
        })
    }

    #[instrument(skip_all)]
    async fn check_incoming_payment_status(
        &self,
        _request_lookup_id: &PaymentIdentifier,
    ) -> Result<MintQuoteState, Self::Err> {
        Ok(MintQuoteState::Paid)
    }

    #[instrument(skip_all)]
    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &str,
    ) -> Result<MakePaymentResponse, Self::Err> {
        // For fake wallet if the state is not explicitly set default to paid
        let states = self.payment_states.lock().await;
        let status = states.get(request_lookup_id).cloned();

        let status = status.unwrap_or(MeltQuoteState::Paid);

        let fail_payments = self.failed_payment_check.lock().await;

        if fail_payments.contains(request_lookup_id) {
            return Err(cdk_payment::Error::InvoicePaymentPending);
        }

        Ok(MakePaymentResponse {
            payment_proof: Some("".to_string()),
            payment_lookup_id: request_lookup_id.to_string(),
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

    let mut rng = thread_rng();
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
