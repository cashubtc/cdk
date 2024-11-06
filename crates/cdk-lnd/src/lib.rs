//! CDK lightning backend for LND

// Copyright (c) 2023 Steffen (MIT)

#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use cdk::amount::{to_unit, Amount, MSAT_IN_SAT};
use cdk::cdk_lightning::{
    self, CreateInvoiceResponse, MintLightning, PayInvoiceResponse, PaymentQuoteResponse, Settings,
};
use cdk::mint::FeeReserve;
use cdk::nuts::{CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteState, MintQuoteState};
use cdk::util::{hex, unix_time};
use cdk::{mint, Bolt11Invoice};
use error::Error;
use fedimint_tonic_lnd::lnrpc::fee_limit::Limit;
use fedimint_tonic_lnd::lnrpc::payment::PaymentStatus;
use fedimint_tonic_lnd::lnrpc::FeeLimit;
use fedimint_tonic_lnd::Client;
use futures::{Stream, StreamExt};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub mod error;

/// Lnd mint backend
#[derive(Clone)]
pub struct Lnd {
    address: String,
    cert_file: PathBuf,
    macaroon_file: PathBuf,
    client: Arc<Mutex<Client>>,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
}

impl Lnd {
    /// Create new [`Lnd`]
    pub async fn new(
        address: String,
        cert_file: PathBuf,
        macaroon_file: PathBuf,
        fee_reserve: FeeReserve,
    ) -> Result<Self, Error> {
        let client = fedimint_tonic_lnd::connect(address.to_string(), &cert_file, &macaroon_file)
            .await
            .map_err(|err| {
                tracing::error!("Connection error: {}", err.to_string());
                Error::Connection
            })?;

        Ok(Self {
            address,
            cert_file,
            macaroon_file,
            client: Arc::new(Mutex::new(client)),
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
        })
    }
}

#[async_trait]
impl MintLightning for Lnd {
    type Err = cdk_lightning::Error;

