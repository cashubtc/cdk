//! CDK Fake LN Backend
//!
//! Used for testing where quotes are auto filled

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use cdk::amount::{to_unit, Amount};
use cdk::cdk_lightning::{
    self, Bolt12PaymentQuoteResponse, CreateInvoiceResponse, CreateOfferResponse, MintLightning,
    PayInvoiceResponse, PaymentQuoteResponse, Settings,
};
use cdk::mint;
use cdk::mint::types::PaymentRequest;
use cdk::mint::FeeReserve;
use cdk::nuts::{
    CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteBolt12Request, MeltQuoteState, MintQuoteState,
};
use cdk::util::unix_time;
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use lightning_invoice::{Bolt11Invoice, Currency, InvoiceBuilder, PaymentSecret};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

pub mod error;

/// Fake Wallet
#[derive(Clone)]
pub struct FakeWallet {
    fee_reserve: FeeReserve,
    sender: tokio::sync::mpsc::Sender<String>,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
    payment_states: Arc<Mutex<HashMap<String, MeltQuoteState>>>,
    failed_payment_check: Arc<Mutex<HashSet<String>>>,
    payment_delay: u64,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl FakeWallet {
    /// Creat new [`FakeWallet`]
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
impl MintLightning for FakeWallet {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
            bolt12_mint: false,
            bolt12_melt: false,
            invoice_description: true,
        }
    }

    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let receiver = self.receiver.lock().await.take().ok_or(Error::NoReceiver)?;
        let receiver_stream = ReceiverStream::new(receiver);
        self.wait_invoice_is_active.store(true, Ordering::SeqCst);
        Ok(Box::pin(receiver_stream.map(|label| label)))
    }

    async fn get_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt11Request,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let invoice_amount_msat = melt_quote_request
            .request
            .amount_milli_satoshis()
            .ok_or(Error::UnknownInvoiceAmount)?;

        let amount = to_unit(
            invoice_amount_msat,
            &CurrencyUnit::Msat,
            &melt_quote_request.unit,
        )?;

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let fee = match relative_fee_reserve > absolute_fee_reserve {
            true => relative_fee_reserve,
            false => absolute_fee_reserve,
        };

        Ok(PaymentQuoteResponse {
            request_lookup_id: melt_quote_request.request.payment_hash().to_string(),
            amount,
            fee: fee.into(),
            state: MeltQuoteState::Unpaid,
        })
    }

    async fn pay_invoice(
        &self,
        melt_quote: mint::MeltQuote,
        _partial_msats: Option<Amount>,
        _max_fee_msats: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let bolt11 = &match melt_quote.request {
            PaymentRequest::Bolt11 { bolt11 } => bolt11,
            PaymentRequest::Bolt12 { .. } => return Err(Error::WrongRequestType.into()),
        };

        let payment_hash = bolt11.payment_hash().to_string();

        let description = bolt11.description().to_string();

        let status: Option<FakeInvoiceDescription> = serde_json::from_str(&description).ok();

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

            if description.pay_err {
                return Err(Error::UnknownInvoice.into());
            }
        }

        Ok(PayInvoiceResponse {
            payment_preimage: Some("".to_string()),
            payment_lookup_id: payment_hash,
            status: payment_status,
            total_spent: melt_quote.amount,
            unit: melt_quote.unit,
        })
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let amount_msat = to_unit(amount, unit, &CurrencyUnit::Msat)?;

        let invoice = create_fake_invoice(amount_msat.into(), description);

        let sender = self.sender.clone();

        let payment_hash = invoice.payment_hash();

        let payment_hash_clone = payment_hash.to_string();

        let duration = time::Duration::from_secs(self.payment_delay);

        tokio::spawn(async move {
            // Wait for the random delay to elapse
            time::sleep(duration).await;

            // Send the message after waiting for the specified duration
            if sender.send(payment_hash_clone.clone()).await.is_err() {
                tracing::error!("Failed to send label: {}", payment_hash_clone);
            }
        });

        let expiry = invoice.expires_at().map(|t| t.as_secs());

        Ok(CreateInvoiceResponse {
            request_lookup_id: payment_hash.to_string(),
            request: invoice,
            expiry,
        })
    }

    async fn check_incoming_invoice_status(
        &self,
        _request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        Ok(MintQuoteState::Paid)
    }

    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &str,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        // For fake wallet if the state is not explicitly set default to paid
        let states = self.payment_states.lock().await;
        let status = states.get(request_lookup_id).cloned();

        let status = status.unwrap_or(MeltQuoteState::Paid);

        let fail_payments = self.failed_payment_check.lock().await;

        if fail_payments.contains(request_lookup_id) {
            return Err(cdk_lightning::Error::InvoicePaymentPending);
        }

        Ok(PayInvoiceResponse {
            payment_preimage: Some("".to_string()),
            payment_lookup_id: request_lookup_id.to_string(),
            status,
            total_spent: Amount::ZERO,
            unit: self.get_settings().unit,
        })
    }

    async fn get_bolt12_payment_quote(
        &self,
        _melt_quote_request: &MeltQuoteBolt12Request,
    ) -> Result<Bolt12PaymentQuoteResponse, Self::Err> {
        todo!()
    }

    /// Pay a bolt12 offer
    async fn pay_bolt12_offer(
        &self,
        _melt_quote: mint::MeltQuote,
        _amount: Option<Amount>,
        _max_fee_amount: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        todo!()
    }

    /// Create bolt12 offer
    async fn create_bolt12_offer(
        &self,
        _amount: Amount,
        _unit: &CurrencyUnit,
        _description: String,
        _unix_expiry: u64,
    ) -> Result<CreateOfferResponse, Self::Err> {
        todo!()
    }
}

/// Create fake invoice
pub fn create_fake_invoice(amount_msat: u64, description: String) -> Bolt11Invoice {
    let private_key = SecretKey::from_slice(
        &[
            0xe1, 0x26, 0xf6, 0x8f, 0x7e, 0xaf, 0xcc, 0x8b, 0x74, 0xf5, 0x4d, 0x26, 0x9f, 0xe2,
            0x06, 0xbe, 0x71, 0x50, 0x00, 0xf9, 0x4d, 0xac, 0x06, 0x7d, 0x1c, 0x04, 0xa8, 0xca,
            0x3b, 0x2d, 0xb7, 0x34,
        ][..],
    )
    .unwrap();

    let mut rng = rand::thread_rng();
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
