use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use cdk_common::payment::{
    CreateIncomingPaymentResponse, IncomingPaymentOptions as CdkIncomingPaymentOptions,
    MakePaymentResponse as CdkMakePaymentResponse, MintPayment,
    PaymentQuoteResponse as CdkPaymentQuoteResponse, WaitPaymentResponse,
};
use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tonic::{async_trait, Request};
use tracing::instrument;

use crate::proto::cdk_payment_processor_client::CdkPaymentProcessorClient;
use crate::proto::{
    CheckIncomingPaymentRequest, CheckOutgoingPaymentRequest, CreatePaymentRequest, EmptyRequest,
    IncomingPaymentOptions, MakePaymentRequest, OutgoingPaymentRequestType, PaymentQuoteRequest,
};

/// Payment Processor
#[derive(Clone)]
pub struct PaymentProcessorClient {
    inner: CdkPaymentProcessorClient<Channel>,
    wait_incoming_payment_stream_is_active: Arc<AtomicBool>,
    cancel_incoming_payment_listener: CancellationToken,
}

impl PaymentProcessorClient {
    /// Payment Processor
    pub async fn new(addr: &str, port: u16, tls_dir: Option<PathBuf>) -> anyhow::Result<Self> {
        let addr = format!("{addr}:{port}");
        let channel = if let Some(tls_dir) = tls_dir {
            // TLS directory exists, configure TLS

            // Check for ca.pem
            let ca_pem_path = tls_dir.join("ca.pem");
            if !ca_pem_path.exists() {
                let err_msg = format!("CA certificate file not found: {}", ca_pem_path.display());
                tracing::error!("{}", err_msg);
                return Err(anyhow!(err_msg));
            }

            // Check for client.pem
            let client_pem_path = tls_dir.join("client.pem");

            // Check for client.key
            let client_key_path = tls_dir.join("client.key");
            // check for ca cert
            let server_root_ca_cert = std::fs::read_to_string(&ca_pem_path)?;
            let server_root_ca_cert = Certificate::from_pem(server_root_ca_cert);
            let tls: ClientTlsConfig = match client_pem_path.exists() && client_key_path.exists() {
                true => {
                    let client_cert = std::fs::read_to_string(&client_pem_path)?;
                    let client_key = std::fs::read_to_string(&client_key_path)?;
                    let client_identity = Identity::from_pem(client_cert, client_key);
                    ClientTlsConfig::new()
                        .ca_certificate(server_root_ca_cert)
                        .identity(client_identity)
                }
                false => ClientTlsConfig::new().ca_certificate(server_root_ca_cert),
            };
            Channel::from_shared(addr)?
                .tls_config(tls)?
                .connect()
                .await?
        } else {
            // No TLS directory, skip TLS configuration
            Channel::from_shared(addr)?.connect().await?
        };

        let client = CdkPaymentProcessorClient::new(channel);

        Ok(Self {
            inner: client,
            wait_incoming_payment_stream_is_active: Arc::new(AtomicBool::new(false)),
            cancel_incoming_payment_listener: CancellationToken::new(),
        })
    }
}

#[async_trait]
impl MintPayment for PaymentProcessorClient {
    type Err = cdk_common::payment::Error;

