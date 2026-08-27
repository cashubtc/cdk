//! CDK lightning backend for CLN

#![doc = include_str!("../README.md")]

use std::cmp::max;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::hashes::sha256;
use cdk_common::amount::Amount;
use cdk_common::common::FeeReserve;
use cdk_common::database::DynKVStore;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11IncomingPaymentOptions, Bolt12IncomingPaymentOptions,
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, SettingsResponse,
    WaitPaymentResponse,
};
use cdk_common::util::{hex, unix_time};
use cdk_common::{Bolt11Invoice, QuoteId};
use cln_rpc::model::requests::{
    DecodeRequest, FetchinvoiceRequest, InvoiceRequest, ListinvoicesRequest, ListpaysRequest,
    OfferRequest, WaitanyinvoiceRequest, XpayRequest,
};
use cln_rpc::model::responses::{
    DecodeResponse, InvoiceResponse, ListinvoicesInvoices, ListinvoicesInvoicesStatus,
    ListpaysPays, ListpaysPaysStatus, WaitanyinvoiceResponse, WaitanyinvoiceStatus,
};
use cln_rpc::primitives::{Amount as CLN_Amount, AmountOrAny, Sha256};
use cln_rpc::ClnRpc;
use error::Error;
use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uuid::Uuid;

pub mod error;

// KV Store constants for CLN
const CLN_KV_PRIMARY_NAMESPACE: &str = "cdk_cln_lightning_backend";
const CLN_KV_SECONDARY_NAMESPACE: &str = "payment_indices";
const CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE: &str = "bolt12_outgoing_payments";
const CLN_KV_BOLT12_CLEANUP_MARKER: &[u8] = b"cleanup-in-progress";
const LAST_PAY_INDEX_KV_KEY: &str = "last_pay_index";
const XPAY_DESTINATION_PERMANENT_FAILURE_ERROR_CODE: i32 = 203;
const XPAY_ROUTE_NOT_FOUND_ERROR_CODE: i32 = 205;
const XPAY_ROUTE_TOO_EXPENSIVE_ERROR_CODE: i32 = 206;
const XPAY_INVOICE_EXPIRED_ERROR_CODE: i32 = 207;

/// CLN mint backend
#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    fee_reserve: FeeReserve,
    expose_private_channels: bool,
    bolt12: bool,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    kv_store: DynKVStore,
}

impl std::fmt::Debug for Cln {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cln")
            .field("rpc_socket", &self.rpc_socket)
            .field("fee_reserve", &self.fee_reserve)
            .finish_non_exhaustive()
    }
}

impl Cln {
    /// Create new [`Cln`]
    pub async fn new(
        rpc_socket: PathBuf,
        fee_reserve: FeeReserve,
        expose_private_channels: bool,
        kv_store: DynKVStore,
    ) -> Result<Self, Error> {
        Ok(Self {
            rpc_socket,
            fee_reserve,
            expose_private_channels,
            bolt12: true,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            kv_store,
        })
    }

    /// Enable or disable BOLT12 support.
    pub fn with_bolt12(mut self, bolt12: bool) -> Self {
        self.bolt12 = bolt12;
        self
    }
}

