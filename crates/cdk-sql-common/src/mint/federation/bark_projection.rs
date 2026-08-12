use super::*;

fn invalid_projection(reason: impl Into<String>) -> Error {
    cdk_common::federation::FederationError::OperationStateInvalid {
        reason: reason.into(),
    }
    .into()
}

fn sql_i64(label: &str, value: u64) -> Result<i64, Error> {
    i64::try_from(value)
        .map_err(|_| invalid_projection(format!("Bark {label} exceeds the SQL integer range")))
}

fn phase_label(phase: cdk_federation_bark::FederationBarkSemanticPhase) -> &'static str {
    use cdk_federation_bark::FederationBarkSemanticPhase as Phase;
    match phase {
        Phase::ReceiveIntent => "receive_intent",
        Phase::ReceivePrepared => "receive_prepared",
        Phase::SendPlanned => "send_planned",
        Phase::ClaimPlanned => "claim_planned",
        Phase::RevocationPlanned => "revocation_planned",
        Phase::RefreshPlanned => "refresh_planned",
        Phase::ExitPlanned => "exit_planned",
        Phase::SigningAttemptAdvanced => "signing_attempt_advanced",
        Phase::ReceiveHtlcsReady => "receive_htlcs_ready",
        Phase::ReceivePreimageDisclosed => "receive_preimage_disclosed",
        Phase::ReceiveClaimed => "receive_claimed",
        Phase::SendHtlcsCosigned => "send_htlcs_cosigned",
        Phase::PaymentSucceeded => "payment_succeeded",
        Phase::PaymentFailed => "payment_failed",
        Phase::RevocationCompleted => "revocation_completed",
        Phase::RefreshCompleted => "refresh_completed",
        Phase::ExitRegistered => "exit_registered",
        Phase::ExitPublished => "exit_published",
        Phase::ExitConfirmed => "exit_confirmed",
        Phase::Observation => "observation",
    }
}

fn action_kind_label(
    kind: Option<cdk_federation_bark::FederationBarkActionKind>,
) -> Option<&'static str> {
    use cdk_federation_bark::FederationBarkActionKind as Kind;
    kind.map(|kind| match kind {
        Kind::LightningSend => "lightning_send",
        Kind::IncomingClaim => "incoming_claim",
        Kind::FailedSendRevocation => "failed_send_revocation",
        Kind::Refresh => "refresh",
        Kind::EmergencyExit => "emergency_exit",
    })
}

fn vtxo_state_label(state: FederationBarkVtxoState) -> &'static str {
    match state {
        FederationBarkVtxoState::Observed => "observed",
        FederationBarkVtxoState::Spendable => "spendable",
        FederationBarkVtxoState::Reserved => "reserved",
        FederationBarkVtxoState::PendingSpend => "pending_spend",
        FederationBarkVtxoState::Spent => "spent",
        FederationBarkVtxoState::Exiting => "exiting",
        FederationBarkVtxoState::Exited => "exited",
    }
}

async fn get_action<C>(
    connection: &C,
    action_id: cdk_federation_bark::FederationBarkActionId,
) -> Result<Option<FederationBarkActionRecord>, Error>
where
    C: DatabaseExecutor,
{
    query(
        r#"
        SELECT record_json
        FROM federation_bark_actions
        WHERE action_id = :action_id
        "#,
    )?
    .bind("action_id", action_id.to_string())
    .fetch_one(connection)
    .await?
    .map(|row| decode_record(row, "Bark action"))
    .transpose()
}

async fn projection_operation_exists<C>(
    connection: &C,
    operation_id: FederationOperationId,
) -> Result<bool, Error>
where
    C: DatabaseExecutor,
{
    Ok(query(
        r#"
        SELECT operation_id
        FROM federation_bark_transitions
        WHERE operation_id = :operation_id
        UNION ALL
        SELECT operation_id
        FROM federation_bark_observations
        WHERE operation_id = :operation_id
        UNION ALL
        SELECT origin_operation_id
        FROM federation_bark_actions
        WHERE origin_operation_id = :operation_id
        "#,
    )?
    .bind("operation_id", operation_id.to_string())
    .fetch_one(connection)
    .await?
    .is_some())
}

async fn ensure_action_bindings_unique<C>(
    connection: &C,
    action: &FederationBarkActionRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    for (column, value) in [
        ("quote", action.quote.clone()),
        (
            "payment_hash",
            action
                .payment_hash
                .map(|payment_hash| payment_hash.to_string()),
        ),
    ] {
        let Some(value) = value else {
            continue;
        };
        let sql = match column {
            "quote" => {
                r#"
                SELECT action_id FROM federation_bark_actions
                WHERE quote = :value AND action_id <> :action_id
                "#
            }
            "payment_hash" => {
                r#"
                SELECT action_id FROM federation_bark_actions
                WHERE payment_hash = :value AND action_id <> :action_id
                "#
            }
            _ => return Err(Error::Internal("Invalid Bark binding column".to_string())),
        };
        if query(sql)?
            .bind("value", value)
            .bind("action_id", action.action_id.to_string())
            .fetch_one(connection)
            .await?
            .is_some()
        {
            return Err(invalid_projection(format!(
                "Bark {column} is already owned by another action"
            )));
        }
    }
    Ok(())
}

