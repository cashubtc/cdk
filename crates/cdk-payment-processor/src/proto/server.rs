use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::payment::MintPayment;
use futures::{Stream, StreamExt};
use serde_json::Value;
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Instant};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{async_trait, Request, Response, Status};
use tracing::instrument;

use super::cdk_payment_processor_server::{CdkPaymentProcessor, CdkPaymentProcessorServer};
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
        let CreatePaymentRequest {
            amount,
            unit,
            description,
            unix_expiry,
        } = request.into_inner();

        let unit =
            CurrencyUnit::from_str(&unit).map_err(|_| Status::invalid_argument("Invalid unit"))?;
        let invoice_response = self
            .inner
            .create_incoming_payment_request(amount.into(), &unit, description, unix_expiry)
            .await
            .map_err(|_| Status::internal("Could not create invoice"))?;

        Ok(Response::new(invoice_response.into()))
    }

    async fn get_payment_quote(
        &self,
        request: Request<PaymentQuoteRequest>,
    ) -> Result<Response<PaymentQuoteResponse>, Status> {
        let request = request.into_inner();

        let options: Option<cdk_common::MeltOptions> =
            request.options.as_ref().map(|options| (*options).into());

        let payment_quote = self
            .inner
            .get_payment_quote(
                &request.request,
                &CurrencyUnit::from_str(&request.unit)
                    .map_err(|_| Status::invalid_argument("Invalid currency unit"))?,
                options,
            )
            .await
            .map_err(|err| {
                tracing::error!("Could not get bolt11 melt quote: {}", err);
                Status::internal("Could not get melt quote")
            })?;

        Ok(Response::new(payment_quote.into()))
    }

    async fn make_payment(
        &self,
        request: Request<MakePaymentRequest>,
    ) -> Result<Response<MakePaymentResponse>, Status> {
        let request = request.into_inner();

        let pay_invoice = self
            .inner
            .make_payment(
                request
                    .melt_quote
                    .ok_or(Status::invalid_argument("Meltquote is required"))?
                    .try_into()
                    .map_err(|_err| Status::invalid_argument("Invalid melt quote"))?,
                request.partial_amount.map(|a| a.into()),
                request.max_fee_amount.map(|a| a.into()),
            )
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

        let check_response = self
            .inner
            .check_incoming_payment_status(&request.request_lookup_id)
            .await
            .map_err(|_| Status::internal("Could not check incoming payment status"))?;

        Ok(Response::new(CheckIncomingPaymentResponse {
            status: QuoteState::from(check_response).into(),
        }))
    }

    async fn check_outgoing_payment(
        &self,
        request: Request<CheckOutgoingPaymentRequest>,
    ) -> Result<Response<MakePaymentResponse>, Status> {
        let request = request.into_inner();

        let check_response = self
            .inner
            .check_outgoing_payment(&request.request_lookup_id)
            .await
            .map_err(|_| Status::internal("Could not check incoming payment status"))?;

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
                            while let Some(request_lookup_id) = stream.next().await {
                                                match tx.send(Result::<_, Status>::Ok(WaitIncomingPaymentResponse{lookup_id: request_lookup_id} )).await {
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
