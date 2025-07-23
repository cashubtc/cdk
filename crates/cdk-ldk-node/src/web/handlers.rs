use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::Form;
use ldk_node::bitcoin::secp256k1::PublicKey;
use ldk_node::bitcoin::Address;
use ldk_node::lightning::ln::msgs::SocketAddress;
use ldk_node::lightning::offers::offer::Offer;
use ldk_node::lightning_invoice::Bolt11Invoice;
use ldk_node::payment::{PaymentDirection, PaymentKind, PaymentStatus};
use ldk_node::UserChannelId;
use maud::html;
use serde::Deserialize;

// Custom deserializer for optional u32 that handles empty strings
fn deserialize_optional_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<u32>().map(Some).map_err(serde::de::Error::custom),
    }
}

// Custom deserializer for optional u64 that handles empty strings
fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
    }
}

use crate::web::templates::{
    balance_card, error_message, form_card, format_msats_as_btc, format_sats_as_btc, info_card,
    info_card_with_copy, layout, payment_list_item, success_message, usage_metrics_card,
};
use crate::CdkLdkNode;

#[derive(Clone)]
pub struct AppState {
    pub node: Arc<CdkLdkNode>,
}

#[derive(Debug)]
pub struct UsageMetrics {
    pub lightning_inflow_24h: u64,
    pub lightning_outflow_24h: u64,
    pub lightning_inflow_all_time: u64,
    pub lightning_outflow_all_time: u64,
    pub onchain_inflow_24h: u64,
    pub onchain_outflow_24h: u64,
    pub onchain_inflow_all_time: u64,
    pub onchain_outflow_all_time: u64,
}

/// Calculate usage metrics from payment history
fn calculate_usage_metrics(payments: &[ldk_node::payment::PaymentDetails]) -> UsageMetrics {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let twenty_four_hours_ago = now.saturating_sub(24 * 60 * 60);

    let mut metrics = UsageMetrics {
        lightning_inflow_24h: 0,
        lightning_outflow_24h: 0,
        lightning_inflow_all_time: 0,
        lightning_outflow_all_time: 0,
        onchain_inflow_24h: 0,
        onchain_outflow_24h: 0,
        onchain_inflow_all_time: 0,
        onchain_outflow_all_time: 0,
    };

    for payment in payments {
        // Only count successful payments
        if payment.status != PaymentStatus::Succeeded {
            continue;
        }

        let amount_sats = payment.amount_msat.unwrap_or(0) / 1000;
        let is_recent = payment.latest_update_timestamp >= twenty_four_hours_ago;

        match &payment.kind {
            // Lightning payments (BOLT11, BOLT12, Spontaneous)
            PaymentKind::Bolt11 { .. }
            | PaymentKind::Bolt12Offer { .. }
            | PaymentKind::Bolt12Refund { .. }
            | PaymentKind::Spontaneous { .. }
            | PaymentKind::Bolt11Jit { .. } => match payment.direction {
                PaymentDirection::Inbound => {
                    metrics.lightning_inflow_all_time += amount_sats;
                    if is_recent {
                        metrics.lightning_inflow_24h += amount_sats;
                    }
                }
                PaymentDirection::Outbound => {
                    metrics.lightning_outflow_all_time += amount_sats;
                    if is_recent {
                        metrics.lightning_outflow_24h += amount_sats;
                    }
                }
            },
            // On-chain payments
            PaymentKind::Onchain { .. } => match payment.direction {
                PaymentDirection::Inbound => {
                    metrics.onchain_inflow_all_time += amount_sats;
                    if is_recent {
                        metrics.onchain_inflow_24h += amount_sats;
                    }
                }
                PaymentDirection::Outbound => {
                    metrics.onchain_outflow_all_time += amount_sats;
                    if is_recent {
                        metrics.onchain_outflow_24h += amount_sats;
                    }
                }
            },
        }
    }

    metrics
}

pub async fn dashboard(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let node = &state.node.inner;

    let node_id = node.node_id().to_string();
    let alias = node
        .node_alias()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "No alias set".to_string());

    let listening_addresses: Vec<String> = state
        .node
        .inner
        .announcement_addresses()
        .as_ref()
        .unwrap_or(&vec![])
        .iter()
        .map(|a| a.to_string())
        .collect();

    let (num_peers, num_connected_peers) =
        node.list_peers()
            .iter()
            .fold((0, 0), |(mut peers, mut connected), p| {
                if p.is_connected {
                    connected += 1;
                }
                peers += 1;
                (peers, connected)
            });

    let (num_active_channels, num_inactive_channels) =
        node.list_channels()
            .iter()
            .fold((0, 0), |(mut active, mut inactive), c| {
                if c.is_usable {
                    active += 1;
                } else {
                    inactive += 1;
                }
                (active, inactive)
            });

    let balances = node.list_balances();

    let content = html! {
        div class="grid" {
            (info_card_with_copy(
                "Node Information",
                vec![
                    ("Node ID", node_id),
                    ("Alias", alias),
                    ("Listening Addresses", listening_addresses.join(", ")),
                    ("Connected Peers", format!("{num_connected_peers} / {num_peers}")),
                    ("Active Channels", format!("{} / {}", num_active_channels, num_active_channels + num_inactive_channels)),
                ]
            ))

            (balance_card(
                "Balance Summary",
                vec![
                    ("Total Lightning Balance", format_sats_as_btc(balances.total_lightning_balance_sats)),
                    ("Total On-chain Balance", format_sats_as_btc(balances.total_onchain_balance_sats)),
                    ("Spendable On-chain Balance", format_sats_as_btc(balances.spendable_onchain_balance_sats)),
                    ("Combined Total", format_sats_as_btc(balances.total_lightning_balance_sats + balances.total_onchain_balance_sats)),
                ]
            ))

            div class="card" {
                h2 { "Quick Actions" }
                div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem;" {
                    a href="/balance" style="text-decoration: none;" {
                        button style="width: 100%;" { "Lightning Balance" }
                    }
                    a href="/onchain" style="text-decoration: none;" {
                        button style="width: 100%;" { "On-chain Balance" }
                    }
                    a href="/channels/open" style="text-decoration: none;" {
                        button style="width: 100%;" { "Open Channel" }
                    }
                    a href="/invoices" style="text-decoration: none;" {
                        button style="width: 100%;" { "Create Invoice" }
                    }
                    a href="/payments/send" style="text-decoration: none;" {
                        button style="width: 100%;" { "Make Lightning Payment" }
                    }
                }
            }
        }
    };

    Ok(Html(layout("Dashboard", content).into_string()))
}

