//! Generic handlers for custom payment methods
//!
//! These handlers work for ANY custom payment method without requiring
//! method-specific validation or request parsing.
//!
//! Special handling for bolt11 and bolt12:
//! When the method parameter is "bolt11" or "bolt12", these handlers use the
//! specific Bolt11/Bolt12 request/response types instead of the generic custom types.

use axum::extract::{FromRequestParts, Json, Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::mint::QuoteId;
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    BatchCheckMintQuoteRequest, BatchMintRequest, MeltOnchainRequest, MeltQuoteBolt11Request,
    MeltQuoteBolt12Request, MeltQuoteCustomRequest, MeltQuoteOnchainRequest,
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintQuoteBolt12Request,
    MintQuoteBolt12Response, MintQuoteCustomRequest, MintQuoteOnchainRequest,
    MintQuoteOnchainResponse, MintRequest, MintResponse, PaymentMethod,
};
use cdk::{MeltQuoteCreateResponse, MeltQuoteResponse};
use serde_json::Value;
use tracing::instrument;

use crate::auth::AuthHeader;
use crate::router_handlers::into_response;
use crate::MintState;

const PREFER_HEADER_KEY: &str = "Prefer";
#[cfg(feature = "federation")]
const FEDERATION_MINT_PAYMENT_OBSERVER_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(5);
#[cfg(feature = "federation")]
const FEDERATION_MINT_PAYMENT_OBSERVER_BATCH_SIZE: usize = 100;

/// Header extractor for the Prefer header
///
/// This extractor checks for the `Prefer: respond-async` header
/// to determine if the client wants asynchronous processing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferHeader {
    pub respond_async: bool,
}

impl<S> FromRequestParts<S> for PreferHeader
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> anyhow::Result<Self, Self::Rejection> {
        // Check for Prefer header
        if let Some(prefer_value) = parts.headers.get(PREFER_HEADER_KEY) {
            let value = prefer_value.to_str().map_err(|_| {
                (
                    StatusCode::BAD_REQUEST,
                    "Invalid Prefer header value".to_string(),
                )
            })?;

            // Check if it contains "respond-async"
            let respond_async = value.to_lowercase().contains("respond-async");

            return Ok(PreferHeader { respond_async });
        }

        // No Prefer header found - default to synchronous processing
        Ok(PreferHeader {
            respond_async: false,
        })
    }
}

/// Serialize a `MeltQuoteResponse` into an HTTP JSON response that carries the
/// per-variant payload shape rather than the enum tag.
fn melt_quote_response_to_json(response: MeltQuoteResponse<QuoteId>) -> Response {
    match response {
        MeltQuoteResponse::Bolt11(r) => Json(r).into_response(),
        MeltQuoteResponse::Bolt12(r) => Json(r).into_response(),
        MeltQuoteResponse::Onchain(r) => Json(r).into_response(),
        MeltQuoteResponse::Custom((_, r)) => Json(r).into_response(),
    }
}

/// Serialize a `MeltQuoteCreateResponse` into an HTTP JSON response that
/// carries the per-variant payload shape rather than the enum tag.
fn melt_quote_create_response_to_json(response: MeltQuoteCreateResponse<QuoteId>) -> Response {
    match response {
        MeltQuoteCreateResponse::Bolt11(r) => Json(r).into_response(),
        MeltQuoteCreateResponse::Bolt12(r) => Json(r).into_response(),
        MeltQuoteCreateResponse::Onchain(r) => Json(r).into_response(),
        MeltQuoteCreateResponse::Custom((_, r)) => Json(r).into_response(),
    }
}

async fn validate_melt_quote_method(
    state: &MintState,
    method: &str,
    quote_id: &QuoteId,
) -> Result<(), cdk::Error> {
    let quote_method = state.mint.get_melt_quote_method(quote_id).await?;
    let expected_method = PaymentMethod::from(method);

    if quote_method != expected_method {
        return Err(cdk::Error::InvalidPaymentMethod);
    }

    Ok(())
}

async fn validate_mint_quote_methods(
    state: &MintState,
    method: &str,
    quote_ids: &[QuoteId],
) -> Result<(), cdk::Error> {
    let expected_method = PaymentMethod::from(method);

    for quote_id in quote_ids {
        let quote_method = state.mint.get_mint_quote_method(quote_id).await?;

        if quote_method != expected_method {
            return Err(cdk::Error::InvalidPaymentMethod);
        }
    }

    Ok(())
}

async fn validate_mint_request_route(
    auth: AuthHeader,
    state: &MintState,
    method: &str,
    quote_ids: &[QuoteId],
) -> Result<(), cdk::Error> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Mint(method.to_owned())),
        )
        .await?;

    validate_mint_quote_methods(state, method, quote_ids).await
}

async fn process_mint_input(
    state: &MintState,
    input: cdk::mint::MintInput,
) -> Result<MintResponse, cdk::Error> {
    #[cfg(feature = "federation")]
    if let Some(response) = process_federation_mint_input(state, &input).await? {
        return Ok(response);
    }

    state.mint.process_mint_request(input).await
}

#[cfg(feature = "federation")]
async fn process_federation_mint_input(
    state: &MintState,
    input: &cdk::mint::MintInput,
) -> Result<Option<MintResponse>, cdk::Error> {
    let Some(operation_id) = submit_federation_mint_operation(state, input.clone()).await? else {
        return Ok(None);
    };

    Ok(Some(
        state
            .mint
            .persisted_mint_share_response(operation_id, input)
            .await?,
    ))
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_quote_operation(
    state: &MintState,
    request: cdk::mint::MintQuoteRequest,
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let envelope = config.mint_quote_operation(
        response.quote().clone(),
        request,
        mint_quote_response_to_string(response),
    );
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_quote_payment_observation(
    state: &MintState,
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> Result<bool, cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(false);
    };
    let (amount_paid, amount_issued) = mint_quote_response_amounts(response);
    if response.state() != Some(cdk::nuts::MintQuoteState::Paid) || amount_paid == cdk::Amount::ZERO
    {
        return Ok(false);
    }
    let mut observation = cdk::federation::FederationMintQuotePaymentObservation {
        quote: response.quote().to_string(),
        observer: config.member_id(),
        method: response.method(),
        state: cdk::nuts::MintQuoteState::Paid,
        amount_paid,
        amount_issued,
        payment_id: None,
        payment_proof: None,
        observed_time: None,
    };
    let recorded_observations = state
        .mint
        .localstore()
        .get_federation_payment_observations(
            cdk::federation::FederationPaymentObservationContext::MintQuote,
            &observation.quote,
        )
        .await?;
    if has_matching_local_mint_payment_observation(&recorded_observations, &observation) {
        return Ok(false);
    }

    let stored_quote = state
        .mint
        .localstore()
        .get_mint_quote(response.quote())
        .await?;
    let payment_metadata = stored_quote.as_ref().and_then(|quote| {
        quote.payments.last().map(|payment| {
            (
                cdk::cdk_payment::PaymentIdentifier::CustomId(payment.payment_id.clone()),
                payment.time,
            )
        })
    });
    observation.payment_id = payment_metadata
        .as_ref()
        .map(|(payment_id, _)| payment_id.clone());
    observation.observed_time = payment_metadata.map(|(_, observed_time)| observed_time);

    let envelope = config.mint_quote_payment_observation_with_metadata(
        observation.quote.clone(),
        observation.method.clone(),
        observation.state,
        observation.amount_paid,
        observation.amount_issued,
        observation.payment_proof.clone(),
        observation.payment_id.clone(),
        observation.observed_time,
    );
    state
        .submit_federation_payment_observation_envelope(envelope)
        .await?;
    Ok(true)
}

#[cfg(feature = "federation")]
pub(crate) async fn submit_federation_mint_quote_payment_observation_round(
    state: &MintState,
) -> Result<usize, cdk::Error> {
    if state.mint.federation_config().is_none() {
        return Ok(0);
    }

    let quotes = state.mint.localstore().get_mint_quotes().await?;
    let quote_ids = quotes
        .into_iter()
        .filter(|quote| quote.state() != cdk::nuts::MintQuoteState::Issued)
        .map(|quote| quote.id)
        .collect::<Vec<_>>();
    if quote_ids.is_empty() {
        return Ok(0);
    }

    let mut submitted = 0usize;
    for chunk in quote_ids.chunks(FEDERATION_MINT_PAYMENT_OBSERVER_BATCH_SIZE) {
        let responses = state.mint.check_mint_quotes(chunk).await?;
        for response in responses {
            if submit_federation_mint_quote_payment_observation(state, &response).await? {
                submitted = submitted.saturating_add(1);
            }
        }
    }

    Ok(submitted)
}

#[cfg(feature = "federation")]
pub(crate) fn spawn_federation_mint_payment_observer_loop(state: MintState) {
    if state.mint.federation_config().is_none() {
        return;
    }

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(FEDERATION_MINT_PAYMENT_OBSERVER_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;

        loop {
            interval.tick().await;
            match submit_federation_mint_quote_payment_observation_round(&state).await {
                Ok(submitted) if submitted > 0 => {
                    tracing::debug!(
                        submitted,
                        "Federation mint payment observer submitted observations"
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!("Federation mint payment observer failed: {}", err);
                }
            }
        }
    });
}

#[cfg(feature = "federation")]
async fn submit_federation_melt_quote_operation(
    state: &MintState,
    request: cdk::MeltQuoteRequest,
    response: &MeltQuoteCreateResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let quote = response.quote().cloned().ok_or(cdk::Error::UnknownQuote)?;
    let envelope = config.melt_quote_operation(quote, request);
    state.submit_federation_envelope(envelope).await?;
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_melt_quote_payment_observation(
    state: &MintState,
    response: &MeltQuoteResponse<QuoteId>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    if !matches!(
        response.state(),
        cdk::nuts::MeltQuoteState::Paid | cdk::nuts::MeltQuoteState::Failed
    ) {
        return Ok(());
    }
    let mut observation = cdk::federation::FederationMeltQuotePaymentObservation {
        quote: response.quote().to_string(),
        observer: config.member_id(),
        method: response.method(),
        state: response.state(),
        amount: response.amount(),
        fee_reserve: response.fee_reserve(),
        payment_id: None,
        payment_proof: response.payment_proof().map(str::to_string),
        failure_reason: None,
        observed_time: None,
    };
    let recorded_observations = state
        .mint
        .localstore()
        .get_federation_payment_observations(
            cdk::federation::FederationPaymentObservationContext::MeltQuote,
            &observation.quote,
        )
        .await?;
    if has_matching_local_melt_payment_observation(&recorded_observations, &observation) {
        return Ok(());
    }

    let stored_quote = state
        .mint
        .localstore()
        .get_melt_quote(response.quote())
        .await?;
    observation.payment_id = stored_quote
        .as_ref()
        .and_then(|quote| quote.request_lookup_id.clone());
    observation.observed_time = stored_quote.as_ref().and_then(|quote| quote.paid_time);

    let envelope = config.melt_quote_payment_observation_with_metadata(
        observation.quote.clone(),
        observation.method.clone(),
        observation.state,
        observation.amount,
        observation.fee_reserve,
        observation.payment_proof.clone(),
        observation.failure_reason.clone(),
        observation.payment_id.clone(),
        observation.observed_time,
    );
    state
        .submit_federation_payment_observation_envelope(envelope)
        .await?;
    Ok(())
}

#[cfg(feature = "federation")]
fn has_matching_local_mint_payment_observation(
    records: &[cdk::federation::FederationPaymentObservationRecord],
    candidate: &cdk::federation::FederationMintQuotePaymentObservation,
) -> bool {
    records.iter().any(|record| {
        let cdk::federation::FederationPaymentObservation::MintQuote(observation) =
            &record.observation
        else {
            return false;
        };

        // Mirror the consensus conflict key so the periodic observer never
        // retries an operation this member has already committed.
        observation.observer == candidate.observer
            && observation.quote == candidate.quote
            && observation.state == candidate.state
    })
}

#[cfg(feature = "federation")]
fn has_matching_local_melt_payment_observation(
    records: &[cdk::federation::FederationPaymentObservationRecord],
    candidate: &cdk::federation::FederationMeltQuotePaymentObservation,
) -> bool {
    records.iter().any(|record| {
        let cdk::federation::FederationPaymentObservation::MeltQuote(observation) =
            &record.observation
        else {
            return false;
        };

        // Mirror the consensus conflict key so the periodic observer never
        // retries an operation this member has already committed.
        observation.observer == candidate.observer
            && observation.quote == candidate.quote
            && observation.state == candidate.state
    })
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_operation(
    state: &MintState,
    input: cdk::mint::MintInput,
) -> Result<Option<cdk::federation::FederationOperationId>, cdk::Error> {
    let Some(config) = state.mint.effective_federation_config().await? else {
        return Ok(None);
    };
    let envelope = config.mint_operation(input)?;
    let response = state.submit_federation_envelope(envelope).await?;
    Ok(Some(response.operation_id))
}

#[cfg(feature = "federation")]
fn mint_quote_response_to_string(
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> cdk::mint::MintQuoteResponse<String> {
    match response {
        cdk::mint::MintQuoteResponse::Bolt11(response) => {
            cdk::mint::MintQuoteResponse::Bolt11(response.to_string_id())
        }
        cdk::mint::MintQuoteResponse::Bolt12(response) => {
            cdk::mint::MintQuoteResponse::Bolt12(response.to_string_id())
        }
        cdk::mint::MintQuoteResponse::Onchain(response) => {
            cdk::mint::MintQuoteResponse::Onchain(response.to_string_id())
        }
        cdk::mint::MintQuoteResponse::Custom { method, response } => {
            cdk::mint::MintQuoteResponse::Custom {
                method: method.clone(),
                response: response.to_string_id(),
            }
        }
    }
}

#[cfg(feature = "federation")]
fn mint_quote_response_amounts(
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
) -> (cdk::Amount, cdk::Amount) {
    match response {
        cdk::mint::MintQuoteResponse::Bolt11(response) => {
            bolt11_mint_quote_amounts(response.state, response.amount)
        }
        cdk::mint::MintQuoteResponse::Bolt12(response) => {
            (response.amount_paid, response.amount_issued)
        }
        cdk::mint::MintQuoteResponse::Onchain(response) => {
            (response.amount_paid, response.amount_issued)
        }
        cdk::mint::MintQuoteResponse::Custom { response, .. } => {
            (response.amount_paid, response.amount_issued)
        }
    }
}

#[cfg(feature = "federation")]
fn bolt11_mint_quote_amounts(
    state: cdk::nuts::MintQuoteState,
    amount: Option<cdk::Amount>,
) -> (cdk::Amount, cdk::Amount) {
    let amount = amount.unwrap_or(cdk::Amount::ZERO);
    match state {
        cdk::nuts::MintQuoteState::Unpaid => (cdk::Amount::ZERO, cdk::Amount::ZERO),
        cdk::nuts::MintQuoteState::Paid => (amount, cdk::Amount::ZERO),
        cdk::nuts::MintQuoteState::Issued => (amount, amount),
    }
}

async fn validate_melt_request_route(
    auth: AuthHeader,
    state: &MintState,
    method: &str,
    quote_id: &QuoteId,
) -> Result<(), cdk::Error> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Melt(method.to_owned())),
        )
        .await?;

    validate_melt_quote_method(state, method, quote_id).await
}

/// Generic handler for custom payment method mint quotes
///
/// This handler works for ANY custom payment method (e.g., paypal, venmo, cashapp, bolt11, bolt12).
/// For bolt11/bolt12, it handles the specific request/response types.
/// For other methods, it passes the request data directly to the payment processor.
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_mint_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    #[cfg(feature = "federation")]
    state
        .ensure_federation_ready_for_public_submit()
        .await
        .map_err(into_response)?;

    match method.as_str() {
        "bolt11" => {
            let bolt11_request: MintQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = bolt11_request.into();
            let quote = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &quote)
                .await
                .map_err(into_response)?;

            let response: MintQuoteBolt11Response<QuoteId> =
                quote.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "bolt12" => {
            if payload.get("pubkey").is_none_or(|v| v.is_null()) {
                return Err(into_response(cdk::Error::PubkeyRequired));
            }
            let bolt12_request: MintQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = bolt12_request.into();
            let quote = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &quote)
                .await
                .map_err(into_response)?;

            let response: MintQuoteBolt12Response<QuoteId> =
                quote.try_into().map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "onchain" => {
            let onchain_request: MintQuoteOnchainRequest = serde_json::from_value(payload)
                .map_err(|e| {
                    tracing::error!("Failed to parse onchain request: {}", e);
                    if e.to_string().contains("missing field `pubkey`") {
                        into_response(cdk::Error::PubkeyRequired)
                    } else {
                        into_response(cdk::Error::InvalidPaymentMethod)
                    }
                })?;

            let quote_request: cdk::mint::MintQuoteRequest = onchain_request.into();
            let quote = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &quote)
                .await
                .map_err(into_response)?;

            let response: MintQuoteOnchainResponse<QuoteId> =
                MintQuoteOnchainResponse::try_from(quote).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        _ => {
            let custom_request: MintQuoteCustomRequest =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse custom request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::mint::MintQuoteRequest::Custom {
                method: cdk::nuts::PaymentMethod::from(method.clone()),
                request: custom_request,
            };

            let response = state
                .mint
                .get_mint_quote(quote_request.clone())
                .await
                .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(&state, quote_request, &response)
                .await
                .map_err(into_response)?;

            match response {
                cdk::mint::MintQuoteResponse::Custom { response, .. } => {
                    Ok(Json(response).into_response())
                }
                _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
            }
        }
    }
}

