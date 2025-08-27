use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use maud::html;

use crate::web::handlers::AppState;
use crate::web::templates::{format_sats_as_btc, layout};

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
                            div style="margin-top: 1rem; display: flex; gap: 0.5rem;" {
                                a href=(format!("/channels/close?channel_id={}&node_id={}", channel.user_channel_id.0, channel.counterparty_node_id)) {
                                    button style="background: #dc3545;" { "Close Channel" }
                                }
                                a href=(format!("/channels/force-close?channel_id={}&node_id={}", channel.user_channel_id.0, channel.counterparty_node_id)) {
                                    button style="background: #d63384;" title="Force close should not be used if normal close is preferred. Force close will broadcast the latest commitment transaction immediately." { "Force Close" }
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
