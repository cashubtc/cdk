//! CDK lightning backend for Blink
//!
//! Blink exposes a GraphQL API at `https://api.blink.sv/graphql` with
//! authentication via API key. Each account has two wallets: BTC (sats)
//! and USD (cents).

use std::cmp::max;
use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use cdk_common::amount::{Amount, MSAT_IN_SAT};
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse,
    MintPayment, OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::util::hex;
use cdk_common::Bolt11Invoice;
use error::Error;
use futures::Stream;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub mod error;

/// Maximum fee percentage for fee probe fallback
const BLINK_MAX_FEE_PERCENT: f64 = 0.005;
/// Minimum fee in millisatoshis for fee probe fallback
const MINIMUM_FEE_MSAT: u64 = 10_000;
/// Timeout in seconds for fee probe requests
const PROBE_FEE_TIMEOUT_SECS: u64 = 5;
/// Default Blink GraphQL endpoint
const DEFAULT_ENDPOINT: &str = "https://api.blink.sv/graphql";

/// Check if a currency unit is USD
fn is_usd_unit(unit: &CurrencyUnit) -> bool {
    unit.to_string().to_lowercase() == "usd"
}

/// Blink lightning backend
#[derive(Clone)]
pub struct Blink {
    client: reqwest::Client,
    endpoint: String,
    btc_wallet_id: String,
    usd_wallet_id: String,
    unit: CurrencyUnit,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    settings: SettingsResponse,
}

impl fmt::Debug for Blink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Blink")
            .field("endpoint", &self.endpoint)
            .field("unit", &self.unit)
            .field("fee_reserve", &self.fee_reserve)
            .finish_non_exhaustive()
    }
}

impl Blink {
    /// Create a new [`Blink`] backend
    ///
    /// The `unit` parameter determines which Blink wallet is used:
    /// - `CurrencyUnit::Sat` / `CurrencyUnit::Msat` → BTC wallet (sats)
    /// - USD → USD wallet (cents)
    pub async fn new(
        api_key: String,
        endpoint: Option<String>,
        fee_reserve: FeeReserve,
        unit: CurrencyUnit,
    ) -> Result<Self, Error> {
        let endpoint = endpoint.unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "X-API-KEY",
            reqwest::header::HeaderValue::from_str(&api_key)
                .map_err(|e| Error::Anyhow(anyhow!("Invalid API key header: {e}")))?,
        );
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(Error::Reqwest)?;

        let (btc_wallet_id, usd_wallet_id) = fetch_wallet_ids(&client, &endpoint).await?;

