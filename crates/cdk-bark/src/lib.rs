//! Bark/Ark payment backend for [`cdk`].
//!
//! Implements [`MintPayment`] on top of [`bark`], routing outgoing
//! payments over Ark (arkoor) when the destination supports it, and
//! falling back to the Ark-to-Lightning gateway otherwise.

#![forbid(unsafe_code)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bark::actions::lightning::pay::LightningSendState;
use bark::movement::{Movement, MovementStatus, PaymentMethod};
use bark::subsystem::Subsystem;
use ark::lightning::PaymentHash;
use ark::Address as ArkAddress;
use bark::{Wallet as BarkWallet, WalletNotification};
use bitcoin::Amount as BarkAmount;
use cdk_common::amount::Amount;
use cdk_common::common::FeeReserve;
use cdk_common::nuts::{CurrencyUnit, MeltQuoteState};
use cdk_common::payment::{
    self, Bolt11Settings, CreateIncomingPaymentResponse, Event, IncomingPaymentOptions,
    MakePaymentResponse, MintPayment, OutgoingPaymentOptions, PaymentIdentifier,
    PaymentQuoteResponse, SettingsResponse, WaitPaymentResponse,
};
use futures::{Stream, StreamExt};
use tokio::sync::Mutex;
use tracing::instrument;

pub mod error;

use error::Error;

/// A [`MintPayment`] backend backed by a single [`bark::Wallet`].
#[derive(Clone)]
pub struct BarkMintPayment {
    wallet: Arc<BarkWallet>,
    #[allow(dead_code)] // reserved for a future fee floor in get_payment_quote
    fee_reserve: FeeReserve,
    unit: CurrencyUnit,
    stream_taken: Arc<Mutex<bool>>,
}

impl std::fmt::Debug for BarkMintPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BarkMintPayment")
            .field("unit", &self.unit)
            .finish_non_exhaustive()
    }
}

impl BarkMintPayment {
    /// Wraps an already-opened [`bark::Wallet`] for use as a mint payment
    /// backend. Wallet construction (seed, [`bark::Config`], persister)
    /// is the caller's responsibility.
    pub fn new(wallet: Arc<BarkWallet>, fee_reserve: FeeReserve, unit: CurrencyUnit) -> Self {
        Self {
            wallet,
            fee_reserve,
            unit,
            stream_taken: Arc::new(Mutex::new(false)),
        }
    }

    /// Resolves a payment request string to an Ark-native destination,
    /// if one is available.
    async fn resolve_arkoor_destination(&self, request: &str) -> Option<ArkAddress> {
        let parsed = self.wallet.parse_payment_request(request).await.ok()?;
        parsed.options.into_iter().find_map(|opt| match opt.method {
            PaymentMethod::Ark(addr) => Some(addr),
            _ => None,
        })
    }
}

#[async_trait]
impl MintPayment for BarkMintPayment {
    type Err = payment::Error;

    #[instrument(skip_all)]
    async fn get_settings(&self) -> Result<SettingsResponse, Self::Err> {
        Ok(SettingsResponse {
            unit: self.unit.to_string(),
            bolt11: Some(Bolt11Settings {
                mpp: false,
                amountless: false,
                invoice_description: false,
            }),
            bolt12: None,
            onchain: None,
            custom: Default::default(),
        })
    }

    #[instrument(skip_all)]
    fn is_payment_event_stream_active(&self) -> bool {
        true
    }

    #[instrument(skip_all)]
    fn cancel_payment_event_stream(&self) {}

