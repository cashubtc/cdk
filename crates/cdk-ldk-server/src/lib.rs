//! CDK Lightning backend for LDK Server.

#![doc = include_str!("../README.md")]

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::hashes::Hash;
use cdk_common::amount::{Amount, MSAT_IN_SAT};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::util::{hex, unix_time};
use futures::Stream;
use ldk_server_client::client::{EventStream, LdkServerClient};
use ldk_server_client::ldk_server_grpc::api::{
    Bolt11ReceiveRequest, Bolt11SendRequest, Bolt12ReceiveRequest, Bolt12SendRequest,
    GetNodeInfoRequest, GetPaymentDetailsRequest, ListPaymentsRequest,
};
use ldk_server_client::ldk_server_grpc::events::{event_envelope, EventEnvelope};
use ldk_server_client::ldk_server_grpc::types::{
    bolt11_invoice_description, payment_kind, Bolt11InvoiceDescription, PageToken, Payment,
    PaymentDirection, PaymentStatus, RouteParametersConfig,
};
use lightning::offers::offer::Amount as OfferAmount;
use tokio_util::sync::CancellationToken;

pub mod error;

use self::error::Error;

const DEFAULT_INVOICE_EXPIRY_SECS: u32 = 36_000;
const DEFAULT_PAYMENT_WAIT_SECS: u64 = 10;
const DEFAULT_MAX_PAYMENT_SCAN_PAGES: u16 = 32;
const ROUTE_DEFAULT_MAX_TOTAL_CLTV_EXPIRY_DELTA: u32 = 1008;
const ROUTE_DEFAULT_MAX_PATH_COUNT: u32 = 10;
const ROUTE_DEFAULT_MAX_CHANNEL_SATURATION_POWER_OF_HALF: u32 = 2;

/// LDK Server backend configuration.
#[derive(Clone)]
pub struct Config {
    /// Server address without scheme, for example `127.0.0.1:3536`.
    pub address: String,
    /// HMAC API key expected by LDK Server.
    pub api_key: String,
    /// PEM encoded TLS certificate to pin.
    pub cert_pem: Vec<u8>,
    /// Fee reserve used for melt quotes.
    pub fee_reserve: FeeReserve,
    /// Maximum `ListPayments` pages to scan for incoming status lookups.
    pub max_payment_scan_pages: u16,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("address", &self.address)
            .field("api_key", &"[REDACTED]")
            .field("cert_pem", &format!("{} bytes", self.cert_pem.len()))
            .field("fee_reserve", &self.fee_reserve)
            .field("max_payment_scan_pages", &self.max_payment_scan_pages)
            .finish()
    }
}

impl Config {
    /// Create a new LDK Server backend configuration.
    pub fn new(
        address: String,
        api_key: String,
        cert_pem: Vec<u8>,
        fee_reserve: FeeReserve,
    ) -> Self {
        Self {
            address,
            api_key,
            cert_pem,
            fee_reserve,
            max_payment_scan_pages: DEFAULT_MAX_PAYMENT_SCAN_PAGES,
        }
    }

    /// Set the maximum number of payment-history pages scanned for incoming payment status.
    pub fn with_max_payment_scan_pages(mut self, max_payment_scan_pages: u16) -> Self {
        self.max_payment_scan_pages = max_payment_scan_pages;
        self
    }
}

/// LDK Server payment backend.
#[derive(Clone)]
pub struct CdkLdkServer {
    client: LdkServerClient,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    max_payment_scan_pages: u16,
}

impl std::fmt::Debug for CdkLdkServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdkLdkServer")
            .field("fee_reserve", &self.fee_reserve)
            .field("max_payment_scan_pages", &self.max_payment_scan_pages)
            .finish_non_exhaustive()
    }
}

impl CdkLdkServer {
    /// Create a new LDK Server backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the TLS certificate cannot be parsed or the client cannot be built.
    pub fn new(config: Config) -> Result<Self, Error> {
        let address = normalize_address(&config.address);
        let client = LdkServerClient::new(address, config.api_key, &config.cert_pem)
            .map_err(Error::Client)?;

        Ok(Self {
            client,
            fee_reserve: config.fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            max_payment_scan_pages: config.max_payment_scan_pages,
        })
    }

    async fn get_payment_details(&self, payment_id: &str) -> Result<Option<Payment>, Error> {
        let response = self
            .client
            .get_payment_details(GetPaymentDetailsRequest {
                payment_id: payment_id.to_string(),
            })
            .await?;

        Ok(response.payment)
    }

