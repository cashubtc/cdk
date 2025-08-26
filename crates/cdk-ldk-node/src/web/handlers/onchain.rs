use std::collections::HashMap;
use std::str::FromStr;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::Form;
use ldk_node::bitcoin::Address;
use maud::html;
use serde::{Deserialize, Serialize};

use crate::web::handlers::utils::deserialize_optional_u64;
use crate::web::handlers::AppState;
use crate::web::templates::{
    error_message, form_card, format_sats_as_btc, info_card, layout, success_message,
};

#[derive(Deserialize, Serialize)]
pub struct SendOnchainActionForm {
    address: String,
    #[serde(deserialize_with = "deserialize_optional_u64")]
    amount_sat: Option<u64>,
    send_action: String,
}

#[derive(Deserialize)]
pub struct ConfirmOnchainForm {
    address: String,
    amount_sat: Option<u64>,
    send_action: String,
    confirmed: Option<String>,
}

pub async fn get_new_address(State(state): State<AppState>) -> Result<Html<String>, StatusCode> {
    let address_result = state.node.inner.onchain_payment().new_address();

    let content = match address_result {
        Ok(address) => {
            html! {
                (success_message(&format!("New address generated: {address}")))
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
                (error_message(&format!("Failed to generate address: {e}")))
                div class="card" {
                    a href="/onchain" { button { "← Back to On-chain" } }
                }
            }
        }
    };

    Ok(Html(layout("New Address", content).into_string()))
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
        h2 style="text-align: center; margin-bottom: 3rem;" { "On-chain" }

        // Quick Actions section - matching dashboard style
        div class="card" style="margin-bottom: 2rem;" {
            h2 { "Quick Actions" }
            div style="display: flex; gap: 1rem; margin-top: 1rem; flex-wrap: wrap;" {
                a href="/onchain?action=receive" style="text-decoration: none; flex: 1; min-width: 200px;" {
                    button class="button-primary" style="width: 100%;" { "Receive Bitcoin" }
                }
                a href="/onchain?action=send" style="text-decoration: none; flex: 1; min-width: 200px;" {
                    button class="button-primary" style="width: 100%;" { "Send Bitcoin" }
                }
            }
        }

        // On-chain Balance as metric cards
        div class="card" {
            h2 { "On-chain Balance" }
            div class="metrics-container" {
                div class="metric-card" {
                    div class="metric-value" { (format_sats_as_btc(balances.total_onchain_balance_sats)) }
                    div class="metric-label" { "Total Balance" }
                }
                div class="metric-card" {
                    div class="metric-value" { (format_sats_as_btc(balances.spendable_onchain_balance_sats)) }
                    div class="metric-label" { "Spendable Balance" }
                }
            }
        }
    };

    match action {
        "send" => {
            content = html! {
                h2 style="text-align: center; margin-bottom: 3rem;" { "On-chain" }

                // Quick Actions section - matching dashboard style
                div class="card" style="margin-bottom: 2rem;" {
                    h2 { "Quick Actions" }
                    div style="display: flex; gap: 1rem; margin-top: 1rem; flex-wrap: wrap;" {
                        a href="/onchain?action=receive" style="text-decoration: none; flex: 1; min-width: 200px;" {
                            button class="button-primary" style="width: 100%;" { "Receive Bitcoin" }
                        }
                        a href="/onchain?action=send" style="text-decoration: none; flex: 1; min-width: 200px;" {
                            button class="button-primary" style="width: 100%;" { "Send Bitcoin" }
                        }
                    }
                }

                // Send form above balance
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
                                input type="number" id="amount_sat" name="amount_sat" placeholder="0" {}
                            }
                            input type="hidden" id="send_action" name="send_action" value="send" {}
                            div style="display: flex; justify-content: space-between; gap: 1rem; margin-top: 2rem;" {
                                a href="/onchain" { button type="button" { "Cancel" } }
                                div style="display: flex; gap: 0.5rem;" {
                                    button type="submit" onclick="document.getElementById('send_action').value='send'" { "Send Payment" }
                                    button type="submit" onclick="document.getElementById('send_action').value='send_all'; document.getElementById('amount_sat').value=''" { "Send All" }
                                }
                            }
                        }
                    }
                ))

                // On-chain Balance as metric cards
                div class="card" {
                    h2 { "On-chain Balance" }
                    div class="metrics-container" {
                        div class="metric-card" {
                            div class="metric-value" { (format_sats_as_btc(balances.total_onchain_balance_sats)) }
                            div class="metric-label" { "Total Balance" }
                        }
                        div class="metric-card" {
                            div class="metric-value" { (format_sats_as_btc(balances.spendable_onchain_balance_sats)) }
                            div class="metric-label" { "Spendable Balance" }
                        }
                    }
                }
            };
        }
        "receive" => {
            content = html! {
                h2 style="text-align: center; margin-bottom: 3rem;" { "On-chain" }

                // Quick Actions section - matching dashboard style
                div class="card" style="margin-bottom: 2rem;" {
                    h2 { "Quick Actions" }
                    div style="display: flex; gap: 1rem; margin-top: 1rem; flex-wrap: wrap;" {
                        a href="/onchain?action=receive" style="text-decoration: none; flex: 1; min-width: 200px;" {
                            button class="button-primary" style="width: 100%;" { "Receive Bitcoin" }
                        }
                        a href="/onchain?action=send" style="text-decoration: none; flex: 1; min-width: 200px;" {
                            button class="button-primary" style="width: 100%;" { "Send Bitcoin" }
                        }
                    }
                }

                // Generate address form above balance
                (form_card(
                    "Generate New Address",
                    html! {
                        form method="post" action="/onchain/new-address" {
                            p style="margin-bottom: 2rem;" { "Click the button below to generate a new Bitcoin address for receiving on-chain payments." }
                            div style="display: flex; justify-content: space-between; gap: 1rem;" {
                                a href="/onchain" { button type="button" { "Cancel" } }
                                button class="button-primary" type="submit" { "Generate New Address" }
                            }
                        }
                    }
                ))

                // On-chain Balance as metric cards
                div class="card" {
                    h2 { "On-chain Balance" }
                    div class="metrics-container" {
                        div class="metric-card" {
                            div class="metric-value" { (format_sats_as_btc(balances.total_onchain_balance_sats)) }
                            div class="metric-label" { "Total Balance" }
                        }
                        div class="metric-card" {
                            div class="metric-value" { (format_sats_as_btc(balances.spendable_onchain_balance_sats)) }
                            div class="metric-label" { "Spendable Balance" }
                        }
                    }
                }
            };
        }
        _ => {
            // Show overview with just the balance and quick actions at the top
        }
    }

    Ok(Html(layout("On-chain", content).into_string()))
}