pub async fn get_new_address(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let address_result = state.node.inner.onchain_payment().new_address();

    let content = match address_result {
        Ok(address) => {
            html! {
                (success_message(&format!("New address generated: {address}")))
                div class="card" {
                    h2 { "Bitcoin Address" }
                    div class="info-item" {
                        span class="info-label" { "Address:" }
                        span class="info-value" style="font-family: monospace; font-size: 0.9rem;" { (address.to_string()) }
                    }
                }
                div class="card" {
                    a href="/onchain" { button { "← Back to On-chain" } }
                    " "
                    a href="/onchain/new-address" { button { "Generate Another Address" } }
                }
            }
        }
        Err(e) => {
            html! {
                (error_message(&format!("Failed to generate address: {e}")))
                div class="card" {
                    a href="/onchain" { button { "← Back to On-chain" } }
                }
            }
        }
    };

    Ok(Html(layout("New Address", content).into_string()))
}

pub async fn balance_page(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let balances = state.node.inner.list_balances();
    let channels = state.node.inner.list_channels();

    let (num_active_channels, num_inactive_channels) =
        channels
            .iter()
            .fold((0, 0), |(mut active, mut inactive), c| {
                if c.is_usable {
                    active += 1;
                } else {
                    inactive += 1;
                }
                (active, inactive)
            });

    let content = if channels.is_empty() {
        html! {
            div class="card" {
                h2 { "Lightning Channels" }

                // Quick Actions moved to the top
                div style="margin-bottom: 2rem; padding-bottom: 1rem; border-bottom: 1px solid #eee;" {
                    h3 { "Quick Actions" }
                    div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem;" {
                        a href="/invoices" style="text-decoration: none;" {
                            button style="width: 100%;" { "Create Lightning Invoice" }
                        }
                        a href="/payments/send" style="text-decoration: none;" {
                            button style="width: 100%;" { "Make Lightning Payment" }
                        }
                        a href="/channels/open" style="text-decoration: none;" {
                            button style="width: 100%;" { "Open New Channel" }
                        }
                        a href="/onchain" style="text-decoration: none;" {
                            button style="width: 100%;" { "View On-chain Balance" }
                        }
                    }
                }

                // Balance information
                (balance_card(
                    "Balance Information",
                    vec![
                        ("Total Lightning Balance", format_sats_as_btc(balances.total_lightning_balance_sats)),
                        ("Active Channels", format!("{} / {}", num_active_channels, num_active_channels + num_inactive_channels)),
                    ]
                ))

                p { "No channels found. Create your first channel to start using Lightning Network." }

                // Add Open New Channel button at the bottom of the card
                div style="margin-top: 1rem; border-top: 1px solid #eee; padding-top: 1rem;" {
                    a href="/channels/open" {
                        button style="width: 100%;" { "Open New Channel" }
                    }
                }
            }
        }
    } else {
        html! {
            div class="card" {
                h2 { "Lightning Channels" }

                // Quick Actions moved to the top
                div style="margin-bottom: 2rem; padding-bottom: 1rem; border-bottom: 1px solid #eee;" {
                    h3 { "Quick Actions" }
                    div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem;" {
                        a href="/invoices" style="text-decoration: none;" {
                            button style="width: 100%;" { "Create Lightning Invoice" }
                        }
                        a href="/payments/send" style="text-decoration: none;" {
                            button style="width: 100%;" { "Make Lightning Payment" }
                        }
                        a href="/channels/open" style="text-decoration: none;" {
                            button style="width: 100%;" { "Open New Channel" }
                        }
                        a href="/onchain" style="text-decoration: none;" {
                            button style="width: 100%;" { "View On-chain Balance" }
                        }
                    }
                }

                // Balance information
                (balance_card(
                    "Balance Information",
                    vec![
                        ("Total Lightning Balance", format_sats_as_btc(balances.total_lightning_balance_sats)),
                        ("Active Channels", format!("{} / {}", num_active_channels, num_active_channels + num_inactive_channels)),
                    ]
                ))

                // Channels list
                @for channel in &channels {
                    div class="channel-item" {
                        div class="channel-header" {
                            span class="channel-id" { "Channel ID: " (channel.channel_id.to_string()) }
                            @if channel.is_usable {
                                span class="status-badge status-active" { "Active" }
                            } @else {
                                span class="status-badge status-inactive" { "Inactive" }
                            }
                        }
                        div class="info-item" {
                            span class="info-label" { "Counterparty" }
                            span class="info-value" style="font-family: monospace; font-size: 0.85rem;" { (channel.counterparty_node_id.to_string()) }
                        }
                        @if let Some(short_channel_id) = channel.short_channel_id {
                            div class="info-item" {
                                span class="info-label" { "Short Channel ID" }
                                span class="info-value" { (short_channel_id.to_string()) }
                            }
                        }
                        div class="balance-info" {
                            div class="balance-item" {
                                div class="balance-amount" { (format_sats_as_btc(channel.outbound_capacity_msat / 1000)) }
                                div class="balance-label" { "Outbound" }
                            }
                            div class="balance-item" {
                                div class="balance-amount" { (format_sats_as_btc(channel.inbound_capacity_msat / 1000)) }
                                div class="balance-label" { "Inbound" }
                            }
                            div class="balance-item" {
                                div class="balance-amount" { (format_sats_as_btc(channel.channel_value_sats)) }
                                div class="balance-label" { "Total" }
                            }
                        }
                        @if channel.is_usable {
                            div style="margin-top: 1rem;" {
                                a href=(format!("/channels/close?channel_id={}&node_id={}", channel.user_channel_id.0, channel.counterparty_node_id)) {
                                    button style="background: #dc3545;" { "Close Channel" }
                                }
                            }
                        }
                    }
                }

                // Add Open New Channel button at the bottom of the card
                div style="margin-top: 1rem; border-top: 1px solid #eee; padding-top: 1rem;" {
                    a href="/channels/open" {
                        button style="width: 100%;" { "Open New Channel" }
                    }
                }
            }
        }
    };

    Ok(Html(layout("Lightning", content).into_string()))
}

