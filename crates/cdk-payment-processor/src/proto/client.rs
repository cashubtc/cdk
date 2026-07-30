use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use anyhow::anyhow;
use cdk_common::grpc::{VersionInterceptor, VERSION_HEADER};
use cdk_common::payment::{
    CreateIncomingPaymentResponse, IncomingPaymentOptions as CdkIncomingPaymentOptions,
    MakePaymentResponse as CdkMakePaymentResponse, MintPayment,
    PaymentQuoteResponse as CdkPaymentQuoteResponse, WaitPaymentResponse,
};
use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tonic::codegen::InterceptedService;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Identity};
use tonic::{async_trait, Request};
use tracing::instrument;

use crate::error::payment_error_from_status;
use crate::proto::cdk_payment_processor_client::CdkPaymentProcessorClient;
use crate::proto::{
    CheckIncomingPaymentRequest, CheckOutgoingPaymentRequest, CreatePaymentRequest, EmptyRequest,
    IncomingPaymentOptions, IntoProtoAmount, MakePaymentRequest, OutgoingPaymentRequestType,
    PaymentQuoteRequest,
};

type RemotePaymentProcessorClient =
    CdkPaymentProcessorClient<InterceptedService<Channel, VersionInterceptor>>;

/// Remote gRPC payment processor adapter implementing [`MintPayment`].
#[derive(Clone)]
pub struct PaymentProcessorClient {
    inner: RemotePaymentProcessorClient,
    payment_event_stream_is_active: Arc<AtomicBool>,
    cancel_payment_event_stream: Arc<Mutex<CancellationToken>>,
}

struct ActivePaymentEventStream {
    inner: Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>,
    active_flag: Arc<AtomicBool>,
}

impl ActivePaymentEventStream {
    fn new(
        inner: Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>,
        active_flag: Arc<AtomicBool>,
    ) -> Self {
        Self { inner, active_flag }
    }
}

impl Drop for ActivePaymentEventStream {
    fn drop(&mut self) {
        self.active_flag.store(false, Ordering::SeqCst);
        tracing::info!("Payment event stream inactive");
    }
}

impl Stream for ActivePaymentEventStream {
    type Item = cdk_common::payment::Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.inner.as_mut().poll_next(cx)
    }
}

impl std::fmt::Debug for PaymentProcessorClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentProcessorClient")
            .finish_non_exhaustive()
    }
}

impl PaymentProcessorClient {
    /// Connect to a remote payment processor.
    pub async fn new(addr: &str, port: u16, tls_dir: Option<PathBuf>) -> anyhow::Result<Self> {
        let scheme = if tls_dir.is_some() { "https" } else { "http" };
        let endpoint = format!("{scheme}://{addr}:{port}");

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
            Channel::from_shared(endpoint)?
                .tls_config(tls)?
                .connect()
                .await?
        } else {
            // No TLS directory, skip TLS configuration
            Channel::from_shared(endpoint)?.connect().await?
        };

        let interceptor = VersionInterceptor::new(
            VERSION_HEADER,
            cdk_common::PAYMENT_PROCESSOR_PROTOCOL_VERSION,
        );
        let client = CdkPaymentProcessorClient::with_interceptor(channel, interceptor);

        Ok(Self {
            inner: client,
            payment_event_stream_is_active: Arc::new(AtomicBool::new(false)),
            cancel_payment_event_stream: Arc::new(Mutex::new(CancellationToken::new())),
        })
    }
}

#[async_trait]
impl MintPayment for PaymentProcessorClient {
    type Err = cdk_common::payment::Error;

    async fn start(&self) -> Result<(), Self::Err> {
        // The remote server owns the backend lifecycle. The connection is
        // established by `new`, so there is no client-side startup work.
        Ok(())
    }

    async fn stop(&self) -> Result<(), Self::Err> {
        // Release any client-side streaming work. The remote server remains
        // responsible for stopping its backend.
        self.cancel_payment_event_stream();
        Ok(())
    }

