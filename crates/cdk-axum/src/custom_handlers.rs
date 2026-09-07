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
const FEDERATION_MINT_PAYMENT_WAIT_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(100);

/// Header extractor for the Prefer header
///
/// This extractor checks for the `Prefer: respond-async` header
/// to determine if the client wants asynchronous processing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferHeader {
    pub respond_async: bool,
}

#[cfg(feature = "federation-bark")]
type RouteFederationMintQuotePredecessor = cdk::federation::FederationOperationEnvelope;
#[cfg(not(feature = "federation-bark"))]
type RouteFederationMintQuotePredecessor = ();

async fn create_mint_quote_for_route(
    state: &MintState,
    request: cdk::mint::MintQuoteRequest,
) -> Result<
    (
        cdk::mint::MintQuoteResponse<QuoteId>,
        Option<cdk::cdk_payment::PaymentIdentifier>,
        Option<RouteFederationMintQuotePredecessor>,
    ),
    cdk::Error,
> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        #[cfg(feature = "federation-bark")]
        if matches!(request, cdk::mint::MintQuoteRequest::Bolt11(_))
            && state.federation.bark_runtime().is_some()
        {
            let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
            let quote_id = crate::federation::bark_incoming::federated_bark_quote_id(
                federation.federation_id(),
                &request,
            )?;
            state.mint.check_mint_request_acceptable(&request).await?;
            let (response, request_lookup_id, receive_prepared) =
                crate::federation::bark_incoming::prepare_federated_bark_mint_quote(
                    state, quote_id, request,
                )
                .await?;
            return Ok((response, Some(request_lookup_id), receive_prepared));
        }
        #[cfg(feature = "federation-bdk")]
        if matches!(request, cdk::mint::MintQuoteRequest::Onchain(_)) {
            let quote_id = QuoteId::new();
            state.mint.check_mint_request_acceptable(&request).await?;
            let runtime = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
            let application = runtime.cdk_onchain_application()?;
            let address = state
                .allocate_federation_onchain_address(
                    &quote_id.to_string(),
                    application.application_id,
                    application.application_key_epoch,
                    cdk::federation::FederationAddressKeychain::External,
                )
                .await?;
            let (response, request_lookup_id) = state
                .mint
                .prepare_onchain_mint_quote(quote_id, request, address)
                .await?;
            return Ok((response, Some(request_lookup_id), None));
        }
        let (response, request_lookup_id) = state.mint.prepare_mint_quote(request).await?;
        return Ok((response, Some(request_lookup_id), None));
    }

    Ok((state.mint.get_mint_quote(request).await?, None, None))
}

async fn check_mint_quotes_for_route(
    state: &MintState,
    quote_ids: &[QuoteId],
) -> Result<Vec<cdk::mint::MintQuoteResponse<QuoteId>>, cdk::Error> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        Box::pin(
            state
                .federation
                .payment_observer
                .observe_mint_quotes(state, quote_ids),
        )
        .await?;
        let mut responses = Vec::with_capacity(quote_ids.len());
        for quote_id in quote_ids {
            let quote = state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await?
                .ok_or(cdk::Error::UnknownQuote)?;
            responses.push(quote.try_into()?);
        }
        return Ok(responses);
    }

    state.mint.check_mint_quotes(quote_ids).await
}

async fn check_melt_quote_for_route(
    state: &MintState,
    quote_id: &QuoteId,
) -> Result<MeltQuoteResponse<QuoteId>, cdk::Error> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        Box::pin(
            state
                .federation
                .payment_observer
                .observe_melt_quote(state, quote_id),
        )
        .await?;
        return federation_melt_response_from_store(state, quote_id).await;
    }

    state.mint.check_melt_quote(quote_id).await
}

#[cfg(feature = "federation")]
type RouteFederationMeltQuoteMetadata = cdk::federation::FederationMeltQuoteMetadata;
#[cfg(not(feature = "federation"))]
type RouteFederationMeltQuoteMetadata = ();

async fn create_melt_quote_for_route(
    state: &MintState,
    request: cdk::MeltQuoteRequest,
) -> Result<
    (
        MeltQuoteCreateResponse<QuoteId>,
        Option<RouteFederationMeltQuoteMetadata>,
    ),
    cdk::Error,
> {
    #[cfg(feature = "federation")]
    if state.mint.federation_config().is_some() {
        let (response, metadata) = state.mint.prepare_melt_quote(request).await?;
        return Ok((response, Some(metadata)));
    }

    Ok((state.mint.get_melt_quote(request).await?, None))
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
    if state.mint.federation_config().is_none() {
        return Ok(None);
    }
    let correlation_id = input
        .quote_ids()
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| "quote-less-mint".to_string());
    let timer = crate::federation::latency::FederationLatencyTimer::start(
        crate::federation::latency::FederationLatencyCorrelation::Quote,
        correlation_id,
        crate::federation::latency::FederationLatencyBackend::Cashu,
        crate::federation::latency::FederationLatencyDirection::Incoming,
        crate::federation::latency::FederationLatencyPhase::CashuMint,
    );
    let result = async {
        state.ensure_federation_ready_for_public_submit().await?;
        ensure_federation_mint_quotes_paid_for_signing(state, input).await?;

        let Some(operation_id) = submit_federation_mint_operation(state, input.clone()).await?
        else {
            return Ok(None);
        };

        Ok(Some(
            state
                .mint
                .persisted_mint_share_response(operation_id, input)
                .await?,
        ))
    }
    .await;
    timer
        .finish(crate::federation::latency::FederationLatencyOutcome::from_success(result.is_ok()));
    result
}

