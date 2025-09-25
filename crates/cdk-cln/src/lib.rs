//! CDK lightning backend for CLN

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![warn(rustdoc::bare_urls)]

use std::cmp::max;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::hashes::sha256::Hash;
use bitcoin::hashes::Hash as OtherHash;
use cdk_common::amount::{to_unit, Amount};
use cdk_common::common::FeeReserve;
use cdk_common::database::mint::DynMintKVStore;
use cdk_common::nuts::{CurrencyUnit, MeltOptions, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11IncomingPaymentOptions, Bolt11Settings, Bolt12IncomingPaymentOptions,
    CreateIncomingPaymentResponse, Event, IncomingPaymentOptions, MakePaymentResponse, MintPayment,
    OutgoingPaymentOptions, PaymentIdentifier, PaymentQuoteResponse, WaitPaymentResponse,
};
use cdk_common::util::{hex, unix_time};
use cdk_common::{Bolt11Invoice, QuoteId};
use cln_rpc::model::requests::{
    DecodeRequest, FetchinvoiceRequest, InvoiceRequest, ListinvoicesRequest, ListpaysRequest,
    OfferRequest, PayRequest, WaitanyinvoiceRequest,
};
use cln_rpc::model::responses::{
    DecodeResponse, ListinvoicesInvoices, ListinvoicesInvoicesStatus, ListpaysPaysStatus,
    PayStatus, WaitanyinvoiceResponse, WaitanyinvoiceStatus,
};
use cln_rpc::primitives::{Amount as CLN_Amount, AmountOrAny, Sha256};
use cln_rpc::ClnRpc;
use error::Error;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uuid::Uuid;

use crate::database::IncomingPaymentIdentifier;
use crate::logging_helpers::{PaymentContext, PaymentType};

mod database;
pub mod error;
mod logging_helpers;

/// Payment status response from CLN
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PaymentStatus {
    /// Payment status
    pub status: MeltQuoteState,
    /// Paymant hash
    pub payment_hash: [u8; 32],
    /// Payment proof (preimage as hex string)
    pub payment_proof: Option<String>,
    /// Total amount spent
    pub total_spent: Amount,
}

impl PaymentStatus {
    fn unpaid(&self) -> Result<(), payment::Error> {
        match self.status {
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

/// CLN mint backend
#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    database: database::Database,
}

// enum PaymentId {}

impl Cln {
    /// Create new [`Cln`]
    pub async fn new(
        rpc_socket: PathBuf,
        fee_reserve: FeeReserve,
        kv_store: DynMintKVStore,
    ) -> Result<Self, Error> {
        let database = database::Database::new(kv_store);
        Ok(Self {
            rpc_socket,
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            database,
        })
    }
}

#[async_trait]
impl MintPayment for Cln {
    type Err = payment::Error;

    async fn get_settings(&self) -> Result<Value, Self::Err> {
        Ok(serde_json::to_value(Bolt11Settings {
            mpp: true,
            unit: CurrencyUnit::Msat,
            invoice_description: true,
            amountless: true,
            bolt12: true,
        })?)
    }

    /// Is wait invoice active
    fn is_wait_invoice_active(&self) -> bool {
        self.wait_invoice_is_active.load(Ordering::SeqCst)
    }

    /// Cancel wait invoice
    fn cancel_wait_invoice(&self) {
        self.wait_invoice_cancel_token.cancel()
    }

    #[instrument(skip_all, fields(component = "cln", operation = "wait_payment_event"))]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let last_pay_index = self.get_last_pay_index().await?.inspect(|&idx| {
            tracing::debug!(last_pay_index = idx, "Retrieved last payment index");
        });