    async fn get_settings(&self) -> Result<Value, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .get_settings(Request::new(EmptyRequest {}))
            .await
            .map_err(|err| {
                tracing::error!("Could not get settings: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let settings = response.into_inner();

        Ok(serde_json::from_str(&settings.inner)?)
    }

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
        unit: &cdk_common::CurrencyUnit,
        options: CdkIncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();

        let proto_options = match options {
            CdkIncomingPaymentOptions::Bolt11(opts) => IncomingPaymentOptions {
                options: Some(super::incoming_payment_options::Options::Bolt11(
                    super::Bolt11IncomingPaymentOptions {
                        description: opts.description,
                        amount: opts.amount.into(),
                        unix_expiry: opts.unix_expiry,
                    },
                )),
            },
            CdkIncomingPaymentOptions::Bolt12(opts) => IncomingPaymentOptions {
                options: Some(super::incoming_payment_options::Options::Bolt12(
                    super::Bolt12IncomingPaymentOptions {
                        description: opts.description,
                        amount: opts.amount.map(Into::into),
                        unix_expiry: opts.unix_expiry,
                    },
                )),
            },
        };

        let response = inner
            .create_payment(Request::new(CreatePaymentRequest {
                unit: unit.to_string(),
                options: Some(proto_options),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not create payment request: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let response = response.into_inner();

        Ok(response.try_into().map_err(|_| {
            cdk_common::payment::Error::Anyhow(anyhow!("Could not create create payment response"))
        })?)
    }

    async fn get_payment_quote(
        &self,
        unit: &cdk_common::CurrencyUnit,
        options: cdk_common::payment::OutgoingPaymentOptions,
    ) -> Result<CdkPaymentQuoteResponse, Self::Err> {
        let mut inner = self.inner.clone();

        let request_type = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(_) => {
                OutgoingPaymentRequestType::Bolt11Invoice
            }
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(_) => {
                OutgoingPaymentRequestType::Bolt12Offer
            }
        };

        let proto_request = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => opts.bolt11.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => opts.offer.to_string(),
        };

        let proto_options = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => opts.melt_options,
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => opts.melt_options,
        };

        let response = inner
            .get_payment_quote(Request::new(PaymentQuoteRequest {
                request: proto_request,
                unit: unit.to_string(),
                options: proto_options.map(Into::into),
                request_type: request_type.into(),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not get payment quote: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let response = response.into_inner();

        Ok(response.into())
    }

    async fn make_payment(
        &self,
        _unit: &cdk_common::CurrencyUnit,
        options: cdk_common::payment::OutgoingPaymentOptions,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();

        let payment_options = match options {
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => {
                super::OutgoingPaymentVariant {
                    options: Some(super::outgoing_payment_variant::Options::Bolt11(
                        super::Bolt11OutgoingPaymentOptions {
                            bolt11: opts.bolt11.to_string(),
                            max_fee_amount: opts.max_fee_amount.map(Into::into),
                            timeout_secs: opts.timeout_secs,
                            melt_options: opts.melt_options.map(Into::into),
                        },
                    )),
                }
            }
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => {
                super::OutgoingPaymentVariant {
                    options: Some(super::outgoing_payment_variant::Options::Bolt12(
                        super::Bolt12OutgoingPaymentOptions {
                            offer: opts.offer.to_string(),
                            max_fee_amount: opts.max_fee_amount.map(Into::into),
                            timeout_secs: opts.timeout_secs,
                            melt_options: opts.melt_options.map(Into::into),
                        },
                    )),
                }
            }
        };

        let response = inner
            .make_payment(Request::new(MakePaymentRequest {
                payment_options: Some(payment_options),
                partial_amount: None,
                max_fee_amount: None,
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not pay payment request: {}", err);

                if err.message().contains("already paid") {
                    cdk_common::payment::Error::InvoiceAlreadyPaid
                } else if err.message().contains("pending") {
                    cdk_common::payment::Error::InvoicePaymentPending
                } else {
                    cdk_common::payment::Error::Custom(err.to_string())
                }
            })?;

        let response = response.into_inner();

        Ok(response.try_into().map_err(|_err| {
            cdk_common::payment::Error::Anyhow(anyhow!("could not make payment"))
        })?)
    }

    #[instrument(skip_all)]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>, Self::Err> {
        self.wait_incoming_payment_stream_is_active
            .store(true, Ordering::SeqCst);
        tracing::debug!("Client waiting for payment");
        let mut inner = self.inner.clone();
        let stream = inner
            .wait_incoming_payment(EmptyRequest {})
            .await
            .map_err(|err| {
                tracing::error!("Could not check incoming payment stream: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?
            .into_inner();

        let cancel_token = self.cancel_incoming_payment_listener.clone();
        let cancel_fut = cancel_token.cancelled_owned();
        let active_flag = self.wait_incoming_payment_stream_is_active.clone();

        let transformed_stream = stream
            .take_until(cancel_fut)
            .filter_map(|item| async {
                match item {
                    Ok(value) => match value.try_into() {
                        Ok(payment_response) => Some(cdk_common::payment::Event::PaymentReceived(
                            payment_response,
                        )),
                        Err(e) => {
                            tracing::error!("Error converting payment response: {}", e);
                            None
                        }
                    },
                    Err(e) => {
                        tracing::error!("Error in payment stream: {}", e);
                        None
                    }
                }
            })
            .inspect(move |_| {
                active_flag.store(false, Ordering::SeqCst);
                tracing::info!("Payment stream inactive");
            });

        Ok(Box::pin(transformed_stream))
    }

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_incoming_payment_stream_is_active
            .load(Ordering::SeqCst)
    }

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self) {
        self.cancel_incoming_payment_listener.cancel();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &cdk_common::payment::PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .check_incoming_payment(Request::new(CheckIncomingPaymentRequest {
                request_identifier: Some(payment_identifier.clone().into()),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check incoming payment: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let check_incoming = response.into_inner();
        check_incoming
            .payments
            .into_iter()
            .map(|resp| resp.try_into().map_err(Self::Err::from))
            .collect()
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &cdk_common::payment::PaymentIdentifier,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .check_outgoing_payment(Request::new(CheckOutgoingPaymentRequest {
                request_identifier: Some(payment_identifier.clone().into()),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check outgoing payment: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let check_outgoing = response.into_inner();

        Ok(check_outgoing
            .try_into()
            .map_err(|_| cdk_common::payment::Error::UnknownPaymentState)?)
    }
}
