use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use std::time::Duration;

use cdk_common::grpc::create_version_check_interceptor;
use cdk_common::payment::{DynMintPayment, IncomingPaymentOptions};
use cdk_common::{CurrencyUnit, QuoteId};
use futures::Stream;
use lightning::offers::offer::Offer;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::{Certificate, Identity, Server, ServerTlsConfig};
use tonic::{async_trait, Request, Response, Status};
use tracing::instrument;

use super::cdk_payment_processor_server::{CdkPaymentProcessor, CdkPaymentProcessorServer};
use super::service::PaymentProcessorService;
use crate::error::{payment_error_to_status, Error};
use crate::proto::{TryFromProtoAmount, *};

type ResponseStream = Pin<Box<dyn Stream<Item = Result<PaymentEventResponse, Status>> + Send>>;

struct PaymentEventStream {
    inner: Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>,
    payment_processor: DynMintPayment,
    completed: bool,
}

impl Stream for PaymentEventStream {
    type Item = Result<PaymentEventResponse, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(event.into()))),
            Poll::Ready(None) => {
                this.completed = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for PaymentEventStream {
    fn drop(&mut self) {
        if !self.completed {
            self.payment_processor.cancel_payment_event_stream();
        }
    }
}

/// Payment Processor
pub struct PaymentProcessorServer {
    service: PaymentProcessorService,
    socket_addr: SocketAddr,
    shutdown: Option<CancellationToken>,
    handle: Option<JoinHandle<anyhow::Result<()>>>,
    backend_started: bool,
}

impl std::fmt::Debug for PaymentProcessorServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentProcessorServer")
            .field("socket_addr", &self.socket_addr)
            .finish_non_exhaustive()
    }
}

impl PaymentProcessorServer {
    /// Create new [`PaymentProcessorServer`]
    pub fn new(payment_processor: DynMintPayment, addr: &str, port: u16) -> anyhow::Result<Self> {
        let socket_addr = SocketAddr::new(addr.parse()?, port);
        Ok(Self {
            service: PaymentProcessorService::new(payment_processor),
            socket_addr,
            shutdown: None,
            handle: None,
            backend_started: false,
        })
    }

    /// Return the address on which the server is configured or currently listening.
    pub fn local_addr(&self) -> SocketAddr {
        self.socket_addr
    }

    /// Start the payment processor gRPC server.
    pub async fn start(&mut self, tls_dir: Option<PathBuf>) -> anyhow::Result<()> {
        if self.handle.is_some() || self.backend_started {
            anyhow::bail!("Payment processor server is already running");
        }

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

                Server::builder().tls_config(tls_config)?.add_service(
                    CdkPaymentProcessorServer::with_interceptor(
                        self.service.clone(),
                        create_version_check_interceptor(
                            cdk_common::grpc::VERSION_HEADER,
                            cdk_common::PAYMENT_PROCESSOR_PROTOCOL_VERSION,
                        ),
                    ),
                )
            }
            None => {
                tracing::warn!("No valid TLS configuration found, starting insecure server");
                Server::builder().add_service(CdkPaymentProcessorServer::with_interceptor(
                    self.service.clone(),
                    create_version_check_interceptor(
                        cdk_common::grpc::VERSION_HEADER,
                        cdk_common::PAYMENT_PROCESSOR_PROTOCOL_VERSION,
                    ),
                ))
            }
        };

        let listener = TcpListener::bind(self.socket_addr).await?;
        self.socket_addr = listener.local_addr()?;
        let incoming = TcpListenerStream::new(listener);

        self.service.start().await?;
        self.backend_started = true;

        let shutdown = CancellationToken::new();
        let shutdown_future = shutdown.clone().cancelled_owned();
        self.shutdown = Some(shutdown);
        self.handle = Some(tokio::spawn(async move {
            server
                .serve_with_incoming_shutdown(incoming, shutdown_future)
                .await?;
            Ok(())
        }));

        Ok(())
    }

    /// Stop the payment processor gRPC server.
    pub async fn stop(&mut self) -> anyhow::Result<()> {
        const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

        self.service.cancel_payment_event_stream();

        if let Some(shutdown) = self.shutdown.take() {
            tracing::info!("Initiating server shutdown");
            shutdown.cancel();
        }

        let server_result = match self.handle.take() {
            Some(mut handle) => match timeout(SHUTDOWN_TIMEOUT, &mut handle).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(error.into()),
                Err(_) => {
                    tracing::error!(
                        "Server shutdown timed out after {} seconds, aborting handle",
                        SHUTDOWN_TIMEOUT.as_secs()
                    );
                    handle.abort();
                    let _ = handle.await;
                    Err(anyhow::anyhow!(
                        "Payment processor server shutdown timed out"
                    ))
                }
            },
            None => Ok(()),
        };

        let backend_result = if self.backend_started {
            self.backend_started = false;
            self.service.stop().await.map_err(anyhow::Error::from)
        } else {
            Ok(())
        };

        backend_result?;
        server_result
    }
}