    async fn list_matching_payments<F>(&self, mut matches_payment: F) -> Result<Vec<Payment>, Error>
    where
        F: FnMut(&Payment) -> bool,
    {
        let mut page_token: Option<PageToken> = None;
        let mut payments = Vec::new();

        for _ in 0..self.max_payment_scan_pages {
            let response = self
                .client
                .list_payments(ListPaymentsRequest { page_token })
                .await?;

            payments.extend(response.payments.into_iter().filter(|p| matches_payment(p)));

            match response.next_page_token {
                Some(next_page_token) => page_token = Some(next_page_token),
                None => return Ok(payments),
            }
        }

        Err(Error::PaymentScanLimitExceeded {
            max_pages: self.max_payment_scan_pages,
        })
    }

    async fn wait_for_payment_details(
        &self,
        payment_id: &str,
        timeout_secs: Option<u64>,
    ) -> Result<Payment, Error> {
        let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_PAYMENT_WAIT_SECS));
        let start = std::time::Instant::now();

        loop {
            match self.get_payment_details(payment_id).await? {
                Some(payment) => return Ok(payment),
                None if start.elapsed() >= timeout => return Err(Error::PaymentNotFound),
                None => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }

    fn make_payment_response_from_payment(
        unit: &CurrencyUnit,
        payment_lookup_id: PaymentIdentifier,
        payment: &Payment,
    ) -> Result<MakePaymentResponse, payment::Error> {
        let status = payment_status(payment)?;
        let status = match status {
            PaymentStatus::Pending => MeltQuoteState::Pending,
            PaymentStatus::Succeeded => MeltQuoteState::Paid,
            PaymentStatus::Failed => MeltQuoteState::Failed,
        };

        let payment_proof = match payment_kind(payment)? {
            payment_kind::Kind::Bolt11(bolt11) => bolt11.preimage.clone(),
            payment_kind::Kind::Bolt12Offer(bolt12) => bolt12.preimage.clone(),
            _ => return Err(Error::UnexpectedPaymentKind.into()),
        };

        let total_spent = if status == MeltQuoteState::Paid {
            let total_spent = payment
                .amount_msat
                .ok_or(Error::CouldNotGetAmountSpent)?
                .checked_add(payment.fee_paid_msat.unwrap_or_default())
                .ok_or(Error::AmountOverflow)?;
            msat_total_spent_for_unit(total_spent, unit)?
        } else {
            Amount::new(0, unit.clone())
        };

        Ok(MakePaymentResponse {
            payment_lookup_id,
            payment_proof,
            status,
            total_spent,
        })
    }

    fn wait_payment_response_from_payment(
        payment: &Payment,
    ) -> Result<Option<WaitPaymentResponse>, Error> {
        if payment_direction(payment)? != PaymentDirection::Inbound {
            return Ok(None);
        }

        if payment_status(payment)? != PaymentStatus::Succeeded {
            return Ok(None);
        }

        let payment_amount = Amount::new(
            payment.amount_msat.ok_or(Error::CouldNotGetPaymentAmount)?,
            CurrencyUnit::Msat,
        );

        let (payment_identifier, payment_id) = match payment_kind(payment)? {
            payment_kind::Kind::Bolt11(bolt11) => (
                PaymentIdentifier::PaymentHash(hex_to_array(&bolt11.hash)?),
                bolt11.hash.clone(),
            ),
            payment_kind::Kind::Bolt12Offer(bolt12) => (
                PaymentIdentifier::OfferId(bolt12.offer_id.clone()),
                bolt12.hash.clone().unwrap_or_else(|| payment.id.clone()),
            ),
            _ => return Ok(None),
        };

        Ok(Some(WaitPaymentResponse {
            payment_identifier,
            payment_amount,
            payment_id,
        }))
    }

    fn event_from_envelope(envelope: EventEnvelope) -> Result<Option<Event>, Error> {
        let event = match envelope.event {
            Some(event) => event,
            None => return Ok(None),
        };

        match event {
            event_envelope::Event::PaymentReceived(payment_received) => {
                let payment = match payment_received.payment {
                    Some(payment) => payment,
                    None => return Ok(None),
                };

                Ok(Self::wait_payment_response_from_payment(&payment)?.map(Event::PaymentReceived))
            }
            event_envelope::Event::PaymentSuccessful(payment_successful) => {
                if let Some(payment) = payment_successful.payment {
                    tracing::debug!("LDK Server outgoing payment succeeded: {}", payment.id);
                }
                Ok(None)
            }
            event_envelope::Event::PaymentFailed(payment_failed) => {
                if let Some(payment) = payment_failed.payment {
                    tracing::warn!("LDK Server outgoing payment failed: {}", payment.id);
                }
                Ok(None)
            }
            event_envelope::Event::PaymentForwarded(_)
            | event_envelope::Event::PaymentClaimable(_) => Ok(None),
        }
    }
}