pub async fn post_send_onchain(
    State(_state): State<AppState>,
    Form(form): Form<SendOnchainActionForm>,
) -> Result<Response, StatusCode> {
    let encoded_form =
        serde_urlencoded::to_string(&form).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Response::builder()
        .status(StatusCode::FOUND)
        .header("Location", format!("/onchain/confirm?{}", encoded_form))
        .body(Body::empty())
        .unwrap())
}

pub async fn onchain_confirm_page(
    State(state): State<AppState>,
    query: Query<ConfirmOnchainForm>,
) -> Result<Response, StatusCode> {
    let form = query.0;

    // If user confirmed, execute the transaction
    if form.confirmed.as_deref() == Some("true") {
        return execute_onchain_transaction(State(state), form).await;
    }

    // Validate address
    let _address = match Address::from_str(&form.address) {
        Ok(addr) => addr,
        Err(e) => {
            let content = html! {
                (error_message(&format!("Invalid address: {e}")))
                div class="card" {
                    a href="/onchain?action=send" { button { "← Back" } }
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

    let balances = state.node.inner.list_balances();
    let spendable_balance = balances.spendable_onchain_balance_sats;

    // Calculate transaction details
    let (amount_to_send, is_send_all) = if form.send_action == "send_all" {
        (spendable_balance, true)
    } else {
        let amount = form.amount_sat.unwrap_or(0);
        if amount > spendable_balance {
            let content = html! {
                (error_message(&format!("Insufficient funds. Requested: {}, Available: {}",
                    format_sats_as_btc(amount), format_sats_as_btc(spendable_balance))))
                div class="card" {
                    a href="/onchain?action=send" { button { "← Back" } }
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
        (amount, false)
    };

    let confirmation_url = if form.send_action == "send_all" {
        format!(
            "/onchain/confirm?address={}&send_action={}&confirmed=true",
            urlencoding::encode(&form.address),
            form.send_action
        )
    } else {
        format!(
            "/onchain/confirm?address={}&amount_sat={}&send_action={}&confirmed=true",
            urlencoding::encode(&form.address),
            form.amount_sat.unwrap_or(0),
            form.send_action
        )
    };

    let content = html! {
        h2 style="text-align: center; margin-bottom: 3rem;" { "Confirm On-chain Transaction" }

        div class="card" style="border: 2px solid hsl(var(--primary)); background-color: hsl(var(--primary) / 0.05);" {
            h2 { "⚠️ Transaction Confirmation" }
            p style="color: hsl(var(--muted-foreground)); margin-bottom: 1.5rem;" {
                "Please review the transaction details carefully before proceeding. This action cannot be undone."
            }
        }

        (info_card(
            "Transaction Details",
            vec![
                ("Recipient Address", form.address.clone()),
                ("Amount to Send", if is_send_all {
                    format!("{} (All available funds)", format_sats_as_btc(amount_to_send))
                } else {
                    format_sats_as_btc(amount_to_send)
                }),
                ("Current Spendable Balance", format_sats_as_btc(spendable_balance)),
            ]
        ))

        @if is_send_all {
            div class="card" style="border: 1px solid hsl(32.6 75.4% 55.1%); background-color: hsl(32.6 75.4% 55.1% / 0.1);" {
                h3 style="color: hsl(32.6 75.4% 55.1%);" { "Send All Notice" }
                p style="color: hsl(32.6 75.4% 55.1%);" {
                    "This transaction will send all available funds to the recipient address. "
                    "Network fees will be deducted from the total amount automatically."
                }
            }
        }

        div class="card" {
            div style="display: flex; justify-content: space-between; gap: 1rem; margin-top: 2rem;" {
                a href="/onchain?action=send" {
                    button type="button" class="button-secondary" { "← Cancel" }
                }
                div style="display: flex; gap: 0.5rem;" {
                    a href=(confirmation_url) {
                        button class="button-primary" {
                            "✓ Confirm & Send Transaction"
                        }
                    }
                }
            }
        }
    };

    Ok(Response::builder()
        .header("content-type", "text/html")
        .body(Body::from(
            layout("Confirm Transaction", content).into_string(),
        ))
        .unwrap())
}

async fn execute_onchain_transaction(
    State(state): State<AppState>,
    form: ConfirmOnchainForm,
) -> Result<Response, StatusCode> {
    tracing::info!(
        "Web interface: Executing on-chain transaction to address={}, send_action={}, amount_sat={:?}",
        form.address,
        form.send_action,
        form.amount_sat
    );

    let address = match Address::from_str(&form.address) {
        Ok(addr) => addr,
        Err(e) => {
            tracing::warn!(
                "Web interface: Invalid address for on-chain transaction: {}",
                e
            );
            let content = html! {
                (error_message(&format!("Invalid address: {e}")))
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

    // Handle send all action
    let txid_result = if form.send_action == "send_all" {
        tracing::info!(
            "Web interface: Sending all available funds to {}",
            form.address
        );
        state.node.inner.onchain_payment().send_all_to_address(
            address.assume_checked_ref(),
            false,
            None,
        )
    } else {
        let amount_sats = form.amount_sat.ok_or(StatusCode::BAD_REQUEST)?;
        tracing::info!(
            "Web interface: Sending {} sats to {}",
            amount_sats,
            form.address
        );
        state.node.inner.onchain_payment().send_to_address(
            address.assume_checked_ref(),
            amount_sats,
            None,
        )
    };

    let content = match txid_result {
        Ok(txid) => {
            if form.send_action == "send_all" {
                tracing::info!(
                    "Web interface: Successfully sent all available funds, txid={}",
                    txid
                );
            } else {
                tracing::info!(
                    "Web interface: Successfully sent {} sats, txid={}",
                    form.amount_sat.unwrap_or(0),
                    txid
                );
            }
            let amount = form.amount_sat;
            html! {
                        (success_message("Transaction sent successfully!"))
                        (info_card(
                            "Transaction Details",
                            vec![
                                ("Transaction ID", txid.to_string()),
                                ("Amount", if form.send_action == "send_all" {
                                    format!("{} (All available funds)", format_sats_as_btc(amount.unwrap_or(0)))
                                } else {
                                    format_sats_as_btc(form.amount_sat.unwrap_or(0))
                                }),
                                ("Recipient", form.address),
                            ]
                        ))
                        div class="card" {
                            a href="/onchain" { button { "← Back to On-chain" } }
                        }
            }
        }
        Err(e) => {
            tracing::error!("Web interface: Failed to send on-chain transaction: {}", e);
            html! {
                (error_message(&format!("Failed to send payment: {e}")))
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
