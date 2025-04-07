use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::payment::{MintPayment, OutgoingPaymentOptions};
use cdk_common::CurrencyUnit;
use futures::{Stream, StreamExt};
use lightning_invoice::Bolt11Invoice;
use serde_json::Value;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Instant};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{async_trait, Request, Response, Status};
use tracing::instrument;

use super::cdk_payment_processor_server::{CdkPaymentProcessor, CdkPaymentProcessorServer};
use super::{cdk_payment_id_to_proto, proto_to_cdk_payment_id};
use crate::proto::*;

type ResponseStream =
    Pin<Box<dyn Stream<Item = Result<WaitIncomingPaymentResponse, Status>> + Send>>;

/// Payment Processor
#[derive(Clone)]
pub struct PaymentProcessorServer {
    inner: Arc<dyn MintPayment<Err = cdk_common::payment::Error> + Send + Sync>,
    socket_addr: SocketAddr,
    shutdown: Arc<Notify>,
    handle: Option<Arc<JoinHandle<anyhow::Result<()>>>>,
}

impl PaymentProcessorServer {
    /// Create new [`PaymentProcessorServer`]
    pub fn new(
        payment_processor: Arc<dyn MintPayment<Err = cdk_common::payment::Error> + Send + Sync>,
        addr: &str,
        port: u16,
    ) -> anyhow::Result<Self> {
        let socket_addr = SocketAddr::new(addr.parse()?, port);
        Ok(Self {
            inner: payment_processor,
            socket_addr,
            shutdown: Arc::new(Notify::new()),
            handle: None,
        })
    }

    /// Start fake wallet grpc server
    pub async fn start(&mut self, tls_dir: Option<PathBuf>) -> anyhow::Result<()> {
        tracing::info!("Starting RPC server {}", self.socket_addr);

        let server = match tls_dir {
            Some(tls_dir) => {
                tracing::info!("TLS configuration found, starting secure server");

                // Check for server.pem
                let server_pem_path = tls_dir.join("server.pem");
                if !server_pem_path.exists() {
                    let err_msg = format!(
                        "TLS certificate file not found: {}",
                        server_pem_path.display()
                    );
                    tracing::error!("{}", err_msg);
                    return Err(anyhow::anyhow!(err_msg));
                }

                // Check for server.key
                let server_key_path = tls_dir.join("server.key");
                if !server_key_path.exists() {
                    let err_msg = format!("TLS key file not found: {}", server_key_path.display());
                    tracing::error!("{}", err_msg);
                    return Err(anyhow::anyhow!(err_msg));
                }

                // Check for ca.pem
                let ca_pem_path = tls_dir.join("ca.pem");
                if !ca_pem_path.exists() {
                    let err_msg =
                        format!("CA certificate file not found: {}", ca_pem_path.display());
                    tracing::error!("{}", err_msg);
                    return Err(anyhow::anyhow!(err_msg));
                }

                let cert = std::fs::read_to_string(&server_pem_path)?;
                let key = std::fs::read_to_string(&server_key_path)?;
                let client_ca_cert = std::fs::read_to_string(&ca_pem_path)?;

                let client_ca_cert = Certificate::from_pem(client_ca_cert);
                let server_identity = Identity::from_pem(cert, key);
                let tls_config = ServerTlsConfig::new()
                    .identity(server_identity)
                    .client_ca_root(client_ca_cert);

                Server::builder()
                    .tls_config(tls_config)?
                    .add_service(CdkPaymentProcessorServer::new(self.clone()))
            }
            None => {
                tracing::warn!("No valid TLS configuration found, starting insecure server");
                Server::builder().add_service(CdkPaymentProcessorServer::new(self.clone()))
            }
        };

        let shutdown = self.shutdown.clone();
        let addr = self.socket_addr;

        self.handle = Some(Arc::new(tokio::spawn(async move {
            let server = server.serve_with_shutdown(addr, async {
                shutdown.notified().await;
            });

            server.await?;
            Ok(())
        })));

        Ok(())
    }

    /// Stop fake wallet grpc server
    pub async fn stop(&self) -> anyhow::Result<()> {
        const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

        if let Some(handle) = &self.handle {
            tracing::info!("Initiating server shutdown");
            self.shutdown.notify_waiters();

            let start = Instant::now();

            while !handle.is_finished() {
                if start.elapsed() >= SHUTDOWN_TIMEOUT {
                    tracing::error!(
                        "Server shutdown timed out after {} seconds, aborting handle",
                        SHUTDOWN_TIMEOUT.as_secs()
                    );
                    handle.abort();
                    break;
                }
                sleep(Duration::from_millis(100)).await;
            }

            if handle.is_finished() {
                tracing::info!("Server shutdown completed successfully");
            }
        } else {
            tracing::info!("No server handle found, nothing to stop");
        }

        Ok(())
    }
}

