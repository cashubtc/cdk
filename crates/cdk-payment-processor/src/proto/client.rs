use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use cdk_common::payment::{
    BaseMintSettings, CreateIncomingPaymentResponse, MakePaymentResponse as CdkMakePaymentResponse,
    MintPayment, PaymentQuoteResponse,
};
use cdk_common::{mint, Amount, CurrencyUnit, MeltOptions, MintQuoteState};
use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tonic::{async_trait, Request};
use tracing::instrument;

use super::cdk_payment_processor_client::CdkPaymentProcessorClient;
use super::{
    CheckIncomingPaymentRequest, CheckOutgoingPaymentRequest, CreatePaymentRequest,
    MakePaymentRequest, SettingsRequest, WaitIncomingPaymentRequest,
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
        let addr = format!("{}:{}", addr, port);
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
            if !client_pem_path.exists() {
                let err_msg = format!(
                    "Client certificate file not found: {}",
                    client_pem_path.display()
                );
                tracing::error!("{}", err_msg);
                return Err(anyhow!(err_msg));
            }

            // Check for client.key
            let client_key_path = tls_dir.join("client.key");
            if !client_key_path.exists() {
                let err_msg = format!("Client key file not found: {}", client_key_path.display());
                tracing::error!("{}", err_msg);
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

    async fn get_settings(&self) -> Result<Box<dyn BaseMintSettings>, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .get_settings(Request::new(SettingsRequest {}))
            .await
            .map_err(|err| {
                tracing::error!("Could not get settings: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        Ok(Box::new(response.into_inner()))
    }

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
        amount: Amount,
        unit: &CurrencyUnit,
        description: String,
        unix_expiry: Option<u64>,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .create_payment(Request::new(CreatePaymentRequest {
                amount: amount.into(),
                unit: unit.to_string(),
                description,
                unix_expiry,
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
        request: &str,
        unit: &CurrencyUnit,
        options: Option<MeltOptions>,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .get_payment_quote(Request::new(super::PaymentQuoteRequest {
                request: request.to_string(),
                unit: unit.to_string(),
                options: options.map(|o| o.into()),
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
        melt_quote: mint::MeltQuote,
        partial_amount: Option<Amount>,
        max_fee_amount: Option<Amount>,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .make_payment(Request::new(MakePaymentRequest {
                melt_quote: Some(melt_quote.into()),
                partial_amount: partial_amount.map(|a| a.into()),
                max_fee_amount: max_fee_amount.map(|a| a.into()),
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

    /// Listen for invoices to be paid to the mint
    #[instrument(skip_all)]
    async fn wait_any_incoming_payment(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, Self::Err> {
        self.wait_incoming_payment_stream_is_active
            .store(true, Ordering::SeqCst);
        tracing::debug!("Client waiting for payment");
        let mut inner = self.inner.clone();
        let stream = inner
            .wait_incoming_payment(WaitIncomingPaymentRequest {})
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
            .filter_map(|item| async move {
                match item {
                    Ok(value) => {
                        tracing::warn!("{}", value.lookup_id);
                        Some(value.lookup_id)
                    }
                    Err(e) => {
                        tracing::error!("Error in payment stream: {}", e);
                        None // Skip this item and continue with the stream
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
        request_lookup_id: &str,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .check_incoming_payment(Request::new(CheckIncomingPaymentRequest {
                request_lookup_id: request_lookup_id.to_string(),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check incoming payment: {}", err);
                cdk_common::payment::Error::Custom(err.to_string())
            })?;

        let check_incoming = response.into_inner();

        let status = check_incoming.status().as_str_name();

        Ok(MintQuoteState::from_str(status)?)
    }

    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &str,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .check_outgoing_payment(Request::new(CheckOutgoingPaymentRequest {
                request_lookup_id: request_lookup_id.to_string(),
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
