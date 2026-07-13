//! Deterministic materialized-state projection for federation convergence.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash};
use cdk_common::error::Error;
use cdk_common::federation::{
    FederatedKeyset, FederationJournalEntry, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, FederationOperation, FederationOperationId,
    FederationOperationStatus, FederationPaymentObservationContext,
    FederationPaymentObservationRecord,
};
use cdk_common::mint::Operation;
use cdk_common::{Amount, PublicKey};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::{Mint, QuoteId};

const MATERIALIZED_STATE_QUERY_BATCH_SIZE: usize = 500;

impl Mint {
    /// Build the deterministic federation materialized-state snapshot.
    #[instrument(skip_all)]
    pub async fn federation_materialized_state_snapshot(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<FederationMaterializedStateSnapshot, Error> {
        let db = self.localstore();
        let mut snapshot = FederationMaterializedStateSnapshot::new();
        for root in db.get_federation_frost_root_public_configs().await? {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_frost_root_public_config",
                    root.root_key_epoch.to_string(),
                )
                .with_field("payload_json", serde_json::to_string(&root)?),
            );
        }
        for application in db.get_federation_frost_application_configs().await? {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_frost_application_config",
                    format!(
                        "{}:{}",
                        application.application_id, application.application_key_epoch
                    ),
                )
                .with_field("payload_json", serde_json::to_string(&application)?),
            );
        }
        for consumer in db.get_federation_frost_consumer_registrations().await? {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_frost_consumer_registration",
                    format!(
                        "{}:{}:{}",
                        consumer.root_key_epoch,
                        consumer.application_id,
                        consumer.application_key_epoch
                    ),
                )
                .with_field("payload_json", serde_json::to_string(&consumer)?),
            );
        }
        if let Some(active_epochs) = db.get_federation_frost_active_epochs().await? {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "federation_frost_active_epochs",
                    "active".to_string(),
                )
                .with_field("payload_json", serde_json::to_string(&active_epochs)?),
            );
        }
        for row in self
            .federation_address_allocation_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        let mut mint_quotes = BTreeSet::new();
        let mut melt_quotes = BTreeSet::new();
        let mut payment_observation_quotes = BTreeSet::new();
        let mut payment_quarantines = BTreeSet::new();
        let mut completed_operations = BTreeSet::new();
        let mut proof_ys = BTreeSet::new();
        let mut blinded_messages = BTreeSet::new();
        let mut keyset_rotations = Vec::new();

        for entry in entries {
            if let Some(rejection) = entry.rejection() {
                snapshot.push(
                    FederationMaterializedStateRow::new(
                        "federation_operation_rejections",
                        entry.operation_id.to_string(),
                    )
                    .with_field("operation_index", entry.index.to_string())
                    .with_field("error_code", rejection.code.to_string())
                    .with_field("detail", rejection.detail.clone()),
                );
            }
            if entry.status() == FederationOperationStatus::Rejected {
                continue;
            }

            match &entry.envelope.operation {
                FederationOperation::AddressAllocation(_) => {}
                FederationOperation::MintQuote(operation) => {
                    mint_quotes.insert(operation.quote.clone());
                }
                FederationOperation::MintQuotePayment(observation) => {
                    mint_quotes.insert(observation.quote.clone());
                    payment_observation_quotes.insert((
                        FederationPaymentObservationContext::MintQuote,
                        observation.quote.clone(),
                    ));
                }
                FederationOperation::Mint(operation) => {
                    completed_operations.insert(entry.operation_id);
                    mint_quotes.extend(
                        operation
                            .request
                            .quote_ids()
                            .into_iter()
                            .map(str::to_string),
                    );
                    match &operation.request {
                        cdk_common::federation::FederationMintRequest::Single(request) => {
                            blinded_messages
                                .extend(request.outputs.iter().map(|output| output.blinded_secret));
                        }
                        cdk_common::federation::FederationMintRequest::Batch(request) => {
                            blinded_messages
                                .extend(request.outputs.iter().map(|output| output.blinded_secret));
                        }
                    }
                }
                FederationOperation::MeltQuote(operation) => {
                    melt_quotes.insert(operation.quote.clone());
                }
                FederationOperation::MeltQuotePayment(observation) => {
                    melt_quotes.insert(observation.quote.clone());
                    payment_observation_quotes.insert((
                        FederationPaymentObservationContext::MeltQuote,
                        observation.quote.clone(),
                    ));
                }
                FederationOperation::PaymentObservationQuarantine(operation) => {
                    payment_observation_quotes.insert((operation.context, operation.quote.clone()));
                    payment_quarantines.insert((operation.context, operation.quote.clone()));
                    match operation.context {
                        FederationPaymentObservationContext::MintQuote => {
                            mint_quotes.insert(operation.quote.clone());
                        }
                        FederationPaymentObservationContext::MeltQuote => {
                            melt_quotes.insert(operation.quote.clone());
                        }
                    }
                }
                FederationOperation::Melt(operation) => {
                    completed_operations.insert(entry.operation_id);
                    melt_quotes.insert(operation.request.quote().clone());
                    for proof in operation.request.inputs() {
                        proof_ys.insert(proof.y()?);
                    }
                    if let Some(outputs) = operation.request.outputs() {
                        blinded_messages.extend(outputs.iter().map(|output| output.blinded_secret));
                    }
                }
                FederationOperation::Swap(operation) => {
                    completed_operations.insert(entry.operation_id);
                    for proof in operation.request.inputs() {
                        proof_ys.insert(proof.y()?);
                    }
                    blinded_messages.extend(
                        operation
                            .request
                            .outputs()
                            .iter()
                            .map(|output| output.blinded_secret),
                    );
                }
                FederationOperation::KeysetRotation(operation) => {
                    keyset_rotations.push((
                        entry.index,
                        entry.operation_id,
                        operation.keyset.clone(),
                    ));
                }
            }
        }

        let completed_operation_ids = completed_operations
            .into_iter()
            .map(|operation_id| (operation_id, federation_operation_uuid(operation_id)))
            .collect::<Vec<_>>();
        let completed_operation_uuids = completed_operation_ids
            .iter()
            .map(|(_, operation_uuid)| *operation_uuid)
            .collect::<Vec<_>>();
        let mut completed_by_id = HashMap::new();
        for operation_ids in completed_operation_uuids.chunks(MATERIALIZED_STATE_QUERY_BATCH_SIZE) {
            completed_by_id.extend(
                db.get_federation_completed_operations(operation_ids)
                    .await?
                    .into_iter()
                    .map(|operation| (*operation.id(), operation)),
            );
        }
        for (operation_id, operation_uuid) in completed_operation_ids {
            match completed_by_id.remove(&operation_uuid) {
                Some(operation) => snapshot.push(completed_operation_materialized_row(
                    operation_id,
                    operation_uuid,
                    &operation,
                )),
                None => snapshot.push(
                    FederationMaterializedStateRow::new(
                        "completed_operations",
                        operation_uuid.to_string(),
                    )
                    .with_field("federation_operation_id", operation_id.to_string())
                    .with_field("missing", "true"),
                ),
            }
        }

        for (operation_index, operation_id, keyset) in keyset_rotations {
            snapshot.push(federation_keyset_materialized_row(
                operation_index,
                operation_id,
                &keyset,
            )?);
        }

        let mint_quote_ids = mint_quotes
            .iter()
            .map(|quote| QuoteId::from_str(quote))
            .collect::<Result<Vec<_>, _>>()?;
        let mut stored_mint_quotes = Vec::with_capacity(mint_quote_ids.len());
        for quote_ids in mint_quote_ids.chunks(MATERIALIZED_STATE_QUERY_BATCH_SIZE) {
            stored_mint_quotes.extend(db.get_federation_mint_quote_states(quote_ids).await?);
        }
        for (quote, stored_quote) in mint_quotes.into_iter().zip(stored_mint_quotes) {
            match stored_quote {
                Some(quote) => snapshot.push(
                    FederationMaterializedStateRow::new("mint_quotes", quote.id.to_string())
                        .with_field("amount", optional_amount_value(quote.amount.as_ref()))
                        .with_field("unit", quote.unit.to_string())
                        .with_field("request", quote.request.clone())
                        .with_field("expiry", quote.expiry.to_string())
                        .with_field("amount_paid", quote.amount_paid.value().to_string())
                        .with_field("amount_issued", quote.amount_issued.value().to_string())
                        .with_field("state", quote.state.to_string())
                        .with_field("payment_method", quote.payment_method.to_string()),
                ),
                None => snapshot.push(
                    FederationMaterializedStateRow::new("mint_quotes", quote)
                        .with_field("missing", "true"),
                ),
            }
        }

        let melt_quote_ids = melt_quotes
            .iter()
            .map(|quote| QuoteId::from_str(quote))
            .collect::<Result<Vec<_>, _>>()?;
        let mut stored_melt_quotes = HashMap::new();
        for quote_ids in melt_quote_ids.chunks(MATERIALIZED_STATE_QUERY_BATCH_SIZE) {
            stored_melt_quotes.extend(
                db.get_federation_melt_quotes(quote_ids)
                    .await?
                    .into_iter()
                    .map(|quote| (quote.id.to_string(), quote)),
            );
        }
        for quote in melt_quotes {
            match stored_melt_quotes.remove(&quote) {
                Some(quote) => snapshot.push(
                    FederationMaterializedStateRow::new("melt_quotes", quote.id.to_string())
                        .with_field("amount", quote.amount().value().to_string())
                        .with_field("fee_reserve", quote.fee_reserve().value().to_string())
                        .with_field("unit", quote.unit.to_string())
                        .with_field("request", quote.request.to_string())
                        .with_field("state", quote.state.to_string())
                        .with_field("payment_method", quote.payment_method.to_string())
                        .with_field("payment_proof", quote.payment_proof.unwrap_or_default()),
                ),
                None => snapshot.push(
                    FederationMaterializedStateRow::new("melt_quotes", quote)
                        .with_field("missing", "true"),
                ),
            }
        }

        for (context, quote) in payment_quarantines {
            snapshot.push(
                FederationMaterializedStateRow::new(
                    "payment_observation_quarantines",
                    format!("{context}:{quote}"),
                )
                .with_field("context", context.to_string())
                .with_field("quote", quote),
            );
        }

        let mut observations_by_quote = BTreeMap::new();
        for record in db.get_all_federation_payment_observations().await? {
            let key = (
                record.observation.context(),
                record.observation.quote().to_string(),
            );
            if payment_observation_quotes.contains(&key) {
                observations_by_quote
                    .entry(key)
                    .or_insert_with(Vec::new)
                    .push(record);
            }
        }
        for (context, quote) in payment_observation_quotes {
            let records = observations_by_quote
                .remove(&(context, quote.clone()))
                .unwrap_or_default();
            if records.is_empty() {
                snapshot.push(
                    FederationMaterializedStateRow::new(
                        "payment_observations",
                        format!("{context}:{quote}"),
                    )
                    .with_field("missing", "true"),
                );
                continue;
            }

            for record in records {
                snapshot.push(payment_observation_materialized_row(&record)?);
            }
        }

        if !proof_ys.is_empty() {
            append_proof_state_rows(&mut snapshot, db.as_ref(), proof_ys).await?;
        }

        if !blinded_messages.is_empty() {
            append_blind_signature_rows(&mut snapshot, db.as_ref(), blinded_messages).await?;
        }

        Ok(snapshot)
    }
}