        Ok(Self {
            client,
            endpoint,
            btc_wallet_id,
            usd_wallet_id,
            unit: unit.clone(),
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            settings: SettingsResponse {
                unit: unit.to_string(),
                bolt11: Some(payment::Bolt11Settings {
                    mpp: false,
                    amountless: false,
                    invoice_description: true,
                }),
                bolt12: None,
                custom: std::collections::HashMap::new(),
            },
        })
    }

    /// Get the wallet ID for a given currency unit
    fn wallet_id_for_unit(&self, unit: &CurrencyUnit) -> Result<&str, Error> {
        match unit {
            CurrencyUnit::Sat | CurrencyUnit::Msat => Ok(&self.btc_wallet_id),
            _ => {
                if is_usd_unit(unit) {
                    Ok(&self.usd_wallet_id)
                } else {
                    Err(Error::UnsupportedUnit)
                }
            }
        }
    }

    /// Get the sats-per-cent exchange rate from Blink.
    ///
    /// Calls `currencyConversionEstimation(amount: 1, currency: "USD")` which returns
    /// how many sats 1 USD is worth (`btcSatAmount`). Then divides by 100 to get
    /// sats per cent.
    async fn get_sats_per_cent(&self) -> Result<f64, Error> {
        let query = r#"
            query currencyConversionEstimation($amount: Float!) {
                currencyConversionEstimation(amount: $amount, currency: "USD") {
                    btcSatAmount
                }
            }
        "#;

        let variables = serde_json::json!({
            "amount": 1
        });

        let data = graphql_request(&self.client, &self.endpoint, query, Some(variables)).await?;

        let btc_sat_amount = data
            .get("currencyConversionEstimation")
            .and_then(|e| e.get("btcSatAmount"))
            .and_then(|a| a.as_f64())
            .ok_or(Error::CurrencyConversionFailed)?;

        if btc_sat_amount == 0.0 {
            return Err(Error::CurrencyConversionFailed);
        }

        // btcSatAmount = sats per 1 USD = sats per 100 cents
        Ok(btc_sat_amount / 100.0)
    }

    /// Convert a sat amount to USD cents using Blink's exchange rate
    async fn sats_to_cents(&self, sats: u64) -> Result<u64, Error> {
        let sats_per_cent = self.get_sats_per_cent().await?;
        Ok((sats as f64 / sats_per_cent).ceil() as u64)
    }

    /// Convert msat to the target unit amount.
    /// For SAT/MSAT uses the built-in Amount conversion.
    /// For USD queries Blink's exchange rate and converts sats → cents.
    async fn msat_to_unit(
        &self,
        msat: u64,
        unit: &CurrencyUnit,
    ) -> Result<Amount<CurrencyUnit>, Error> {
        if is_usd_unit(unit) {
            let sats = msat / MSAT_IN_SAT;
            let cents = self.sats_to_cents(sats).await?;
            Ok(Amount::new(cents, unit.clone()))
        } else {
            Amount::new(msat, CurrencyUnit::Msat)
                .convert_to(unit)
                .map_err(|e| Error::Anyhow(anyhow!("Cannot convert units: {e}")))
        }
    }

    /// Return an Amount in the instance's native unit from a settlement value.
    /// Blink settlement amounts are in sats for BTC wallets, cents for USD wallets.
    fn settlement_to_amount(
        &self,
        settlement_value: u64,
        unit: &CurrencyUnit,
    ) -> Amount<CurrencyUnit> {
        if is_usd_unit(unit) {
            // USD wallet settlement amounts are in cents
            Amount::new(settlement_value, unit.clone())
        } else {
            // BTC wallet settlement amounts are in sats → convert to msat
            Amount::new(settlement_value * MSAT_IN_SAT, CurrencyUnit::Msat)
        }
    }
}

/// Execute a GraphQL request against the Blink API
async fn graphql_request(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    variables: Option<Value>,
) -> Result<Value, Error> {
    let body = serde_json::json!({
        "query": query,
        "variables": variables.unwrap_or(Value::Null),
    });

    let response = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(Error::Reqwest)?;

    let json: Value = response.json().await.map_err(Error::Reqwest)?;

    // Check for GraphQL errors
    if let Some(errors) = json.get("errors") {
        if let Some(arr) = errors.as_array() {
            if !arr.is_empty() {
                let msg = arr
                    .iter()
                    .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(Error::GraphQL(msg));
            }
        }
    }

    json.get("data")
        .cloned()
        .ok_or_else(|| Error::GraphQL("No data in GraphQL response".to_string()))
}

/// Execute a GraphQL request with a timeout
async fn graphql_request_with_timeout(
    client: &reqwest::Client,
    endpoint: &str,
    query: &str,
    variables: Option<Value>,
    timeout_secs: u64,
) -> Result<Value, Error> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        graphql_request(client, endpoint, query, variables),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(Error::Anyhow(anyhow!("GraphQL request timed out"))),
    }
}

/// Fetch BTC and USD wallet IDs from the Blink `me` query
async fn fetch_wallet_ids(
    client: &reqwest::Client,
    endpoint: &str,
) -> Result<(String, String), Error> {
    let query = r#"
        query me {
            me {
                defaultAccount {
                    wallets {
                        id
                        walletCurrency
                    }
                }
            }
        }
    "#;

    let data = graphql_request(client, endpoint, query, None).await?;

    let wallets = data
        .get("me")
        .and_then(|me| me.get("defaultAccount"))
        .and_then(|acc| acc.get("wallets"))
        .and_then(|w| w.as_array())
        .ok_or_else(|| Error::GraphQL("Could not parse wallets from me query".to_string()))?;

    let mut btc_id = None;
    let mut usd_id = None;

    for wallet in wallets {
        let currency = wallet
            .get("walletCurrency")
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        let id = wallet
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default();

        match currency {
            "BTC" => btc_id = Some(id.to_string()),
            "USD" => usd_id = Some(id.to_string()),
            _ => {}
        }
    }

    let btc_wallet_id = btc_id.ok_or(Error::WalletIdNotFound)?;
    let usd_wallet_id = usd_id.ok_or(Error::WalletIdNotFound)?;

    tracing::info!(
        "Blink wallets discovered: BTC={}, USD={}",
        btc_wallet_id,
        usd_wallet_id
    );

    Ok((btc_wallet_id, usd_wallet_id))
}