#[async_trait]
impl MintPayment for Cln {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        use std::collections::HashMap;
        Ok(SettingsResponse {
            unit: CurrencyUnit::Msat.to_string(),
            bolt11: Some(payment::Bolt11Settings {
                mpp: true,
                amountless: true,
                invoice_description: true,
            }),
            bolt12: self
                .bolt12
                .then_some(payment::Bolt12Settings { amountless: true }),
            onchain: None,
            custom: HashMap::new(),
        })
    }

    /// Is payment event stream active
    fn is_payment_event_stream_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel payment event stream
    fn cancel_payment_event_stream(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    #[instrument(skip_all)]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        tracing::info!(
            "CLN: Starting wait_any_incoming_payment with socket: {:?}",
            self.rpc_socket
        );

        let last_pay_index = self.get_last_pay_index().await?.inspect(|&idx| {
            tracing::info!("CLN: Found last payment index: {}", idx);
        });

        tracing::debug!("CLN: Connecting to CLN node...");
        let cln_client = match cln_rpc::ClnRpc::new(&self.rpc_socket).await {
            Ok(client) => {
                tracing::debug!("CLN: Successfully connected to CLN node");
                client
            }
            Err(err) => {
                tracing::error!("CLN: Failed to connect to CLN node: {}", err);
                return Err(Error::from(err).into());
            }
        };

        tracing::debug!("CLN: Creating stream processing pipeline");
        let kv_store = self.kv_store.clone();
        let stream = futures::stream::unfold(
            (
                cln_client,
                last_pay_index,
                self.wait_invoice_cancel_token.clone(),
                Arc::clone(&self.wait_invoice_is_active),
                kv_store,
            ),
            |(mut cln_client, mut last_pay_idx, cancel_token, is_active, kv_store)| async move {
                // Set the stream as active
                is_active.store(true, Ordering::SeqCst);
                tracing::debug!("CLN: Stream is now active, waiting for invoice events with lastpay_index: {:?}", last_pay_idx);

                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Set the stream as inactive
                            is_active.store(false, Ordering::SeqCst);
                            tracing::info!("CLN: Invoice stream cancelled");
                            // End the stream
                            return None;
                        }
                        result = cln_client.call(cln_rpc::Request::WaitAnyInvoice(WaitanyinvoiceRequest {
                            timeout: None,
                            lastpay_index: last_pay_idx,
                        })) => {
                            tracing::debug!("CLN: Received response from WaitAnyInvoice call");
                            match result {
                                Ok(invoice) => {
                                    tracing::debug!("CLN: Successfully received invoice data");
                                        // Try to convert the invoice to WaitanyinvoiceResponse
                            let wait_any_response_result: Result<WaitanyinvoiceResponse, _> =
                                invoice.try_into();

                            let wait_any_response = match wait_any_response_result {
                                Ok(response) => {
                                    tracing::debug!("CLN: Parsed WaitAnyInvoice response successfully");
                                    response
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "CLN: Failed to parse WaitAnyInvoice response: {:?}",
                                        e
                                    );
                                    // Continue to the next iteration without panicking
                                    continue;
                                }
                            };

                            // Check the status of the invoice
                            // We only want to yield invoices that have been paid
                            match wait_any_response.status {
                                WaitanyinvoiceStatus::PAID => {
                                    tracing::info!("CLN: Invoice with payment index {} is PAID",
                                                 wait_any_response.pay_index.unwrap_or_default());
                                }
                                WaitanyinvoiceStatus::EXPIRED => {
                                    tracing::debug!("CLN: Invoice with payment index {} is EXPIRED, skipping",
                                                  wait_any_response.pay_index.unwrap_or_default());
                                    continue;
                                }
                            }

                            last_pay_idx = wait_any_response.pay_index;
                            tracing::debug!("CLN: Updated last_pay_idx to {:?}", last_pay_idx);


                            // Store the updated pay index in KV store for persistence
                            if let Some(pay_index) = last_pay_idx {
                                let index_str = pay_index.to_string();
                                if let Ok(mut tx) = kv_store.begin_transaction().await {
                                    if let Err(e) = tx.kv_write(CLN_KV_PRIMARY_NAMESPACE, CLN_KV_SECONDARY_NAMESPACE, LAST_PAY_INDEX_KV_KEY, index_str.as_bytes()).await {
                                        tracing::warn!("CLN: Failed to write last pay index {} to KV store: {}", pay_index, e);
                                    } else if let Err(e) = tx.commit().await {
                                        tracing::warn!("CLN: Failed to commit last pay index {} to KV store: {}", pay_index, e);
                                    } else {
                                        tracing::debug!("CLN: Stored last pay index {} in KV store", pay_index);
                                    }
                                } else {
                                    tracing::warn!("CLN: Failed to begin KV transaction for storing pay index {}", pay_index);
                                }
                            }

                            let payment_hash = wait_any_response.payment_hash;
                            tracing::debug!("CLN: Payment hash: {}", payment_hash);

                            let amount_msats = match wait_any_response.amount_received_msat {
                                Some(amt) => {
                                    tracing::info!("CLN: Received payment of {} msats for {}",
                                                 amt.msat(), payment_hash);
                                    amt
                                }
                                None => {
                                    tracing::error!("CLN: No amount in paid invoice, this should not happen");
                                    continue;
                                }
                            };

                            let payment_hash =
                                sha256::Hash::from_bytes_ref(payment_hash.as_ref());

                            let request_lookup_id = match wait_any_response.bolt12 {
                                // If it is a bolt12 payment we need to get the offer_id as this is what we use as the request look up.
                                // Since this is not returned in the wait any response,
                                // we need to do a second query for it.
                                Some(bolt12) => {
                                    tracing::info!("CLN: Processing BOLT12 payment, bolt12 value: {}", bolt12);
                                    match fetch_invoice_by_payment_hash(
                                        &mut cln_client,
                                        payment_hash,
                                    )
                                    .await
                                    {
                                        Ok(Some(invoice)) => {
                                            if let Some(local_offer_id) = invoice.local_offer_id {
                                                tracing::info!("CLN: Received bolt12 payment of {} msats for offer {}",
                                                             amount_msats.msat(), local_offer_id);
                                                PaymentIdentifier::OfferId(local_offer_id.to_string())
                                            } else {
                                                tracing::warn!("CLN: BOLT12 invoice has no local_offer_id, skipping");
                                                continue;
                                            }
                                        }
                                        Ok(None) => {
                                            tracing::warn!("CLN: Failed to find invoice by payment hash, skipping");
                                            continue;
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "CLN: Error fetching invoice by payment hash: {e}"
                                            );
                                            continue;
                                        }
                                    }
                                }
                                None => {
                                 tracing::info!("CLN: Processing BOLT11 payment with hash {}", payment_hash);
                                 PaymentIdentifier::PaymentHash(*payment_hash.as_ref())
                                },
                            };

                            let response = WaitPaymentResponse {
                                payment_identifier: request_lookup_id,
                                payment_amount: Amount::new(amount_msats.msat(), CurrencyUnit::Msat),
                                payment_id: payment_hash.to_string(),
                            };
                            tracing::info!("CLN: Created WaitPaymentResponse with amount {} msats", amount_msats.msat());
                            let event = Event::PaymentReceived(response);

                            break Some((event, (cln_client, last_pay_idx, cancel_token, is_active, kv_store)));
                                }
                                Err(e) => {
                                    tracing::warn!("CLN: Error fetching invoice: {e}");
                                    tokio::time::sleep(Duration::from_secs(1)).await;
                                    continue;
                                }
                            }
                        }
                    }
                }
            },
        )
        .boxed();

        tracing::info!("CLN: Successfully initialized invoice stream");
        Ok(stream)
    }

    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            cdk_common::payment::OutgoingPaymentOptions::Custom(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
            }
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                // If we have specific amount options, use those
                let amount_msat: Amount = if let Some(melt_options) = bolt11_options.melt_options {
                    match melt_options {
                        MeltOptions::Amountless { amountless } => {
                            let amount_msat = amountless.amount_msat;

                            if let Some(invoice_amount) =
                                bolt11_options.bolt11.amount_milli_satoshis()
                            {
                                if invoice_amount != u64::from(amount_msat) {
                                    return Err(payment::Error::AmountMismatch);
                                }
                            }
                            amount_msat
                        }
                        MeltOptions::Mpp { mpp } => mpp.amount,
                    }
                } else {
                    // Fall back to invoice amount
                    bolt11_options
                        .bolt11
                        .amount_milli_satoshis()
                        .ok_or(Error::UnknownInvoiceAmount)?
                        .into()
                };
                // Convert to target unit
                let amount =
                    Amount::new(amount_msat.into(), CurrencyUnit::Msat).convert_to(unit)?;

                // Calculate fee
                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * amount.value() as f32) as u64;
                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();
                let fee = max(relative_fee_reserve, absolute_fee_reserve);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    )),
                    amount,
                    fee: Amount::new(fee, unit.clone()),
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
                })
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let quote_id = bolt12_options.quote_id.clone();
                Self::bolt12_quote_payment_hash_key(&quote_id)?;
                let offer = bolt12_options.offer;

                let amount_msat: u64 = if let Some(amount) = bolt12_options.melt_options {
                    amount.amount_msat().into()
                } else {
                    // Fall back to offer amount
                    let decode_response = self.decode_string(offer.to_string()).await?;

                    decode_response
                        .offer_amount_msat
                        .ok_or(Error::UnknownInvoiceAmount)?
                        .msat()
                };

                // Convert to target unit
                let amount = Amount::new(amount_msat, CurrencyUnit::Msat).convert_to(unit)?;

                // Calculate fee
                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * amount.value() as f32) as u64;
                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();
                let fee = max(relative_fee_reserve, absolute_fee_reserve);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::QuoteId(quote_id)),
                    amount,
                    fee: Amount::new(fee, unit.clone()),
                    state: MeltQuoteState::Unpaid,
                    extra_json: None,
                    estimated_blocks: None,
                    fee_options: None,
                })
            }
            OutgoingPaymentOptions::Onchain(_) => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let max_fee_msat: Option<u64>;
        let mut partial_amount: Option<u64> = None;
        let mut amount_msat: Option<u64> = None;
        let payment_lookup_id: PaymentIdentifier;
        let mut bolt12_binding = None;

        let mut cln_client = self.cln_client().await?;

        let invoice = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let payment_identifier =
                    PaymentIdentifier::PaymentHash(*bolt11_options.bolt11.payment_hash().as_ref());

                self.check_outgoing_unpaided(&payment_identifier).await?;
                payment_lookup_id = payment_identifier;

                if let Some(melt_options) = bolt11_options.melt_options {
                    match melt_options {
                        MeltOptions::Mpp { mpp } => partial_amount = Some(mpp.amount.into()),
                        MeltOptions::Amountless { amountless } => {
                            amount_msat = Some(amountless.amount_msat.into());
                        }
                    }
                }

                max_fee_msat = bolt11_options
                    .max_fee_amount
                    .as_ref()
                    .map(|a| a.to_msat())
                    .transpose()?;

                bolt11_options.bolt11.to_string()
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let offer = &bolt12_options.offer;
                let quote_id = bolt12_options.quote_id.clone();
                let quote_payment_identifier = PaymentIdentifier::QuoteId(quote_id.clone());

                max_fee_msat = bolt12_options
                    .max_fee_amount
                    .as_ref()
                    .map(|a| a.to_msat())
                    .transpose()?;

                self.check_outgoing_unpaided(&quote_payment_identifier)
                    .await?;

                let amount_msat: u64 = if let Some(amount) = bolt12_options.melt_options {
                    amount.amount_msat().into()
                } else {
                    // Fall back to offer amount
                    let decode_response = self.decode_string(offer.to_string()).await?;

                    decode_response
                        .offer_amount_msat
                        .ok_or(Error::UnknownInvoiceAmount)?
                        .msat()
                };

                // Fetch invoice from offer

                let cln_response = cln_client
                    .call_typed(&FetchinvoiceRequest {
                        amount_msat: Some(CLN_Amount::from_msat(amount_msat)),
                        payer_metadata: None,
                        payer_note: None,
                        quantity: None,
                        recurrence_counter: None,
                        recurrence_label: None,
                        recurrence_start: None,
                        timeout: None,
                        offer: offer.to_string(),
                        bip353: None,
                    })
                    .await
                    .map_err(|err| {
                        tracing::error!("Could not fetch invoice for offer: {:?}", err);
                        Error::ClnRpc(err)
                    })?;

                if cln_response.invoice.is_empty() {
                    return Err(Error::UnknownInvoice.into());
                }

                let decode_response = self.decode_string(cln_response.invoice.clone()).await?;

                let payment_hash = Self::parse_payment_hash(
                    decode_response
                        .invoice_payment_hash
                        .ok_or(Error::UnknownInvoice)?,
                )?;

                let payment_identifier = PaymentIdentifier::Bolt12PaymentHash(payment_hash);

                self.check_outgoing_unpaided(&payment_identifier).await?;
                self.write_bolt12_quote_payment_hash(&quote_id, &payment_hash)
                    .await?;
                payment_lookup_id = quote_payment_identifier;
                bolt12_binding = Some((quote_id, payment_hash));

                cln_response.invoice
            }
            _ => {
                return Err(payment::Error::UnsupportedPaymentOption);
            }
        };
        if invoice.is_empty() {
            return Err(Error::UnknownInvoice.into());
        }

        tracing::debug!("Attempting payment with max fee: {:?}", max_fee_msat);

        let cln_response = cln_client
            .call_typed(&XpayRequest {
                invstring: invoice,
                amount_msat: amount_msat.map(CLN_Amount::from_msat),
                dev_use_shadow: None,
                label: None,
                localinvreqid: None,
                maxdelay: None,
                maxfee: max_fee_msat.map(CLN_Amount::from_msat),
                layers: None,
                payer_note: None,
                retry_for: None,
                partial_msat: partial_amount.map(CLN_Amount::from_msat),
            })
            .await;

        let response = match cln_response {
            Ok(pay_response) => MakePaymentResponse {
                payment_lookup_id,
                payment_proof: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                status: MeltQuoteState::Paid,
                total_spent: Amount::new(pay_response.amount_sent_msat.msat(), CurrencyUnit::Msat)
                    .convert_to(unit)?,
            },
            Err(err) => {
                tracing::warn!("xpay returned an error: {}", err);

                // Reconcile BOLT12 payments by hash so a failed observation does
                // not release the quote binding before the xpay error itself has
                // been classified.
                let reconciliation_payment_identifier = match &bolt12_binding {
                    Some((_, payment_hash)) => PaymentIdentifier::Bolt12PaymentHash(*payment_hash),
                    None => payment_lookup_id.clone(),
                };

                let (observed_state, response) = match self
                    .check_outgoing_payment(&reconciliation_payment_identifier)
                    .await
                {
                    Ok(observed_response)
                        if matches!(
                            observed_response.status,
                            MeltQuoteState::Paid | MeltQuoteState::Pending
                        ) =>
                    {
                        (
                            Some(observed_response.status),
                            MakePaymentResponse {
                                payment_lookup_id: payment_lookup_id.clone(),
                                ..observed_response
                            },
                        )
                    }
                    Ok(observed_response) => {
                        let observed_state = Some(observed_response.status);
                        (
                            observed_state,
                            outgoing_payment_response_with_status(
                                &payment_lookup_id,
                                xpay_error_state(&err, observed_state),
                            ),
                        )
                    }
                    Err(status_err) => {
                        tracing::warn!(
                            "Could not reconcile xpay error with listpays: {}",
                            status_err
                        );
                        (
                            None,
                            outgoing_payment_response_with_status(
                                &payment_lookup_id,
                                xpay_error_state(&err, None),
                            ),
                        )
                    }
                };

                if let Some((quote_id, payment_hash)) = &bolt12_binding {
                    match should_retain_bolt12_binding_after_xpay_error(&err, observed_state) {
                        true => {
                            tracing::warn!(
                                quote_id = %quote_id,
                                "CLN xpay failed without a definitive terminal payment state; \
                                 retaining the BOLT12 quote binding because the payment may have \
                                 been dispatched"
                            );
                        }
                        false => {
                            self.cleanup_bolt12_quote_payment_hash(quote_id, payment_hash)
                                .await;
                        }
                    }
                }

                response
            }
        };

        Ok(response)
    }

    #[instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            cdk_common::payment::IncomingPaymentOptions::Custom(_) => {
                Err(cdk_common::payment::Error::UnsupportedPaymentOption)
            }
            IncomingPaymentOptions::Bolt11(Bolt11IncomingPaymentOptions {
                description,
                amount,
                unix_expiry,
            }) => {
                let time_now = unix_time();

                let mut cln_client = self.cln_client().await?;

                let label = Uuid::new_v4().to_string();

                let amount_converted = amount.convert_to(&CurrencyUnit::Msat)?;
                let amount_msat =
                    AmountOrAny::Amount(CLN_Amount::from_msat(amount_converted.value()));

                let expiry = unix_expiry
                    .map(|t| t.checked_sub(time_now).ok_or(payment::Error::InvalidExpiry))
                    .transpose()?;

                let request = InvoiceRequest {
                    amount_msat,
                    description: description.unwrap_or_default(),
                    label: label.clone(),
                    expiry,
                    fallbacks: None,
                    preimage: None,
                    cltv: None,
                    deschashonly: None,
                    exposeprivatechannels: None,
                };

                // cln-rpc types exposeprivatechannels as Option<Vec<ShortChannelId>>
                // which cannot represent boolean true. Use call_raw to bypass this
                // limitation when expose_private_channels is enabled.
                let invoice_response: InvoiceResponse = if self.expose_private_channels {
                    let mut params = serde_json::to_value(&request).map_err(Error::from)?;
                    params["exposeprivatechannels"] = serde_json::Value::Bool(true);
                    cln_client
                        .call_raw("invoice", &params)
                        .await
                        .map_err(Error::from)?
                } else {
                    cln_client.call_typed(&request).await.map_err(Error::from)?
                };

                let request = Bolt11Invoice::from_str(&invoice_response.bolt11)?;
                let expiry = request.expires_at().map(|t| t.as_secs());
                let payment_hash = request.payment_hash();

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::PaymentHash(*payment_hash.as_ref()),
                    request: request.to_string(),
                    expiry,
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Bolt12(bolt12_options) => {
                let Bolt12IncomingPaymentOptions {
                    description,
                    amount,
                    unix_expiry,
                } = *bolt12_options;
                let mut cln_client = self.cln_client().await?;

                let label = Uuid::new_v4().to_string();

                // Match like this until we change to option
                let amount = match amount {
                    Some(amount) => {
                        let amount = amount.convert_to(&CurrencyUnit::Msat)?;

                        amount.value().to_string()
                    }
                    None => "any".to_string(),
                };

                // It seems that the only way to force cln to create a unique offer
                // is to encode some random data in the offer
                let issuer = Uuid::new_v4().to_string();

                let offer_response = cln_client
                    .call_typed(&OfferRequest {
                        amount,
                        absolute_expiry: unix_expiry,
                        description: Some(description.unwrap_or_default()),
                        issuer: Some(issuer.to_string()),
                        label: Some(label.to_string()),
                        single_use: None,
                        quantity_max: None,
                        recurrence: None,
                        recurrence_base: None,
                        recurrence_limit: None,
                        recurrence_paywindow: None,
                        optional_recurrence: None,
                        proportional_amount: None,
                        fronting_nodes: None,
                    })
                    .await
                    .map_err(Error::from)?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::OfferId(
                        offer_response.offer_id.to_string(),
                    ),
                    request: offer_response.bolt12,
                    expiry: unix_expiry,
                    extra_json: None,
                })
            }
            IncomingPaymentOptions::Onchain(_) => Err(payment::Error::UnsupportedPaymentOption),
        }
    }

    #[instrument(skip(self))]
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let mut cln_client = self.cln_client().await?;

        let listinvoices_response = match payment_identifier {
            PaymentIdentifier::Label(label) => {
                // Query by label
                cln_client
                    .call_typed(&ListinvoicesRequest {
                        payment_hash: None,
                        label: Some(label.to_string()),
                        invstring: None,
                        offer_id: None,
                        index: None,
                        limit: None,
                        start: None,
                    })
                    .await
                    .map_err(Error::from)?
            }
            PaymentIdentifier::OfferId(offer_id) => {
                // Query by offer_id
                cln_client
                    .call_typed(&ListinvoicesRequest {
                        payment_hash: None,
                        label: None,
                        invstring: None,
                        offer_id: Some(offer_id.to_string()),
                        index: None,
                        limit: None,
                        start: None,
                    })
                    .await
                    .map_err(Error::from)?
            }
            PaymentIdentifier::PaymentHash(payment_hash) => {
                // Query by payment_hash
                cln_client
                    .call_typed(&ListinvoicesRequest {
                        payment_hash: Some(hex::encode(payment_hash)),
                        label: None,
                        invstring: None,
                        offer_id: None,
                        index: None,
                        limit: None,
                        start: None,
                    })
                    .await
                    .map_err(Error::from)?
            }
            _ => {
                tracing::error!("Unsupported payment id for CLN");
                return Err(payment::Error::UnknownPaymentState);
            }
        };

        Ok(listinvoices_response
            .invoices
            .iter()
            .filter(|p| p.status == ListinvoicesInvoicesStatus::PAID)
            .filter(|p| p.amount_msat.is_some()) // Filter out invoices without an amount
            .map(|p| WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
                payment_amount: Amount::new(
                    p.amount_msat
                        // Safe to expect since we filtered for Some
                        .expect("We have filter out those without amounts")
                        .msat(),
                    CurrencyUnit::Msat,
                ),
                payment_id: p.payment_hash.to_string(),
            })
            .collect())
    }

    #[instrument(skip(self))]
    /// Check the status of an outgoing payment.
    ///
    /// CLN never reports `Unpaid`/`Failed` for a dispatched payment: an
    /// `xpay` command may survive a client disconnect and keep retrying, so a
    /// not-paid observation is not an authoritative terminal outcome and is
    /// reported as `Unknown` to keep the mint fail-closed (see
    /// [`MintPayment::check_outgoing_payment`]). A failed payment's BOLT12
    /// quote binding is retained for the same reason — releasing it would
    /// turn a later poll's missing-binding answer into an authoritative
    /// `Unpaid`. The one exception is a BOLT12 quote with no recorded
    /// payment-hash binding: the binding is written before any payment is
    /// dispatched, so its absence proves the payment was never attempted and
    /// `Unpaid` is truthful.
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let (payment_hash, missing_payment_state) = match payment_identifier {
            PaymentIdentifier::PaymentHash(hash) | PaymentIdentifier::Bolt12PaymentHash(hash) => {
                (*hash, MeltQuoteState::Unknown)
            }
            PaymentIdentifier::QuoteId(quote_id) => match self
                .read_bolt12_quote_payment_hash(quote_id)
                .await
                .map_err(payment::Error::from)?
            {
                Bolt12QuotePaymentHashLookup::Found(payment_hash) => {
                    (payment_hash, MeltQuoteState::Unknown)
                }
                Bolt12QuotePaymentHashLookup::Missing => {
                    return Ok(outgoing_payment_response_with_status(
                        payment_identifier,
                        MeltQuoteState::Unpaid,
                    ));
                }
                Bolt12QuotePaymentHashLookup::Malformed => {
                    return Ok(outgoing_payment_response_with_status(
                        payment_identifier,
                        MeltQuoteState::Unknown,
                    ));
                }
            },
            _ => {
                tracing::error!("Unsupported identifier to check outgoing payment for cln.");
                return Err(payment::Error::UnknownPaymentState);
            }
        };

        let mut cln_client = self.cln_client().await?;

        let listpays_response = cln_client
            .call_typed(&ListpaysRequest {
                payment_hash: Some(*Sha256::from_bytes_ref(&payment_hash)),
                bolt11: None,
                status: None,
                start: None,
                index: None,
                limit: None,
            })
            .await
            .map_err(Error::from)?;

        match select_cln_payment(&listpays_response.pays) {
            Some(pays_response) => {
                // A negative CLN status is not an authoritative terminal
                // outcome for the mint; report it as Unknown so recovery
                // cannot refund proofs while the payment may still settle.
                // The BOLT12 quote binding is deliberately retained as well:
                // releasing it would let a later poll observe the missing
                // binding as authoritative `Unpaid` and compensate a payment
                // that may still settle. Only the synchronous make_payment
                // error path may release a binding, once it holds a
                // definitive terminal failure.
                let status = match cln_pays_status_to_mint_state(pays_response.status) {
                    MeltQuoteState::Unpaid | MeltQuoteState::Failed => MeltQuoteState::Unknown,
                    status => status,
                };

                Ok(MakePaymentResponse {
                    payment_lookup_id: payment_identifier.clone(),
                    payment_proof: pays_response.preimage.map(|p| hex::encode(p.to_vec())),
                    status,
                    total_spent: pays_response
                        .amount_sent_msat
                        .map_or(Amount::new(0, CurrencyUnit::Msat), |a| {
                            Amount::new(a.msat(), CurrencyUnit::Msat)
                        }),
                })
            }
            None => Ok(MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: None,
                status: missing_payment_state,
                total_spent: Amount::new(0, CurrencyUnit::Msat),
            }),
        }
    }
}