        let cln_client = self.cln_client().await?;
        let database = self.database.clone();
        let stream = futures::stream::unfold(
            (
                cln_client,
                last_pay_index,
                self.wait_invoice_cancel_token.clone(),
                Arc::clone(&self.wait_invoice_is_active),
                database,
            ),
            |(mut cln_client, mut last_pay_idx, cancel_token, is_active, database)| async move {
                // Set the stream as active
                is_active.store(true, Ordering::SeqCst);
                tracing::debug!(last_pay_index = ?last_pay_idx, "Stream is now active, waiting for invoice events");

                loop {
                    tokio::select! {
                        _ = cancel_token.cancelled() => {
                            // Set the stream as inactive
                            is_active.store(false, Ordering::SeqCst);
                            tracing::info!("Invoice stream cancelled");
                            // End the stream
                            return None;
                        }
                        result = cln_client.call(cln_rpc::Request::WaitAnyInvoice(WaitanyinvoiceRequest {
                            timeout: None,
                            lastpay_index: last_pay_idx,
                        })) => {
                            match result {
                                Ok(invoice) => {
                                        // Try to convert the invoice to WaitanyinvoiceResponse
                            let wait_any_response_result: Result<WaitanyinvoiceResponse, _> =
                                invoice.try_into();

                            let wait_any_response = match wait_any_response_result {
                                Ok(response) => {
                                    response
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = ?e,
                                        "Failed to parse WaitAnyInvoice response"
                                    );
                                    // Continue to the next iteration without panicking
                                    continue;
                                }
                            };

                            // Check the status of the invoice
                            // We only want to yield invoices that have been paid
                            match wait_any_response.status {
                                WaitanyinvoiceStatus::PAID => {
                                    tracing::info!(pay_index = ?wait_any_response.pay_index, "Invoice is PAID");
                                }
                                WaitanyinvoiceStatus::EXPIRED => {
                                    tracing::debug!(pay_index = ?wait_any_response.pay_index, "Invoice is EXPIRED, skipping");
                                    continue;
                                }
                            }

                            last_pay_idx = wait_any_response.pay_index;

                            // Store the updated pay index in KV store for persistence
                            if let Some(pay_index) = last_pay_idx {
                                if let Err(e) = database.store_last_pay_index(pay_index).await {
                                    tracing::warn!(pay_index = pay_index, error = %e, "Failed to store last pay index to KV store");
                                }
                            }

                            let payment_hash = wait_any_response.payment_hash;

                            let amount_msats = match wait_any_response.amount_received_msat {
                                Some(amt) => {
                                    tracing::info!(amount_msat = amt.msat(), payment_hash = %payment_hash, "Received payment");
                                    amt
                                }
                                None => {
                                    tracing::error!("No amount in paid invoice, this should not happen");
                                    continue;
                                }
                            };

                            let payment_hash = Hash::from_bytes_ref(payment_hash.as_ref());

                            let request_lookup_id = match wait_any_response.bolt12 {
                                // If it is a bolt12 payment we need to get the offer_id as this is what we use as the request look up.
                                // Since this is not returned in the wait any response,
                                // we need to do a second query for it.
                                Some(bolt12) => {
                                    tracing::info!(bolt12 = %bolt12, "Processing BOLT12 payment");
                                    match fetch_invoice_by_payment_hash(
                                        &mut cln_client,
                                        payment_hash,
                                    )
                                    .await
                                    {
                                        Ok(Some(invoice)) => {
                                            if let Some(local_offer_id) = invoice.local_offer_id {
                                                tracing::info!(amount_msat = amount_msats.msat(), offer_id = %local_offer_id, "Received bolt12 payment");
                                                // Look up quote ID by offer ID
                                                match database.get_quote_id_by_incoming_bolt12_offer(&local_offer_id.to_string()).await {
                                                    Ok(Some(quote_id)) => {
                                                        tracing::info!(quote_id = %quote_id, offer_id = %local_offer_id, "Found quote_id for offer_id");
                                                        PaymentIdentifier::QuoteId(quote_id)
                                                    }
                                                    Ok(None) => {
                                                        tracing::warn!(offer_id = %local_offer_id, "No quote_id found for offer_id, falling back to offer_id");
                                                        PaymentIdentifier::OfferId(local_offer_id.to_string())
                                                    }
                                                    Err(e) => {
                                                        tracing::error!(offer_id = %local_offer_id, error = %e, "Database error looking up quote_id for offer_id, falling back to offer_id");
                                                        PaymentIdentifier::OfferId(local_offer_id.to_string())
                                                    }
                                                }
                                            } else {
                                                tracing::warn!("BOLT12 invoice has no local_offer_id, skipping");
                                                continue;
                                            }
                                        }
                                        Ok(None) => {
                                            tracing::warn!("Failed to find invoice by payment hash, skipping");
                                            continue;
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                error = %e,
                                                "Error fetching invoice by payment hash"
                                            );
                                            continue;
                                        }
                                    }
                                }
                                None => {
                                    tracing::info!(payment_hash = %payment_hash, "Processing BOLT11 payment");
                                    // Look up quote ID by payment hash
                                    match database.get_quote_id_by_incoming_bolt11_hash(payment_hash.as_ref()).await {
                                        Ok(Some(quote_id)) => {
                                            tracing::info!(quote_id = %quote_id, payment_hash = %payment_hash, "Found quote_id for payment_hash");
                                            PaymentIdentifier::QuoteId(quote_id)
                                        }
                                        Ok(None) => {
                                            tracing::warn!(payment_hash = %payment_hash, "No quote_id found for payment_hash, falling back to payment_hash");
                                            PaymentIdentifier::PaymentHash(*payment_hash.as_ref())
                                        }
                                        Err(e) => {
                                            tracing::error!(payment_hash = %payment_hash, error = %e, "Database error looking up quote_id for payment_hash, falling back to payment_hash");
                                            PaymentIdentifier::PaymentHash(*payment_hash.as_ref())
                                        }
                                    }
                                },
                            };

                            let response = WaitPaymentResponse {
                                payment_identifier: request_lookup_id,
                                payment_amount: amount_msats.msat().into(),
                                unit: CurrencyUnit::Msat,
                                payment_id: payment_hash.to_string()
                            };
                            let event = Event::PaymentReceived(response);

                            break Some((event, (cln_client, last_pay_idx, cancel_token, is_active, database)));
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "Error fetching invoice");
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

        Ok(stream)
    }

    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        quote_id: &QuoteId,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        match options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                // If we have specific amount options, use those
                let amount_msat: Amount = if let Some(melt_options) = bolt11_options.melt_options {
                    match melt_options {
                        MeltOptions::Amountless { amountless } => {
                            let amount_msat = amountless.amount_msat;

                            if let Some(invoice_amount) =
                                bolt11_options.bolt11.amount_milli_satoshis()
                            {
                                if !invoice_amount == u64::from(amount_msat) {
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
                let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

                let fee = self.calculate_fee(amount);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::QuoteId(quote_id.to_owned())),
                    amount,
                    fee,
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
                })
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
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
                let amount = to_unit(amount_msat, &CurrencyUnit::Msat, unit)?;

                let fee = self.calculate_fee(amount);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::QuoteId(quote_id.to_owned())),
                    amount,
                    fee,
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
                })
            }
        }
    }

    #[instrument(skip_all, fields(component = "cln", quote_id = %quote_id, operation = "make_payment"))]
    async fn make_payment(
        &self,
        quote_id: &QuoteId,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let max_fee_msat: Option<u64>;
        let mut partial_amount: Option<u64> = None;
        let mut amount_msat: Option<u64> = None;

        let mut cln_client = self.cln_client().await?;
        self.check_outgoing_unpaided(quote_id).await?;

        let (invoice, hash, payment_ctx) = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let payment_hash = bolt11_options.bolt11.payment_hash();
                let payment_hash_hex = hex::encode(payment_hash.to_byte_array());
                let ctx =
                    PaymentContext::new(quote_id, PaymentType::Bolt11).with_hash(&payment_hash_hex);

                ctx.log_start("Processing BOLT11 payment");

                // Check if invoice already paid
                self.check_outgoing_payment_by_hash(&payment_hash.to_byte_array())
                    .await?
                    .unpaid()?;

                if let Some(melt_options) = bolt11_options.melt_options {
                    match melt_options {
                        MeltOptions::Mpp { mpp } => {
                            partial_amount = Some(mpp.amount.into());
                            ctx.log_info(&format!(
                                "Payment is partial amount (MPP): {} msat",
                                mpp.amount
                            ));
                        }
                        MeltOptions::Amountless { amountless } => {
                            amount_msat = Some(amountless.amount_msat.into());
                            ctx.log_info(&format!(
                                "Payment is amountless with specified amount: {} msat",
                                amountless.amount_msat
                            ));
                        }
                    }
                } else {
                    ctx.log_info(&format!(
                        "Payment using full invoice amount: {:?} msat",
                        bolt11_options.bolt11.amount_milli_satoshis()
                    ));
                }

                max_fee_msat = bolt11_options.max_fee_amount.map(|a| a.into());
                if let Some(max_fee) = max_fee_msat {
                    ctx.log_info(&format!("Maximum fee limit set: {} msat", max_fee));
                }

                (
                    bolt11_options.bolt11.to_string(),
                    payment_hash.to_byte_array(),
                    ctx,
                )
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let ctx = PaymentContext::new(quote_id, PaymentType::Bolt12);
                ctx.log_start("Processing BOLT12 payment");

                let offer = &bolt12_options.offer;
                let amount_msat: u64 = if let Some(amount) = bolt12_options.melt_options {
                    let amt = amount.amount_msat().into();
                    ctx.log_info(&format!("Using specified amount: {} msat", amt));
                    amt
                } else {
                    let decode_response = self.decode_string(offer.to_string()).await?;
                    let amt = decode_response
                        .offer_amount_msat
                        .ok_or(Error::UnknownInvoiceAmount)?
                        .msat();
                    ctx.log_info(&format!("Using offer amount: {} msat", amt));
                    amt
                };

                ctx.log_debug("Fetching invoice from offer");

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
                        ctx.log_error("Could not fetch invoice for offer", &err);
                        Error::ClnRpc(err)
                    })?;

                ctx.log_info("Successfully fetched invoice from offer");

                let decode_response = self.decode_string(cln_response.invoice.clone()).await?;
                let payment_hash: [u8; 32] = hex::decode(
                    decode_response
                        .invoice_payment_hash
                        .ok_or(Error::UnknownInvoice)?,
                )
                .map_err(|e| Error::Bolt12(e.to_string()))?
                .try_into()
                .map_err(|_| Error::InvalidHash)?;

                let payment_hash_hex = hex::encode(payment_hash);
                let ctx_with_hash = ctx.with_hash(&payment_hash_hex);

                max_fee_msat = bolt12_options.max_fee_amount.map(|a| a.into());
                if let Some(max_fee) = max_fee_msat {
                    ctx_with_hash.log_info(&format!("Maximum fee limit set: {} msat", max_fee));
                }

                (cln_response.invoice, payment_hash, ctx_with_hash)
            }
        };

        // Store pending payment status
        let payment_status = PaymentStatus {
            status: MeltQuoteState::Pending,
            payment_hash: hash,
            payment_proof: None,
            total_spent: Amount::ZERO,
        };
        self.database
            .store_outgoing_payment(quote_id, payment_status)
            .await?;

        // Execute payment
        payment_ctx.log_info("Executing payment");

        let cln_response = cln_client
            .call_typed(&PayRequest {
                bolt11: invoice,
                amount_msat: amount_msat.map(CLN_Amount::from_msat),
                label: None,
                riskfactor: None,
                maxfeepercent: None,
                retry_for: None,
                maxdelay: None,
                exemptfee: None,
                localinvreqid: None,
                exclude: None,
                maxfee: max_fee_msat.map(CLN_Amount::from_msat),
                description: None,
                partial_msat: partial_amount.map(CLN_Amount::from_msat),
            })
            .await;

        let response = match cln_response {
            Ok(pay_response) => {
                let status = match pay_response.status {
                    PayStatus::COMPLETE => MeltQuoteState::Paid,
                    PayStatus::PENDING => MeltQuoteState::Pending,
                    PayStatus::FAILED => MeltQuoteState::Failed,
                };

                let total_spent_msat = pay_response.amount_sent_msat.msat();

                MakePaymentResponse {
                    payment_proof: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                    payment_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                    status,
                    total_spent: to_unit(total_spent_msat, &CurrencyUnit::Msat, unit)?,
                    unit: unit.clone(),
                }
            }
            Err(err) => {
                payment_ctx.log_error("Payment failed", &err);
                return Err(Error::ClnRpc(err).into());
            }
        };

        // Store final payment status
        let payment_status = PaymentStatus {
            status: response.status,
            payment_hash: hash,
            payment_proof: response.payment_proof.clone(),
            total_spent: response.total_spent,
        };
        self.database
            .store_outgoing_payment(quote_id, payment_status)
            .await?;

        payment_ctx.log_success(
            "Payment process completed",
            Some(response.total_spent.into()),
        );

        Ok(response)
    }

    #[instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
        quote_id: &QuoteId,
        unit: &CurrencyUnit,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        match options {
            IncomingPaymentOptions::Bolt11(Bolt11IncomingPaymentOptions {
                description,
                amount,
                unix_expiry,
            }) => {
                let time_now = unix_time();

                let mut cln_client = self.cln_client().await?;

                let label = Uuid::new_v4().to_string();

                let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;
                let amount_msat = AmountOrAny::Amount(CLN_Amount::from_msat(amount.into()));

                let invoice_response = cln_client
                    .call_typed(&InvoiceRequest {
                        amount_msat,
                        description: description.unwrap_or_default(),
                        label: label.clone(),
                        expiry: unix_expiry.map(|t| t - time_now),
                        fallbacks: None,
                        preimage: None,
                        cltv: None,
                        deschashonly: None,
                        exposeprivatechannels: None,
                    })
                    .await
                    .map_err(Error::from)?;

                let request = Bolt11Invoice::from_str(&invoice_response.bolt11)?;
                let expiry = request.expires_at().map(|t| t.as_secs());
                let payment_hash = request.payment_hash();

                // Store BOLT11 request mapping: payment_hash -> quote_id
                self.database
                    .store_incoming_bolt11_payment(payment_hash.as_ref(), quote_id)
                    .await?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                    request: request.to_string(),
                    expiry,
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
                        let amount = to_unit(amount, unit, &CurrencyUnit::Msat)?;

                        amount.to_string()
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
                        recurrence_start_any_period: None,
                    })
                    .await
                    .map_err(Error::from)?;

                // Store BOLT12 request mapping: local_offer_id -> quote_id
                self.database
                    .store_incoming_bolt12_payment(&offer_response.offer_id.to_string(), quote_id)
                    .await?;

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::QuoteId(quote_id.clone()),
                    request: offer_response.bolt12,
                    expiry: unix_expiry,
                })
            }
        }
    }

    #[instrument(skip(self))]
    async fn check_incoming_payment_status(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let mut cln_client = self.cln_client().await?;

        let listinvoices_response = match payment_identifier {
            PaymentIdentifier::QuoteId(quote_id) => {
                if let Some(lookup) = self
                    .database
                    .get_incoming_payment_identifier_by_quote_id(quote_id)
                    .await?
                {
                    match lookup {
                        IncomingPaymentIdentifier::Bolt11PaymentHash(payment_hash) => {
                            let payment_hash_identifier =
                                PaymentIdentifier::PaymentHash(payment_hash);
                            self.query_invoices_by_identifier(
                                &mut cln_client,
                                &payment_hash_identifier,
                            )
                            .await?
                        }
                        IncomingPaymentIdentifier::Bolt12OfferId(offer_id) => {
                            let offer_id_identifier = PaymentIdentifier::OfferId(offer_id);
                            self.query_invoices_by_identifier(&mut cln_client, &offer_id_identifier)
                                .await?
                        }
                    }
                } else {
                    tracing::error!("Unsupported payment id for CLN");
                    return Err(payment::Error::UnknownPaymentState);
                }
            }
            _ => {
                // For other identifiers, use the helper method directly
                self.query_invoices_by_identifier(&mut cln_client, payment_identifier)
                    .await
                    .map_err(|e| payment::Error::Custom(e.to_string()))?
            }
        };

        Ok(listinvoices_response
            .invoices
            .iter()
            .filter(|p| p.status == ListinvoicesInvoicesStatus::PAID)
            .filter(|p| p.amount_msat.is_some()) // Filter out invoices without an amount
            .map(|p| WaitPaymentResponse {
                payment_identifier: payment_identifier.clone(),
                payment_amount: p
                    .amount_msat
                    // Safe to expect since we filtered for Some
                    .expect("We have filter out those without amounts")
                    .msat()
                    .into(),
                unit: CurrencyUnit::Msat,
                payment_id: p.payment_hash.to_string(),
            })
            .collect())
    }

    #[instrument(skip(self))]
    async fn check_outgoing_payment(
        &self,
        payment_identifier: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        match payment_identifier {
            PaymentIdentifier::PaymentHash(hash) => {
                match self.check_outgoing_payment_by_hash(hash).await {
                    Ok(response) => Ok(MakePaymentResponse {
                        payment_lookup_id: payment_identifier.clone(),
                        payment_proof: response.payment_proof,
                        status: response.status,
                        total_spent: response.total_spent,
                        unit: CurrencyUnit::Msat,
                    }),
                    Err(e) => Err(e),
                }
            }
            PaymentIdentifier::Bolt12PaymentHash(hash) => {
                match self.check_outgoing_payment_by_hash(hash).await {
                    Ok(response) => Ok(MakePaymentResponse {
                        payment_lookup_id: payment_identifier.clone(),
                        payment_proof: response.payment_proof,
                        status: response.status,
                        total_spent: response.total_spent,
                        unit: CurrencyUnit::Msat,
                    }),
                    Err(e) => Err(e),
                }
            }
            PaymentIdentifier::QuoteId(quote_id) => {
                // Look up payment status from database using quote_id
                match self.database.load_outgoing_payment_status(quote_id).await {
                    Ok(Some(payment_status)) => {
                        // We have stored payment status, check for any updates from CLN
                        match self
                            .check_outgoing_payment_by_hash(&payment_status.payment_hash)
                            .await
                        {
                            Ok(current_status) => Ok(MakePaymentResponse {
                                payment_lookup_id: payment_identifier.clone(),
                                payment_proof: current_status.payment_proof,
                                status: current_status.status,
                                total_spent: current_status.total_spent,
                                unit: CurrencyUnit::Msat,
                            }),
                            Err(e) => Err(e),
                        }
                    }
                    Ok(None) => {
                        // No payment status found for this quote_id, means payment hasn't been attempted yet
                        Ok(MakePaymentResponse {
                            payment_lookup_id: payment_identifier.clone(),
                            payment_proof: None,
                            status: MeltQuoteState::Unpaid,
                            total_spent: Amount::ZERO,
                            unit: CurrencyUnit::Msat,
                        })
                    }
                    Err(e) => {
                        tracing::error!("Database error when checking quote payment: {}", e);
                        Err(payment::Error::Custom(e.to_string()))
                    }
                }
            }
            _ => {
                tracing::error!("Unsupported identifier to check outgoing payment for cln.");
                Err(payment::Error::UnknownPaymentState)
            }
        }
    }
}

