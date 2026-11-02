use std::collections::HashMap;
use std::str::FromStr;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::{Extension, Json};
use ldk_node::bitcoin::secp256k1::PublicKey;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::{NodeError, UserChannelId};
use maud::html;
use serde::Deserialize;

use crate::web::csrf::{csrf_input, CsrfForm, CsrfToken};
use crate::web::handlers::utils::deserialize_optional_u64;
use crate::web::handlers::AppState;
use crate::web::templates::{
    error_message, form_card, format_sats_as_btc, info_card, is_node_running, layout_with_status,
    success_message,
};

const MSATS_PER_SAT: u64 = 1_000;

#[derive(Deserialize)]
pub struct OpenChannelForm {
    node_id: String,
    address: String,
    port: u32,
    amount_sats: u64,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    push_btc: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateOpenChannelForm {
    _csrf: String,
    node_id: String,
    address: String,
    port: u16,
    amount_sats: u64,
}

/// Initiate a private channel with zero push using the running node.
pub async fn post_open_private_channel(
    State(state): State<AppState>,
    form: Result<CsrfForm<PrivateOpenChannelForm>, StatusCode>,
) -> Response {
    let form = match form {
        Ok(CsrfForm(form)) => form,
        Err(StatusCode::FORBIDDEN) => {
            return private_channel_error(
                StatusCode::FORBIDDEN,
                "csrf_failed",
                "CSRF verification failed",
                "not_started",
            );
        }
        Err(_) => return invalid_private_channel_request("Invalid private channel request"),
    };
    if form.amount_sats == 0 || form.port == 0 {
        return invalid_private_channel_request("Channel amount and port must be positive");
    }
    let pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pubkey) => pubkey,
        Err(_) => return invalid_private_channel_request("Invalid node public key"),
    };
    let address = match SocketAddress::from_str(&format!("{}:{}", form.address, form.port)) {
        Ok(address) => address,
        Err(_) => return invalid_private_channel_request("Invalid address or port"),
    };

    tracing::info!(
        "Initiating private channel to {} with amount {} sats",
        pubkey,
        form.amount_sats
    );
    // This API connects if necessary and always opens an unannounced channel.
    private_channel_response(state.node.inner.open_channel(
        pubkey,
        address,
        form.amount_sats,
        None,
        None,
    ))
}

fn private_channel_response(result: Result<UserChannelId, NodeError>) -> Response {
    match result {
        Ok(id) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "initiated",
                "user_channel_id": id.0.to_string(),
                "announce_channel": false,
            })),
        )
            .into_response(),
        Err(error) => {
            tracing::error!("Private channel opening returned an error: {}", error);
            // LDK can fail persisting the peer after channel creation starts.
            // The caller must inspect channel state before considering a retry.
            private_channel_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "channel_open_failed",
                &error.to_string(),
                "unknown",
            )
        }
    }
}

fn invalid_private_channel_request(message: &str) -> Response {
    private_channel_error(
        StatusCode::BAD_REQUEST,
        "invalid_request",
        message,
        "not_started",
    )
}

fn private_channel_error(
    status: StatusCode,
    error: &str,
    message: &str,
    outcome: &str,
) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": error, "message": message, "outcome": outcome })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct CloseChannelForm {
    channel_id: String,
    node_id: String,
}

pub async fn channels_page(State(_state): State<AppState>) -> Result<Response, StatusCode> {
    // Redirect to the balance page since channels are now part of the Lightning section
    Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/balance")
        .body(Body::empty())
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn open_channel_page(
    State(state): State<AppState>,
    Extension(csrf_token): Extension<CsrfToken>,
) -> Result<Html<String>, StatusCode> {
    let content = form_card(
        "Open New Channel",
        html! {
            form method="post" action="/channels/open" {
                (csrf_input(&csrf_token))
                div class="form-group" {
                    label for="node_id" { "Node Public Key" }
                    input type="text" id="node_id" name="node_id" required placeholder="02..." {}
                }
                div class="form-group" {
                    label for="address" { "Node Address" }
                    input type="text" id="address" name="address" required placeholder="127.0.0.1" {}
                }
                div class="form-group" {
                    label for="port" { "Port" }
                    input type="number" id="port" name="port" required value="9735" {}
                }
                div class="form-group" {
                    label for="amount_btc" { "Channel Size" }
                    input type="number" id="amount_sats" name="amount_sats" required placeholder="₿0" step="1" {}
                }
                div class="form-group" {
                    label for="push_btc" { "Push Amount (optional)" }
                    input type="number" id="push_btc" name="push_btc" placeholder="₿0" step="1" {}
                }
                div class="form-actions" {
                    a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                    button type="submit" class="button-primary" { "Open Channel" }
                }
            }
        },
    );

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Open Channel", content, is_running).into_string(),
    ))
}