#[async_trait]
impl MintPayment for CdkLdkServer {
    type Err = payment::Error;

    async fn start(&self) -> Result<(), Self::Err> {
        self.client
            .get_node_info(GetNodeInfoRequest {})
            .await
            .map_err(Error::from)?;
        Ok(())
    }

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: CurrencyUnit::Msat.to_string(),
            bolt11: Some(payment::Bolt11Settings {
                mpp: false,
                amountless: true,
                invoice_description: true,
            }),
            bolt12: Some(payment::Bolt12Settings { amountless: true }),
            onchain: None,
            custom: std::collections::HashMap::new(),
        })
    }

    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = bolt11_options
                    .amount
                    .convert_to(&CurrencyUnit::Msat)?
                    .value();
                let expiry_secs = expiry_secs_or_default(bolt11_options.unix_expiry)?;
                let response = self
                    .client
                    .bolt11_receive(Bolt11ReceiveRequest {
                        amount_msat: Some(amount_msat),
                        description: Some(Bolt11InvoiceDescription {
                            kind: Some(bolt11_invoice_description::Kind::Direct(
                                bolt11_options.description.unwrap_or_default(),
                            )),
                        }),
                        expiry_secs,
                    })
                    .await
                    .map_err(Error::from)?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::PaymentHash(hex_to_array(
                        &response.payment_hash,
                    )?),
                    request: response.invoice,
                    expiry: Some(unix_time() + u64::from(expiry_secs)),
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Bolt12(bolt12_options) => {
                let amount_msat = bolt12_options
                    .amount
                    .map(|amount| amount.convert_to(&CurrencyUnit::Msat).map(|a| a.value()))
                    .transpose()?;
                let expiry_secs = bolt12_options.unix_expiry.map(expiry_secs).transpose()?;
                let response = self
                    .client
                    .bolt12_receive(Bolt12ReceiveRequest {
                        description: bolt12_options.description.unwrap_or_default(),
                        amount_msat,
                        expiry_secs,
                        quantity: None,
                    })
                    .await
                    .map_err(Error::from)?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::OfferId(response.offer_id),
                    request: response.offer,
                    expiry: bolt12_options.unix_expiry,
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Custom(_) | IncomingPaymentOptions::Onchain(_) => {
                Err(payment::Error::UnsupportedPaymentOption)
            }
        }
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = bolt11_amount_msat_for_quote(&bolt11_options)?;
                let amount = Amount::new(amount_msat, CurrencyUnit::Msat).convert_to(unit)?;
                let fee = self.fee_reserve_for_amount(unit, amount.value());

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_byte_array(),
                    )),
                    amount,
                    fee,
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
                })
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let amount_msat = bolt12_amount_msat_for_quote(&bolt12_options)?;
                let amount = Amount::new(amount_msat, CurrencyUnit::Msat).convert_to(unit)?;
                let fee = self.fee_reserve_for_amount(unit, amount.value());

                Ok(PaymentQuoteResponse {
                    request_lookup_id: None,
                    amount,
                    fee,
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
                })
            }
            OutgoingPaymentOptions::Custom(_) | OutgoingPaymentOptions::Onchain(_) => {
                Err(payment::Error::UnsupportedPaymentOption)
            }
        }
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = bolt11_amount_msat_for_send(&bolt11_options)?;
                let response = self
                    .client
                    .bolt11_send(Bolt11SendRequest {
                        invoice: bolt11_options.bolt11.to_string(),
                        amount_msat,
                        route_parameters: route_parameters_from_max_fee(
                            bolt11_options.max_fee_amount.clone(),
                        )?,
                    })
                    .await
                    .map_err(Error::from)?;
                let payment_id = hex_to_array(&response.payment_id)?;
                let payment = self
                    .wait_for_payment_details(&response.payment_id, bolt11_options.timeout_secs)
                    .await?;

                Self::make_payment_response_from_payment(
                    unit,
                    PaymentIdentifier::PaymentId(payment_id),
                    &payment,
                )
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let amount_msat = bolt12_amount_msat_for_send(&bolt12_options)?;
                let response = self
                    .client
                    .bolt12_send(Bolt12SendRequest {
                        offer: bolt12_options.offer.to_string(),
                        amount_msat,
                        quantity: None,
                        payer_note: None,
                        route_parameters: route_parameters_from_max_fee(
                            bolt12_options.max_fee_amount.clone(),
                        )?,
                    })
                    .await
                    .map_err(Error::from)?;
                let payment_id = hex_to_array(&response.payment_id)?;
                let payment = self
                    .wait_for_payment_details(&response.payment_id, bolt12_options.timeout_secs)
                    .await?;

                Self::make_payment_response_from_payment(
                    unit,
                    PaymentIdentifier::PaymentId(payment_id),
                    &payment,
                )
            }
            OutgoingPaymentOptions::Custom(_) | OutgoingPaymentOptions::Onchain(_) => {
                Err(payment::Error::UnsupportedPaymentOption)
            }
        }
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let state = EventStreamState {
            client: self.client.clone(),
            cancel_token: self.wait_invoice_cancel_token.clone(),
            is_active: Arc::clone(&self.wait_invoice_is_active),
            stream: None,
            retry_count: 0,
        };

        Ok(Box::pin(futures::stream::unfold(
            state,
            |mut state| async move {
                state.is_active.store(true, Ordering::SeqCst);

                loop {
                    if state.stream.is_none() {
                        match state.client.subscribe_events().await {
                            Ok(stream) => {
                                state.stream = Some(stream);
                                state.retry_count = 0;
                            }
                            Err(err) => {
                                tracing::warn!("Failed to subscribe to LDK Server events: {}", err);
                                if !sleep_or_cancel(&state.cancel_token, state.retry_delay()).await
                                {
                                    state.is_active.store(false, Ordering::SeqCst);
                                    return None;
                                }
                                state.retry_count = state.retry_count.saturating_add(1);
                                continue;
                            }
                        }
                    }

                    let mut stream = match state.stream.take() {
                        Some(stream) => stream,
                        None => continue,
                    };

                    tokio::select! {
                        _ = state.cancel_token.cancelled() => {
                            state.is_active.store(false, Ordering::SeqCst);
                            return None;
                        }
                        message = stream.next_message() => {
                            state.stream = Some(stream);

                            match message {
                                Some(Ok(envelope)) => match Self::event_from_envelope(envelope) {
                                    Ok(Some(event)) => return Some((event, state)),
                                    Ok(None) => continue,
                                    Err(err) => {
                                        tracing::warn!("Could not map LDK Server event: {}", err);
                                        continue;
                                    }
                                },
                                Some(Err(err)) => {
                                    tracing::warn!("LDK Server event stream error: {}", err);
                                    state.stream = None;
                                    if !sleep_or_cancel(&state.cancel_token, state.retry_delay()).await {
                                        state.is_active.store(false, Ordering::SeqCst);
                                        return None;
                                    }
                                    state.retry_count = state.retry_count.saturating_add(1);
                                    continue;
                                }
                                None => {
                                    tracing::warn!("LDK Server event stream ended");
                                    state.stream = None;
                                    if !sleep_or_cancel(&state.cancel_token, state.retry_delay()).await {
                                        state.is_active.store(false, Ordering::SeqCst);
                                        return None;
                                    }
                                    state.retry_count = state.retry_count.saturating_add(1);
                                    continue;
                                }
                            }
                        }
                    }
                }
            },
        )))
    }

    fn is_payment_event_stream_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_payment_event_stream(&self) {
        self.wait_invoice_cancel_token.cancel();
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        match payment_identifier {
            PaymentIdentifier::OfferId(offer_id) => {
                let payments = self
                    .list_matching_payments(|payment| {
                        matches!(payment_direction(payment), Ok(PaymentDirection::Inbound))
                            && matches!(payment_status(payment), Ok(PaymentStatus::Succeeded))
                            && matches!(
                                payment_kind(payment),
                                Ok(payment_kind::Kind::Bolt12Offer(bolt12))
                                    if bolt12.offer_id == *offer_id
                            )
                    })
                    .await?;

                Ok(payments
                    .iter()
                    .filter_map(
                        |payment| match Self::wait_payment_response_from_payment(payment) {
                            Ok(response) => response,
                            Err(err) => {
                                tracing::warn!(
                                    "Could not map LDK Server incoming payment: {}",
                                    err
                                );
                                None
                            }
                        },
                    )
                    .collect())
            }
            PaymentIdentifier::PaymentHash(hash) => {
                let hash = hex::encode(hash);
                let payments = self
                    .list_matching_payments(|payment| {
                        matches!(payment_direction(payment), Ok(PaymentDirection::Inbound))
                            && matches!(payment_status(payment), Ok(PaymentStatus::Succeeded))
                            && matches!(
                                payment_kind(payment),
                                Ok(payment_kind::Kind::Bolt11(bolt11))
                                    if bolt11.hash.eq_ignore_ascii_case(&hash)
                            )
                    })
                    .await?;

                Ok(payments
                    .iter()
                    .filter_map(
                        |payment| match Self::wait_payment_response_from_payment(payment) {
                            Ok(response) => response,
                            Err(err) => {
                                tracing::warn!(
                                    "Could not map LDK Server incoming payment: {}",
                                    err
                                );
                                None
                            }
                        },
                    )
                    .collect())
            }
            PaymentIdentifier::PaymentId(payment_id) => {
                let payment_id = hex::encode(payment_id);
                let payment = match self.get_payment_details(&payment_id).await? {
                    Some(payment) => payment,
                    None => return Ok(vec![]),
                };

                Ok(Self::wait_payment_response_from_payment(&payment)?
                    .into_iter()
                    .collect())
            }
            _ => Err(Error::UnsupportedPaymentIdentifierType.into()),
        }
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match payment_identifier {
            PaymentIdentifier::PaymentId(payment_id) => {
                let payment_id = hex::encode(payment_id);
                let payment = self
                    .get_payment_details(&payment_id)
                    .await?
                    .ok_or(Error::PaymentNotFound)?;

                if payment_direction(&payment)? != PaymentDirection::Outbound {
                    return Err(Error::InvalidPaymentDirection.into());
                }

                Self::make_payment_response_from_payment(
                    &CurrencyUnit::Msat,
                    payment_identifier.clone(),
                    &payment,
                )
            }
            PaymentIdentifier::PaymentHash(hash) => {
                let hash = hex::encode(hash);
                let payments = self
                    .list_matching_payments(|payment| {
                        matches!(payment_direction(payment), Ok(PaymentDirection::Outbound))
                            && matches!(
                                payment_kind(payment),
                                Ok(payment_kind::Kind::Bolt11(bolt11))
                                    if bolt11.hash.eq_ignore_ascii_case(&hash)
                            )
                    })
                    .await?;
                let payment = select_bolt11_payment(payments).ok_or(Error::PaymentNotFound)?;

                Self::make_payment_response_from_payment(
                    &CurrencyUnit::Msat,
                    payment_identifier.clone(),
                    &payment,
                )
            }
            _ => Ok(MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: None,
                status: MeltQuoteState::Unknown,
                total_spent: Amount::new(0, CurrencyUnit::Msat),
            }),
        }
    }
}