impl Drop for PaymentProcessorServer {
    fn drop(&mut self) {
        tracing::debug!("Dropping payment process server");
        self.shutdown.notify_one();
    }
}

#[async_trait]
impl CdkPaymentProcessor for PaymentProcessorServer {
    async fn get_settings(
        &self,
        _request: Request<SettingsRequest>,
    ) -> Result<Response<SettingsResponse>, Status> {
        let settings: Value = self
            .inner
            .get_settings()
            .await
            .map_err(|_| Status::internal("Could not get settings"))?;

        Ok(Response::new(SettingsResponse {
            inner: settings.to_string(),
        }))
    }

    async fn create_payment(
        &self,
        request: Request<CreatePaymentRequest>,
    ) -> Result<Response<CreatePaymentResponse>, Status> {
        let CreatePaymentRequest { unit, options } = request.into_inner();

        let unit =
            CurrencyUnit::from_str(&unit).map_err(|_| Status::invalid_argument("Invalid unit"))?;

        let options =
            options.ok_or_else(|| Status::invalid_argument("Payment options required"))?;

        // Convert from protobuf IncomingPaymentOptions to common IncomingPaymentOptions
        let payment_options = match options.options {
            Some(crate::proto::incoming_payment_options::Options::Bolt11(bolt11_options)) => {
                cdk_common::payment::IncomingPaymentOptions::Bolt11(
                    cdk_common::payment::Bolt11IncomingPaymentOptions {
                        description: bolt11_options.description,
                        amount: bolt11_options.amount.into(),
                        unix_expiry: bolt11_options.unix_expiry,
                    },
                )
            }
            Some(crate::proto::incoming_payment_options::Options::Bolt12(bolt12_options)) => {
                cdk_common::payment::IncomingPaymentOptions::Bolt12(Box::new(
                    cdk_common::payment::Bolt12IncomingPaymentOptions {
                        description: bolt12_options.description,
                        amount: bolt12_options.amount.map(|a| a.into()),
                        unix_expiry: bolt12_options.unix_expiry,
                        single_use: bolt12_options.single_use,
                    },
                ))
            }
            None => return Err(Status::invalid_argument("No payment options provided")),
        };

        let invoice_response = self
            .inner
            .create_incoming_payment_request(&unit, payment_options)
            .await
            .map_err(|_| Status::internal("Could not create invoice"))?;

        Ok(Response::new(invoice_response.into()))
    }

    async fn get_payment_quote(
        &self,
        request: Request<PaymentQuoteRequest>,
    ) -> Result<Response<PaymentQuoteResponse>, Status> {
        let request = request.into_inner();

        // Convert the request type from proto enum to OutgoingPaymentOptions
        let request_str = &request.request;
        let payment_options = match request.request_type() {
            OutgoingPaymentRequestType::Bolt11Invoice => {
                let bolt11 = Bolt11Invoice::from_str(request_str)
                    .map_err(|_| Status::invalid_argument("Invalid BOLT11 invoice"))?;

                cdk_common::payment::OutgoingPaymentOptions::Bolt11(Box::new(
                    cdk_common::payment::Bolt11OutgoingPaymentOptions {
                        bolt11,
                        max_fee_amount: None,
                        timeout_secs: None,
                        melt_options: request.options.as_ref().map(|o| (*o).into()),
                    },
                ))
            }
            OutgoingPaymentRequestType::Bolt12Offer => {
                // We'll skip the Offer parse for now since we removed the dependency
                return Err(Status::unimplemented("BOLT12 is not yet supported"));
            }
        };

        let payment_quote = self
            .inner
            .get_payment_quote(
                &CurrencyUnit::from_str(&request.unit)
                    .map_err(|_| Status::invalid_argument("Invalid currency unit"))?,
                payment_options,
            )
            .await
            .map_err(|err| {
                tracing::error!("Could not get payment quote: {}", err);
                Status::internal("Could not get melt quote")
            })?;

        Ok(Response::new(payment_quote.into()))
    }

