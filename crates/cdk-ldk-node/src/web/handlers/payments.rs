use std::str::FromStr;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::Form;
use cdk_common::util::hex;
use ldk_node::lightning::offers::offer::Offer;
use ldk_node::lightning_invoice::Bolt11Invoice;
use ldk_node::payment::{PaymentDirection, PaymentKind, PaymentStatus};
use maud::html;
use serde::Deserialize;

use crate::web::handlers::utils::{deserialize_optional_u64, get_paginated_payments_streaming};
use crate::web::handlers::AppState;
use crate::web::templates::{
    error_message, format_msats_as_btc, format_sats_as_btc, info_card, is_node_running,
    layout_with_status, payment_list_item, success_message,
};

#[derive(Deserialize)]
pub struct PaymentsQuery {
    filter: Option<String>,
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct PayBolt11Form {
    invoice: String,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    amount_btc: Option<u64>,
}

#[derive(Deserialize)]
pub struct PayBolt12Form {
    offer: String,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    amount_btc: Option<u64>,
}

pub async fn payments_page(
    State(state): State<AppState>,
    query: Query<PaymentsQuery>,
) -> Result<Html<String>, StatusCode> {
    let filter = query.filter.as_deref().unwrap_or("all");
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(25).clamp(10, 100); // Limit between 10-100 items per page

    // Use efficient pagination function
    let (current_page_payments, total_count) = get_paginated_payments_streaming(
        &state.node.inner,
        filter,
        ((page - 1) * per_page) as usize,
        per_page as usize,
    );

    // Calculate pagination
    let total_pages = ((total_count as f64) / (per_page as f64)).ceil() as u32;
    let start_index = ((page - 1) * per_page) as usize;
    let end_index = (start_index + per_page as usize).min(total_count);

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
        h2 style="text-align: center; margin-bottom: 3rem;" { "Payments" }
        div class="card" {
            div class="payment-list-header" {
                div {
                    h2 style="font-size: 0.875rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.05em; opacity: 0.5; padding-bottom: 1rem; border-bottom: 1px solid hsl(var(--border)); margin-bottom: 0;" { "Payment History" }
                    @if total_count > 0 {
                        p style="margin: 0.25rem 0 0 0; color: #666; font-size: 0.9rem;" {
                            "Showing " (start_index + 1) " to " (end_index) " of " (total_count) " payments"
                        }
                    }
                }
                div class="payment-filter-tabs" {
                    a href=(build_url(1, "all", per_page)) class=(if filter == "all" { "payment-filter-tab active" } else { "payment-filter-tab" }) { "All" }
                    a href=(build_url(1, "incoming", per_page)) class=(if filter == "incoming" { "payment-filter-tab active" } else { "payment-filter-tab" }) { "Incoming" }
                    a href=(build_url(1, "outgoing", per_page)) class=(if filter == "outgoing" { "payment-filter-tab active" } else { "payment-filter-tab" }) { "Outgoing" }
                }
            }

            // Payment list (no metrics here)
            @if current_page_payments.is_empty() {
                @if total_count == 0 {
                    p { "No payments found." }
                } @else {
                    p { "No payments found on this page. "
                        a href=(build_url(1, filter, per_page)) { "Go to first page" }
                    }
                }
            } @else {
                @for payment in &current_page_payments {
                    @let direction_str = match payment.direction {
                        PaymentDirection::Inbound => "Inbound",
                        PaymentDirection::Outbound => "Outbound",
                    };

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

                    @let status_str = {
                        // Helper function to determine invoice status
                        fn get_invoice_status(status: PaymentStatus, direction: PaymentDirection, payment_type: &str) -> &'static str {
                            match status {
                                PaymentStatus::Succeeded => "Succeeded",
                                PaymentStatus::Failed => "Failed",
                                PaymentStatus::Pending => {
                                    // For inbound BOLT11 payments, show "Unpaid" instead of "Pending"
                                    if direction == PaymentDirection::Inbound && payment_type == "BOLT11" {
                                        "Unpaid"
                                    } else {
                                        "Pending"
                                    }
                                }
                            }
                        }
                        get_invoice_status(payment.status, payment.direction, payment_type)
                    };

                    @let amount_str = payment.amount_msat.map(format_msats_as_btc).unwrap_or_else(|| "Unknown".to_string());

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

            // Compact per-page selector integrated with pagination
            @if total_count > 0 {
                div class="per-page-selector" {
                    label for="per-page" { "Show:" }
                    select id="per-page" onchange="changePage()" {
                        option value="10" selected[per_page == 10] { "10" }
                        option value="25" selected[per_page == 25] { "25" }
                        option value="50" selected[per_page == 50] { "50" }
                        option value="100" selected[per_page == 100] { "100" }
                    }
                    span { "per page" }
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

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Payment History", content, is_running).into_string(),
    ))
}

pub async fn send_payments_page(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let content = html! {
        h2 style="text-align: center; margin-bottom: 3rem;" { "Send Payment" }

        div class="card" {
            // Tab navigation
            div class="payment-tabs" style="display: flex; gap: 0.5rem; margin-bottom: 1.5rem; border-bottom: 1px solid hsl(var(--border)); padding-bottom: 0;" {
                button type="button" class="payment-tab active" onclick="switchTab('bolt11')" data-tab="bolt11" {
                    "BOLT11 Invoice"
                }
                button type="button" class="payment-tab" onclick="switchTab('bolt12')" data-tab="bolt12" {
                    "BOLT12 Offer"
                }
            }

            // BOLT11 tab content
            div id="bolt11-content" class="tab-content active" {
                form method="post" action="/payments/bolt11" {
                    div class="form-group" {
                        label for="invoice" { "BOLT11 Invoice" }
                        textarea id="invoice" name="invoice" required placeholder="lnbc..." rows="4" {}
                    }
                    div class="form-group" {
                        label for="amount_btc_bolt11" { "Amount Override (optional)" }
                        input type="number" id="amount_btc_bolt11" name="amount_btc" placeholder="Leave empty to use invoice amount" step="1" {}
                        p style="font-size: 0.8125rem; color: hsl(var(--muted-foreground)); margin-top: 0.5rem;" {
                            "Only specify an amount if you want to override the invoice amount"
                        }
                    }
                    div class="form-actions" {
                        a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                        button type="submit" class="button-primary" { "Pay Invoice" }
                    }
                }
            }

            // BOLT12 tab content
            div id="bolt12-content" class="tab-content" {
                form method="post" action="/payments/bolt12" {
                    div class="form-group" {
                        label for="offer" { "BOLT12 Offer" }
                        textarea id="offer" name="offer" required placeholder="lno..." rows="4" {}
                    }
                    div class="form-group" {
                        label for="amount_btc_bolt12" { "Amount" }
                        input type="number" id="amount_btc_bolt12" name="amount_btc" placeholder="Amount in satoshis" step="1" {}
                        p style="font-size: 0.8125rem; color: hsl(var(--muted-foreground)); margin-top: 0.5rem;" {
                            "Required for variable amount offers, ignored for fixed amount offers"
                        }
                    }
                    div class="form-actions" {
                        a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                        button type="submit" class="button-primary" { "Pay Offer" }
                    }
                }
            }
        }

        // Tab switching script
        script type="text/javascript" {
            (maud::PreEscaped(r#"
            function switchTab(tabName) {
                console.log('Switching to tab:', tabName);

                // Hide all tab contents
                const contents = document.querySelectorAll('.tab-content');
                contents.forEach(content => content.classList.remove('active'));

                // Remove active class from all tabs
                const tabs = document.querySelectorAll('.payment-tab');
                tabs.forEach(tab => tab.classList.remove('active'));

                // Show selected tab content
                const tabContent = document.getElementById(tabName + '-content');
                if (tabContent) {
                    tabContent.classList.add('active');
                    console.log('Activated tab content:', tabName);
                }

                // Add active class to selected tab
                const tabButton = document.querySelector('[data-tab="' + tabName + '"]');
                if (tabButton) {
                    tabButton.classList.add('active');
                    console.log('Activated tab button:', tabName);
                }
            }
            "#))
        }
    };

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("Send Payments", content, is_running).into_string(),
    ))
}

pub async fn post_pay_bolt11(
    State(state): State<AppState>,
    Form(form): Form<PayBolt11Form>,
) -> Result<Response, StatusCode> {
    let invoice = match Bolt11Invoice::from_str(form.invoice.trim()) {
        Ok(inv) => inv,
        Err(e) => {
            tracing::warn!("Web interface: Invalid BOLT11 invoice provided: {}", e);
            let content = html! {
                (error_message(&format!("Invalid BOLT11 invoice: {e}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Payment Error", content, true).into_string(),
                ))
                .unwrap());
        }
    };

    tracing::info!(
        "Web interface: Attempting to pay BOLT11 invoice payment_hash={}, amount_override={:?}",
        invoice.payment_hash(),
        form.amount_btc
    );

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
        Ok(id) => {
            tracing::info!(
                "Web interface: BOLT11 payment initiated with payment_id={}",
                hex::encode(id.0)
            );
            id
        }
        Err(e) => {
            tracing::error!("Web interface: Failed to initiate BOLT11 payment: {}", e);
            let content = html! {
                (error_message(&format!("Failed to initiate payment: {e}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Payment Error", content, true).into_string(),
                ))
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
                    tracing::info!(
                        "Web interface: BOLT11 payment succeeded for payment_hash={}",
                        invoice.payment_hash()
                    );
                    break Ok(details);
                }
                PaymentStatus::Failed => {
                    tracing::error!(
                        "Web interface: BOLT11 payment failed for payment_hash={}",
                        invoice.payment_hash()
                    );
                    break Err("Payment failed".to_string());
                }
                PaymentStatus::Pending => {
                    if start.elapsed() > timeout {
                        tracing::warn!(
                            "Web interface: BOLT11 payment timeout for payment_hash={}",
                            invoice.payment_hash()
                        );
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
        .body(Body::from(
            layout_with_status("Payment Result", content, true).into_string(),
        ))
        .unwrap())
}

pub async fn post_pay_bolt12(
    State(state): State<AppState>,
    Form(form): Form<PayBolt12Form>,
) -> Result<Response, StatusCode> {
    let offer = match Offer::from_str(form.offer.trim()) {
        Ok(offer) => offer,
        Err(e) => {
            tracing::warn!("Web interface: Invalid BOLT12 offer provided: {:?}", e);
            let content = html! {
                (error_message(&format!("Invalid BOLT12 offer: {e:?}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Payment Error", content, true).into_string(),
                ))
                .unwrap());
        }
    };

    tracing::info!(
        "Web interface: Attempting to pay BOLT12 offer offer_id={}, amount_override={:?}",
        offer.id(),
        form.amount_btc
    );

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
                    tracing::warn!("Web interface: Amount required for variable amount BOLT12 offer but not provided");
                    let content = html! {
                        (error_message("Amount is required for variable amount offers. This offer does not have a fixed amount, so you must specify how much you want to pay."))
                        div class="card" {
                            a href="/payments" { button { "← Try Again" } }
                        }
                    };
                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header("content-type", "text/html")
                        .body(Body::from(
                            layout_with_status("Payment Error", content, true).into_string(),
                        ))
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
        Ok(id) => {
            tracing::info!(
                "Web interface: BOLT12 payment initiated with payment_id={}",
                hex::encode(id.0)
            );
            id
        }
        Err(e) => {
            tracing::error!("Web interface: Failed to initiate BOLT12 payment: {}", e);
            let content = html! {
                (error_message(&format!("Failed to initiate payment: {e}")))
                div class="card" {
                    a href="/payments" { button { "← Try Again" } }
                }
            };
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/html")
                .body(Body::from(
                    layout_with_status("Payment Error", content, true).into_string(),
                ))
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
                    tracing::info!(
                        "Web interface: BOLT12 payment succeeded for offer_id={}",
                        offer.id()
                    );
                    break Ok(details);
                }
                PaymentStatus::Failed => {
                    tracing::error!(
                        "Web interface: BOLT12 payment failed for offer_id={}",
                        offer.id()
                    );
                    break Err("Payment failed".to_string());
                }
                PaymentStatus::Pending => {
                    if start.elapsed() > timeout {
                        tracing::warn!(
                            "Web interface: BOLT12 payment timeout for offer_id={}",
                            offer.id()
                        );
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
        .body(Body::from(
            layout_with_status("Payment Result", content, true).into_string(),
        ))
        .unwrap())
}
