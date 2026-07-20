//! Deterministic materialized-state projection for federation convergence.

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use bitcoin::hashes::{sha256, Hash};
use cdk_common::error::Error;
use cdk_common::federation::{
    ark_server_commitment, FederatedKeyset, FederationBarkActionKind,
    FederationBarkObservationStatus, FederationBarkOperation, FederationBarkProjectionSnapshot,
    FederationBarkReceiveIntent, FederationBarkSemanticPhase, FederationBarkVtxoState,
    FederationMaterializedStateKey, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, FederationOperation, FederationOperationId,
    FederationPaymentObservationRecord,
};
use cdk_common::mint::Operation;
use cdk_common::{Amount, PublicKey};
use tracing::instrument;
use uuid::Uuid;

use crate::mint::{Mint, QuoteId};

const MATERIALIZED_STATE_QUERY_BATCH_SIZE: usize = 500;

impl Mint {
    /// Load the canonical Bark action/VTXO projection for route orchestration and workers.
    #[instrument(skip_all)]
    pub async fn federation_bark_projection_snapshot(
        &self,
    ) -> Result<FederationBarkProjectionSnapshot, Error> {
        Ok(self
            .localstore()
            .get_federation_bark_projection_snapshot()
            .await?)
    }

    /// Load one applied receive intent by its accepted operation ID.
    #[instrument(skip_all)]
    pub async fn federation_bark_receive_intent(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationBarkReceiveIntent>, Error> {
        let Some(entry) = self
            .localstore()
            .get_federation_journal_entry_by_operation_id(operation_id)
            .await?
        else {
            return Ok(None);
        };
        let FederationOperation::Bark(FederationBarkOperation::ReceiveIntent(intent)) =
            entry.envelope.operation
        else {
            return Ok(None);
        };
        Ok(Some(intent))
    }

    /// Validate ordered-state Bark preconditions that cannot be checked from
    /// one operation in isolation.
    #[instrument(skip_all)]
    pub async fn validate_federation_bark_operation_submission(
        &self,
        operation: &FederationBarkOperation,
    ) -> Result<(), Error> {
        operation.validate()?;
        let config = self
            .effective_federation_config()
            .await?
            .ok_or(Error::Internal)?;
        let application = config
            .public_config()
            .frost_wallet
            .as_ref()
            .and_then(|wallet| wallet.bark_application.as_ref())
            .ok_or(Error::Internal)?;
        let snapshot = self.federation_bark_projection_snapshot().await?;
        let action = snapshot
            .actions
            .iter()
            .find(|action| action.action_id == operation.action_id());

        match operation {
            FederationBarkOperation::MemberObservation(observation) => {
                let action = action.ok_or_else(|| bark_state_error("observation has no action"))?;
                if action.quote.as_deref() != observation.quote.as_deref()
                    || action.payment_hash != observation.payment_hash
                    || observation.source_scope
                        != ark_server_commitment(&application.ark_server_public_key)?
                {
                    return Err(bark_state_error(
                        "observation differs from its action or Ark source",
                    ));
                }
                if observation.status == FederationBarkObservationStatus::Succeeded {
                    let FederationBarkOperation::ActionTransition(transition) = &action.operation
                    else {
                        return Err(bark_state_error(
                            "successful observation does not follow an action result",
                        ));
                    };
                    let amount = transition
                        .vtxo_transitions
                        .iter()
                        .filter(|mutation| {
                            mutation.previous_state.is_none()
                                && mutation.new_state == FederationBarkVtxoState::Spendable
                        })
                        .try_fold(Amount::ZERO, |sum, mutation| {
                            sum.checked_add(mutation.amount).ok_or_else(|| {
                                bark_state_error("claimed output observation amount overflow")
                            })
                        })?;
                    if action.phase != FederationBarkSemanticPhase::ReceiveClaimed
                        || observation.result_hash != transition.result_hash
                        || observation.amount != amount
                        || observation.fee != Amount::ZERO
                    {
                        return Err(bark_state_error(
                            "successful observation differs from the accepted claim result",
                        ));
                    }
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ReceiveHtlcsReady =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("HTLC evidence has no action"))?;
                if action.phase != FederationBarkSemanticPhase::ReceivePrepared {
                    return Err(bark_state_error(
                        "HTLC evidence does not follow receive preparation",
                    ));
                }
                let intent = self
                    .federation_bark_receive_intent(action.origin_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("receive intent is missing"))?;
                let amount = transition.vtxo_transitions.iter().try_fold(
                    Amount::ZERO,
                    |sum, mutation| {
                        sum.checked_add(mutation.amount)
                            .ok_or_else(|| bark_state_error("HTLC evidence amount overflow"))
                    },
                )?;
                let fee = intent
                    .gross_amount
                    .checked_sub(amount)
                    .ok_or_else(|| bark_state_error("HTLC evidence exceeds invoice amount"))?;
                if amount < intent.minimum_net_amount || fee > intent.receive_fee {
                    return Err(bark_state_error(
                        "HTLC evidence does not cover the accepted Cashu liability",
                    ));
                }
                let mut observers = BTreeSet::new();
                for record in &snapshot.observations {
                    let FederationBarkOperation::MemberObservation(observation) = &record.operation
                    else {
                        continue;
                    };
                    if observation.action_id == transition.action_id
                        && observation.quote == transition.quote
                        && observation.payment_hash == transition.payment_hash
                        && observation.source_scope
                            == ark_server_commitment(&application.ark_server_public_key)?
                        && observation.status == FederationBarkObservationStatus::Accepted
                        && observation.amount == amount
                        && observation.fee == fee
                        && observation.result_hash == transition.result_hash
                    {
                        observers.insert(observation.observer);
                    }
                }
                if observers.len()
                    < usize::from(config.public_config().threshold_params.signature_threshold)
                {
                    return Err(bark_state_error(
                        "HTLC evidence has not reached the accepted member threshold",
                    ));
                }
            }
            FederationBarkOperation::ActionPlan(plan)
                if plan.kind == FederationBarkActionKind::IncomingClaim =>
            {
                let action = action.ok_or_else(|| bark_state_error("claim plan has no action"))?;
                if action.phase != FederationBarkSemanticPhase::ReceiveHtlcsReady
                    || plan.signer_set.len() < usize::from(application.threshold)
                    || plan
                        .signer_set
                        .iter()
                        .any(|member| !config.public_config().members.contains_key(member))
                {
                    return Err(bark_state_error(
                        "claim plan is not backed by ready HTLCs and an eligible signer set",
                    ));
                }
                for input in &plan.inputs {
                    let vtxo = snapshot
                        .vtxos
                        .iter()
                        .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("claim plan input is unknown"))?;
                    if vtxo.state != FederationBarkVtxoState::Observed
                        || vtxo.amount != input.amount
                        || vtxo.locator
                            != self
                                .federation_bark_receive_intent(action.origin_operation_id)
                                .await?
                                .ok_or_else(|| bark_state_error("receive intent is missing"))?
                                .destination
                    {
                        return Err(bark_state_error(
                            "claim plan input differs from accepted HTLC evidence",
                        ));
                    }
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ReceivePreimageDisclosed =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("preimage disclosure has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted claim plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error("claim transition references a non-plan"));
                };
                let transitioned = transition
                    .vtxo_transitions
                    .iter()
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let planned = plan
                    .inputs
                    .iter()
                    .map(|input| input.vtxo_id)
                    .collect::<BTreeSet<_>>();
                if action.phase != FederationBarkSemanticPhase::ClaimPlanned
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || transitioned != planned
                {
                    return Err(bark_state_error(
                        "preimage disclosure differs from the accepted claim plan",
                    ));
                }
                for input in &plan.inputs {
                    let vtxo = snapshot
                        .vtxos
                        .iter()
                        .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                        .ok_or_else(|| bark_state_error("reserved claim input is missing"))?;
                    if vtxo.state != FederationBarkVtxoState::Reserved
                        || vtxo.reservation_action_id != Some(plan.action_id)
                        || vtxo.amount != input.amount
                    {
                        return Err(bark_state_error(
                            "preimage disclosure input is not reserved by its claim",
                        ));
                    }
                }
            }
            FederationBarkOperation::ActionTransition(transition)
                if transition.phase == FederationBarkSemanticPhase::ReceiveClaimed =>
            {
                let action =
                    action.ok_or_else(|| bark_state_error("claimed receive has no action"))?;
                let plan_entry = self
                    .localstore()
                    .get_federation_journal_entry_by_operation_id(transition.plan_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("accepted claim plan is missing"))?;
                let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
                    &plan_entry.envelope.operation
                else {
                    return Err(bark_state_error("claim result references a non-plan"));
                };
                let intent = self
                    .federation_bark_receive_intent(action.origin_operation_id)
                    .await?
                    .ok_or_else(|| bark_state_error("receive intent is missing"))?;
                let spent = transition
                    .vtxo_transitions
                    .iter()
                    .filter(|mutation| {
                        mutation.previous_state == Some(FederationBarkVtxoState::PendingSpend)
                            && mutation.new_state == FederationBarkVtxoState::Spent
                    })
                    .map(|mutation| mutation.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let planned = plan
                    .inputs
                    .iter()
                    .map(|input| input.vtxo_id)
                    .collect::<BTreeSet<_>>();
                let output = plan
                    .outputs
                    .first()
                    .ok_or_else(|| bark_state_error("claim plan has no output allocation"))?;
                let mut spendable_sat = 0_u64;
                for mutation in transition.vtxo_transitions.iter().filter(|mutation| {
                    mutation.previous_state.is_none()
                        && mutation.new_state == FederationBarkVtxoState::Spendable
                }) {
                    if mutation.locator != output.locator {
                        return Err(bark_state_error(
                            "claim result uses an unallocated output locator",
                        ));
                    }
                    spendable_sat = spendable_sat
                        .checked_add(u64::from(mutation.amount))
                        .ok_or_else(|| bark_state_error("claim result amount overflow"))?;
                }
                if action.phase != FederationBarkSemanticPhase::ReceivePreimageDisclosed
                    || transition.predecessor_operation_id != action.latest_operation_id
                    || spent != planned
                    || spendable_sat < u64::from(intent.cashu_amount)
                    || spendable_sat > u64::from(output.amount)
                {
                    return Err(bark_state_error(
                        "claim result does not prove sufficient accepted spendable value",
                    ));
                }
            }
            FederationBarkOperation::ReceiveIntent(_)
            | FederationBarkOperation::ReceivePrepared(_)
            | FederationBarkOperation::ActionPlan(_)
            | FederationBarkOperation::ActionTransition(_) => {}
        }
        Ok(())
    }

    /// Build the deterministic federation materialized-state snapshot.
    #[instrument(skip_all)]
    pub async fn federation_materialized_state_snapshot(
        &self,
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
        for row in self
            .federation_address_allocation_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        for row in self
            .federation_chain_observation_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        for row in self
            .federation_transaction_state()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        for row in db
            .get_federation_bark_projection_snapshot()
            .await?
            .materialized_rows()?
        {
            snapshot.push(row);
        }
        let mut mint_quotes = BTreeSet::new();
        let mut melt_quotes = BTreeSet::new();
        let mut completed_operations = BTreeSet::new();
        let mut proof_ys = BTreeSet::new();
        let mut blinded_messages = BTreeSet::new();
        let mut keyset_rotations = Vec::new();

        for record in db.get_federation_materialized_state_keys().await? {
            match record.key {
                FederationMaterializedStateKey::MintQuote { quote } => {
                    mint_quotes.insert(quote);
                }
                FederationMaterializedStateKey::MeltQuote { quote } => {
                    melt_quotes.insert(quote);
                }
                FederationMaterializedStateKey::CompletedOperation => {
                    completed_operations.insert(record.operation_id);
                }
                FederationMaterializedStateKey::ProofState { y } => {
                    proof_ys.insert(y);
                }
                FederationMaterializedStateKey::BlindSignature { blinded_message } => {
                    blinded_messages.insert(blinded_message);
                }
                FederationMaterializedStateKey::KeysetRotation { keyset } => {
                    keyset_rotations.push((record.operation_index, record.operation_id, keyset));
                }
                FederationMaterializedStateKey::BarkAction { .. }
                | FederationMaterializedStateKey::BarkVtxo { .. }
                | FederationMaterializedStateKey::BarkDerivationAllocation { .. }
                | FederationMaterializedStateKey::BarkReservation { .. }
                | FederationMaterializedStateKey::BarkTransition { .. }
                | FederationMaterializedStateKey::BarkObservation { .. } => {}
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

        for record in db.get_all_federation_payment_observations().await? {
            snapshot.push(payment_observation_materialized_row(&record)?);
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

fn bark_state_error(reason: &str) -> Error {
    Error::Custom(format!("invalid federated Bark state: {reason}"))
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