#[cfg(feature = "federation")]
async fn ensure_federation_mint_quotes_paid_for_signing(
    state: &MintState,
    input: &cdk::mint::MintInput,
) -> Result<(), cdk::Error> {
    if state.mint.federation_config().is_none() {
        return Ok(());
    }

    let quote_ids = input.quote_ids();
    if quote_ids.is_empty() {
        return Ok(());
    }

    let timer = crate::federation::latency::FederationLatencyTimer::start(
        crate::federation::latency::FederationLatencyCorrelation::Quote,
        quote_ids
            .first()
            .map(ToString::to_string)
            .unwrap_or_else(|| "quote-less-mint".to_string()),
        crate::federation::latency::FederationLatencyBackend::Cashu,
        crate::federation::latency::FederationLatencyDirection::Incoming,
        crate::federation::latency::FederationLatencyPhase::MintPaymentWait,
    );
    let result = async {
        Box::pin(
            state
                .federation
                .payment_observer
                .observe_mint_quotes(state, &quote_ids),
        )
        .await?;

        let wait_timeout = state
            .federation
            .peer_sync_config
            .unwrap_or_default()
            .operation_submission_timeout;
        tokio::time::timeout(
            wait_timeout,
            wait_for_federation_mint_quotes_paid_locally(state, &quote_ids),
        )
        .await
        .map_err(|_| cdk::Error::PendingQuote)?
    }
    .await;
    let outcome = match &result {
        Ok(()) => crate::federation::latency::FederationLatencyOutcome::Success,
        Err(cdk::Error::PendingQuote) => {
            crate::federation::latency::FederationLatencyOutcome::Timeout
        }
        Err(_) => crate::federation::latency::FederationLatencyOutcome::Error,
    };
    timer.finish(outcome);
    result
}

#[cfg(feature = "federation")]
async fn wait_for_federation_mint_quotes_paid_locally(
    state: &MintState,
    quote_ids: &[QuoteId],
) -> Result<(), cdk::Error> {
    loop {
        let mut all_paid = true;
        for quote_id in quote_ids {
            let quote = state
                .mint
                .localstore()
                .get_mint_quote(quote_id)
                .await?
                .ok_or(cdk::Error::UnknownQuote)?;
            if !matches!(
                quote.state(),
                cdk::nuts::MintQuoteState::Paid | cdk::nuts::MintQuoteState::Issued
            ) {
                all_paid = false;
                break;
            }
        }

        if all_paid {
            return Ok(());
        }

        tokio::time::sleep(FEDERATION_MINT_PAYMENT_WAIT_POLL_INTERVAL).await;
    }
}

#[cfg(feature = "federation")]
async fn submit_federation_mint_quote_operation(
    state: &MintState,
    request: cdk::mint::MintQuoteRequest,
    response: &cdk::mint::MintQuoteResponse<QuoteId>,
    request_lookup_id: Option<cdk::cdk_payment::PaymentIdentifier>,
    ordered_predecessor: Option<RouteFederationMintQuotePredecessor>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let request_lookup_id = request_lookup_id.ok_or(cdk::Error::Internal)?;
    let envelope = config.mint_quote_operation_with_lookup_id(
        response.quote().clone(),
        request_lookup_id,
        request,
        mint_quote_response_to_string(response),
    );
    #[cfg(feature = "federation-bark")]
    match ordered_predecessor {
        Some(predecessor) => {
            let responses = state
                .submit_federation_ordered_successor_group(vec![predecessor, envelope])
                .await?;
            let [predecessor_response, quote_response] = responses.as_slice() else {
                return Err(cdk::Error::Internal);
            };
            predecessor_response.ensure_accepted()?;
            quote_response.ensure_accepted()?;
        }
        None => {
            state.submit_federation_envelope(envelope).await?;
        }
    }
    #[cfg(not(feature = "federation-bark"))]
    {
        let _ = ordered_predecessor;
        state.submit_federation_envelope(envelope).await?;
    }
    match Box::pin(
        state
            .federation
            .payment_observer
            .observe_mint_quotes(state, std::slice::from_ref(response.quote())),
    )
    .await
    {
        Ok(submitted) if submitted > 0 => {
            tracing::debug!(
                quote = %response.quote(),
                "Submitted immediate federation mint payment observation"
            );
        }
        Ok(_) => {}
        Err(err) => {
            tracing::debug!(
                quote = %response.quote(),
                "Immediate federation mint payment observation failed: {}",
                err
            );
        }
    }
    Ok(())
}