impl Drop for PaymentProcessorServer {
    fn drop(&mut self) {
        tracing::debug!("Dropping payment processor server");
        self.service.cancel_payment_event_stream();
        if let Some(shutdown) = &self.shutdown {
            shutdown.cancel();
        }
        if let Some(handle) = &self.handle {
            handle.abort();
        }
        if self.backend_started {
            tracing::warn!(
                "Payment processor server dropped while running; backend stop was not awaited"
            );
        }
    }
}

#[async_trait]
impl CdkPaymentProcessor for PaymentProcessorService {
    async fn get_settings(
        &self,
        _request: Request<EmptyRequest>,
    ) -> Result<Response<SettingsResponse>, Status> {
        let settings = self
            .inner
            .get_settings()
            .await
            .map_err(payment_error_to_status)?;

        Ok(Response::new(SettingsResponse {
            unit: settings.unit,
            bolt11: settings.bolt11.map(|b| super::Bolt11Settings {
                mpp: b.mpp,
                amountless: b.amountless,
                invoice_description: b.invoice_description,
            }),
            bolt12: settings.bolt12.map(|b| super::Bolt12Settings {
                amountless: b.amountless,
            }),
            onchain: settings.onchain.map(|o| super::OnchainSettings {
                confirmations: o.confirmations,
                min_receive_amount_sat: o.min_receive_amount_sat,
                min_send_amount_sat: o.min_send_amount_sat,
            }),
            custom: settings.custom,
        }))
    }

    async fn create_payment(
        &self,
        request: Request<CreatePaymentRequest>,
    ) -> Result<Response<CreatePaymentResponse>, Status> {
        let CreatePaymentRequest { options, .. } = request.into_inner();

        let options = options.ok_or_else(|| Status::invalid_argument("Missing payment options"))?;

        let proto_options = match options
            .options
            .ok_or_else(|| Status::invalid_argument("Missing options"))?
        {
            incoming_payment_options::Options::Custom(opts) => {
                let amount: Option<cdk_common::Amount<CurrencyUnit>> = match opts.amount {
                    Some(a) => Some(
                        a.try_into()
                            .map_err(|_| Status::invalid_argument("Invalid amount"))?,
                    ),
                    None => None,
                };
                IncomingPaymentOptions::Custom(Box::new(
                    cdk_common::payment::CustomIncomingPaymentOptions {
                        method: opts.method.unwrap_or_default(),
                        description: opts.description,
                        amount,
                        unix_expiry: opts.unix_expiry,
                        extra_json: opts.extra_json,
                    },
                ))
            }
            incoming_payment_options::Options::Bolt11(opts) => {
                let amount = opts
                    .amount
                    .ok_or_else(|| Status::invalid_argument("Missing amount"))?
                    .try_into()
                    .map_err(|_| Status::invalid_argument("Invalid amount"))?;
                IncomingPaymentOptions::Bolt11(cdk_common::payment::Bolt11IncomingPaymentOptions {
                    description: opts.description,
                    amount,
                    unix_expiry: opts.unix_expiry,
                })
            }
            incoming_payment_options::Options::Bolt12(opts) => {
                let amount: Option<cdk_common::Amount<CurrencyUnit>> = match opts.amount {
                    Some(a) => Some(
                        a.try_into()
                            .map_err(|_| Status::invalid_argument("Invalid amount"))?,
                    ),
                    None => None,
                };
                IncomingPaymentOptions::Bolt12(Box::new(
                    cdk_common::payment::Bolt12IncomingPaymentOptions {
                        description: opts.description,
                        amount,
                        unix_expiry: opts.unix_expiry,
                    },
                ))
            }
            incoming_payment_options::Options::Onchain(opts) => IncomingPaymentOptions::Onchain(
                cdk_common::payment::OnchainIncomingPaymentOptions {
                    quote_id: opts.quote_id.parse().map_err(|_| {
                        Status::invalid_argument("Invalid quote_id in Onchain options")
                    })?,
                },
            ),
        };

        let invoice_response = self
            .inner
            .create_incoming_payment_request(proto_options)
            .await
            .map_err(payment_error_to_status)?;

        Ok(Response::new(invoice_response.into()))
    }