/// Map Blink payment status to MeltQuoteState
fn blink_to_melt_status(status: &str) -> MeltQuoteState {
    match status.to_uppercase().as_str() {
        "SUCCESS" => MeltQuoteState::Paid,
        "FAILED" => MeltQuoteState::Unpaid,
        "PENDING" => MeltQuoteState::Pending,
        _ => MeltQuoteState::Unknown,
    }
}

#[async_trait]
impl MintPayment for Blink {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(self.settings.clone())
    }

    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let cancel_token = self.wait_invoice_cancel_token.clone();
        let is_active = Arc::clone(&self.wait_invoice_is_active);

        Ok(Box::pin(futures::stream::unfold(
            (cancel_token, is_active),
            |(cancel_token, is_active)| async move {
                is_active.store(true, Ordering::SeqCst);
                cancel_token.cancelled().await;
                is_active.store(false, Ordering::SeqCst);
                tracing::info!("Blink: wait_payment_event cancelled");
                None
            },
        )))
    }

    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let amount_msat = match bolt11_options.melt_options {
                    Some(amount) => {
                        if matches!(amount, MeltOptions::Mpp { mpp: _ }) {
                            return Err(payment::Error::UnsupportedPaymentOption);
                        }
                        amount.amount_msat()
                    }
                    None => Amount::from(
                        bolt11_options
                            .bolt11
                            .amount_milli_satoshis()
                            .ok_or(Error::UnknownInvoiceAmount)?,
                    ),
                };

                let invoice_str = bolt11_options.bolt11.to_string();
                let wallet_id = self.wallet_id_for_unit(unit)?;

                // Try fee probe with timeout
                let probe_fee = self.probe_fee(wallet_id, &invoice_str, unit).await;

                let fee_msat = match probe_fee {
                    Ok(fee) => fee,
                    Err(e) => {
                        tracing::warn!("Blink fee probe failed, using fallback: {}", e);
                        // Fallback: max(amount * 0.5%, 10_000 msat)
                        let relative =
                            (u64::from(amount_msat) as f64 * BLINK_MAX_FEE_PERCENT) as u64;
                        max(relative, MINIMUM_FEE_MSAT)
                    }
                };

                // Also apply our configured fee reserve
                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * u64::from(amount_msat) as f32) as u64;
                let absolute_fee_reserve: u64 =
                    u64::from(self.fee_reserve.min_fee_reserve) * MSAT_IN_SAT;
                let reserve_fee = max(relative_fee_reserve, absolute_fee_reserve);

                // Use the larger of probe fee and reserve fee
                let fee = max(fee_msat, reserve_fee);

                // Convert amounts from msat to the target unit
                let amount = self.msat_to_unit(u64::from(amount_msat), unit).await?;
                let mut fee = self.msat_to_unit(fee, unit).await?;

                // Ensure minimum fee in the target unit.
                // min_fee_reserve is in native units (sats for BTC, cents for USD).
                // The msat-based reserve calculation above is wrong for USD because it
                // treats min_fee_reserve as sats, so we enforce it post-conversion.
                let min_fee =
                    Amount::new(u64::from(self.fee_reserve.min_fee_reserve), unit.clone());
                if fee < min_fee {
                    fee = min_fee;
                }

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    )),
                    amount,
                    fee,
                    state: MeltQuoteState::Unpaid,
                })
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by Blink")))
            }
            OutgoingPaymentOptions::Custom(_) => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let invoice_str = bolt11_options.bolt11.to_string();
                let wallet_id = self.wallet_id_for_unit(unit)?;

                let query = r#"
                    mutation lnInvoicePaymentSend($input: LnInvoicePaymentInput!) {
                        lnInvoicePaymentSend(input: $input) {
                            status
                            errors {
                                message
                            }
                        }
                    }
                "#;

                let variables = serde_json::json!({
                    "input": {
                        "walletId": wallet_id,
                        "paymentRequest": invoice_str,
                    }
                });

                let data = graphql_request(&self.client, &self.endpoint, query, Some(variables))
                    .await
                    .map_err(|e| {
                        tracing::error!("Blink: Could not pay invoice: {e}");
                        Self::Err::Anyhow(anyhow!("Could not pay invoice: {e}"))
                    })?;

                let send_result = data.get("lnInvoicePaymentSend").ok_or_else(|| {
                    Self::Err::Anyhow(anyhow!("No lnInvoicePaymentSend in response"))
                })?;

                // Check for errors in the mutation response
                if let Some(errors) = send_result.get("errors").and_then(|e| e.as_array()) {
                    if !errors.is_empty() {
                        let msg = errors
                            .iter()
                            .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                            .collect::<Vec<_>>()
                            .join("; ");
                        return Err(Self::Err::Anyhow(anyhow!("Payment failed: {msg}")));
                    }
                }

                let status_str = send_result
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("PENDING");

                let status = blink_to_melt_status(status_str);

                let payment_hash = bolt11_options.bolt11.payment_hash();
                let payment_hash_hex = hex::encode(AsRef::<[u8]>::as_ref(payment_hash));

                // Get the settlement fee from checking the transaction
                let total_spent = self
                    .get_transaction_total_spent(&payment_hash_hex, unit)
                    .await
                    .unwrap_or_else(|_| {
                        // Fallback: zero amount in the target unit
                        Amount::new(0, unit.clone())
                    });

                Ok(MakePaymentResponse {
                    payment_lookup_id: PaymentIdentifier::PaymentHash(*payment_hash.as_ref()),
                    payment_proof: Some(payment_hash_hex),
                    status,
                    total_spent,
                })
            }
            OutgoingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by Blink")))
            }
            OutgoingPaymentOptions::Custom(_) => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    async fn create_incoming_payment_request(
        &self,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(bolt11_options) => {
                let description = bolt11_options.description.unwrap_or_default();
                let amount = bolt11_options.amount;
                let wallet_id = self.wallet_id_for_unit(unit)?;

                // Use different GraphQL mutations for SAT vs USD
                let (data, mutation_name) = if is_usd_unit(unit) {
                    // USD: amount is in cents, use USD-specific mutation
                    let query = r#"
                        mutation LnUsdInvoiceCreateOnBehalfOfRecipient($input: LnUsdInvoiceCreateOnBehalfOfRecipientInput!) {
                            lnUsdInvoiceCreateOnBehalfOfRecipient(input: $input) {
                                invoice {
                                    paymentRequest
                                    paymentHash
                                    satoshis
                                }
                                errors {
                                    message
                                }
                            }
                        }
                    "#;

                    let variables = serde_json::json!({
                        "input": {
                            "recipientWalletId": wallet_id,
                            "amount": amount,
                            "memo": description,
                        }
                    });

                    let data =
                        graphql_request(&self.client, &self.endpoint, query, Some(variables))
                            .await
                            .map_err(|e| {
                                tracing::error!("Blink: Could not create USD invoice: {e}");
                                Self::Err::Anyhow(anyhow!("Could not create invoice: {e}"))
                            })?;

                    (data, "lnUsdInvoiceCreateOnBehalfOfRecipient")
                } else {
                    // SAT: amount is in sats, use BTC mutation
                    let query = r#"
                        mutation LnInvoiceCreateOnBehalfOfRecipient($input: LnInvoiceCreateOnBehalfOfRecipientInput!) {
                            lnInvoiceCreateOnBehalfOfRecipient(input: $input) {
                                invoice {
                                    paymentRequest
                                    paymentHash
                                    satoshis
                                }
                                errors {
                                    message
                                }
                            }
                        }
                    "#;

                    let variables = serde_json::json!({
                        "input": {
                            "recipientWalletId": wallet_id,
                            "amount": amount,
                            "memo": description,
                        }
                    });

                    let data =
                        graphql_request(&self.client, &self.endpoint, query, Some(variables))
                            .await
                            .map_err(|e| {
                                tracing::error!("Blink: Could not create invoice: {e}");
                                Self::Err::Anyhow(anyhow!("Could not create invoice: {e}"))
                            })?;

                    (data, "lnInvoiceCreateOnBehalfOfRecipient")
                };

                let create_result = data
                    .get(mutation_name)
                    .ok_or_else(|| Self::Err::Anyhow(anyhow!("No {mutation_name} in response")))?;

                // Check for errors
                if let Some(errors) = create_result.get("errors").and_then(|e| e.as_array()) {
                    if !errors.is_empty() {
                        let msg = errors
                            .iter()
                            .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                            .collect::<Vec<_>>()
                            .join("; ");
                        return Err(Self::Err::Anyhow(anyhow!("Invoice creation failed: {msg}")));
                    }
                }

                let invoice_data = create_result
                    .get("invoice")
                    .ok_or_else(|| Self::Err::Anyhow(anyhow!("No invoice in response")))?;

                let payment_request = invoice_data
                    .get("paymentRequest")
                    .and_then(|p| p.as_str())
                    .ok_or_else(|| Self::Err::Anyhow(anyhow!("No paymentRequest in invoice")))?;

                let request: Bolt11Invoice = payment_request.parse()?;
                let expiry = request.expires_at().map(|t| t.as_secs());

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::PaymentHash(
                        *request.payment_hash().as_ref(),
                    ),
                    request: request.to_string(),
                    expiry,
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Bolt12(_) => {
                Err(Self::Err::Anyhow(anyhow!("BOLT12 not supported by Blink")))
            }
            IncomingPaymentOptions::Custom(_) => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let payment_hash = payment_identifier.to_string();

        let query = r#"
            query lnInvoicePaymentStatusByHash($input: LnInvoicePaymentStatusByHashInput!) {
                lnInvoicePaymentStatusByHash(input: $input) {
                    status
                }
            }
        "#;

        let variables = serde_json::json!({
            "input": {
                "paymentHash": payment_hash,
            }
        });

        let data = graphql_request(&self.client, &self.endpoint, query, Some(variables))
            .await
            .map_err(|e| {
                tracing::error!("Blink: Could not check invoice status: {e}");
                Self::Err::Anyhow(anyhow!("Could not check invoice status: {e}"))
            })?;

        let status_result = data.get("lnInvoicePaymentStatusByHash").ok_or_else(|| {
            Self::Err::Anyhow(anyhow!("No lnInvoicePaymentStatusByHash in response"))
        })?;

        let status = status_result
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("PENDING");

        if status == "PAID" {
            // Get amount from transaction lookup using the instance's unit
            let amount = self
                .get_received_amount(&payment_hash)
                .await
                .unwrap_or(Amount::new(0, self.unit.clone()));

            Ok(vec![WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
                payment_amount: amount,
                payment_id: payment_hash,
            }])
        } else {
            Ok(vec![])
        }
    }

    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let payment_hash = payment_identifier.to_string();

        let query = r#"
            query TransactionsByPaymentHash($paymentHash: PaymentHash!, $walletId: WalletId!) {
                me {
                    defaultAccount {
                        walletById(walletId: $walletId) {
                            transactionsByPaymentHash(paymentHash: $paymentHash) {
                                status
                                direction
                                settlementAmount
                                settlementFee
                                settlementCurrency
                            }
                        }
                    }
                }
            }
        "#;

        // Use the wallet matching our configured unit
        let wallet_id = self.wallet_id_for_unit(&self.unit)?;

        let variables = serde_json::json!({
            "paymentHash": payment_hash,
            "walletId": wallet_id,
        });

        let data = graphql_request(&self.client, &self.endpoint, query, Some(variables))
            .await
            .map_err(|e| {
                tracing::error!("Blink: Could not check outgoing payment: {e}");
                Self::Err::Anyhow(anyhow!("Could not check outgoing payment: {e}"))
            })?;

        let transactions = data
            .get("me")
            .and_then(|me| me.get("defaultAccount"))
            .and_then(|acc| acc.get("walletById"))
            .and_then(|w| w.get("transactionsByPaymentHash"))
            .and_then(|t| t.as_array());

        let (status, total_spent) = match transactions {
            Some(txs) => self.parse_outgoing_transactions(txs),
            None => (MeltQuoteState::Unknown, Amount::new(0, self.unit.clone())),
        };

        Ok(MakePaymentResponse {
            payment_lookup_id: payment_identifier.clone(),
            payment_proof: Some(payment_hash),
            status,
            total_spent,
        })
    }
}