impl CdkLdkServer {
    fn fee_reserve_for_amount(&self, unit: &CurrencyUnit, amount: u64) -> Amount<CurrencyUnit> {
        let relative_fee_reserve = (self.fee_reserve.percent_fee_reserve * amount as f32) as u64;
        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();
        Amount::new(relative_fee_reserve.max(absolute_fee_reserve), unit.clone())
    }
}

impl Drop for CdkLdkServer {
    fn drop(&mut self) {
        self.wait_invoice_cancel_token.cancel();
    }
}

struct EventStreamState {
    client: LdkServerClient,
    cancel_token: CancellationToken,
    is_active: Arc<AtomicBool>,
    stream: Option<EventStream>,
    retry_count: u32,
}

impl EventStreamState {
    fn retry_delay(&self) -> Duration {
        Duration::from_secs(2_u64.saturating_pow(self.retry_count).min(10))
    }
}

fn normalize_address(address: &str) -> String {
    address
        .trim()
        .strip_prefix("https://")
        .or_else(|| address.trim().strip_prefix("http://"))
        .unwrap_or_else(|| address.trim())
        .trim_end_matches('/')
        .to_string()
}

fn expiry_secs(unix_expiry: u64) -> Result<u32, payment::Error> {
    let seconds = unix_expiry
        .checked_sub(unix_time())
        .ok_or(payment::Error::InvalidExpiry)?;
    seconds
        .try_into()
        .map_err(|_| payment::Error::InvalidExpiry)
}

