use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use bitcoin::key::Secp256k1;
use cdk::amount::{to_unit, Amount};
use cdk::cdk_lightning::bolt12::{Bolt12Settings, MintBolt12Lightning};
use cdk::cdk_lightning::{
    self, Bolt12PaymentQuoteResponse, CreateOfferResponse, PayInvoiceResponse, WaitInvoiceResponse,
};
use cdk::mint;
use cdk::mint::types::PaymentRequest;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt12Request};
use futures::stream::StreamExt;
use futures::Stream;
use lightning::offers::offer::{Amount as LDKAmount, Offer, OfferBuilder};
use tokio::time;
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::FakeWallet;

#[async_trait]
impl MintBolt12Lightning for FakeWallet {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Bolt12Settings {
        Bolt12Settings {
            mint: true,
            melt: true,
            unit: CurrencyUnit::Sat,
            offer_description: true,
        }
    }

    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    async fn wait_any_offer(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitInvoiceResponse> + Send>>, Self::Err> {
        let receiver = self
            .bolt12_receiver
            .lock()
            .await
            .take()
            .ok_or(super::Error::NoReceiver)?;
        let receiver_stream = ReceiverStream::new(receiver);
        self.wait_invoice_is_active.store(true, Ordering::SeqCst);

        Ok(Box::pin(receiver_stream.map(|label| WaitInvoiceResponse {
            request_lookup_id: label.clone(),
            payment_amount: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            payment_id: label,
        })))
    }

    async fn get_bolt12_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt12Request,
    ) -> Result<Bolt12PaymentQuoteResponse, Self::Err> {
        let amount = match melt_quote_request.amount {
            Some(amount) => amount,
            None => {
                let offer = Offer::from_str(&melt_quote_request.request)
                    .map_err(|_| anyhow!("Invalid offer in request"))?;

                match offer.amount() {
                    Some(LDKAmount::Bitcoin { amount_msats }) => amount_msats.into(),
                    None => {
                        return Err(cdk_lightning::Error::Anyhow(anyhow!(
                            "Amount not defined in offer or request"
                        )))
                    }
                    _ => return Err(cdk_lightning::Error::Anyhow(anyhow!("Unsupported unit"))),
                }
            }
        };

        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;

        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();

        let fee = match relative_fee_reserve > absolute_fee_reserve {
            true => relative_fee_reserve,
            false => absolute_fee_reserve,
        };

        Ok(Bolt12PaymentQuoteResponse {
            request_lookup_id: Uuid::new_v4().to_string(),
            amount,
            fee: fee.into(),
            state: cdk::nuts::MeltQuoteState::Unpaid,
            invoice: Some("".to_string()),
        })
    }

    async fn pay_bolt12_offer(
        &self,
        melt_quote: mint::MeltQuote,
        _amount: Option<Amount>,
        _max_fee_amount: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let bolt12 = &match melt_quote.request {
            PaymentRequest::Bolt11 { .. } => return Err(super::Error::WrongRequestType.into()),
            PaymentRequest::Bolt12 { offer, invoice: _ } => offer,
        };

        // let description = bolt12.description().to_string();

        // let status: Option<FakeInvoiceDescription> = serde_json::from_str(&description).ok();

        // let mut payment_states = self.payment_states.lock().await;
        // let payment_status = status
        //     .clone()
        //     .map(|s| s.pay_invoice_state)
        //     .unwrap_or(MeltQuoteState::Paid);

        // let checkout_going_status = status
        //     .clone()
        //     .map(|s| s.check_payment_state)
        //     .unwrap_or(MeltQuoteState::Paid);

        // payment_states.insert(payment_hash.clone(), checkout_going_status);

        // if let Some(description) = status {
        //     if description.check_err {
        //         let mut fail = self.failed_payment_check.lock().await;
        //         fail.insert(payment_hash.clone());
        //     }

        //     if description.pay_err {
        //         return Err(Error::UnknownInvoice.into());
        //     }
        // }

        Ok(PayInvoiceResponse {
            payment_preimage: Some("".to_string()),
            payment_lookup_id: bolt12.to_string(),
            status: super::MeltQuoteState::Paid,
            total_spent: melt_quote.amount,
            unit: melt_quote.unit,
        })
    }

    async fn create_bolt12_offer(
        &self,
        amount: Option<Amount>,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
        _single_use: bool,
    ) -> Result<CreateOfferResponse, Self::Err> {
        let secret_key = bitcoin::secp256k1::SecretKey::new(&mut rand::thread_rng());

        let secp_ctx = Secp256k1::new();

        let offer_builder = OfferBuilder::new(secret_key.public_key(&secp_ctx))
            .description(description)
            .absolute_expiry(Duration::from_secs(unix_expiry));

        let offer_builder = match amount {
            Some(amount) => {
                let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;
                offer_builder.amount_msats(amount.into())
            }
            None => offer_builder,
        };

        let offer = offer_builder.build().unwrap();

        let offer_string = offer.to_string();

        let sender = self.bolt12_sender.clone();

        let duration = time::Duration::from_secs(self.payment_delay);

        tokio::spawn(async move {
            // Wait for the random delay to elapse
            time::sleep(duration).await;

            // Send the message after waiting for the specified duration
            if sender.send(offer_string.clone()).await.is_err() {
                tracing::error!("Failed to send label: {}", offer_string);
            }
        });

        Ok(CreateOfferResponse {
            request_lookup_id: offer.to_string(),
            request: offer,
            expiry: Some(unix_expiry),
        })
    }
}
