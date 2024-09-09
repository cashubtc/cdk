//! CDK Fake LN Backend
//!
//! Used for testing where quotes are auto filled

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bitcoin::hashes::{sha256, Hash};
use bitcoin::secp256k1::{Secp256k1, SecretKey};
use cdk::amount::Amount;
use cdk::cdk_lightning::{
    self, to_unit, CreateInvoiceResponse, MintLightning, MintMeltSettings, PayInvoiceResponse,
    PaymentQuoteResponse, Settings,
};
use cdk::mint;
use cdk::mint::FeeReserve;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use cdk::util::unix_time;
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
use tokio::sync::Mutex;
use tokio::time;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

pub mod error;

/// Fake Wallet
#[derive(Clone)]
pub struct FakeWallet {
    fee_reserve: FeeReserve,
    sender: tokio::sync::mpsc::Sender<String>,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
    mint_settings: MintMeltSettings,
    melt_settings: MintMeltSettings,
}

impl FakeWallet {
    /// Creat new [`FakeWallet`]
    pub fn new(
        fee_reserve: FeeReserve,
        mint_settings: MintMeltSettings,
        melt_settings: MintMeltSettings,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);

        Self {
            fee_reserve,
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
            mint_settings,
            melt_settings,
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
            melt_settings: self.melt_settings,
            mint_settings: self.mint_settings,
            invoice_description: true,
        }
    }

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let receiver = self.receiver.lock().await.take().ok_or(Error::NoReceiver)?;
        let receiver_stream = ReceiverStream::new(receiver);
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
        Ok(PayInvoiceResponse {
            payment_preimage: Some("".to_string()),
            payment_hash: "".to_string(),
            status: MeltQuoteState::Paid,
            total_spent: melt_quote.amount,
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

        let label = Uuid::new_v4().to_string();

        let private_key = SecretKey::from_slice(
            &[
                0xe1, 0x26, 0xf6, 0x8f, 0x7e, 0xaf, 0xcc, 0x8b, 0x74, 0xf5, 0x4d, 0x26, 0x9f, 0xe2,
                0x06, 0xbe, 0x71, 0x50, 0x00, 0xf9, 0x4d, 0xac, 0x06, 0x7d, 0x1c, 0x04, 0xa8, 0xca,
                0x3b, 0x2d, 0xb7, 0x34,
            ][..],
        )
        .unwrap();

        let payment_hash = sha256::Hash::from_slice(&[0; 32][..]).unwrap();
        let payment_secret = PaymentSecret([42u8; 32]);

        let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;

        let invoice = InvoiceBuilder::new(Currency::Bitcoin)
            .description(description)
            .payment_hash(payment_hash)
            .payment_secret(payment_secret)
            .amount_milli_satoshis(amount.into())
            .current_timestamp()
            .min_final_cltv_expiry_delta(144)
            .build_signed(|hash| Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key))
            .unwrap();

        // Create a random delay between 3 and 6 seconds
        let duration = time::Duration::from_secs(3)
            + time::Duration::from_millis(rand::random::<u64>() % 3001);

        let sender = self.sender.clone();
        let label_clone = label.clone();

        tokio::spawn(async move {
            // Wait for the random delay to elapse
            time::sleep(duration).await;

            // Send the message after waiting for the specified duration
            if sender.send(label_clone.clone()).await.is_err() {
                tracing::error!("Failed to send label: {}", label_clone);
            }
        });

        let expiry = invoice.expires_at().map(|t| t.as_secs());

        Ok(CreateInvoiceResponse {
            request_lookup_id: label,
            request: invoice,
            expiry,
        })
    }

    async fn check_invoice_status(
        &self,
        _request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        Ok(MintQuoteState::Paid)
    }
}