fn expiry_secs_or_default(unix_expiry: Option<u64>) -> Result<u32, payment::Error> {
    match unix_expiry {
        Some(unix_expiry) => expiry_secs(unix_expiry),
        None => Ok(DEFAULT_INVOICE_EXPIRY_SECS),
    }
}

fn hex_to_array(hex_value: &str) -> Result<[u8; 32], Error> {
    hex::decode(hex_value)?
        .try_into()
        .map_err(|_| Error::InvalidPaymentId)
}

fn payment_kind(payment: &Payment) -> Result<&payment_kind::Kind, Error> {
    payment
        .kind
        .as_ref()
        .and_then(|kind| kind.kind.as_ref())
        .ok_or(Error::MissingPaymentKind)
}

fn payment_direction(payment: &Payment) -> Result<PaymentDirection, Error> {
    PaymentDirection::from_i32(payment.direction)
        .ok_or(Error::UnknownPaymentDirection(payment.direction))
}

fn payment_status(payment: &Payment) -> Result<PaymentStatus, Error> {
    PaymentStatus::from_i32(payment.status).ok_or(Error::UnknownPaymentStatus(payment.status))
}

fn bolt11_amount_msat_for_quote(
    options: &cdk_common::payment::Bolt11OutgoingPaymentOptions,
) -> Result<u64, payment::Error> {
    match &options.melt_options {
        Some(MeltOptions::Amountless { amountless }) => {
            let amount_msat = u64::from(amountless.amount_msat);

            if let Some(invoice_amount) = options.bolt11.amount_milli_satoshis() {
                if invoice_amount != amount_msat {
                    return Err(payment::Error::AmountMismatch);
                }
            }

            Ok(amount_msat)
        }
        Some(MeltOptions::Mpp { mpp }) => Ok(mpp.amount.into()),
        None => options
            .bolt11
            .amount_milli_satoshis()
            .ok_or(Error::UnknownInvoiceAmount.into()),
    }
}