async fn insert_action<C>(connection: &C, action: &FederationBarkActionRecord) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    ensure_action_bindings_unique(connection, action).await?;
    query(
        r#"
        INSERT INTO federation_bark_actions
        (action_id, quote, payment_hash, action_kind, phase, actor_member_id, attempt,
         origin_operation_id, origin_operation_index, latest_operation_id,
         latest_operation_index, plan_operation_id, predecessor_operation_id,
         operation_commitment, record_json)
        VALUES
        (:action_id, :quote, :payment_hash, :action_kind, :phase, :actor_member_id, :attempt,
         :origin_operation_id, :origin_operation_index, :latest_operation_id,
         :latest_operation_index, :plan_operation_id, :predecessor_operation_id,
         :operation_commitment, :record_json)
        "#,
    )?
    .bind("action_id", action.action_id.to_string())
    .bind("quote", action.quote.clone())
    .bind(
        "payment_hash",
        action
            .payment_hash
            .map(|payment_hash| payment_hash.to_string()),
    )
    .bind("action_kind", action_kind_label(action.action_kind))
    .bind("phase", phase_label(action.phase))
    .bind("actor_member_id", i64::from(action.actor.get()))
    .bind("attempt", i64::from(action.attempt))
    .bind(
        "origin_operation_id",
        action.origin_operation_id.to_string(),
    )
    .bind(
        "origin_operation_index",
        sql_i64("origin operation index", action.origin_operation_index)?,
    )
    .bind(
        "latest_operation_id",
        action.latest_operation_id.to_string(),
    )
    .bind(
        "latest_operation_index",
        sql_i64("latest operation index", action.latest_operation_index)?,
    )
    .bind(
        "plan_operation_id",
        action
            .plan_operation_id
            .map(|operation_id| operation_id.to_string()),
    )
    .bind(
        "predecessor_operation_id",
        action
            .predecessor_operation_id
            .map(|operation_id| operation_id.to_string()),
    )
    .bind(
        "operation_commitment",
        action.operation_commitment.to_string(),
    )
    .bind("record_json", serde_json::to_string(action)?)
    .execute(connection)
    .await?;
    Ok(())
}

async fn update_action<C>(
    connection: &C,
    previous: &FederationBarkActionRecord,
    action: &FederationBarkActionRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    ensure_action_bindings_unique(connection, action).await?;
    let rows_affected = query(
        r#"
        UPDATE federation_bark_actions
        SET quote = :quote,
            payment_hash = :payment_hash,
            action_kind = :action_kind,
            phase = :phase,
            actor_member_id = :actor_member_id,
            attempt = :attempt,
            latest_operation_id = :latest_operation_id,
            latest_operation_index = :latest_operation_index,
            plan_operation_id = :plan_operation_id,
            predecessor_operation_id = :predecessor_operation_id,
            operation_commitment = :operation_commitment,
            record_json = :record_json
        WHERE action_id = :action_id
          AND latest_operation_id = :expected_latest_operation_id
        "#,
    )?
    .bind("quote", action.quote.clone())
    .bind(
        "payment_hash",
        action
            .payment_hash
            .map(|payment_hash| payment_hash.to_string()),
    )
    .bind("action_kind", action_kind_label(action.action_kind))
    .bind("phase", phase_label(action.phase))
    .bind("actor_member_id", i64::from(action.actor.get()))
    .bind("attempt", i64::from(action.attempt))
    .bind(
        "latest_operation_id",
        action.latest_operation_id.to_string(),
    )
    .bind(
        "latest_operation_index",
        sql_i64("latest operation index", action.latest_operation_index)?,
    )
    .bind(
        "plan_operation_id",
        action
            .plan_operation_id
            .map(|operation_id| operation_id.to_string()),
    )
    .bind(
        "predecessor_operation_id",
        action
            .predecessor_operation_id
            .map(|operation_id| operation_id.to_string()),
    )
    .bind(
        "operation_commitment",
        action.operation_commitment.to_string(),
    )
    .bind("record_json", serde_json::to_string(action)?)
    .bind("action_id", action.action_id.to_string())
    .bind(
        "expected_latest_operation_id",
        previous.latest_operation_id.to_string(),
    )
    .execute(connection)
    .await?;
    if rows_affected != 1 {
        return Err(invalid_projection("Bark action head changed concurrently"));
    }
    Ok(())
}

fn ensure_same_action_binding(
    action: &FederationBarkActionRecord,
    operation: &FederationBarkOperation,
) -> Result<(), Error> {
    if action.action_id != operation.action_id() {
        return Err(invalid_projection(
            "Bark action ID changed within one lineage",
        ));
    }
    if let Some(quote) = operation.quote() {
        if action.quote.as_deref() != Some(quote) {
            return Err(invalid_projection(
                "Bark action quote does not match its lineage",
            ));
        }
    }
    if let Some(payment_hash) = operation.payment_hash() {
        if action.payment_hash != Some(payment_hash) {
            return Err(invalid_projection(
                "Bark action payment hash does not match its lineage",
            ));
        }
    }
    Ok(())
}

fn ensure_predecessor(
    action: &FederationBarkActionRecord,
    predecessor: FederationOperationId,
) -> Result<(), Error> {
    if action.latest_operation_id != predecessor {
        return Err(invalid_projection(
            "Bark operation does not name the current action predecessor",
        ));
    }
    Ok(())
}

fn planned_phase(
    kind: cdk_federation_bark::FederationBarkActionKind,
) -> cdk_federation_bark::FederationBarkSemanticPhase {
    use cdk_federation_bark::{
        FederationBarkActionKind as Kind, FederationBarkSemanticPhase as Phase,
    };
    match kind {
        Kind::LightningSend => Phase::SendPlanned,
        Kind::IncomingClaim => Phase::ClaimPlanned,
        Kind::FailedSendRevocation => Phase::RevocationPlanned,
        Kind::Refresh => Phase::RefreshPlanned,
        Kind::EmergencyExit => Phase::ExitPlanned,
    }
}