    #[instrument(skip_all)]
    async fn wait_payment_event(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Event> + Send>>, Self::Err> {
        let mut taken = self.stream_taken.lock().await;
        if *taken {
            return Err(Error::StreamAlreadyTaken.into());
        }

        let notifications = self.wallet.subscribe_notifications();
        *taken = true;

        let stream = notifications.filter_map(|notification| async move {
            match notification {
                WalletNotification::MovementUpdated { movement } => {
                    movement_to_payment_event(&movement)
                }
                WalletNotification::MovementCreated { .. } => None,
                WalletNotification::ChannelLagging => {
                    tracing::warn!("bark notification channel lagged; events may be missing");
                    None
                }
            }
        });

        Ok(Box::pin(stream))
    }

    #[instrument(skip_all)]
    async fn get_payment_quote(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<PaymentQuoteResponse, Self::Err> {
        let OutgoingPaymentOptions::Bolt11(bolt11_options) = options else {
            return Err(payment::Error::UnsupportedPaymentOption);
        };

        let invoice = &bolt11_options.bolt11;
        let amount_sat = invoice
            .amount_milli_satoshis()
            .ok_or(Error::UnknownInvoiceAmount)?
            / 1000;
        let amount = Amount::new(amount_sat, unit.clone());

        let fee = match self.resolve_arkoor_destination(&invoice.to_string()).await {
            Some(_) => Amount::new(0, unit.clone()),
            None => {
                let bark_amount = BarkAmount::from_sat(amount_sat);
                let estimate = self
                    .wallet
                    .estimate_lightning_send_fee(bark_amount)
                    .await
                    .map_err(Error::Bark)?;
                Amount::new(estimate.fee.to_sat(), unit.clone())
            }
        };

        Ok(PaymentQuoteResponse {
            request_lookup_id: Some(PaymentIdentifier::PaymentHash(
                *invoice.payment_hash().as_ref(),
            )),
            amount,
            fee,
            state: MeltQuoteState::Unpaid,
            extra_json: None,
            estimated_blocks: None,
            fee_options: None,
        })
    }

    #[instrument(skip_all)]
    async fn make_payment(
        &self,
        unit: &CurrencyUnit,
        options: OutgoingPaymentOptions,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let OutgoingPaymentOptions::Bolt11(bolt11_options) = options else {
            return Err(payment::Error::UnsupportedPaymentOption);
        };

        let invoice = bolt11_options.bolt11;
        let payment_hash = *invoice.payment_hash();
        let amount_sat = invoice
            .amount_milli_satoshis()
            .ok_or(Error::UnknownInvoiceAmount)?
            / 1000;

        if let Some(ark_addr) = self.resolve_arkoor_destination(&invoice.to_string()).await {
            let bark_amount = BarkAmount::from_sat(amount_sat);
            self.wallet
                .send_arkoor_payment(&ark_addr, bark_amount)
                .await
                .map_err(Error::Bark)?;

            return Ok(MakePaymentResponse {
                payment_lookup_id: PaymentIdentifier::PaymentHash(*payment_hash.as_ref()),
                payment_proof: Some(ark_addr.to_string()),
                status: MeltQuoteState::Paid,
                total_spent: Amount::new(amount_sat, unit.clone()),
            });
        }

        self.wallet
            .pay_lightning_invoice(invoice, None, false)
            .await
            .map_err(Error::Bark)?;

        Ok(MakePaymentResponse {
            payment_lookup_id: PaymentIdentifier::PaymentHash(*payment_hash.as_ref()),
            payment_proof: None,
            status: MeltQuoteState::Pending,
            total_spent: Amount::new(0, unit.clone()),
        })
    }

    #[instrument(skip_all)]
    async fn create_incoming_payment_request(
        &self,
        options: IncomingPaymentOptions,
    ) -> Result<CreateIncomingPaymentResponse, Self::Err> {
        let IncomingPaymentOptions::Bolt11(bolt11_options) = options else {
            return Err(payment::Error::UnsupportedPaymentOption);
        };

        let amount_sat = bolt11_options.amount.value();
        let bark_amount = BarkAmount::from_sat(amount_sat);

        let invoice = self
            .wallet
            .bolt11_invoice(bark_amount, bolt11_options.description)
            .await
            .map_err(Error::Bark)?;

        Ok(CreateIncomingPaymentResponse {
            request_lookup_id: PaymentIdentifier::PaymentHash(
                *invoice.payment_hash().as_ref(),
            ),
            request: invoice.to_string(),
            expiry: bolt11_options.unix_expiry,
            extra_json: None,
        })
    }

    #[instrument(skip_all)]
    async fn check_incoming_payment_status(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {
        let PaymentIdentifier::PaymentHash(hash) = request_lookup_id else {
            return Ok(vec![]);
        };
        let payment_hash = PaymentHash::from(*hash);

        if let Some(receive) = self
            .wallet
            .lightning_receive_status(payment_hash)
            .await
            .map_err(Error::Bark)?
        {
            if receive.preimage_revealed_at.is_none() {
                return Ok(vec![]);
            }
            let amount_sat = receive
                .invoice
                .amount_milli_satoshis()
                .map(|msat| msat / 1000)
                .unwrap_or(0);

            return Ok(vec![WaitPaymentResponse {
                payment_identifier: request_lookup_id.clone(),
                payment_amount: Amount::new(amount_sat, CurrencyUnit::Sat),
                payment_id: receive
                    .movement_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| payment_hash.to_string()),
            }]);
        }

        Ok(vec![])
    }

    #[instrument(skip_all)]
    async fn check_outgoing_payment(
        &self,
        request_lookup_id: &PaymentIdentifier,
    ) -> Result<MakePaymentResponse, Self::Err> {
        let PaymentIdentifier::PaymentHash(hash) = request_lookup_id else {
            return Err(Error::UnsupportedIdentifier.into());
        };
        let payment_hash = PaymentHash::from(*hash);

        let send_state = self
            .wallet
            .check_lightning_payment(payment_hash, false)
            .await
            .map_err(Error::Bark)?;

        let response = match send_state {
            LightningSendState::Paid(paid) => MakePaymentResponse {
                payment_lookup_id: request_lookup_id.clone(),
                payment_proof: Some(paid.preimage.to_string()),
                status: MeltQuoteState::Paid,
                total_spent: Amount::new(0, self.unit.clone()),
            },
            LightningSendState::InProgress(_) => MakePaymentResponse {
                payment_lookup_id: request_lookup_id.clone(),
                payment_proof: None,
                status: MeltQuoteState::Pending,
                total_spent: Amount::new(0, self.unit.clone()),
            },
            LightningSendState::Unknown => MakePaymentResponse {
                payment_lookup_id: request_lookup_id.clone(),
                payment_proof: None,
                status: MeltQuoteState::Unknown,
                total_spent: Amount::new(0, self.unit.clone()),
            },
        };

        Ok(response)
    }
}

fn movement_to_payment_event(movement: &Movement) -> Option<Event> {
    if movement.status != MovementStatus::Successful {
        return None;
    }
    if movement.subsystem.kind != "receive" {
        return None;
    }
    if !movement.subsystem.is_subsystem(Subsystem::LIGHTNING_RECEIVE)
        && !movement.subsystem.is_subsystem(Subsystem::ARKOOR)
    {
        return None;
    }

    let effective_sat = movement.effective_balance.to_sat();
    if effective_sat <= 0 {
        return None;
    }

    let payment_identifier = match movement.lightning_payment_hash() {
        Some(hash) => PaymentIdentifier::PaymentHash(hash.to_byte_array()),
        None => PaymentIdentifier::CustomId(movement.id.to_string()),
    };

    Some(Event::PaymentReceived(WaitPaymentResponse {
        payment_identifier,
        payment_amount: Amount::new(effective_sat as u64, CurrencyUnit::Sat),
        payment_id: movement.id.to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use bark::movement::{MovementId, MovementSubsystem};
    use chrono::Local;

    use super::*;

    fn subsystem(name: &str, kind: &str) -> MovementSubsystem {
        MovementSubsystem {
            name: name.to_string(),
            kind: kind.to_string(),
        }
    }

    #[test]
    fn ignores_non_successful_movements() {
        let mut movement = test_movement();
        movement.status = MovementStatus::Pending;
        assert!(movement_to_payment_event(&movement).is_none());
    }

    #[test]
    fn ignores_send_movements() {
        let mut movement = test_movement();
        movement.subsystem = subsystem("bark.lightning_send", "send");
        assert!(movement_to_payment_event(&movement).is_none());
    }

    fn test_movement() -> Movement {
        Movement::new(
            MovementId(1),
            MovementStatus::Successful,
            &subsystem("bark.arkoor", "receive"),
            Local::now(),
        )
    }
}

