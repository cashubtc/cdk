use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use maud::html;

use crate::web::handlers::utils::AppState;
use crate::web::templates::{format_sats_as_btc, is_node_running, layout_with_status};

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

            // Inactive channels warning (only show if > 0)
            @if num_inactive_channels > 0 {
                div class="card" style="background-color: #fef3c7; border: 1px solid #f59e0b; margin-bottom: 2rem;" {
                    h3 style="color: #92400e; margin-bottom: 0.5rem;" { "⚠️ Inactive Channels Detected" }
                    p style="color: #78350f; margin: 0;" {
                        "You have " (num_inactive_channels) " inactive channel(s). This may indicate a connectivity issue that requires attention."
                    }
                }
            }

            // Balance Information with action buttons in header
            div class="card" {
                div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" {
                    h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; margin: 0;" { "Balance Information" }
                    div style="display: flex; gap: 0.5rem;" {
                        a href="/payments/send" style="text-decoration: none;" {
                            button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Send" }
                        }
                        a href="/invoices" style="text-decoration: none;" {
                            button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Receive" }
                        }
                        a href="/channels/open" style="text-decoration: none;" {
                            button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Open Channel" }
                        }
                    }
                }
                div class="metrics-container" style="margin-top: 1.5rem;" {
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
                    @if num_inactive_channels > 0 {
                        div class="metric-card" {
                            div class="metric-value" style="color: #f59e0b;" { (format!("{}", num_inactive_channels)) }
                            div class="metric-label" { "Inactive Channels" }
                        }
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

            // Inactive channels warning (only show if > 0)
            @if num_inactive_channels > 0 {
                div class="card" style="background-color: #fef3c7; border: 1px solid #f59e0b; margin-bottom: 2rem;" {
                    h3 style="color: #92400e; margin-bottom: 0.5rem;" { "⚠️ Inactive Channels Detected" }
                    p style="color: #78350f; margin: 0;" {
                        "You have " (num_inactive_channels) " inactive channel(s). This may indicate a connectivity issue that requires attention."
                    }
                }
            }

            // Balance Information with action buttons in header
            div class="card" {
                div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" {
                    h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; margin: 0;" { "Balance Information" }
                    div style="display: flex; gap: 0.5rem;" {
                        a href="/payments/send" style="text-decoration: none;" {
                            button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Send" }
                        }
                        a href="/invoices" style="text-decoration: none;" {
                            button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Receive" }
                        }
                        a href="/channels/open" style="text-decoration: none;" {
                            button class="button-outline" style="padding: 0.5rem 1rem; font-size: 0.875rem;" { "Open Channel" }
                        }
                    }
                }
                div class="metrics-container" style="margin-top: 1.5rem;" {
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
                    @if num_inactive_channels > 0 {
                        div class="metric-card" {
                            div class="metric-value" style="color: #f59e0b;" { (format!("{}", num_inactive_channels)) }
                            div class="metric-label" { "Inactive Channels" }
                        }
                    }
                }
            }

            // Channel Details header (outside card)
            h2 class="section-header" style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5;" { "Channel Details" }

                    // Channels list
                    @for (index, channel) in channels.iter().enumerate() {
                @let node_id = channel.counterparty_node_id.to_string();
                @let channel_number = index + 1;

                div class="channel-box" {
                    // Channel header with number on left and status badge on right
                    div style="display: flex; justify-content: space-between; align-items: center; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 1.5rem;" {
                        div class="channel-alias" style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; margin: 0;" { (format!("Channel {}", channel_number)) }
                        @if channel.is_usable {
                            span class="status-badge status-active" { "Active" }
                        } @else {
                            span class="status-badge status-inactive" { "Inactive" }
                        }
                    }

                    // Channel details - ordered by label length
                    div class="channel-details" {
                        div class="detail-row" {
                            span class="detail-label" { "Node ID" }
                            span class="detail-value" { (node_id) }
                        }
                        div class="detail-row" {
                            span class="detail-label" { "Channel ID" }
                            span class="detail-value" { (channel.channel_id.to_string()) }
                        }
                        @if let Some(short_channel_id) = channel.short_channel_id {
                            div class="detail-row" {
                                span class="detail-label" { "Short Channel ID" }
                                span class="detail-value" { (short_channel_id.to_string()) }
                            }
                        }
                    }

                    // Balance information cards (keeping existing style)
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

                    // Action buttons
                    @if channel.is_usable {
                        div class="channel-actions" {
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
    };

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Lightning", content, is_running).into_string(),
    ))
}