fn direct_transition_allowed(
    previous: cdk_federation_bark::FederationBarkSemanticPhase,
    next: cdk_federation_bark::FederationBarkSemanticPhase,
) -> bool {
    use cdk_federation_bark::FederationBarkSemanticPhase as Phase;
    matches!(
        (previous, next),
        (Phase::ReceiveIntent, Phase::ReceivePrepared)
            | (Phase::ReceivePrepared, Phase::ReceiveHtlcsReady)
            | (Phase::ReceiveHtlcsReady, Phase::ClaimPlanned)
            | (Phase::ClaimPlanned, Phase::ReceivePreimageDisclosed)
            | (Phase::ReceivePreimageDisclosed, Phase::ReceiveClaimed)
            | (Phase::SendPlanned, Phase::SendHtlcsCosigned)
            | (
                Phase::SendHtlcsCosigned,
                Phase::PaymentSucceeded | Phase::PaymentFailed
            )
            | (Phase::PaymentFailed, Phase::RevocationPlanned)
            | (Phase::RevocationPlanned, Phase::RevocationCompleted)
            | (Phase::RefreshPlanned, Phase::ExitPlanned)
            | (Phase::RefreshPlanned, Phase::RefreshCompleted)
            | (Phase::ExitPlanned, Phase::ExitRegistered)
            | (Phase::ExitRegistered, Phase::ExitPublished)
            | (Phase::ExitPublished, Phase::ExitConfirmed)
    )
}

fn transition_allowed(
    action: &FederationBarkActionRecord,
    next: cdk_federation_bark::FederationBarkSemanticPhase,
) -> bool {
    use cdk_federation_bark::FederationBarkSemanticPhase as Phase;

    let retry_phase = match action.phase {
        Phase::SigningAttemptAdvanced => match action.signing_retry_phase {
            Some(phase) => phase,
            None => return false,
        },
        phase => phase,
    };
    match next {
        Phase::SigningAttemptAdvanced => matches!(
            retry_phase,
            Phase::ReceiveIntent
                | Phase::ReceivePrepared
                | Phase::ReceiveHtlcsReady
                | Phase::ClaimPlanned
                | Phase::ReceivePreimageDisclosed
                | Phase::SendPlanned
                | Phase::SendHtlcsCosigned
                | Phase::PaymentFailed
                | Phase::RevocationPlanned
                | Phase::RefreshPlanned
                | Phase::ExitPlanned
                | Phase::ExitRegistered
                | Phase::ExitPublished
        ),
        _ => direct_transition_allowed(retry_phase, next),
    }
}

async fn insert_allocation<C>(
    connection: &C,
    allocation: &FederationBarkDerivationAllocationRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let rows_affected = query(
        r#"
        INSERT INTO federation_bark_derivation_allocations
        (locator_role, locator_index, locator_version, action_id, amount, artifact_hash,
         vtxo_id, operation_id, operation_index, record_json)
        VALUES
        (:locator_role, :locator_index, :locator_version, :action_id, :amount, :artifact_hash,
         :vtxo_id, :operation_id, :operation_index, :record_json)
        ON CONFLICT (locator_role, locator_index, locator_version)
        DO UPDATE SET record_json = EXCLUDED.record_json
        WHERE federation_bark_derivation_allocations.record_json = EXCLUDED.record_json
        "#,
    )?
    .bind("locator_role", i64::from(allocation.locator.role))
    .bind("locator_index", i64::from(allocation.locator.index))
    .bind("locator_version", i64::from(allocation.locator.version))
    .bind("action_id", allocation.action_id.to_string())
    .bind(
        "amount",
        sql_i64("allocation amount", u64::from(allocation.amount))?,
    )
    .bind(
        "artifact_hash",
        allocation.artifact_hash.map(|hash| hash.to_string()),
    )
    .bind(
        "vtxo_id",
        allocation.vtxo_id.map(|vtxo_id| vtxo_id.to_string()),
    )
    .bind("operation_id", allocation.operation_id.to_string())
    .bind(
        "operation_index",
        sql_i64("allocation operation index", allocation.operation_index)?,
    )
    .bind("record_json", serde_json::to_string(allocation)?)
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        return Err(invalid_projection(
            "Bark derivation locator is already allocated differently",
        ));
    }
    Ok(())
}

async fn get_allocation<C>(
    connection: &C,
    locator: cdk_federation_bark::FederationBarkVtxoLocator,
) -> Result<Option<FederationBarkDerivationAllocationRecord>, Error>
where
    C: DatabaseExecutor,
{
    query(
        r#"
        SELECT record_json
        FROM federation_bark_derivation_allocations
        WHERE locator_role = :locator_role
          AND locator_index = :locator_index
          AND locator_version = :locator_version
        "#,
    )?
    .bind("locator_role", i64::from(locator.role))
    .bind("locator_index", i64::from(locator.index))
    .bind("locator_version", i64::from(locator.version))
    .fetch_one(connection)
    .await?
    .map(|row| decode_record(row, "Bark derivation allocation"))
    .transpose()
}

async fn materialize_allocation<C>(
    connection: &C,
    previous: &FederationBarkDerivationAllocationRecord,
    updated: &FederationBarkDerivationAllocationRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let rows_affected = query(
        r#"
        UPDATE federation_bark_derivation_allocations
        SET amount = :amount,
            artifact_hash = :artifact_hash,
            vtxo_id = :vtxo_id,
            operation_id = :operation_id,
            operation_index = :operation_index,
            record_json = :record_json
        WHERE locator_role = :locator_role
          AND locator_index = :locator_index
          AND locator_version = :locator_version
          AND record_json = :expected_record_json
        "#,
    )?
    .bind(
        "amount",
        sql_i64("allocation amount", u64::from(updated.amount))?,
    )
    .bind(
        "artifact_hash",
        updated.artifact_hash.map(|hash| hash.to_string()),
    )
    .bind(
        "vtxo_id",
        updated.vtxo_id.map(|vtxo_id| vtxo_id.to_string()),
    )
    .bind("operation_id", updated.operation_id.to_string())
    .bind(
        "operation_index",
        sql_i64("allocation operation index", updated.operation_index)?,
    )
    .bind("record_json", serde_json::to_string(updated)?)
    .bind("locator_role", i64::from(updated.locator.role))
    .bind("locator_index", i64::from(updated.locator.index))
    .bind("locator_version", i64::from(updated.locator.version))
    .bind("expected_record_json", serde_json::to_string(previous)?)
    .execute(connection)
    .await?;
    if rows_affected != 1 {
        return Err(invalid_projection(
            "Bark locator allocation changed concurrently",
        ));
    }
    Ok(())
}

