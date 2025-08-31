use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use maud::html;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::web::handlers::AppState;
use crate::web::templates::{format_sats_as_btc, layout};

// Cache for node aliases to avoid repeated lookups
lazy_static::lazy_static! {
    static ref NODE_ALIAS_CACHE: RwLock<HashMap<String, String>> = RwLock::new(HashMap::new());
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
    }
    
    alias
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
            h2 style="text-align: center; margin-bottom: 3rem;" { "Lightning" }

            // Quick Actions section - matching dashboard style
            div class="card" style="margin-bottom: 2rem;" {
                h2 { "Quick Actions" }
                div style="display: flex; gap: 1rem; margin-top: 1rem; flex-wrap: wrap;" {
                    a href="/channels/open" style="text-decoration: none; flex: 1; min-width: 200px;" {
                        button class="button-primary" style="width: 100%;" { "Open Channel" }
                    }
                    a href="/invoices" style="text-decoration: none; flex: 1; min-width: 200px;" {
                        button class="button-primary" style="width: 100%;" { "Create Invoice" }
                    }
                    a href="/payments/send" style="text-decoration: none; flex: 1; min-width: 200px;" {
                        button class="button-primary" style="width: 100%;" { "Make Lightning Payment" }
                    }
                }
            }

            // Balance Information as metric cards
            div class="card" {
                h2 { "Balance Information" }
                div class="metrics-container" {
                    div class="metric-card" {
                        div class="metric-value" { (format_sats_as_btc(balances.total_lightning_balance_sats)) }
                        div class="metric-label" { "Lightning Balance" }
                    }
                    div class="metric-card" {
                        div class="metric-value" { (format!("{}", num_active_channels + num_inactive_channels)) }
                        div class="metric-label" { "Total Channels" }
                    }
                    div class="metric-card" {
                        div class="metric-value" { (format!("{}", num_active_channels)) }
                        div class="metric-label" { "Active Channels" }
                    }
                    div class="metric-card" {
                        div class="metric-value" { (format!("{}", num_inactive_channels)) }
                        div class="metric-label" { "Inactive Channels" }
                    }
                }
            }

            div class="card" {
                p { "No channels found. Create your first channel to start using Lightning Network." }
            }
        }
    } else {
        html! {
            h2 style="text-align: center; margin-bottom: 3rem;" { "Lightning" }

            // Quick Actions section - matching dashboard style
            div class="card" style="margin-bottom: 2rem;" {
                h2 { "Quick Actions" }
                div style="display: flex; gap: 1rem; margin-top: 1rem; flex-wrap: wrap;" {
                    a href="/channels/open" style="text-decoration: none; flex: 1; min-width: 200px;" {
                        button class="button-primary" style="width: 100%;" { "Open Channel" }
                    }
                    a href="/invoices" style="text-decoration: none; flex: 1; min-width: 200px;" {
                        button class="button-primary" style="width: 100%;" { "Create Invoice" }
                    }
                    a href="/payments/send" style="text-decoration: none; flex: 1; min-width: 200px;" {
                        button class="button-primary" style="width: 100%;" { "Make Lightning Payment" }
                    }
                }
            }

            // Balance Information as metric cards
            div class="card" {
                h2 { "Balance Information" }
                div class="metrics-container" {
                    div class="metric-card" {
                        div class="metric-value" { (format_sats_as_btc(balances.total_lightning_balance_sats)) }
                        div class="metric-label" { "Lightning Balance" }
                    }
                    div class="metric-card" {
                        div class="metric-value" { (format!("{}", num_active_channels + num_inactive_channels)) }
                        div class="metric-label" { "Total Channels" }
                    }
                    div class="metric-card" {
                        div class="metric-value" { (format!("{}", num_active_channels)) }
                        div class="metric-label" { "Active Channels" }
                    }
                    div class="metric-card" {
                        div class="metric-value" { (format!("{}", num_inactive_channels)) }
                        div class="metric-label" { "Inactive Channels" }
                    }
                }
            }

            div class="card" {
                h2 { "Channel Details" }

                // Channels list
                @for channel in &channels {
                    @let node_id = channel.counterparty_node_id.to_string();
                    @let node_alias = get_node_alias(&node_id).await;
                    
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
                            div class="info-value" {
                                @if let Some(alias) = node_alias {
                                    div style="font-weight: bold; margin-bottom: 0.25rem;" { (alias) }
                                }
                                span style="font-family: monospace; font-size: 0.85rem;" { (node_id) }
                            }
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
                            div style="margin-top: 1rem; display: flex; justify-content: space-between; align-items: center;" {
                                a href=(format!("/channels/close?channel_id={}&node_id={}", channel.user_channel_id.0, channel.counterparty_node_id)) {
                                    button class="button-secondary" { "Close Channel" }
                                }
                                a href=(format!("/channels/force-close?channel_id={}&node_id={}", channel.user_channel_id.0, channel.counterparty_node_id)) {
                                    button class="button-destructive" title="Force close should not be used if normal close is preferred. Force close will broadcast the latest commitment transaction immediately." { "Force Close" }
                                }
                            }
                        }
                    }
                }

            }
        }
    };

    Ok(Html(layout("Lightning", content).into_string()))
}