pub async fn post_open_channel(
    State(state): State<AppState>,
    CsrfForm(form): CsrfForm<OpenChannelForm>,
) -> Result<Response, StatusCode> {
    tracing::info!(
        "Web interface: Attempting to open channel to node_id={}, address={}:{}, amount_sats={}, push_btc={:?}",
        form.node_id,
        form.address,
        form.port,
        form.amount_sats,
        form.push_btc
    );

    let pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pk) => pk,
        Err(e) => {
            tracing::warn!("Web interface: Invalid node public key provided: {}", e);
            let content = html! {
                (error_message(&format!("Invalid node public key: {e}")))
                div class="card" {
                    a href="/channels/open" { button { "← Try Again" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Open Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let socket_addr = match SocketAddress::from_str(&format!("{}:{}", form.address, form.port)) {
        Ok(addr) => addr,
        Err(e) => {
            tracing::warn!("Web interface: Invalid address:port combination: {}", e);
            let content = html! {
                (error_message(&format!("Invalid address:port combination: {e}")))
                div class="card" {
                    a href="/channels/open" { button { "← Try Again" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Open Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // First connect to the peer
    tracing::info!(
        "Web interface: Connecting to peer {} at {}",
        pubkey,
        socket_addr
    );
    if let Err(e) = state.node.inner.connect(pubkey, socket_addr.clone(), true) {
        tracing::error!("Web interface: Failed to connect to peer {}: {}", pubkey, e);
        let content = html! {
            (error_message(&format!("Failed to connect to peer: {e}")))
            div class="card" {
                a href="/channels/open" { button { "← Try Again" } }
            }
        };
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "text/html")
            .body(Body::from(
                layout_with_status("Open Channel Error", content, true).into_string(),
            ))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }

    let push_msats = match form
        .push_btc
        .map(|amount| amount.checked_mul(MSATS_PER_SAT))
    {
        Some(Some(push_msats)) => Some(push_msats),
        Some(None) => {
            let content = html! {
                (error_message("Push amount is too large"))
                div class="card" {
                    a href="/channels/open" { button { "← Try Again" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Open Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
        None => None,
    };

    // Then open the channel
    tracing::info!(
        "Web interface: Opening announced channel to {} with amount {} sats and push amount {:?} msats",
        pubkey,
        form.amount_sats,
        push_msats
    );
    let channel_result = state.node.inner.open_announced_channel(
        pubkey,
        socket_addr,
        form.amount_sats,
        push_msats,
        None,
    );

    let content = match channel_result {
        Ok(user_channel_id) => {
            tracing::info!(
                "Web interface: Successfully initiated channel opening with user_channel_id={} to {}",
                user_channel_id.0,
                pubkey
            );
            html! {
                (success_message("Channel opening initiated successfully!"))
                (info_card(
                    "Channel Details",
                    vec![
                        ("Temporary Channel ID", user_channel_id.0.to_string()),
                        ("Node ID", form.node_id),
                        ("Amount", format_sats_as_btc(form.amount_sats)),
                        ("Push Amount", form.push_btc.map(format_sats_as_btc).unwrap_or_else(|| "₿ 0".to_string())),
                    ]
                ))
                div class="card" {
                    p { "The channel is now being opened. It may take some time for the channel to become active." }
                    a href="/balance" { button { "← Back to Lightning" } }
                }
            }
        }
        Err(e) => {
            tracing::error!("Web interface: Failed to open channel to {}: {}", pubkey, e);
            html! {
                (error_message(&format!("Failed to open channel: {e}")))
                div class="card" {
                    a href="/channels/open" { button { "← Try Again" } }
                }
            }
        }
    };

    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout_with_status("Open Channel Result", content, true).into_string(),
        ))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn close_channel_page(
    State(state): State<AppState>,
    Extension(csrf_token): Extension<CsrfToken>,
    query: Query<HashMap<String, String>>,
) -> Result<Html<String>, StatusCode> {
    let channel_id = query.get("channel_id").unwrap_or(&"".to_string()).clone();
    let node_id = query.get("node_id").unwrap_or(&"".to_string()).clone();

    if channel_id.is_empty() || node_id.is_empty() {
        let content = html! {
            (error_message("Missing channel ID or node ID"))
            div class="card" {
                a href="/balance" { button { "← Back to Lightning" } }
            }
        };
        return Ok(Html(
            layout_with_status("Close Channel Error", content, true).into_string(),
        ));
    }

    // Get channel information for amount display
    let channels = state.node.inner.list_channels();
    let channel = channels
        .iter()
        .find(|c| c.user_channel_id.0.to_string() == channel_id);

    let content = form_card(
        "Close Channel",
        html! {
            p style="margin-bottom: 1.5rem;" { "Are you sure you want to close this channel?" }

            // Channel details in consistent format
            div class="channel-details" {
                div class="detail-row" {
                    span class="detail-label" { "User Channel ID" }
                    span class="detail-value-amount" { (channel_id) }
                }
                div class="detail-row" {
                    span class="detail-label" { "Node ID" }
                    span class="detail-value-amount" { (node_id) }
                }
                @if let Some(ch) = channel {
                    div class="detail-row" {
                        span class="detail-label" { "Channel Amount" }
                        span class="detail-value-amount" { (format_sats_as_btc(ch.channel_value_sats)) }
                    }
                }
            }

            form method="post" action="/channels/close" style="margin-top: 1rem; display: flex; justify-content: space-between; align-items: center;" {
                (csrf_input(&csrf_token))
                input type="hidden" name="channel_id" value=(channel_id) {}
                input type="hidden" name="node_id" value=(node_id) {}
                a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                button type="submit" class="button-destructive" { "Close Channel" }
            }
        },
    );

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Close Channel", content, is_running).into_string(),
    ))
}

pub async fn force_close_channel_page(
    State(state): State<AppState>,
    Extension(csrf_token): Extension<CsrfToken>,
    query: Query<HashMap<String, String>>,
) -> Result<Html<String>, StatusCode> {
    let channel_id = query.get("channel_id").unwrap_or(&"".to_string()).clone();
    let node_id = query.get("node_id").unwrap_or(&"".to_string()).clone();

    if channel_id.is_empty() || node_id.is_empty() {
        let content = html! {
            (error_message("Missing channel ID or node ID"))
            div class="card" {
                a href="/balance" { button { "← Back to Lightning" } }
            }
        };
        return Ok(Html(
            layout_with_status("Force Close Channel Error", content, true).into_string(),
        ));
    }

    // Get channel information for amount display
    let channels = state.node.inner.list_channels();
    let channel = channels
        .iter()
        .find(|c| c.user_channel_id.0.to_string() == channel_id);

    let content = form_card(
        "Force Close Channel",
        html! {
            div style="border: 2px solid #f97316; background-color: rgba(249, 115, 22, 0.1); padding: 1rem; margin-bottom: 1rem; border-radius: 0.5rem;" {
                h4 style="color: #f97316; margin: 0 0 0.5rem 0;" { "⚠️ Warning: Force Close" }
                p style="color: #f97316; margin: 0; font-size: 0.9rem;" {
                    "Force close should NOT be used if normal close is preferred. "
                    "Force close will immediately broadcast the latest commitment transaction and may result in delayed fund recovery. "
                    "Only use this if the channel counterparty is unresponsive or there are other issues preventing normal closure."
                }
            }
            p style="margin-bottom: 1.5rem;" { "Are you sure you want to force close this channel?" }

            // Channel details in consistent format
            div class="channel-details" {
                div class="detail-row" {
                    span class="detail-label" { "User Channel ID" }
                    span class="detail-value-amount" { (channel_id) }
                }
                div class="detail-row" {
                    span class="detail-label" { "Node ID" }
                    span class="detail-value-amount" { (node_id) }
                }
                @if let Some(ch) = channel {
                    div class="detail-row" {
                        span class="detail-label" { "Channel Amount" }
                        span class="detail-value-amount" { (format_sats_as_btc(ch.channel_value_sats)) }
                    }
                }
            }

            form method="post" action="/channels/force-close" style="margin-top: 1rem; display: flex; justify-content: space-between; align-items: center;" {
                (csrf_input(&csrf_token))
                input type="hidden" name="channel_id" value=(channel_id) {}
                input type="hidden" name="node_id" value=(node_id) {}
                a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                button type="submit" class="button-destructive" { "Force Close Channel" }
            }
        },
    );

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Force Close Channel", content, is_running).into_string(),
    ))
}

pub async fn post_close_channel(
    State(state): State<AppState>,
    CsrfForm(form): CsrfForm<CloseChannelForm>,
) -> Result<Response, StatusCode> {
    tracing::info!(
        "Web interface: Attempting to close channel_id={} with node_id={}",
        form.channel_id,
        form.node_id
    );

    let node_pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pk) => pk,
        Err(e) => {
            tracing::warn!(
                "Web interface: Invalid node public key for channel close: {}",
                e
            );
            let content = html! {
                (error_message(&format!("Invalid node public key: {e}")))
                div class="card" {
                    a href="/channels" { button { "← Back to Channels" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Close Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let channel_id: u128 = match form.channel_id.parse() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("Web interface: Invalid channel ID for channel close: {}", e);
            let content = html! {
                (error_message(&format!("Invalid channel ID: {e}")))
                div class="card" {
                    a href="/channels" { button { "← Back to Channels" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Close Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let user_channel_id = UserChannelId(channel_id);
    tracing::info!(
        "Web interface: Initiating cooperative close for channel {} with {}",
        channel_id,
        node_pubkey
    );
    let close_result = state
        .node
        .inner
        .close_channel(&user_channel_id, node_pubkey);

    let content = match close_result {
        Ok(()) => {
            tracing::info!(
                "Web interface: Successfully initiated cooperative close for channel {} with {}",
                channel_id,
                node_pubkey
            );
            html! {
                (success_message("Channel closing initiated successfully!"))
                div class="card" {
                    p { "The channel is now being closed. It may take some time for the closing transaction to be confirmed." }
                    a href="/balance" { button { "← Back to Lightning" } }
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "Web interface: Failed to close channel {} with {}: {}",
                channel_id,
                node_pubkey,
                e
            );
            html! {
                (error_message(&format!("Failed to close channel: {e}")))
                div class="card" {
                    a href="/balance" { button { "← Back to Lightning" } }
                }
            }
        }
    };

    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout_with_status("Close Channel Result", content, true).into_string(),
        ))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn post_force_close_channel(
    State(state): State<AppState>,
    CsrfForm(form): CsrfForm<CloseChannelForm>,
) -> Result<Response, StatusCode> {
    tracing::info!(
        "Web interface: Attempting to FORCE CLOSE channel_id={} with node_id={}",
        form.channel_id,
        form.node_id
    );

    let node_pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pk) => pk,
        Err(e) => {
            tracing::warn!(
                "Web interface: Invalid node public key for force close: {}",
                e
            );
            let content = html! {
                (error_message(&format!("Invalid node public key: {e}")))
                div class="card" {
                    a href="/channels" { button { "← Back to Channels" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Force Close Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let channel_id: u128 = match form.channel_id.parse() {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!("Web interface: Invalid channel ID for force close: {}", e);
            let content = html! {
                (error_message(&format!("Invalid channel ID: {e}")))
                div class="card" {
                    a href="/channels" { button { "← Back to Channels" } }
                }
            };
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Force Close Channel Error", content, true).into_string(),
                ))
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let user_channel_id = UserChannelId(channel_id);
    tracing::warn!("Web interface: Initiating FORCE CLOSE for channel {} with {} - this will broadcast the latest commitment transaction", channel_id, node_pubkey);
    let force_close_result =
        state
            .node
            .inner
            .force_close_channel(&user_channel_id, node_pubkey, None);

    let content = match force_close_result {
        Ok(()) => {
            tracing::info!(
                "Web interface: Successfully initiated force close for channel {} with {}",
                channel_id,
                node_pubkey
            );
            html! {
                (success_message("Channel force close initiated successfully!"))
                div class="card" style="border: 1px solid #d63384; background-color: rgba(214, 51, 132, 0.1);" {
                    h4 style="color: #d63384;" { "Force Close Complete" }
                    p { "The channel has been force closed. The latest commitment transaction has been broadcast to the network." }
                    p style="color: #d63384; font-size: 0.9rem;" {
                        "Note: Your funds may be subject to a time delay before they can be spent. "
                        "This delay depends on the channel configuration and may be several blocks."
                    }
                    a href="/balance" { button { "← Back to Lightning" } }
                }
            }
        }
        Err(e) => {
            tracing::error!(
                "Web interface: Failed to force close channel {} with {}: {}",
                channel_id,
                node_pubkey,
                e
            );
            html! {
                (error_message(&format!("Failed to force close channel: {e}")))
                div class="card" {
                    a href="/balance" { button { "← Back to Lightning" } }
                }
            }
        }
    };

    Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout_with_status("Force Close Channel Result", content, true).into_string(),
        ))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn private_channel_receipt_preserves_full_user_channel_id() {
        let response = private_channel_response(Ok(UserChannelId(u128::MAX)));
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10_000).await.unwrap()).unwrap();
        assert_eq!(body["status"], "initiated");
        assert_eq!(body["user_channel_id"], u128::MAX.to_string());
        assert_eq!(body["announce_channel"], false);
    }

    #[tokio::test]
    async fn private_channel_persistence_error_has_unknown_outcome() {
        let response = private_channel_response(Err(NodeError::PersistenceFailed));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 10_000).await.unwrap()).unwrap();
        assert_eq!(body["error"], "channel_open_failed");
        assert_eq!(body["message"], NodeError::PersistenceFailed.to_string());
        assert_eq!(body["outcome"], "unknown");
    }
}
