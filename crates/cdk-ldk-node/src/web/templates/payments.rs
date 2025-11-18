use maud::{html, Markup};

use crate::web::templates::formatters::format_timestamp;

#[allow(clippy::too_many_arguments)]
pub fn payment_list_item(
    _payment_id: &str,
    direction: &str,
    status: &str,
    amount: &str,
    payment_hash: Option<&str>,
    description: Option<&str>,
    timestamp: Option<u64>,
    payment_type: &str,
    preimage: Option<&str>,
) -> Markup {
    let status_class = match status {
        "Succeeded" => "status-active",
        "Failed" => "status-inactive",
        "Pending" => "status-pending",
        "Unpaid" => "status-pending", // Use pending styling for unpaid
        _ => "status-badge",
    };

    let direction_icon = match direction {
        "Inbound" => "↓",
        "Outbound" => "↑",
        _ => "•",
    };

    let type_class = match payment_type {
        "BOLT11" => "payment-type-bolt11",
        "BOLT12" => "payment-type-bolt12",
        "On-chain" => "payment-type-onchain",
        "Spontaneous" => "payment-type-spontaneous",
        "BOLT11 JIT" => "payment-type-bolt11-jit",
        _ => "payment-type-unknown",
    };

    html! {
        div class="payment-item" {
            div class="payment-header" {
                div class="payment-direction" {
                    span class="direction-icon" { (direction_icon) }
                    span { (direction) " Payment" }
                    span class=(format!("payment-type-badge {}", type_class)) { (payment_type) }
                }
                span class=(format!("status-badge {}", status_class)) { (status) }
            }

            div class="payment-details" {
                div class="payment-amount" { (amount) }

                @if let Some(hash) = payment_hash {
                    div class="payment-info" {
                        span class="payment-label" {
                            @if payment_type == "BOLT11" || payment_type == "BOLT12" || payment_type == "Spontaneous" || payment_type == "BOLT11 JIT" { "Payment Hash:" }
                            @else { "Transaction ID:" }
                        }
                        span class="payment-value" title=(hash) {
                            (&hash[..std::cmp::min(16, hash.len())]) "..."
                        }
                        button class="copy-button" data-copy=(hash)
                               onclick="navigator.clipboard.writeText(this.getAttribute('data-copy')).then(() => { this.textContent = 'Copied!'; setTimeout(() => this.textContent = 'Copy', 2000); })" {
                            "Copy"
                        }
                    }
                }

                // Show preimage for successful outgoing BOLT11 or BOLT12 payments
                @if let Some(preimage_str) = preimage {
                    @if !preimage_str.is_empty() && direction == "Outbound" && status == "Succeeded" && (payment_type == "BOLT11" || payment_type == "BOLT12") {
                        div class="payment-info" {
                            span class="payment-label" { "Preimage:" }
                            span class="payment-value" title=(preimage_str) {
                                (&preimage_str[..std::cmp::min(16, preimage_str.len())]) "..."
                            }
                            button class="copy-button" data-copy=(preimage_str)
                                   onclick="navigator.clipboard.writeText(this.getAttribute('data-copy')).then(() => { this.textContent = 'Copied!'; setTimeout(() => this.textContent = 'Copy', 2000); })" {
                                "Copy"
                            }
                        }
                    }
                }

                @if let Some(desc) = description {
                    @if !desc.is_empty() {
                        div class="payment-info" {
                            span class="payment-label" { "Description:" }
                            span class="payment-value" { (desc) }
                        }
                    }
                }

                @if let Some(ts) = timestamp {
                    div class="payment-info" {
                        span class="payment-label" { "Last Update:" }
                        span class="payment-value" {
                            (format_timestamp(ts))
                        }
                    }
                }
            }
        }
    }
}
