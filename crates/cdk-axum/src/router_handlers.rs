use anyhow::Result;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::error::ErrorResponse;
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, Id, KeysResponse, KeysetResponse, MintInfo,
    RestoreRequest, RestoreResponse, SwapRequest, SwapResponse,
};
use cdk::util::unix_time;
use paste::paste;
use tracing::instrument;

use crate::auth::AuthHeader;
use crate::ws::main_websocket;
use crate::MintState;

/// Macro to add cache to endpoint
#[macro_export]
macro_rules! post_cache_wrapper {
    ($handler:ident, $request_type:ty, $response_type:ty) => {
        paste! {
            /// Cache wrapper function for $handler:
            /// Wrap $handler into a function that caches responses using the request as key
            pub async fn [<cache_ $handler>](
                auth: AuthHeader,
                state: State<MintState>,
                payload: Json<$request_type>
            ) -> Result<Json<$response_type>, Response> {
                use std::ops::Deref;
                let json_extracted_payload = payload.deref();
                let State(mint_state) = state.clone();
                let cache_key = match mint_state.cache.calculate_key(&json_extracted_payload) {
                    Some(key) => key,
                    None => {
                        // Could not calculate key, just return the handler result
                        return $handler(auth, state, payload).await;
                    }
                };
                if let Some(cached_response) = mint_state.cache.get::<$response_type>(&cache_key).await {
                    return Ok(Json(cached_response));
                }
                let response = $handler(auth, state, payload).await?;
                mint_state.cache.set(cache_key, &response.deref()).await;
                Ok(response)
            }
        }
    };
}

/// Macro to add cache to endpoint with prefer header support (for async operations)
#[macro_export]
macro_rules! post_cache_wrapper_with_prefer {
    ($handler:ident, $request_type:ty, $response_type:ty) => {
        paste! {
            /// Cache wrapper function for $handler with PreferHeader support:
            /// Wrap $handler into a function that caches responses using the request as key
            pub async fn [<cache_ $handler>](
                auth: AuthHeader,
                prefer: PreferHeader,
                state: State<MintState>,
                payload: Json<$request_type>
            ) -> Result<Json<$response_type>, Response> {
                use std::ops::Deref;

                let json_extracted_payload = payload.deref();
                let State(mint_state) = state.clone();
                let cache_key = match mint_state.cache.calculate_key(&json_extracted_payload) {
                    Some(key) => key,
                    None => {
                        // Could not calculate key, just return the handler result
                        return $handler(auth, prefer, state, payload).await;
                    }
                };
                if let Some(cached_response) = mint_state.cache.get::<$response_type>(&cache_key).await {
                    return Ok(Json(cached_response));
                }
                let response = $handler(auth, prefer, state, payload).await?;
                mint_state.cache.set(cache_key, &response.deref()).await;
                Ok(response)
            }
        }
    };
}