#[cfg(feature = "federation")]
async fn submit_federation_melt_quote_operation(
    state: &MintState,
    request: cdk::MeltQuoteRequest,
    response: &MeltQuoteCreateResponse<QuoteId>,
    metadata: Option<RouteFederationMeltQuoteMetadata>,
) -> Result<(), cdk::Error> {
    let Some(config) = state.mint.federation_config() else {
        return Ok(());
    };
    let quote = response.quote().cloned().ok_or(cdk::Error::UnknownQuote)?;
    let metadata = metadata.ok_or(cdk::Error::Internal)?;
    let envelope = config.melt_quote_operation(quote, request, response.clone(), metadata);
    state.submit_federation_envelope(envelope).await?;
    Ok(())
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
            let (quote, _request_lookup_id, _ordered_predecessor) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &quote,
                _request_lookup_id,
                _ordered_predecessor,
            )
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
            let (quote, _request_lookup_id, _ordered_predecessor) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &quote,
                _request_lookup_id,
                _ordered_predecessor,
            )
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
            let (quote, _request_lookup_id, _ordered_predecessor) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &quote,
                _request_lookup_id,
                _ordered_predecessor,
            )
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

            let (response, _request_lookup_id, _ordered_predecessor) =
                create_mint_quote_for_route(&state, quote_request.clone())
                    .await
                    .map_err(into_response)?;
            #[cfg(feature = "federation")]
            submit_federation_mint_quote_operation(
                &state,
                quote_request,
                &response,
                _request_lookup_id,
                _ordered_predecessor,
            )
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

    let quote_response = check_mint_quotes_for_route(&state, &[quote_id])
        .await
        .map_err(into_response)?
        .first()
        .cloned()
        .ok_or(cdk::Error::UnknownQuote)
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

    let responses = check_mint_quotes_for_route(&state, &payload.quotes)
        .await
        .map_err(into_response)?;

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

    let (quote_request, response, federation_metadata) = match method.as_str() {
        "bolt11" => {
            let bolt11_request: MeltQuoteBolt11Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt11 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt11(bolt11_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
        "bolt12" => {
            let bolt12_request: MeltQuoteBolt12Request =
                serde_json::from_value(payload).map_err(|e| {
                    tracing::error!("Failed to parse bolt12 melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Bolt12(bolt12_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
        "onchain" => {
            let onchain_request: MeltQuoteOnchainRequest = serde_json::from_value(payload)
                .map_err(|e| {
                    tracing::error!("Failed to parse onchain melt request: {}", e);
                    into_response(cdk::Error::InvalidPaymentMethod)
                })?;

            let quote_request = cdk::MeltQuoteRequest::Onchain(onchain_request);
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
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
            let (response, metadata) = create_melt_quote_for_route(&state, quote_request.clone())
                .await
                .map_err(into_response)?;

            (quote_request, response, metadata)
        }
    };

    #[cfg(feature = "federation")]
    submit_federation_melt_quote_operation(&state, quote_request, &response, federation_metadata)
        .await
        .map_err(into_response)?;
    #[cfg(not(feature = "federation"))]
    let _ = (quote_request, federation_metadata);

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

    let quote = Box::pin(check_melt_quote_for_route(&state, &quote_id))
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
    if let Some(response) = Box::pin(process_federation_melt_request(state, payload)).await? {
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
    let timer = crate::federation::latency::FederationLatencyTimer::start(
        crate::federation::latency::FederationLatencyCorrelation::Quote,
        payload.quote().to_string(),
        crate::federation::latency::FederationLatencyBackend::Cashu,
        crate::federation::latency::FederationLatencyDirection::Outgoing,
        crate::federation::latency::FederationLatencyPhase::CashuMelt,
    );
    let result = async {
        #[cfg(feature = "federation-bark")]
        let submitted_bark_group = {
            let quote = state
                .mint
                .localstore()
                .get_melt_quote(payload.quote())
                .await?
                .ok_or(cdk::Error::UnknownQuote)?;
            if state.federation.bark_runtime().is_some()
                && matches!(
                    quote.request,
                    cdk_common::mint::MeltPaymentRequest::Bolt11 { .. }
                )
            {
                crate::federation::bark_outgoing::submit_federated_bark_melt_with_plan(
                    state,
                    envelope.clone(),
                    payload.quote(),
                )
                .await?;
                true
            } else {
                false
            }
        };
        #[cfg(not(feature = "federation-bark"))]
        let submitted_bark_group = false;
        if !submitted_bark_group {
            state.submit_federation_envelope(envelope).await?;
        }
        if let Err(err) = Box::pin(
            state
                .federation
                .payment_observer
                .observe_melt_quote(state, payload.quote()),
        )
        .await
        {
            tracing::debug!(
                quote = %payload.quote(),
                "Immediate federation melt payment observation deferred: {}",
                err
            );
        }
        if federation_melt_quote_is_failed_terminal(state, payload.quote()).await? {
            return Err(cdk::Error::PaymentFailed);
        }

        Ok(Some(
            federation_melt_response_from_store(state, payload.quote()).await?,
        ))
    }
    .await;
    timer
        .finish(crate::federation::latency::FederationLatencyOutcome::from_success(result.is_ok()));
    result
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
mod tests;
