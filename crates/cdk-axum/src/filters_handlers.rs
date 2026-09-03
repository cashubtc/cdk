//! Compact state filter endpoints
//!
//! Filters are public and byte-identical for every requester, so these routes
//! carry no authentication. The cache directives are load-bearing: a complete
//! page never changes again, while the pending filter must never be reused.

use axum::extract::{Path, State};
use axum::http::header::{HeaderValue, CACHE_CONTROL};
use axum::response::{IntoResponse, Response};
use axum::Json;
use cdk_common::Error;
use tracing::instrument;

use crate::router_handlers::into_response;
use crate::MintState;

/// A complete page never changes again, so it may be held indefinitely. Only
/// the current page grows, and it grows once per epoch.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

fn cached<T: serde::Serialize>(directive: &str, body: T) -> Response {
    match HeaderValue::from_str(directive) {
        Ok(value) => ([(CACHE_CONTROL, value)], Json(body)).into_response(),
        Err(err) => {
            tracing::error!("Invalid cache directive {directive}: {err}");
            Json(body).into_response()
        }
    }
}

#[instrument(skip_all)]
pub(crate) async fn get_filters_info(State(state): State<MintState>) -> Result<Response, Response> {
    let service = state
        .mint
        .state_filter_service()
        .ok_or_else(|| into_response(Error::FilterNotAvailable))?;

    let info = service.info().await.map_err(into_response)?;

    Ok(cached("public, max-age=60", info))
}

#[instrument(skip_all, fields(page = ?page))]
pub(crate) async fn get_filters(
    State(state): State<MintState>,
    Path(page): Path<u64>,
) -> Result<Response, Response> {
    let service = state
        .mint
        .state_filter_service()
        .ok_or_else(|| into_response(Error::FilterNotAvailable))?;

    let filters = service.page(page).await.map_err(into_response)?;

    let directive = if service
        .page_is_complete(page)
        .await
        .map_err(into_response)?
    {
        IMMUTABLE.to_string()
    } else {
        let epoch = service.info().await.map_err(into_response)?.epoch;
        format!("public, max-age={epoch}")
    };

    Ok(cached(&directive, filters))
}

#[instrument(skip_all)]
pub(crate) async fn get_filters_pending(
    State(state): State<MintState>,
) -> Result<Response, Response> {
    let service = state
        .mint
        .state_filter_service()
        .ok_or_else(|| into_response(Error::FilterNotAvailable))?;

    let pending = service.pending().await.map_err(into_response)?;

    Ok(cached("no-store", pending))
}