async fn get_vtxo<C>(
    connection: &C,
    vtxo_id: cdk_federation_bark::FederationBarkVtxoId,
) -> Result<Option<FederationBarkVtxoRecord>, Error>
where
    C: DatabaseExecutor,
{
    query("SELECT record_json FROM federation_bark_vtxos WHERE vtxo_id = :vtxo_id")?
        .bind("vtxo_id", vtxo_id.to_string())
        .fetch_one(connection)
        .await?
        .map(|row| decode_record(row, "Bark VTXO"))
        .transpose()
}

async fn insert_vtxo<C>(connection: &C, vtxo: &FederationBarkVtxoRecord) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    query(
        r#"
        INSERT INTO federation_bark_vtxos
        (vtxo_id, locator_role, locator_index, locator_version, amount, state, artifact_hash,
         reservation_action_id, origin_operation_id, origin_operation_index,
         latest_operation_id, latest_operation_index, record_json)
        VALUES
        (:vtxo_id, :locator_role, :locator_index, :locator_version, :amount, :state, :artifact_hash,
         :reservation_action_id, :origin_operation_id, :origin_operation_index,
         :latest_operation_id, :latest_operation_index, :record_json)
        "#,
    )?
    .bind("vtxo_id", vtxo.vtxo_id.to_string())
    .bind("locator_role", i64::from(vtxo.locator.role))
    .bind("locator_index", i64::from(vtxo.locator.index))
    .bind("locator_version", i64::from(vtxo.locator.version))
    .bind("amount", sql_i64("VTXO amount", u64::from(vtxo.amount))?)
    .bind("state", vtxo_state_label(vtxo.state))
    .bind("artifact_hash", vtxo.artifact.hash.to_string())
    .bind(
        "reservation_action_id",
        vtxo.reservation_action_id
            .map(|action_id| action_id.to_string()),
    )
    .bind("origin_operation_id", vtxo.origin_operation_id.to_string())
    .bind(
        "origin_operation_index",
        sql_i64("VTXO origin operation index", vtxo.origin_operation_index)?,
    )
    .bind("latest_operation_id", vtxo.latest_operation_id.to_string())
    .bind(
        "latest_operation_index",
        sql_i64("VTXO latest operation index", vtxo.latest_operation_index)?,
    )
    .bind("record_json", serde_json::to_string(vtxo)?)
    .execute(connection)
    .await?;
    Ok(())
}

async fn update_vtxo<C>(
    connection: &C,
    previous: &FederationBarkVtxoRecord,
    updated: &FederationBarkVtxoRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let rows_affected = query(
        r#"
        UPDATE federation_bark_vtxos
        SET state = :state,
            reservation_action_id = :reservation_action_id,
            latest_operation_id = :latest_operation_id,
            latest_operation_index = :latest_operation_index,
            record_json = :record_json
        WHERE vtxo_id = :vtxo_id
          AND record_json = :expected_record_json
        "#,
    )?
    .bind("state", vtxo_state_label(updated.state))
    .bind(
        "reservation_action_id",
        updated
            .reservation_action_id
            .map(|action_id| action_id.to_string()),
    )
    .bind(
        "latest_operation_id",
        updated.latest_operation_id.to_string(),
    )
    .bind(
        "latest_operation_index",
        sql_i64(
            "VTXO latest operation index",
            updated.latest_operation_index,
        )?,
    )
    .bind("record_json", serde_json::to_string(updated)?)
    .bind("vtxo_id", updated.vtxo_id.to_string())
    .bind("expected_record_json", serde_json::to_string(previous)?)
    .execute(connection)
    .await?;
    if rows_affected != 1 {
        return Err(invalid_projection("Bark VTXO state changed concurrently"));
    }
    Ok(())
}

async fn reserve_input<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    action_id: cdk_federation_bark::FederationBarkActionId,
    kind: cdk_federation_bark::FederationBarkActionKind,
    input: &cdk_federation_bark::FederationBarkVtxoInput,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let previous = get_vtxo(connection, input.vtxo_id)
        .await?
        .ok_or_else(|| invalid_projection("Bark action plan references an unknown VTXO"))?;
    let expected_state = match kind {
        cdk_federation_bark::FederationBarkActionKind::IncomingClaim => {
            FederationBarkVtxoState::Observed
        }
        cdk_federation_bark::FederationBarkActionKind::FailedSendRevocation => {
            FederationBarkVtxoState::PendingSpend
        }
        _ => FederationBarkVtxoState::Spendable,
    };
    if previous.state != expected_state
        || previous.locator != input.locator
        || previous.amount != input.amount
        || match kind {
            cdk_federation_bark::FederationBarkActionKind::FailedSendRevocation => {
                previous.reservation_action_id != Some(action_id)
            }
            _ => previous.reservation_action_id.is_some(),
        }
    {
        return Err(invalid_projection(
            "Bark action plan input is not in its exact reservable state",
        ));
    }
    let updated = FederationBarkVtxoRecord {
        state: FederationBarkVtxoState::Reserved,
        reservation_action_id: Some(action_id),
        latest_operation_id: entry.operation_id,
        latest_operation_index: entry.index,
        ..previous.clone()
    };
    update_vtxo(connection, &previous, &updated).await?;

    let reservation = FederationBarkReservationRecord {
        vtxo_id: input.vtxo_id,
        action_id,
        operation_id: entry.operation_id,
        operation_index: entry.index,
        amount: input.amount,
    };
    query(
        r#"
        INSERT INTO federation_bark_reservations
        (vtxo_id, action_id, operation_id, operation_index, amount, record_json)
        VALUES (:vtxo_id, :action_id, :operation_id, :operation_index, :amount, :record_json)
        "#,
    )?
    .bind("vtxo_id", reservation.vtxo_id.to_string())
    .bind("action_id", reservation.action_id.to_string())
    .bind("operation_id", reservation.operation_id.to_string())
    .bind(
        "operation_index",
        sql_i64("reservation operation index", reservation.operation_index)?,
    )
    .bind(
        "amount",
        sql_i64("reservation amount", u64::from(reservation.amount))?,
    )
    .bind("record_json", serde_json::to_string(&reservation)?)
    .execute(connection)
    .await?;
    Ok(())
}

