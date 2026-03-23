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
/// Get the index page
#[instrument(skip_all)]
pub(crate) async fn get_index(
    State(state): State<MintState>,
) -> Result<impl IntoResponse, Response> {
    use maud::html;

    let mint_info = state.mint.mint_info().await.map_err(into_response)?;

    let name = mint_info.name.clone().unwrap_or("CDK Mint".to_string());
    let description = mint_info.description.clone().unwrap_or_default();
    let long_description = mint_info.description_long.clone();
    let motd = mint_info.motd.clone();
    let pubkey = mint_info.pubkey.map(|p| p.to_hex());
    let version = mint_info.version.as_ref().map(|v| v.to_string());
    let time = mint_info.time;
    let contact = mint_info.contact.clone().unwrap_or_default();
    let tos_url = mint_info.tos_url.clone();
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

    let mut supported_nuts = Vec::new();
    if !mint_info.nuts.nut04.disabled {
        supported_nuts.push(4);
    }
    if !mint_info.nuts.nut05.disabled {
        supported_nuts.push(5);
    }
    if mint_info.nuts.nut07.supported {
        supported_nuts.push(7);
    }
    if mint_info.nuts.nut08.supported {
        supported_nuts.push(8);
    }
    if mint_info.nuts.nut09.supported {
        supported_nuts.push(9);
    }
    if mint_info.nuts.nut10.supported {
        supported_nuts.push(10);
    }
    if mint_info.nuts.nut11.supported {
        supported_nuts.push(11);
    }
    if mint_info.nuts.nut12.supported {
        supported_nuts.push(12);
    }
    if mint_info.nuts.nut14.supported {
        supported_nuts.push(14);
    }
    if !mint_info.nuts.nut15.methods.is_empty() {
        supported_nuts.push(15);
    }
    if !mint_info.nuts.nut17.supported.is_empty() {
        supported_nuts.push(17);
    }
    if !mint_info.nuts.nut19.cached_endpoints.is_empty() {
        supported_nuts.push(19);
    }
    if mint_info.nuts.nut20.supported {
        supported_nuts.push(20);
    }
    if mint_info.nuts.nut21.is_some() {
        supported_nuts.push(21);
    }
    if mint_info.nuts.nut22.is_some() {
        supported_nuts.push(22);
    }
    if !mint_info.nuts.nut29.is_empty() {
        supported_nuts.push(29);
    }

    let markup = html! {
        (maud::DOCTYPE)
        html lang="en" {
            head {
                title { (name) }
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                style {
                    "
                    :root {
                        /* Light mode (default) */
                        --background: 0 0% 100%;
                        --foreground: 222.2 84% 4.9%;
                        --card: 0 0% 100%;
                        --card-foreground: 222.2 84% 4.9%;
                        --popover: 0 0% 100%;
                        --popover-foreground: 222.2 84% 4.9%;
                        --primary: 222.2 47.4% 11.2%;
                        --primary-foreground: 210 40% 98%;
                        --secondary: 210 40% 96%;
                        --secondary-foreground: 222.2 84% 4.9%;
                        --muted: 210 40% 96%;
                        --muted-foreground: 215.4 16.3% 46.9%;
                        --accent: 210 40% 96%;
                        --accent-foreground: 222.2 84% 4.9%;
                        --destructive: 0 84.2% 60.2%;
                        --destructive-foreground: 210 40% 98%;
                        --border: 214.3 31.8% 91.4%;
                        --input: 214.3 31.8% 91.4%;
                        --ring: 222.2 84% 4.9%;
                        --radius: 0;

                        /* Typography scale */
                        --fs-title: 1.25rem;
                        --fs-label: 0.8125rem;
                        --fs-value: 1.625rem;

                        /* Line heights */
                        --lh-tight: 1.15;
                        --lh-normal: 1.4;

                        /* Font weights */
                        --fw-medium: 500;
                        --fw-semibold: 600;
                        --fw-bold: 700;

                        /* Colors */
                        --fg-primary: #0f172a;
                        --fg-muted: #6b7280;

                        /* Header text colors for light mode */
                        --header-title: #000000;
                        --header-subtitle: #333333;
                    }

                    @media (prefers-color-scheme: dark) {
                        body {
                            background: linear-gradient(rgb(23, 25, 29), rgb(18, 19, 21));
                        }

                        :root {
                            --background: 0 0% 0%;
                            --foreground: 0 0% 100%;
                            --card: 0 0% 0%;
                            --card-foreground: 0 0% 100%;
                            --popover: 0 0% 0%;
                            --popover-foreground: 0 0% 100%;
                            --primary: 0 0% 100%;
                            --primary-foreground: 0 0% 0%;
                            --secondary: 0 0% 20%;
                            --secondary-foreground: 0 0% 100%;
                            --muted: 0 0% 20%;
                            --muted-foreground: 0 0% 70%;
                            --accent: 0 0% 20%;
                            --accent-foreground: 0 0% 100%;
                            --destructive: 0 62.8% 30.6%;
                            --destructive-foreground: 0 0% 100%;
                            --border: 0 0% 20%;
                            --input: 0 0% 20%;
                            --ring: 0 0% 83.9%;

                            /* Dark mode text hierarchy colors */
                            --text-primary: #ffffff;
                            --text-secondary: #e6e6e6;
                            --text-tertiary: #cccccc;
                            --text-quaternary: #b3b3b3;
                            --text-muted: #999999;
                            --text-muted-2: #888888;
                            --text-muted-3: #666666;
                            --text-muted-4: #333333;
                            --text-subtle: #1a1a1a;

                            /* Header text colors for dark mode */
                            --header-title: #ffffff;
                            --header-subtitle: #e6e6e6;
                        }

                        .card {
                            background-color: rgba(255, 255, 255, 0.03) !important;
                            border: none !important;
                        }

                        .card h2 {
                            border-bottom-color: rgba(255, 255, 255, 0.1) !important;
                        }

                        .unit-badge {
                            background-color: rgba(255, 255, 255, 0.08) !important;
                            color: var(--text-secondary) !important;
                        }

                        .motd {
                            background-color: rgba(255, 255, 255, 0.05) !important;
                            border-left-color: var(--text-primary) !important;
                        }

                        .info-item {
                            border-bottom-color: rgba(255, 255, 255, 0.1) !important;
                        }

                        .footer {
                            border-top-color: rgba(255, 255, 255, 0.1) !important;
                        }

                        h1, h2, h3, h4, h5, h6 {
                            color: var(--text-primary) !important;
                        }
                    }

                    * {
                        box-sizing: border-box;
                        margin: 0;
                        padding: 0;
                    }

                    body {
                        font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Roboto', 'Oxygen', 'Ubuntu', 'Cantarell', 'Fira Sans', 'Droid Sans', 'Helvetica Neue', sans-serif;
                        font-size: 14px;
                        line-height: 1.5;
                        color: hsl(var(--foreground));
                        background-color: hsl(var(--background));
                        -webkit-font-smoothing: antialiased;
                        -moz-osx-font-smoothing: grayscale;
                        text-rendering: geometricPrecision;
                        min-height: 100vh;
                    }

                    .container {
                        max-width: 1200px;
                        margin: 0 auto;
                        padding: 0 1rem;
                    }

                    @media (min-width: 640px) {
                        .container {
                            padding: 0 2rem;
                        }
                    }

                    header {
                        position: relative;
                        background-color: hsl(var(--background));
                        background-image:
                            linear-gradient(hsl(var(--border)) 1px, transparent 1px),
                            linear-gradient(90deg, hsl(var(--border)) 1px, transparent 1px);
                        background-size: 40px 40px;
                        background-position: -1px -1px;
                        border-bottom: 1px solid hsl(var(--border));
                        margin-bottom: 2rem;
                        width: 100%;
                        height: 200px;
                        display: flex;
                        align-items: center;
                    }

                    header::before {
                        content: '';
                        position: absolute;
                        top: 0;
                        left: 0;
                        right: 0;
                        bottom: 0;
                        background:
                            linear-gradient(90deg, hsl(var(--background)) 0%, transparent 15%, transparent 85%, hsl(var(--background)) 100%),
                            linear-gradient(180deg, hsl(var(--background)) 0%, transparent 15%, transparent 85%, hsl(var(--background)) 100%);
                        pointer-events: none;
                        z-index: 1;
                    }

                    @media (prefers-color-scheme: dark) {
                        header {
                            background-color: rgb(18, 19, 21);
                            background-image:
                                linear-gradient(rgba(255, 255, 255, 0.03) 1px, transparent 1px),
                                linear-gradient(90deg, rgba(255, 255, 255, 0.03) 1px, transparent 1px);
                        }

                        header::before {
                            background:
                                linear-gradient(90deg, rgb(18, 19, 21) 0%, transparent 15%, transparent 85%, rgb(18, 19, 21) 100%),
                                linear-gradient(180deg, rgb(18, 19, 21) 0%, transparent 15%, transparent 85%, rgb(18, 19, 21) 100%);
                        }
                    }

                    header .container {
                        position: relative;
                        z-index: 2;
                        width: 100%;
                        display: flex;
                        align-items: center;
                        gap: 2rem;
                    }

                    .header-avatar {
                        flex-shrink: 0;
                        background-color: hsl(var(--muted) / 0.3);
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 0.75rem;
                        display: flex;
                        align-items: center;
                        justify-content: center;
                        width: 80px;
                        height: 80px;
                    }

                    .header-avatar-image {
                        width: 48px;
                        height: 48px;
                        object-fit: cover;
                        display: block;
                    }

                    .node-info {
                        display: flex;
                        flex-direction: column;
                        gap: 0.25rem;
                    }

                    .node-title {
                        font-size: 1.875rem;
                        font-weight: 600;
                        color: var(--header-title);
                        margin: 0;
                        line-height: 1.1;
                    }

                    .node-subtitle {
                        font-size: 0.75rem;
                        color: var(--fg-muted);
                        font-weight: 500;
                        letter-spacing: 0.05em;
                        text-transform: uppercase;
                    }

                    .card {
                        position: relative;
                        background-color: hsl(var(--card));
                        border: 1px solid hsl(var(--border));
                        border-radius: 0;
                        padding: 1.5rem;
                        margin-bottom: 1.5rem;
                    }

                    .card::before,
                    .card::after {
                        content: '';
                        position: absolute;
                        width: 16px;
                        height: 16px;
                        border: 1px solid hsl(var(--border));
                    }

                    .card::before {
                        top: -1px;
                        left: -1px;
                        border-right: none;
                        border-bottom: none;
                    }

                    .card::after {
                        bottom: -1px;
                        right: -1px;
                        border-left: none;
                        border-top: none;
                    }

                    @media (prefers-color-scheme: dark) {
                        .card::before,
                        .card::after {
                            border-color: rgba(255, 255, 255, 0.2);
                        }
                    }

                    .card h2 {
                        font-size: 0.875rem;
                        font-weight: 600;
                        text-transform: uppercase;
                        letter-spacing: 0.05em;
                        margin-bottom: 1.5rem;
                        padding-bottom: 1rem;
                        border-bottom: 1px solid hsl(var(--border));
                        opacity: 0.5;
                    }

                    .info-grid {
                        display: grid;
                        grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
                        gap: 1.5rem;
                    }

                    .info-item {
                        display: flex;
                        flex-direction: column;
                        gap: 0.5rem;
                        padding: 1rem 0;
                        border-bottom: 1px solid hsl(var(--border));
                    }

                    .info-item:last-child {
                        border-bottom: none;
                    }

                    .label {
                        font-size: var(--fs-label);
                        font-weight: var(--fw-medium);
                        color: var(--fg-muted);
                        text-transform: uppercase;
                        letter-spacing: 0.02em;
                    }

                    .value {
                        font-size: 0.875rem;
                        font-family: 'SF Mono', 'Monaco', 'Inconsolata', 'Roboto Mono', 'Courier New', monospace;
                        word-break: break-all;
                    }

                    .unit-badge {
                        display: inline-block;
                        background: hsl(var(--muted));
                        padding: 0.25rem 0.75rem;
                        font-size: 0.75rem;
                        font-weight: 600;
                        margin-right: 0.5rem;
                        margin-bottom: 0.5rem;
                        color: hsl(var(--foreground));
                        text-decoration: none;
                    }

                    .motd {
                        background: hsl(var(--muted) / 0.5);
                        border-left: 4px solid hsl(var(--primary));
                        padding: 1.25rem;
                        margin-bottom: 2rem;
                        font-style: italic;
                    }

                    .footer {
                        margin-top: 4rem;
                        padding: 2rem 0;
                        border-top: 1px solid hsl(var(--border));
                        text-align: center;
                        font-size: 0.875rem;
                        color: var(--fg-muted);
                    }

                    a {
                        color: inherit;
                        text-decoration: underline;
                        text-underline-offset: 2px;
                    }

                    a:hover {
                        opacity: 0.8;
                    }

                    ul {
                        list-style: none;
                    }

                    @media (max-width: 768px) {
                        header .container {
                            flex-direction: column;
                            text-align: center;
                            justify-content: center;
                            gap: 1rem;
                        }
                        .header-avatar {
                            width: 64px;
                            height: 64px;
                        }
                    }
                    "
                }
            }
            body {
                header {
                    div class="container" {
                        @if let Some(url) = icon_url {
                            div class="header-avatar" {
                                img class="header-avatar-image" src=(url) alt=(name);
                            }
                        }
                        div class="node-info" {
                            p class="node-subtitle" { "Cashu Mint" }
                            h1 class="node-title" { (name) }
                            @if !description.is_empty() {
                                p style="margin-top: 0.5rem; opacity: 0.7;" { (description) }
                            }
                        }
                    }
                }

                main class="container" {
                    @if let Some(m) = motd {
                        div class="motd" {
                            span style="font-weight: bold; margin-right: 0.5rem; font-style: normal;" { "📢" }
                            (m)
                        }
                    }

                    @if let Some(long) = long_description {
                        div class="card" {
                            h2 { "About" }
                            p { (long) }
                        }
                    }

                    div class="card" {
                        h2 { "Connect" }
                        p {
                            "To use this mint, copy one of the Mint URLs listed below and add it to a Cashu-compatible wallet such as "
                            a href="https://cashu.me" target="_blank" { "Cashu.me" }
                            ", "
                            a href="https://macadamia.cash/" target="_blank" { "Macadamia" }
                            ", or "
                            a href="https://minibits.cash/" target="_blank" { "Minibits" }
                            "."
                        }
                    }

                    div class="card" {
                        h2 { "Mint Details" }
                        div class="info-grid" {
                            @if let Some(pk) = pubkey {
                                div class="info-item" {
                                    span class="label" { "Public Key" }
                                    span class="value" { (pk) }
                                }
                            }
                            @if !urls.is_empty() {
                                div class="info-item" {
                                    span class="label" { "Mint URLs" }
                                    div class="value" {
                                        @for url in urls {
                                            div { (url) }
                                        }
                                    }
                                }
                            }
                            @if let Some(v) = version {
                                div class="info-item" {
                                    span class="label" { "Version" }
                                    span class="value" { (v) }
                                }
                            }
                            @if let Some(t) = time {
                                div class="info-item" {
                                    span class="label" { "Server Time" }
                                    span class="value" { (t) }
                                }
                            }
                            @if !units.is_empty() {
                                div class="info-item" {
                                    span class="label" { "Supported Units" }
                                    div {
                                        @for unit in units {
                                            span class="unit-badge" { (unit) }
                                        }
                                    }
                                }
                            }
                            @if !mint_methods.is_empty() {
                                div class="info-item" {
                                    span class="label" { "Minting Methods" }
                                    div {
                                        @for method in mint_methods {
                                            span class="unit-badge" { (method) }
                                        }
                                    }
                                }
                            }
                            @if !melt_methods.is_empty() {
                                div class="info-item" {
                                    span class="label" { "Melting Methods" }
                                    div {
                                        @for method in melt_methods {
                                            span class="unit-badge" { (method) }
                                        }
                                    }
                                }
                            }
                            @if !supported_nuts.is_empty() {
                                div class="info-item" {
                                    span class="label" { "Supported NUTs" }
                                    div {
                                        @for nut in supported_nuts {
                                            a class="unit-badge" href=(format!("https://github.com/cashubtc/nuts/blob/main/{:02}.md", nut)) target="_blank" { "NUT-" (nut) }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    @if !contact.is_empty() || tos_url.is_some() {
                        div class="card" {
                            h2 { "Contact & Support" }
                            div class="info-grid" {
                                @if !contact.is_empty() {
                                    div class="info-item" {
                                        span class="label" { "Contact" }
                                        ul {
                                            @for c in contact {
                                                li {
                                                    span style="font-weight: 600;" { (c.method) ": " }
                                                    @if c.method.to_lowercase() == "nostr" {
                                                        a href=(format!("nostr:{}", c.info)) { (c.info) }
                                                    } @else if c.method.to_lowercase() == "email" {
                                                        a href=(format!("mailto:{}", c.info)) { (c.info) }
                                                    } @else {
                                                        (c.info)
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                @if let Some(url) = tos_url {
                                    div class="info-item" {
                                        span class="label" { "Legal" }
                                        a href=(url) { "Terms of Service" }
                                    }
                                }
                            }
                        }
                    }
                }

                footer class="container" {
                    div class="footer" {
                        p {
                            "Powered by "
                            a href="https://cashudevkit.org/" { "Cashu Development Kit (CDK)" }
                        }
                        p style="margin-top: 0.5rem;" {
                            "Source code: "
                            a href="https://github.com/cashubtc/cdk" { "GitHub" }
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