post_cache_wrapper!(post_swap, SwapRequest, SwapResponse);

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/keys",
    responses(
        (status = 200, description = "Successful response", body = KeysResponse, content_type = "application/json")
    )
))]
/// Get the public keys of the newest mint keyset
///
/// This endpoint returns a dictionary of all supported token values of the mint and their associated public key.
#[instrument(skip_all)]
pub(crate) async fn get_keys(
    State(state): State<MintState>,
) -> Result<Json<KeysResponse>, Response> {
    Ok(Json(state.mint.pubkeys()))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/keys/{keyset_id}",
    params(
        ("keyset_id" = String, description = "The keyset ID"),
    ),
    responses(
        (status = 200, description = "Successful response", body = KeysResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get the public keys of a specific keyset
///
/// Get the public keys of the mint from a specific keyset ID.
#[instrument(skip_all, fields(keyset_id = ?keyset_id))]
pub(crate) async fn get_keyset_pubkeys(
    State(state): State<MintState>,
    Path(keyset_id): Path<Id>,
) -> Result<Json<KeysResponse>, Response> {
    let pubkeys = state.mint.keyset_pubkeys(&keyset_id).map_err(|err| {
        tracing::error!("Could not get keyset pubkeys: {}", err);
        into_response(err)
    })?;

    Ok(Json(pubkeys))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/keysets",
    responses(
        (status = 200, description = "Successful response", body = KeysetResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Get all active keyset IDs of the mint
///
/// This endpoint returns a list of keysets that the mint currently supports and will accept tokens from.
#[instrument(skip_all)]
pub(crate) async fn get_keysets(
    State(state): State<MintState>,
) -> Result<Json<KeysetResponse>, Response> {
    Ok(Json(state.mint.keysets()))
}

#[instrument(skip_all)]
pub(crate) async fn ws_handler(
    auth: AuthHeader,
    State(state): State<MintState>,
    ws: WebSocketUpgrade,
) -> Result<impl IntoResponse, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Get, RoutePath::Ws),
        )
        .await
        .map_err(into_response)?;

    Ok(ws.on_upgrade(|ws| main_websocket(ws, state)))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/checkstate",
    request_body(content = CheckStateRequest, description = "State params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = CheckStateResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Check whether a proof is spent already or is pending in a transaction
///
/// Check whether a secret has been spent already or not.
#[instrument(skip_all, fields(y_count = ?payload.ys.len()))]
pub(crate) async fn post_check(
    auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<CheckStateRequest>,
) -> Result<Json<CheckStateResponse>, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Checkstate),
        )
        .await
        .map_err(into_response)?;

    let state = state.mint.check_state(&payload).await.map_err(|err| {
        tracing::error!("Could not check state of proofs");
        into_response(err)
    })?;

    Ok(Json(state))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    get,
    context_path = "/v1",
    path = "/info",
    responses(
        (status = 200, description = "Successful response", body = MintInfo)
    )
))]
/// Mint information, operator contact information, and other info
#[instrument(skip_all)]
pub(crate) async fn get_mint_info(
    State(state): State<MintState>,
) -> Result<Json<MintInfo>, Response> {
    Ok(Json(
        state
            .mint
            .mint_info()
            .await
            .map_err(|err| {
                tracing::error!("Could not get mint info: {}", err);
                into_response(err)
            })?
            .clone()
            .time(unix_time()),
    ))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/swap",
    request_body(content = SwapRequest, description = "Swap params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = SwapResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Swap inputs for outputs of the same value
///
/// Requests a set of Proofs to be swapped for another set of BlindSignatures.
///
/// This endpoint can be used by Alice to swap a set of proofs before making a payment to Carol. It can then used by Carol to redeem the tokens for new proofs.
#[instrument(skip_all, fields(inputs_count = ?payload.inputs().len()))]
pub(crate) async fn post_swap(
    auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<SwapRequest>,
) -> Result<Json<SwapResponse>, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Swap),
        )
        .await
        .map_err(into_response)?;

    let swap_response = state
        .mint
        .process_swap_request(payload)
        .await
        .map_err(|err| {
            tracing::error!("Could not process swap request: {}", err);
            into_response(err)
        })?;

    Ok(Json(swap_response))
}

#[cfg_attr(feature = "swagger", utoipa::path(
    post,
    context_path = "/v1",
    path = "/restore",
    request_body(content = RestoreRequest, description = "Restore params", content_type = "application/json"),
    responses(
        (status = 200, description = "Successful response", body = RestoreResponse, content_type = "application/json"),
        (status = 500, description = "Server error", body = ErrorResponse, content_type = "application/json")
    )
))]
/// Restores blind signature for a set of outputs.
#[instrument(skip_all, fields(outputs_count = ?payload.outputs.len()))]
pub(crate) async fn post_restore(
    auth: AuthHeader,
    State(state): State<MintState>,
    Json(payload): Json<RestoreRequest>,
) -> Result<Json<RestoreResponse>, Response> {
    state
        .mint
        .verify_auth(
            auth.into(),
            &ProtectedEndpoint::new(Method::Post, RoutePath::Restore),
        )
        .await
        .map_err(into_response)?;

    let restore_response = state.mint.restore(payload).await.map_err(|err| {
        tracing::error!("Could not process restore: {}", err);
        into_response(err)
    })?;

    Ok(Json(restore_response))
}

