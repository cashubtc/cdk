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
use cdk_common::Bolt11Invoice;
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
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uuid::Uuid;

pub mod error;

// KV Store constants for CLN
const CLN_KV_PRIMARY_NAMESPACE: &str = "cdk_cln_lightning_backend";
const CLN_KV_SECONDARY_NAMESPACE: &str = "payment_indices";
const LAST_PAY_INDEX_KV_KEY: &str = "last_pay_index";

/// CLN mint backend
#[derive(Clone)]
pub struct Cln {
    rpc_socket: PathBuf,
    fee_reserve: FeeReserve,
    wait_invoice_cancel_token: CancellationToken,
    wait_invoice_is_active: Arc<AtomicBool>,
    kv_store: DynMintKVStore,
}

impl Cln {
    /// Create new [`Cln`]
    pub async fn new(
        rpc_socket: PathBuf,
        fee_reserve: FeeReserve,
        kv_store: DynMintKVStore,
    ) -> Result<Self, Error> {
        Ok(Self {
            rpc_socket,
            fee_reserve,
            wait_invoice_cancel_token: CancellationToken::new(),
            wait_invoice_is_active: Arc::new(AtomicBool::new(false)),
            kv_store,
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

                            let payment_hash = Hash::from_bytes_ref(payment_hash.as_ref());

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
                                payment_amount: amount_msats.msat().into(),
                                unit: CurrencyUnit::Msat,
                                payment_id: payment_hash.to_string()
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

                // Calculate fee
                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;
                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();
                let fee = max(relative_fee_reserve, absolute_fee_reserve);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                        *bolt11_options.bolt11.payment_hash().as_ref(),
                    )),
                    amount,
                    fee: fee.into(),
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

                // Calculate fee
                let relative_fee_reserve =
                    (self.fee_reserve.percent_fee_reserve * u64::from(amount) as f32) as u64;
                let absolute_fee_reserve: u64 = self.fee_reserve.min_fee_reserve.into();
                let fee = max(relative_fee_reserve, absolute_fee_reserve);

                Ok(PaymentQuoteResponse {
                    request_lookup_id: None,
                    amount,
                    fee: fee.into(),
                    state: MeltQuoteState::Unpaid,
                    unit: unit.clone(),
                })
            }
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

        let mut cln_client = self.cln_client().await?;

        let invoice = match &options {
            OutgoingPaymentOptions::Bolt11(bolt11_options) => {
                let payment_identifier =
                    PaymentIdentifier::PaymentHash(*bolt11_options.bolt11.payment_hash().as_ref());

                self.check_outgoing_unpaided(&payment_identifier).await?;

                if let Some(melt_options) = bolt11_options.melt_options {
                    match melt_options {
                        MeltOptions::Mpp { mpp } => partial_amount = Some(mpp.amount.into()),
                        MeltOptions::Amountless { amountless } => {
                            amount_msat = Some(amountless.amount_msat.into());
                        }
                    }
                }

                max_fee_msat = bolt11_options.max_fee_amount.map(|a| a.into());

                bolt11_options.bolt11.to_string()
            }
            OutgoingPaymentOptions::Bolt12(bolt12_options) => {
                let offer = &bolt12_options.offer;

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

                let decode_response = self.decode_string(cln_response.invoice.clone()).await?;

                let payment_identifier = PaymentIdentifier::Bolt12PaymentHash(
                    hex::decode(
                        decode_response
                            .invoice_payment_hash
                            .ok_or(Error::UnknownInvoice)?,
                    )
                    .map_err(|e| Error::Bolt12(e.to_string()))?
                    .try_into()
                    .map_err(|_| Error::InvalidHash)?,
                );

                self.check_outgoing_unpaided(&payment_identifier).await?;

                max_fee_msat = bolt12_options.max_fee_amount.map(|a| a.into());

                cln_response.invoice
            }
        };

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

                let payment_identifier = match options {
                    OutgoingPaymentOptions::Bolt11(_) => {
                        PaymentIdentifier::PaymentHash(*pay_response.payment_hash.as_ref())
                    }
                    OutgoingPaymentOptions::Bolt12(_) => {
                        PaymentIdentifier::Bolt12PaymentHash(*pay_response.payment_hash.as_ref())
                    }
                };

                MakePaymentResponse {
                    payment_proof: Some(hex::encode(pay_response.payment_preimage.to_vec())),
                    payment_lookup_id: payment_identifier,
                    status,
                    total_spent: to_unit(
                        pay_response.amount_sent_msat.msat(),
                        &CurrencyUnit::Msat,
                        unit,
                    )?,
                    unit: unit.clone(),
                }
            }
            Err(err) => {
                tracing::error!("Could not pay invoice: {}", err);
                return Err(Error::ClnRpc(err).into());
            }
        };

        Ok(response)
    }

    #[instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
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

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::PaymentHash(*payment_hash.as_ref()),
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

                Ok(CreateIncomingPaymentResponse {
                    request_lookup_id: PaymentIdentifier::OfferId(
                        offer_response.offer_id.to_string(),
                    ),
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
        let mut cln_client = self.cln_client().await?;

        let payment_hash = match payment_identifier {
            PaymentIdentifier::PaymentHash(hash) => hash,
            PaymentIdentifier::Bolt12PaymentHash(hash) => hash,
            _ => {
                tracing::error!("Unsupported identifier to check outgoing payment for cln.");
                return Err(payment::Error::UnknownPaymentState);
            }
        };

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
            .map_err(Error::from)?;

        match listpays_response.pays.first() {
            Some(pays_response) => {
                let status = cln_pays_status_to_mint_state(pays_response.status);

                Ok(MakePaymentResponse {
                    payment_lookup_id: payment_identifier.clone(),
                    payment_proof: pays_response.preimage.map(|p| hex::encode(p.to_vec())),
                    status,
                    total_spent: pays_response
                        .amount_sent_msat
                        .map_or(Amount::ZERO, |a| a.msat().into()),
                    unit: CurrencyUnit::Msat,
                })
            }
            None => Ok(MakePaymentResponse {
                payment_lookup_id: payment_identifier.clone(),
                payment_proof: None,
                status: MeltQuoteState::Unknown,
                total_spent: Amount::ZERO,
                unit: CurrencyUnit::Msat,
            }),
        }
    }
}

impl Cln {
    async fn cln_client(&self) -> Result<ClnRpc, Error> {
        Ok(cln_rpc::ClnRpc::new(&self.rpc_socket).await?)
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

fn cln_pays_status_to_mint_state(status: ListpaysPaysStatus) -> MeltQuoteState {
    match status {
        ListpaysPaysStatus::PENDING => MeltQuoteState::Pending,
        ListpaysPaysStatus::COMPLETE => MeltQuoteState::Paid,
        ListpaysPaysStatus::FAILED => MeltQuoteState::Failed,
    }
}

async fn fetch_invoice_by_payment_hash(
    cln_client: &mut cln_rpc::ClnRpc,
    payment_hash: &Hash,
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
