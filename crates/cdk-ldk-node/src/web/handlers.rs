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
use ldk_node::payment::{PaymentKind, PaymentStatus};
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
    error_message, form_card, format_msats_as_btc, format_sats_as_btc, info_card, layout,
    success_message,
};
use crate::CdkLdkNode;

#[derive(Clone)]
pub struct AppState {
    pub node: Arc<CdkLdkNode>,
}

pub async fn dashboard(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let node = &state.node.inner;

    let node_id = node.node_id().to_string();
    let alias = node
        .node_alias()
        .map(|a| a.to_string())
        .unwrap_or_else(|| "No alias set".to_string());

    let config = state.node.inner.config();
    let listening_addresses: Vec<String> = config
        .announcement_addresses
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
            (info_card(
                "Node Information",
                vec![
                    ("Node ID", node_id),
                    ("Alias", alias),
                    ("Listening Addresses", listening_addresses.join(", ")),
                    ("Connected Peers", format!("{} / {}", num_connected_peers, num_peers)),
                    ("Active Channels", format!("{} / {}", num_active_channels, num_active_channels + num_inactive_channels)),
                ]
            ))

            (info_card(
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
                    a href="/channels" style="text-decoration: none;" {
                        button style="width: 100%;" { "Manage Channels" }
                    }
                    a href="/invoices" style="text-decoration: none;" {
                        button style="width: 100%;" { "Create Invoice" }
                    }
                    a href="/payments" style="text-decoration: none;" {
                        button style="width: 100%;" { "Make Payment" }
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
                (success_message(&format!("New address generated: {}", address)))
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
                (error_message(&format!("Failed to generate address: {}", e)))
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

    let content = html! {
        (info_card(
            "Lightning Balance Details",
            vec![
                ("Total Lightning Balance", format_sats_as_btc(balances.total_lightning_balance_sats)),
            ]
        ))

        div class="card" {
            h2 { "Quick Actions" }
            div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-top: 1rem;" {
                a href="/invoices" style="text-decoration: none;" {
                    button style="width: 100%;" { "Create Lightning Invoice" }
                }
                a href="/payments" style="text-decoration: none;" {
                    button style="width: 100%;" { "Make Lightning Payment" }
                }
                a href="/channels" style="text-decoration: none;" {
                    button style="width: 100%;" { "Manage Channels" }
                }
                a href="/onchain" style="text-decoration: none;" {
                    button style="width: 100%;" { "View On-chain Balance" }
                }
            }
        }
    };

    Ok(Html(layout("Lightning Balance", content).into_string()))
}

#[derive(Deserialize)]
pub struct SendOnchainForm {
    address: String,
    amount_sat: u64, // Changed to u64 to handle satoshis directly
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
        (info_card(
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
                                input type="number" id="amount_sat" name="amount_sat" required placeholder="0" {}
                            }
                            button type="submit" { "Send Payment" }
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
                    (form_card(
                        "Receive Bitcoin",
                        html! {
                            p { "Generate a new Bitcoin address to receive on-chain payments." }
                            a href="/onchain?action=receive" {
                                button style="width: 100%; margin-top: 1rem;" { "Generate New Address" }
                            }
                        }
                    ))

                    (form_card(
                        "Send Bitcoin",
                        html! {
                            p { "Send Bitcoin to any address on the network." }
                            a href="/onchain?action=send" {
                                button style="width: 100%; margin-top: 1rem;" { "Send Payment" }
                            }
                        }
                    ))
                }
            };
        }
    }

    Ok(Html(layout("On-chain", content).into_string()))
}

