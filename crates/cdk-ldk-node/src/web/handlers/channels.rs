use std::collections::HashMap;
use std::str::FromStr;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::Form;
use ldk_node::bitcoin::secp256k1::PublicKey;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::UserChannelId;
use maud::html;
use serde::Deserialize;

use crate::web::handlers::utils::deserialize_optional_u64;
use crate::web::handlers::AppState;
use crate::web::templates::{
    error_message, form_card, format_sats_as_btc, info_card, layout, success_message,
};

// Node alias cache for consistent display
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap as StdHashMap;

lazy_static::lazy_static! {
    static ref NODE_ALIAS_CACHE: Arc<RwLock<StdHashMap<String, String>>> = Arc::new(RwLock::new(StdHashMap::new()));
}

/// Fetch node alias from external sources
async fn get_node_alias(node_id: &str) -> Option<String> {
    // Check cache first
    {
        let cache = NODE_ALIAS_CACHE.read().await;
        if let Some(alias) = cache.get(node_id) {
            return Some(alias.clone());
        }
    }

    // Try to fetch from 1ml.com API (for mainnet/testnet)
    let alias = fetch_node_alias_from_1ml(node_id).await;

    if let Some(alias) = &alias {
        // Cache the result
        let mut cache = NODE_ALIAS_CACHE.write().await;
        cache.insert(node_id.to_string(), alias.clone());
        return Some(alias.clone());
    }

    // Fallback: Generate a test alias for unknown nodes (useful for testnet/regtest)
    let test_alias = generate_test_alias(node_id);
    let mut cache = NODE_ALIAS_CACHE.write().await;
    cache.insert(node_id.to_string(), test_alias.clone());
    Some(test_alias)
}

/// Fetch node alias from 1ml.com API
async fn fetch_node_alias_from_1ml(node_id: &str) -> Option<String> {
    let client = reqwest::Client::new();
    let url = format!("https://1ml.com/node/{}/json", node_id);

    match client.get(&url).send().await {
        Ok(response) => {
            if response.status().is_success() {
                match response.json::<serde_json::Value>().await {
                    Ok(json) => {
                        if let Some(alias) = json.get("alias").and_then(|v| v.as_str()) {
                            return Some(alias.to_string());
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        Err(_) => {}
    }

    None
}

/// Generate a test alias for nodes that don't have aliases in the database
fn generate_test_alias(node_id: &str) -> String {
    // Use the first 8 characters of the node ID to create a readable alias
    let short_id = &node_id[..8.min(node_id.len())];
    format!("TestNode_{}", short_id)
}

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
pub struct CloseChannelForm {
    channel_id: String,
    node_id: String,
}

pub async fn channels_page(State(_state): State<AppState>) -> Result<Response, StatusCode> {
    // Redirect to the balance page since channels are now part of the Lightning section
    Ok(Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", "/balance")
        .body(Body::empty())
        .unwrap())
}

pub async fn open_channel_page(State(_state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let content = form_card(
        "Open New Channel",
        html! {
            form method="post" action="/channels/open" {
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
                button type="submit" { "Open Channel" }
                " "
                a href="/balance" { button type="button" { "Cancel" } }
            }
        },
    );

    Ok(Html(layout("Open Channel", content).into_string()))
}

pub async fn post_open_channel(
    State(state): State<AppState>,
    Form(form): Form<OpenChannelForm>,
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
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Open Channel Error", content).into_string(),
                ))
                .unwrap());
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
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Open Channel Error", content).into_string(),
                ))
                .unwrap());
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
        return Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "text/html")
            .body(Body::from(
                layout("Open Channel Error", content).into_string(),
            ))
            .unwrap());
    }

    // Then open the channel
    tracing::info!(
        "Web interface: Opening announced channel to {} with amount {} sats and push amount {:?} msats",
        pubkey,
        form.amount_sats,
        form.push_btc.map(|a| a * 1000)
    );
    let channel_result = state.node.inner.open_announced_channel(
        pubkey,
        socket_addr,
        form.amount_sats,
        form.push_btc.map(|a| a * 1000),
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

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("Open Channel Result", content).into_string(),
        ))
        .unwrap())
}

pub async fn close_channel_page(
    State(_state): State<AppState>,
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
        return Ok(Html(layout("Close Channel Error", content).into_string()));
    }

    // Get node alias for consistent display
    let node_alias = get_node_alias(&node_id).await;

    let content = form_card(
        "Close Channel",
        html! {
            p style="margin-bottom: 1.5rem;" { "Are you sure you want to close this channel?" }

            // Channel details in consistent format
            div class="channel-details" {
                @if let Some(alias) = node_alias {
                    div class="detail-row" {
                        span class="detail-label" { "Node Alias" }
                        span class="detail-value" { (alias) }
                    }
                }
                div class="detail-row" {
                    span class="detail-label" { "User Channel ID" }
                    span class="detail-value" { (channel_id) }
                }
                div class="detail-row" {
                    span class="detail-label" { "Node ID" }
                    span class="detail-value" { (node_id) }
                }
            }

            form method="post" action="/channels/close" style="margin-top: 1rem; display: flex; justify-content: space-between; align-items: center;" {
                input type="hidden" name="channel_id" value=(channel_id) {}
                input type="hidden" name="node_id" value=(node_id) {}
                a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                button type="submit" class="button-destructive" { "Close Channel" }
            }
        },
    );

    Ok(Html(layout("Close Channel", content).into_string()))
}

pub async fn force_close_channel_page(
    State(_state): State<AppState>,
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
            layout("Force Close Channel Error", content).into_string(),
        ));
    }

    // Get node alias for consistent display
    let node_alias = get_node_alias(&node_id).await;

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
                @if let Some(alias) = node_alias {
                    div class="detail-row" {
                        span class="detail-label" { "Node Alias" }
                        span class="detail-value" { (alias) }
                    }
                }
                div class="detail-row" {
                    span class="detail-label" { "User Channel ID" }
                    span class="detail-value" { (channel_id) }
                }
                div class="detail-row" {
                    span class="detail-label" { "Node ID" }
                    span class="detail-value" { (node_id) }
                }
            }

            form method="post" action="/channels/force-close" style="margin-top: 1rem; display: flex; justify-content: space-between; align-items: center;" {
                input type="hidden" name="channel_id" value=(channel_id) {}
                input type="hidden" name="node_id" value=(node_id) {}
                a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                button type="submit" class="button-destructive" { "Force Close Channel" }
            }
        },
    );

    Ok(Html(layout("Force Close Channel", content).into_string()))
}

pub async fn post_close_channel(
    State(state): State<AppState>,
    Form(form): Form<CloseChannelForm>,
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
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Close Channel Error", content).into_string(),
                ))
                .unwrap());
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
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Close Channel Error", content).into_string(),
                ))
                .unwrap());
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

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("Close Channel Result", content).into_string(),
        ))
        .unwrap())
}

pub async fn post_force_close_channel(
    State(state): State<AppState>,
    Form(form): Form<CloseChannelForm>,
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
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Force Close Channel Error", content).into_string(),
                ))
                .unwrap());
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
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Force Close Channel Error", content).into_string(),
                ))
                .unwrap());
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

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("Force Close Channel Result", content).into_string(),
        ))
        .unwrap())
}
