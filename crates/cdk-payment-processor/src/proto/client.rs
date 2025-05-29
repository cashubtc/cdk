use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use cdk_common::payment::{
    CreateIncomingPaymentResponse, MakePaymentResponse as CdkMakePaymentResponse, MintPayment,
    OutgoingPaymentOptions, PaymentIdentifier as CdkPaymentIdentifier, PaymentQuoteResponse,
    WaitPaymentResponse,
};
use cdk_common::CurrencyUnit;
use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tonic::{async_trait, Request};
use tracing::instrument;

use super::cdk_payment_processor_client::CdkPaymentProcessorClient;
use super::{
    cdk_payment_id_to_proto, proto_to_cdk_payment_id, CheckIncomingPaymentRequest,
    CheckOutgoingPaymentRequest, CreatePaymentRequest, EmptyRequest, MakePaymentRequest,
    OutgoingPaymentVariant,
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
                tracing::error!("{err_msg}");
                return Err(anyhow!(err_msg));
            }

            // Check for client.pem
            let client_pem_path = tls_dir.join("client.pem");
            if !client_pem_path.exists() {
                let err_msg = format!(
                    "Client certificate file not found: {}",
                    client_pem_path.display()
                );
                tracing::error!("{err_msg}");
                return Err(anyhow!(err_msg));
            }

            // Check for client.key
            let client_key_path = tls_dir.join("client.key");
            if !client_key_path.exists() {
                let err_msg = format!("Client key file not found: {}", client_key_path.display());
                tracing::error!("{err_msg}");
                return Err(anyhow!(err_msg));
            }

            let server_root_ca_cert = std::fs::read_to_string(&ca_pem_path)?;
            let server_root_ca_cert = Certificate::from_pem(server_root_ca_cert);
            let client_cert = std::fs::read_to_string(&client_pem_path)?;
            let client_key = std::fs::read_to_string(&client_key_path)?;
            let client_identity = Identity::from_pem(client_cert, client_key);
            let tls = ClientTlsConfig::new()
                .ca_certificate(server_root_ca_cert)
                .identity(client_identity);

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
                tracing::error!("Could not get settings: {err}");
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let settings = response.into_inner();

        Ok(serde_json::from_str(&settings.inner)?)
    }

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: cdk_common::payment::IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();

        // Convert from common IncomingPaymentOptions to protobuf IncomingPaymentOptions
        let proto_options = match options {
            cdk_common::payment::IncomingPaymentOptions::Bolt11(bolt11_options) => {
                super::IncomingPaymentOptions {
                    options: Some(super::incoming_payment_options::Options::Bolt11(
                        super::Bolt11IncomingPaymentOptions {
                            description: bolt11_options.description,
                            amount: bolt11_options.amount.into(),
                            unix_expiry: bolt11_options.unix_expiry,
                        },
                    )),
                }
            }
            cdk_common::payment::IncomingPaymentOptions::Bolt12(bolt12_options) => {
                super::IncomingPaymentOptions {
                    options: Some(super::incoming_payment_options::Options::Bolt12(
                        super::Bolt12IncomingPaymentOptions {
                            description: bolt12_options.description,
                            amount: bolt12_options.amount.map(|a| a.into()),
                            unix_expiry: bolt12_options.unix_expiry,
                        },
                    )),
                }
            }
        };

        let response = inner
            .create_payment(Request::new(CreatePaymentRequest {
                unit: unit.to_string(),
                options: Some(proto_options),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not create payment request: {err}");
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let response = response.into_inner();

        Ok(response.try_into().map_err(|_| {
            cdk_common::payment::Error::Anyhow(anyhow!("Could not create create payment response"))
        })?)
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let mut inner = self.inner.clone();

        // Determine the request type and string based on the OutgoingPaymentOptions variant
        let (request_str, request_type) = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => (
                bolt11_options.bolt11.to_string(),
                super::OutgoingPaymentRequestType::Bolt11Invoice,
            ),
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                // Get the reference from the Box
                let bolt12_options = &**bolt12_options;
                (
                    bolt12_options.offer.to_string(),
                    super::OutgoingPaymentRequestType::Bolt12Offer,
                )
            }
        };

        // Extract MeltOptions if present
        let melt_options = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => bolt11_options.melt_options,
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                // For Bolt12, we might have MeltOptions in the form of Amountless
                bolt12_options.melt_options
            }
        };

        let response = inner
            .get_payment_quote(Request::new(super::PaymentQuoteRequest {
                request: request_str,
                unit: unit.to_string(),
                options: melt_options.map(|o| o.into()),
                request_type: request_type.into(),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not get payment quote: {err}");
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let response = response.into_inner();

        response.try_into().map_err(|err| {
            tracing::error!("Could not convert payment quote response: {err}");
            cdk_common::payment::Error::Custom(format!("Failed to convert payment quote: {err}"))
        })
    }

    async fn make_payment(
        &self,
        _unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();

        // Extract max fee amount if present
        let max_fee_amount = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11) => bolt11.max_fee_amount,
            OutgoingPaymentOptions::Bolt12(bolt12) => bolt12.max_fee_amount,
        };

        let response = inner
            .make_payment(Request::new(MakePaymentRequest {
                payment_options: Some(OutgoingPaymentVariant {
                    options: Some(match &options {
                        OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                            super::outgoing_payment_variant::Options::Bolt11(
                                super::Bolt11OutgoingPaymentOptions {
                                    bolt11: bolt11_options.bolt11.to_string(),
                                    max_fee_amount: max_fee_amount.map(|a| a.into()),
                                    timeout_secs: None,
                                    melt_options: bolt11_options.melt_options.map(|o| o.into()),
                                },
                            )
                        }
                        OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                            super::outgoing_payment_variant::Options::Bolt12(
                                super::Bolt12OutgoingPaymentOptions {
                                    offer: bolt12_options.offer.to_string(),
                                    max_fee_amount: max_fee_amount.map(|a| a.into()),
                                    timeout_secs: None,
                                    invoice: bolt12_options.invoice.clone(),
                                    melt_options: bolt12_options.melt_options.map(|o| o.into()),
                                },
                            )
                        }
                    }),
                }),
                partial_amount: None,
                max_fee_amount: max_fee_amount.map(|a| a.into()),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not pay payment request: {err}");

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

    /// Listen for invoices to be paid to the mint
    #[instrument(skip_all)]
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = WaitPaymentResponse> + Send>>, Self::Err> {
        self.wait_incoming_payment_stream_is_active
            .store(true, Ordering::SeqCst);
        tracing::debug!("Client waiting for payment");
        let mut inner = self.inner.clone();
        let stream = inner
            .wait_incoming_payment(EmptyRequest {})
            .await
            .map_err(|err| {
                tracing::error!("Could not check incoming payment stream: {err}");
                cdk_common::payment::Error::Custom(err.to_string())
            })?;
        let stream = stream.into_inner();

        let cancel_token = self.cancel_incoming_payment_listener.clone();
        let cancel_fut = cancel_token.cancelled_owned();
        // let active_flag = self.wait_incoming_payment_stream_is_active.clone();

        let transformed_stream = stream
            .take_until(cancel_fut)
            .filter_map(|item| async move {
                match item {
                    Ok(value) => {
                        if let Some(payment_id) = &value.payment_identifier {
                            match proto_to_cdk_payment_id(payment_id) {
                                Ok(identifier) => Some(WaitPaymentResponse {
                                    payment_identifier: identifier,
                                    payment_amount: value.payment_amount.into(),
                                    // TODO: Handle this error
                                    unit: CurrencyUnit::from_str(&value.unit).expect("Valid unit"),
                                    payment_id: value.payment_id,
                                }),
                                Err(e) => {
                                    tracing::error!("Error converting payment identifier: {e}");
                                    None
                                }
                            }
                        } else {
                            tracing::error!("Payment identifier is missing");
                            None
                        }
                    }
                    Err(e) => {
                        tracing::error!("Error in payment stream: {e}");
                        None // Skip this item and continue with the stream
                    }
                }
            })
            .inspect(move |_| {
                tracing::debug!("Got event stream");
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
        request_lookup_id: &CdkPaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .check_incoming_payment(Request::new(CheckIncomingPaymentRequest {
                request_identifier: Some(cdk_payment_id_to_proto(request_lookup_id)),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check incoming payment: {err}");
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let check_incoming = response.into_inner();

        // Convert the CheckIncomingPaymentResponse to Vec<WaitPaymentResponse>
        Ok(check_incoming
            .payments
            .into_iter()
            .map(|p| WaitPaymentResponse {
                payment_identifier: proto_to_cdk_payment_id(p.payment_identifier.as_ref().unwrap())
                    .unwrap(),
                payment_amount: p.payment_amount.into(),
                unit: CurrencyUnit::from_str(&p.unit).expect("Valid unit"),
                payment_id: p.payment_id,
            })
            .collect())
    }

    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &CdkPaymentIdentifier,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .check_outgoing_payment(Request::new(CheckOutgoingPaymentRequest {
                request_identifier: Some(cdk_payment_id_to_proto(request_lookup_id)),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check outgoing payment: {err}");
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let check_outgoing = response.into_inner();

        Ok(check_outgoing
            .try_into()
            .map_err(|_| cdk_common::payment::Error::UnknownPaymentState)?)
    }
}