impl Cln {
    async fn cln_client(&self) -> Result<ClnRpc, Error> {
        Ok(cln_rpc::ClnRpc::new(&self.rpc_socket).await?)
    }

    fn bolt12_quote_payment_hash_key(quote_id: &QuoteId) -> Result<String, Error> {
        match quote_id {
            QuoteId::UUID(uuid) => Ok(uuid.to_string()),
            QuoteId::BASE64(_) => Err(Error::InvalidQuoteId),
        }
    }

    fn parse_payment_hash(payment_hash: String) -> Result<[u8; 32], Error> {
        hex::decode(payment_hash)
            .map_err(|e| Error::Bolt12(e.to_string()))?
            .try_into()
            .map_err(|_| Error::InvalidHash)
    }

    /// Binds the BOLT12 quote id to the fetched invoice's payment hash.
    ///
    /// The binding is write-once: the first dispatch for a quote claims it
    /// atomically, so a duplicate or concurrent dispatch is rejected before
    /// any funds move. Repeating the same payment hash is an idempotent
    /// no-op for recovery.
    async fn write_bolt12_quote_payment_hash(
        &self,
        quote_id: &QuoteId,
        payment_hash: &[u8; 32],
    ) -> Result<(), Error> {
        let key = Self::bolt12_quote_payment_hash_key(quote_id)?;
        let value = hex::encode(payment_hash);
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        let written = tx
            .kv_write_if_absent(
                CLN_KV_PRIMARY_NAMESPACE,
                CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
                &key,
                value.as_bytes(),
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        if written {
            tx.commit()
                .await
                .map_err(|e| Error::Database(e.to_string()))?;
            return Ok(());
        }

        let existing = tx
            .kv_read(
                CLN_KV_PRIMARY_NAMESPACE,
                CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
                &key,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?;
        tx.rollback()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        match existing {
            Some(existing) if existing.as_slice() == value.as_bytes() => Ok(()),
            _ => Err(Error::Bolt12QuoteBindingConflict {
                quote_id: quote_id.to_string(),
            }),
        }
    }

    /// Best-effort release of a binding whose exact payment attempt is known
    /// to have failed terminally.
    async fn cleanup_bolt12_quote_payment_hash(&self, quote_id: &QuoteId, payment_hash: &[u8; 32]) {
        match self
            .delete_bolt12_quote_payment_hash_if_equals(quote_id, payment_hash)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!(
                    quote_id = %quote_id,
                    "BOLT12 quote binding changed before failed-payment cleanup"
                );
            }
            Err(err) => {
                tracing::warn!(
                    quote_id = %quote_id,
                    "Could not release failed BOLT12 quote binding: {err}"
                );
            }
        }
    }