    async fn get_payment_quote(
        &self,
        request: Request<PaymentQuoteRequest>,
    ) -> Result<Response<PaymentQuoteResponse>, Status> {
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid currency unit"))?;

        let quote_id = parse_quote_id(&request.quote_id)?;

        let options = match request.request_type() {
            OutgoingPaymentRequestType::Bolt11Invoice => {
                let bolt11: cdk_common::Bolt11Invoice =
                    request.request.parse().map_err(Error::Invoice)?;

                cdk_common::payment::OutgoingPaymentOptions::Bolt11(Box::new(
                    cdk_common::payment::Bolt11OutgoingPaymentOptions {
                        bolt11,
                        max_fee_amount: None,
                        timeout_secs: None,
                        melt_options: request.options.map(TryInto::try_into).transpose()?,
                        quote_id,
                    },
                ))
            }
            OutgoingPaymentRequestType::Bolt12Offer => {
                // Parse offer to verify it's valid, but store as string
                let _: Offer = request.request.parse().map_err(|_| Error::Bolt12Parse)?;

                cdk_common::payment::OutgoingPaymentOptions::Bolt12(Box::new(
                    cdk_common::payment::Bolt12OutgoingPaymentOptions {
                        offer: Offer::from_str(&request.request)
                            .expect("Already validated offer above"),
                        max_fee_amount: None,
                        timeout_secs: None,
                        melt_options: request.options.map(TryInto::try_into).transpose()?,
                        quote_id,
                    },
                ))
            }
            OutgoingPaymentRequestType::Custom => {
                let amount = request
                    .amount
                    .try_from_proto()
                    .map_err(|_| Status::invalid_argument("Invalid amount"))?;

                // Custom payment method - pass request as-is with no validation
                cdk_common::payment::OutgoingPaymentOptions::Custom(Box::new(
                    cdk_common::payment::CustomOutgoingPaymentOptions {
                        method: request.custom_method.clone().unwrap_or_default(),
                        request: request.request.clone(),
                        amount,
                        max_fee_amount: None,
                        timeout_secs: None,
                        melt_options: request.options.map(TryInto::try_into).transpose()?,
                        extra_json: request.extra_json.clone(),
                        quote_id,
                    },
                ))
            }
            OutgoingPaymentRequestType::Onchain => {
                let opts = request.onchain_options.ok_or_else(|| {
                    Status::invalid_argument("Missing onchain_options for onchain quote")
                })?;
                let amount = opts
                    .amount
                    .ok_or_else(|| Status::invalid_argument("Missing amount in onchain quote"))?
                    .try_into()
                    .map_err(|_| Status::invalid_argument("Invalid amount"))?;
                let max_fee_amount = opts
                    .max_fee_amount
                    .try_from_proto()
                    .map_err(|_| Status::invalid_argument("Invalid max_fee_amount"))?;
                let onchain_quote_id = parse_quote_id(&opts.quote_id)?;
                if onchain_quote_id != quote_id {
                    return Err(Status::invalid_argument(
                        "quote_id does not match onchain_options quote_id",
                    ));
                }

                cdk_common::payment::OutgoingPaymentOptions::Onchain(Box::new(
                    cdk_common::payment::OnchainOutgoingPaymentOptions {
                        address: opts.address,
                        amount,
                        max_fee_amount,
                        quote_id,
                        fee_index: opts.fee_index,
                        metadata: opts.metadata,
                    },
                ))
            }
            OutgoingPaymentRequestType::Unspecified => {
                return Err(Status::invalid_argument("Unspecified payment request type"));
            }
        };

        let payment_quote = self
            .inner
            .get_payment_quote(&unit, options)
            .await
            .map_err(|err| {
                tracing::error!("Could not get payment quote: {}", err);
                payment_error_to_status(err)
            })?;

        Ok(Response::new(payment_quote.into()))
    }