    async fn get_settings(&self) -> Result<cdk_common::payment::SettingsResponse, Self::Err> {
        let mut inner = self.inner.clone();
        let response = inner
            .get_settings(Request::new(EmptyRequest {}))
            .await
            .map_err(|err| {
                tracing::error!("Could not get settings: {}", err);
                payment_error_from_status(err)
            })?;
        let settings = response.into_inner();

        Ok(cdk_common::payment::SettingsResponse {
            unit: settings.unit,
            bolt11: settings
                .bolt11
                .map(|b| cdk_common::payment::Bolt11Settings {
                    mpp: b.mpp,
                    amountless: b.amountless,
                    invoice_description: b.invoice_description,
                }),
            bolt12: settings
                .bolt12
                .map(|b| cdk_common::payment::Bolt12Settings {
                    amountless: b.amountless,
                }),
            onchain: settings
                .onchain
                .map(|o| cdk_common::payment::OnchainSettings {
                    confirmations: o.confirmations,
                    min_receive_amount_sat: o.min_receive_amount_sat,
                    min_send_amount_sat: o.min_send_amount_sat,
                }),
            custom: settings.custom,
        })
    }

    /// Create a new invoice
    async fn create_incoming_payment_request(
        &self,
        options: CdkIncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let proto_options = match options {
            CdkIncomingPaymentOptions::Custom(opts) => IncomingPaymentOptions {
                options: Some(super::incoming_payment_options::Options::Custom(
                    super::CustomIncomingPaymentOptions {
                        description: opts.description,
                        amount: opts.amount.map(Into::into),
                        unix_expiry: opts.unix_expiry,
                        extra_json: opts.extra_json.clone(),
                        method: Some(opts.method.clone()),
                    },
                )),
            },
            CdkIncomingPaymentOptions::Bolt11(opts) => IncomingPaymentOptions {
                options: Some(super::incoming_payment_options::Options::Bolt11(
                    super::Bolt11IncomingPaymentOptions {
                        description: opts.description,
                        amount: Some(opts.amount.into()),
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
            CdkIncomingPaymentOptions::Onchain(opts) => IncomingPaymentOptions {
                options: Some(super::incoming_payment_options::Options::Onchain(
                    super::OnchainIncomingPaymentOptions {
                        quote_id: opts.quote_id.to_string(),
                    },
                )),
            },
        };

        let mut inner = self.inner.clone();
        let response = inner
            .create_payment(Request::new(CreatePaymentRequest {
                options: Some(proto_options),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not create payment request: {}", err);
                payment_error_from_status(err)
            })?
            .into_inner();

        Ok(response.try_into().map_err(|_| {
            cdk_common::payment::Error::Anyhow(anyhow!("Could not create create payment response"))
        })?)
    }

    async fn get_payment_quote(
        &self,
        unit: &cdk_common::CurrencyUnit,
        options: cdk_common::payment::OutgoingPaymentOptions,
    ) -> Result<CdkPaymentQuoteResponse, Self::Err> {
        let request_type = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(_) => {
                OutgoingPaymentRequestType::Custom
            }
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(_) => {
                OutgoingPaymentRequestType::Bolt11Invoice
            }
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(_) => {
                OutgoingPaymentRequestType::Bolt12Offer
            }
            cdk_common::payment::OutgoingPaymentOptions::Onchain(_) => {
                OutgoingPaymentRequestType::Onchain
            }
        };

        let proto_request = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => opts.request.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => opts.bolt11.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => opts.offer.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Onchain(opts) => opts.address.clone(),
        };

        let proto_options = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => opts.melt_options,
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => opts.melt_options,
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => opts.melt_options,
            cdk_common::payment::OutgoingPaymentOptions::Onchain(_) => None,
        };

        let onchain_options = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Onchain(opts) => {
                Some(super::OnchainOutgoingPaymentOptions {
                    address: opts.address.clone(),
                    amount: Some(opts.amount.clone().into()),
                    max_fee_amount: opts.max_fee_amount.clone().into_proto(),
                    quote_id: opts.quote_id.to_string(),
                    fee_index: opts.fee_index,
                    metadata: opts.metadata.clone(),
                })
            }
            _ => None,
        };

        let extra_json = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => opts.extra_json.clone(),
            _ => None,
        };

        let custom_method = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => Some(opts.method.clone()),
            _ => None,
        };