    fn get_settings(&self) -> Settings {
        Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
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
        let mut client =
            fedimint_tonic_lnd::connect(self.address.clone(), &self.cert_file, &self.macaroon_file)
                .await
                .map_err(|_| Error::Connection)?;

        let stream_req = fedimint_tonic_lnd::lnrpc::InvoiceSubscription {
            add_index: 0,
            settle_index: 0,
        };

        let stream = client
            .lightning()
            .subscribe_invoices(stream_req)
            .await
            .unwrap()
            .into_inner();

        let cancel_token = self.wait_invoice_cancel_token.clone();

        Ok(futures::stream::unfold(
            (
                stream,
                cancel_token,
                Arc::clone(&self.wait_invoice_is_active),
            ),
            |(mut stream, cancel_token, is_active)| async move {
                is_active.store(true, Ordering::SeqCst);

                tokio::select! {
                    _ = cancel_token.cancelled() => {
                    // Stream is cancelled
                    is_active.store(false, Ordering::SeqCst);
                    tracing::info!("Waiting for lnd invoice ending");
                    None

                    }
                    msg = stream.message() => {

                match msg {
                    Ok(Some(msg)) => {
                        if msg.state == 1 {
                            Some((hex::encode(msg.r_hash), (stream, cancel_token, is_active)))
                        } else {
                            None
                        }
                    }
                    Ok(None) => {
                    is_active.store(false, Ordering::SeqCst);
                    tracing::info!("LND invoice stream ended.");
                        None
                    }, // End of stream
                    Err(err) => {
                    is_active.store(false, Ordering::SeqCst);
                    tracing::warn!("Encounrdered error in LND invoice stream. Stream ending");
                    tracing::error!("{:?}", err);
                    None

                    },   // Handle errors gracefully, ends the stream on error
                }
                    }
                }
            },
        )
        .boxed())
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
        partial_amount: Option<Amount>,
        max_fee: Option<Amount>,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let payment_request = melt_quote.request;

        let pay_req = fedimint_tonic_lnd::lnrpc::SendRequest {
            payment_request,
            fee_limit: max_fee.map(|f| {
                let limit = Limit::Fixed(u64::from(f) as i64);

                FeeLimit { limit: Some(limit) }
            }),
            amt_msat: partial_amount
                .map(|a| {
                    let msat = to_unit(a, &melt_quote.unit, &CurrencyUnit::Msat).unwrap();

                    u64::from(msat) as i64
                })
                .unwrap_or_default(),
            ..Default::default()
        };

        let payment_response = self
            .client
            .lock()
            .await
            .lightning()
            .send_payment_sync(fedimint_tonic_lnd::tonic::Request::new(pay_req))
            .await
            .map_err(|_| Error::PaymentFailed)?
            .into_inner();

        let total_amount = payment_response
            .payment_route
            .map_or(0, |route| route.total_amt_msat / MSAT_IN_SAT as i64)
            as u64;

        let (status, payment_preimage) = match total_amount == 0 {
            true => (MeltQuoteState::Unpaid, None),
            false => (
                MeltQuoteState::Paid,
                Some(hex::encode(payment_response.payment_preimage)),
            ),
        };

        Ok(PayInvoiceResponse {
            payment_lookup_id: hex::encode(payment_response.payment_hash),
            payment_preimage,
            status,
            total_spent: total_amount.into(),
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
        let time_now = unix_time();
        assert!(unix_expiry > time_now);

        let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;

        let invoice_request = fedimint_tonic_lnd::lnrpc::Invoice {
            value_msat: u64::from(amount) as i64,
            memo: description,
            ..Default::default()
        };

        let invoice = self
            .client
            .lock()
            .await
            .lightning()
            .add_invoice(fedimint_tonic_lnd::tonic::Request::new(invoice_request))
            .await
            .unwrap()
            .into_inner();

        let bolt11 = Bolt11Invoice::from_str(&invoice.payment_request)?;

        Ok(CreateInvoiceResponse {
            request_lookup_id: bolt11.payment_hash().to_string(),
            request: bolt11,
            expiry: Some(unix_expiry),
        })
    }

    async fn check_incoming_invoice_status(
        &self,
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let invoice_request = fedimint_tonic_lnd::lnrpc::PaymentHash {
            r_hash: hex::decode(request_lookup_id).unwrap(),
            ..Default::default()
        };

        let invoice = self
            .client
            .lock()
            .await
            .lightning()
            .lookup_invoice(fedimint_tonic_lnd::tonic::Request::new(invoice_request))
            .await
            .unwrap()
            .into_inner();

        match invoice.state {
            // Open
            0 => Ok(MintQuoteState::Unpaid),
            // Settled
            1 => Ok(MintQuoteState::Paid),
            // Canceled
            2 => Ok(MintQuoteState::Unpaid),
            // Accepted
            3 => Ok(MintQuoteState::Unpaid),
            _ => Err(Self::Err::Anyhow(anyhow!("Invalid status"))),
        }
    }

    async fn check_outgoing_payment(
        &self,
        payment_hash: &str,
    ) -> Result<PayInvoiceResponse, Self::Err> {
        let track_request = fedimint_tonic_lnd::routerrpc::TrackPaymentRequest {
            payment_hash: hex::decode(payment_hash).map_err(|_| Error::InvalidHash)?,
            no_inflight_updates: true,
        };
        let mut payment_stream = self
            .client
            .lock()
            .await
            .router()
            .track_payment_v2(track_request)
            .await
            .unwrap()
            .into_inner();

        while let Some(update_result) = payment_stream.next().await {
            match update_result {
                Ok(update) => {
                    let status = update.status();

                    let response = match status {
                        PaymentStatus::Unknown => PayInvoiceResponse {
                            payment_lookup_id: payment_hash.to_string(),
                            payment_preimage: Some(update.payment_preimage),
                            status: MeltQuoteState::Unknown,
                            total_spent: Amount::ZERO,
                            unit: self.get_settings().unit,
                        },
                        PaymentStatus::InFlight => {
                            // Continue waiting for the next update
                            continue;
                        }
                        PaymentStatus::Succeeded => PayInvoiceResponse {
                            payment_lookup_id: payment_hash.to_string(),
                            payment_preimage: Some(update.payment_preimage),
                            status: MeltQuoteState::Paid,
                            total_spent: Amount::from((update.value_sat + update.fee_sat) as u64),
                            unit: CurrencyUnit::Sat,
                        },
                        PaymentStatus::Failed => PayInvoiceResponse {
                            payment_lookup_id: payment_hash.to_string(),
                            payment_preimage: Some(update.payment_preimage),
                            status: MeltQuoteState::Failed,
                            total_spent: Amount::ZERO,
                            unit: self.get_settings().unit,
                        },
                    };

                    return Ok(response);
                }
                Err(_) => {
                    // Handle the case where the update itself is an error (e.g., stream failure)
                    return Err(Error::UnknownPaymentStatus.into());
                }
            }
        }

        // If the stream is exhausted without a final status
        Err(Error::UnknownPaymentStatus.into())
    }
}