async fn retain_refresh_reservation_for_exit<C>(
    connection: &C,
    action_id: cdk_federation_bark::FederationBarkActionId,
    input: &cdk_federation_bark::FederationBarkVtxoInput,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let reserved = get_vtxo(connection, input.vtxo_id)
        .await?
        .ok_or_else(|| invalid_projection("Bark exit escalation references an unknown VTXO"))?;
    if reserved.state != FederationBarkVtxoState::Reserved
        || reserved.reservation_action_id != Some(action_id)
        || reserved.locator != input.locator
        || reserved.amount != input.amount
    {
        return Err(invalid_projection(
            "Bark exit escalation differs from the existing refresh reservation",
        ));
    }
    Ok(())
}

async fn apply_vtxo_transition<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    transition: &cdk_federation_bark::FederationBarkActionTransition,
    mutation: &cdk_federation_bark::FederationBarkVtxoTransition,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    apply_vtxo_transition_from_artifacts(
        connection,
        entry,
        transition.action_id,
        &transition.artifacts,
        mutation,
    )
    .await
}

async fn apply_vtxo_transition_from_artifacts<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    action_id: cdk_federation_bark::FederationBarkActionId,
    artifacts: &[cdk_federation_bark::FederationBarkArtifact],
    mutation: &cdk_federation_bark::FederationBarkVtxoTransition,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let artifact = artifacts
        .iter()
        .find(|artifact| {
            artifact.kind == cdk_federation_bark::FederationBarkArtifactKind::Vtxo
                && artifact.hash == mutation.artifact_hash
        })
        .cloned()
        .ok_or_else(|| invalid_projection("Bark VTXO artifact is missing"))?;

    match get_vtxo(connection, mutation.vtxo_id).await? {
        Some(previous) => {
            if mutation.previous_state != Some(previous.state)
                || previous.locator != mutation.locator
                || previous.amount != mutation.amount
                || previous.expiry_height != mutation.expiry_height
                || previous.artifact != artifact
            {
                return Err(invalid_projection(
                    "Bark VTXO transition does not match the current authoritative row",
                ));
            }
            if matches!(
                previous.state,
                FederationBarkVtxoState::Reserved
                    | FederationBarkVtxoState::PendingSpend
                    | FederationBarkVtxoState::Exiting
            ) && previous.reservation_action_id != Some(action_id)
            {
                return Err(invalid_projection(
                    "Bark VTXO transition is not owned by this action",
                ));
            }
            let reservation_action_id = match mutation.new_state {
                FederationBarkVtxoState::Reserved
                | FederationBarkVtxoState::PendingSpend
                | FederationBarkVtxoState::Exiting => Some(action_id),
                _ => None,
            };
            let updated = FederationBarkVtxoRecord {
                state: mutation.new_state,
                reservation_action_id,
                latest_operation_id: entry.operation_id,
                latest_operation_index: entry.index,
                ..previous.clone()
            };
            update_vtxo(connection, &previous, &updated).await
        }
        None => {
            if mutation.previous_state.is_some() {
                return Err(invalid_projection(
                    "Bark VTXO transition expected a missing prior row",
                ));
            }
            let allocation = get_allocation(connection, mutation.locator)
                .await?
                .ok_or_else(|| invalid_projection("Bark VTXO locator was not allocated"))?;
            let materialized_amount = allocation
                .materialized_amount
                .checked_add(mutation.amount)
                .ok_or_else(|| invalid_projection("Bark allocation amount overflow"))?;
            if allocation.action_id != action_id
                || materialized_amount > allocation.amount
                || allocation.vtxo_ids.contains(&mutation.vtxo_id)
            {
                return Err(invalid_projection(
                    "Bark VTXO does not match its derivation allocation",
                ));
            }
            let vtxo = FederationBarkVtxoRecord {
                vtxo_id: mutation.vtxo_id,
                locator: mutation.locator,
                amount: mutation.amount,
                expiry_height: mutation.expiry_height,
                state: mutation.new_state,
                artifact,
                reservation_action_id: match mutation.new_state {
                    FederationBarkVtxoState::Reserved
                    | FederationBarkVtxoState::PendingSpend
                    | FederationBarkVtxoState::Exiting => Some(action_id),
                    _ => None,
                },
                origin_operation_id: entry.operation_id,
                origin_operation_index: entry.index,
                latest_operation_id: entry.operation_id,
                latest_operation_index: entry.index,
            };
            insert_vtxo(connection, &vtxo).await?;
            let mut vtxo_ids = allocation.vtxo_ids.clone();
            vtxo_ids.push(mutation.vtxo_id);
            vtxo_ids.sort();
            let updated_allocation = FederationBarkDerivationAllocationRecord {
                vtxo_id: allocation.vtxo_id.or(Some(mutation.vtxo_id)),
                vtxo_ids,
                materialized_amount,
                operation_id: entry.operation_id,
                operation_index: entry.index,
                ..allocation.clone()
            };
            materialize_allocation(connection, &allocation, &updated_allocation).await
        }
    }
}

