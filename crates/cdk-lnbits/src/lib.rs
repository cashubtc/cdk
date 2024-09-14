//! CDK lightning backend for lnbits

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::pin::Pin;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use axum::Router;
use cdk::amount::Amount;
use cdk::cdk_lightning::{
    self, to_unit, CreateInvoiceResponse, MintLightning, PayInvoiceResponse, PaymentQuoteResponse,
    Settings,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{
    CurrencyUnit, MeltMethodSettings, MeltQuoteBolt11Request, MeltQuoteState, MintMethodSettings,
    MintQuoteState,
};
use cdk::util::unix_time;
use cdk::{mint, Bolt11Invoice};
use error::Error;
use futures::stream::StreamExt;
use futures::Stream;
use lnbits_rs::api::invoice::CreateInvoiceRequest;
use lnbits_rs::LNBitsClient;
use tokio::sync::Mutex;

pub mod error;

/// LNbits
#[derive(Clone)]
pub struct LNbits {
    lnbits_api: LNBitsClient,
    mint_settings: MintMethodSettings,
    melt_settings: MeltMethodSettings,
    fee_reserve: FeeReserve,
    receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
    webhook_url: String,
}

impl LNbits {
    /// Create new [`LNbits`] wallet
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        admin_api_key: String,
        invoice_api_key: String,
        api_url: String,
        mint_settings: MintMethodSettings,
        melt_settings: MeltMethodSettings,
        fee_reserve: FeeReserve,
        receiver: Arc<Mutex<Option<tokio::sync::mpsc::Receiver<String>>>>,
        webhook_url: String,
    ) -> Result<Self, Error> {
        let lnbits_api = LNBitsClient::new("", &admin_api_key, &invoice_api_key, &api_url, None)?;

        Ok(Self {
            lnbits_api,
            mint_settings,
            melt_settings,
            receiver,
            fee_reserve,
            webhook_url,
        })
    }
}

#[async_trait]
impl MintLightning for LNbits {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: false,
            unit: CurrencyUnit::Sat,
            mint_settings: self.mint_settings,
            melt_settings: self.melt_settings,
            invoice_description: true,
        }
    }

    async fn wait_any_invoice(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        let receiver = self
            .receiver
            .lock()
            .await
            .take()
            .ok_or(anyhow!("No receiver"))?;

        let lnbits_api = self.lnbits_api.clone();

        Ok(futures::stream::unfold(
            (receiver, lnbits_api),
            |(mut receiver, lnbits_api)| async move {
                match receiver.recv().await {
                    Some(msg) => {
                        let check = lnbits_api.is_invoice_paid(&msg).await;

                        match check {
                            Ok(state) => {
                                if state {
                                    Some((msg, (receiver, lnbits_api)))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    }
                    None => None,
                }
            },
        )
        .boxed())
    }

    async fn get_payment_quote(
        &self,
        melt_quote_request: &MeltQuoteBolt11Request,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        if melt_quote_request.unit != CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

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
        let pay_response = self
            .lnbits_api
            .pay_invoice(&melt_quote.request)
            .await
            .map_err(|err| {
                tracing::error!("Could not pay invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not pay invoice"))
            })?;

        let invoice_info = self
            .lnbits_api
            .find_invoice(&pay_response.payment_hash)
            .await
            .map_err(|err| {
                tracing::error!("Could not find invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not find invoice"))
            })?;

        let status = match invoice_info.pending {
            true => MeltQuoteState::Unpaid,
            false => MeltQuoteState::Paid,
        };

        let total_spent = Amount::from((invoice_info.amount + invoice_info.fee).unsigned_abs());

        Ok(PayInvoiceResponse {
            payment_hash: pay_response.payment_hash,
            payment_preimage: Some(invoice_info.payment_hash),
            status,
            total_spent,
            unit: CurrencyUnit::Sat,
        })
    }

    async fn create_invoice(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: u64,
    ) -> Result<CreateInvoiceResponse, Self::Err> {
        if unit != &CurrencyUnit::Sat {
            return Err(Self::Err::Anyhow(anyhow!("Unsupported unit")));
        }

        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let expiry = unix_expiry - time_now;

        let invoice_request = CreateInvoiceRequest {
            amount: to_unit(amount, unit, &CurrencyUnit::Sat)?.into(),
            memo: Some(description),
            unit: unit.to_string(),
            expiry: Some(expiry),
            webhook: Some(self.webhook_url.clone()),
            internal: None,
            out: false,
        };

        let create_invoice_response = self
            .lnbits_api
            .create_invoice(&invoice_request)
            .await
            .map_err(|err| {
                tracing::error!("Could not create invoice");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not create invoice"))
            })?;

        let request: Bolt11Invoice = create_invoice_response.payment_request.parse()?;
        let expiry = request.expires_at().map(|t| t.as_secs());

        Ok(CreateInvoiceResponse {
            request_lookup_id: create_invoice_response.payment_hash,
            request,
            expiry,
        })
    }

    async fn check_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let paid = self
            .lnbits_api
            .is_invoice_paid(request_lookup_id)
            .await
            .map_err(|err| {
                tracing::error!("Could not check invoice status");
                tracing::error!("{}", err.to_string());
                Self::Err::Anyhow(anyhow!("Could not check invoice status"))
            })?;

        let state = match paid {
            true => MintQuoteState::Paid,
            false => MintQuoteState::Unpaid,
        };

        Ok(state)
    }
}

impl LNbits {
    /// Create invoice webhook
    pub async fn create_invoice_webhook_router(
        &self,
        webhook_endpoint: &str,
        sender: tokio::sync::mpsc::Sender<String>,
    ) -> anyhow::Result<Router> {
        self.lnbits_api
            .create_invoice_webhook_router(webhook_endpoint, sender)
            .await
    }
}