pub async fn post_send_onchain(
    State(state): State<AppState>,
    Form(form): Form<SendOnchainForm>,
) -> Result<Response, StatusCode> {
    let address_result = Address::from_str(&form.address);
    let address = match address_result {
        Ok(addr) => addr,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid address: {}", e)))
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

    let amount_satoshis = form.amount_sat;

    let txid_result = state.node.inner.onchain_payment().send_to_address(
        address.assume_checked_ref(),
        amount_satoshis,
        None,
    );

    let content = match txid_result {
        Ok(txid) => {
            html! {
                (success_message("Transaction sent successfully!"))
                (info_card(
                    "Transaction Details",
                    vec![
                        ("Transaction ID", txid.to_string()),
                        ("Amount", format_sats_as_btc(amount_satoshis)),
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
                (error_message(&format!("Failed to send payment: {}", e)))
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

pub async fn channels_page(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let channels = state.node.inner.list_channels();

    let mut content = html! {
        div class="card" {
            h2 { "Lightning Channels" }
            div style="margin-bottom: 1rem;" {
                a href="/channels/open" { button { "Open New Channel" } }
            }
        }
    };

    if channels.is_empty() {
        content = html! {
            (content)
            div class="card" {
                p { "No channels found. Create your first channel to start using Lightning Network." }
            }
        };
    } else {
        for channel in channels {
            let status_badge = if channel.is_usable {
                html! { span class="status-badge status-active" { "Active" } }
            } else {
                html! { span class="status-badge status-inactive" { "Inactive" } }
            };

            content = html! {
                (content)
                div class="channel-item" {
                    div class="channel-header" {
                        span class="channel-id" { "Channel ID: " (channel.channel_id.to_string()) }
                        (status_badge)
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
                            div class="balance-amount" { (format_sats_as_btc((channel.outbound_capacity_msat + channel.inbound_capacity_msat) / 1000)) }
                            div class="balance-label" { "Total" }
                        }
                    }
                    @if channel.is_usable {
                        div style="margin-top: 1rem;" {
                            a href=(format!("/channels/close?channel_id={}&node_id={}", channel.channel_id, channel.counterparty_node_id)) {
                                button style="background: #dc3545;" { "Close Channel" }
                            }
                        }
                    }
                }
            };
        }
    }

    Ok(Html(layout("Channels", content).into_string()))
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
                    input type="number" id="amount_btc" name="amount_btc" required placeholder="₿0" step="0.00000001" {}
                }
                div class="form-group" {
                    label for="push_btc" { "Push Amount (optional)" }
                    input type="number" id="push_btc" name="push_btc" placeholder="₿0" step="0.00000001" {}
                }
                button type="submit" { "Open Channel" }
                " "
                a href="/channels" { button type="button" { "Cancel" } }
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
    amount_btc: f64,
    push_btc: Option<f64>,
}

pub async fn post_open_channel(
    State(state): State<AppState>,
    Form(form): Form<OpenChannelForm>,
) -> Result<Response, StatusCode> {
    let pubkey = match PublicKey::from_str(&form.node_id) {
        Ok(pk) => pk,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid node public key: {}", e)))
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
                (error_message(&format!("Invalid address:port combination: {}", e)))
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
            (error_message(&format!("Failed to connect to peer: {}", e)))
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
    let amount_msats = (form.amount_btc * 100_000_000_000.0) as u64;
    let push_msats = form.push_btc.map(|p| (p * 100_000_000_000.0) as u64);

    // Then open the channel
    let channel_result = state.node.inner.open_announced_channel(
        pubkey,
        socket_addr,
        amount_msats,
        push_msats,
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
                        ("Amount", format_sats_as_btc((form.amount_btc * 100_000_000.0) as u64)),
                        ("Push Amount", form.push_btc.map(|p| format_sats_as_btc((p * 100_000_000.0) as u64)).unwrap_or_else(|| "0 ₿".to_string())),
                    ]
                ))
                div class="card" {
                    p { "The channel is now being opened. It may take some time for the channel to become active." }
                    a href="/channels" { button { "← Back to Channels" } }
                }
            }
        }
        Err(e) => {
            html! {
                (error_message(&format!("Failed to open channel: {}", e)))
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
                a href="/channels" { button { "← Back to Channels" } }
            }
        };
        return Ok(Html(layout("Close Channel Error", content).into_string()));
    }

    let content = form_card(
        "Close Channel",
        html! {
            p { "Are you sure you want to close this channel?" }
            div class="info-item" {
                span class="info-label" { "Channel ID:" }
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
                a href="/channels" { button type="button" { "Cancel" } }
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
                (error_message(&format!("Invalid node public key: {}", e)))
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
                (error_message(&format!("Invalid channel ID: {}", e)))
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
                    a href="/channels" { button { "← Back to Channels" } }
                }
            }
        }
        Err(e) => {
            html! {
                (error_message(&format!("Failed to close channel: {}", e)))
                div class="card" {
                    a href="/channels" { button { "← Back to Channels" } }
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
    amount_btc: f64,
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
                    (error_message(&format!("Invalid description: {}", e)))
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

    // Convert Bitcoin to millisatoshis (1 BTC = 100,000,000,000 msats)
    let amount_msats = (form.amount_btc * 100_000_000_000.0) as u64;

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
                        ("Amount", format_sats_as_btc((form.amount_btc * 100_000_000.0) as u64)),
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
                (error_message(&format!("Failed to create invoice: {}", e)))
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
                (error_message(&format!("Failed to create offer: {}", e)))
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

pub async fn payments_page(State(_state): State<AppState>) -> Result<Html<String>, StatusCode> {
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
                            input type="number" id="amount_btc" name="amount_btc" placeholder="Leave empty to use invoice amount" step="0.00000001" {}
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
                            label for="amount_btc" { "Amount" }
                            input type="number" id="amount_btc" name="amount_btc" required placeholder="₿0" step="0.00000001" {}
                        }
                        button type="submit" { "Pay BOLT12 Offer" }
                    }
                }
            ))
        }
    };

    Ok(Html(layout("Make Payments", content).into_string()))
}