        let amount = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => {
                opts.amount.clone().into_proto()
            }
            _ => None,
        };

        let quote_id = match &options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => opts.quote_id.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => opts.quote_id.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => opts.quote_id.to_string(),
            cdk_common::payment::OutgoingPaymentOptions::Onchain(opts) => opts.quote_id.to_string(),
        };

        let mut inner = self.inner.clone();
        let response = inner
            .get_payment_quote(Request::new(PaymentQuoteRequest {
                request: proto_request,
                unit: unit.to_string(),
                options: proto_options.map(Into::into),
                request_type: request_type.into(),
                extra_json,
                quote_id,
                onchain_options,
                amount,
                custom_method,
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not get payment quote: {}", err);
                payment_error_from_status(err)
            })?
            .into_inner();

        Ok(response.try_into().map_err(|_| {
            cdk_common::payment::Error::Custom(
                "Failed to convert payment quote response".to_string(),
            )
        })?)
    }

    async fn make_payment(
        &self,
        unit: &cdk_common::CurrencyUnit,
        options: cdk_common::payment::OutgoingPaymentOptions,
    ) -> Result<CdkMakePaymentResponse, Self::Err> {
        let payment_options = match options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(opts) => {
                super::OutgoingPaymentVariant {
                    options: Some(super::outgoing_payment_variant::Options::Custom(
                        super::CustomOutgoingPaymentOptions {
                            offer: opts.request.to_string(),
                            amount: opts.amount.map(Into::into),
                            max_fee_amount: opts.max_fee_amount.into_proto(),
                            timeout_secs: opts.timeout_secs,
                            melt_options: opts.melt_options.map(Into::into),
                            extra_json: opts.extra_json.clone(),
                            quote_id: opts.quote_id.to_string(),
                            method: Some(opts.method.clone()),
                        },
                    )),
                }
            }
            cdk_common::payment::OutgoingPaymentOptions::Bolt11(opts) => {
                super::OutgoingPaymentVariant {
                    options: Some(super::outgoing_payment_variant::Options::Bolt11(
                        super::Bolt11OutgoingPaymentOptions {
                            bolt11: opts.bolt11.to_string(),
                            max_fee_amount: opts.max_fee_amount.into_proto(),
                            timeout_secs: opts.timeout_secs,
                            melt_options: opts.melt_options.map(Into::into),
                            quote_id: opts.quote_id.to_string(),
                        },
                    )),
                }
            }
            cdk_common::payment::OutgoingPaymentOptions::Bolt12(opts) => {
                super::OutgoingPaymentVariant {
                    options: Some(super::outgoing_payment_variant::Options::Bolt12(
                        super::Bolt12OutgoingPaymentOptions {
                            offer: opts.offer.to_string(),
                            max_fee_amount: opts.max_fee_amount.into_proto(),
                            timeout_secs: opts.timeout_secs,
                            melt_options: opts.melt_options.map(Into::into),
                            quote_id: opts.quote_id.to_string(),
                        },
                    )),
                }
            }
            cdk_common::payment::OutgoingPaymentOptions::Onchain(opts) => {
                super::OutgoingPaymentVariant {
                    options: Some(super::outgoing_payment_variant::Options::Onchain(
                        super::OnchainOutgoingPaymentOptions {
                            address: opts.address.clone(),
                            amount: Some(opts.amount.into()),
                            max_fee_amount: opts.max_fee_amount.into_proto(),
                            quote_id: opts.quote_id.to_string(),
                            fee_index: opts.fee_index,
                            metadata: opts.metadata.clone(),
                        },
                    )),
                }
            }
        };

        let mut inner = self.inner.clone();
        let response = inner
            .make_payment(Request::new(MakePaymentRequest {
                payment_options: Some(payment_options),
                partial_amount: None,
                max_fee_amount: None,
                unit: unit.to_string(),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not pay payment request: {}", err);
                payment_error_from_status(err)
            })?
            .into_inner();

        Ok(response.try_into().map_err(|_err| {
            cdk_common::payment::Error::Anyhow(anyhow!("could not make payment"))
        })?)
    }

    #[instrument(skip_all)]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = cdk_common::payment::Event> + Send>>, Self::Err> {
        tracing::debug!("Client waiting for payment");
        let mut inner = self.inner.clone();
        let stream = inner
            .wait_payment_event(Request::new(EmptyRequest {}))
            .await
            .map_err(|err| {
                self.payment_event_stream_is_active
                    .store(false, Ordering::SeqCst);
                tracing::error!("Could not open payment event stream: {}", err);
                payment_error_from_status(err)
            })?
            .into_inner();

        self.payment_event_stream_is_active
            .store(true, Ordering::SeqCst);

        let cancel_token = self
            .cancel_payment_event_stream
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let cancel_fut = cancel_token.cancelled_owned();
        let active_flag = self.payment_event_stream_is_active.clone();

        let transformed_stream = stream.take_until(cancel_fut).filter_map(|item| async {
            match item {
                Ok(value) => match value.try_into() {
                    Ok(payment_event) => Some(payment_event),
                    Err(e) => {
                        tracing::error!("Error converting payment event: {}", e);
                        None
                    }
                },
                Err(e) => {
                    tracing::error!("Error in payment event stream: {}", e);
                    None
                }
            }
        });

        Ok(Box::pin(ActivePaymentEventStream::new(
            Box::pin(transformed_stream),
            active_flag,
        )))
    }

    /// Is payment event stream active
    fn is_payment_event_stream_active(&self) -> bool {
        self.payment_event_stream_is_active.load(Ordering::SeqCst)
    }

    /// Cancel payment event stream
    fn cancel_payment_event_stream(&self) {
        let mut cancel_token = self
            .cancel_payment_event_stream
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancel_token.cancel();
        *cancel_token = CancellationToken::new();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &cdk_common::payment::PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let mut inner = self.inner.clone();
        let check_incoming = inner
            .check_incoming_payment(Request::new(CheckIncomingPaymentRequest {
                request_identifier: Some(payment_identifier.clone().into()),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check incoming payment: {}", err);
                payment_error_from_status(err)
            })?
            .into_inner();

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
        let check_outgoing = inner
            .check_outgoing_payment(Request::new(CheckOutgoingPaymentRequest {
                request_identifier: Some(payment_identifier.clone().into()),
            }))
            .await
            .map_err(|err| {
                tracing::error!("Could not check outgoing payment: {}", err);
                payment_error_from_status(err)
            })?
            .into_inner();

        Ok(check_outgoing
            .try_into()
            .map_err(|_| cdk_common::payment::Error::UnknownPaymentState)?)
    }
}