#[derive(Deserialize)]
pub struct SendOnchainActionForm {
    address: String,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    amount_sat: Option<u64>,
    send_action: String,
}

pub async fn onchain_page(
    State(state): State<AppState>,
    query: Query<HashMap<String, String>>,
) -> Result<Html<String>, StatusCode> {
    let balances = state.node.inner.list_balances();
    let action = query
        .get("action")
        .map(|s| s.as_str())
        .unwrap_or("overview");

    let mut content = html! {
        // Balance overview for onchain
        (balance_card(
            "On-chain Balance",
            vec![
                ("Total On-chain Balance", format_sats_as_btc(balances.total_onchain_balance_sats)),
                ("Spendable On-chain Balance", format_sats_as_btc(balances.spendable_onchain_balance_sats)),
            ]
        ))
    };

    match action {
        "send" => {
            // Show send form
            content = html! {
                (content)
                (form_card(
                    "Send On-chain Payment",
                    html! {
                        form method="post" action="/onchain/send" {
                            div class="form-group" {
                                label for="address" { "Recipient Address" }
                                input type="text" id="address" name="address" required placeholder="bc1..." {}
                            }
                            div class="form-group" {
                                label for="amount_sat" { "Amount (sats)" }
                                input type="number" id="amount_sat" name="amount_sat" placeholder="0" {}
                            }
                            input type="hidden" id="send_action" name="send_action" value="send" {}
                            button type="submit" onclick="document.getElementById('send_action').value='send'" { "Send Payment" }
                            " "
                            button type="submit" onclick="document.getElementById('send_action').value='send_all'; document.getElementById('amount_sat').value=''" { "Send All" }
                            " "
                            a href="/onchain" { button type="button" { "Cancel" } }
                        }
                    }
                ))
            };
        }
        "receive" => {
            // Show generate address form
            content = html! {
                (content)
                (form_card(
                    "Generate New Address",
                    html! {
                        form method="post" action="/onchain/new-address" {
                            p { "Click the button below to generate a new Bitcoin address for receiving on-chain payments." }
                            button type="submit" { "Generate New Address" }
                            " "
                            a href="/onchain" { button type="button" { "Cancel" } }
                        }
                    }
                ))
            };
        }
        _ => {
            // Show actions overview
            content = html! {
                (content)
                div class="grid" {
                    div class="card card-flex" {
                        div class="card-flex-content" {
                            h2 { "Receive Bitcoin" }
                            p { "Generate a new Bitcoin address to receive on-chain payments." }
                        }
                        div class="card-flex-button" {
                            a href="/onchain?action=receive" {
                                button style="width: 100%;" { "Generate New Address" }
                            }
                        }
                    }

                    div class="card card-flex" {
                        div class="card-flex-content" {
                            h2 { "Send Bitcoin" }
                            p { "Send Bitcoin to any address on the network." }
                        }
                        div class="card-flex-button" {
                            a href="/onchain?action=send" {
                                button style="width: 100%;" { "Send Payment" }
                            }
                        }
                    }
                }
            };
        }
    }

    Ok(Html(layout("On-chain", content).into_string()))
}