    async fn make_payment(
        &self,
        request: Request<MakePaymentRequest>,
    ) -> Result<Response<MakePaymentResponse>, Status> {
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid currency unit"))?;

        let options = request
            .payment_options
            .ok_or_else(|| Status::invalid_argument("Missing payment options"))?;

        let payment_options = match options
            .options
            .ok_or_else(|| Status::invalid_argument("Missing options"))?
        {
            outgoing_payment_variant::Options::Bolt11(opts) => {
                let bolt11: cdk_common::Bolt11Invoice =
                    opts.bolt11.parse().map_err(Error::Invoice)?;

                let max_fee_amount = opts
                    .max_fee_amount
                    .try_from_proto()
                    .map_err(|_| Status::invalid_argument("Invalid max_fee_amount"))?;
                let quote_id = parse_quote_id(&opts.quote_id)?;

                cdk_common::payment::OutgoingPaymentOptions::Bolt11(Box::new(
                    cdk_common::payment::Bolt11OutgoingPaymentOptions {
                        bolt11,
                        max_fee_amount,
                        timeout_secs: opts.timeout_secs,
                        melt_options: opts.melt_options.map(TryInto::try_into).transpose()?,
                        quote_id,
                    },
                ))
            }
            outgoing_payment_variant::Options::Bolt12(opts) => {
                let offer = Offer::from_str(&opts.offer).map_err(|_| Error::Bolt12Parse)?;

                let max_fee_amount = opts
                    .max_fee_amount
                    .try_from_proto()
                    .map_err(|_| Status::invalid_argument("Invalid max_fee_amount"))?;
                let quote_id = parse_quote_id(&opts.quote_id)?;

                cdk_common::payment::OutgoingPaymentOptions::Bolt12(Box::new(
                    cdk_common::payment::Bolt12OutgoingPaymentOptions {
                        offer,
                        max_fee_amount,
                        timeout_secs: opts.timeout_secs,
                        melt_options: opts.melt_options.map(TryInto::try_into).transpose()?,
                        quote_id,
                    },
                ))
            }
            outgoing_payment_variant::Options::Custom(opts) => {
                let max_fee_amount = opts
                    .max_fee_amount
                    .try_from_proto()
                    .map_err(|_| Status::invalid_argument("Invalid max_fee_amount"))?;
                let quote_id = parse_quote_id(&opts.quote_id)?;
                let amount: Option<cdk_common::Amount<CurrencyUnit>> = match opts.amount {
                    Some(a) => Some(
                        a.try_into()
                            .map_err(|_| Status::invalid_argument("Invalid amount"))?,
                    ),
                    None => None,
                };

                cdk_common::payment::OutgoingPaymentOptions::Custom(Box::new(
                    cdk_common::payment::CustomOutgoingPaymentOptions {
                        method: opts.method.unwrap_or_default(),
                        request: opts.offer, // Reusing offer field for custom request string
                        amount,
                        max_fee_amount,
                        timeout_secs: opts.timeout_secs,
                        melt_options: opts.melt_options.map(TryInto::try_into).transpose()?,
                        extra_json: opts.extra_json,
                        quote_id,
                    },
                ))
            }
            outgoing_payment_variant::Options::Onchain(opts) => {
                let amount = opts
                    .amount
                    .ok_or_else(|| Status::invalid_argument("Missing amount"))?
                    .try_into()
                    .map_err(|_| Status::invalid_argument("Invalid amount"))?;

                let max_fee_amount = opts
                    .max_fee_amount
                    .try_from_proto()
                    .map_err(|_| Status::invalid_argument("Invalid max_fee_amount"))?;

                cdk_common::payment::OutgoingPaymentOptions::Onchain(Box::new(
                    cdk_common::payment::OnchainOutgoingPaymentOptions {
                        address: opts.address,
                        amount,
                        max_fee_amount,
                        quote_id: opts.quote_id.parse().map_err(|_| {
                            Status::invalid_argument("Invalid quote_id in Onchain options")
                        })?,
                        fee_index: opts.fee_index,
                        metadata: opts.metadata,
                    },
                ))
            }
        };

        let pay_response = self
            .inner
            .make_payment(&unit, payment_options)
            .await
            .map_err(|err| {
                tracing::error!("Could not make payment: {}", err);
                payment_error_to_status(err)
            })?;

        Ok(Response::new(pay_response.into()))
    }