async fn insert_transition<C>(
    connection: &C,
    record: &FederationBarkTransitionRecord,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let rows_affected = query(
        r#"
        INSERT INTO federation_bark_transitions
        (operation_id, operation_index, action_id, phase, predecessor_operation_id,
         attempt, record_json)
        VALUES
        (:operation_id, :operation_index, :action_id, :phase, :predecessor_operation_id,
         :attempt, :record_json)
        ON CONFLICT (operation_id)
        DO UPDATE SET record_json = EXCLUDED.record_json
        WHERE federation_bark_transitions.record_json = EXCLUDED.record_json
        "#,
    )?
    .bind("operation_id", record.operation_id.to_string())
    .bind(
        "operation_index",
        sql_i64("transition operation index", record.operation_index)?,
    )
    .bind("action_id", record.action_id.to_string())
    .bind("phase", phase_label(record.phase))
    .bind(
        "predecessor_operation_id",
        record
            .predecessor_operation_id
            .map(|operation_id| operation_id.to_string()),
    )
    .bind("attempt", i64::from(record.attempt))
    .bind("record_json", serde_json::to_string(record)?)
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        return Err(invalid_projection("Bark transition operation conflicts"));
    }
    Ok(())
}

async fn apply_receive_intent<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    operation: &FederationBarkOperation,
    intent: &cdk_federation_bark::FederationBarkReceiveIntent,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    if get_action(connection, intent.action_id).await?.is_some() {
        return Err(invalid_projection("Bark receive action already exists"));
    }
    let action = FederationBarkActionRecord::new(entry, operation)?;
    insert_action(connection, &action).await?;
    insert_allocation(
        connection,
        &FederationBarkDerivationAllocationRecord {
            locator: intent.destination,
            action_id: intent.action_id,
            amount: intent.minimum_net_amount,
            artifact_hash: None,
            vtxo_id: None,
            vtxo_ids: Vec::new(),
            materialized_amount: cdk_common::Amount::ZERO,
            operation_id: entry.operation_id,
            operation_index: entry.index,
        },
    )
    .await
}

async fn apply_receive_prepared<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    operation: &FederationBarkOperation,
    prepared: &cdk_federation_bark::FederationBarkReceivePrepared,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let action = get_action(connection, prepared.action_id)
        .await?
        .ok_or_else(|| invalid_projection("Bark receive intent does not exist"))?;
    let cdk_federation_bark::FederationBarkOperation::ReceiveIntent(intent) = &action.operation
    else {
        return Err(invalid_projection(
            "Bark receive preparation predecessor is not an intent",
        ));
    };
    prepared
        .validate_against_intent(&intent)
        .map_err(|_| invalid_projection("Bark receive preparation differs from its intent"))?;
    ensure_same_action_binding(&action, operation)?;
    ensure_predecessor(&action, prepared.intent_operation_id)?;
    if !transition_allowed(&action, operation.phase()) {
        return Err(invalid_projection(
            "invalid Bark receive preparation transition",
        ));
    }
    let updated = action.advanced(entry, operation)?;
    update_action(connection, &action, &updated).await?;
    insert_transition(
        connection,
        &FederationBarkTransitionRecord::new(entry, operation),
    )
    .await
}