    /// Atomically removes the binding only if it still names `payment_hash`.
    async fn delete_bolt12_quote_payment_hash_if_equals(
        &self,
        quote_id: &QuoteId,
        payment_hash: &[u8; 32],
    ) -> Result<bool, Error> {
        let key = Self::bolt12_quote_payment_hash_key(quote_id)?;
        let expected = hex::encode(payment_hash);
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        let claimed = tx
            .kv_write_if_equals(
                CLN_KV_PRIMARY_NAMESPACE,
                CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
                &key,
                expected.as_bytes(),
                CLN_KV_BOLT12_CLEANUP_MARKER,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        if !claimed {
            tx.rollback()
                .await
                .map_err(|e| Error::Database(e.to_string()))?;
            return Ok(false);
        }

        tx.kv_remove(
            CLN_KV_PRIMARY_NAMESPACE,
            CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
            &key,
        )
        .await
        .map_err(|e| Error::Database(e.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| Error::Database(e.to_string()))?;

        Ok(true)
    }

    async fn read_bolt12_quote_payment_hash(
        &self,
        quote_id: &QuoteId,
    ) -> Result<Bolt12QuotePaymentHashLookup, Error> {
        let key = Self::bolt12_quote_payment_hash_key(quote_id)?;
        let Some(stored_hash) = self
            .kv_store
            .kv_read(
                CLN_KV_PRIMARY_NAMESPACE,
                CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
                &key,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        else {
            return Ok(Bolt12QuotePaymentHashLookup::Missing);
        };

        let payment_hash = match String::from_utf8(stored_hash) {
            Ok(payment_hash) => payment_hash,
            Err(err) => {
                tracing::warn!(
                    "CLN: invalid UTF-8 in BOLT12 payment hash mapping for quote {quote_id}: {err}"
                );
                return Ok(Bolt12QuotePaymentHashLookup::Malformed);
            }
        };

        match Self::parse_payment_hash(payment_hash) {
            Ok(payment_hash) => Ok(Bolt12QuotePaymentHashLookup::Found(payment_hash)),
            Err(err) => {
                tracing::warn!(
                    "CLN: invalid BOLT12 payment hash mapping for quote {quote_id}: {err}"
                );
                Ok(Bolt12QuotePaymentHashLookup::Malformed)
            }
        }
    }

    #[cfg(test)]
    async fn outgoing_payment_hash(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Option<[u8; 32]>, payment::Error> {
        match payment_identifier {
            PaymentIdentifier::PaymentHash(hash) | PaymentIdentifier::Bolt12PaymentHash(hash) => {
                Ok(Some(*hash))
            }
            PaymentIdentifier::QuoteId(quote_id) => {
                match self
                    .read_bolt12_quote_payment_hash(quote_id)
                    .await
                    .map_err(payment::Error::from)?
                {
                    Bolt12QuotePaymentHashLookup::Found(payment_hash) => Ok(Some(payment_hash)),
                    Bolt12QuotePaymentHashLookup::Missing
                    | Bolt12QuotePaymentHashLookup::Malformed => Ok(None),
                }
            }
            _ => {
                tracing::error!("Unsupported identifier to check outgoing payment for cln.");
                Err(payment::Error::UnknownPaymentState)
            }
        }
    }

    /// Get last pay index for cln
    async fn get_last_pay_index(&self) -> Result<Option<u64>, Error> {
        // First try to read from KV store
        if let Some(stored_index) = self
            .kv_store
            .kv_read(
                CLN_KV_PRIMARY_NAMESPACE,
                CLN_KV_SECONDARY_NAMESPACE,
                LAST_PAY_INDEX_KV_KEY,
            )
            .await
            .map_err(|e| Error::Database(e.to_string()))?
        {
            if let Ok(index_str) = std::str::from_utf8(&stored_index) {
                if let Ok(index) = index_str.parse::<u64>() {
                    tracing::debug!("CLN: Retrieved last pay index {} from KV store", index);
                    return Ok(Some(index));
                }
            }
        }

        // Fall back to querying CLN directly
        tracing::debug!("CLN: No stored last pay index found in KV store, querying CLN directly");
        let mut cln_client = self.cln_client().await?;
        let listinvoices_response = cln_client
            .call_typed(&ListinvoicesRequest {
                index: None,
                invstring: None,
                label: None,
                limit: None,
                offer_id: None,
                payment_hash: None,
                start: None,
            })
            .await
            .map_err(Error::from)?;

        match listinvoices_response.invoices.last() {
            Some(last_invoice) => Ok(last_invoice.pay_index),
            None => Ok(None),
        }
    }

    /// Decode string
    #[instrument(skip(self))]
    async fn decode_string(&self, string: String) -> Result<DecodeResponse, Error> {
        let mut cln_client = self.cln_client().await?;

        cln_client
            .call_typed(&DecodeRequest { string })
            .await
            .map_err(|err| {
                tracing::error!("Could not fetch invoice for offer: {:?}", err);
                Error::ClnRpc(err)
            })
    }

    /// Checks that outgoing payment is not already paid
    #[instrument(skip(self))]
    async fn check_outgoing_unpaided(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<(), payment::Error> {
        let pay_state = self.check_outgoing_payment(payment_identifier).await?;

        match pay_state.status {
            MeltQuoteState::Unpaid | MeltQuoteState::Unknown | MeltQuoteState::Failed => Ok(()),
            MeltQuoteState::Paid => {
                tracing::debug!("Melt attempted on invoice already paid");
                Err(payment::Error::InvoiceAlreadyPaid)
            }
            MeltQuoteState::Pending => {
                tracing::debug!("Melt attempted on invoice already pending");
                Err(payment::Error::InvoicePaymentPending)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bolt12QuotePaymentHashLookup {
    Found([u8; 32]),
    Missing,
    Malformed,
}

/// Whether an `xpay` error and the reconciled CLN payment state require the
/// BOLT12 quote binding to be retained.
fn should_retain_bolt12_binding_after_xpay_error(
    xpay_error: &cln_rpc::RpcError,
    observed_state: Option<MeltQuoteState>,
) -> bool {
    xpay_error_state(xpay_error, observed_state) != MeltQuoteState::Failed
}

fn cln_pays_status_to_mint_state(status: ListpaysPaysStatus) -> MeltQuoteState {
    match status {
        ListpaysPaysStatus::PENDING => MeltQuoteState::Pending,
        ListpaysPaysStatus::COMPLETE => MeltQuoteState::Paid,
        ListpaysPaysStatus::FAILED => MeltQuoteState::Failed,
    }
}

fn select_cln_payment(payments: &[ListpaysPays]) -> Option<&ListpaysPays> {
    payments.iter().min_by_key(|payment| match payment.status {
        ListpaysPaysStatus::COMPLETE => 0_u8,
        ListpaysPaysStatus::PENDING => 1,
        ListpaysPaysStatus::FAILED => 2,
    })
}

async fn fetch_invoice_by_payment_hash(
    cln_client: &mut cln_rpc::ClnRpc,
    payment_hash: &sha256::Hash,
) -> Result<Option<ListinvoicesInvoices>, Error> {
    tracing::debug!("Fetching invoice by payment hash: {}", payment_hash);

    let payment_hash_str = payment_hash.to_string();
    tracing::debug!("Payment hash string: {}", payment_hash_str);

    let request = ListinvoicesRequest {
        payment_hash: Some(payment_hash_str),
        index: None,
        invstring: None,
        label: None,
        limit: None,
        offer_id: None,
        start: None,
    };
    tracing::debug!("Created ListinvoicesRequest");

    match cln_client.call_typed(&request).await {
        Ok(invoice_response) => {
            let invoice_count = invoice_response.invoices.len();
            tracing::debug!(
                "Received {} invoices for payment hash {}",
                invoice_count,
                payment_hash
            );

            if invoice_count > 0 {
                let first_invoice = invoice_response.invoices.first().cloned();
                if let Some(invoice) = &first_invoice {
                    tracing::debug!("Found invoice with payment hash {}", payment_hash);
                    tracing::debug!(
                        "Invoice details - local_offer_id: {:?}, status: {:?}",
                        invoice.local_offer_id,
                        invoice.status
                    );
                } else {
                    tracing::warn!("No invoice found with payment hash {}", payment_hash);
                }
                Ok(first_invoice)
            } else {
                tracing::warn!("No invoices returned for payment hash {}", payment_hash);
                Ok(None)
            }
        }
        Err(e) => {
            tracing::error!(
                "Error fetching invoice by payment hash {}: {}",
                payment_hash,
                e
            );
            Err(Error::from(e))
        }
    }
}

fn outgoing_payment_response_with_status(
    payment_identifier: &PaymentIdentifier,
    status: MeltQuoteState,
) -> MakePaymentResponse {
    MakePaymentResponse {
        payment_lookup_id: payment_identifier.clone(),
        payment_proof: None,
        status,
        total_spent: Amount::new(0, CurrencyUnit::Msat),
    }
}

fn xpay_error_state(
    error: &cln_rpc::RpcError,
    observed_state: Option<MeltQuoteState>,
) -> MeltQuoteState {
    // listpays aggregates attempts by payment hash, so a negative or missing
    // record cannot prove what happened to this specific xpay invocation. Only
    // positive evidence (Paid/Pending) may override the RPC result. Explicit
    // terminal xpay errors prove that this invocation cannot settle even when
    // no send attempt was created and listpays has no row.
    match observed_state {
        Some(MeltQuoteState::Paid) => MeltQuoteState::Paid,
        Some(MeltQuoteState::Pending) => MeltQuoteState::Pending,
        _ if xpay_error_is_explicit_terminal_failure(error) => MeltQuoteState::Failed,
        _ => MeltQuoteState::Unknown,
    }
}

fn xpay_error_is_explicit_terminal_failure(error: &cln_rpc::RpcError) -> bool {
    matches!(
        error.code,
        Some(
            XPAY_DESTINATION_PERMANENT_FAILURE_ERROR_CODE
                | XPAY_ROUTE_NOT_FOUND_ERROR_CODE
                | XPAY_ROUTE_TOO_EXPENSIVE_ERROR_CODE
                | XPAY_INVOICE_EXPIRED_ERROR_CODE
        )
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};

    use cdk_common::database::{
        DbTransactionFinalizer, Error as DatabaseError, KVStore, KVStoreDatabase,
        KVStoreTransaction,
    };
    use cdk_common::payment::Bolt11OutgoingPaymentOptions;

    use super::*;

    const XPAY_ALREADY_PAID_ERROR_CODE: i32 = 219;

    type MemoryKvKey = (String, String, String);

    #[derive(Debug, Default)]
    struct MemoryKvStore {
        entries: Arc<Mutex<HashMap<MemoryKvKey, Vec<u8>>>>,
    }

    #[derive(Debug)]
    struct MemoryKvTransaction {
        entries: Arc<Mutex<HashMap<MemoryKvKey, Vec<u8>>>>,
        writes: HashMap<MemoryKvKey, Option<Vec<u8>>>,
    }

    fn memory_kv_lock_error() -> DatabaseError {
        DatabaseError::Database(Box::new(std::io::Error::other(
            "memory kv store lock poisoned",
        )))
    }

    fn test_rpc_error(code: Option<i32>) -> cln_rpc::RpcError {
        cln_rpc::RpcError {
            code,
            message: "xpay failed".to_string(),
            data: None,
        }
    }

    fn memory_kv_key(primary_namespace: &str, secondary_namespace: &str, key: &str) -> MemoryKvKey {
        (
            primary_namespace.to_string(),
            secondary_namespace.to_string(),
            key.to_string(),
        )
    }

    fn test_listpays_payment(id: u8, status: ListpaysPaysStatus) -> ListpaysPays {
        ListpaysPays {
            amount_msat: None,
            amount_sent_msat: None,
            bolt11: None,
            bolt12: None,
            completed_at: None,
            created_index: None,
            description: None,
            destination: None,
            erroronion: None,
            label: None,
            number_of_parts: None,
            preimage: None,
            updated_index: None,
            status,
            created_at: 0,
            payment_hash: *Sha256::from_bytes_ref(&[id; 32]),
        }
    }

    #[async_trait::async_trait]
    impl KVStoreDatabase for MemoryKvStore {
        type Err = DatabaseError;

        async fn kv_read(
            &self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
        ) -> Result<Option<Vec<u8>>, Self::Err> {
            let entries = self.entries.lock().map_err(|_| memory_kv_lock_error())?;

            Ok(entries
                .get(&memory_kv_key(primary_namespace, secondary_namespace, key))
                .cloned())
        }

        async fn kv_list(
            &self,
            primary_namespace: &str,
            secondary_namespace: &str,
        ) -> Result<Vec<String>, Self::Err> {
            let entries = self.entries.lock().map_err(|_| memory_kv_lock_error())?;

            Ok(entries
                .keys()
                .filter(|(primary, secondary, _)| {
                    primary == primary_namespace && secondary == secondary_namespace
                })
                .map(|(_, _, key)| key.clone())
                .collect())
        }
    }

    #[async_trait::async_trait]
    impl KVStore for MemoryKvStore {
        async fn begin_transaction(
            &self,
        ) -> Result<Box<dyn KVStoreTransaction<Self::Err> + Send + Sync>, DatabaseError> {
            Ok(Box::new(MemoryKvTransaction {
                entries: Arc::clone(&self.entries),
                writes: HashMap::new(),
            }))
        }
    }

    #[async_trait::async_trait]
    impl KVStoreTransaction<DatabaseError> for MemoryKvTransaction {
        async fn kv_read(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
        ) -> Result<Option<Vec<u8>>, DatabaseError> {
            let key = memory_kv_key(primary_namespace, secondary_namespace, key);

            if let Some(value) = self.writes.get(&key) {
                return Ok(value.clone());
            }

            let entries = self.entries.lock().map_err(|_| memory_kv_lock_error())?;

            Ok(entries.get(&key).cloned())
        }

        async fn kv_write(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
            value: &[u8],
        ) -> Result<(), DatabaseError> {
            self.writes.insert(
                memory_kv_key(primary_namespace, secondary_namespace, key),
                Some(value.to_vec()),
            );

            Ok(())
        }

        async fn kv_remove(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
        ) -> Result<(), DatabaseError> {
            self.writes.insert(
                memory_kv_key(primary_namespace, secondary_namespace, key),
                None,
            );

            Ok(())
        }

        async fn kv_write_if_absent(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
            value: &[u8],
        ) -> Result<bool, DatabaseError> {
            let key = memory_kv_key(primary_namespace, secondary_namespace, key);

            let exists = match self.writes.get(&key) {
                Some(pending) => pending.is_some(),
                None => {
                    let entries = self.entries.lock().map_err(|_| memory_kv_lock_error())?;
                    entries.contains_key(&key)
                }
            };

            if exists {
                return Ok(false);
            }

            self.writes.insert(key, Some(value.to_vec()));

            Ok(true)
        }

        async fn kv_write_if_equals(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
            key: &str,
            expected: &[u8],
            replacement: &[u8],
        ) -> Result<bool, DatabaseError> {
            let key = memory_kv_key(primary_namespace, secondary_namespace, key);

            let current = match self.writes.get(&key) {
                Some(pending) => pending.clone(),
                None => {
                    let entries = self.entries.lock().map_err(|_| memory_kv_lock_error())?;
                    entries.get(&key).cloned()
                }
            };

            match current {
                Some(current) if current == expected => {
                    self.writes.insert(key, Some(replacement.to_vec()));
                    Ok(true)
                }
                _ => Ok(false),
            }
        }

        async fn kv_list(
            &mut self,
            primary_namespace: &str,
            secondary_namespace: &str,
        ) -> Result<Vec<String>, DatabaseError> {
            let entries = self.entries.lock().map_err(|_| memory_kv_lock_error())?;
            let mut keys = entries
                .keys()
                .filter(|(primary, secondary, _)| {
                    primary == primary_namespace && secondary == secondary_namespace
                })
                .map(|(_, _, key)| key.clone())
                .collect::<BTreeSet<_>>();

            for ((primary, secondary, key), value) in &self.writes {
                if primary == primary_namespace && secondary == secondary_namespace {
                    match value {
                        Some(_) => {
                            keys.insert(key.clone());
                        }
                        None => {
                            keys.remove(key);
                        }
                    }
                }
            }

            Ok(keys.into_iter().collect())
        }
    }

    #[async_trait::async_trait]
    impl DbTransactionFinalizer for MemoryKvTransaction {
        type Err = DatabaseError;

        async fn commit(self: Box<Self>) -> Result<(), Self::Err> {
            let this = *self;
            let mut entries = this.entries.lock().map_err(|_| memory_kv_lock_error())?;

            for (key, value) in this.writes {
                match value {
                    Some(value) => {
                        entries.insert(key, value);
                    }
                    None => {
                        entries.remove(&key);
                    }
                }
            }

            Ok(())
        }

        async fn rollback(self: Box<Self>) -> Result<(), Self::Err> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct UnusedKvStore;

    #[async_trait::async_trait]
    impl KVStoreDatabase for UnusedKvStore {
        type Err = DatabaseError;

        async fn kv_read(
            &self,
            _primary_namespace: &str,
            _secondary_namespace: &str,
            _key: &str,
        ) -> Result<Option<Vec<u8>>, Self::Err> {
            Ok(None)
        }

        async fn kv_list(
            &self,
            _primary_namespace: &str,
            _secondary_namespace: &str,
        ) -> Result<Vec<String>, Self::Err> {
            Ok(Vec::new())
        }
    }

    #[async_trait::async_trait]
    impl KVStore for UnusedKvStore {
        async fn begin_transaction(
            &self,
        ) -> Result<Box<dyn KVStoreTransaction<Self::Err> + Send + Sync>, DatabaseError> {
            Err(DatabaseError::Database(Box::new(std::io::Error::other(
                "unused kv store transaction",
            ))))
        }
    }

    fn test_cln_with_kv(kv_store: DynKVStore) -> Cln {
        Cln {
            rpc_socket: PathBuf::new(),
            fee_reserve: FeeReserve {
                min_fee_reserve: Amount::ZERO,
                percent_fee_reserve: 0.0,
            },
            expose_private_channels: false,
            bolt12: true,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            kv_store,
        }
    }

    fn test_cln() -> Cln {
        test_cln_with_kv(Arc::new(UnusedKvStore))
    }

    fn test_cln_with_memory_kv() -> Cln {
        test_cln_with_kv(Arc::new(MemoryKvStore::default()))
    }

    fn test_invoice() -> Bolt11Invoice {
        Bolt11Invoice::from_str("lnbc100n1pnvpufspp5djn8hrq49r8cghwye9kqw752qjncwyfnrprhprpqk43mwcy4yfsqdq5g9kxy7fqd9h8vmmfvdjscqzzsxqyz5vqsp5uhpjt36rj75pl7jq2sshaukzfkt7uulj456s4mh7uy7l6vx7lvxs9qxpqysgqedwz08acmqwtk8g4vkwm2w78suwt2qyzz6jkkwcgrjm3r3hs6fskyhvud4fan3keru7emjm8ygqpcrwtlmhfjfmer3afs5hhwamgr4cqtactdq")
            .expect("test invoice must parse")
    }

    #[tokio::test]
    async fn get_payment_quote_rejects_amountless_mismatch() {
        let invoice = test_invoice();
        let invoice_amount = invoice
            .amount_milli_satoshis()
            .expect("test invoice must include amount");
        let options = Bolt11OutgoingPaymentOptions {
            bolt11: invoice,
            max_fee_amount: None,
            timeout_secs: None,
            melt_options: Some(MeltOptions::new_amountless(invoice_amount + 1)),
            quote_id: cdk_common::QuoteId::new(),
        };

        let err = test_cln()
            .get_payment_quote(
                &CurrencyUnit::Msat,
                OutgoingPaymentOptions::Bolt11(Box::new(options)),
            )
            .await
            .expect_err("amountless override must match invoice amount");

        assert!(matches!(err, payment::Error::AmountMismatch));
    }

    #[tokio::test]
    async fn get_payment_quote_accepts_matching_amountless_amount() {
        let invoice = test_invoice();
        let invoice_amount = invoice
            .amount_milli_satoshis()
            .expect("test invoice must include amount");
        let options = Bolt11OutgoingPaymentOptions {
            bolt11: invoice,
            max_fee_amount: None,
            timeout_secs: None,
            melt_options: Some(MeltOptions::new_amountless(invoice_amount)),
            quote_id: cdk_common::QuoteId::new(),
        };

        let quote = test_cln()
            .get_payment_quote(
                &CurrencyUnit::Msat,
                OutgoingPaymentOptions::Bolt11(Box::new(options)),
            )
            .await
            .expect("matching amountless override must be accepted");

        assert_eq!(
            quote.amount,
            Amount::new(invoice_amount, CurrencyUnit::Msat)
        );
    }

    #[tokio::test]
    async fn bolt12_quote_payment_hash_round_trips_through_kv() {
        let cln = test_cln_with_memory_kv();
        let quote_id = QuoteId::new();
        let payment_hash = [42; 32];

        cln.write_bolt12_quote_payment_hash(&quote_id, &payment_hash)
            .await
            .expect("payment hash should be written");

        let stored_payment_hash = cln
            .read_bolt12_quote_payment_hash(&quote_id)
            .await
            .expect("payment hash should be read");

        assert_eq!(
            stored_payment_hash,
            Bolt12QuotePaymentHashLookup::Found(payment_hash)
        );
        assert_eq!(
            Cln::bolt12_quote_payment_hash_key(&quote_id).expect("uuid quote id should be valid"),
            quote_id.to_string()
        );
    }

    #[tokio::test]
    async fn bolt12_quote_payment_hash_repeat_of_same_hash_is_idempotent() {
        let cln = test_cln_with_memory_kv();
        let quote_id = QuoteId::new();
        let payment_hash = [1; 32];

        cln.write_bolt12_quote_payment_hash(&quote_id, &payment_hash)
            .await
            .expect("first payment hash should be written");
        cln.write_bolt12_quote_payment_hash(&quote_id, &payment_hash)
            .await
            .expect("repeating the same payment hash must succeed for recovery");

        let stored_payment_hash = cln
            .read_bolt12_quote_payment_hash(&quote_id)
            .await
            .expect("payment hash should be read");

        assert_eq!(
            stored_payment_hash,
            Bolt12QuotePaymentHashLookup::Found(payment_hash)
        );
    }

    #[tokio::test]
    async fn bolt12_quote_payment_hash_binding_is_write_once() {
        let cln = test_cln_with_memory_kv();
        let quote_id = QuoteId::new();
        let first_payment_hash = [1; 32];
        let second_payment_hash = [2; 32];

        cln.write_bolt12_quote_payment_hash(&quote_id, &first_payment_hash)
            .await
            .expect("first payment hash should be written");

        let err = cln
            .write_bolt12_quote_payment_hash(&quote_id, &second_payment_hash)
            .await
            .expect_err("a conflicting payment hash must be rejected");
        assert!(
            matches!(err, Error::Bolt12QuoteBindingConflict { .. }),
            "unexpected error: {err}"
        );

        let stored_payment_hash = cln
            .read_bolt12_quote_payment_hash(&quote_id)
            .await
            .expect("payment hash should be read");

        assert_eq!(
            stored_payment_hash,
            Bolt12QuotePaymentHashLookup::Found(first_payment_hash),
            "the first binding must be retained"
        );
    }

    #[tokio::test]
    async fn failed_bolt12_binding_can_be_released_without_removing_a_retry() {
        let cln = test_cln_with_memory_kv();
        let quote_id = QuoteId::new();
        let failed_payment_hash = [1; 32];
        let retry_payment_hash = [2; 32];

        cln.write_bolt12_quote_payment_hash(&quote_id, &failed_payment_hash)
            .await
            .expect("failed payment hash should be written");
        assert!(cln
            .delete_bolt12_quote_payment_hash_if_equals(&quote_id, &failed_payment_hash)
            .await
            .expect("failed binding should be released"));

        cln.write_bolt12_quote_payment_hash(&quote_id, &retry_payment_hash)
            .await
            .expect("retry should claim the released quote");
        assert!(!cln
            .delete_bolt12_quote_payment_hash_if_equals(&quote_id, &failed_payment_hash)
            .await
            .expect("stale cleanup should be checked atomically"));

        assert_eq!(
            cln.read_bolt12_quote_payment_hash(&quote_id)
                .await
                .expect("retry binding should remain readable"),
            Bolt12QuotePaymentHashLookup::Found(retry_payment_hash),
            "stale failed-payment cleanup must not remove a newer retry"
        );
    }

    #[tokio::test]
    async fn bolt12_quote_payment_hash_concurrent_claims_have_single_winner() {
        let kv_store = Arc::new(MemoryKvStore::default());
        let first_cln = test_cln_with_kv(kv_store.clone());
        let second_cln = test_cln_with_kv(kv_store);
        let quote_id = QuoteId::new();
        let first_payment_hash = [1; 32];
        let second_payment_hash = [2; 32];

        let (first_result, second_result) = tokio::join!(
            first_cln.write_bolt12_quote_payment_hash(&quote_id, &first_payment_hash),
            second_cln.write_bolt12_quote_payment_hash(&quote_id, &second_payment_hash),
        );

        let outcomes = [&first_result, &second_result];
        let winners = outcomes.iter().filter(|result| result.is_ok()).count();
        let conflicts = outcomes
            .iter()
            .filter(|result| matches!(result, Err(Error::Bolt12QuoteBindingConflict { .. })))
            .count();

        assert_eq!(winners, 1, "exactly one dispatch may claim the quote");
        assert_eq!(conflicts, 1, "the losing dispatch must be rejected");

        let winner_hash = if first_result.is_ok() {
            first_payment_hash
        } else {
            second_payment_hash
        };
        let stored_payment_hash = first_cln
            .read_bolt12_quote_payment_hash(&quote_id)
            .await
            .expect("payment hash should be read");
        assert_eq!(
            stored_payment_hash,
            Bolt12QuotePaymentHashLookup::Found(winner_hash),
            "the stored binding must belong to the winning dispatch"
        );
    }

    #[test]
    fn cln_payment_selection_prefers_pending_over_failed() {
        let failed = test_listpays_payment(1, ListpaysPaysStatus::FAILED);
        let pending = test_listpays_payment(2, ListpaysPaysStatus::PENDING);
        let payments = [failed, pending];

        let selected = select_cln_payment(&payments).expect("payment should be selected");

        assert_eq!(selected.status, ListpaysPaysStatus::PENDING);
        assert_eq!(selected.payment_hash, *Sha256::from_bytes_ref(&[2; 32]));
    }

    #[test]
    fn xpay_error_classification_controls_bolt12_binding_retention() {
        let transport_error = cln_rpc::RpcError {
            code: None,
            message: "no response from lightningd".to_string(),
            data: None,
        };
        let coded_error = cln_rpc::RpcError {
            code: Some(209),
            message: "other payment error".to_string(),
            data: None,
        };
        let already_paid_error = cln_rpc::RpcError {
            code: Some(219),
            message: "invoice already paid".to_string(),
            data: None,
        };
        let no_route_error = cln_rpc::RpcError {
            code: Some(XPAY_ROUTE_NOT_FOUND_ERROR_CODE),
            message: "could not find a route".to_string(),
            data: None,
        };

        assert!(should_retain_bolt12_binding_after_xpay_error(
            &transport_error,
            Some(MeltQuoteState::Failed)
        ));
        assert!(should_retain_bolt12_binding_after_xpay_error(
            &coded_error,
            None
        ));
        assert!(!should_retain_bolt12_binding_after_xpay_error(
            &no_route_error,
            None
        ));
        assert!(should_retain_bolt12_binding_after_xpay_error(
            &coded_error,
            Some(MeltQuoteState::Unknown)
        ));
        assert!(should_retain_bolt12_binding_after_xpay_error(
            &coded_error,
            Some(MeltQuoteState::Pending)
        ));
        assert!(should_retain_bolt12_binding_after_xpay_error(
            &already_paid_error,
            Some(MeltQuoteState::Paid)
        ));
        assert!(should_retain_bolt12_binding_after_xpay_error(
            &coded_error,
            Some(MeltQuoteState::Failed)
        ));
        assert!(should_retain_bolt12_binding_after_xpay_error(
            &coded_error,
            Some(MeltQuoteState::Unpaid)
        ));
    }

    #[test]
    fn cln_payment_selection_prefers_complete_over_pending() {
        let pending = test_listpays_payment(1, ListpaysPaysStatus::PENDING);
        let complete = test_listpays_payment(2, ListpaysPaysStatus::COMPLETE);
        let payments = [pending, complete];

        let selected = select_cln_payment(&payments).expect("payment should be selected");

        assert_eq!(selected.status, ListpaysPaysStatus::COMPLETE);
        assert_eq!(selected.payment_hash, *Sha256::from_bytes_ref(&[2; 32]));
    }

    #[test]
    fn cln_payment_selection_returns_failed_when_all_failed() {
        let failed = test_listpays_payment(1, ListpaysPaysStatus::FAILED);
        let payments = [failed];

        let selected = select_cln_payment(&payments).expect("payment should be selected");

        assert_eq!(selected.status, ListpaysPaysStatus::FAILED);
    }

    #[test]
    fn cln_payment_selection_returns_none_when_missing() {
        assert!(select_cln_payment(&[]).is_none());
    }

    #[test]
    fn xpay_explicit_terminal_errors_do_not_require_payment_record() {
        for code in [
            XPAY_DESTINATION_PERMANENT_FAILURE_ERROR_CODE,
            XPAY_ROUTE_NOT_FOUND_ERROR_CODE,
            XPAY_ROUTE_TOO_EXPENSIVE_ERROR_CODE,
            XPAY_INVOICE_EXPIRED_ERROR_CODE,
        ] {
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), None),
                MeltQuoteState::Failed
            );
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), Some(MeltQuoteState::Unknown)),
                MeltQuoteState::Failed
            );
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), Some(MeltQuoteState::Failed)),
                MeltQuoteState::Failed
            );
        }
    }

    #[test]
    fn xpay_nonspecific_rpc_errors_ignore_negative_reconciliation() {
        for code in [-1, 209] {
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), None),
                MeltQuoteState::Unknown
            );
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), Some(MeltQuoteState::Unknown)),
                MeltQuoteState::Unknown
            );
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), Some(MeltQuoteState::Failed)),
                MeltQuoteState::Unknown
            );
            assert_eq!(
                xpay_error_state(&test_rpc_error(Some(code)), Some(MeltQuoteState::Unpaid)),
                MeltQuoteState::Unknown
            );
        }
    }

    #[test]
    fn xpay_transport_and_already_paid_errors_are_ambiguous() {
        assert_eq!(
            xpay_error_state(&test_rpc_error(None), None),
            MeltQuoteState::Unknown
        );
        assert_eq!(
            xpay_error_state(&test_rpc_error(Some(XPAY_ALREADY_PAID_ERROR_CODE)), None),
            MeltQuoteState::Unknown
        );
        assert_eq!(
            xpay_error_state(&test_rpc_error(None), Some(MeltQuoteState::Failed)),
            MeltQuoteState::Unknown
        );
    }

    #[test]
    fn xpay_positive_reconciliation_takes_precedence() {
        let transport_error = test_rpc_error(None);
        let terminal_rpc_error = test_rpc_error(Some(XPAY_ROUTE_NOT_FOUND_ERROR_CODE));

        assert_eq!(
            xpay_error_state(&transport_error, Some(MeltQuoteState::Paid)),
            MeltQuoteState::Paid
        );
        assert_eq!(
            xpay_error_state(&terminal_rpc_error, Some(MeltQuoteState::Pending)),
            MeltQuoteState::Pending
        );
    }

    #[tokio::test]
    async fn quote_id_outgoing_lookup_without_mapping_returns_unpaid() {
        let cln = test_cln_with_memory_kv();
        let payment_identifier = PaymentIdentifier::QuoteId(QuoteId::new());

        let response = cln
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("missing quote mapping should not require CLN lookup");

        assert_eq!(response.payment_lookup_id, payment_identifier);
        assert_eq!(response.status, MeltQuoteState::Unpaid);
        assert_eq!(response.payment_proof, None);
        assert_eq!(response.total_spent, Amount::new(0, CurrencyUnit::Msat));
    }

    #[tokio::test]
    async fn malformed_quote_id_mapping_returns_unknown() {
        let kv_store = Arc::new(MemoryKvStore::default());
        let cln = test_cln_with_kv(kv_store.clone());
        let quote_id = QuoteId::new();
        let key =
            Cln::bolt12_quote_payment_hash_key(&quote_id).expect("uuid quote id should be valid");

        let mut tx = kv_store
            .begin_transaction()
            .await
            .expect("transaction should begin");
        tx.kv_write(
            CLN_KV_PRIMARY_NAMESPACE,
            CLN_KV_BOLT12_OUTGOING_SECONDARY_NAMESPACE,
            &key,
            b"not-a-payment-hash",
        )
        .await
        .expect("malformed payment hash should be written");
        tx.commit().await.expect("transaction should commit");

        let payment_identifier = PaymentIdentifier::QuoteId(quote_id);
        let response = cln
            .check_outgoing_payment(&payment_identifier)
            .await
            .expect("malformed quote mapping should not require CLN lookup");

        assert_eq!(response.payment_lookup_id, payment_identifier);
        assert_eq!(response.status, MeltQuoteState::Unknown);
    }

    #[tokio::test]
    async fn base64_quote_id_mapping_is_rejected() {
        let cln = test_cln_with_memory_kv();
        let quote_id = QuoteId::BASE64("SGVsbG8gV29ybGQh".to_string());
        let payment_hash = [9; 32];

        let err = cln
            .write_bolt12_quote_payment_hash(&quote_id, &payment_hash)
            .await
            .expect_err("base64 quote ids should not be stored in CLN bolt12 mapping");

        assert!(matches!(err, Error::InvalidQuoteId));
    }

    #[tokio::test]
    async fn direct_hash_outgoing_lookup_bypasses_quote_mapping() {
        let cln = test_cln();
        let payment_hash = [7; 32];

        assert_eq!(
            cln.outgoing_payment_hash(&PaymentIdentifier::PaymentHash(payment_hash))
                .await
                .expect("payment hash should resolve"),
            Some(payment_hash)
        );
        assert_eq!(
            cln.outgoing_payment_hash(&PaymentIdentifier::Bolt12PaymentHash(payment_hash))
                .await
                .expect("bolt12 payment hash should resolve"),
            Some(payment_hash)
        );
    }
}