#[cfg(all(test, feature = "fake"))]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicUsize;

    use cdk_common::common::FeeReserve;
    use cdk_common::payment::{
        CustomIncomingPaymentOptions, CustomOutgoingPaymentOptions, DynMintPayment, Event,
        IncomingPaymentOptions, OutgoingPaymentOptions, PaymentIdentifier,
    };
    use cdk_common::{Amount, CurrencyUnit, QuoteId};
    use cdk_fake_wallet::FakeWallet;

    use super::*;

    struct RecordingBackend {
        inner: FakeWallet,
        starts: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
    }

    fn recording_backend(starts: Arc<AtomicUsize>, stops: Arc<AtomicUsize>) -> DynMintPayment {
        let wallet = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 0.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::new(),
            HashSet::new(),
            0,
            CurrencyUnit::Sat,
        )
        .with_custom_payment_methods(HashMap::from([("venmo".to_string(), "{}".to_string())]));

        Arc::new(RecordingBackend {
            inner: wallet,
            starts,
            stops,
        })
    }

    fn custom_options(method: &str) -> OutgoingPaymentOptions {
        OutgoingPaymentOptions::Custom(Box::new(CustomOutgoingPaymentOptions {
            method: method.to_owned(),
            request: "venmo-payment-request".to_string(),
            amount: Some(Amount::new(20, CurrencyUnit::Sat)),
            max_fee_amount: None,
            timeout_secs: None,
            melt_options: None,
            extra_json: Some(r#"{"recipient":"alice"}"#.to_string()),
            quote_id: QuoteId::new(),
        }))
    }

    #[async_trait]
    impl MintPayment for RecordingBackend {
        type Err = cdk_common::payment::Error;

        async fn start(&self) -> Result<(), Self::Err> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn stop(&self) -> Result<(), Self::Err> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn get_settings(&self) -> Result<cdk_common::payment::SettingsResponse, Self::Err> {
            self.inner.get_settings().await
        }

        async fn create_incoming_payment_request(
            &self,
            options: IncomingPaymentOptions,
        ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
            self.inner.create_incoming_payment_request(options).await
        }

        async fn get_payment_quote(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<CdkPaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<CdkMakePaymentResponse, Self::Err> {
            self.inner.make_payment(unit, options).await
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            self.inner.wait_payment_event().await
        }

        fn is_payment_event_stream_active(&self) -> bool {
            self.inner.is_payment_event_stream_active()
        }

        fn cancel_payment_event_stream(&self) {
            self.inner.cancel_payment_event_stream();
        }

        async fn check_incoming_payment_status(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
            self.inner
                .check_incoming_payment_status(payment_identifier)
                .await
        }

        async fn check_outgoing_payment(
            &self,
            payment_identifier: &PaymentIdentifier,
        ) -> Result<CdkMakePaymentResponse, Self::Err> {
            self.inner.check_outgoing_payment(payment_identifier).await
        }
    }

    #[tokio::test]
    async fn remote_adapter_preserves_mint_payment_contract() {
        let remote_starts = Arc::new(AtomicUsize::new(0));
        let remote_stops = Arc::new(AtomicUsize::new(0));
        let server = super::super::PaymentProcessorServer::new(
            recording_backend(remote_starts.clone(), remote_stops.clone()),
            "127.0.0.1",
            0,
        )
        .expect("server should be constructed");
        let server_clone = server.clone();
        server
            .start(None)
            .await
            .expect("remote backend should start");
        assert_eq!(remote_starts.load(Ordering::SeqCst), 1);

        let second_start = server.start(None).await;
        assert!(second_start.is_err(), "double start should be rejected");
        assert_eq!(remote_starts.load(Ordering::SeqCst), 1);

        let remote = PaymentProcessorClient::new(
            &server.local_addr().ip().to_string(),
            server.local_addr().port(),
            None,
        )
        .await
        .expect("remote client should connect");

        remote
            .start()
            .await
            .expect("remote adapter start should succeed");
        assert_eq!(
            remote_starts.load(Ordering::SeqCst),
            1,
            "the remote server owns its backend lifecycle"
        );

        let remote_settings = remote.get_settings().await.expect("remote settings");
        assert!(remote_settings.custom.contains_key("venmo"));

        let mut stream = remote
            .wait_payment_event()
            .await
            .expect("remote payment stream should open");
        assert!(remote.is_payment_event_stream_active());

        let incoming = remote
            .create_incoming_payment_request(IncomingPaymentOptions::Custom(Box::new(
                CustomIncomingPaymentOptions {
                    method: "venmo".to_string(),
                    description: None,
                    amount: Some(Amount::new(20, CurrencyUnit::Sat)),
                    unix_expiry: None,
                    extra_json: None,
                },
            )))
            .await
            .expect("custom incoming method should survive gRPC");
        assert!(incoming.request.starts_with("venmo:"));

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("remote payment event should arrive")
            .expect("remote payment stream should remain open");
        assert!(matches!(event, Event::PaymentReceived(_)));

        remote.cancel_payment_event_stream();
        let end = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .expect("remote payment stream cancellation should complete");
        assert!(end.is_none());
        drop(stream);
        assert!(!remote.is_payment_event_stream_active());

        let remote_quote = remote
            .get_payment_quote(&CurrencyUnit::Sat, custom_options("venmo"))
            .await
            .expect("remote quote");
        assert_eq!(remote_quote.amount, Amount::new(20, CurrencyUnit::Sat));
        assert_eq!(
            remote_quote.extra_json,
            Some(serde_json::json!({ "recipient": "alice" }))
        );

        let payment = remote
            .make_payment(&CurrencyUnit::Sat, custom_options("venmo"))
            .await
            .expect("custom payment method should survive gRPC");
        assert_eq!(
            payment.payment_proof,
            Some("venmo-payment-request".to_string())
        );

        let remote_error = remote
            .get_payment_quote(&CurrencyUnit::Sat, custom_options("unsupported"))
            .await
            .expect_err("remote unsupported method should fail");
        assert!(matches!(
            remote_error,
            cdk_common::payment::Error::UnsupportedPaymentOption
        ));

        remote
            .stop()
            .await
            .expect("remote adapter stop should succeed");
        assert_eq!(
            remote_stops.load(Ordering::SeqCst),
            0,
            "the remote server owns its backend lifecycle"
        );

        server_clone
            .stop()
            .await
            .expect("a cloned server handle should stop the remote backend");
        assert_eq!(remote_stops.load(Ordering::SeqCst), 1);

        server
            .stop()
            .await
            .expect("repeated server stop should be harmless");
        assert_eq!(remote_stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn server_bind_failure_does_not_start_backend() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener address");
        let starts = Arc::new(AtomicUsize::new(0));
        let stops = Arc::new(AtomicUsize::new(0));
        let server = super::super::PaymentProcessorServer::new(
            recording_backend(starts.clone(), stops.clone()),
            &address.ip().to_string(),
            address.port(),
        )
        .expect("server should be constructed");

        server
            .start(None)
            .await
            .expect_err("occupied address should fail");
        assert_eq!(starts.load(Ordering::SeqCst), 0);

        server
            .stop()
            .await
            .expect("stopping a server that never started should succeed");
        assert_eq!(stops.load(Ordering::SeqCst), 0);
    }
}