fn bolt11_amount_msat_for_send(
    options: &cdk_common::payment::Bolt11OutgoingPaymentOptions,
) -> Result<Option<u64>, payment::Error> {
    match &options.melt_options {
        Some(MeltOptions::Amountless { amountless }) => {
            let amount_msat = u64::from(amountless.amount_msat);

            if let Some(invoice_amount) = options.bolt11.amount_milli_satoshis() {
                if invoice_amount != amount_msat {
                    return Err(payment::Error::AmountMismatch);
                }
            }

            Ok(Some(amount_msat))
        }
        Some(MeltOptions::Mpp { mpp: _ }) => Err(payment::Error::UnsupportedPaymentOption),
        None => Ok(None),
    }
}

fn bolt12_amount_msat_for_quote(
    options: &cdk_common::payment::Bolt12OutgoingPaymentOptions,
) -> Result<u64, payment::Error> {
    match bolt12_melt_options_amount_msat(options)? {
        Some(amount_msat) => Ok(amount_msat),
        None => {
            let amount = options
                .offer
                .amount()
                .ok_or(payment::Error::AmountMismatch)?;

            match amount {
                OfferAmount::Bitcoin { amount_msats } => Ok(amount_msats),
                _ => Err(payment::Error::AmountMismatch),
            }
        }
    }
}

fn bolt12_amount_msat_for_send(
    options: &cdk_common::payment::Bolt12OutgoingPaymentOptions,
) -> Result<Option<u64>, payment::Error> {
    bolt12_melt_options_amount_msat(options)
}

fn bolt12_melt_options_amount_msat(
    options: &cdk_common::payment::Bolt12OutgoingPaymentOptions,
) -> Result<Option<u64>, payment::Error> {
    match &options.melt_options {
        Some(MeltOptions::Amountless { amountless }) => {
            let amount_msat = u64::from(amountless.amount_msat);
            validate_bolt12_amount_msat(&options.offer, amount_msat)?;
            Ok(Some(amount_msat))
        }
        Some(MeltOptions::Mpp { mpp: _ }) => Err(payment::Error::UnsupportedPaymentOption),
        None => Ok(None),
    }
}

fn validate_bolt12_amount_msat(
    offer: &lightning::offers::offer::Offer,
    amount_msat: u64,
) -> Result<(), payment::Error> {
    match offer.amount() {
        Some(OfferAmount::Bitcoin { amount_msats }) if amount_msats == amount_msat => Ok(()),
        Some(_) => Err(payment::Error::AmountMismatch),
        None => Ok(()),
    }
}

