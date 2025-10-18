use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::Form;
use ldk_node::lightning_invoice::{Bolt11InvoiceDescription, Description};
use maud::html;
use serde::Deserialize;

use crate::web::handlers::utils::{deserialize_optional_f64, deserialize_optional_u32};
use crate::web::handlers::AppState;
use crate::web::templates::{
    error_message, format_sats_as_btc, invoice_display_card, is_node_running, layout_with_status,
    success_message,
};

#[derive(Deserialize)]
pub struct CreateBolt11Form {
    amount_btc: u64,
    description: Option<String>,
    #[serde(deserialize_with = "deserialize_optional_u32")]
    expiry_seconds: Option<u32>,
}

#[derive(Deserialize)]
pub struct CreateBolt12Form {
    #[serde(deserialize_with = "deserialize_optional_f64")]
    amount_btc: Option<f64>,
    description: Option<String>,
    #[serde(deserialize_with = "deserialize_optional_u32")]
    expiry_seconds: Option<u32>,
}

pub async fn invoices_page(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let content = html! {
        h2 style="text-align: center; margin-bottom: 3rem;" { "Invoices" }

        div class="card" {
            // Tab navigation
            div class="payment-tabs" style="display: flex; gap: 0.5rem; margin-bottom: 1.5rem; border-bottom: 1px solid hsl(var(--border)); padding-bottom: 0;" {
                button type="button" class="payment-tab active" onclick="switchInvoiceTab('bolt11')" data-tab="bolt11" {
                    "BOLT11 Invoice"
                }
                button type="button" class="payment-tab" onclick="switchInvoiceTab('bolt12')" data-tab="bolt12" {
                    "BOLT12 Offer"
                }
            }

            // BOLT11 tab content
            div id="bolt11-content" class="tab-content active" {
                form method="post" action="/invoices/bolt11" {
                    div class="form-group" {
                        label for="amount_btc_bolt11" { "Amount" }
                        input type="number" id="amount_btc_bolt11" name="amount_btc" required placeholder="₿0" step="0.00000001" {}
                    }
                    div class="form-group" {
                        label for="description_bolt11" { "Description (optional)" }
                        input type="text" id="description_bolt11" name="description" placeholder="Payment for..." {}
                    }
                    div class="form-group" {
                        label for="expiry_seconds_bolt11" { "Expiry (seconds, optional)" }
                        input type="number" id="expiry_seconds_bolt11" name="expiry_seconds" placeholder="3600" {}
                    }
                    div class="form-actions" {
                        a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                        button type="submit" class="button-primary" { "Create BOLT11 Invoice" }
                    }
                }
            }

            // BOLT12 tab content
            div id="bolt12-content" class="tab-content" {
                form method="post" action="/invoices/bolt12" {
                    div class="form-group" {
                        label for="amount_btc_bolt12" { "Amount (optional for variable amount)" }
                        input type="number" id="amount_btc_bolt12" name="amount_btc" placeholder="₿0" step="0.00000001" {}
                        p style="font-size: 0.8125rem; color: hsl(var(--muted-foreground)); margin-top: 0.5rem;" {
                            "Leave empty for variable amount offers, specify amount for fixed offers"
                        }
                    }
                    div class="form-group" {
                        label for="description_bolt12" { "Description (optional)" }
                        input type="text" id="description_bolt12" name="description" placeholder="Payment for..." {}
                    }
                    div class="form-group" {
                        label for="expiry_seconds_bolt12" { "Expiry (seconds, optional)" }
                        input type="number" id="expiry_seconds_bolt12" name="expiry_seconds" placeholder="3600" {}
                    }
                    div class="form-actions" {
                        a href="/balance" { button type="button" class="button-secondary" { "Cancel" } }
                        button type="submit" class="button-primary" { "Create BOLT12 Offer" }
                    }
                }
            }
        }

        // Tab switching script
        script type="text/javascript" {
            (maud::PreEscaped(r#"
            function switchInvoiceTab(tabName) {
                console.log('Switching to invoice tab:', tabName);

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
                    console.log('Activated invoice tab content:', tabName);
                }

                // Add active class to selected tab
                const tabButton = document.querySelector('[data-tab="' + tabName + '"]');
                if (tabButton) {
                    tabButton.classList.add('active');
                    console.log('Activated invoice tab button:', tabName);
                }
            }
            "#))
        }
    };

    let is_running = is_node_running(&state.node.inner);
    Ok(Html(
        layout_with_status("s", content, is_running).into_string(),
    ))
}

pub async fn post_create_bolt11(
    State(state): State<AppState>,
    Form(form): Form<CreateBolt11Form>,
) -> Result<Response, StatusCode> {
    tracing::info!(
        "Web interface: Creating BOLT11 invoice for amount={} sats, description={:?}, expiry={}s",
        form.amount_btc,
        form.description,
        form.expiry_seconds.unwrap_or(3600)
    );

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
                tracing::warn!(
                    "Web interface: Invalid description for BOLT11 invoice: {}",
                    e
                );
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
                        layout_with_status(" Error", content, true).into_string(),
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
            tracing::info!(
                "Web interface: Successfully created BOLT11 invoice with payment_hash={}",
                invoice.payment_hash()
            );
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let description_display = if description_text.is_empty() {
                "None".to_string()
            } else {
                description_text.clone()
            };

            let invoice_details = vec![
                ("Payment Hash", invoice.payment_hash().to_string()),
                ("Amount", format_sats_as_btc(form.amount_btc)),
                ("Description", description_display),
                (
                    "Expires At",
                    format!("{}", current_time + expiry_seconds as u64),
                ),
            ];

            html! {
                (success_message("BOLT11 Invoice created successfully!"))
                (invoice_display_card(&invoice.to_string(), &format_sats_as_btc(form.amount_btc), invoice_details, "/invoices"))
            }
        }
        Err(e) => {
            tracing::error!("Web interface: Failed to create BOLT11 invoice: {}", e);
            html! {
                (error_message(&format!("Failed to : {e}")))
                div class="card" {
                    a href="/invoices" { button { "← Try Again" } }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout_with_status("BOLT11 Invoice Created", content, true).into_string(),
        ))
        .unwrap())
}