async fn append_proof_state_rows(
    snapshot: &mut FederationMaterializedStateSnapshot,
    db: &(dyn cdk_common::database::MintDatabase<cdk_common::database::Error> + Send + Sync),
    proof_ys: BTreeSet<PublicKey>,
) -> Result<(), Error> {
    let proof_ys = proof_ys.into_iter().collect::<Vec<_>>();
    let proof_states = db.get_proofs_states(&proof_ys).await?;
    for (y, state) in proof_ys.into_iter().zip(proof_states) {
        let mut row = FederationMaterializedStateRow::new("proof_states", y.to_hex());
        match state {
            Some(state) => row.insert_field("state", state.to_string()),
            None => row.insert_field("missing", "true"),
        }
        snapshot.push(row);
    }
    Ok(())
}

async fn append_blind_signature_rows(
    snapshot: &mut FederationMaterializedStateSnapshot,
    db: &(dyn cdk_common::database::MintDatabase<cdk_common::database::Error> + Send + Sync),
    blinded_messages: BTreeSet<PublicKey>,
) -> Result<(), Error> {
    let blinded_messages = blinded_messages.into_iter().collect::<Vec<_>>();
    let blind_signatures = db.get_blind_signatures(&blinded_messages).await?;
    for (blinded_message, signature) in blinded_messages.into_iter().zip(blind_signatures) {
        let mut row =
            FederationMaterializedStateRow::new("blind_signatures", blinded_message.to_hex());
        match signature {
            Some(signature) => {
                row.insert_field("amount", signature.amount.to_string());
                row.insert_field("keyset_id", signature.keyset_id.to_string());
            }
            None => row.insert_field("missing", "true"),
        }
        snapshot.push(row);
    }
    Ok(())
}

