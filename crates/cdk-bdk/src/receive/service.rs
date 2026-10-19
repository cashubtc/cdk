use std::str::FromStr;

use bdk_wallet::KeychainKind;
use cdk_common::payment::{Event, PaymentIdentifier, WaitPaymentResponse};
use cdk_common::{Amount, CurrencyUnit, QuoteId};

use crate::error::Error;
use crate::receive::receive_intent::{
    self, state as receive_state, ReceiveIntent, ReceiveIntentAny,
};
use crate::CdkBdk;

impl CdkBdk {
    pub(crate) async fn finalize_receive_intent_and_emit(
        &self,
        intent: ReceiveIntent<receive_state::Detected>,
    ) -> Result<(), Error> {
        let intent_id = intent.intent_id;
        let payment_amount = Amount::new(intent.state.amount_sat, CurrencyUnit::Sat);
        let quote_id = QuoteId::from_str(&intent.state.quote_id)
            .map_err(|_| Error::Wallet("Invalid QuoteId".to_string()))?;
        let outpoint = intent.state.outpoint.clone();

        intent.finalize(&self.storage).await.map_err(|e| {
            tracing::error!("Failed to finalize receive intent {}: {}", intent_id, e);
            e
        })?;

        let response = WaitPaymentResponse {
            payment_identifier: PaymentIdentifier::QuoteId(quote_id),
            payment_amount,
            payment_id: outpoint,
        };

        if let Err(err) = self.payment_sender.send(Event::PaymentReceived(response)) {
            tracing::error!(
                "Could not send payment received event for receive intent {}: {}",
                intent_id,
                err
            );
        }

        Ok(())
    }

    pub(crate) async fn confirmed_receive_intents_from_record(
        &self,
        persisted_intents: &[receive_intent::record::ReceiveIntentRecord],
    ) -> Vec<ReceiveIntent<receive_state::Detected>> {
        let mut confirmed = Vec::new();
        for persisted in persisted_intents {
            let ReceiveIntentAny::Detected(intent) = receive_intent::from_record(persisted);
            if self
                .txid_has_required_confirmations(
                    &intent.state.txid,
                    "receive_intent",
                    &intent.intent_id.to_string(),
                )
                .await
            {
                confirmed.push(intent);
            }
        }
        confirmed
    }

    pub(crate) async fn scan_for_new_payments(&self) -> Result<(), Error> {
        let tracked_addresses = self.storage.get_tracked_receive_addresses().await?;
        tracing::debug!(
            tracked_address_count = tracked_addresses.len(),
            "Scanning wallet for tracked onchain receive addresses"
        );
        if tracked_addresses.is_empty() {
            tracing::debug!("No tracked receive addresses found, skipping wallet output scan");
            return Ok(());
        }

        let address_set: std::collections::HashSet<String> =
            tracked_addresses.into_iter().collect();

        let utxos = self
            .wallet_engine
            .confirmed_outputs()
            .await
            .into_iter()
            .filter_map(|o| {
                if o.keychain != KeychainKind::External {
                    return None;
                }
                address_set.contains(&o.address).then_some((
                    o.address,
                    o.txid,
                    o.outpoint,
                    o.amount_sat,
                    o.block_height,
                ))
            })
            .collect::<Vec<_>>();

        for (address, txid, outpoint, amount_sat, block_height) in utxos {
            if self.should_ignore_receive_amount(amount_sat) {
                tracing::trace!(
                    address,
                    txid,
                    outpoint,
                    amount_sat,
                    min_receive_amount_sat = self.min_receive_amount_sat,
                    "Ignoring tracked receive UTXO below configured minimum amount"
                );
                continue;
            }

            match ReceiveIntent::new(
                &self.storage,
                address,
                txid,
                outpoint.clone(),
                amount_sat,
                block_height,
            )
            .await
            {
                Ok(Some(intent)) => {
                    tracing::debug!(
                        address = %intent.state.address,
                        txid = %intent.state.txid,
                        amount_sat = intent.state.amount_sat,
                        "Created receive intent {} for outpoint {} during wallet scan",
                        intent.intent_id,
                        outpoint
                    );
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::error!(
                        "Failed to create receive intent for outpoint {} during wallet scan: {}",
                        outpoint,
                        err
                    );
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn finalize_receive_intents(
        &self,
        intents: Vec<ReceiveIntent<receive_state::Detected>>,
    ) -> Result<(), Error> {
        for intent in intents {
            self.finalize_receive_intent_and_emit(intent).await?;
        }

        Ok(())
    }

    pub(crate) async fn check_receive_saga_confirmations(&self) -> Result<(), Error> {
        let all_persisted = self.storage.get_all_receive_intents().await?;

        if all_persisted.is_empty() {
            return Ok(());
        }

        let to_finalize = self
            .confirmed_receive_intents_from_record(&all_persisted)
            .await;

        self.finalize_receive_intents(to_finalize).await
    }
}