#[derive(Deserialize)]
pub struct PayBolt11Form {
    invoice: String,
    amount_btc: Option<f64>,
}

pub async fn post_pay_bolt11(
    State(state): State<AppState>,
    Form(form): Form<PayBolt11Form>,
) -> Result<Response, StatusCode> {
    let invoice = match Bolt11Invoice::from_str(&form.invoice.trim()) {
        Ok(inv) => inv,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid BOLT11 invoice: {}", e)))
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
        // Convert Bitcoin to millisatoshis (1 BTC = 100,000,000,000 msats)
        let amount_msats = (amount_btc * 100_000_000_000.0) as u64;
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
                (error_message(&format!("Failed to initiate payment: {}", e)))
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
                        ("Amount", form.amount_btc.map(|a| format_sats_as_btc((a * 100_000_000.0) as u64)).unwrap_or_else(|| "As per invoice".to_string())),
                    ]
                ))
                div class="card" {
                    a href="/payments" { button { "← Make Another Payment" } }
                }
            }
        }
        Err(error) => {
            html! {
                (error_message(&format!("Payment failed: {}", error)))
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
    amount_btc: f64,
}

pub async fn post_pay_bolt12(
    State(state): State<AppState>,
    Form(form): Form<PayBolt12Form>,
) -> Result<Response, StatusCode> {
    let offer = match Offer::from_str(&form.offer.trim()) {
        Ok(offer) => offer,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid BOLT12 offer: {:?}", e)))
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

    // Convert Bitcoin to millisatoshis (1 BTC = 100,000,000,000 msats)
    let amount_msats = (form.amount_btc * 100_000_000_000.0) as u64;

    let payment_id =
        state
            .node
            .inner
            .bolt12_payment()
            .send_using_amount(&offer, amount_msats, None, None);

    let payment_id = match payment_id {
        Ok(id) => id,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Failed to initiate payment: {}", e)))
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
                        ("Amount Paid", format_sats_as_btc((form.amount_btc * 100_000_000.0) as u64)),
                    ]
                ))
                div class="card" {
                    a href="/payments" { button { "← Make Another Payment" } }
                }
            }
        }
        Err(error) => {
            html! {
                (error_message(&format!("Payment failed: {}", error)))
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
