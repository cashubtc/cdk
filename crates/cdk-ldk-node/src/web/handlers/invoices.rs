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
    error_message, form_card, format_sats_as_btc, info_card, layout, success_message,
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

pub async fn invoices_page(State(_state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let content = html! {
        h2 style="text-align: center; margin-bottom: 3rem;" { "Invoices" }
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
            tracing::error!("Web interface: Failed to create BOLT11 invoice: {}", e);
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

pub async fn post_create_bolt12(
    State(state): State<AppState>,
    Form(form): Form<CreateBolt12Form>,
) -> Result<Response, StatusCode> {
    let expiry_seconds = form.expiry_seconds.unwrap_or(3600);
    let description_text = form.description.unwrap_or_else(|| "".to_string());

    tracing::info!(
        "Web interface: Creating BOLT12 offer for amount={:?} btc, description={:?}, expiry={}s",
        form.amount_btc,
        description_text,
        expiry_seconds
    );

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
            layout("BOLT12 Offer Created", content).into_string(),
        ))
        .unwrap())
}