/// Get custom payment method mint quote status
#[instrument(skip_all, fields(method = ?method, quote_id = ?quote_id))]
pub async fn get_check_mint_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Get, RoutePath::MintQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    let quote_response = state
        .mint
        .check_mint_quotes(&[quote_id])
        .await
        .map_err(into_response)?
        .first()
        .cloned()
        .ok_or(cdk::Error::UnknownQuote)
        .map_err(into_response)?;
    #[cfg(feature = "federation")]
    submit_federation_mint_quote_payment_observation(&state, &quote_response)
        .await
        .map_err(into_response)?;

    match method.as_str() {
        "bolt11" => {
            let response: MintQuoteBolt11Response<QuoteId> =
                MintQuoteBolt11Response::try_from(quote_response).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "bolt12" => {
            let response: MintQuoteBolt12Response<QuoteId> =
                MintQuoteBolt12Response::try_from(quote_response).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        "onchain" => {
            let response: MintQuoteOnchainResponse<QuoteId> =
                MintQuoteOnchainResponse::try_from(quote_response).map_err(into_response)?;
            Ok(Json(response).into_response())
        }
        _ => {
            // Extract and verify it's a Custom payment method
            match quote_response {
                cdk::mint::MintQuoteResponse::Custom {
                    method: quote_method,
                    response,
                    ..
                } => {
                    if quote_method.to_string() != method {
                        return Err(into_response(cdk::Error::InvalidPaymentMethod));
                    }
                    Ok(Json(response).into_response())
                }
                _ => Err(into_response(cdk::Error::InvalidPaymentMethod)),
            }
        }
    }
}

/// Batch check mint quote status (NUT-29)
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_batch_check_mint_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<BatchCheckMintQuoteRequest<QuoteId>>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MintQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    validate_mint_quote_methods(&state, &method, &payload.quotes)
        .await
        .map_err(into_response)?;

    let responses = state
        .mint
        .check_mint_quotes(&payload.quotes)
        .await
        .map_err(into_response)?;
    #[cfg(feature = "federation")]
    for response in &responses {
        submit_federation_mint_quote_payment_observation(&state, response)
            .await
            .map_err(into_response)?;
    }

    match method.as_str() {
        "bolt11" => {
            let responses: Vec<MintQuoteBolt11Response<QuoteId>> = responses
                .into_iter()
                .map(MintQuoteBolt11Response::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
        "bolt12" => {
            let responses: Vec<MintQuoteBolt12Response<QuoteId>> = responses
                .into_iter()
                .map(MintQuoteBolt12Response::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
        "onchain" => {
            let responses: Vec<MintQuoteOnchainResponse<QuoteId>> = responses
                .into_iter()
                .map(MintQuoteOnchainResponse::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
        _ => {
            let responses: Vec<cdk::nuts::MintQuoteCustomResponse<QuoteId>> = responses
                .into_iter()
                .map(|r| match r {
                    cdk::mint::MintQuoteResponse::Custom { response, .. } => Ok(response),
                    _ => Err(cdk::Error::InvalidPaymentMethod),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(into_response)?;
            Ok(Json(responses).into_response())
        }
    }
}

/// Request a melt quote for custom payment method
#[instrument(skip_all, fields(method = ?method))]
pub async fn post_melt_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path(method): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::MeltQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    #[cfg(feature = "federation")]
    state
        .ensure_federation_ready_for_public_submit()
        .await
        .map_err(into_response)?;

    let (quote_request, response) = match method.as_str() {
        "bolt11" => {
            let bolt11_request: MeltQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt11(bolt11_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
        "bolt12" => {
            let bolt12_request: MeltQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt12(bolt12_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
        "onchain" => {
            let onchain_request: MeltQuoteOnchainRequest = serde_json::from_value(payload)
                .map_err(|e| {
                    tracing::error!("Failed to parse onchain melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Onchain(onchain_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
        _ => {
            let custom_request: MeltQuoteCustomRequest =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse custom melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let request_method = PaymentMethod::from(custom_request.method.as_str());
            let route_method = PaymentMethod::from(method.as_str());

            if request_method != route_method {
                return Err(into_response(cdk::Error::InvalidPaymentMethod));
            }

            let quote_request = cdk::MeltQuoteRequest::Custom(custom_request);
            let response = state
                .mint
                .get_melt_quote(quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response)
        }
    };

    #[cfg(feature = "federation")]
    submit_federation_melt_quote_operation(&state, quote_request, &response)
        .await
        .map_err(into_response)?;
    #[cfg(not(feature = "federation"))]
    let _ = quote_request;

    Ok(melt_quote_create_response_to_json(response))
}

/// Get custom payment method melt quote status
#[instrument(skip_all, fields(method = ?method, quote_id = ?quote_id))]
pub async fn get_check_melt_custom_quote(
    auth: AuthHeader,
    State(state): State<MintState>,
    Path((method, quote_id)): Path<(String, QuoteId)>,
) -> Result<Response, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Get, RoutePath::MeltQuote(method.clone())),
        )
        .await
        .map_err(into_response)?;

    validate_melt_quote_method(&state, &method, &quote_id)
        .await
        .map_err(into_response)?;

    let quote = state
        .mint
        .check_melt_quote(&quote_id)
        .await
        .map_err(into_response)?;

    #[cfg(feature = "federation")]
    submit_federation_melt_quote_payment_observation(&state, &quote)
        .await
        .map_err(into_response)?;

    Ok(melt_quote_response_to_json(quote))
}

async fn process_melt_request(
    prefer: PreferHeader,
    state: &MintState,
    method: &str,
    payload: &cdk::nuts::MeltRequest<QuoteId>,
) -> Result<MeltQuoteResponse<QuoteId>, cdk::Error> {
    #[cfg(feature = "federation")]
    if let Some(response) = process_federation_melt_request(state, payload).await? {
        return Ok(response);
    }

    // Check for async preference in either the Prefer header or the request body
    // For onchain we always want to do the async flow
    let respond_async = prefer.respond_async || payload.is_prefer_async() || method == "onchain";

    let pending = state.mint.melt(payload).await?;

    let res = if respond_async {
        // Asynchronous processing - return immediately after setup
        pending.into_pending_response()
    } else {
        // Synchronous processing - wait for completion
        pending.await?
    };

    Ok(res)
}

#[cfg(feature = "federation")]
async fn process_federation_melt_request(
    state: &MintState,
    payload: &cdk::nuts::MeltRequest<QuoteId>,
) -> Result<Option<MeltQuoteResponse<QuoteId>>, cdk::Error> {
    let Some(config) = state.mint.effective_federation_config().await? else {
        return Ok(None);
    };
    let envelope = config.melt_operation(payload.clone())?;
    state.submit_federation_envelope(envelope).await?;
    if federation_melt_quote_is_failed_terminal(state, payload.quote()).await? {
        return Err(cdk::Error::PaymentFailed);
    }

    Ok(Some(
        federation_melt_response_from_store(state, payload.quote()).await?,
    ))
}

#[cfg(feature = "federation")]
async fn federation_melt_response_from_store(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<MeltQuoteResponse<QuoteId>, cdk::Error> {
    let quote = state
        .mint
        .localstore()
        .get_melt_quote(quote_id)
        .await?
        .ok_or(cdk::Error::UnknownQuote)?;
    let change = state
        .mint
        .localstore()
        .get_blind_signatures_for_quote(quote_id)
        .await?;
    let change_opt = if change.is_empty() {
        None
    } else {
        Some(change)
    };

    Ok(quote.into_response(change_opt))
}

#[cfg(feature = "federation")]
async fn federation_melt_quote_is_failed_terminal(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<bool, cdk::Error> {
    let Some(quote) = state.mint.localstore().get_melt_quote(quote_id).await? else {
        return Ok(false);
    };
    if quote.state != cdk::nuts::MeltQuoteState::Unpaid {
        return Ok(false);
    }
    let saga = state
        .mint
        .localstore()
        .get_melt_saga_by_quote_id(quote_id)
        .await?;

    Ok(saga.is_none())
}

// ============================================================================
// CACHED HANDLERS FOR NUT-19 SUPPORT
// ============================================================================

/// Cached version of post_mint_custom for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method, quote_id = ?payload.quote))]
pub async fn cache_post_mint_custom(
    auth: AuthHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<MintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    use std::ops::Deref;

    let State(mint_state) = state;
    let method = method.0;
    let payload = payload.0;

    validate_mint_request_route(
        auth,
        &mint_state,
        &method,
        std::slice::from_ref(&payload.quote),
    )
    .await
    .map_err(into_response)?;

    let cache_key = match mint_state
        .cache
        .calculate_key(&("mint", method.as_str(), &payload))
    {
        Some(key) => key,
        None => {
            let result = process_mint_input(&mint_state, cdk::mint::MintInput::Single(payload))
                .await
                .map_err(into_response)?;

            return Ok(Json(result));
        }
    };

    if let Some(cached_response) = mint_state.cache.get::<MintResponse>(&cache_key).await {
        return Ok(Json(cached_response));
    }

    let result = Json(
        process_mint_input(&mint_state, cdk::mint::MintInput::Single(payload))
            .await
            .map_err(into_response)?,
    );

    // Cache the response
    mint_state.cache.set(cache_key, result.deref()).await;

    Ok(result)
}

/// Cached version of post_melt_custom for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method))]
pub async fn cache_post_melt_custom(
    auth: AuthHeader,
    prefer: PreferHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<Value>,
) -> Result<Response, Response> {
    use std::ops::Deref;

    let State(mint_state) = state.clone();
    let method = method.0;
    tracing::debug!(method = %method, "melt request received");
    let parsed_payload = parse_melt_payload(&method, payload.deref().clone())?;

    validate_melt_request_route(auth, &mint_state, &method, parsed_payload.quote())
        .await
        .map_err(into_response)?;

    let cache_key =
        match mint_state
            .cache
            .calculate_key(&("melt", method.as_str(), &parsed_payload))
        {
            Some(key) => key,
            None => {
                let result = process_melt_request(prefer, &mint_state, &method, &parsed_payload)
                    .await
                    .map_err(into_response)?;

                return Ok(melt_quote_response_to_json(result));
            }
        };

    if let Some(cached_response) = mint_state
        .cache
        .get::<MeltQuoteResponse<QuoteId>>(&cache_key)
        .await
    {
        return Ok(melt_quote_response_to_json(cached_response));
    }

    let result = process_melt_request(prefer, &mint_state, &method, &parsed_payload)
        .await
        .map_err(into_response)?;

    mint_state.cache.set(cache_key, &result).await;

    Ok(melt_quote_response_to_json(result))
}

// The `Err` variant carries an axum `Response` (a pre-built 400 with the full
// body), which is large but matches the style used throughout this module.
// Refactoring to a boxed/smaller error would be a cross-cutting change outside
// the scope of this bug fix.
#[allow(clippy::result_large_err)]
fn parse_melt_payload(
    method: &str,
    payload: Value,
) -> Result<cdk::nuts::MeltRequest<QuoteId>, Response> {
    if method == "onchain" {
        let request: MeltOnchainRequest<QuoteId> =
            serde_json::from_value(payload).map_err(|e| {
                tracing::warn!(
                    method = %method,
                    "failed to parse onchain melt request body: {e}",
                );
                into_response(cdk::Error::InvalidPaymentRequest)
            })?;
        Ok(request.into())
    } else {
        serde_json::from_value(payload).map_err(|e| {
            tracing::warn!(
                method = %method,
                "failed to parse melt request body: {e}",
            );
            into_response(cdk::Error::InvalidPaymentRequest)
        })
    }
}

/// Cached version of post_batch_mint for NUT-19 caching support
#[instrument(skip_all, fields(method = ?method))]
pub async fn cache_post_batch_mint(
    auth: AuthHeader,
    state: State<MintState>,
    method: Path<String>,
    payload: Json<BatchMintRequest<QuoteId>>,
) -> Result<Json<MintResponse>, Response> {
    use std::ops::Deref;

    let State(mint_state) = state;
    let method = method.0;
    let payload = payload.0;

    validate_mint_request_route(auth, &mint_state, &method, &payload.quotes)
        .await
        .map_err(into_response)?;

    let cache_key = match mint_state
        .cache
        .calculate_key(&("mint_batch", method.as_str(), &payload))
    {
        Some(key) => key,
        None => {
            let result = process_mint_input(&mint_state, cdk::mint::MintInput::Batch(payload))
                .await
                .map_err(into_response)?;

            return Ok(Json(result));
        }
    };

    if let Some(cached_response) = mint_state.cache.get::<MintResponse>(&cache_key).await {
        return Ok(Json(cached_response));
    }

    let result = Json(
        process_mint_input(&mint_state, cdk::mint::MintInput::Batch(payload))
            .await
            .map_err(into_response)?,
    );

    mint_state.cache.set(cache_key, result.deref()).await;

    Ok(result)
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "federation")]
    use std::collections::BTreeMap;
    use std::collections::{HashMap, HashSet};
    #[cfg(feature = "federation")]
    use std::pin::Pin;
    #[cfg(feature = "federation")]
    use std::str::FromStr;
    #[cfg(feature = "federation")]
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[cfg(feature = "federation")]
    use async_trait::async_trait;
    use axum::http::{HeaderValue, Request, StatusCode};
    use bip39::Mnemonic;
    #[cfg(feature = "federation")]
    use cdk::cdk_payment::{
        self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
        MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse,
        SettingsResponse, WaitPaymentResponse,
    };
    #[cfg(feature = "federation")]
    use cdk::federation::{
        generate_trusted_dealer_configs, FederationConfig, FederationConsensusBackendKind,
        FederationConsensusEngine, FederationConsensusRuntime, FederationJournalEntry,
        FederationKeysetRotationOperation, FederationOperation, FederationOperationEnvelope,
        FederationOperationId, InMemoryFederationConsensus, InMemoryFederationJournal,
        MemberConfigPrivate, MemberId, ThresholdParams, TrustedDealerConfigBundle,
        TrustedDealerKeysetSpec, TrustedDealerMember,
    };
    use cdk::mint::{Mint, MintBuilder, MintMeltLimits, MintQuoteResponse};
    use cdk::nuts::nut00::KnownMethod;
    #[cfg(feature = "federation")]
    use cdk::nuts::nut01::{
        verify_blind_signature_share, BlsBlindSignatureShare, BlsG1PublicKey, BlsSecretKey,
    };
    #[cfg(feature = "federation")]
    use cdk::nuts::MeltQuoteState;
    #[cfg(feature = "federation")]
    use cdk::nuts::SecretKey;
    use cdk::nuts::{
        BatchMintRequest, BlindSignature, BlindedMessage, CheckStateRequest, CurrencyUnit,
        MeltRequest, MintQuoteState, MintRequest, PaymentMethod, Proof, RestoreRequest,
        SwapRequest,
    };
    #[cfg(feature = "federation")]
    use cdk::secret::Secret;
    use cdk::types::{FeeReserve, QuoteTTL};
    #[cfg(feature = "federation")]
    use cdk::wallet::{FederatedMintConnector, MintConnector};
    use cdk::Amount;
    use cdk_fake_wallet::FakeWallet;
    #[cfg(feature = "federation")]
    use cdk_fake_wallet::{create_fake_invoice, FakeInvoiceDescription};
    #[cfg(feature = "federation")]
    use futures::Stream;

    use super::*;
    use crate::cache::HttpCache;

    #[cfg(feature = "federation")]
    struct TempMintDbFile {
        path: String,
    }

    #[cfg(feature = "federation")]
    impl TempMintDbFile {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("{name}-{}.sqlite", uuid::Uuid::new_v4()))
                .to_string_lossy()
                .to_string();

            Self { path }
        }

        fn path(&self) -> &str {
            &self.path
        }
    }

    #[cfg(feature = "federation")]
    impl Drop for TempMintDbFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(format!("{}-shm", self.path));
            let _ = std::fs::remove_file(format!("{}-wal", self.path));
        }
    }

    #[cfg(feature = "federation")]
    fn production_section<'a>(source: &'a str, test_marker: &str) -> &'a str {
        source
            .split_once(test_marker)
            .map(|(production, _tests)| production)
            .expect("source file contains test module marker")
    }

    #[cfg(feature = "federation")]
    fn assert_contains_in_order(source: &str, first: &str, second: &str) {
        let first_index = source.find(first).expect("source contains first marker");
        let second_index = source.find(second).expect("source contains second marker");
        assert!(
            first_index < second_index,
            "`{first}` must appear before `{second}`"
        );
    }

    #[cfg(feature = "federation")]
    fn source_slice<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let start_index = source.find(start).expect("source contains start marker");
        let end_index = source[start_index..]
            .find(end)
            .expect("source contains end marker")
            + start_index;
        &source[start_index..end_index]
    }

    #[cfg(feature = "federation")]
    fn assert_occurrences(source: &str, needle: &str, expected: usize) {
        assert_eq!(
            source.match_indices(needle).count(),
            expected,
            "source must contain exactly {expected} reviewed occurrence(s) of `{needle}`"
        );
    }

    #[cfg(feature = "federation")]
    #[test]
    fn public_handlers_do_not_directly_sign_in_production() {
        let custom_handlers = production_section(
            include_str!("custom_handlers.rs"),
            "\n#[cfg(test)]\nmod tests {",
        );
        let router_handlers = include_str!("router_handlers.rs");

        for (file, source) in [
            ("custom_handlers.rs", custom_handlers),
            ("router_handlers.rs", router_handlers),
        ] {
            for forbidden in [
                ".blind_sign(",
                ".sign_blinded_messages(",
                "blind_sign_share(",
                "auth_blind_sign(",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{file} production handlers must not call `{forbidden}` directly"
                );
            }
        }

        let mint_handler = source_slice(
            custom_handlers,
            "async fn process_federation_mint_input",
            "fn mint_quote_response_to_string",
        );
        assert_contains_in_order(
            mint_handler,
            "submit_federation_mint_operation(state, input.clone()).await?",
            "persisted_mint_share_response(operation_id, input)",
        );

        let melt_handler = source_slice(
            custom_handlers,
            "async fn process_federation_melt_request",
            "async fn federation_melt_response_from_store",
        );
        assert_contains_in_order(
            melt_handler,
            "state.submit_federation_envelope(envelope).await?",
            "federation_melt_response_from_store(state, payload.quote()).await?",
        );

        let swap_handler = source_slice(
            router_handlers,
            "async fn process_federation_swap",
            "/// Restores blind signature",
        );
        assert_contains_in_order(
            swap_handler,
            "state.submit_federation_envelope(envelope).await?",
            "persisted_swap_share_response(response.operation_id, payload)",
        );
    }

    #[cfg(feature = "federation")]
    #[test]
    fn federation_payment_observation_handlers_are_backend_agnostic() {
        let custom_handlers = production_section(
            include_str!("custom_handlers.rs"),
            "\n#[cfg(test)]\nmod tests {",
        );

        for forbidden in [
            "cdk_fake_wallet",
            "FakeWallet",
            "fakewallet",
            "fake_wallet",
            "fake-wallet",
            "CLN",
            "Cln",
            "LND",
            "Lnd",
            "LNBits",
            "Lnbits",
            "LDK",
            "Ldk",
        ] {
            assert!(
                !custom_handlers.contains(forbidden),
                "production custom handlers must not branch on concrete payment backend `{forbidden}`"
            );
        }

        let mint_observation = source_slice(
            custom_handlers,
            "async fn submit_federation_mint_quote_payment_observation",
            "async fn submit_federation_melt_quote_operation",
        );
        assert_contains_in_order(
            mint_observation,
            "response: &cdk::mint::MintQuoteResponse<QuoteId>",
            "mint_quote_response_amounts(response)",
        );
        assert_contains_in_order(
            mint_observation,
            "response.state() != Some(cdk::nuts::MintQuoteState::Paid)",
            "cdk::federation::FederationMintQuotePaymentObservation",
        );
        assert!(mint_observation.contains("method: response.method()"));
        assert!(mint_observation.contains(".submit_federation_payment_observation_envelope("));
        assert_contains_in_order(
            mint_observation,
            ".get_mint_quotes().await?",
            ".filter(|quote| quote.state() != cdk::nuts::MintQuoteState::Issued)",
        );
        assert_contains_in_order(
            mint_observation,
            ".filter(|quote| quote.state() != cdk::nuts::MintQuoteState::Issued)",
            ".check_mint_quotes(chunk).await?",
        );

        let melt_observation = source_slice(
            custom_handlers,
            "async fn submit_federation_melt_quote_payment_observation",
            "async fn submit_federation_mint_operation",
        );
        assert_contains_in_order(
            melt_observation,
            "response: &MeltQuoteResponse<QuoteId>",
            "response.state()",
        );
        assert_contains_in_order(
            melt_observation,
            "response.state()",
            "cdk::federation::FederationMeltQuotePaymentObservation",
        );
        assert!(melt_observation.contains("method: response.method()"));
        assert!(melt_observation.contains(".submit_federation_payment_observation_envelope("));
    }

    #[cfg(feature = "federation")]
    #[test]
    fn public_endpoints_are_consensus_gated_in_federation_mode() {
        let custom_handlers = production_section(
            include_str!("custom_handlers.rs"),
            "\n#[cfg(test)]\nmod tests {",
        );
        let router_handlers = include_str!("router_handlers.rs");
        let mint_mod = production_section(
            include_str!("../../cdk/src/mint/mod.rs"),
            "\n#[cfg(test)]\nmod tests {",
        );
        let mint_issue_mod = production_section(
            include_str!("../../cdk/src/mint/issue/mod.rs"),
            "\n#[cfg(test)]\nmod batch_mint_tests",
        );
        let mint_swap_mod = include_str!("../../cdk/src/mint/swap/mod.rs");
        let mint_melt_mod = include_str!("../../cdk/src/mint/melt/mod.rs");

        let get_keys = source_slice(
            router_handlers,
            "pub(crate) async fn get_keys",
            "/// Get the public keys of a specific keyset",
        );
        assert!(get_keys.contains(".effective_federation_pubkeys()"));

        let get_keyset_pubkeys = source_slice(
            router_handlers,
            "pub(crate) async fn get_keyset_pubkeys",
            "/// Get all active keyset IDs of the mint",
        );
        assert!(get_keyset_pubkeys.contains(".effective_federation_keyset_pubkeys(&keyset_id)"));

        let get_keysets = source_slice(
            router_handlers,
            "pub(crate) async fn get_keysets",
            "#[instrument(skip_all)]\npub(crate) async fn ws_handler",
        );
        assert!(get_keysets.contains(".effective_federation_keysets()"));

        let mint_quote = source_slice(
            custom_handlers,
            "pub async fn post_mint_custom_quote",
            "pub async fn get_check_mint_custom_quote",
        );
        assert_occurrences(
            mint_quote,
            "submit_federation_mint_quote_operation(&state",
            4,
        );
        assert_contains_in_order(
            mint_quote,
            ".ensure_federation_ready_for_public_submit()",
            ".get_mint_quote(quote_request.clone())",
        );

        let mint_public_submit = source_slice(
            custom_handlers,
            "async fn process_mint_input",
            "#[cfg(feature = \"federation\")]\nasync fn process_federation_mint_input",
        );
        assert_contains_in_order(
            mint_public_submit,
            "process_federation_mint_input(state, &input).await?",
            "state.mint.process_mint_request(input).await",
        );

        let mint_quote_status = source_slice(
            custom_handlers,
            "pub async fn get_check_mint_custom_quote",
            "/// Batch check mint quote status",
        );
        assert_contains_in_order(
            mint_quote_status,
            ".check_mint_quotes(&[quote_id])",
            "submit_federation_mint_quote_payment_observation(&state, &quote_response)",
        );

        let batch_mint_quote_status = source_slice(
            custom_handlers,
            "pub async fn post_batch_check_mint_quote",
            "pub async fn post_melt_custom_quote",
        );
        assert_contains_in_order(
            batch_mint_quote_status,
            ".check_mint_quotes(&payload.quotes)",
            "submit_federation_mint_quote_payment_observation(&state, response)",
        );

        let mint_submit = source_slice(
            custom_handlers,
            "async fn process_federation_mint_input",
            "fn mint_quote_response_to_string",
        );
        assert_contains_in_order(
            mint_submit,
            "submit_federation_mint_operation(state, input.clone()).await?",
            "persisted_mint_share_response(operation_id, input)",
        );

        let swap_submit = source_slice(
            router_handlers,
            "async fn process_federation_swap",
            "/// Restores blind signature",
        );
        assert_contains_in_order(
            swap_submit,
            "state.submit_federation_envelope(envelope).await?",
            "persisted_swap_share_response(response.operation_id, payload)",
        );

        let swap_public_submit = source_slice(
            router_handlers,
            "pub(crate) async fn post_swap",
            "#[cfg(feature = \"federation\")]\nasync fn process_federation_swap",
        );
        assert_contains_in_order(
            swap_public_submit,
            "process_federation_swap(&state, &payload)",
            ".process_swap_request(payload)",
        );

        let melt_quote = source_slice(
            custom_handlers,
            "pub async fn post_melt_custom_quote",
            "/// Get custom payment method melt quote status",
        );
        assert_contains_in_order(
            melt_quote,
            ".ensure_federation_ready_for_public_submit()",
            ".get_melt_quote(quote_request.clone())",
        );
        assert_contains_in_order(
            melt_quote,
            ".get_melt_quote(quote_request.clone())",
            "submit_federation_melt_quote_operation(&state, quote_request, &response)",
        );

        let melt_quote_status = source_slice(
            custom_handlers,
            "pub async fn get_check_melt_custom_quote",
            "async fn process_melt_request",
        );
        assert_contains_in_order(
            melt_quote_status,
            ".check_melt_quote(&quote_id)",
            "submit_federation_melt_quote_payment_observation(&state, &quote)",
        );

        let melt_submit = source_slice(
            custom_handlers,
            "async fn process_federation_melt_request",
            "async fn federation_melt_response_from_store",
        );
        assert_contains_in_order(
            melt_submit,
            "state.submit_federation_envelope(envelope).await?",
            "federation_melt_response_from_store(state, payload.quote()).await?",
        );

        let melt_public_submit = source_slice(
            custom_handlers,
            "async fn process_melt_request",
            "#[cfg(feature = \"federation\")]\nasync fn process_federation_melt_request",
        );
        assert_contains_in_order(
            melt_public_submit,
            "process_federation_melt_request(state, payload).await?",
            "state.mint.melt(payload).await?",
        );

        let checkstate_handler = source_slice(
            router_handlers,
            "pub(crate) async fn post_check",
            "/// Mint information",
        );
        assert_contains_in_order(
            checkstate_handler,
            ".ensure_federation_ready_for_authoritative_read()",
            "state.mint.check_state(&payload).await",
        );

        let restore_handler = source_slice(
            router_handlers,
            "pub(crate) async fn post_restore",
            "#[cfg(feature = \"info-page\")]",
        );
        assert_contains_in_order(
            restore_handler,
            ".ensure_federation_ready_for_authoritative_read()",
            "state.mint.restore(payload).await",
        );

        let restore_impl = source_slice(
            mint_mod,
            "pub async fn restore",
            "/// Get the total amount issed by keyset",
        );
        assert!(restore_impl.contains("config.validate_restore_outputs(&request)?"));
        assert!(restore_impl.contains("get_federation_signature_shares_for_blinded_message"));
        for forbidden in [
            ".blind_sign(",
            ".sign_blinded_messages(",
            "blind_sign_share(",
            "auth_blind_sign(",
        ] {
            assert!(
                !restore_impl.contains(forbidden),
                "federation restore must not call `{forbidden}`"
            );
        }

        assert!(
            mint_issue_mod.contains("self.reject_standalone_federation_path(\"mint issuance\")?")
        );
        assert!(mint_swap_mod.contains("self.reject_standalone_federation_path(\"swap\")?"));
        assert!(mint_melt_mod.contains("self.reject_standalone_federation_path(\"melt\")?"));
        assert!(mint_mod.contains("self.reject_standalone_federation_path(\"raw blind signing\")?"));
    }

    fn create_test_request(prefer_header: Option<&str>) -> Request<()> {
        let mut req = Request::builder()
            .method("POST")
            .uri("/test")
            .body(())
            .unwrap();

        if let Some(header_value) = prefer_header {
            req.headers_mut().insert(
                PREFER_HEADER_KEY,
                HeaderValue::from_str(header_value).unwrap(),
            );
        }

        req
    }

    fn create_test_request_with_bytes(bytes: &[u8]) -> Request<()> {
        let mut req = Request::builder()
            .method("POST")
            .uri("/test")
            .body(())
            .unwrap();

        req.headers_mut()
            .insert(PREFER_HEADER_KEY, HeaderValue::from_bytes(bytes).unwrap());

        req
    }

    async fn create_test_state() -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]));
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .unwrap();

        let mnemonic = Mnemonic::generate(12).unwrap();
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        MintState {
            mint: Arc::new(mint),
            cache: Arc::new(HttpCache::default()),
            #[cfg(feature = "federation")]
            federation_consensus: None,
            #[cfg(feature = "federation")]
            federation_peer_sync_config: None,
            #[cfg(feature = "federation")]
            federation_sequences: None,
            #[cfg(feature = "federation")]
            federation_sync: None,
            #[cfg(feature = "federation")]
            federation_notifications: None,
            #[cfg(feature = "federation")]
            federation_finalizer: None,
            #[cfg(feature = "federation")]
            federation_private_rest_admission: None,
            #[cfg(feature = "federation")]
            federation_invalid_operation_alerts: None,
            #[cfg(feature = "federation")]
            federation_aleph_bft_pending_sequences: None,
        }
    }

    #[cfg(feature = "federation")]
    struct DelayedStatusFakeWallet {
        inner: FakeWallet,
        status_checks: AtomicUsize,
        settle_after_checks: usize,
        final_status: Option<MeltQuoteState>,
    }

    #[cfg(feature = "federation")]
    impl DelayedStatusFakeWallet {
        fn new(settle_after_checks: usize, final_status: Option<MeltQuoteState>) -> Self {
            let fee_reserve = FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            };

            Self {
                inner: FakeWallet::new(
                    fee_reserve,
                    HashMap::default(),
                    HashSet::default(),
                    0,
                    CurrencyUnit::Sat,
                ),
                status_checks: AtomicUsize::new(0),
                settle_after_checks,
                final_status,
            }
        }
    }

    #[cfg(feature = "federation")]
    #[async_trait]
    impl MintPayment for DelayedStatusFakeWallet {
        type Err = cdk_payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
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
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            let mut response = self.inner.make_payment(unit, options).await?;
            response.status = MeltQuoteState::Pending;
            response.payment_proof = None;
            Ok(response)
        }

        async fn wait_payment_event(
            &self,
        ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
            Ok(Box::pin(futures::stream::pending()))
        }

        fn is_payment_event_stream_active(&self) -> bool {
            false
        }

        fn cancel_payment_event_stream(&self) {}

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
        ) -> Result<MakePaymentResponse, Self::Err> {
            let mut response = self
                .inner
                .check_outgoing_payment(payment_identifier)
                .await?;
            let attempts = self.status_checks.fetch_add(1, Ordering::SeqCst) + 1;
            if attempts < self.settle_after_checks {
                response.status = MeltQuoteState::Pending;
                response.payment_proof = None;
                response.total_spent = Amount::new(0, CurrencyUnit::Sat);
                return Ok(response);
            }

            let Some(final_status) = self.final_status else {
                response.status = MeltQuoteState::Pending;
                response.payment_proof = None;
                response.total_spent = Amount::new(0, CurrencyUnit::Sat);
                return Ok(response);
            };

            response.status = final_status;
            if final_status != MeltQuoteState::Paid {
                response.payment_proof = None;
                response.total_spent = Amount::new(0, CurrencyUnit::Sat);
            }
            Ok(response)
        }
    }

    #[cfg(feature = "federation")]
    struct CountingFakeWallet {
        inner: FakeWallet,
        make_payment_calls: Arc<AtomicUsize>,
    }

    #[cfg(feature = "federation")]
    impl CountingFakeWallet {
        fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            let fee_reserve = FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            };
            let wallet = Self {
                inner: FakeWallet::new(
                    fee_reserve,
                    HashMap::default(),
                    HashSet::default(),
                    0,
                    CurrencyUnit::Sat,
                ),
                make_payment_calls: calls.clone(),
            };

            (Arc::new(wallet), calls)
        }
    }

    #[cfg(feature = "federation")]
    #[async_trait]
    impl MintPayment for CountingFakeWallet {
        type Err = cdk_payment::Error;

        async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
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
        ) -> Result<PaymentQuoteResponse, Self::Err> {
            self.inner.get_payment_quote(unit, options).await
        }

        async fn make_payment(
            &self,
            unit: &CurrencyUnit,
            options: OutgoingPaymentOptions,
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.make_payment_calls.fetch_add(1, Ordering::SeqCst);
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
        ) -> Result<MakePaymentResponse, Self::Err> {
            self.inner.check_outgoing_payment(payment_identifier).await
        }
    }

    #[cfg(feature = "federation")]
    fn test_federation_runtime_config() -> cdk::mint::FederationRuntimeConfig {
        let member = cdk::federation::TrustedDealerMember::new(
            cdk::federation::MemberId::new(1).expect("member id"),
            "https://mint1.example.com"
                .parse::<cdk::mint_url::MintUrl>()
                .expect("public mint url"),
            "https://fed1.example.com"
                .parse::<cdk::mint_url::MintUrl>()
                .expect("federation api url"),
            SecretKey::from_slice(&[1; 32]).expect("identity secret"),
        );
        let keyset = cdk::federation::TrustedDealerKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            std::collections::BTreeMap::from([
                (
                    Amount::from(1),
                    vec![cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[2; 32])],
                ),
                (
                    Amount::from(2),
                    vec![cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[3; 32])],
                ),
                (
                    Amount::from(4),
                    vec![cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[4; 32])],
                ),
            ]),
        );
        let bundle = cdk::federation::generate_trusted_dealer_configs(
            cdk::federation::ThresholdParams::with_bft_consensus(1, 1).expect("params"),
            vec![member],
            vec![keyset],
        )
        .expect("trusted dealer bundle");
        let private_config = bundle
            .private_configs
            .get(&cdk::federation::MemberId::new(1).expect("member id"))
            .expect("private config")
            .clone();

        cdk::mint::FederationRuntimeConfig::new(bundle.public_config, private_config)
            .expect("runtime config")
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state() -> MintState {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        create_federated_test_state_with_backend(Arc::new(fake)).await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state_with_backend(
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
    ) -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mnemonic = Mnemonic::generate(12).unwrap();
        create_federated_test_state_with_backend_db_and_seed(
            payment_processor,
            db,
            &mnemonic.to_seed_normalized(""),
        )
        .await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state_with_backend_db_and_seed(
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
        db: Arc<cdk_sqlite::MintSqliteDatabase>,
        seed: &[u8],
    ) -> MintState {
        create_federated_test_state_with_backend_db_seed_and_consensus(
            payment_processor,
            db,
            seed,
            FederationConsensusBackendKind::InMemory,
        )
        .await
    }

    #[cfg(feature = "federation")]
    async fn create_aleph_bft_federated_test_state() -> MintState {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mnemonic = Mnemonic::generate(12).unwrap();
        create_federated_test_state_with_backend_db_seed_and_consensus(
            Arc::new(fake),
            db,
            &mnemonic.to_seed_normalized(""),
            FederationConsensusBackendKind::AlephBft,
        )
        .await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_test_state_with_backend_db_seed_and_consensus(
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
        db: Arc<cdk_sqlite::MintSqliteDatabase>,
        seed: &[u8],
        consensus_backend: FederationConsensusBackendKind,
    ) -> MintState {
        let runtime_config = test_federation_runtime_config();
        let runtime_config = cdk::mint::FederationRuntimeConfig::new_with_consensus_backend(
            runtime_config.public_config().clone(),
            runtime_config.private_config().clone(),
            consensus_backend,
        )
        .expect("runtime config with consensus backend");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config.clone());
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                payment_processor,
            )
            .await
            .unwrap();

        let mint = builder.build_with_seed(db, seed).await.unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        MintState {
            mint: Arc::new(mint),
            cache: Arc::new(HttpCache::default()),
            federation_consensus: Some(Arc::new(tokio::sync::Mutex::new(
                match consensus_backend {
                    FederationConsensusBackendKind::InMemory => {
                        FederationConsensusRuntime::in_memory(
                            runtime_config.federation_id(),
                            runtime_config.member_id(),
                        )
                    }
                    FederationConsensusBackendKind::AlephBft => {
                        FederationConsensusRuntime::aleph_bft_from_finalized_items(
                            runtime_config.federation_id(),
                            runtime_config.member_id(),
                            0,
                            runtime_config.public_config().members.keys().copied(),
                            runtime_config
                                .public_config()
                                .threshold_params
                                .consensus_threshold,
                            Vec::new(),
                        )
                        .expect("AlephBFT consensus runtime")
                    }
                },
            ))),
            federation_peer_sync_config: Some(crate::FederationPeerSyncConfig::default()),
            federation_sequences: Some(Arc::new(tokio::sync::Mutex::new(
                cdk::federation::FederationRestSequenceTracker::new(),
            ))),
            federation_sync: Some(Arc::new(tokio::sync::Mutex::new(
                crate::FederationSyncState::for_consensus_backend(consensus_backend),
            ))),
            federation_notifications: Some(Arc::new(
                crate::FederationOperationNotifications::default(),
            )),
            federation_finalizer: None,
            federation_private_rest_admission: Some(Arc::new(tokio::sync::Semaphore::new(
                crate::FederationPeerSyncConfig::default().private_rest_inbound_concurrency_limit,
            ))),
            federation_invalid_operation_alerts: Some(Arc::new(tokio::sync::Mutex::new(
                crate::FederationInvalidOperationAlertTracker::default(),
            ))),
            federation_aleph_bft_pending_sequences: Some(Arc::new(tokio::sync::Mutex::new(
                HashMap::new(),
            ))),
        }
    }

    #[cfg(feature = "federation")]
    async fn restart_federated_test_state_from_db(
        db_file: &TempMintDbFile,
        seed: &[u8],
    ) -> MintState {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("restarted mint db"),
        );
        let mut state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, seed).await;
        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
        state.federation_consensus = consensus;
        state.federation_sequences = sequences;
        state
    }

    #[cfg(feature = "federation")]
    async fn restart_federated_test_state_from_db_with_backend(
        db_file: &TempMintDbFile,
        seed: &[u8],
        payment_processor: Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync + 'static>,
    ) -> MintState {
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("restarted mint db"),
        );
        let mut state =
            create_federated_test_state_with_backend_db_and_seed(payment_processor, db, seed).await;
        let (consensus, sequences) = crate::create_federation_runtime_state(&state.mint)
            .await
            .expect("recreate federation runtime state from sqlite");
        state.federation_consensus = consensus;
        state.federation_sequences = sequences;
        state
    }

    #[cfg(feature = "federation")]
    fn limit_federation_response_cache_to_one(state: &mut MintState) {
        state.federation_peer_sync_config = Some(crate::FederationPeerSyncConfig {
            response_cache_limit: 1,
            ..crate::FederationPeerSyncConfig::default()
        });
    }

    #[cfg(feature = "federation")]
    async fn federation_operation_kinds(
        state: &MintState,
    ) -> Vec<cdk::federation::FederationOperationKind> {
        let consensus = state
            .federation_consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        consensus
            .journal()
            .entries()
            .iter()
            .map(|entry| entry.envelope.operation.kind())
            .collect()
    }

    #[cfg(feature = "federation")]
    async fn first_federation_operation_id(
        state: &MintState,
        kind: cdk::federation::FederationOperationKind,
    ) -> cdk::federation::FederationOperationId {
        state
            .mint
            .localstore()
            .get_federation_journal_entries()
            .await
            .expect("federation journal lookup")
            .into_iter()
            .find(|entry| entry.envelope.operation.kind() == kind)
            .expect("federation operation entry")
            .operation_id
    }

    #[cfg(feature = "federation")]
    async fn apply_peer_catch_up(state: &MintState, peer_consensus: &InMemoryFederationConsensus) {
        let local_consensus = state
            .federation_consensus
            .as_ref()
            .expect("federation consensus");
        let catch_up_response = peer_consensus
            .catch_up_checked(&local_consensus.lock().await.catch_up_request())
            .expect("peer catch-up response");

        state
            .apply_federation_catch_up_response(&catch_up_response)
            .await
            .expect("apply peer catch-up response");
    }

    async fn create_test_state_with_custom_methods(methods: &[&str]) -> MintState {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let mut builder = MintBuilder::new(db.clone()).with_batch_minting(
            Some(10),
            Some(methods.iter().map(|method| method.to_string()).collect()),
        );

        let custom_methods = methods
            .iter()
            .map(|method| (method.to_string(), "{}".to_string()))
            .collect::<HashMap<_, _>>();

        for method in methods {
            let fake = FakeWallet::new(
                FeeReserve {
                    min_fee_reserve: 1.into(),
                    percent_fee_reserve: 0.0,
                },
                HashMap::default(),
                HashSet::default(),
                0,
                CurrencyUnit::Sat,
            )
            .with_custom_payment_methods(custom_methods.clone());

            builder
                .add_payment_processor(
                    CurrencyUnit::Sat,
                    PaymentMethod::Custom(method.to_string()),
                    MintMeltLimits::new(1, 10_000),
                    Arc::new(fake),
                )
                .await
                .unwrap();
        }

        let mnemonic = Mnemonic::generate(12).unwrap();
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .unwrap();
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .unwrap();
        mint.start().await.unwrap();

        MintState {
            mint: Arc::new(mint),
            cache: Arc::new(HttpCache::default()),
            #[cfg(feature = "federation")]
            federation_consensus: None,
            #[cfg(feature = "federation")]
            federation_peer_sync_config: None,
            #[cfg(feature = "federation")]
            federation_sequences: None,
            #[cfg(feature = "federation")]
            federation_sync: None,
            #[cfg(feature = "federation")]
            federation_notifications: None,
            #[cfg(feature = "federation")]
            federation_finalizer: None,
            #[cfg(feature = "federation")]
            federation_private_rest_admission: None,
            #[cfg(feature = "federation")]
            federation_invalid_operation_alerts: None,
            #[cfg(feature = "federation")]
            federation_aleph_bft_pending_sequences: None,
        }
    }

    async fn create_custom_quote(state: &MintState, method: &str, amount: u64) -> QuoteId {
        let quote = state
            .mint
            .get_mint_quote(cdk::mint::MintQuoteRequest::Custom {
                method: PaymentMethod::Custom(method.to_string()),
                request: MintQuoteCustomRequest {
                    amount: Amount::from(amount),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                    extra: Value::Null,
                },
            })
            .await
            .unwrap();

        quote.quote().clone()
    }

    async fn create_paid_bolt11_quote(state: &MintState) -> QuoteId {
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .unwrap()
            .try_into()
            .unwrap();

        for _ in 0..100 {
            let check = state
                .mint
                .check_mint_quotes(std::slice::from_ref(&quote.quote))
                .await
                .unwrap();
            if let MintQuoteResponse::Bolt11(q) = &check[0] {
                if q.state == MintQuoteState::Paid {
                    return quote.quote;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        panic!("bolt11 quote was not paid by the fake wallet");
    }

    fn outputs_for_amount(state: &MintState, amount: u64) -> Vec<BlindedMessage> {
        outputs_for_amount_with_label(
            state,
            amount,
            &format!("axum federated mint output {amount}"),
        )
    }

    fn outputs_for_amount_with_label(
        state: &MintState,
        amount: u64,
        label: &str,
    ) -> Vec<BlindedMessage> {
        #[cfg(feature = "federation")]
        let keyset_id = match state.mint.federation_config() {
            Some(config) => config.public_config().keysets[0].id,
            None => *state
                .mint
                .get_active_keysets()
                .get(&CurrencyUnit::Sat)
                .expect("active sat keyset"),
        };
        #[cfg(not(feature = "federation"))]
        let keyset_id = *state
            .mint
            .get_active_keysets()
            .get(&CurrencyUnit::Sat)
            .expect("active sat keyset");

        let blinded_secret = cdk::nuts::PublicKey::from(
            cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(label.as_bytes()),
        );

        vec![BlindedMessage::new(
            Amount::from(amount),
            keyset_id,
            blinded_secret,
        )]
    }

    #[cfg(feature = "federation")]
    fn non_v3_output_for_state(state: &MintState, amount: Amount, label: &[u8]) -> BlindedMessage {
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let non_v3_keyset_id = cdk::nuts::Id::v2_from_data(
            &keyset.keys(),
            &keyset.unit,
            keyset.input_fee_ppk,
            keyset.final_expiry,
        );
        let blinded_secret =
            cdk::nuts::PublicKey::from(cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(label));

        BlindedMessage::new(amount, non_v3_keyset_id, blinded_secret)
    }

    #[cfg(feature = "federation")]
    fn non_v3_change_output_for_state(state: &MintState) -> BlindedMessage {
        non_v3_output_for_state(
            state,
            Amount::ZERO,
            b"axum federated melt non-v3 change output",
        )
    }

    #[cfg(feature = "federation")]
    fn aggregate_secret_for_amount(amount: u64) -> cdk::nuts::nut01::BlsSecretKey {
        let byte = match amount {
            1 => 2,
            2 => 3,
            4 => 4,
            _ => panic!("missing aggregate secret fixture for amount {amount}"),
        };

        cdk::nuts::nut01::BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    #[cfg(feature = "federation")]
    fn proof_for_amount(state: &MintState, amount: u64, label: &str) -> cdk::nuts::Proof {
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let secret = cdk::secret::Secret::new(format!("federated-axum-proof-{label}"));
        let signature = cdk::nuts::nut01::BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        cdk::nuts::Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            cdk::nuts::PublicKey::from(signature),
        )
    }

    #[cfg(feature = "federation")]
    fn threshold_member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("member id")
    }

    #[cfg(feature = "federation")]
    fn threshold_identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("identity secret")
    }

    #[cfg(feature = "federation")]
    fn threshold_member(
        member_id: MemberId,
        public_mint_url: cdk::mint_url::MintUrl,
        federation_api_url: cdk::mint_url::MintUrl,
    ) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            public_mint_url,
            federation_api_url,
            threshold_identity_secret(member_id),
        )
    }

    #[cfg(feature = "federation")]
    fn threshold_bundle(
        member_urls: &[(MemberId, cdk::mint_url::MintUrl)],
    ) -> TrustedDealerConfigBundle {
        let members = member_urls
            .iter()
            .map(|(member_id, url)| threshold_member(*member_id, url.clone(), url.clone()))
            .collect::<Vec<_>>();

        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            members,
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (
                        Amount::from(1),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[2; 32]),
                            BlsSecretKey::from_reduced_bytes(&[12; 32]),
                        ],
                    ),
                    (
                        Amount::from(2),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[3; 32]),
                            BlsSecretKey::from_reduced_bytes(&[13; 32]),
                        ],
                    ),
                    (
                        Amount::from(4),
                        vec![
                            BlsSecretKey::from_reduced_bytes(&[4; 32]),
                            BlsSecretKey::from_reduced_bytes(&[14; 32]),
                        ],
                    ),
                ]),
            )],
        )
        .expect("trusted dealer threshold bundle")
    }

    #[cfg(feature = "federation")]
    fn threshold_rotation_bundle(
        member_urls: &[(MemberId, cdk::mint_url::MintUrl)],
    ) -> TrustedDealerConfigBundle {
        let members = member_urls
            .iter()
            .map(|(member_id, url)| threshold_member(*member_id, url.clone(), url.clone()))
            .collect::<Vec<_>>();

        generate_trusted_dealer_configs(
            ThresholdParams::with_bft_consensus(3, 2).expect("threshold params"),
            members,
            vec![
                TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    true,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[2; 32]),
                                BlsSecretKey::from_reduced_bytes(&[12; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[3; 32]),
                                BlsSecretKey::from_reduced_bytes(&[13; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[4; 32]),
                                BlsSecretKey::from_reduced_bytes(&[14; 32]),
                            ],
                        ),
                    ]),
                ),
                TrustedDealerKeysetSpec::new(
                    CurrencyUnit::Sat,
                    false,
                    0,
                    None,
                    BTreeMap::from([
                        (
                            Amount::from(1),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[5; 32]),
                                BlsSecretKey::from_reduced_bytes(&[15; 32]),
                            ],
                        ),
                        (
                            Amount::from(2),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[6; 32]),
                                BlsSecretKey::from_reduced_bytes(&[16; 32]),
                            ],
                        ),
                        (
                            Amount::from(4),
                            vec![
                                BlsSecretKey::from_reduced_bytes(&[7; 32]),
                                BlsSecretKey::from_reduced_bytes(&[17; 32]),
                            ],
                        ),
                    ]),
                ),
            ],
        )
        .expect("trusted dealer threshold rotation bundle")
    }

    #[cfg(feature = "federation")]
    fn active_keyset(bundle: &TrustedDealerConfigBundle) -> cdk::federation::FederatedKeyset {
        bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| keyset.active)
            .expect("active keyset")
            .clone()
    }

    #[cfg(feature = "federation")]
    fn inactive_keyset(bundle: &TrustedDealerConfigBundle) -> cdk::federation::FederatedKeyset {
        bundle
            .public_config
            .keysets
            .iter()
            .find(|keyset| !keyset.active)
            .expect("inactive keyset")
            .clone()
    }

    #[cfg(feature = "federation")]
    fn activated_rotation_keyset(
        bundle: &TrustedDealerConfigBundle,
    ) -> cdk::federation::FederatedKeyset {
        let mut keyset = inactive_keyset(bundle);
        keyset.active = true;
        keyset
    }

    #[cfg(feature = "federation")]
    fn rotated_public_config(bundle: &TrustedDealerConfigBundle) -> FederationConfig {
        bundle
            .public_config
            .apply_keyset_rotation(&FederationKeysetRotationOperation {
                keyset: activated_rotation_keyset(bundle),
            })
            .expect("rotated public config")
    }

    #[cfg(feature = "federation")]
    async fn build_threshold_member_mint(
        public_config: FederationConfig,
        private_config: MemberConfigPrivate,
    ) -> Arc<Mint> {
        let db = Arc::new(cdk_sqlite::mint::memory::empty().await.unwrap());
        let runtime_config = cdk::mint::FederationRuntimeConfig::new(public_config, private_config)
            .expect("runtime config");
        let mut builder = MintBuilder::new(db.clone())
            .with_batch_minting(Some(10), Some(vec![KnownMethod::Bolt11.to_string()]))
            .with_federation_config(runtime_config);
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        builder
            .add_payment_processor(
                CurrencyUnit::Sat,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintMeltLimits::new(1, 10_000),
                Arc::new(fake),
            )
            .await
            .expect("payment processor");

        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let mint = builder
            .build_with_seed(db, &mnemonic.to_seed_normalized(""))
            .await
            .expect("mint");
        mint.set_quote_ttl(QuoteTTL::new(10_000, 10_000))
            .await
            .expect("quote ttl");
        mint.start().await.expect("mint start");
        Arc::new(mint)
    }

    #[cfg(feature = "federation")]
    async fn local_member_listeners() -> (
        Vec<tokio::net::TcpListener>,
        Vec<(MemberId, cdk::mint_url::MintUrl)>,
    ) {
        let mut listeners = Vec::new();
        let mut member_urls = Vec::new();
        for id in 1..=3 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind local member listener");
            let url = cdk::mint_url::MintUrl::from_str(&format!(
                "http://{}/",
                listener.local_addr().expect("local address")
            ))
            .expect("member URL");
            member_urls.push((threshold_member_id(id), url));
            listeners.push(listener);
        }
        (listeners, member_urls)
    }

    #[cfg(feature = "federation")]
    async fn spawn_member_routers(
        mints: &[Arc<Mint>],
        listeners: Vec<tokio::net::TcpListener>,
    ) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();
        for (mint, listener) in mints.iter().cloned().zip(listeners) {
            let router = crate::create_mint_router(mint, vec![KnownMethod::Bolt11.to_string()])
                .await
                .expect("mint router");
            handles.push(tokio::spawn(async move {
                axum::serve(listener, router).await.expect("member router");
            }));
        }
        handles
    }

    #[cfg(feature = "federation")]
    fn appended_entry(
        journal: &mut InMemoryFederationJournal,
        envelope: FederationOperationEnvelope,
    ) -> FederationJournalEntry {
        journal.append(envelope).expect("append").entry().clone()
    }

    #[cfg(feature = "federation")]
    async fn persist_and_apply_entry(mint: &Mint, entry: &FederationJournalEntry) {
        let mut tx = mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        tx.add_federation_journal_entry(entry)
            .await
            .expect("persist journal entry");
        tx.commit().await.expect("commit journal entry");

        mint.apply_federation_journal_entries(std::slice::from_ref(entry))
            .await
            .expect("apply journal entry");
    }

    #[cfg(feature = "federation")]
    async fn add_restore_signature_share_to_mint(
        mint: &Mint,
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        operation_id: FederationOperationId,
        output: &BlindedMessage,
    ) {
        let share_set = private_config
            .sign_blinded_messages(public_config, operation_id, std::slice::from_ref(output))
            .expect("restore signature share set");
        let signatures = share_set
            .shares
            .into_iter()
            .map(|share| BlindSignature {
                amount: share.amount,
                keyset_id: share.keyset_id,
                c: cdk::nuts::PublicKey::from(share.share.signature()),
                dleq: None,
            })
            .collect::<Vec<_>>();
        let mut tx = mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        tx.add_blind_signatures(&[output.blinded_secret], &signatures, None)
            .await
            .expect("add restore signature share");
        tx.commit().await.expect("commit restore signature share");
    }

    #[cfg(feature = "federation")]
    async fn add_paid_bolt11_quote_to_mint(mint: &Mint, quote_id: &QuoteId, amount: u64) {
        let amount = Amount::new(amount, CurrencyUnit::Sat);
        let created_time = 1_700_000_000;
        let mut quote = cdk::mint::MintQuote::new(
            Some(quote_id.clone()),
            format!("invoice-{quote_id}"),
            CurrencyUnit::Sat,
            Some(amount.clone()),
            created_time + 10_000,
            cdk::cdk_payment::PaymentIdentifier::QuoteId(quote_id.clone()),
            None,
            Amount::new(0, CurrencyUnit::Sat),
            Amount::new(0, CurrencyUnit::Sat),
            PaymentMethod::Known(KnownMethod::Bolt11),
            created_time,
            Vec::new(),
            Vec::new(),
            None,
        );
        quote
            .add_payment(amount, format!("payment-{quote_id}"), None)
            .expect("add payment");
        let mut tx = mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        let mut quote = tx.add_mint_quote(quote).await.expect("add mint quote");
        tx.update_mint_quote(&mut quote)
            .await
            .expect("update paid mint quote");
        tx.commit().await.expect("commit quote");
    }

    #[cfg(feature = "federation")]
    async fn clone_primary_melt_quote_to_members(mints: &[Arc<Mint>], quote_id: &QuoteId) {
        let quote = mints[0]
            .localstore()
            .get_melt_quote(quote_id)
            .await
            .expect("melt quote lookup")
            .expect("primary melt quote");

        for mint in mints.iter().skip(1) {
            let mut tx = mint
                .localstore()
                .begin_transaction()
                .await
                .expect("begin tx");
            tx.add_melt_quote(quote.clone())
                .await
                .expect("add melt quote");
            tx.commit().await.expect("commit melt quote");
        }
    }

    #[cfg(feature = "federation")]
    fn blinded_output(
        keyset: &cdk::federation::FederatedKeyset,
        amount: u64,
        label: &str,
    ) -> BlindedMessage {
        BlindedMessage::new(
            Amount::from(amount),
            keyset.id,
            cdk::nuts::PublicKey::from(BlsG1PublicKey::hash_to_curve(label.as_bytes())),
        )
    }

    #[cfg(feature = "federation")]
    fn blinded_change_output(
        keyset: &cdk::federation::FederatedKeyset,
        amount_label: &str,
    ) -> BlindedMessage {
        let mut output = blinded_output(keyset, 2, amount_label);
        output.amount = Amount::ZERO;
        output
    }

    #[cfg(feature = "federation")]
    fn proof_for_keyset(
        keyset: &cdk::federation::FederatedKeyset,
        amount: u64,
        label: &str,
    ) -> Proof {
        let secret = Secret::new(format!("federated-axum-http-proof-{label}"));
        let signature = BlsG1PublicKey::hash_to_curve(secret.as_bytes())
            .mul(&aggregate_secret_for_amount(amount));

        Proof::new(
            Amount::from(amount),
            keyset.id,
            secret,
            cdk::nuts::PublicKey::from(signature),
        )
    }

    #[cfg(feature = "federation")]
    fn assert_aggregate_signatures_verify(
        public_config: &FederationConfig,
        outputs: &[BlindedMessage],
        signatures: &[BlindSignature],
    ) {
        assert_eq!(signatures.len(), outputs.len());
        for (signature, output) in signatures.iter().zip(outputs) {
            let keyset = public_config
                .keysets
                .iter()
                .find(|keyset| keyset.id == signature.keyset_id)
                .expect("signature keyset");
            let aggregate_key = keyset
                .keys()
                .amount_key(signature.amount)
                .expect("amount key");
            cdk::dhke::verify_bls_blind_signature(
                aggregate_key,
                signature.c,
                output.blinded_secret,
            )
            .expect("aggregate signature verifies");
        }
    }

    #[cfg(feature = "federation")]
    fn threshold_http_member_connectors(
        public_config: &FederationConfig,
    ) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
        public_config
            .members
            .iter()
            .map(|(member_id, member)| {
                (
                    *member_id,
                    Arc::new(cdk::HttpClient::new(member.public_mint_url.clone(), None))
                        as Arc<dyn MintConnector + Send + Sync>,
                )
            })
            .collect()
    }

    #[cfg(feature = "federation")]
    async fn wait_for_federated_http_members(connector: &FederatedMintConnector) {
        for _ in 0..100 {
            if connector.get_mint_keys().await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        connector
            .get_mint_keys()
            .await
            .expect("federated HTTP members ready");
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_quote(state: &MintState, amount_msat: u64) -> QuoteId {
        create_federated_melt_quote_with_description(state, amount_msat, "".to_string()).await
    }

    #[cfg(feature = "federation")]
    async fn create_federated_melt_quote_with_description(
        state: &MintState,
        amount_msat: u64,
        description: String,
    ) -> QuoteId {
        let invoice = create_fake_invoice(amount_msat, description);
        let response = state
            .mint
            .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote");

        match response {
            MeltQuoteCreateResponse::Bolt11(response) => response.quote,
            _ => panic!("expected bolt11 melt quote"),
        }
    }

    #[cfg(feature = "federation")]
    async fn set_federated_melt_quote_state(
        state: &MintState,
        quote_id: &QuoteId,
        target_state: MeltQuoteState,
        payment_proof: Option<&str>,
    ) {
        let mut tx = state
            .mint
            .localstore()
            .begin_transaction()
            .await
            .expect("begin tx");
        let mut quote = tx
            .get_melt_quote(quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        if target_state == MeltQuoteState::Paid {
            tx.update_melt_quote_state(&mut quote, MeltQuoteState::Pending, None)
                .await
                .expect("mark melt quote pending before paid");
        }
        tx.update_melt_quote_state(&mut quote, target_state, payment_proof.map(str::to_string))
            .await
            .expect("update melt quote state");
        tx.commit().await.expect("commit melt quote state");
    }

    async fn response_error_detail(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    #[cfg(feature = "federation")]
    async fn assert_aleph_bft_http_response_not_ready(response: Response) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_error_detail(response).await;
        assert!(
            body.contains("public federation submissions are disabled"),
            "AlephBFT public route should fail closed before queueing: {body}"
        );
    }

    #[cfg(feature = "federation")]
    async fn assert_http_response_not_ready_contains(response: Response, expected: &str) {
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_error_detail(response).await;
        assert!(
            body.contains(expected),
            "public route should fail closed with `{expected}` in the response: {body}"
        );
    }

    #[cfg(feature = "federation")]
    fn assert_aleph_bft_error_not_ready(error: &cdk::Error) {
        match error {
            cdk::Error::Custom(detail)
                if detail.contains("public federation submissions are disabled") => {}
            _ => panic!("AlephBFT public submit should fail closed, got: {error}"),
        }
    }

    #[cfg(feature = "federation")]
    fn assert_error_not_ready_contains(error: &cdk::Error, expected: &str) {
        match error {
            cdk::Error::Custom(detail) if detail.contains(expected) => {}
            _ => panic!("public route should fail closed with `{expected}`, got: {error}"),
        }
    }

    #[cfg(feature = "federation")]
    async fn assert_no_aleph_bft_public_submit_work(state: &MintState) {
        assert!(
            federation_operation_kinds(state).await.is_empty(),
            "halted AlephBFT public route must not append a journal entry"
        );
        let consensus = state
            .federation_consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        assert_eq!(
            consensus.queued_len(),
            0,
            "halted AlephBFT public route must not queue an item"
        );
        assert_eq!(
            consensus.peer_status().operation_count,
            0,
            "halted AlephBFT public route must not accept an operation"
        );
        drop(consensus);
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "halted AlephBFT public route must not register a response waiter"
        );
    }

    #[cfg(feature = "federation")]
    async fn assert_no_federation_public_submit_work(state: &MintState) {
        assert!(
            federation_operation_kinds(state).await.is_empty(),
            "not-ready federation public route must not append a journal entry"
        );
        let consensus = state
            .federation_consensus
            .as_ref()
            .expect("federation consensus")
            .lock()
            .await;
        assert_eq!(
            consensus.queued_len(),
            0,
            "not-ready federation public route must not queue an item"
        );
        assert_eq!(
            consensus.peer_status().operation_count,
            0,
            "not-ready federation public route must not accept an operation"
        );
        drop(consensus);
        assert_eq!(
            state.federation_pending_response_waiter_count(),
            0,
            "not-ready federation public route must not register a response waiter"
        );
    }

    #[tokio::test]
    async fn test_prefer_header_respond_async() {
        let req = create_test_request(Some("respond-async"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_respond_async_with_other_values() {
        let req = create_test_request(Some("respond-async; wait=10"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_case_insensitive() {
        let req = create_test_request(Some("RESPOND-ASYNC"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_no_respond_async() {
        let req = create_test_request(Some("wait=10"));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_missing() {
        let req = create_test_request(None);
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().respond_async);
    }

    #[tokio::test]
    async fn test_prefer_header_invalid_value() {
        let req = create_test_request_with_bytes(&[0xFF, 0xFE]);
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err());
        let (status, message) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(message, "Invalid Prefer header value");
    }

    #[tokio::test]
    async fn test_prefer_header_empty_value() {
        let req = create_test_request(Some(""));
        let (mut parts, _) = req.into_parts();

        let result = PreferHeader::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().respond_async);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_mint_custom_quote_records_federation_quote_operation() {
        let state = create_federated_test_state().await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .unwrap();

        let result = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await;

        assert!(result.is_ok(), "mint quote creation should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuote]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_mint_custom_quote_aleph_bft_public_route_fails_closed_before_quote_creation() {
        let state = create_aleph_bft_federated_test_state().await;
        let payload = serde_json::to_value(MintQuoteBolt11Request {
            amount: Amount::from(2u64),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        })
        .expect("mint quote payload");

        let error = post_mint_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await
        .expect_err("AlephBFT public mint quote should fail closed before local quote creation");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state).await;
        assert!(
            state
                .mint
                .localstore()
                .get_mint_quotes()
                .await
                .expect("mint quotes lookup")
                .is_empty(),
            "halted AlephBFT public mint quote route must not persist a local quote"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_melt_custom_quote_records_federation_quote_operation() {
        let state = create_federated_test_state().await;
        let payload = serde_json::to_value(MeltQuoteBolt11Request {
            request: create_fake_invoice(1000, "".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        })
        .expect("melt quote payload");

        let result = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await;

        assert!(result.is_ok(), "melt quote creation should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuote]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_melt_custom_quote_aleph_bft_public_route_fails_closed_before_quote_creation() {
        let state = create_aleph_bft_federated_test_state().await;
        let payload = serde_json::to_value(MeltQuoteBolt11Request {
            request: create_fake_invoice(1000, "aleph-bft-halted-melt-quote".to_string()),
            unit: CurrencyUnit::Sat,
            options: None,
        })
        .expect("melt quote payload");

        let error = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(payload),
        )
        .await
        .expect_err("AlephBFT public melt quote should fail closed before local quote creation");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state).await;
        assert!(
            state
                .mint
                .localstore()
                .get_melt_quotes()
                .await
                .expect("melt quotes lookup")
                .is_empty(),
            "halted AlephBFT public melt quote route must not persist a local quote"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_records_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(result.is_ok(), "mint quote check should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_skips_duplicate_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id.clone()],
            }),
        )
        .await;

        assert!(result.is_ok(), "initial mint quote check should succeed");

        let (finalizer, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let mut duplicate_state = state.clone();
        duplicate_state.federation_finalizer = Some(finalizer);

        let duplicate = post_batch_check_mint_quote(
            AuthHeader::None,
            State(duplicate_state),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(
            duplicate.is_ok(),
            "duplicate mint quote check should use the recorded observation"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_skips_unpaid_federation_payment_observation() {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            60,
            CurrencyUnit::Sat,
        );
        let state = create_federated_test_state_with_backend(Arc::new(fake)).await;
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote.quote],
            }),
        )
        .await;

        assert!(result.is_ok(), "unpaid mint quote check should succeed");
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "unpaid mint quote checks should not submit payment observations"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn mint_payment_observer_round_submits_paid_quote_observation() {
        let state = create_federated_test_state().await;
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");

        let mut submitted = 0;
        for _ in 0..10 {
            submitted = submit_federation_mint_quote_payment_observation_round(&state)
                .await
                .expect("observer round");
            if submitted == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(submitted, 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
        let checked = state
            .mint
            .check_mint_quotes(std::slice::from_ref(&quote.quote))
            .await
            .expect("stored quote status");
        assert_eq!(checked[0].state(), Some(cdk::nuts::MintQuoteState::Paid));

        let duplicate = submit_federation_mint_quote_payment_observation_round(&state)
            .await
            .expect("duplicate observer round");
        assert_eq!(duplicate, 0);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn mint_payment_observer_round_skips_consensus_finalized_quote() {
        let state = create_federated_test_state().await;
        let quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");
        let config = state
            .mint
            .federation_config()
            .expect("federation config")
            .clone();
        state
            .submit_federation_payment_observation_envelope(config.mint_quote_payment_observation(
                quote.quote.clone(),
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintQuoteState::Paid,
                Amount::from(2),
                Amount::ZERO,
                Some("settled-by-consensus".to_string()),
            ))
            .await
            .expect("payment observation accepted");
        let stored_quote = state
            .mint
            .localstore()
            .get_mint_quote(&quote.quote)
            .await
            .expect("quote lookup")
            .expect("quote");
        assert_eq!(stored_quote.state(), MintQuoteState::Paid);

        let duplicate = submit_federation_mint_quote_payment_observation_round(&state)
            .await
            .expect("observer round");

        assert_eq!(duplicate, 0);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn mint_payment_observer_round_skips_unpaid_quotes() {
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            60,
            CurrencyUnit::Sat,
        );
        let state = create_federated_test_state_with_backend(Arc::new(fake)).await;
        let _quote: MintQuoteBolt11Response<QuoteId> = state
            .mint
            .get_mint_quote(
                MintQuoteBolt11Request {
                    amount: Amount::from(2u64),
                    unit: CurrencyUnit::Sat,
                    description: None,
                    pubkey: None,
                }
                .into(),
            )
            .await
            .expect("mint quote")
            .try_into()
            .expect("bolt11 quote");

        let submitted = submit_federation_mint_quote_payment_observation_round(&state)
            .await
            .expect("observer round");

        assert_eq!(submitted, 0);
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "unpaid observer rounds should not submit payment observations"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_batch_check_mint_quote_records_observation_while_sync_not_ready() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        state
            .mark_federation_verifying_session_outcome(MemberId::new(2).unwrap(), 1)
            .await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(
            result.is_ok(),
            "mint quote check should still record an internal payment observation"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MintQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_records_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        set_federated_melt_quote_state(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("preimage-terminal"),
        )
        .await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(result.is_ok(), "melt quote check should succeed");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_records_observation_while_sync_not_ready() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        set_federated_melt_quote_state(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("preimage-sync-not-ready"),
        )
        .await;
        state
            .mark_federation_verifying_session_outcome(MemberId::new(2).expect("member id"), 1)
            .await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        if let Err(response) = result {
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("error response body");
            panic!(
                "melt quote check should still record an internal payment observation: {} {}",
                status,
                String::from_utf8_lossy(&body)
            );
        }
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_skips_duplicate_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        set_federated_melt_quote_state(
            &state,
            &quote_id,
            MeltQuoteState::Paid,
            Some("preimage-duplicate"),
        )
        .await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id.clone())),
        )
        .await;

        assert!(result.is_ok(), "initial melt quote check should succeed");

        let (finalizer, receiver) = tokio::sync::mpsc::channel(1);
        drop(receiver);
        let mut duplicate_state = state.clone();
        duplicate_state.federation_finalizer = Some(finalizer);

        let duplicate = get_check_melt_custom_quote(
            AuthHeader::None,
            State(duplicate_state),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(
            duplicate.is_ok(),
            "duplicate melt quote check should use the recorded observation"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::MeltQuotePayment]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn get_check_melt_custom_quote_skips_non_terminal_federation_payment_observation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        set_federated_melt_quote_state(&state, &quote_id, MeltQuoteState::Pending, None).await;

        let result = get_check_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path(("bolt11".to_string(), quote_id)),
        )
        .await;

        assert!(result.is_ok(), "pending melt quote check should succeed");
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "pending melt quote checks should not submit payment observations"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federated_mint_advertises_aggregate_keysets() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let active = state.mint.get_active_keysets();
        let pubkeys = state.mint.pubkeys();
        let keysets = state.mint.keysets();
        let specific = state
            .mint
            .keyset_pubkeys(&keyset.id)
            .expect("federation keyset pubkeys");

        assert_eq!(active.get(&CurrencyUnit::Sat), Some(&keyset.id));
        assert_eq!(pubkeys.keysets.len(), 1);
        assert_eq!(pubkeys.keysets[0].id, keyset.id);
        assert_eq!(pubkeys.keysets[0].keys, keyset.keys());
        assert_eq!(
            pubkeys.keysets[0].id.get_version(),
            cdk::nuts::KeySetVersion::Version02
        );
        assert_eq!(keysets.keysets.len(), 1);
        assert_eq!(keysets.keysets[0].id, keyset.id);
        assert_eq!(specific.keysets[0], pubkeys.keysets[0]);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn submit_batch_mint_records_federation_mint_operation() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount(&state, 2),
            signatures: None,
        };

        submit_federation_mint_operation(&state, cdk::mint::MintInput::Batch(batch_request))
            .await
            .expect("federation mint operation should be accepted");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn submit_batch_mint_rejects_non_v3_outputs_before_consensus() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: vec![non_v3_output_for_state(
                &state,
                Amount::from(2),
                b"axum federated mint non-v3 output",
            )],
            signatures: None,
        };

        let error =
            submit_federation_mint_operation(&state, cdk::mint::MintInput::Batch(batch_request))
                .await
                .expect_err("non-v3 mint output should be rejected");

        assert!(matches!(
            error,
            cdk::Error::Federation(
                cdk::federation::FederationError::InvalidFederatedKeysetVersion { .. }
            )
        ));
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "invalid federated mint outputs must fail before consensus"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn cache_post_batch_mint_returns_federation_signature_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount(&state, 2);
        let blinded_secret = outputs[0]
            .blinded_secret
            .as_bls_g1()
            .expect("blinded output should be BLS");
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs.clone(),
            signatures: None,
        };

        let response = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request),
        )
        .await
        .expect("federation batch mint should return share response");
        let signature = response
            .signatures
            .first()
            .expect("one federation signature share");
        let signature_share = signature.c.as_bls_g1().expect("share should be BLS G1");
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let public_share = keyset
            .public_shares
            .get(&config.member_id())
            .expect("member public shares")
            .get(&Amount::from(2))
            .expect("amount public share");
        let share =
            BlsBlindSignatureShare::new(config.member_id().to_bls_signer_id(), signature_share);

        assert_eq!(response.signatures.len(), 1);
        assert_eq!(signature.amount, Amount::from(2));
        assert_eq!(signature.keyset_id, outputs[0].keyset_id);
        assert!(signature.dleq.is_none());
        assert!(
            verify_blind_signature_share(&share, &blinded_secret, public_share)
                .expect("share verification")
        );
        let operation_id =
            first_federation_operation_id(&state, cdk::federation::FederationOperationKind::Mint)
                .await;
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("persisted mint share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].member_id, config.member_id());
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(share_records[0].keyset_id, signature.keyset_id);
        assert_eq!(share_records[0].amount, signature.amount);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            signature.c
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn cache_post_batch_mint_aleph_bft_public_route_fails_closed_before_queueing() {
        let state = create_aleph_bft_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount(&state, 2),
            signatures: None,
        };

        let error = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(request),
        )
        .await
        .expect_err("AlephBFT public mint should fail closed before real finality exists");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state).await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_batch_mint_returns_persisted_signature_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount(&state, 2);
        let input = cdk::mint::MintInput::Batch(BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs,
            signatures: None,
        });

        let first = process_federation_mint_input(&state, &input)
            .await
            .expect("first federation mint submit")
            .expect("first federation mint response");
        let duplicate = process_federation_mint_input(&state, &input)
            .await
            .expect("duplicate federation mint submit")
            .expect("duplicate federation mint response");

        assert_eq!(duplicate, first);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_mint_after_cache_eviction_returns_persisted_shares() {
        let mut state = create_federated_test_state().await;
        limit_federation_response_cache_to_one(&mut state);
        let first_quote_id = create_paid_bolt11_quote(&state).await;
        let first_outputs = outputs_for_amount_with_label(&state, 2, "mint-cache-eviction-first");
        let first_input = cdk::mint::MintInput::Batch(BatchMintRequest {
            quotes: vec![first_quote_id],
            quote_amounts: None,
            outputs: first_outputs,
            signatures: None,
        });

        let first = process_federation_mint_input(&state, &first_input)
            .await
            .expect("first federation mint submit")
            .expect("first federation mint response");
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let second_quote_id = create_paid_bolt11_quote(&state).await;
        let second_outputs = outputs_for_amount_with_label(&state, 2, "mint-cache-eviction-second");
        let second_input = cdk::mint::MintInput::Batch(BatchMintRequest {
            quotes: vec![second_quote_id],
            quote_amounts: None,
            outputs: second_outputs,
            signatures: None,
        });
        let second = process_federation_mint_input(&state, &second_input)
            .await
            .expect("second federation mint submit")
            .expect("second federation mint response");
        assert_ne!(second, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let duplicate = process_federation_mint_input(&state, &first_input)
            .await
            .expect("duplicate federation mint submit")
            .expect("duplicate federation mint response");

        assert_eq!(duplicate, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::Mint,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_mint_after_sqlite_restart_returns_persisted_shares() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-mint-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, &seed).await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let outputs = outputs_for_amount_with_label(&state, 2, "mint-sqlite-restart");
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs,
            signatures: None,
        };

        let first = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request.clone()),
        )
        .await
        .expect("first federation batch mint should return share response")
        .0;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
        );
        drop(state);

        let restarted_state = restart_federated_test_state_from_db(&db_file, &seed).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
        );

        let duplicate = cache_post_batch_mint(
            AuthHeader::None,
            State(restarted_state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request),
        )
        .await
        .expect("duplicate federation batch mint after restart should return persisted shares")
        .0;

        assert_eq!(duplicate, first);
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Mint]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_mint_after_catch_up_returns_persisted_shares() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let quote_id = QuoteId::new();
        let quote_request = cdk::mint::MintQuoteRequest::Bolt11(MintQuoteBolt11Request {
            amount: Amount::from(2),
            unit: CurrencyUnit::Sat,
            description: None,
            pubkey: None,
        });
        let quote_response = cdk::mint::MintQuoteResponse::Bolt11(MintQuoteBolt11Response {
            quote: quote_id.to_string(),
            request: format!("invoice-{quote_id}"),
            amount: Some(Amount::from(2)),
            unit: Some(CurrencyUnit::Sat),
            state: MintQuoteState::Unpaid,
            expiry: Some(10_000),
            pubkey: None,
        });
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id.clone()],
            quote_amounts: None,
            outputs: outputs_for_amount_with_label(&state, 2, "mint-catch-up-output"),
            signatures: None,
        };
        let mint_input = cdk::mint::MintInput::Batch(batch_request.clone());
        let mint_envelope = config
            .mint_operation(mint_input.clone())
            .expect("mint operation envelope");
        let mint_operation_id = mint_envelope.operation_id().expect("mint operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(config.federation_id(), config.member_id());
        peer_consensus
            .submit_envelope(config.mint_quote_operation(
                quote_id.clone(),
                quote_request,
                quote_response,
            ))
            .expect("peer accepted mint quote");
        peer_consensus
            .submit_envelope(config.mint_quote_payment_observation(
                quote_id,
                PaymentMethod::Known(KnownMethod::Bolt11),
                MintQuoteState::Paid,
                Amount::from(2),
                Amount::ZERO,
                Some("mint-catch-up-payment-proof".to_string()),
            ))
            .expect("peer accepted mint quote payment");
        peer_consensus
            .submit_envelope(mint_envelope)
            .expect("peer accepted mint operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        let expected = state
            .mint
            .persisted_mint_share_response(mint_operation_id, &mint_input)
            .await
            .expect("persisted mint response after catch-up");

        let duplicate = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request),
        )
        .await
        .expect("duplicate federation batch mint after catch-up should return persisted shares")
        .0;

        assert_eq!(duplicate, expected);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::MintQuote,
                cdk::federation::FederationOperationKind::MintQuotePayment,
                cdk::federation::FederationOperationKind::Mint,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_returns_federation_signature_shares() {
        let state = create_federated_test_state().await;
        let outputs = outputs_for_amount(&state, 2);
        let inputs = vec![proof_for_amount(&state, 2, "swap-input")];
        let input_y = inputs[0].y().expect("input y");
        let blinded_secret = outputs[0]
            .blinded_secret
            .as_bls_g1()
            .expect("blinded output should be BLS");

        let response = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(cdk::nuts::SwapRequest::new(inputs, outputs.clone())),
        )
        .await
        .expect("federation swap should return share response");
        let signature = response
            .signatures
            .first()
            .expect("one federation signature share");
        let signature_share = signature.c.as_bls_g1().expect("share should be BLS G1");
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let public_share = keyset
            .public_shares
            .get(&config.member_id())
            .expect("member public shares")
            .get(&Amount::from(2))
            .expect("amount public share");
        let share =
            BlsBlindSignatureShare::new(config.member_id().to_bls_signer_id(), signature_share);

        assert_eq!(response.signatures.len(), 1);
        assert_eq!(signature.amount, Amount::from(2));
        assert_eq!(signature.keyset_id, outputs[0].keyset_id);
        assert!(signature.dleq.is_none());
        assert!(
            verify_blind_signature_share(&share, &blinded_secret, public_share)
                .expect("share verification")
        );
        let operation_id =
            first_federation_operation_id(&state, cdk::federation::FederationOperationKind::Swap)
                .await;
        let share_records = state
            .mint
            .localstore()
            .get_federation_signature_shares_for_output(operation_id, 0)
            .await
            .expect("persisted swap share lookup");
        assert_eq!(share_records.len(), 1);
        assert_eq!(share_records[0].member_id, config.member_id());
        assert_eq!(share_records[0].output_index, 0);
        assert_eq!(share_records[0].blinded_secret, outputs[0].blinded_secret);
        assert_eq!(share_records[0].keyset_id, signature.keyset_id);
        assert_eq!(share_records[0].amount, signature.amount);
        assert_eq!(
            cdk::nuts::PublicKey::from(share_records[0].share.signature()),
            signature.c
        );
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let stored_signatures = state
            .mint
            .localstore()
            .get_blind_signatures(&[outputs[0].blinded_secret])
            .await
            .expect("signature lookup");
        assert_eq!(stored_signatures, vec![Some(signature.clone())]);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_aleph_bft_public_route_fails_closed_before_queueing() {
        let state = create_aleph_bft_federated_test_state().await;
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "aleph-bft-halted-swap-input")],
            outputs_for_amount_with_label(&state, 2, "aleph-bft-halted-swap-output"),
        );

        let error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request),
        )
        .await
        .expect_err("AlephBFT public swap should fail closed before real finality exists");

        assert_aleph_bft_http_response_not_ready(error).await;
        assert_no_aleph_bft_public_submit_work(&state).await;
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_returns_persisted_signature_shares() {
        let state = create_federated_test_state().await;
        let outputs = outputs_for_amount(&state, 2);
        let inputs = vec![proof_for_amount(&state, 2, "duplicate-swap-input")];
        let request = cdk::nuts::SwapRequest::new(inputs, outputs);

        let first = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request.clone()),
        )
        .await
        .expect("first federation swap should return share response");
        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request),
        )
        .await
        .expect("duplicate federation swap should return persisted share response");

        assert_eq!(duplicate.0, first.0);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_after_cache_eviction_returns_persisted_signature_shares() {
        let mut state = create_federated_test_state().await;
        limit_federation_response_cache_to_one(&mut state);
        let first_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-cache-eviction-first")],
            outputs_for_amount_with_label(&state, 2, "swap-cache-eviction-first"),
        );

        let first = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(first_request.clone()),
        )
        .await
        .expect("first federation swap should return share response");
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let second_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-cache-eviction-second")],
            outputs_for_amount_with_label(&state, 2, "swap-cache-eviction-second"),
        );
        let second = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(second_request),
        )
        .await
        .expect("second federation swap should return share response");
        assert_ne!(second.0, first.0);
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(first_request),
        )
        .await
        .expect("duplicate federation swap should return persisted share response");

        assert_eq!(duplicate.0, first.0);
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::Swap,
                cdk::federation::FederationOperationKind::Swap,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_after_sqlite_restart_returns_persisted_signature_shares() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-swap-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, &seed).await;
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-sqlite-restart-input")],
            outputs_for_amount_with_label(&state, 2, "swap-sqlite-restart-output"),
        );

        let first = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request.clone()),
        )
        .await
        .expect("first federation swap should return share response")
        .0;
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        drop(state);

        let restarted_state = restart_federated_test_state_from_db(&db_file, &seed).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );

        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(restarted_state.clone()),
            Json(request),
        )
        .await
        .expect("duplicate federation swap after restart should return persisted shares")
        .0;

        assert_eq!(duplicate, first);
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_post_swap_after_catch_up_returns_persisted_signature_shares() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "swap-catch-up-input")],
            outputs_for_amount_with_label(&state, 2, "swap-catch-up-output"),
        );
        let swap_envelope = config
            .swap_operation(request.clone())
            .expect("swap operation envelope");
        let swap_operation_id = swap_envelope.operation_id().expect("swap operation id");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(config.federation_id(), config.member_id());
        peer_consensus
            .submit_envelope(swap_envelope)
            .expect("peer accepted swap operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        let expected = state
            .mint
            .persisted_swap_share_response(swap_operation_id, &request)
            .await
            .expect("persisted swap response after catch-up");

        let duplicate = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(request),
        )
        .await
        .expect("duplicate federation swap after catch-up should return persisted shares")
        .0;

        assert_eq!(duplicate, expected);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Swap]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_swap_rejects_non_v3_outputs_before_consensus() {
        let state = create_federated_test_state().await;
        let outputs = vec![non_v3_output_for_state(
            &state,
            Amount::from(2),
            b"axum federated swap non-v3 output",
        )];

        let error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(cdk::nuts::SwapRequest::new(Vec::new(), outputs)),
        )
        .await
        .expect_err("non-v3 swap output should be rejected");

        let body = response_error_detail(error).await;
        assert!(
            body.contains("Cashu keyset v3"),
            "error response should mention the v3 federation keyset requirement: {body}"
        );
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "invalid federated swap outputs must fail before consensus"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_restore_accepts_v3_restore_placeholders() {
        let state = create_federated_test_state().await;
        let mut restore_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("restore output");
        restore_output.amount = Amount::ZERO;

        let response = crate::router_handlers::post_restore(
            AuthHeader::None,
            State(state),
            Json(cdk::nuts::RestoreRequest {
                outputs: vec![restore_output],
            }),
        )
        .await
        .expect("restore response")
        .0;

        assert!(response.signatures.is_empty());
        assert!(response.outputs.is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn post_restore_rejects_non_v3_outputs() {
        let state = create_federated_test_state().await;

        let error = crate::router_handlers::post_restore(
            AuthHeader::None,
            State(state.clone()),
            Json(cdk::nuts::RestoreRequest {
                outputs: vec![non_v3_change_output_for_state(&state)],
            }),
        )
        .await
        .expect_err("non-v3 restore output should be rejected");

        let body = response_error_detail(error).await;
        assert!(
            body.contains("Cashu keyset v3"),
            "error response should mention the v3 federation keyset requirement: {body}"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_records_federation_melt_operation() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 2, "no-change")],
            None,
        );

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should settle");

        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert!(response.change().is_none());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_aleph_bft_public_route_fails_closed_before_queueing() {
        let state = create_aleph_bft_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            vec![proof_for_amount(&state, 2, "aleph-bft-halted-melt")],
            None,
        );

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("AlephBFT public melt should fail closed before real finality exists");

        assert_aleph_bft_error_not_ready(&error);
        assert_no_aleph_bft_public_submit_work(&state).await;
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn value_routes_fail_closed_while_catching_up_and_resume_when_healthy() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let peer = config.member_id();
        let not_ready = "catching up";

        let quote_id = create_paid_bolt11_quote(&state).await;
        let mint_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount_with_label(&state, 2, "catching-up-mint-output"),
            signatures: None,
        };
        let swap_request = cdk::nuts::SwapRequest::new(
            vec![proof_for_amount(&state, 2, "catching-up-swap-input")],
            outputs_for_amount_with_label(&state, 2, "catching-up-swap-output"),
        );
        let melt_quote_id = create_federated_melt_quote(&state, 1000).await;
        let melt_request = cdk::nuts::MeltRequest::new(
            melt_quote_id.clone(),
            vec![proof_for_amount(&state, 2, "catching-up-melt-input")],
            None,
        );

        state.mark_federation_catching_up(peer).await;

        let mint_error = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(mint_request.clone()),
        )
        .await
        .expect_err("mint should fail while catching up");
        assert_http_response_not_ready_contains(mint_error, not_ready).await;

        let swap_error = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(swap_request.clone()),
        )
        .await
        .expect_err("swap should fail while catching up");
        assert_http_response_not_ready_contains(swap_error, not_ready).await;

        let melt_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &melt_request,
        )
        .await
        .expect_err("melt should fail while catching up");
        assert_error_not_ready_contains(&melt_error, not_ready);
        assert_no_federation_public_submit_work(&state).await;

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&melt_quote_id)
            .await
            .expect("melt quote lookup")
            .expect("melt quote");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);

        state.mark_federation_healthy().await;

        let mint_response = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(mint_request),
        )
        .await
        .expect("mint should resume after catch-up")
        .0;
        assert_eq!(mint_response.signatures.len(), 1);

        let swap_response = crate::router_handlers::post_swap(
            AuthHeader::None,
            State(state.clone()),
            Json(swap_request),
        )
        .await
        .expect("swap should resume after catch-up")
        .0;
        assert_eq!(swap_response.signatures.len(), 1);

        let melt_response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &melt_request,
        )
        .await
        .expect("melt should resume after catch-up");
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);

        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::Mint,
                cdk::federation::FederationOperationKind::Swap,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_returns_federated_change_shares() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let blinded_secret = change_output
            .blinded_secret
            .as_bls_g1()
            .expect("change output should be BLS");
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 4, "with-change")],
            Some(vec![change_output.clone()]),
        );

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should settle with change");
        let change = response.change().expect("federated change share");
        let signature = change.first().expect("one change share");
        let config = state.mint.federation_config().expect("federation config");
        let keyset = &config.public_config().keysets[0];
        let public_share = keyset
            .public_shares
            .get(&config.member_id())
            .expect("member public shares")
            .get(&Amount::from(2))
            .expect("amount public share");
        let share = BlsBlindSignatureShare::new(
            config.member_id().to_bls_signer_id(),
            signature.c.as_bls_g1().expect("share should be BLS G1"),
        );

        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert_eq!(change.len(), 1);
        assert_eq!(signature.amount, Amount::from(2));
        assert_eq!(signature.keyset_id, change_output.keyset_id);
        assert!(signature.dleq.is_none());
        assert!(
            verify_blind_signature_share(&share, &blinded_secret, public_share)
                .expect("share verification")
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_returns_persisted_response() {
        let state = create_federated_test_state().await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 4, "duplicate-melt-with-change")],
            Some(vec![change_output]),
        );

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("first federated fake-wallet melt should settle");
        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("duplicate federated fake-wallet melt should return persisted response");

        assert_eq!(duplicate, first);
        assert_eq!(duplicate.state(), MeltQuoteState::Paid);
        assert!(duplicate.change().is_some());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_after_cache_eviction_returns_persisted_response() {
        let mut state = create_federated_test_state().await;
        limit_federation_response_cache_to_one(&mut state);
        let first_quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut first_change_output =
            outputs_for_amount_with_label(&state, 2, "melt-cache-eviction-first")
                .into_iter()
                .next()
                .expect("first change output");
        first_change_output.amount = Amount::ZERO;
        let first_request = cdk::nuts::MeltRequest::new(
            first_quote_id,
            vec![proof_for_amount(&state, 4, "melt-cache-eviction-first")],
            Some(vec![first_change_output]),
        );

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &first_request,
        )
        .await
        .expect("first federated fake-wallet melt should settle");
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let second_quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut second_change_output =
            outputs_for_amount_with_label(&state, 2, "melt-cache-eviction-second")
                .into_iter()
                .next()
                .expect("second change output");
        second_change_output.amount = Amount::ZERO;
        let second_request = cdk::nuts::MeltRequest::new(
            second_quote_id,
            vec![proof_for_amount(&state, 4, "melt-cache-eviction-second")],
            Some(vec![second_change_output]),
        );
        let second = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &second_request,
        )
        .await
        .expect("second federated fake-wallet melt should settle");
        assert_ne!(second, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &first_request,
        )
        .await
        .expect("duplicate federated fake-wallet melt should return persisted response");

        assert_eq!(duplicate, first);
        assert_eq!(state.federation_response_cache_entry_count(), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![
                cdk::federation::FederationOperationKind::Melt,
                cdk::federation::FederationOperationKind::Melt,
            ]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_after_sqlite_restart_returns_persisted_response() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-melt-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let fake = FakeWallet::new(
            FeeReserve {
                min_fee_reserve: 1.into(),
                percent_fee_reserve: 0.0,
            },
            HashMap::default(),
            HashSet::default(),
            0,
            CurrencyUnit::Sat,
        );
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state =
            create_federated_test_state_with_backend_db_and_seed(Arc::new(fake), db, &seed).await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut change_output = outputs_for_amount_with_label(&state, 2, "melt-sqlite-restart")
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 4, "melt-sqlite-restart")],
            Some(vec![change_output]),
        );

        let first = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("first federated fake-wallet melt should settle");
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        drop(state);

        let restarted_state = restart_federated_test_state_from_db(&db_file, &seed).await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &restarted_state,
            "bolt11",
            &request,
        )
        .await
        .expect(
            "duplicate federated fake-wallet melt after restart should return persisted response",
        );

        assert_eq!(duplicate, first);
        assert_eq!(duplicate.state(), MeltQuoteState::Paid);
        assert!(duplicate.change().is_some());
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_federation_melt_after_catch_up_returns_persisted_response() {
        let state = create_federated_test_state().await;
        let config = state.mint.federation_config().expect("federation config");
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let mut change_output = outputs_for_amount_with_label(&state, 2, "melt-catch-up")
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id,
            vec![proof_for_amount(&state, 4, "melt-catch-up")],
            Some(vec![change_output]),
        );
        let melt_envelope = config
            .melt_operation(request.clone())
            .expect("melt operation envelope");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(config.federation_id(), config.member_id());
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted melt operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        let expected = federation_melt_response_from_store(&state, request.quote())
            .await
            .expect("persisted melt response after catch-up");

        let duplicate = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect(
            "duplicate federated fake-wallet melt after catch-up should return persisted response",
        );

        assert_eq!(duplicate, expected);
        assert_eq!(duplicate.state(), MeltQuoteState::Paid);
        assert!(duplicate.change().is_some());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federated_wallet_connector_aggregates_real_axum_member_routes() {
        let (listeners, member_urls) = local_member_listeners().await;
        let bundle = threshold_bundle(&member_urls);
        let mut mints = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint =
                build_threshold_member_mint(bundle.public_config.clone(), private_config).await;
            mints.push(mint);
        }

        let mint_quote_id = QuoteId::new();
        for mint in &mints {
            add_paid_bolt11_quote_to_mint(mint, &mint_quote_id, 3).await;
        }

        let melt_invoice = create_fake_invoice(1000, "axum threshold http melt".to_string());
        let melt_quote_id = match mints[0]
            .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote")
        {
            cdk::MeltQuoteCreateResponse::Bolt11(response) => response.quote,
            _ => panic!("expected bolt11 melt quote"),
        };
        clone_primary_melt_quote_to_members(&mints, &melt_quote_id).await;

        let handles = spawn_member_routers(&mints, listeners).await;

        let connector = FederatedMintConnector::new(
            bundle.public_config.clone(),
            threshold_http_member_connectors(&bundle.public_config),
        )
        .expect("federated HTTP connector");
        wait_for_federated_http_members(&connector).await;

        let keyset = &bundle.public_config.keysets[0];
        let mint_outputs = vec![
            blinded_output(keyset, 1, "axum-http-mint-output-1"),
            blinded_output(keyset, 2, "axum-http-mint-output-2"),
        ];
        let mint_response = connector
            .post_batch_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchMintRequest {
                    quotes: vec![mint_quote_id.to_string()],
                    quote_amounts: None,
                    outputs: mint_outputs.clone(),
                    signatures: None,
                },
            )
            .await
            .expect("threshold HTTP batch mint");
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &mint_outputs,
            &mint_response.signatures,
        );

        let mint_status = connector
            .get_mint_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &mint_quote_id.to_string(),
            )
            .await
            .expect("mint quote status");
        assert_eq!(mint_status.state(), Some(MintQuoteState::Issued));

        let swap_input = proof_for_keyset(keyset, 2, "swap-input");
        let swap_y = swap_input.y().expect("swap proof y");
        let swap_outputs = vec![blinded_output(keyset, 2, "axum-http-swap-output")];
        let swap_response = connector
            .post_swap(SwapRequest::new(vec![swap_input], swap_outputs.clone()))
            .await
            .expect("threshold HTTP swap");
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &swap_outputs,
            &swap_response.signatures,
        );

        let spent_after_swap = connector
            .post_check_state(CheckStateRequest { ys: vec![swap_y] })
            .await
            .expect("swap check-state");
        assert_eq!(spent_after_swap.states[0].state, cdk::nuts::State::Spent);

        let melt_input = proof_for_keyset(keyset, 4, "melt-input");
        let melt_y = melt_input.y().expect("melt proof y");
        let melt_change_outputs = vec![blinded_change_output(keyset, "axum-http-melt-change")];
        let melt_response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![melt_input],
                    Some(melt_change_outputs.clone()),
                ),
            )
            .await
            .expect("threshold HTTP melt");
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        let change = melt_response.change().expect("melt change");
        assert_aggregate_signatures_verify(&bundle.public_config, &melt_change_outputs, change);

        let melt_status = connector
            .get_melt_quote_status(
                PaymentMethod::Known(KnownMethod::Bolt11),
                &melt_quote_id.to_string(),
            )
            .await
            .expect("melt quote status");
        assert_eq!(melt_status.state(), MeltQuoteState::Paid);
        let spent_after_melt = connector
            .post_check_state(CheckStateRequest { ys: vec![melt_y] })
            .await
            .expect("melt check-state");
        assert_eq!(spent_after_melt.states[0].state, cdk::nuts::State::Spent);

        let restore_response = connector
            .post_restore(RestoreRequest {
                outputs: mint_outputs.clone(),
            })
            .await
            .expect("threshold HTTP restore");
        assert_eq!(restore_response.outputs, mint_outputs);
        assert_aggregate_signatures_verify(
            &bundle.public_config,
            &restore_response.outputs,
            &restore_response.signatures,
        );

        for handle in handles {
            handle.abort();
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn federated_wallet_connector_aggregates_rotated_real_axum_member_routes() {
        let (listeners, member_urls) = local_member_listeners().await;
        let bundle = threshold_rotation_bundle(&member_urls);
        let rotated_public_config = rotated_public_config(&bundle);
        let old_keyset = active_keyset(&bundle);
        let new_keyset = activated_rotation_keyset(&bundle);
        let new_mint_quote_id = QuoteId::new();
        let old_restore_output =
            blinded_output(&old_keyset, 2, "axum-http-rotated-old-restore-output");
        let old_restore_operation_id = FederationOperationId::from_bytes([42; 32]);

        let mut mints = Vec::new();
        for member_id in [1, 2, 3].map(threshold_member_id) {
            let private_config = bundle
                .private_configs
                .get(&member_id)
                .expect("private member config")
                .clone();
            let mint =
                build_threshold_member_mint(bundle.public_config.clone(), private_config).await;
            add_paid_bolt11_quote_to_mint(&mint, &new_mint_quote_id, 3).await;
            add_restore_signature_share_to_mint(
                &mint,
                &bundle.public_config,
                bundle
                    .private_configs
                    .get(&member_id)
                    .expect("private member config"),
                old_restore_operation_id,
                &old_restore_output,
            )
            .await;

            let config = mint.federation_config().expect("federation config").clone();
            let mut journal = InMemoryFederationJournal::new();
            let rotation_entry = appended_entry(
                &mut journal,
                config.operation_envelope(FederationOperation::KeysetRotation(
                    FederationKeysetRotationOperation {
                        keyset: new_keyset.clone(),
                    },
                )),
            );
            persist_and_apply_entry(&mint, &rotation_entry).await;

            mints.push(mint);
        }

        let melt_invoice = create_fake_invoice(1000, "axum rotated http melt".to_string());
        let melt_quote_id = match mints[0]
            .get_melt_quote(cdk::MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
                request: melt_invoice,
                unit: CurrencyUnit::Sat,
                options: None,
            }))
            .await
            .expect("melt quote")
        {
            cdk::MeltQuoteCreateResponse::Bolt11(response) => response.quote,
            _ => panic!("expected bolt11 melt quote"),
        };
        clone_primary_melt_quote_to_members(&mints, &melt_quote_id).await;

        let handles = spawn_member_routers(&mints, listeners).await;
        let connector = FederatedMintConnector::new(
            rotated_public_config.clone(),
            threshold_http_member_connectors(&rotated_public_config),
        )
        .expect("rotated federated HTTP connector");
        wait_for_federated_http_members(&connector).await;

        let active_keysets = connector.get_mint_keys().await.expect("active keys");
        assert_eq!(active_keysets.len(), 1);
        assert_eq!(active_keysets[0].id, new_keyset.id);

        let keysets = connector.get_mint_keysets().await.expect("keysets");
        let old_keyset_info = keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == old_keyset.id)
            .expect("old keyset info");
        let new_keyset_info = keysets
            .keysets
            .iter()
            .find(|keyset| keyset.id == new_keyset.id)
            .expect("new keyset info");
        assert_eq!(old_keyset_info.active, false);
        assert_eq!(new_keyset_info.active, true);

        let mint_outputs = vec![
            blinded_output(&new_keyset, 1, "axum-http-rotated-mint-output-1"),
            blinded_output(&new_keyset, 2, "axum-http-rotated-mint-output-2"),
        ];
        let mint_response = connector
            .post_batch_mint(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                BatchMintRequest {
                    quotes: vec![new_mint_quote_id.to_string()],
                    quote_amounts: None,
                    outputs: mint_outputs.clone(),
                    signatures: None,
                },
            )
            .await
            .expect("rotated threshold HTTP batch mint");
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &mint_outputs,
            &mint_response.signatures,
        );

        let swap_input = proof_for_keyset(&old_keyset, 2, "rotated-swap-input");
        let swap_y = swap_input.y().expect("swap proof y");
        let swap_outputs = vec![blinded_output(
            &new_keyset,
            2,
            "axum-http-rotated-swap-output",
        )];
        let swap_response = connector
            .post_swap(SwapRequest::new(vec![swap_input], swap_outputs.clone()))
            .await
            .expect("rotated threshold HTTP swap");
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &swap_outputs,
            &swap_response.signatures,
        );
        let spent_after_swap = connector
            .post_check_state(CheckStateRequest { ys: vec![swap_y] })
            .await
            .expect("rotated swap check-state");
        assert_eq!(spent_after_swap.states[0].state, cdk::nuts::State::Spent);

        let melt_input = proof_for_keyset(&old_keyset, 4, "rotated-melt-input");
        let melt_y = melt_input.y().expect("melt proof y");
        let melt_change_outputs = vec![blinded_change_output(
            &new_keyset,
            "axum-http-rotated-melt-change",
        )];
        let melt_response = connector
            .post_melt(
                &PaymentMethod::Known(KnownMethod::Bolt11),
                MeltRequest::new(
                    melt_quote_id.to_string(),
                    vec![melt_input],
                    Some(melt_change_outputs.clone()),
                ),
            )
            .await
            .expect("rotated threshold HTTP melt");
        assert_eq!(melt_response.state(), MeltQuoteState::Paid);
        let change = melt_response.change().expect("rotated melt change");
        assert_aggregate_signatures_verify(&rotated_public_config, &melt_change_outputs, change);

        let spent_after_melt = connector
            .post_check_state(CheckStateRequest { ys: vec![melt_y] })
            .await
            .expect("rotated melt check-state");
        assert_eq!(spent_after_melt.states[0].state, cdk::nuts::State::Spent);

        let restore_response = connector
            .post_restore(RestoreRequest {
                outputs: mint_outputs.clone(),
            })
            .await
            .expect("rotated threshold HTTP restore");
        assert_eq!(restore_response.outputs, mint_outputs);
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &restore_response.outputs,
            &restore_response.signatures,
        );

        let old_restore_response = connector
            .post_restore(RestoreRequest {
                outputs: vec![old_restore_output.clone()],
            })
            .await
            .expect("rotated old inactive restore");
        assert_eq!(old_restore_response.outputs, vec![old_restore_output]);
        assert_aggregate_signatures_verify(
            &rotated_public_config,
            &old_restore_response.outputs,
            &old_restore_response.signatures,
        );

        for handle in handles {
            handle.abort();
        }
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_pending_melt_request_persists_federation_operation_id() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            vec![proof_for_amount(&state, 4, "pending-with-change")],
            Some(vec![change_output.clone()]),
        );
        let expected_operation_id = state
            .mint
            .federation_config()
            .expect("federation config")
            .melt_operation(request.clone())
            .expect("melt operation")
            .operation_id()
            .expect("operation id")
            .to_string();

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should remain pending");
        let saga = state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga query")
            .expect("pending saga");

        assert_eq!(response.state(), MeltQuoteState::Pending);
        assert!(response.change().is_none());
        assert_eq!(
            saga.federation_operation_id.as_deref(),
            Some(expected_operation_id.as_str())
        );

        let still_pending = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("pending recovery check");

        assert_eq!(still_pending.state(), MeltQuoteState::Pending);
        assert!(still_pending.change().is_none());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_quote_check_later_finalizes_with_change() {
        let backend = Arc::new(DelayedStatusFakeWallet::new(2, Some(MeltQuoteState::Paid)));
        let state = create_federated_test_state_with_backend(backend).await;
        let quote_id = create_federated_melt_quote(&state, 1000).await;
        let input = proof_for_amount(&state, 4, "pending-quote-check-paid");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request =
            cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], Some(vec![change_output]));

        let pending = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("federated fake-wallet melt should initially remain pending");
        assert_eq!(pending.state(), MeltQuoteState::Pending);
        assert!(pending.change().is_none());

        let settled = state
            .mint
            .check_melt_quote(&quote_id)
            .await
            .expect("later quote check should reconcile pending melt");
        assert_eq!(settled.state(), MeltQuoteState::Paid);
        let change = settled.change().expect("settled melt change");
        assert_eq!(change.len(), 1);
        assert_eq!(change[0].amount, Amount::from(2));

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_verified_paid_finalizes_with_change() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Paid,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 4, "pending-later-paid");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request = cdk::nuts::MeltRequest::new(
            quote_id.clone(),
            vec![input],
            Some(vec![change_output.clone()]),
        );

        let response = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect("pending fake-wallet melt verified as paid should finalize");
        assert_eq!(response.state(), MeltQuoteState::Paid);
        assert!(response.change().is_some());

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Paid);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Spent)]);

        let change = state
            .mint
            .localstore()
            .get_blind_signatures(&[change_output.blinded_secret])
            .await
            .expect("change signature lookup");
        let change_signature = change[0].clone().expect("change share persisted");
        assert_eq!(change_signature.amount, Amount::from(2));
        assert_eq!(change_signature.keyset_id, change_output.keyset_id);
        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none());
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn pending_federated_melt_check_error_keeps_recovery_state() {
        let state = create_federated_test_state().await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Pending,
            check_payment_state: MeltQuoteState::Pending,
            pay_err: false,
            check_err: true,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 2, "pending-check-error");
        let input_y = input.y().expect("input y");
        let mut change_output = outputs_for_amount(&state, 2)
            .into_iter()
            .next()
            .expect("change output");
        change_output.amount = Amount::ZERO;
        let request =
            cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], Some(vec![change_output]));

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("ambiguous fake-wallet melt with failed check should stay unresolved");

        assert!(matches!(error, cdk::Error::Internal));
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Pending);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert_eq!(proof_states, vec![Some(cdk::nuts::State::Pending)]);

        let saga = state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .expect("unresolved saga");
        assert_eq!(saga.state.state(), "payment_attempted");
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures_for_quote(&quote_id)
            .await
            .expect("change signature lookup")
            .is_empty());
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_failed_melt_request_releases_inputs_without_change_shares() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(backend).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 2, "failed-melt");
        let input_y = input.y().expect("input y");
        let request = cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], None);

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("federated fake-wallet melt should fail and compensate");

        assert!(matches!(error, cdk::Error::PaymentFailed));
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("quote lookup")
            .expect("quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);

        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "failed federated melt should release its reserved input proof: {proof_states:?}"
        );

        assert!(state
            .mint
            .localstore()
            .get_melt_saga_by_quote_id(&quote_id)
            .await
            .expect("saga lookup")
            .is_none());
        assert!(state
            .mint
            .localstore()
            .get_blind_signatures_for_quote(&quote_id)
            .await
            .expect("change signature lookup")
            .is_empty());

        let duplicate_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("duplicate failed federated fake-wallet melt should stay failed");

        assert!(matches!(duplicate_error, cdk::Error::PaymentFailed));
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            1,
            "duplicate failed melt must not attempt a second outgoing payment"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("duplicate quote lookup")
            .expect("duplicate quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("duplicate proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "duplicate failed federated melt should leave its input released: {proof_states:?}"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_failed_federation_melt_after_sqlite_restart_stays_failed_without_payment() {
        let db_file = TempMintDbFile::new("cdk-axum-fed-public-failed-melt-restart");
        let mnemonic = Mnemonic::generate(12).expect("mnemonic");
        let seed = mnemonic.to_seed_normalized("");
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let db = Arc::new(
            cdk_sqlite::MintSqliteDatabase::new(db_file.path())
                .await
                .expect("mint db"),
        );
        let state = create_federated_test_state_with_backend_db_and_seed(backend, db, &seed).await;
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 2, "failed-melt-sqlite-restart");
        let input_y = input.y().expect("input y");
        let request = cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], None);

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("first failed federated fake-wallet melt should fail");

        assert!(matches!(error, cdk::Error::PaymentFailed));
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 1);
        drop(state);

        let (restarted_backend, restarted_make_payment_calls) = CountingFakeWallet::new();
        let restarted_state =
            restart_federated_test_state_from_db_with_backend(&db_file, &seed, restarted_backend)
                .await;
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );

        let duplicate_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &restarted_state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("duplicate failed melt after restart should stay failed");

        assert!(matches!(duplicate_error, cdk::Error::PaymentFailed));
        assert_eq!(
            restarted_make_payment_calls.load(Ordering::SeqCst),
            0,
            "duplicate failed melt after restart must not attempt payment"
        );
        assert_eq!(
            federation_operation_kinds(&restarted_state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        let stats = restarted_state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
        let quote = restarted_state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("duplicate quote lookup")
            .expect("duplicate quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        let proof_states = restarted_state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("duplicate proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "duplicate failed melt after restart should leave its input released: {proof_states:?}"
        );
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment() {
        let (backend, make_payment_calls) = CountingFakeWallet::new();
        let state = create_federated_test_state_with_backend(backend).await;
        let config = state.mint.federation_config().expect("federation config");
        let fake_description = FakeInvoiceDescription {
            pay_invoice_state: MeltQuoteState::Failed,
            check_payment_state: MeltQuoteState::Failed,
            pay_err: false,
            check_err: false,
        };
        let quote_id = create_federated_melt_quote_with_description(
            &state,
            1000,
            serde_json::to_string(&fake_description).expect("fake invoice description"),
        )
        .await;
        let input = proof_for_amount(&state, 2, "failed-melt-catch-up");
        let input_y = input.y().expect("input y");
        let request = cdk::nuts::MeltRequest::new(quote_id.clone(), vec![input], None);
        let melt_envelope = config
            .melt_operation(request.clone())
            .expect("melt operation envelope");
        let mut peer_consensus =
            InMemoryFederationConsensus::new(config.federation_id(), config.member_id());
        peer_consensus
            .submit_envelope(melt_envelope)
            .expect("peer accepted failed melt operation");

        apply_peer_catch_up(&state, &peer_consensus).await;
        assert_eq!(make_payment_calls.load(Ordering::SeqCst), 1);

        let quote = state
            .mint
            .localstore()
            .get_melt_quote(&quote_id)
            .await
            .expect("catch-up quote lookup")
            .expect("catch-up quote persisted");
        assert_eq!(quote.state, MeltQuoteState::Unpaid);
        let proof_states = state
            .mint
            .localstore()
            .get_proofs_states(&[input_y])
            .await
            .expect("catch-up proof state lookup");
        assert!(
            matches!(
                proof_states.as_slice(),
                [None | Some(cdk::nuts::State::Unspent)]
            ),
            "failed melt catch-up should leave its input released: {proof_states:?}"
        );

        let duplicate_error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("duplicate failed melt after catch-up should stay failed");

        assert!(matches!(duplicate_error, cdk::Error::PaymentFailed));
        assert_eq!(
            make_payment_calls.load(Ordering::SeqCst),
            1,
            "duplicate failed melt after catch-up must not attempt payment again"
        );
        assert_eq!(
            federation_operation_kinds(&state).await,
            vec![cdk::federation::FederationOperationKind::Melt]
        );
        let stats = state.federation_operation_notification_stats();
        assert_eq!(stats.duplicate_submissions, 1);
    }

    #[cfg(feature = "federation")]
    #[tokio::test]
    async fn process_melt_request_rejects_non_v3_change_before_consensus() {
        let state = create_federated_test_state().await;
        let request = cdk::nuts::MeltRequest::new(
            QuoteId::new(),
            Vec::new(),
            Some(vec![non_v3_change_output_for_state(&state)]),
        );

        let error = process_melt_request(
            PreferHeader {
                respond_async: false,
            },
            &state,
            "bolt11",
            &request,
        )
        .await
        .expect_err("non-v3 aggregate keysets must be rejected");

        assert!(matches!(
            error,
            cdk::Error::Federation(
                cdk::federation::FederationError::InvalidFederatedKeysetVersion { .. }
            )
        ));
        assert!(
            federation_operation_kinds(&state).await.is_empty(),
            "invalid federated change outputs must fail before consensus"
        );
    }

    #[tokio::test]
    async fn cache_post_mint_custom_rejects_cached_url_method_quote_method_mismatch() {
        let state = create_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let mint_request = MintRequest {
            quote: quote_id,
            outputs: outputs_for_amount(&state, 2),
            signature: None,
        };

        let result = cache_post_mint_custom(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(mint_request.clone()),
        )
        .await;
        if let Err(response) = result {
            panic!(
                "bolt11 mint should populate the cache: {}",
                response_error_detail(response).await
            );
        }

        let result = cache_post_mint_custom(
            AuthHeader::None,
            State(state),
            Path("bolt12".to_string()),
            Json(mint_request),
        )
        .await;

        assert!(
            result.is_err(),
            "cache_post_mint_custom must reject cross-method cached mint"
        );
    }

    #[tokio::test]
    async fn cache_post_batch_mint_rejects_cached_url_method_quote_method_mismatch() {
        let state = create_test_state().await;
        let quote_id = create_paid_bolt11_quote(&state).await;
        let batch_request = BatchMintRequest {
            quotes: vec![quote_id],
            quote_amounts: None,
            outputs: outputs_for_amount(&state, 2),
            signatures: None,
        };

        let result = cache_post_batch_mint(
            AuthHeader::None,
            State(state.clone()),
            Path("bolt11".to_string()),
            Json(batch_request.clone()),
        )
        .await;
        if let Err(response) = result {
            panic!(
                "bolt11 batch mint should populate the cache: {}",
                response_error_detail(response).await
            );
        }

        let result = cache_post_batch_mint(
            AuthHeader::None,
            State(state),
            Path("bolt12".to_string()),
            Json(batch_request),
        )
        .await;

        assert!(
            result.is_err(),
            "cache_post_batch_mint must reject cross-method cached mint"
        );
    }

    #[tokio::test]
    async fn post_batch_check_mint_quote_rejects_url_method_quote_method_mismatch() {
        let state = create_test_state_with_custom_methods(&["paypal", "venmo"]).await;
        let quote_id = create_custom_quote(&state, "venmo", 2).await;

        let result = post_batch_check_mint_quote(
            AuthHeader::None,
            State(state),
            Path("paypal".to_string()),
            Json(BatchCheckMintQuoteRequest {
                quotes: vec![quote_id],
            }),
        )
        .await;

        assert!(
            result.is_err(),
            "post_batch_check_mint_quote must reject cross-method quote checks"
        );
    }

    #[tokio::test]
    async fn post_melt_custom_quote_rejects_url_method_request_body_mismatch() {
        let state = create_test_state_with_custom_methods(&["paypal", "venmo"]).await;

        let payload = serde_json::to_value(MeltQuoteCustomRequest {
            method: "venmo".to_string(),
            request: "test-payment-request".to_string(),
            unit: CurrencyUnit::Sat,
            extra: Value::Null,
        })
        .unwrap();

        let result = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("paypal".to_string()),
            Json(payload),
        )
        .await;

        assert!(
            result.is_err(),
            "post_melt_custom_quote must reject cross-method quote creation"
        );

        let venmo_quote_created = state
            .mint
            .melt_quotes()
            .await
            .unwrap()
            .into_iter()
            .any(|quote| quote.payment_method == PaymentMethod::Custom("venmo".to_string()));

        assert!(
            !venmo_quote_created,
            "request to the paypal melt-quote endpoint must not create a venmo-method quote"
        );
    }

    #[tokio::test]
    async fn post_melt_custom_quote_accepts_normalized_method_match() {
        let state = create_test_state_with_custom_methods(&["paypal"]).await;

        let payload = serde_json::to_value(MeltQuoteCustomRequest {
            method: "PayPal".to_string(),
            request: "test-payment-request".to_string(),
            unit: CurrencyUnit::Sat,
            extra: Value::Null,
        })
        .unwrap();

        let result = post_melt_custom_quote(
            AuthHeader::None,
            State(state.clone()),
            Path("paypal".to_string()),
            Json(payload),
        )
        .await;

        assert!(
            result.is_ok(),
            "post_melt_custom_quote must accept path/body methods that normalize to the same value"
        );

        let paypal_quote_created = state
            .mint
            .melt_quotes()
            .await
            .unwrap()
            .into_iter()
            .any(|quote| quote.payment_method == PaymentMethod::Custom("paypal".to_string()));

        assert!(
            paypal_quote_created,
            "mixed-case body method should create a quote under the normalized payment method"
        );
    }
}