async fn apply_action_plan<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    operation: &FederationBarkOperation,
    plan: &cdk_federation_bark::FederationBarkActionPlan,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    if operation.phase() != planned_phase(plan.kind) {
        return Err(invalid_projection(
            "Bark action plan phase does not match its kind",
        ));
    }
    let existing = get_action(connection, plan.action_id).await?;
    let combined_receive_claim = plan.certified_receive_htlcs.is_some();
    let refresh_exit_escalation = existing.as_ref().is_some_and(|action| {
        action.phase == cdk_federation_bark::FederationBarkSemanticPhase::RefreshPlanned
            && plan.kind == cdk_federation_bark::FederationBarkActionKind::EmergencyExit
    });
    let action = match &existing {
        Some(action) => {
            ensure_same_action_binding(action, operation)?;
            ensure_predecessor(action, plan.predecessor_operation_id)?;
            if !(transition_allowed(action, operation.phase())
                || combined_receive_claim
                    && action.phase
                        == cdk_federation_bark::FederationBarkSemanticPhase::ReceivePrepared)
            {
                return Err(invalid_projection("invalid Bark action plan transition"));
            }
            action.advanced(entry, operation)?
        }
        None => {
            if matches!(
                plan.kind,
                cdk_federation_bark::FederationBarkActionKind::IncomingClaim
                    | cdk_federation_bark::FederationBarkActionKind::FailedSendRevocation
            ) {
                return Err(invalid_projection(
                    "Bark recovery action plan has no existing lineage",
                ));
            }
            FederationBarkActionRecord::new(entry, operation)?
        }
    };
    match &existing {
        Some(previous) => update_action(connection, previous, &action).await?,
        None => insert_action(connection, &action).await?,
    }

    if let Some(certified) = plan.certified_receive_htlcs.as_deref() {
        for mutation in &certified.vtxo_transitions {
            apply_vtxo_transition_from_artifacts(
                connection,
                entry,
                plan.action_id,
                &plan.artifacts,
                mutation,
            )
            .await?;
        }
    }

    for input in &plan.inputs {
        if refresh_exit_escalation {
            retain_refresh_reservation_for_exit(connection, plan.action_id, input).await?;
        } else {
            reserve_input(connection, entry, plan.action_id, plan.kind, input).await?;
        }
    }
    for output in &plan.outputs {
        if plan.kind == cdk_federation_bark::FederationBarkActionKind::FailedSendRevocation {
            let previous = get_allocation(connection, output.locator)
                .await?
                .ok_or_else(|| invalid_projection("Bark revocation locator was not reserved"))?;
            if previous.action_id != plan.action_id
                || previous.amount != cdk_common::Amount::ZERO
                || previous.artifact_hash.is_some()
                || previous.vtxo_id.is_some()
                || !previous.vtxo_ids.is_empty()
                || previous.materialized_amount != cdk_common::Amount::ZERO
            {
                return Err(invalid_projection(
                    "Bark revocation locator differs from its send reservation",
                ));
            }
            let updated = FederationBarkDerivationAllocationRecord {
                amount: output.amount,
                artifact_hash: Some(output.artifact_hash),
                operation_id: entry.operation_id,
                operation_index: entry.index,
                ..previous.clone()
            };
            materialize_allocation(connection, &previous, &updated).await?;
        } else {
            let amount =
                if plan.kind == cdk_federation_bark::FederationBarkActionKind::LightningSend {
                    plan.inputs
                        .iter()
                        .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                            sum.checked_add(input.amount).ok_or_else(|| {
                                invalid_projection("Bark send allocation amount overflow")
                            })
                        })?
                } else {
                    output.amount
                };
            insert_allocation(
                connection,
                &FederationBarkDerivationAllocationRecord {
                    locator: output.locator,
                    action_id: plan.action_id,
                    amount,
                    artifact_hash: Some(output.artifact_hash),
                    vtxo_id: None,
                    vtxo_ids: Vec::new(),
                    materialized_amount: cdk_common::Amount::ZERO,
                    operation_id: entry.operation_id,
                    operation_index: entry.index,
                },
            )
            .await?;
        }
    }
    if let Some(locator) = plan.revocation_locator {
        insert_allocation(
            connection,
            &FederationBarkDerivationAllocationRecord {
                locator,
                action_id: plan.action_id,
                amount: cdk_common::Amount::ZERO,
                artifact_hash: None,
                vtxo_id: None,
                vtxo_ids: Vec::new(),
                materialized_amount: cdk_common::Amount::ZERO,
                operation_id: entry.operation_id,
                operation_index: entry.index,
            },
        )
        .await?;
    }
    insert_transition(
        connection,
        &FederationBarkTransitionRecord::new(entry, operation),
    )
    .await
}

async fn apply_action_transition<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    operation: &FederationBarkOperation,
    transition: &cdk_federation_bark::FederationBarkActionTransition,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let action = get_action(connection, transition.action_id)
        .await?
        .ok_or_else(|| invalid_projection("Bark action transition has no action"))?;
    ensure_same_action_binding(&action, operation)?;
    ensure_predecessor(&action, transition.predecessor_operation_id)?;
    let expected_plan = action
        .plan_operation_id
        .unwrap_or(action.latest_operation_id);
    if transition.plan_operation_id != expected_plan {
        return Err(invalid_projection(
            "Bark action transition does not reference the current plan",
        ));
    }
    if transition.phase == cdk_federation_bark::FederationBarkSemanticPhase::SigningAttemptAdvanced
    {
        if transition.attempt <= action.attempt {
            return Err(invalid_projection("Bark signing attempt did not advance"));
        }
    } else if transition.attempt != action.attempt && action.attempt != 0 {
        return Err(invalid_projection("Bark transition attempt is not current"));
    }
    if !transition_allowed(&action, transition.phase) {
        return Err(invalid_projection("invalid Bark semantic phase transition"));
    }

    for mutation in &transition.vtxo_transitions {
        apply_vtxo_transition(connection, entry, transition, mutation).await?;
    }
    let updated = action.advanced(entry, operation)?;
    update_action(connection, &action, &updated).await?;
    insert_transition(
        connection,
        &FederationBarkTransitionRecord::new(entry, operation),
    )
    .await
}

async fn apply_observation<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    operation: &FederationBarkOperation,
    observation: &cdk_federation_bark::FederationBarkMemberObservation,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let action = get_action(connection, observation.action_id)
        .await?
        .ok_or_else(|| invalid_projection("Bark observation has no action"))?;
    ensure_same_action_binding(&action, operation)?;
    let record = FederationBarkObservationRecord {
        operation_id: entry.operation_id,
        operation_index: entry.index,
        operation: operation.clone(),
    };
    let rows_affected = query(
        r#"
        INSERT INTO federation_bark_observations
        (operation_id, operation_index, action_id, observer_member_id, observation_epoch, record_json)
        VALUES
        (:operation_id, :operation_index, :action_id, :observer_member_id, :observation_epoch, :record_json)
        ON CONFLICT (operation_id)
        DO UPDATE SET record_json = EXCLUDED.record_json
        WHERE federation_bark_observations.record_json = EXCLUDED.record_json
        "#,
    )?
    .bind("operation_id", record.operation_id.to_string())
    .bind(
        "operation_index",
        sql_i64("observation operation index", record.operation_index)?,
    )
    .bind("action_id", observation.action_id.to_string())
    .bind("observer_member_id", i64::from(observation.observer.get()))
    .bind(
        "observation_epoch",
        sql_i64("observation epoch", observation.observation_epoch)?,
    )
    .bind("record_json", serde_json::to_string(&record)?)
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        return Err(invalid_projection("Bark observation operation conflicts"));
    }
    Ok(())
}