    async fn make_payment(
        &self,
        request: Request<MakePaymentRequest>,
    ) -> Result<Response<MakePaymentResponse>, Status> {
        let request = request.into_inner();

        let melt_quote = request
            .melt_quote
            .ok_or(Status::invalid_argument("Meltquote is required"))?;

        // Instead of trying to convert MeltQuote to OutgoingPaymentOptions,
        // let's manually create the appropriate OutgoingPaymentOptions
        let options = if melt_quote.payment_method == "bolt11" {
            // For BOLT11, create Bolt11OutgoingPaymentOptions
            let bolt11 = Bolt11Invoice::from_str(&melt_quote.request)
                .map_err(|_| Status::invalid_argument("Invalid BOLT11 invoice"))?;

            OutgoingPaymentOptions::Bolt11(Box::new(
                cdk_common::payment::Bolt11OutgoingPaymentOptions {
                    bolt11,
                    max_fee_amount: Some(melt_quote.fee_reserve.into()),
                    timeout_secs: None,
                    melt_options: melt_quote.options.map(|o| o.into()),
                },
            ))
        } else if melt_quote.payment_method == "bolt12" {
            // For now, return not implemented for BOLT12
            return Err(Status::unimplemented("BOLT12 not yet supported"));
        } else {
            return Err(Status::invalid_argument("Unsupported payment method"));
        };

        // Extract currency unit from the melt quote
        let unit = CurrencyUnit::from_str(&melt_quote.unit)
            .map_err(|_| Status::invalid_argument("Invalid currency unit"))?;

        let pay_invoice = self
            .inner
            .make_payment(&unit, options)
            .await
            .map_err(|err| {
                tracing::error!("Could not make payment: {}", err);

                match err {
                    cdk_common::payment::Error::InvoiceAlreadyPaid => {
                        Status::already_exists("Payment request already paid")
                    }
                    cdk_common::payment::Error::InvoicePaymentPending => {
                        Status::already_exists("Payment request pending")
                    }
                    _ => Status::internal("Could not pay invoice"),
                }
            })?;

        Ok(Response::new(pay_invoice.into()))
    }

    async fn check_incoming_payment(
        &self,
        request: Request<CheckIncomingPaymentRequest>,
    ) -> Result<Response<CheckIncomingPaymentResponse>, Status> {
        let request = request.into_inner();

        let request_identifier = request
            .request_identifier
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("Missing request identifier"))
            .and_then(|id| {
                proto_to_cdk_payment_id(id)
                    .map_err(|_| Status::invalid_argument("Invalid request identifier"))
            })?;

        let check_response = self
            .inner
            .check_incoming_payment_status(&request_identifier)
            .await
            .map_err(|_| Status::internal("Could not check incoming payment status"))?;

        Ok(Response::new(CheckIncomingPaymentResponse {
            payments: check_response
                .into_iter()
                .map(|p| WaitIncomingPaymentResponse {
                    payment_identifier: Some(cdk_payment_id_to_proto(&p.payment_identifier)),
                    payment_amount: p.payment_amount.into(),
                    unit: p.unit.to_string(),
                    payment_id: p.payment_id,
                })
                .collect(),
        }))
    }

    async fn check_outgoing_payment(
        &self,
        request: Request<CheckOutgoingPaymentRequest>,
    ) -> Result<Response<MakePaymentResponse>, Status> {
        let request = request.into_inner();

        let request_identifier = request
            .request_identifier
            .as_ref()
            .ok_or_else(|| Status::invalid_argument("Missing request identifier"))
            .and_then(|id| {
                proto_to_cdk_payment_id(id)
                    .map_err(|_| Status::invalid_argument("Invalid request identifier"))
            })?;

        let check_response = self
            .inner
            .check_outgoing_payment(&request_identifier)
            .await
            .map_err(|_| Status::internal("Could not check outgoing payment status"))?;

        Ok(Response::new(check_response.into()))
    }

    type WaitIncomingPaymentStream = ResponseStream;

    // Clippy thinks select is not stable but it compiles fine on MSRV (1.63.0)
    #[allow(clippy::incompatible_msrv)]
    #[instrument(skip_all)]
    async fn wait_incoming_payment(
        &self,
        _request: Request<WaitIncomingPaymentRequest>,
    ) -> Result<Response<Self::WaitIncomingPaymentStream>, Status> {
        tracing::debug!("Server waiting for payment stream");
        let (tx, rx) = mpsc::channel(128);

        let shutdown_clone = self.shutdown.clone();
        let ln = self.inner.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                _ = shutdown_clone.notified() => {
                    tracing::info!("Shutdown signal received, stopping task for ");
                    ln.cancel_wait_invoice();
                    break;
                }
                result = ln.wait_any_incoming_payment() => {
                    match result {
                        Ok(mut stream) => {
                            while let Some(response) = stream.next().await {
                                match tx.send(Result::<_, Status>::Ok(WaitIncomingPaymentResponse{
                                    payment_identifier: Some(cdk_payment_id_to_proto(&response.payment_identifier)),
                                    payment_amount: response.payment_amount.into(),
                                    unit: response.unit.to_string(),
                                    payment_id: response.payment_id
                                } )).await {
                    Ok(_) => {
                        // item (server response) was queued to be send to client
                    }
                    Err(item) => {
                        tracing::error!("Error adding incoming payment to stream: {}", item);
                        break;
                    }
                }
                            }
                        }
                        Err(err) => {
                            tracing::warn!("Could not get invoice stream for {}", err);

                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                    }
                }
                }
            }
        });

        let output_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(output_stream) as Self::WaitIncomingPaymentStream
        ))
    }
}