impl Blink {
    /// Probe fee for an invoice using Blink's fee probe mutation.
    /// Uses `lnUsdInvoiceFeeProbe` for USD and `lnInvoiceFeeProbe` for BTC.
    async fn probe_fee(
        &self,
        wallet_id: &str,
        invoice: &str,
        unit: &CurrencyUnit,
    ) -> Result<u64, Error> {
        let (query, response_key) = if is_usd_unit(unit) {
            (
                r#"
                mutation lnUsdInvoiceFeeProbe($input: LnUsdInvoiceFeeProbeInput!) {
                    lnUsdInvoiceFeeProbe(input: $input) {
                        amount
                        errors {
                            message
                        }
                    }
                }
                "#,
                "lnUsdInvoiceFeeProbe",
            )
        } else {
            (
                r#"
                mutation lnInvoiceFeeProbe($input: LnInvoiceFeeProbeInput!) {
                    lnInvoiceFeeProbe(input: $input) {
                        amount
                        errors {
                            message
                        }
                    }
                }
                "#,
                "lnInvoiceFeeProbe",
            )
        };

        let variables = serde_json::json!({
            "input": {
                "walletId": wallet_id,
                "paymentRequest": invoice,
            }
        });

        let data = graphql_request_with_timeout(
            &self.client,
            &self.endpoint,
            query,
            Some(variables),
            PROBE_FEE_TIMEOUT_SECS,
        )
        .await?;

        let probe_result = data
            .get(response_key)
            .ok_or_else(|| Error::GraphQL(format!("No {response_key} in response")))?;

        // Check for errors
        if let Some(errors) = probe_result.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                let msg = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(Error::GraphQL(format!("Fee probe failed: {msg}")));
            }
        }

        // Amount is in sats, convert to msat
        let fee_sats = probe_result
            .get("amount")
            .and_then(|a| a.as_u64())
            .unwrap_or(0);

        Ok(fee_sats * MSAT_IN_SAT)
    }

    /// Get the total amount spent on an outgoing payment (amount + fee).
    /// Returns amount in the wallet's native unit (msat for BTC, cents for USD).
    async fn get_transaction_total_spent(
        &self,
        payment_hash: &str,
        unit: &CurrencyUnit,
    ) -> Result<Amount<CurrencyUnit>, Error> {
        let query = r#"
            query TransactionsByPaymentHash($paymentHash: PaymentHash!, $walletId: WalletId!) {
                me {
                    defaultAccount {
                        walletById(walletId: $walletId) {
                            transactionsByPaymentHash(paymentHash: $paymentHash) {
                                settlementAmount
                                settlementFee
                                direction
                            }
                        }
                    }
                }
            }
        "#;

        let wallet_id = self.wallet_id_for_unit(unit)?;
        let variables = serde_json::json!({
            "paymentHash": payment_hash,
            "walletId": wallet_id,
        });

        let data = graphql_request(&self.client, &self.endpoint, query, Some(variables)).await?;

        let transactions = data
            .get("me")
            .and_then(|me| me.get("defaultAccount"))
            .and_then(|acc| acc.get("walletById"))
            .and_then(|w| w.get("transactionsByPaymentHash"))
            .and_then(|t| t.as_array())
            .ok_or(Error::UnknownInvoice)?;

        for tx in transactions {
            let direction = tx
                .get("direction")
                .and_then(|d| d.as_str())
                .unwrap_or_default();

            if direction == "SEND" {
                let amount = tx
                    .get("settlementAmount")
                    .and_then(|a| a.as_i64())
                    .unwrap_or(0)
                    .unsigned_abs();
                let fee = tx
                    .get("settlementFee")
                    .and_then(|f| f.as_i64())
                    .unwrap_or(0)
                    .unsigned_abs();

                // For USD wallet, settlementAmount already includes the fee.
                // For BTC wallet, settlementAmount is the net amount and fee is separate.
                let total = if is_usd_unit(unit) {
                    amount
                } else {
                    amount.checked_add(fee).ok_or(Error::AmountOverflow)?
                };

                return Ok(self.settlement_to_amount(total, unit));
            }
        }

        Err(Error::UnknownInvoice)
    }

    /// Get the received amount for an incoming payment.
    /// Uses the instance's configured unit to determine which wallet to query.
    async fn get_received_amount(&self, payment_hash: &str) -> Result<Amount<CurrencyUnit>, Error> {
        let query = r#"
            query TransactionsByPaymentHash($paymentHash: PaymentHash!, $walletId: WalletId!) {
                me {
                    defaultAccount {
                        walletById(walletId: $walletId) {
                            transactionsByPaymentHash(paymentHash: $paymentHash) {
                                settlementAmount
                                direction
                            }
                        }
                    }
                }
            }
        "#;

        // Use the wallet matching our configured unit
        let wallet_id = self.wallet_id_for_unit(&self.unit)?;

        let variables = serde_json::json!({
            "paymentHash": payment_hash,
            "walletId": wallet_id,
        });

        let data = graphql_request(&self.client, &self.endpoint, query, Some(variables)).await?;

        let transactions = data
            .get("me")
            .and_then(|me| me.get("defaultAccount"))
            .and_then(|acc| acc.get("walletById"))
            .and_then(|w| w.get("transactionsByPaymentHash"))
            .and_then(|t| t.as_array())
            .ok_or(Error::UnknownInvoice)?;

        for tx in transactions {
            let direction = tx
                .get("direction")
                .and_then(|d| d.as_str())
                .unwrap_or_default();

            if direction == "RECEIVE" {
                let amount = tx
                    .get("settlementAmount")
                    .and_then(|a| a.as_i64())
                    .unwrap_or(0)
                    .unsigned_abs();

                return Ok(self.settlement_to_amount(amount, &self.unit));
            }
        }

        Err(Error::UnknownInvoice)
    }

    /// Parse outgoing transactions to determine status and total spent.
    /// Handles the edge case where both SEND and RECEIVE transactions exist
    /// (internal transfer that should be treated as FAILED).
    fn parse_outgoing_transactions(&self, txs: &[Value]) -> (MeltQuoteState, Amount<CurrencyUnit>) {
        let mut has_send = false;
        let mut has_receive = false;
        let mut send_status = MeltQuoteState::Unknown;
        let mut total_spent = Amount::new(0, self.unit.clone());

        for tx in txs {
            let direction = tx
                .get("direction")
                .and_then(|d| d.as_str())
                .unwrap_or_default();

            match direction {
                "SEND" => {
                    has_send = true;
                    let status = tx
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("PENDING");
                    send_status = blink_to_melt_status(status);

                    let amount = tx
                        .get("settlementAmount")
                        .and_then(|a| a.as_i64())
                        .unwrap_or(0)
                        .unsigned_abs();

                    // For USD wallet, settlementAmount already includes the fee.
                    // For BTC wallet, we need to add settlementFee separately.
                    let total = if self.unit == CurrencyUnit::Usd {
                        amount
                    } else {
                        let fee = tx
                            .get("settlementFee")
                            .and_then(|f| f.as_i64())
                            .unwrap_or(0)
                            .unsigned_abs();
                        amount + fee
                    };

                    total_spent = self.settlement_to_amount(total, &self.unit);
                }
                "RECEIVE" => {
                    has_receive = true;
                }
                _ => {}
            }
        }

        // Edge case: if both SEND and RECEIVE exist, it was an internal
        // transfer that failed to reach the external destination
        if has_send && has_receive {
            return (MeltQuoteState::Unpaid, Amount::new(0, self.unit.clone()));
        }

        if has_send {
            (send_status, total_spent)
        } else {
            (MeltQuoteState::Unknown, Amount::new(0, self.unit.clone()))
        }
    }
}