fn msat_total_spent_for_unit(
    total_msat: u64,
    unit: &CurrencyUnit,
) -> Result<Amount<CurrencyUnit>, payment::Error> {
    match unit {
        CurrencyUnit::Msat => Ok(Amount::new(total_msat, CurrencyUnit::Msat)),
        CurrencyUnit::Sat => Ok(Amount::new(
            total_msat.div_ceil(MSAT_IN_SAT),
            CurrencyUnit::Sat,
        )),
        _ => Amount::new(total_msat, CurrencyUnit::Msat)
            .convert_to(unit)
            .map_err(payment::Error::from),
    }
}

fn route_parameters_from_max_fee(
    max_fee_amount: Option<Amount<CurrencyUnit>>,
) -> Result<Option<RouteParametersConfig>, payment::Error> {
    max_fee_amount
        .map(|amount| {
            amount
                .convert_to(&CurrencyUnit::Msat)
                .map(|amount_msat| RouteParametersConfig {
                    max_total_routing_fee_msat: Some(amount_msat.value()),
                    max_total_cltv_expiry_delta: ROUTE_DEFAULT_MAX_TOTAL_CLTV_EXPIRY_DELTA,
                    max_path_count: ROUTE_DEFAULT_MAX_PATH_COUNT,
                    max_channel_saturation_power_of_half:
                        ROUTE_DEFAULT_MAX_CHANNEL_SATURATION_POWER_OF_HALF,
                })
        })
        .transpose()
        .map_err(payment::Error::from)
}

fn select_bolt11_payment(payments: Vec<Payment>) -> Option<Payment> {
    payments.into_iter().min_by_key(|payment| {
        let status_order = match payment_status(payment) {
            Ok(PaymentStatus::Succeeded) => 0_u8,
            Ok(PaymentStatus::Pending) => 1,
            Ok(PaymentStatus::Failed) | Err(_) => 2,
        };

        (
            status_order,
            std::cmp::Reverse(payment.latest_update_timestamp),
        )
    })
}