impl Cln {
    async fn cln_client(&self) -> Result<ClnRpc, Error> {
        Ok(cln_rpc::ClnRpc::new(&self.rpc_socket).await?)
    }

    /// Calculate fee based on amount and fee reserve settings
    fn calculate_fee(&self, amount: Amount) -> Amount {
        let relative_fee_reserve =
            (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;
        let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();
        max(relative_fee_reserve, absolute_fee_reserve).into()
    }

    /// Helper to query invoices by different identifiers
    async fn query_invoices_by_identifier(
        &self,
        cln_client: &mut ClnRpc,
        identifier: &PaymentIdentifier,
    ) -> Result<cln_rpc::model::responses::ListinvoicesResponse, Error> {
        let request = match identifier {
            PaymentIdentifier::Label(label) => ListinvoicesRequest {
                payment_hash: None,
                label: Some(label.to_string()),
                invstring: None,
                offer_id: None,
                index: None,
                limit: None,
                start: None,
            },
            PaymentIdentifier::OfferId(offer_id) => ListinvoicesRequest {
                payment_hash: None,
                label: None,
                invstring: None,
                offer_id: Some(offer_id.to_string()),
                index: None,
                limit: None,
                start: None,
            },
            PaymentIdentifier::PaymentHash(payment_hash) => ListinvoicesRequest {
                payment_hash: Some(hex::encode(payment_hash)),
                label: None,
                invstring: None,
                offer_id: None,
                index: None,
                limit: None,
                start: None,
            },
            _ => return Err(Error::WrongClnResponse),
        };

        cln_client.call_typed(&request).await.map_err(Error::from)
    }

    /// Get last pay index for cln
    #[instrument(skip(self), fields(component = "cln"))]
    async fn get_last_pay_index(&self) -> Result<Option<u64>, Error> {
        if let Some(index) = self.database.load_last_pay_index().await? {
            tracing::debug!(pay_index = index, "Retrieved last pay index from KV store");
            return Ok(Some(index));
        }

        // Fall back to querying CLN directly
        tracing::debug!("No stored last pay index found in KV store, querying CLN directly");
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
    #[instrument(skip(self), fields(component = "cln"))]
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
    async fn check_outgoing_unpaided(&self, quote_id: &QuoteId) -> Result<(), payment::Error> {
        let pay_state = self.get_quote_payment_status(quote_id).await?;

        match pay_state {
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

    /// Check outgoing payment status by payment hash
    #[instrument(skip(self), fields(component = "cln", payment_hash = %hex::encode(payment_hash)))]
    async fn check_outgoing_payment_by_hash(
        &self,
        payment_hash: &[u8; 32],
    ) -> Result<PaymentStatus, payment::Error> {
        let mut cln_client = self.cln_client().await.map_err(|e| {
            tracing::error!(error = %e, "Failed to create CLN client");
            payment::Error::Custom(e.to_string())
        })?;

        let listpays_response = cln_client
            .call_typed(&ListpaysRequest {
                payment_hash: Some(*Sha256::from_bytes_ref(payment_hash)),
                bolt11: None,
                status: None,
                start: None,
                index: None,
                limit: None,
            })
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "ListpaysRequest failed");
                Error::from(e)
            })?;

        match listpays_response.pays.first() {
            Some(pays_response) => {
                let status = cln_pays_status_to_mint_state(pays_response.status);
                let payment_proof = pays_response.preimage.map(|p| hex::encode(p.to_vec()));
                let total_spent = pays_response
                    .amount_sent_msat
                    .map_or(Amount::ZERO, |a| a.msat().into());

                tracing::info!(
                    cln_status = ?pays_response.status,
                    mapped_status = ?status,
                    total_spent_msat = ?pays_response.amount_sent_msat,
                    total_spent = %total_spent,
                    has_preimage = pays_response.preimage.is_some(),
                    "Found payment with status"
                );

                Ok(PaymentStatus {
                    status,
                    payment_proof,
                    total_spent,
                    payment_hash: *payment_hash,
                })
            }
            None => {
                tracing::info!("No payment found, returning Unknown status");

                Ok(PaymentStatus {
                    status: MeltQuoteState::Unknown,
                    payment_proof: None,
                    total_spent: Amount::ZERO,
                    payment_hash: *payment_hash,
                })
            }
        }
    }

    /// Get quote payment status by checking database first, then CLN if payment hash exists
    #[instrument(skip(self), fields(component = "cln", quote_id = %quote_id))]
    async fn get_quote_payment_status(
        &self,
        quote_id: &QuoteId,
    ) -> Result<MeltQuoteState, payment::Error> {
        match self.database.load_outgoing_payment_status(quote_id).await {
            Ok(Some(payment_status)) => {
                // Payment status exists in database, check for updates with CLN
                match self
                    .check_outgoing_payment_by_hash(&payment_status.payment_hash)
                    .await
                {
                    Ok(response) => {
                        tracing::debug!(
                            status = ?response.status,
                            total_spent = %response.total_spent,
                            has_proof = response.payment_proof.is_some(),
                            "Retrieved payment status"
                        );
                        Ok(response.status)
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to check payment status");
                        Err(e)
                    }
                }
            }
            Ok(None) => {
                tracing::debug!("No payment hash found, quote is unpaid");
                Ok(MeltQuoteState::Unpaid)
            }
            Err(e) => {
                tracing::error!(error = %e, "Database error while checking payment hash");
                Err(payment::Error::Custom(e.to_string()))
            }
        }
    }
}

fn cln_pays_status_to_mint_state(status: ListpaysPaysStatus) -> MeltQuoteState {
    match status {
        ListpaysPaysStatus::PENDING => MeltQuoteState::Pending,
        ListpaysPaysStatus::COMPLETE => MeltQuoteState::Paid,
        ListpaysPaysStatus::FAILED => MeltQuoteState::Failed,
    }
}

#[instrument(skip(cln_client), fields(component = "cln", payment_hash = %payment_hash))]
async fn fetch_invoice_by_payment_hash(
    cln_client: &mut cln_rpc::ClnRpc,
    payment_hash: &Hash,
) -> Result<Option<ListinvoicesInvoices>, Error> {
    let request = ListinvoicesRequest {
        payment_hash: Some(payment_hash.to_string()),
        index: None,
        invstring: None,
        label: None,
        limit: None,
        offer_id: None,
        start: None,
    };

    match cln_client.call_typed(&request).await {
        Ok(invoice_response) => {
            let invoice_count = invoice_response.invoices.len();

            if invoice_count > 0 {
                let first_invoice = invoice_response.invoices.first().cloned();
                if let Some(invoice) = &first_invoice {
                    tracing::debug!(
                        local_offer_id = ?invoice.local_offer_id,
                        status = ?invoice.status,
                        "Found invoice"
                    );
                }
                Ok(first_invoice)
            } else {
                tracing::warn!("No invoices found");
                Ok(None)
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Error fetching invoice");
            Err(Error::from(e))
        }
    }
}