pub async fn post_create_bolt12(
    State(state): State<AppState>,
    Form(form): Form<CreateBolt12Form>,
) -> Result<Response, StatusCode> {
    let expiry_seconds = form.expiry_seconds.unwrap_or(3600);
    let description_text = form.description.unwrap_or_else(|| "".to_string());

    tracing::info!(
        "Web interface: Creating BOLT12 offer for amount={:?} sats, description={:?}, expiry={}s",
        form.amount_btc,
        description_text,
        expiry_seconds
    );

    let offer_result = if let Some(amount_btc) = form.amount_btc {
        // Convert satoshis to millisatoshis (1 sat = 1,000 msats)
        let amount_msats = (amount_btc * 1_000.0) as u64;
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
            tracing::info!(
                "Web interface: Successfully created BOLT12 offer with offer_id={}",
                offer.id()
            );
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            let amount_display = form
                .amount_btc
                .map(|a| format_sats_as_btc(a as u64))
                .unwrap_or_else(|| "Variable amount".to_string());

            let description_display = if description_text.is_empty() {
                "None".to_string()
            } else {
                description_text
            };

            let offer_details = vec![
                ("Offer ID", offer.id().to_string()),
                ("Amount", amount_display.clone()),
                ("Description", description_display),
                (
                    "Expires At",
                    format!("{}", current_time + expiry_seconds as u64),
                ),
            ];

            html! {
                (success_message("BOLT12 Offer created successfully!"))
                (invoice_display_card(&offer.to_string(), &amount_display, offer_details, "/invoices"))
            }
        }
        Err(e) => {
            tracing::error!("Web interface: Failed to create BOLT12 offer: {}", e);
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
            layout_with_status("BOLT12 Offer Created", content, true).into_string(),
        ))
        .unwrap())
}