#[cfg(feature = "info-page")]
const CSS: &str = r#"
:root {
  --bg: #000;
  --surface: #0e0e0e;
  --surface-2: #191919;
  --border: rgba(255,255,255,0.08);
  --border-section: rgba(255,255,255,0.06);
  --text-primary: #fff;
  --text-secondary: rgba(255,255,255,0.72);
  --text-muted: rgba(255,255,255,0.45);
  --text-faint: rgba(255,255,255,0.28);
  --green: #00d632;
  --green-soft: rgba(0, 214, 50, 0.1);
  --green-glow: rgba(0, 214, 50, 0.06);
  --red: #ff5555;
  --red-soft: rgba(255, 68, 68, 0.1);
  --yellow: #ffb800;
  --yellow-soft: rgba(255, 184, 0, 0.1);
  --radius: 16px;
  --radius-sm: 12px;
}

* { margin: 0; padding: 0; box-sizing: border-box; }

body {
  background: var(--bg);
  color: var(--text-primary);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
  min-height: 100vh;
  -webkit-font-smoothing: antialiased;
}

.page {
  max-width: 520px;
  margin: 0 auto;
  padding: 0 20px 100px;
}

/* ── Topbar ── */
.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 0;
  position: sticky;
  top: 0;
  background: rgba(0,0,0,0.88);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
  z-index: 10;
}

.cashu-wordmark {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-muted);
  letter-spacing: 0.08em;
  text-transform: uppercase;
}

.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--green);
  background: var(--green-soft);
  padding: 5px 11px;
  border-radius: 20px;
}

.status-dot {
  width: 6px;
  height: 6px;
  background: var(--green);
  border-radius: 50%;
  animation: pulse 2.4s ease-in-out infinite;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}

/* ── Hero ── */
.hero {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 40px 0 16px;
  position: relative;
}

.hero::before {
  content: '';
  position: absolute;
  top: 16px;
  left: 50%;
  transform: translateX(-50%);
  width: 180px;
  height: 180px;
  background: radial-gradient(circle, var(--green-glow) 0%, transparent 70%);
  pointer-events: none;
}

.avatar-ring {
  width: 88px;
  height: 88px;
  border-radius: 50%;
  padding: 2.5px;
  background: linear-gradient(135deg, var(--green) 0%, rgba(0,214,50,0.15) 100%);
  margin-bottom: 20px;
  position: relative;
  z-index: 1;
}

.avatar {
  width: 100%;
  height: 100%;
  border-radius: 50%;
  background: var(--surface-2);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 34px;
  font-weight: 700;
  color: var(--green);
  overflow: hidden;
}

.avatar img { width: 100%; height: 100%; object-fit: cover; }

.mint-name {
  font-size: 30px;
  font-weight: 800;
  letter-spacing: -0.03em;
  text-align: center;
  line-height: 1.15;
  margin-bottom: 8px;
}

.mint-desc {
  font-size: 15px;
  font-weight: 400;
  color: var(--text-secondary);
  text-align: center;
  line-height: 1.5;
  max-width: 380px;
}

.mint-desc-long {
  font-size: 14px;
  font-weight: 400;
  color: var(--text-muted);
  text-align: center;
  line-height: 1.5;
  max-width: 380px;
  margin-top: 4px;
  font-style: italic;
}

.version-chip {
  font-size: 11px;
  font-family: ui-monospace, 'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Segoe UI Mono', monospace;
  font-weight: 500;
  color: var(--text-muted);
  background: var(--surface);
  padding: 5px 12px;
  border-radius: 20px;
  border: 1px solid var(--border);
  margin-top: 14px;
}

/* ── MOTD ── */
.motd {
  background: var(--yellow-soft);
  border: 1px solid rgba(255,184,0,0.12);
  border-radius: var(--radius-sm);
  padding: 14px 16px;
  margin: 24px 0 0;
}

.motd-label {
  font-size: 10px;
  font-weight: 700;
  color: var(--yellow);
  text-transform: uppercase;
  letter-spacing: 0.1em;
  margin-bottom: 4px;
}

.motd-text {
  font-size: 14px;
  color: rgba(255,255,255,0.85);
  line-height: 1.5;
}

