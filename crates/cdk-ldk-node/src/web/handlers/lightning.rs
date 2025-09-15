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

            // Quick Actions section - individual cards
            div class="card" style="margin-bottom: 2rem;" {
                h2 { "Quick Actions" }
                div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 1.5rem; margin-top: 1.5rem;" {
                    // Open Channel Card
                    div class="quick-action-card" {
                        h3 style="font-size: 1.125rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);" { "Open Channel" }
                        p style="font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; line-height: 1.4;" { "Create a new Lightning Network channel to connect with another node." }
                        a href="/channels/open" style="text-decoration: none;" {
                            button class="button-outline" { "Open Channel" }
                        }
                    }

                    // Create Invoice Card
                    div class="quick-action-card" {
                        h3 style="font-size: 1.125rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);" { "Create Invoice" }
                        p style="font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; line-height: 1.4;" { "Generate a Lightning invoice to receive payments from other users or services." }
                        a href="/invoices" style="text-decoration: none;" {
                            button class="button-outline" { "Create Invoice" }
                        }
                    }

                    // Make Payment Card
                    div class="quick-action-card" {
                        h3 style="font-size: 1.125rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);" { "Make Lightning Payment" }
                        p style="font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; line-height: 1.4;" { "Send Lightning payments to other users using invoices. BOLT 11 & 12 supported." }
                        a href="/invoices" style="text-decoration: none;" {
                            button class="button-outline" { "Make Payment" }
                        }
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

            // Quick Actions section - individual cards
            div class="card" style="margin-bottom: 2rem;" {
                h2 { "Quick Actions" }
                div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(280px, 1fr)); gap: 1.5rem; margin-top: 1.5rem;" {
                    // Open Channel Card
                    div class="quick-action-card" {
                        h3 style="font-size: 1.125rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);" { "Open Channel" }
                        p style="font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; line-height: 1.4;" { "Create a new Lightning channel by connecting with another node." }
                        a href="/channels/open" style="text-decoration: none;" {
                            button class="button-outline" { "Open Channel" }
                        }
                    }

                    // Create Invoice Card
                    div class="quick-action-card" {
                        h3 style="font-size: 1.125rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);" { "Create Invoice" }
                        p style="font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; line-height: 1.4;" { "Generate a Lightning invoice to receive payments." }
                        a href="/invoices" style="text-decoration: none;" {
                            button class="button-outline" { "Create Invoice" }
                        }
                    }

                    // Make Payment Card
                    div class="quick-action-card" {
                        h3 style="font-size: 1.125rem; font-weight: 600; margin-bottom: 0.5rem; color: var(--text-primary);" { "Make Lightning Payment" }
                        p style="font-size: 0.875rem; color: var(--text-muted); margin-bottom: 1rem; line-height: 1.4;" { "Send Lightning payments to other users using invoices." }
                        a href="/payments/send" style="text-decoration: none;" {
                            button class="button-outline" { "Make Payment" }
                        }
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

            // Channel Details header (outside card)
            h2 class="section-header" { "Channel Details" }

            // Channels list
            @for (index, channel) in channels.iter().enumerate() {
                @let node_id = channel.counterparty_node_id.to_string();
                @let channel_number = index + 1;

                div class="channel-box" {
                    // Channel number as prominent header
                    div class="channel-alias" { (format!("Channel {}", channel_number)) }

                    // Channel details in left-aligned format
                    div class="channel-details" {
                        div class="detail-row" {
                            span class="detail-label" { "Channel ID" }
                            span class="detail-value-amount" { (channel.channel_id.to_string()) }
                        }
                        @if let Some(short_channel_id) = channel.short_channel_id {
                            div class="detail-row" {
                                span class="detail-label" { "Short Channel ID" }
                                span class="detail-value-amount" { (short_channel_id.to_string()) }
                            }
                        }
                        div class="detail-row" {
                            span class="detail-label" { "Node ID" }
                            span class="detail-value-amount" { (node_id) }
                        }
                        div class="detail-row" {
                            span class="detail-label" { "Status" }
                            @if channel.is_usable {
                                span class="status-badge status-active" { "Active" }
                            } @else {
                                span class="status-badge status-inactive" { "Inactive" }
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