async fn sleep_or_cancel(cancel_token: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = cancel_token.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::secp256k1::{Keypair, PublicKey, Secp256k1, SecretKey};
    use cdk_common::payment::Bolt12OutgoingPaymentOptions;
    use ldk_server_client::ldk_server_grpc::types::{Bolt11, PaymentKind};
    use lightning::offers::offer::OfferBuilder;

    fn test_bolt11_payment(status: PaymentStatus, amount_msat: Option<u64>) -> Payment {
        Payment {
            id: "02".repeat(32),
            kind: Some(PaymentKind {
                kind: Some(payment_kind::Kind::Bolt11(Bolt11 {
                    hash: "01".repeat(32),
                    preimage: Some("03".repeat(32)),
                    secret: None,
                })),
            }),
            amount_msat,
            fee_paid_msat: None,
            direction: PaymentDirection::Outbound as i32,
            status: status as i32,
            latest_update_timestamp: 0,
        }
    }

    fn test_bolt11_payment_with_id(
        id_byte: u8,
        status: PaymentStatus,
        latest_update_timestamp: u64,
    ) -> Payment {
        Payment {
            id: format!("{id_byte:02x}").repeat(32),
            latest_update_timestamp,
            ..test_bolt11_payment(status, None)
        }
    }

    fn test_offer(amount_msats: Option<u64>) -> lightning::offers::offer::Offer {
        let secp_ctx = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[42; 32]).expect("test secret key should be valid");
        let keys = Keypair::from_secret_key(&secp_ctx, &secret_key);
        let pubkey = PublicKey::from(keys);
        let builder = OfferBuilder::new(pubkey);

        match amount_msats {
            Some(amount_msats) => builder
                .amount_msats(amount_msats)
                .build()
                .expect("fixed test offer should build"),
            None => builder.build().expect("variable test offer should build"),
        }
    }

    fn bolt12_options(
        offer: lightning::offers::offer::Offer,
        melt_options: Option<MeltOptions>,
    ) -> Bolt12OutgoingPaymentOptions {
        Bolt12OutgoingPaymentOptions {
            offer,
            max_fee_amount: None,
            timeout_secs: None,
            melt_options,
            quote_id: cdk_common::QuoteId::new(),
        }
    }

    #[test]
    fn normalize_address_removes_scheme() {
        assert_eq!(
            normalize_address("https://127.0.0.1:3536/"),
            "127.0.0.1:3536"
        );
        assert_eq!(normalize_address("127.0.0.1:3536"), "127.0.0.1:3536");
    }

    #[test]
    fn failed_payment_response_does_not_require_amount() {
        let payment = test_bolt11_payment(PaymentStatus::Failed, None);

        let response = CdkLdkServer::make_payment_response_from_payment(
            &CurrencyUnit::Msat,
            PaymentIdentifier::PaymentId([2; 32]),
            &payment,
        )
        .expect("failed payment details should map without amount");

        assert_eq!(response.status, MeltQuoteState::Failed);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Msat));
    }

    #[test]
    fn paid_payment_response_requires_amount() {
        let payment = test_bolt11_payment(PaymentStatus::Succeeded, None);

        let err = CdkLdkServer::make_payment_response_from_payment(
            &CurrencyUnit::Msat,
            PaymentIdentifier::PaymentId([2; 32]),
            &payment,
        )
        .expect_err("paid payment details without amount should fail");

        assert!(matches!(err, payment::Error::Lightning(_)));
    }

    #[test]
    fn paid_payment_response_rounds_msats_up_for_sat_unit() {
        let payment = Payment {
            fee_paid_msat: Some(1),
            ..test_bolt11_payment(PaymentStatus::Succeeded, Some(1000))
        };

        let response = CdkLdkServer::make_payment_response_from_payment(
            &CurrencyUnit::Sat,
            PaymentIdentifier::PaymentId([2; 32]),
            &payment,
        )
        .expect("paid payment details should map");

        assert_eq!(response.status, MeltQuoteState::Paid);
        assert_eq!(response.total_spent, Amount::new(2, CurrencyUnit::Sat));
    }

    #[test]
    fn bolt12_amountless_quote_rejects_mismatched_fixed_offer() {
        let offer = test_offer(Some(10_000));
        let options = bolt12_options(offer, Some(MeltOptions::new_amountless(1_000_u64)));

        let err = bolt12_amount_msat_for_quote(&options)
            .expect_err("mismatched fixed offer amount should fail");

        assert!(matches!(err, payment::Error::AmountMismatch));
    }

    #[test]
    fn bolt12_amountless_send_rejects_mismatched_fixed_offer() {
        let offer = test_offer(Some(10_000));
        let options = bolt12_options(offer, Some(MeltOptions::new_amountless(1_000_u64)));

        let err = bolt12_amount_msat_for_send(&options)
            .expect_err("mismatched fixed offer amount should fail");

        assert!(matches!(err, payment::Error::AmountMismatch));
    }

    #[test]
    fn bolt12_amountless_accepts_matching_fixed_offer() {
        let offer = test_offer(Some(10_000));
        let options = bolt12_options(offer, Some(MeltOptions::new_amountless(10_000_u64)));

        let amount_msat =
            bolt12_amount_msat_for_quote(&options).expect("matching fixed offer amount is valid");

        assert_eq!(amount_msat, 10_000);
    }

    #[test]
    fn bolt12_amountless_accepts_variable_offer() {
        let offer = test_offer(None);
        let options = bolt12_options(offer, Some(MeltOptions::new_amountless(10_000_u64)));

        let amount_msat =
            bolt12_amount_msat_for_send(&options).expect("variable offer amount is valid");

        assert_eq!(amount_msat, Some(10_000));
    }

    #[test]
    fn bolt11_payment_selection_prefers_pending_over_failed() {
        let failed = test_bolt11_payment_with_id(1, PaymentStatus::Failed, 2);
        let pending = test_bolt11_payment_with_id(2, PaymentStatus::Pending, 1);

        let selected = select_bolt11_payment(vec![failed, pending])
            .expect("payment details should be selected");

        assert_eq!(selected.id, "02".repeat(32));
        assert_eq!(
            payment_status(&selected).expect("valid status"),
            PaymentStatus::Pending
        );
    }

    #[test]
    fn bolt11_payment_selection_prefers_succeeded_over_pending() {
        let pending = test_bolt11_payment_with_id(1, PaymentStatus::Pending, 2);
        let succeeded = Payment {
            amount_msat: Some(1000),
            ..test_bolt11_payment_with_id(2, PaymentStatus::Succeeded, 1)
        };

        let selected = select_bolt11_payment(vec![pending, succeeded])
            .expect("payment details should be selected");

        assert_eq!(selected.id, "02".repeat(32));
        assert_eq!(
            payment_status(&selected).expect("valid status"),
            PaymentStatus::Succeeded
        );
    }
}