pub async fn post_send_onchain(
    State(state): State<AppState>,
    Form(form): Form<SendOnchainActionForm>,
) -> Result<Response, StatusCode> {
    let address_result = Address::from_str(&form.address);
    let address = match address_result {
        Ok(addr) => addr,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid address: {e}")))
                div class="card" {
                    a href="/onchain" { button { "← Back" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout("Send On-chain Error", content).into_string(),
                ))
                .unwrap());
        }
    };

    // Handle send all action
    let txid_result = if form.send_action == "send_all" {
        // Use send_all_to_address function
        state.node.inner.onchain_payment().send_all_to_address(
            address.assume_checked_ref(),
            false,
            None,
        )
    } else {
        // Use regular send_to_address function
        state.node.inner.onchain_payment().send_to_address(
            address.assume_checked_ref(),
            form.amount_sat.ok_or(StatusCode::BAD_REQUEST)?,
            None,
        )
    };

    let content = match txid_result {
        Ok(txid) => {
            html! {
                (success_message("Transaction sent successfully!"))
                (info_card(
                    "Transaction Details",
                    vec![
                        ("Transaction ID", txid.to_string()),
                        ("Amount", if form.send_action == "send_all" { "All available funds".to_string() } else { format_sats_as_btc(form.amount_sat.unwrap_or(0)) }),
                        ("Recipient", form.address),
                    ]
                ))
                div class="card" {
                    a href="/onchain" { button { "← Back to On-chain" } }
                }
            }
        }
        Err(e) => {
            html! {
                (error_message(&format!("Failed to send payment: {e}")))
                div class="card" {
                    a href="/onchain" { button { "← Try Again" } }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("Send On-chain Result", content).into_string(),
        ))
        .unwrap())
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

#[derive(Deserialize)]
pub struct OpenChannelForm {
    node_id: String,
    address: String,
    port: u32,
    amount_sats: u64,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    push_btc: Option<u64>,
}

pub async fn post_open_channel(
    State(state): State<AppState>,
    Form(form): Form<OpenChannelForm>,
) -> Result<Response, StatusCode> {
    let pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pk) => pk,
        Err(e) => {
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
    if let Err(e) = state.node.inner.connect(pubkey, socket_addr.clone(), true) {
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

    // Convert Bitcoin to millisatoshis (1 BTC = 100,000,000,000 msats)

    // Then open the channel
    let channel_result = state.node.inner.open_announced_channel(
        pubkey,
        socket_addr,
        form.amount_sats,
        form.push_btc.map(|a| a * 1000), // Pass None when empty, Some(value) when provided
        None,
    );

    let content = match channel_result {
        Ok(user_channel_id) => {
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

    let content = form_card(
        "Close Channel",
        html! {
            p { "Are you sure you want to close this channel?" }
            div class="info-item" {
                span class="info-label" { "User Channel ID:" }
                span class="info-value" style="font-family: monospace; font-size: 0.85rem;" { (channel_id) }
            }
            div class="info-item" {
                span class="info-label" { "Node ID:" }
                span class="info-value" style="font-family: monospace; font-size: 0.85rem;" { (node_id) }
            }
            form method="post" action="/channels/close" style="margin-top: 1rem;" {
                input type="hidden" name="channel_id" value=(channel_id) {}
                input type="hidden" name="node_id" value=(node_id) {}
                button type="submit" style="background: #dc3545;" { "Close Channel" }
                " "
                a href="/balance" { button type="button" { "Cancel" } }
            }
        },
    );

    Ok(Html(layout("Close Channel", content).into_string()))
}

#[derive(Deserialize)]
pub struct CloseChannelForm {
    channel_id: String,
    node_id: String,
}

pub async fn post_close_channel(
    State(state): State<AppState>,
    Form(form): Form<CloseChannelForm>,
) -> Result<Response, StatusCode> {
    let node_pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pk) => pk,
        Err(e) => {
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
    let close_result = state
        .node
        .inner
        .close_channel(&user_channel_id, node_pubkey);

    let content = match close_result {
        Ok(()) => {
            html! {
                (success_message("Channel closing initiated successfully!"))
                div class="card" {
                    p { "The channel is now being closed. It may take some time for the closing transaction to be confirmed." }
                    a href="/balance" { button { "← Back to Lightning" } }
                }
            }
        }
        Err(e) => {
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

pub async fn invoices_page(State(_state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let content = html! {
        div class="grid" {
            (form_card(
                "Create BOLT11 Invoice",
                html! {
                    form method="post" action="/invoices/bolt11" {
                        div class="form-group" {
                            label for="amount_btc" { "Amount" }
                            input type="number" id="amount_btc" name="amount_btc" required placeholder="₿0" step="0.00000001" {}
                        }
                        div class="form-group" {
                            label for="description" { "Description (optional)" }
                            input type="text" id="description" name="description" placeholder="Payment for..." {}
                        }
                        div class="form-group" {
                            label for="expiry_seconds" { "Expiry (seconds, optional)" }
                            input type="number" id="expiry_seconds" name="expiry_seconds" placeholder="3600" {}
                        }
                        button type="submit" { "Create BOLT11 Invoice" }
                    }
                }
            ))

            (form_card(
                "Create BOLT12 Offer",
                html! {
                    form method="post" action="/invoices/bolt12" {
                        div class="form-group" {
                            label for="amount_btc" { "Amount (optional for variable amount)" }
                            input type="number" id="amount_btc" name="amount_btc" placeholder="₿0" step="0.00000001" {}
                        }
                        div class="form-group" {
                            label for="description" { "Description (optional)" }
                            input type="text" id="description" name="description" placeholder="Payment for..." {}
                        }
                        div class="form-group" {
                            label for="expiry_seconds" { "Expiry (seconds, optional)" }
                            input type="number" id="expiry_seconds" name="expiry_seconds" placeholder="3600" {}
                        }
                        button type="submit" { "Create BOLT12 Offer" }
                    }
                }
            ))
        }
    };

    Ok(Html(layout("Create Invoices", content).into_string()))
}

#[derive(Deserialize)]
pub struct CreateBolt11Form {
    amount_btc: u64,
    description: Option<String>,
    #[serde(deserialize_with = "deserialize_optional_u32")]
    expiry_seconds: Option<u32>,
}

pub async fn post_create_bolt11(
    State(state): State<AppState>,
    Form(form): Form<CreateBolt11Form>,
) -> Result<Response, StatusCode> {
    use ldk_node::lightning_invoice::{Bolt11InvoiceDescription, Description};

    // Handle optional description
    let description_text = form.description.clone().unwrap_or_else(|| "".to_string());
    let description = if description_text.is_empty() {
        // Use empty description for empty or missing description
        match Description::new("".to_string()) {
            Ok(desc) => Bolt11InvoiceDescription::Direct(desc),
            Err(_) => {
                // Fallback to a minimal valid description
                let desc = Description::new(" ".to_string()).unwrap();
                Bolt11InvoiceDescription::Direct(desc)
            }
        }
    } else {
        match Description::new(description_text.clone()) {
            Ok(desc) => Bolt11InvoiceDescription::Direct(desc),
            Err(e) => {
                let content = html! {
                    (error_message(&format!("Invalid description: {e}")))
                    div class="card" {
                        a href="/invoices" { button { "← Try Again" } }
                    }
                };
                return Ok(Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header("content-type", "text/html")
                    .body(Body::from(
                        layout("Create Invoice Error", content).into_string(),
                    ))
                    .unwrap());
            }
        }
    };

    // Convert Bitcoin to millisatoshis
    let amount_msats = form.amount_btc * 1_000;

    let expiry_seconds = form.expiry_seconds.unwrap_or(3600);
    let invoice_result =
        state
            .node
            .inner
            .bolt11_payment()
            .receive(amount_msats, &description, expiry_seconds);

    let content = match invoice_result {
        Ok(invoice) => {
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let description_display = if description_text.is_empty() {
                "None".to_string()
            } else {
                description_text.clone()
            };

            html! {
                (success_message("BOLT11 Invoice created successfully!"))
                (info_card(
                    "Invoice Details",
                    vec![
                        ("Payment Hash", invoice.payment_hash().to_string()),
                        ("Amount", format_sats_as_btc(form.amount_btc)),
                        ("Description", description_display),
                        ("Expires At", format!("{}", current_time + expiry_seconds as u64)),
                    ]
                ))
                div class="card" {
                    h3 { "Invoice (copy this to share)" }
                    textarea readonly style="width: 100%; height: 150px; font-family: monospace; font-size: 0.8rem;" {
                        (invoice.to_string())
                    }
                }
                div class="card" {
                    a href="/invoices" { button { "← Create Another Invoice" } }
                }
            }
        }
        Err(e) => {
            html! {
                (error_message(&format!("Failed to create invoice: {e}")))
                div class="card" {
                    a href="/invoices" { button { "← Try Again" } }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("BOLT11 Invoice Created", content).into_string(),
        ))
        .unwrap())
}

#[derive(Deserialize)]
pub struct CreateBolt12Form {
    amount_btc: Option<f64>,
    description: Option<String>,
    #[serde(deserialize_with = "deserialize_optional_u32")]
    expiry_seconds: Option<u32>,
}

pub async fn post_create_bolt12(
    State(state): State<AppState>,
    Form(form): Form<CreateBolt12Form>,
) -> Result<Response, StatusCode> {
    let expiry_seconds = form.expiry_seconds.unwrap_or(3600);
    let description_text = form.description.unwrap_or_else(|| "".to_string());

    let offer_result = if let Some(amount_btc) = form.amount_btc {
        // Convert Bitcoin to millisatoshis (1 BTC = 100,000,000,000 msats)
        let amount_msats = (amount_btc * 100_000_000_000.0) as u64;
        state.node.inner.bolt12_payment().receive(
            amount_msats,
            &description_text,
            Some(expiry_seconds),
            None,
        )
    } else {
        state
            .node
            .inner
            .bolt12_payment()
            .receive_variable_amount(&description_text, Some(expiry_seconds))
    };

    let content = match offer_result {
        Ok(offer) => {
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let amount_display = form
                .amount_btc
                .map(|a| format_sats_as_btc((a * 100_000_000.0) as u64))
                .unwrap_or_else(|| "Variable amount".to_string());

            let description_display = if description_text.is_empty() {
                "None".to_string()
            } else {
                description_text
            };

            html! {
                (success_message("BOLT12 Offer created successfully!"))
                (info_card(
                    "Offer Details",
                    vec![
                        ("Offer ID", offer.id().to_string()),
                        ("Amount", amount_display),
                        ("Description", description_display),
                        ("Expires At", format!("{}", current_time + expiry_seconds as u64)),
                    ]
                ))
                div class="card" {
                    h3 { "Offer (copy this to share)" }
                    textarea readonly style="width: 100%; height: 150px; font-family: monospace; font-size: 0.8rem;" {
                        (offer.to_string())
                    }
                }
                div class="card" {
                    a href="/invoices" { button { "← Create Another Offer" } }
                }
            }
        }
        Err(e) => {
            html! {
                (error_message(&format!("Failed to create offer: {e}")))
                div class="card" {
                    a href="/invoices" { button { "← Try Again" } }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("BOLT12 Offer Created", content).into_string(),
        ))
        .unwrap())
}

#[derive(Deserialize)]
pub struct PaymentsQuery {
    filter: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

pub async fn payments_page(
    State(state): State<AppState>,
    query: Query<PaymentsQuery>,
) -> Result<Html<String>, StatusCode> {
    let filter = query.filter.as_deref().unwrap_or("all");
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(25).clamp(10, 100); // Limit between 10-100 items per page

    // Get all payments using list_payments_with_filter
    let all_payments = state.node.inner.list_payments_with_filter(|_| true);

    // Calculate usage metrics from all payments
    let metrics = calculate_usage_metrics(&all_payments);

    // Filter payments based on the filter parameter
    let mut filtered_payments: Vec<_> = match filter {
        "incoming" => all_payments
            .into_iter()
            .filter(|p| p.direction == PaymentDirection::Inbound)
            .collect(),
        "outgoing" => all_payments
            .into_iter()
            .filter(|p| p.direction == PaymentDirection::Outbound)
            .collect(),
        _ => all_payments,
    };

    // Sort payments by latest_update_timestamp with newest first
    filtered_payments.sort_by(|a, b| b.latest_update_timestamp.cmp(&a.latest_update_timestamp));

    // Calculate pagination
    let total_payments = filtered_payments.len();
    let total_pages = ((total_payments as f64) / (per_page as f64)).ceil() as u32;
    let start_index = ((page - 1) * per_page) as usize;
    let end_index = (start_index + per_page as usize).min(total_payments);

    // Get the current page of payments
    let current_page_payments = if start_index < total_payments {
        &filtered_payments[start_index..end_index]
    } else {
        &[]
    };

    // Helper function to build URL with pagination params
    let build_url = |new_page: u32, new_filter: &str, new_per_page: u32| -> String {
        let mut params = vec![];
        if new_filter != "all" {
            params.push(format!("filter={}", new_filter));
        }
        if new_page != 1 {
            params.push(format!("page={}", new_page));
        }
        if new_per_page != 25 {
            params.push(format!("per_page={}", new_per_page));
        }

        if params.is_empty() {
            "/payments".to_string()
        } else {
            format!("/payments?{}", params.join("&"))
        }
    };

    let content = html! {
        div class="card" {
            div class="payment-list-header" {
                h2 { "All Payments" }
                p style="margin: 0.5rem 0; color: #666; font-size: 0.9rem;" {
                    "Lightning (BOLT11, BOLT12, Spontaneous) and On-chain payments"
                    @if total_payments > 0 {
                        " - Showing " (start_index + 1) " to " (end_index) " of " (total_payments) " payments"
                    }
                }
                div class="payment-filter-tabs" {
                    a href=(build_url(1, "all", per_page)) class=(if filter == "all" { "payment-filter-tab active" } else { "payment-filter-tab" }) { "All" }
                    a href=(build_url(1, "incoming", per_page)) class=(if filter == "incoming" { "payment-filter-tab active" } else { "payment-filter-tab" }) { "Incoming" }
                    a href=(build_url(1, "outgoing", per_page)) class=(if filter == "outgoing" { "payment-filter-tab active" } else { "payment-filter-tab" }) { "Outgoing" }
                }
            }

            // Usage metrics instead of quick actions
            div style="margin-bottom: 2rem; padding-bottom: 1rem; border-bottom: 1px solid #eee;" {
                h3 { "Usage Metrics" }
                div class="grid" style="margin-top: 1rem;" {
                    (usage_metrics_card(
                        "Lightning Network",
                        vec![
                            ("24h Inflow", format_sats_as_btc(metrics.lightning_inflow_24h)),
                            ("24h Outflow", format_sats_as_btc(metrics.lightning_outflow_24h)),
                            ("All-time Inflow", format_sats_as_btc(metrics.lightning_inflow_all_time)),
                            ("All-time Outflow", format_sats_as_btc(metrics.lightning_outflow_all_time)),
                        ]
                    ))
                    (usage_metrics_card(
                        "On-chain",
                        vec![
                            ("24h Inflow", format_sats_as_btc(metrics.onchain_inflow_24h)),
                            ("24h Outflow", format_sats_as_btc(metrics.onchain_outflow_24h)),
                            ("All-time Inflow", format_sats_as_btc(metrics.onchain_inflow_all_time)),
                            ("All-time Outflow", format_sats_as_btc(metrics.onchain_outflow_all_time)),
                        ]
                    ))
                }
            }

            // Payment list
            @if current_page_payments.is_empty() {
                @if total_payments == 0 {
                    p { "No payments found." }
                } @else {
                    p { "No payments found on this page. "
                        a href=(build_url(1, filter, per_page)) { "Go to first page" }
                    }
                }
            } @else {
                @for payment in current_page_payments {
                    @let direction_str = match payment.direction {
                        PaymentDirection::Inbound => "Inbound",
                        PaymentDirection::Outbound => "Outbound",
                    };

                    @let status_str = match payment.status {
                        PaymentStatus::Pending => "Pending",
                        PaymentStatus::Succeeded => "Succeeded",
                        PaymentStatus::Failed => "Failed",
                    };

                    @let amount_str = payment.amount_msat.map(format_msats_as_btc).unwrap_or_else(|| "Unknown".to_string());

                    @let (payment_hash, description, payment_type, preimage) = match &payment.kind {
                        PaymentKind::Bolt11 { hash, preimage, .. } => {
                            (Some(hash.to_string()), None::<String>, "BOLT11", preimage.map(|p| p.to_string()))
                        },
                        PaymentKind::Bolt12Offer { hash, offer_id, preimage, .. } => {
                            // For BOLT12, we can use either the payment hash or offer ID
                            let identifier = hash.map(|h| h.to_string()).unwrap_or_else(|| offer_id.to_string());
                            (Some(identifier), None::<String>, "BOLT12", preimage.map(|p| p.to_string()))
                        },
                        PaymentKind::Bolt12Refund { hash, preimage, .. } => {
                            (hash.map(|h| h.to_string()), None::<String>, "BOLT12", preimage.map(|p| p.to_string()))
                        },
                        PaymentKind::Spontaneous { hash, preimage, .. } => {
                            (Some(hash.to_string()), None::<String>, "Spontaneous", preimage.map(|p| p.to_string()))
                        },
                        PaymentKind::Onchain { txid, .. } => {
                            (Some(txid.to_string()), None::<String>, "On-chain", None)
                        },
                        PaymentKind::Bolt11Jit { hash, .. } => {
                            (Some(hash.to_string()), None::<String>, "BOLT11 JIT", None)
                        },
                    };

                    (payment_list_item(
                        &payment.id.to_string(),
                        direction_str,
                        status_str,
                        &amount_str,
                        payment_hash.as_deref(),
                        description.as_deref(),
                        Some(payment.latest_update_timestamp), // Use the actual timestamp
                        payment_type,
                        preimage.as_deref(),
                    ))
                }
            }

            // Pagination controls (bottom)
            @if total_pages > 1 {
                div class="pagination-controls" style="margin-top: 2rem; padding-top: 1rem; border-top: 1px solid #eee;" {
                    div class="pagination" style="display: flex; justify-content: center; align-items: center; gap: 0.5rem;" {
                        // Previous page
                        @if page > 1 {
                            a href=(build_url(page - 1, filter, per_page)) class="pagination-btn" { "← Previous" }
                        } @else {
                            span class="pagination-btn disabled" { "← Previous" }
                        }

                        // Page numbers
                        @let start_page = (page.saturating_sub(2)).max(1);
                        @let end_page = (page + 2).min(total_pages);

                        @if start_page > 1 {
                            a href=(build_url(1, filter, per_page)) class="pagination-number" { "1" }
                            @if start_page > 2 {
                                span class="pagination-ellipsis" { "..." }
                            }
                        }

                        @for p in start_page..=end_page {
                            @if p == page {
                                span class="pagination-number active" { (p) }
                            } @else {
                                a href=(build_url(p, filter, per_page)) class="pagination-number" { (p) }
                            }
                        }

                        @if end_page < total_pages {
                            @if end_page < total_pages - 1 {
                                span class="pagination-ellipsis" { "..." }
                            }
                            a href=(build_url(total_pages, filter, per_page)) class="pagination-number" { (total_pages) }
                        }

                        // Next page
                        @if page < total_pages {
                            a href=(build_url(page + 1, filter, per_page)) class="pagination-btn" { "Next →" }
                        } @else {
                            span class="pagination-btn disabled" { "Next →" }
                        }
                    }
                }
            }

            // Per-page selector (bottom)
            @if total_payments > 0 {
                div style="margin-top: 1rem; padding-top: 1rem; border-top: 1px solid #eee; display: flex; justify-content: center; align-items: center; gap: 0.5rem;" {
                    label for="per-page" style="font-size: 0.9rem; color: #6c757d;" { "Show:" }
                    select id="per-page" onchange="changePage()" style="padding: 0.25rem; font-size: 0.9rem; border: 1px solid #dee2e6; border-radius: 4px;" {
                        option value="10" selected[per_page == 10] { "10" }
                        option value="25" selected[per_page == 25] { "25" }
                        option value="50" selected[per_page == 50] { "50" }
                        option value="100" selected[per_page == 100] { "100" }
                    }
                    span style="font-size: 0.9rem; color: #6c757d;" { "payments per page" }
                }
            }
        }

        // JavaScript for per-page selector
        script {
            "function changePage() {
                const perPageSelect = document.getElementById('per-page');
                const newPerPage = perPageSelect.value;
                const currentUrl = new URL(window.location);
                currentUrl.searchParams.set('per_page', newPerPage);
                currentUrl.searchParams.set('page', '1'); // Reset to first page when changing per_page
                window.location.href = currentUrl.toString();
            }"
        }
    };

    Ok(Html(layout("Payments", content).into_string()))
}

pub async fn send_payments_page(
    State(_state): State<AppState>,
) -> Result<Html<String>, StatusCode> {
    let content = html! {
        div class="grid" {
            (form_card(
                "Pay BOLT11 Invoice",
                html! {
                    form method="post" action="/payments/bolt11" {
                        div class="form-group" {
                            label for="invoice" { "BOLT11 Invoice" }
                            textarea id="invoice" name="invoice" required placeholder="lnbc..." style="height: 120px;" {}
                        }
                        div class="form-group" {
                            label for="amount_btc" { "Amount Override (optional)" }
                            input type="number" id="amount_btc" name="amount_btc" placeholder="Leave empty to use invoice amount" step="1" {}
                        }
                        button type="submit" { "Pay BOLT11 Invoice" }
                    }
                }
            ))

            (form_card(
                "Pay BOLT12 Offer",
                html! {
                    form method="post" action="/payments/bolt12" {
                        div class="form-group" {
                            label for="offer" { "BOLT12 Offer" }
                            textarea id="offer" name="offer" required placeholder="lno..." style="height: 120px;" {}
                        }
                        div class="form-group" {
                            label for="amount_btc" { "Amount (required for variable amount offers)" }
                            input type="number" id="amount_btc" name="amount_btc" placeholder="Required for variable amount offers, ignored for fixed amount offers" step="1" {}
                        }
                        button type="submit" { "Pay BOLT12 Offer" }
                    }
                }
            ))
        }

        div class="card" {
            h3 { "Payment History" }
            a href="/payments" { button { "View All Payments" } }
        }
    };

    Ok(Html(layout("Send Payments", content).into_string()))
}

#[derive(Debug, Deserialize)]
pub struct PayBolt11Form {
    invoice: String,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    amount_btc: Option<u64>,
}

pub async fn post_pay_bolt11(
    State(state): State<AppState>,
    Form(form): Form<PayBolt11Form>,
) -> Result<Response, StatusCode> {
    println!("{form:?}");
    let invoice = match Bolt11Invoice::from_str(form.invoice.trim()) {
        Ok(inv) => inv,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid BOLT11 invoice: {e}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(layout("Payment Error", content).into_string()))
                .unwrap());
        }
    };

    let payment_id = if let Some(amount_btc) = form.amount_btc {
        // Convert Bitcoin to millisatoshis
        let amount_msats = amount_btc * 1000;
        state
            .node
            .inner
            .bolt11_payment()
            .send_using_amount(&invoice, amount_msats, None)
    } else {
        state.node.inner.bolt11_payment().send(&invoice, None)
    };

    let payment_id = match payment_id {
        Ok(id) => id,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Failed to initiate payment: {e}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/html")
                .body(Body::from(layout("Payment Error", content).into_string()))
                .unwrap());
        }
    };

    // Wait for payment to complete (max 10 seconds)
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);

    let payment_result = loop {
        if let Some(details) = state.node.inner.payment(&payment_id) {
            match details.status {
                PaymentStatus::Succeeded => {
                    break Ok(details);
                }
                PaymentStatus::Failed => {
                    break Err("Payment failed".to_string());
                }
                PaymentStatus::Pending => {
                    if start.elapsed() > timeout {
                        break Err("Payment is still pending after timeout".to_string());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            }
        } else {
            break Err("Payment not found".to_string());
        }
    };

    let content = match payment_result {
        Ok(details) => {
            let (preimage, fee_msats) = match details.kind {
                PaymentKind::Bolt11 {
                    hash: _,
                    preimage,
                    secret: _,
                } => (
                    preimage.map(|p| p.to_string()).unwrap_or_default(),
                    details.fee_paid_msat.unwrap_or(0),
                ),
                _ => (String::new(), 0),
            };

            html! {
                (success_message("Payment succeeded!"))
                (info_card(
                    "Payment Details",
                    vec![
                        ("Payment Hash", invoice.payment_hash().to_string()),
                        ("Payment Preimage", preimage),
                        ("Fee Paid", format_msats_as_btc(fee_msats)),
                        ("Amount", form.amount_btc.map(|_a| format_sats_as_btc(details.amount_msat.unwrap_or(1000) / 1000)).unwrap_or_default()),
                    ]
                ))
                div class="card" {
                    a href="/payments" { button { "← Make Another Payment" } }
                }
            }
        }
        Err(error) => {
            html! {
                (error_message(&format!("Payment failed: {error}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(layout("Payment Result", content).into_string()))
        .unwrap())
}

#[derive(Deserialize)]
pub struct PayBolt12Form {
    offer: String,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    amount_btc: Option<u64>,
}

pub async fn post_pay_bolt12(
    State(state): State<AppState>,
    Form(form): Form<PayBolt12Form>,
) -> Result<Response, StatusCode> {
    let offer = match Offer::from_str(form.offer.trim()) {
        Ok(offer) => offer,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid BOLT12 offer: {e:?}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(layout("Payment Error", content).into_string()))
                .unwrap());
        }
    };

    // Determine payment method based on offer type and user input
    let payment_id = match offer.amount() {
        Some(_) => {
            // Fixed amount offer - use send() method, ignore user input amount
            state.node.inner.bolt12_payment().send(&offer, None, None)
        }
        None => {
            // Variable amount offer - requires user to specify amount via send_using_amount()
            let amount_btc = match form.amount_btc {
                Some(amount) => amount,
                None => {
                    let content = html! {
                        (error_message("Amount is required for variable amount offers. This offer does not have a fixed amount, so you must specify how much you want to pay."))
                        div class="card" {
                            a href="/payments" { button { "← Try Again" } }
                        }
                    };
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header("content-type", "text/html")
                        .body(Body::from(layout("Payment Error", content).into_string()))
                        .unwrap());
                }
            };
            let amount_msats = amount_btc * 1_000;
            state
                .node
                .inner
                .bolt12_payment()
                .send_using_amount(&offer, amount_msats, None, None)
        }
    };

    let payment_id = match payment_id {
        Ok(id) => id,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Failed to initiate payment: {e}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/html")
                .body(Body::from(layout("Payment Error", content).into_string()))
                .unwrap());
        }
    };

    // Wait for payment to complete (max 10 seconds)
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(10);

    let payment_result = loop {
        if let Some(details) = state.node.inner.payment(&payment_id) {
            match details.status {
                PaymentStatus::Succeeded => {
                    break Ok(details);
                }
                PaymentStatus::Failed => {
                    break Err("Payment failed".to_string());
                }
                PaymentStatus::Pending => {
                    if start.elapsed() > timeout {
                        break Err("Payment is still pending after timeout".to_string());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            }
        } else {
            break Err("Payment not found".to_string());
        }
    };

    let content = match payment_result {
        Ok(details) => {
            let (payment_hash, preimage, fee_msats) = match details.kind {
                PaymentKind::Bolt12Offer {
                    hash,
                    preimage,
                    secret: _,
                    offer_id: _,
                    payer_note: _,
                    quantity: _,
                } => (
                    hash.map(|h| h.to_string()).unwrap_or_default(),
                    preimage.map(|p| p.to_string()).unwrap_or_default(),
                    details.fee_paid_msat.unwrap_or(0),
                ),
                _ => (String::new(), String::new(), 0),
            };

            html! {
                (success_message("Payment succeeded!"))
                (info_card(
                    "Payment Details",
                    vec![
                        ("Payment Hash", payment_hash),
                        ("Payment Preimage", preimage),
                        ("Fee Paid", format_msats_as_btc(fee_msats)),
                        ("Amount Paid", form.amount_btc.map(format_sats_as_btc).unwrap_or_else(|| {
                            // If no amount was specified in the form, show the actual amount from the payment details
                            details.amount_msat.map(format_msats_as_btc).unwrap_or_else(|| "Unknown".to_string())
                        })),
                    ]
                ))
                div class="card" {
                    a href="/payments" { button { "← Make Another Payment" } }
                }
            }
        }
        Err(error) => {
            html! {
                (error_message(&format!("Payment failed: {error}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(layout("Payment Result", content).into_string()))
        .unwrap())
}