/* ── Disabled banners ── */
.disabled-banner {
  background: var(--red-soft);
  border: 1px solid rgba(255,68,68,0.12);
  border-radius: var(--radius-sm);
  padding: 12px 16px;
  margin-top: 16px;
  font-size: 14px;
  font-weight: 500;
  color: var(--red);
  text-align: center;
}

/* ── URL section ── */
.url-section { margin-top: 28px; }

.url-bar {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  padding: 14px 16px;
  display: flex;
  align-items: center;
  gap: 12px;
}

.url-text {
  font-family: ui-monospace, 'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Segoe UI Mono', monospace;
  font-size: 13px;
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
}

.extra-urls { margin-top: 8px; display: flex; flex-direction: column; gap: 6px; }

.extra-url {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 10px 14px;
  display: flex;
  align-items: center;
  gap: 10px;
}

.extra-url .url-text { font-size: 11px; }

.url-label {
  font-size: 10px;
  font-weight: 600;
  color: var(--text-faint);
  text-transform: uppercase;
  letter-spacing: 0.06em;
  flex-shrink: 0;
}

/* ── Detail card ── */
.detail-card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  margin-top: 28px;
  overflow: hidden;
}

.card-section-header {
  padding: 18px 20px 0;
  font-size: 15px;
  font-weight: 700;
  color: var(--text-primary);
  letter-spacing: -0.01em;
}

.card-section-header.has-rule {
  border-top: 1px solid var(--border-section);
  margin-top: 16px;
  padding-top: 18px;
}

.detail-row {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  padding: 7px 20px;
  gap: 16px;
}

.detail-row:first-child,
.card-section-header + .detail-row {
  padding-top: 12px;
}

.detail-row:last-child,
.detail-row + .card-divider {
  padding-bottom: 4px;
}

.detail-row.row-last {
  padding-bottom: 16px;
}

.detail-label {
  font-size: 14px;
  font-weight: 400;
  color: var(--text-secondary);
  flex-shrink: 0;
}

.detail-value {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  text-align: right;
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
  justify-content: flex-end;
}

.detail-value-mono {
  font-family: ui-monospace, 'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Segoe UI Mono', monospace;
  font-size: 13px;
  font-weight: 500;
  color: var(--text-secondary);
}

/* Tags */
.tag {
  font-size: 12px;
  font-weight: 600;
  font-family: ui-monospace, 'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Segoe UI Mono', monospace;
  padding: 4px 11px;
  border-radius: 20px;
  background: var(--surface-2);
  color: var(--text-primary);
  border: 1px solid var(--border);
  display: inline-block;
  text-transform: uppercase;
}

.tag-red {
  background: var(--red-soft);
  color: var(--red);
  border-color: rgba(255,68,68,0.12);
}

/* ── Features grid ── */
.features-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0;
  margin: 0;
}

.feature {
  padding: 12px 20px;
  display: flex;
  align-items: flex-start;
  gap: 10px;
  border-bottom: 1px solid var(--border-section);
  border-right: 1px solid var(--border-section);
}

.feature:nth-child(2n) { border-right: none; }
.feature:nth-last-child(-n+2) { border-bottom: none; }
.feature:last-child:nth-child(odd) { border-right: none; }

.feature-dot {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--green-soft);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  margin-top: 2px;
}

.feature-dot svg { width: 10px; height: 10px; }

.feature-name {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  line-height: 1.3;
}

/* ── Contact ── */
.contact-chips { display: flex; gap: 8px; flex-wrap: wrap; padding: 4px 20px 18px; }

.contact-chip {
  font-size: 12px;
  font-weight: 600;
  font-family: ui-monospace, 'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Segoe UI Mono', monospace;
  color: var(--text-primary);
  background: var(--surface-2);
  border: 1px solid var(--border);
  padding: 4px 11px;
  border-radius: 20px;
  text-decoration: none;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.contact-chip svg { width: 12px; height: 12px; opacity: 0.5; }

/* ── Pubkey row ── */
.pubkey-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 20px 18px;
}

.pubkey-mono {
  font-family: ui-monospace, 'SFMono-Regular', 'SF Mono', 'Cascadia Code', 'Segoe UI Mono', monospace;
  font-size: 11px;
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
  min-width: 0;
}