pub(super) async fn apply_bark_operation<C>(
    connection: &C,
    entry: &FederationJournalEntry,
    operation: &FederationBarkOperation,
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    operation.validate()?;
    if entry.envelope.operation_id()? != entry.operation_id {
        return Err(invalid_projection(
            "Bark journal operation ID does not match its canonical envelope",
        ));
    }
    let FederationOperation::Application(entry_operation) = &entry.envelope.operation else {
        return Err(invalid_projection(
            "Bark projection operation does not match its journal entry",
        ));
    };
    let entry_operation =
        cdk_federation_bark::validate_federation_bark_application_operation(entry_operation)?;
    if &entry_operation != operation {
        return Err(invalid_projection(
            "Bark projection operation does not match its journal entry",
        ));
    }
    if projection_operation_exists(connection, entry.operation_id).await? {
        return Ok(());
    }
    if let Some(author) = operation.claimed_author() {
        if entry
            .envelope
            .authorization
            .as_ref()
            .map(|authorization| authorization.author)
            != Some(author)
        {
            return Err(invalid_projection(
                "Bark projection operation author does not match its envelope",
            ));
        }
    }

    match operation {
        FederationBarkOperation::ReceiveIntent(intent) => {
            apply_receive_intent(connection, entry, operation, intent).await
        }
        FederationBarkOperation::ReceivePrepared(prepared) => {
            apply_receive_prepared(connection, entry, operation, prepared).await
        }
        FederationBarkOperation::ActionPlan(plan) => {
            apply_action_plan(connection, entry, operation, plan).await
        }
        FederationBarkOperation::ActionTransition(transition) => {
            apply_action_transition(connection, entry, operation, transition).await
        }
        FederationBarkOperation::MemberObservation(observation) => {
            apply_observation(connection, entry, operation, observation).await
        }
        FederationBarkOperation::EvidenceCertificate(certificate) => {
            let observer = certificate
                .attestations
                .first()
                .ok_or_else(|| invalid_projection("Bark evidence certificate has no signer"))?
                .signer;
            let observation = certificate.observation_for(observer);
            apply_observation(connection, entry, operation, &observation).await
        }
    }
}

pub(super) async fn get_bark_projection_snapshot<C>(
    connection: &C,
) -> Result<FederationBarkProjectionSnapshot, Error>
where
    C: DatabaseExecutor,
{
    Ok(FederationBarkProjectionSnapshot {
        actions: fetch_records(
            connection,
            "SELECT record_json FROM federation_bark_actions ORDER BY action_id ASC",
            "Bark action",
        )
        .await?,
        vtxos: fetch_records(
            connection,
            "SELECT record_json FROM federation_bark_vtxos ORDER BY vtxo_id ASC",
            "Bark VTXO",
        )
        .await?,
        derivation_allocations: fetch_records(
            connection,
            "SELECT record_json FROM federation_bark_derivation_allocations ORDER BY locator_role ASC, locator_index ASC, locator_version ASC",
            "Bark derivation allocation",
        )
        .await?,
        reservations: fetch_records(
            connection,
            "SELECT record_json FROM federation_bark_reservations ORDER BY vtxo_id ASC",
            "Bark reservation",
        )
        .await?,
        transitions: fetch_records(
            connection,
            "SELECT record_json FROM federation_bark_transitions ORDER BY operation_id ASC",
            "Bark transition",
        )
        .await?,
        observations: fetch_records(
            connection,
            "SELECT record_json FROM federation_bark_observations ORDER BY operation_id ASC",
            "Bark observation",
        )
        .await?,
    })
}

async fn fetch_records<C, T>(connection: &C, sql: &str, label: &str) -> Result<Vec<T>, Error>
where
    C: DatabaseExecutor,
    T: DeserializeOwned,
{
    query(sql)?
        .fetch_all(connection)
        .await?
        .into_iter()
        .map(|row| decode_record(row, label))
        .collect()
}

fn decode_record<T>(row: Vec<Column>, label: &str) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    unpack_into!(let (record_json) = row);
    serde_json::from_str(&column_as_string!(&record_json))
        .map_err(|error| Error::Internal(format!("Invalid federation {label} JSON: {error}")))
}

#[cfg(test)]
mod tests {
    use super::direct_transition_allowed;
    use cdk_federation_bark::FederationBarkSemanticPhase as Phase;

    #[test]
    fn maintenance_projection_allows_only_the_frozen_lifecycle() {
        for (previous, next) in [
            (Phase::RefreshPlanned, Phase::RefreshCompleted),
            (Phase::RefreshPlanned, Phase::ExitPlanned),
            (Phase::ExitPlanned, Phase::ExitRegistered),
            (Phase::ExitRegistered, Phase::ExitPublished),
            (Phase::ExitPublished, Phase::ExitConfirmed),
        ] {
            assert!(direct_transition_allowed(previous, next));
        }

        for (previous, next) in [
            (Phase::RefreshPlanned, Phase::ExitRegistered),
            (Phase::ExitPlanned, Phase::ExitPublished),
            (Phase::ExitRegistered, Phase::ExitConfirmed),
            (Phase::ExitPublished, Phase::RefreshCompleted),
            (Phase::ExitConfirmed, Phase::ExitPlanned),
        ] {
            assert!(!direct_transition_allowed(previous, next));
        }
    }

    #[test]
    fn outgoing_terminal_results_follow_the_accepted_htlc_package_directly() {
        assert!(direct_transition_allowed(
            Phase::SendHtlcsCosigned,
            Phase::PaymentSucceeded,
        ));
        assert!(direct_transition_allowed(
            Phase::SendHtlcsCosigned,
            Phase::PaymentFailed,
        ));
        assert!(!direct_transition_allowed(
            Phase::SendPlanned,
            Phase::PaymentSucceeded,
        ));
    }
}