    async fn check_incoming_payment(
        &self,
        request: Request<CheckIncomingPaymentRequest>,
    ) -> Result<Response<CheckIncomingPaymentResponse>, Status> {
        let request = request.into_inner();

        let payment_identifier = request
            .request_identifier
            .ok_or_else(|| Status::invalid_argument("Missing request identifier"))?
            .try_into()
            .map_err(|_| Status::invalid_argument("Invalid request identifier"))?;

        let check_responses = self
            .inner
            .check_incoming_payment_status(&payment_identifier)
            .await
            .map_err(payment_error_to_status)?;

        Ok(Response::new(CheckIncomingPaymentResponse {
            payments: check_responses.into_iter().map(|r| r.into()).collect(),
        }))
    }

    async fn check_outgoing_payment(
        &self,
        request: Request<CheckOutgoingPaymentRequest>,
    ) -> Result<Response<MakePaymentResponse>, Status> {
        let request = request.into_inner();

        let payment_identifier = request
            .request_identifier
            .ok_or_else(|| Status::invalid_argument("Missing request identifier"))?
            .try_into()
            .map_err(|_| Status::invalid_argument("Invalid request identifier"))?;

        let check_response = self
            .inner
            .check_outgoing_payment(&payment_identifier)
            .await
            .map_err(payment_error_to_status)?;

        Ok(Response::new(check_response.into()))
    }

    type WaitPaymentEventStream = ResponseStream;

    #[allow(clippy::incompatible_msrv)]
    #[instrument(skip_all)]
    async fn wait_payment_event(
        &self,
        _request: Request<EmptyRequest>,
    ) -> Result<Response<Self::WaitPaymentEventStream>, Status> {
        tracing::debug!("Server waiting for payment stream");
        let stream = self.inner.wait_payment_event().await.map_err(|err| {
            tracing::warn!("Could not get payment event stream: {}", err);
            payment_error_to_status(err)
        })?;
        let output_stream = PaymentEventStream {
            inner: stream,
            payment_processor: self.inner.clone(),
            completed: false,
        };

        Ok(Response::new(Box::pin(output_stream)))
    }
}

fn parse_quote_id(s: &str) -> Result<QuoteId, Status> {
    s.parse()
        .map_err(|err| Status::invalid_argument(format!("Invalid quote_id: {err}")))
}