/* ── Info tip ── */
.info-tip {
  margin-top: 28px;
  padding: 18px 20px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  display: flex;
  gap: 12px;
  align-items: flex-start;
}

.info-tip-icon {
  width: 18px; height: 18px;
  flex-shrink: 0;
  color: var(--text-muted);
  margin-top: 2px;
}

.info-tip-text {
  font-size: 13.5px;
  color: var(--text-secondary);
  line-height: 1.6;
}

.info-tip-text a {
  color: var(--text-primary);
  font-weight: 600;
  text-decoration: none;
  border-bottom: 1px solid var(--text-faint);
}

/* ── Footer ── */
.footer {
  text-align: center;
  padding: 36px 0 20px;
  font-size: 12px;
  color: var(--text-faint);
}

.footer a {
  color: var(--text-muted);
  text-decoration: none;
}
"#;

#[cfg(feature = "info-page")]
/// Get the index page
#[instrument(skip_all)]
pub(crate) async fn get_index(
    State(state): State<MintState>,
) -> Result<impl IntoResponse, Response> {
    use maud::html;

    let mint_info = state.mint.mint_info().await.map_err(into_response)?;

    let name = mint_info.name.clone().unwrap_or("CDK Mint".to_string());
    let description = mint_info.description.clone();
    let long_description = mint_info.description_long.clone();
    let motd = mint_info.motd.clone();
    let pubkey = mint_info.pubkey.map(|p| p.to_hex());
    let version = mint_info.version.as_ref().map(|v| v.to_string());
    let contact = mint_info.contact.clone().unwrap_or_default();
    let icon_url = mint_info.icon_url.clone();
    let urls = mint_info.urls.clone().unwrap_or_default();
    let units: Vec<String> = mint_info
        .supported_units()
        .into_iter()
        .map(|u| u.to_string())
        .collect();

    let mut mint_methods: Vec<String> = mint_info
        .nuts
        .nut04
        .supported_methods()
        .into_iter()
        .map(|m| m.to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    mint_methods.sort();

    let mut melt_methods: Vec<String> = mint_info
        .nuts
        .nut05
        .supported_methods()
        .into_iter()
        .map(|m| m.to_string())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    melt_methods.sort();

    let minting_disabled = mint_info.nuts.nut04.disabled;
    let melting_disabled = mint_info.nuts.nut05.disabled;

    // Collect mint limits from nut04 methods (deduplicated)
    let mint_limits: std::collections::BTreeSet<String> = mint_info
        .nuts
        .nut04
        .methods
        .iter()
        .filter(|m| m.min_amount.is_some() || m.max_amount.is_some())
        .map(|m| {
            let parts: Vec<String> = [m.min_amount.as_ref(), m.max_amount.as_ref()]
                .iter()
                .filter_map(|a| a.map(|v| v.to_string()))
                .collect();
            format!("{} {}", parts.join(" – "), m.unit)
        })
        .collect();

    // Collect melt limits from nut05 methods (deduplicated)
    let melt_limits: std::collections::BTreeSet<String> = mint_info
        .nuts
        .nut05
        .methods
        .iter()
        .filter(|m| m.min_amount.is_some() || m.max_amount.is_some())
        .map(|m| {
            let parts: Vec<String> = [m.min_amount.as_ref(), m.max_amount.as_ref()]
                .iter()
                .filter_map(|a| a.map(|v| v.to_string()))
                .collect();
            format!("{} {}", parts.join(" – "), m.unit)
        })
        .collect();

    // Build supported features list (NUT-7+)
    let mut supported_features: Vec<(u32, &str)> = Vec::new();
    if mint_info.nuts.nut07.supported {
        supported_features.push((7, "Token state check"));
    }
    if mint_info.nuts.nut08.supported {
        supported_features.push((8, "Lightning fee returns"));
    }
    if mint_info.nuts.nut09.supported {
        supported_features.push((9, "Signature restore"));
    }
    if mint_info.nuts.nut10.supported {
        supported_features.push((10, "Spending conditions"));
    }
    if mint_info.nuts.nut11.supported {
        supported_features.push((11, "Pay-to-Pubkey"));
    }
    if mint_info.nuts.nut12.supported {
        supported_features.push((12, "DLEQ proofs"));
    }
    if mint_info.nuts.nut14.supported {
        supported_features.push((14, "HTLCs"));
    }
    if !mint_info.nuts.nut15.methods.is_empty() {
        supported_features.push((15, "Multi-path payments"));
    }
    if !mint_info.nuts.nut17.supported.is_empty() {
        supported_features.push((17, "WebSocket subscriptions"));
    }
    if !mint_info.nuts.nut19.cached_endpoints.is_empty() {
        supported_features.push((19, "Cached responses"));
    }
    if mint_info.nuts.nut20.supported {
        supported_features.push((20, "Signed mint quotes"));
    }
    if mint_info.nuts.nut21.is_some() {
        supported_features.push((21, "Clear auth"));
    }
    if mint_info.nuts.nut22.is_some() {
        supported_features.push((22, "Blind auth"));
    }
    if !mint_info.nuts.nut29.is_empty() {
        supported_features.push((29, "Batched minting"));
    }

    // Avatar fallback letter
    let avatar_letter = name
        .chars()
        .next()
        .unwrap_or('M')
        .to_uppercase()
        .to_string();

    let markup = html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                title { (name) }
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                style { (maud::PreEscaped(CSS)) }
            }
            body {
                div class="page" {

                    // Topbar
                    div class="topbar" {
                        span class="cashu-wordmark" { "Cashu Mint" }
                        span class="status-badge" {
                            span class="status-dot" {}
                            " Online"
                        }
                    }

                    // Hero
                    div class="hero" {
                        div class="avatar-ring" {
                            div class="avatar" {
                                @if let Some(ref url) = icon_url {
                                    img src=(url) alt=(name);
                                } @else {
                                    (avatar_letter)
                                }
                            }
                        }
                        div class="mint-name" { (name) }
                        @if let Some(ref desc) = description {
                            div class="mint-desc" { (desc) }
                        }
                        @if let Some(ref long) = long_description {
                            div class="mint-desc-long" { (long) }
                        }
                        @if let Some(ref v) = version {
                            div class="version-chip" { (v) }
                        }
                    }

                    // MOTD
                    @if let Some(ref m) = motd {
                        div class="motd" {
                            div class="motd-label" { "Mint notice" }
                            div class="motd-text" { (m) }
                        }
                    }

                    // Disabled banners
                    @if minting_disabled {
                        div class="disabled-banner" { "Minting is currently disabled" }
                    }
                    @if melting_disabled {
                        div class="disabled-banner" { "Melting is currently disabled" }
                    }

                    // URL section
                    @if !urls.is_empty() {
                        div class="url-section" {
                            div class="url-bar" {
                                span class="url-text" { (urls[0]) }
                            }
                            @if urls.len() > 1 {
                                div class="extra-urls" {
                                    @for url in &urls[1..] {
                                        div class="extra-url" {
                                            span class="url-label" {
                                                @if url.as_str().contains(".onion") {
                                                    "TOR"
                                                } @else {
                                                    "ALT"
                                                }
                                            }
                                            span class="url-text" { (url) }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Unified detail card
                    div class="detail-card" {

                        // Mint details section
                        div class="card-section-header" { "Mint details" }

                        @if !units.is_empty() {
                            div class="detail-row" style="padding-top:14px" {
                                span class="detail-label" { "Units" }
                                div class="detail-value" {
                                    @for unit in &units {
                                        span class="tag" { (unit) }
                                    }
                                }
                            }
                        }

                        div class="detail-row" {
                            span class="detail-label" { "Minting" }
                            div class="detail-value" {
                                @if minting_disabled {
                                    span class="tag tag-red" { "disabled" }
                                } @else {
                                    @for method in &mint_methods {
                                        span class="tag" { (method) }
                                    }
                                }
                            }
                        }

                        div class="detail-row" {
                            span class="detail-label" { "Melting" }
                            div class="detail-value" {
                                @if melting_disabled {
                                    span class="tag tag-red" { "disabled" }
                                } @else {
                                    @for method in &melt_methods {
                                        span class="tag" { (method) }
                                    }
                                }
                            }
                        }

                        @if !mint_limits.is_empty() {
                            div class="detail-row" {
                                span class="detail-label" { "Mint limits" }
                                span class="detail-value detail-value-mono" {
                                    (mint_limits.iter().cloned().collect::<Vec<_>>().join(" · "))
                                }
                            }
                        }

                        @if !melt_limits.is_empty() {
                            div class="detail-row row-last" {
                                span class="detail-label" { "Melt limits" }
                                span class="detail-value detail-value-mono" {
                                    (melt_limits.iter().cloned().collect::<Vec<_>>().join(" · "))
                                }
                            }
                        }

                        // Supported features section
                        @if !supported_features.is_empty() {
                            div class="card-section-header has-rule" { "Supported features" }
                            div style="padding-top:12px" {
                                div class="features-grid" {
                                    @for (_nut_num, feature_name) in &supported_features {
                                        div class="feature" {
                                            div class="feature-dot" {
                                                (maud::PreEscaped(r#"<svg viewBox="0 0 24 24" fill="none" stroke="var(--green)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>"#))
                                            }
                                            span class="feature-name" { (feature_name) }
                                        }
                                    }
                                }
                            }
                        }

                        // Contact section
                        @if !contact.is_empty() {
                            div class="card-section-header has-rule" { "Contact" }
                            div style="padding-top:12px" {
                                div class="contact-chips" {
                                    @for c in &contact {
                                        @if c.method.to_lowercase() == "email" {
                                            a class="contact-chip" href=(format!("mailto:{}", c.info)) target="_blank" {
                                                (maud::PreEscaped(r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="2" y="4" width="20" height="16" rx="2"/><polyline points="22,4 12,13 2,4"/></svg>"#))
                                                (c.info)
                                            }
                                        } @else if c.method.to_lowercase() == "twitter" {
                                            a class="contact-chip" href=(format!("https://x.com/{}", c.info.trim_start_matches('@'))) target="_blank" {
                                                (maud::PreEscaped(r#"<svg viewBox="0 0 24 24" fill="currentColor"><path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z"/></svg>"#))
                                                (c.info)
                                            }
                                        } @else if c.method.to_lowercase() == "nostr" {
                                            a class="contact-chip" href=(format!("https://njump.me/{}", c.info)) target="_blank" {
                                                (maud::PreEscaped(r#"<svg viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="12" r="10"/></svg>"#))
                                                (c.info)
                                            }
                                        } @else {
                                            span class="contact-chip" {
                                                (c.method) ": " (c.info)
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Public key section
                        @if let Some(ref pk) = pubkey {
                            div class="card-section-header has-rule" { "Public key" }
                            div class="pubkey-row" {
                                span class="pubkey-mono" { (pk) }
                            }
                        }
                    }

                    // Info tip
                    div class="info-tip" {
                        div class="info-tip-icon" {
                            (maud::PreEscaped(r#"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" width="18" height="18"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>"#))
                        }
                        div class="info-tip-text" {
                            "To use this mint, copy the mint URL above and add it to a Cashu-compatible wallet such as "
                            a href="https://wallet.cashu.me" target="_blank" { "Cashu.me" }
                            ", "
                            a href="https://macadamia.cash" target="_blank" { "Macadamia" }
                            ", or "
                            a href="https://www.minibits.cash" target="_blank" { "Minibits" }
                            "."
                        }
                    }

                    // Footer
                    div class="footer" {
                        div {
                            "Powered by "
                            a href="https://cashudevkit.org" target="_blank" { "Cashu Development Kit (CDK)" }
                        }
                        div style="margin-top: 8px" {
                            a href="https://iscashucustodial.com/" target="_blank" { "isCashuCustodial.com" }
                        }
                    }
                }
            }
        }
    };

    Ok(markup)
}

#[instrument(skip_all)]
pub(crate) fn into_response<T>(error: T) -> Response
where
    T: Into<ErrorResponse>,
{
    let err_response: ErrorResponse = error.into();
    // Per NUT-00 spec: "In case of an error, mints respond with the HTTP status code 400"
    (StatusCode::BAD_REQUEST, Json(err_response)).into_response()
}
