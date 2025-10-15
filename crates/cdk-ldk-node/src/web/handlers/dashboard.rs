use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use ldk_node::payment::{PaymentDirection, PaymentKind, PaymentStatus};
use maud::html;

use crate::web::handlers::AppState;
use crate::web::templates::{format_sats_as_btc, is_node_running, layout_with_status};

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
        if payment.status != PaymentStatus::Succeeded {
            continue;
        }

        let amount_sats = payment.amount_msat.unwrap_or(0) / 1000;
        let is_recent = payment.latest_update_timestamp >= twenty_four_hours_ago;

        match &payment.kind {
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

    let _node_id = node.node_id().to_string();
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

    // Calculate payment metrics for dashboard
    let all_payments = node.list_payments_with_filter(|_| true);
    let metrics = calculate_usage_metrics(&all_payments);

    let content = html! {
        h2 style="text-align: center; margin-bottom: 3rem;" { "Dashboard" }

        // Balance Summary as metric cards
        div class="card" {
            h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" { "Balance Summary" }
            div class="metrics-container" style="margin-top: 1.5rem;" {
                div class="metric-card" {
                    div class="metric-value" { (format_sats_as_btc(balances.total_lightning_balance_sats)) }
                    div class="metric-label" { "Lightning Balance" }
                }
                div class="metric-card" {
                    div class="metric-value" { (format_sats_as_btc(balances.total_onchain_balance_sats)) }
                    div class="metric-label" { "On-chain Balance" }
                }
                div class="metric-card" {
                    div class="metric-value" { (format_sats_as_btc(balances.spendable_onchain_balance_sats)) }
                    div class="metric-label" { "Spendable Balance" }
                }
                div class="metric-card" {
                    div class="metric-value" { (format_sats_as_btc(balances.total_lightning_balance_sats + balances.total_onchain_balance_sats)) }
                    div class="metric-label" { "Combined Total" }
                }
            }
        }

        // Node Information - new layout based on Figma design
        section class="node-info-section" {
            div class="node-info-main-container" {
                // Left side - Node avatar and info
                div class="node-info-left" {
                    div class="node-avatar" {
                        img src="/static/images/nut.png" alt="Node Avatar" class="avatar-image";
                    }
                    div class="node-details" {
                        h2 class="node-name" { (alias.clone()) }
                        p class="node-address" {
                            "Listening Address: "
                            (listening_addresses.first().unwrap_or(&"127.0.0.1:8090".to_string()))
                        }
                    }
                }

                // Middle - Gray container with spinning globe animation
                div class="node-content-box" {
                    div class="globe-container" {
                        svg aria-hidden="true" style="position: absolute; width: 0; height: 0; overflow: hidden;" version="1.1" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" {
                            defs {
                                symbol id="icon-world" viewBox="0 0 216 100" {
                                    title { "world" }
                                    g fill-rule="nonzero" {
                                        path d="M48 94l-3-4-2-14c0-3-1-5-3-8-4-5-6-9-4-11l1-4 1-3c2-1 9 0 11 1l3 2 2 3 1 2 8 2c1 1 2 2 0 7-1 5-2 7-4 7l-2 3-2 4-2 3-2 1c-2 2-2 9 0 10v1l-3-2zM188 90l3-2h1l-4 2zM176 87h2l-1 1-1-1zM195 86l3-2-2 2h-1zM175 83l-1-2-2-1-6 1c-5 1-5 1-5-2l1-4 2-2 4-3c5-4 9-5 9-3 0 3 3 3 4 1s1-2 1 0l3 4c2 4 1 6-2 10-4 3-7 4-8 1zM100 80c-2-4-4-11-3-14l-1-6c-1-1-2-3-1-4 0-2-4-3-9-3-4 0-5 0-7-3-1-2-2-4-1-7l3-6 3-3c1-2 10-4 11-2l6 3 5-1c3 1 4 0 5-1s-1-2-2-2l-4-1c0-1 3-3 6-2 3 0 3 0 2-2-2-2-6-2-7 0l-2 2-1 2-3-2-3-3c-1 0-1 1 1 2l1 2-2-1c-4-3-6-2-8 1-2 2-4 3-5 1-1-1 0-4 2-4l2-2 1-2 3-2 3-2 2 1c3 0 7-3 5-4l-1-3h-1l-1 3-2 2h-1l-2-1c-2-1-2-1 1-4 5-4 6-4 11-3 4 1 4 1 2 2v1l3-1 6-1c5 0 6-1 5-2l2 1c1 2 2 2 2 1-2-4 12-7 14-4l11 1 29 3 1 2-3 3c-2 0-2 0-1 1l1 3h-2c-1-1-2-3-1-4h-4l-6 2c-1 1-1 1 2 2 3 2 4 6 1 8v3c1 3 0 3-3 0s-4-1-2 3c3 4 3 7-2 8-5 2-4 1-2 5 2 3 0 5-3 4l-2-1-2-2-1-1-1-1-2-2c-1-2-1-2-4 0-2 1-3 4-3 5-1 3-1 3-3 1l-2-4c0-2-1-3-2-3l-1-1-4-2-6-1-4-2c-1 1 3 4 5 4h2c1 1 0 2-1 4-3 2-7 4-8 3l-7-10 5 10c2 2 3 3 5 2 3 0 2 1-2 7-4 4-4 5-4 8 1 3 1 4-1 6l-2 3c0 2-6 9-8 9l-3-2zm22-51l-2-3-1-1v-1c-2 0-2 2-1 4 2 3 4 4 4 1z" {}
                                        path d="M117 75c-1-2 0-6 2-7h2l-2 5c0 2-1 3-2 1zM186 64h-3c-2 0-6-3-5-5 1-1 6 1 7 3l2 3-2-1zM160 62h2c1 1 0 1-1 1l-1-1zM154 57l-1-2c2 2 3 1 2-2l-2-3 2 2 1 4 1 3v2l-3-4zM161 59c-1-1-1-2 1-4 3-3 4-3 4 0 0 4-2 6-5 4zM167 59l1-1 1 1-1 1-1-1zM176 59l1-1v2l-1-1zM141 52l1-1v2l-1-1zM170 52l1-1v2l-1-1zM32 50c-1-2-4-3-6-4-4-1-5-3-7-6l-3-5-2-2c-1-3-1-6 2-9 1-1 2-3 1-5 0-4-3-5-8-4H4l2-2 1-1 1-1 2-1c1-2 7-2 23-1 12 1 12 1 12-1h1c1 1 2 2 3 1l1 1-3 1c-2 0-8 4-8 5l2 1 2 3 4-3c3-4 4-4 5-3l3 1 1 2 1 2c3 0-1 2-4 2-2 0-2 0-2 2 1 1 0 2-2 2-4 1-12 9-12 12 0 2 0 2-1 1 0-2-2-3-6-2-3 0-4 1-4 3-2 4 0 6 3 4 3-1 3-1 2 1s-1 2 1 2l1 2 1 3 1 1-3-2zm8-24l1-1c0-1-4-3-5-2l1 1v2c-1 1-1 1 0 0h3zM167 47v-3l1 2c1 2 0 3-1 1z" {}
                                        path d="M41 43h2l-1 1-1-1zM37 42v-1l2 1h-2zM16 38l1-1v2l-1-1zM172 32l2-3h1c1 2 0 4-3 4v-1zM173 26h2l-1 1-1-1zM56 22h2l-2 1v-1zM87 19l1-2 1 3-1 1-1-2zM85 19l1-1v1l-1 1v-1zM64 12l1-3c2 0-1-4-3-4s-2 0 0-1V3l-6 2c-3 1-3 1-2-1 2-1 4-2 15-2h14c0 2-6 7-10 9l-5 2-2 1-2-2zM53 12l1-1c2 0-1-3-3-3-2-1-1-1 1-1l4 2c2 1 2 1 1 3-2 1-4 2-4 0zM80 12l1-1 1 1-1 1-1-1zM36 8h-2V7c1-1 7 0 7 1h-5zM116 7l1-1v1l-1 1V7zM50 5h2l-1 1-1-1zM97 5l2-1c0-1 1-1 0 0l-2 1z" {}
                                    }
                                }
                                symbol id="icon-repeated-world" viewBox="0 0 432 100" {
                                    use href="#icon-world" x="0" {}
                                    use href="#icon-world" x="189" {}
                                }
                            }
                        }
                        span class="world" {
                            span class="images" {
                                svg { use href="#icon-repeated-world" {} }
                            }
                        }
                    }
                }
            }

            // Right side - Connections metrics
            aside class="node-metrics" {
                div class="card" {
                    h3 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" { "Connections" }
                    div class="metrics-container" style="margin-top: 1.5rem;" {
                        div class="metric-card" {
                            div class="metric-value" { (format!("{}/{}", num_connected_peers, num_peers)) }
                            div class="metric-label" { "Connected Peers" }
                        }
                        div class="metric-card" {
                            div class="metric-value" { (format!("{}/{}", num_active_channels, num_active_channels + num_inactive_channels)) }
                            div class="metric-label" { "Active Channels" }
                        }
                    }
                }
            }
        }

        // Activity Sections - Side by Side Layout
        div class="card" {
            h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; padding-bottom: 1rem; margin-bottom: 0;" { "Activity Overview" }

            div class="activity-grid" {
                // Lightning Network Activity
                div class="activity-section" {
                    div class="activity-header" {
                        div class="activity-icon-box" {
                            svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" {
                                path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z" {}
                            }
                        }
                        h3 class="activity-title" { "Lightning Network Activity" }
                    }

                    div class="activity-metrics" {
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "24h Inflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.lightning_inflow_24h)) }
                        }
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "24h Outflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.lightning_outflow_24h)) }
                        }
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "All-time Inflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.lightning_inflow_all_time)) }
                        }
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "All-time Outflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.lightning_outflow_all_time)) }
                        }
                    }
                }

                // On-chain Activity
                div class="activity-section" {
                    div class="activity-header" {
                        div class="activity-icon-box" {
                            svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" {
                                path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" {}
                                path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" {}
                            }
                        }
                        h3 class="activity-title" { "On-chain Activity" }
                    }

                    div class="activity-metrics" {
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "24h Inflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.onchain_inflow_24h)) }
                        }
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "24h Outflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.onchain_outflow_24h)) }
                        }
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "All-time Inflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.onchain_inflow_all_time)) }
                        }
                        div class="activity-metric-card" {
                            div class="activity-metric-label" { "All-time Outflow" }
                            div class="activity-metric-value" { (format_sats_as_btc(metrics.onchain_outflow_all_time)) }
                        }
                    }
                }
            }
        }
    };

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Dashboard", content, is_running).into_string(),
    ))
}