fn federation_operation_uuid(operation_id: FederationOperationId) -> Uuid {
    let bytes = operation_id.to_bytes();
    let mut uuid = [0_u8; 16];
    uuid.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(uuid)
}

fn completed_operation_materialized_row(
    operation_id: FederationOperationId,
    operation_uuid: Uuid,
    operation: &Operation,
) -> FederationMaterializedStateRow {
    FederationMaterializedStateRow::new("completed_operations", operation_uuid.to_string())
        .with_field("federation_operation_id", operation_id.to_string())
        .with_field("kind", operation.kind().to_string())
        .with_field(
            "total_issued",
            operation.total_issued().to_u64().to_string(),
        )
        .with_field(
            "total_redeemed",
            operation.total_redeemed().to_u64().to_string(),
        )
        .with_field(
            "fee_collected",
            operation.fee_collected().to_u64().to_string(),
        )
        .with_field(
            "completed_at",
            operation
                .completed_at()
                .map(|completed_at| completed_at.to_string())
                .unwrap_or_default(),
        )
        .with_field(
            "payment_method",
            operation
                .payment_method()
                .map(|method| method.to_string())
                .unwrap_or_default(),
        )
}

fn federation_keyset_materialized_row(
    operation_index: u64,
    operation_id: FederationOperationId,
    keyset: &FederatedKeyset,
) -> Result<FederationMaterializedStateRow, Error> {
    Ok(
        FederationMaterializedStateRow::new("federation_keysets", keyset.id.to_string())
            .with_field("operation_index", operation_index.to_string())
            .with_field("operation_id", operation_id.to_string())
            .with_field("unit", keyset.unit.to_string())
            .with_field("active", keyset.active.to_string())
            .with_field("input_fee_ppk", keyset.input_fee_ppk.to_string())
            .with_field(
                "final_expiry",
                keyset
                    .final_expiry
                    .map(|expiry| expiry.to_string())
                    .unwrap_or_default(),
            )
            .with_field(
                "aggregate_keys",
                serde_json::to_string(&keyset.aggregate_keys)?,
            )
            .with_field(
                "public_shares",
                serde_json::to_string(&keyset.public_shares)?,
            ),
    )
}

fn optional_amount_value(amount: Option<&Amount<cdk_common::CurrencyUnit>>) -> String {
    amount
        .map(|amount| amount.value().to_string())
        .unwrap_or_default()
}

fn payment_observation_materialized_row(
    record: &FederationPaymentObservationRecord,
) -> Result<FederationMaterializedStateRow, Error> {
    let key = format!(
        "{}:{}:{}:{}",
        record.observation.context(),
        record.observation.quote(),
        record.operation_index,
        record.operation_id
    );
    let canonical = serde_json::to_vec(&record.observation)?;
    Ok(
        FederationMaterializedStateRow::new("payment_observations", key)
            .with_field("context", record.observation.context().to_string())
            .with_field("quote", record.observation.quote().to_string())
            .with_field("operation_index", record.operation_index.to_string())
            .with_field("operation_id", record.operation_id.to_string())
            .with_field("observer", record.observation.observer().to_string())
            .with_field("state", record.observation.state_label())
            .with_field("evidence_hash", sha256::Hash::hash(&canonical).to_string()),
    )
}
