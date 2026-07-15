use super::*;

async fn ensure_federation_checkpoint_state<RM>(
    transaction: &SQLTransaction<RM>,
) -> Result<(), Error>
where
    RM: DatabasePool + 'static,
{
    query(
        r#"
        INSERT INTO federation_checkpoint_state
        (
            state_key,
            journal_next_index,
            journal_order_digest,
            finalized_next_item_index,
            finalized_item_digest,
            application_next_index,
            application_digest,
            materialized_state_audit_mismatch
        )
        VALUES
        (
            :state_key,
            0,
            :journal_order_digest,
            0,
            :finalized_item_digest,
            0,
            :application_digest,
            0
        )
        ON CONFLICT (state_key) DO NOTHING
        "#,
    )?
    .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
    .bind(
        "journal_order_digest",
        FederationJournalDigest::genesis().to_string(),
    )
    .bind(
        "finalized_item_digest",
        FederationFinalizedItemDigest::genesis().to_string(),
    )
    .bind(
        "application_digest",
        FederationApplicationDigest::genesis().to_string(),
    )
    .execute(&transaction.inner)
    .await?;
    Ok(())
}

async fn federation_checkpoint_frontier_for_update<RM>(
    transaction: &SQLTransaction<RM>,
) -> Result<FederationCheckpointFrontier, Error>
where
    RM: DatabasePool + 'static,
{
    ensure_federation_checkpoint_state(transaction).await?;
    query(
        r#"
        SELECT
            journal_next_index,
            journal_order_digest,
            finalized_next_item_index,
            finalized_item_digest,
            application_next_index,
            application_digest,
            trusted_checkpoint_id,
            latest_audited_checkpoint_id,
            latest_materialized_state_digest,
            materialized_state_audit_mismatch
        FROM federation_checkpoint_state
        WHERE state_key = :state_key
        FOR UPDATE
        "#,
    )?
    .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
    .fetch_one(&transaction.inner)
    .await?
    .ok_or_else(|| Error::Internal("Federation checkpoint state row disappeared".to_string()))
    .and_then(sql_row_to_federation_checkpoint_frontier)
}

async fn advance_federation_journal_frontier<RM>(
    transaction: &SQLTransaction<RM>,
) -> Result<(), Error>
where
    RM: DatabasePool + 'static,
{
    let mut frontier = federation_checkpoint_frontier_for_update(transaction).await?;
    loop {
        let Some(row) = query(
            r#"
            SELECT previous_order_digest, order_digest
            FROM federation_journal_entries
            WHERE operation_index = :operation_index
            FOR UPDATE
            "#,
        )?
        .bind("operation_index", frontier.journal_next_index as i64)
        .fetch_one(&transaction.inner)
        .await?
        else {
            break;
        };
        unpack_into!(let (previous_order_digest, order_digest) = row);
        let previous_order_digest =
            FederationJournalDigest::from_hex(column_as_string!(&previous_order_digest))
                .map_err(|e| Error::Internal(format!("Invalid federation journal digest: {e}")))?;
        let order_digest = FederationJournalDigest::from_hex(column_as_string!(&order_digest))
            .map_err(|e| Error::Internal(format!("Invalid federation journal digest: {e}")))?;
        if previous_order_digest != frontier.journal_order_digest {
            return Err(Error::Internal(format!(
                "Federation journal frontier digest mismatch at index {}",
                frontier.journal_next_index
            )));
        }
        frontier.journal_next_index = frontier
            .journal_next_index
            .checked_add(1)
            .ok_or_else(|| Error::Internal("Federation journal cursor overflow".to_string()))?;
        frontier.journal_order_digest = order_digest;
    }

    query(
        r#"
        UPDATE federation_checkpoint_state
        SET
            journal_next_index = :journal_next_index,
            journal_order_digest = :journal_order_digest
        WHERE state_key = :state_key
        "#,
    )?
    .bind("journal_next_index", frontier.journal_next_index as i64)
    .bind(
        "journal_order_digest",
        frontier.journal_order_digest.to_string(),
    )
    .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
    .execute(&transaction.inner)
    .await?;
    Ok(())
}

async fn advance_federation_application_frontier<RM>(
    transaction: &SQLTransaction<RM>,
) -> Result<(), Error>
where
    RM: DatabasePool + 'static,
{
    let mut frontier = federation_checkpoint_frontier_for_update(transaction).await?;
    loop {
        if frontier.application_next_index >= frontier.journal_next_index {
            break;
        }
        let Some(entry) = query(federation_journal_entry_select!(
            "WHERE operation_index = :operation_index FOR UPDATE"
        ))?
        .bind("operation_index", frontier.application_next_index as i64)
        .fetch_one(&transaction.inner)
        .await?
        .map(sql_row_to_federation_journal_entry)
        .transpose()?
        else {
            return Err(Error::Internal(format!(
                "Federation application frontier is missing journal index {}",
                frontier.application_next_index
            )));
        };
        if !entry.application.is_terminal() {
            break;
        }
        let next_digest = frontier
            .application_digest
            .append_terminal(entry.index, entry.operation_id, &entry.application)
            .map_err(|e| Error::Internal(format!("Invalid terminal application state: {e}")))?;
        let rows_affected = query(
            r#"
            UPDATE federation_journal_entries
            SET application_digest = :application_digest
            WHERE operation_index = :operation_index
              AND (
                    application_digest IS NULL
                    OR application_digest = :application_digest
                  )
            "#,
        )?
        .bind("operation_index", entry.index as i64)
        .bind("application_digest", next_digest.to_string())
        .execute(&transaction.inner)
        .await?;
        if rows_affected == 0 {
            return Err(Error::Internal(format!(
                "Conflicting federation application digest at index {}",
                entry.index
            )));
        }
        frontier.application_next_index = frontier
            .application_next_index
            .checked_add(1)
            .ok_or_else(|| Error::Internal("Federation application cursor overflow".to_string()))?;
        frontier.application_digest = next_digest;
    }

    query(
        r#"
        UPDATE federation_checkpoint_state
        SET
            application_next_index = :application_next_index,
            application_digest = :application_digest
        WHERE state_key = :state_key
        "#,
    )?
    .bind(
        "application_next_index",
        frontier.application_next_index as i64,
    )
    .bind(
        "application_digest",
        frontier.application_digest.to_string(),
    )
    .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
    .execute(&transaction.inner)
    .await?;
    Ok(())
}

async fn record_federation_materialized_state_keys<RM>(
    transaction: &SQLTransaction<RM>,
    entry: &FederationJournalEntry,
) -> Result<(), Error>
where
    RM: DatabasePool + 'static,
{
    let records = entry.materialized_state_keys().map_err(|e| {
        Error::Internal(format!(
            "Could not derive federation materialized-state keys: {e}"
        ))
    })?;
    for record in records {
        query(
            r#"
            INSERT INTO federation_materialized_state_keys
            (state_kind, state_key, operation_index, operation_id, record_json)
            VALUES
            (:state_kind, :state_key, :operation_index, :operation_id, :record_json)
            ON CONFLICT (state_kind, state_key)
            DO UPDATE SET
                operation_index = EXCLUDED.operation_index,
                operation_id = EXCLUDED.operation_id,
                record_json = EXCLUDED.record_json
            "#,
        )?
        .bind("state_kind", record.kind_label())
        .bind("state_key", record.row_key())
        .bind("operation_index", record.operation_index as i64)
        .bind("operation_id", record.operation_id.to_string())
        .bind("record_json", serde_json::to_string(&record)?)
        .execute(&transaction.inner)
        .await?;
    }
    Ok(())
}

async fn advance_federation_finalized_item_frontier<RM>(
    transaction: &SQLTransaction<RM>,
) -> Result<(), Error>
where
    RM: DatabasePool + 'static,
{
    let mut frontier = federation_checkpoint_frontier_for_update(transaction).await?;
    loop {
        let Some(item) = query(federation_aleph_bft_finalized_item_select!(
            "WHERE accepted_item_index = :accepted_item_index FOR UPDATE"
        ))?
        .bind(
            "accepted_item_index",
            frontier.finalized_next_item_index as i64,
        )
        .fetch_one(&transaction.inner)
        .await?
        .map(sql_row_to_federation_aleph_bft_finalized_item)
        .transpose()?
        else {
            break;
        };
        let next_digest = frontier
            .finalized_item_digest
            .append(&item)
            .map_err(|e| Error::Internal(format!("Invalid finalized item: {e}")))?;
        let rows_affected = query(
            r#"
            UPDATE federation_aleph_bft_finalized_items
            SET finalized_item_digest = :finalized_item_digest
            WHERE accepted_item_index = :accepted_item_index
              AND (
                    finalized_item_digest IS NULL
                    OR finalized_item_digest = :finalized_item_digest
                  )
            "#,
        )?
        .bind("accepted_item_index", item.accepted_item_index as i64)
        .bind("finalized_item_digest", next_digest.to_string())
        .execute(&transaction.inner)
        .await?;
        if rows_affected == 0 {
            return Err(Error::Internal(format!(
                "Conflicting federation finalized-item digest at index {}",
                item.accepted_item_index
            )));
        }
        frontier.finalized_next_item_index = frontier
            .finalized_next_item_index
            .checked_add(1)
            .ok_or_else(|| {
                Error::Internal("Federation finalized-item cursor overflow".to_string())
            })?;
        frontier.finalized_item_digest = next_digest;
    }

    query(
        r#"
        UPDATE federation_checkpoint_state
        SET
            finalized_next_item_index = :finalized_next_item_index,
            finalized_item_digest = :finalized_item_digest
        WHERE state_key = :state_key
        "#,
    )?
    .bind(
        "finalized_next_item_index",
        frontier.finalized_next_item_index as i64,
    )
    .bind(
        "finalized_item_digest",
        frontier.finalized_item_digest.to_string(),
    )
    .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
    .execute(&transaction.inner)
    .await?;
    Ok(())
}

async fn insert_federation_checkpoint<RM>(
    transaction: &SQLTransaction<RM>,
    checkpoint: &FederationCheckpoint,
) -> Result<FederationCheckpointId, Error>
where
    RM: DatabasePool + 'static,
{
    let checkpoint_id = checkpoint
        .id()
        .map_err(|e| Error::Internal(format!("Invalid federation checkpoint: {e}")))?;
    let checkpoint_json = serde_json::to_string(checkpoint)?;
    let rows_affected = query(
        r#"
        INSERT INTO federation_checkpoints
        (
            checkpoint_id,
            checkpoint_kind,
            scheduled_checkpoint_sequence,
            scheduled_anchor_id,
            journal_next_index,
            finalized_next_item_index,
            checkpoint_json
        )
        VALUES
        (
            :checkpoint_id,
            :checkpoint_kind,
            :scheduled_checkpoint_sequence,
            :scheduled_anchor_id,
            :journal_next_index,
            :finalized_next_item_index,
            :checkpoint_json
        )
        ON CONFLICT (checkpoint_id)
        DO UPDATE SET checkpoint_json = EXCLUDED.checkpoint_json
        WHERE federation_checkpoints.checkpoint_kind = EXCLUDED.checkpoint_kind
          AND federation_checkpoints.scheduled_checkpoint_sequence = EXCLUDED.scheduled_checkpoint_sequence
          AND (
                federation_checkpoints.scheduled_anchor_id = EXCLUDED.scheduled_anchor_id
                OR (
                    federation_checkpoints.scheduled_anchor_id IS NULL
                    AND EXCLUDED.scheduled_anchor_id IS NULL
                )
              )
          AND federation_checkpoints.journal_next_index = EXCLUDED.journal_next_index
          AND federation_checkpoints.finalized_next_item_index = EXCLUDED.finalized_next_item_index
          AND federation_checkpoints.checkpoint_json = EXCLUDED.checkpoint_json
        "#,
    )?
    .bind("checkpoint_id", checkpoint_id.to_string())
    .bind("checkpoint_kind", checkpoint_kind_to_str(checkpoint.kind))
    .bind(
        "scheduled_checkpoint_sequence",
        checkpoint.scheduled_checkpoint_sequence as i64,
    )
    .bind(
        "scheduled_anchor_id",
        checkpoint.scheduled_anchor_id.map(|anchor| anchor.to_string()),
    )
    .bind("journal_next_index", checkpoint.journal_next_index as i64)
    .bind(
        "finalized_next_item_index",
        checkpoint.finalized_next_item_index as i64,
    )
    .bind("checkpoint_json", checkpoint_json)
    .execute(&transaction.inner)
    .await?;
    if rows_affected == 0 {
        return Err(Error::Internal(format!(
            "Conflicting federation checkpoint payload for ID {checkpoint_id}"
        )));
    }
    Ok(checkpoint_id)
}

async fn federation_journal_digest_at<RM>(
    transaction: &SQLTransaction<RM>,
    next_index: u64,
) -> Result<FederationJournalDigest, Error>
where
    RM: DatabasePool + 'static,
{
    if next_index == 0 {
        return Ok(FederationJournalDigest::genesis());
    }
    let index = next_index - 1;
    let row = query(
        r#"
        SELECT order_digest
        FROM federation_journal_entries
        WHERE operation_index = :operation_index
        "#,
    )?
    .bind("operation_index", index as i64)
    .fetch_one(&transaction.inner)
    .await?
    .ok_or_else(|| {
        Error::Internal(format!(
            "Federation checkpoint journal boundary is missing index {index}"
        ))
    })?;
    let digest = row.into_iter().next().ok_or_else(|| {
        Error::Internal("Federation checkpoint journal digest row had no value".to_string())
    })?;
    FederationJournalDigest::from_hex(column_as_string!(&digest))
        .map_err(|e| Error::Internal(format!("Invalid federation journal digest: {e}")))
}

async fn federation_application_digest_at<RM>(
    transaction: &SQLTransaction<RM>,
    next_index: u64,
) -> Result<FederationApplicationDigest, Error>
where
    RM: DatabasePool + 'static,
{
    if next_index == 0 {
        return Ok(FederationApplicationDigest::genesis());
    }
    let index = next_index - 1;
    let row = query(
        r#"
        SELECT application_digest
        FROM federation_journal_entries
        WHERE operation_index = :operation_index
        "#,
    )?
    .bind("operation_index", index as i64)
    .fetch_one(&transaction.inner)
    .await?
    .ok_or_else(|| {
        Error::Internal(format!(
            "Federation checkpoint application boundary is missing index {index}"
        ))
    })?;
    let digest = row.into_iter().next().ok_or_else(|| {
        Error::Internal("Federation checkpoint application digest row had no value".to_string())
    })?;
    let digest = column_as_nullable_string!(&digest).ok_or_else(|| {
        Error::Internal(format!(
            "Federation checkpoint application boundary is non-terminal at index {index}"
        ))
    })?;
    FederationApplicationDigest::from_hex(digest)
        .map_err(|e| Error::Internal(format!("Invalid federation application digest: {e}")))
}

async fn federation_finalized_item_digest_at<RM>(
    transaction: &SQLTransaction<RM>,
    next_index: u64,
) -> Result<FederationFinalizedItemDigest, Error>
where
    RM: DatabasePool + 'static,
{
    if next_index == 0 {
        return Ok(FederationFinalizedItemDigest::genesis());
    }
    let index = next_index - 1;
    let row = query(
        r#"
        SELECT finalized_item_digest
        FROM federation_aleph_bft_finalized_items
        WHERE accepted_item_index = :accepted_item_index
        "#,
    )?
    .bind("accepted_item_index", index as i64)
    .fetch_one(&transaction.inner)
    .await?
    .ok_or_else(|| {
        Error::Internal(format!(
            "Federation checkpoint finalized-item boundary is missing index {index}"
        ))
    })?;
    let digest = row.into_iter().next().ok_or_else(|| {
        Error::Internal("Federation checkpoint finalized digest row had no value".to_string())
    })?;
    let digest = column_as_nullable_string!(&digest).ok_or_else(|| {
        Error::Internal(format!(
            "Federation finalized-item digest is missing at index {index}"
        ))
    })?;
    FederationFinalizedItemDigest::from_hex(digest)
        .map_err(|e| Error::Internal(format!("Invalid finalized-item digest: {e}")))
}

async fn ensure_checkpoint_not_ahead_of_local_state<RM>(
    transaction: &SQLTransaction<RM>,
    checkpoint: &FederationCheckpoint,
) -> Result<FederationCheckpointFrontier, Error>
where
    RM: DatabasePool + 'static,
{
    let frontier = federation_checkpoint_frontier_for_update(transaction).await?;
    if frontier.materialized_state_audit_mismatch {
        return Err(Error::Internal(
            "Federation materialized-state audit mismatch is latched".to_string(),
        ));
    }
    if checkpoint.journal_next_index > frontier.journal_next_index
        || checkpoint.journal_next_index > frontier.application_next_index
        || checkpoint.finalized_next_item_index > frontier.finalized_next_item_index
    {
        return Err(Error::Internal(
            "Federation checkpoint is ahead of durable local frontiers".to_string(),
        ));
    }
    if federation_journal_digest_at(transaction, checkpoint.journal_next_index).await?
        != checkpoint.journal_order_digest
        || federation_application_digest_at(transaction, checkpoint.journal_next_index).await?
            != checkpoint.application_digest
        || federation_finalized_item_digest_at(transaction, checkpoint.finalized_next_item_index)
            .await?
            != checkpoint.finalized_item_digest
    {
        return Err(Error::Internal(
            "Federation checkpoint digest does not match durable local state".to_string(),
        ));
    }
    Ok(frontier)
}

#[async_trait]
impl<RM> FederationJournalTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn set_federation_config(
        &mut self,
        config: &FederationConfigSnapshot,
    ) -> Result<(), Self::Err> {
        let config_json = serde_json::to_string(&config.config)?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_config
            (federation_id, config_digest, config_json)
            VALUES
            (:federation_id, :config_digest, :config_json)
            ON CONFLICT (federation_id)
            DO UPDATE SET
                config_digest = EXCLUDED.config_digest,
                config_json = EXCLUDED.config_json
            WHERE federation_config.config_digest = EXCLUDED.config_digest
              AND federation_config.config_json = EXCLUDED.config_json
            "#,
        )?
        .bind("federation_id", config.federation_id.to_string())
        .bind("config_digest", config.config_digest.to_string())
        .bind("config_json", config_json.clone())
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            let mut rows = query(
                r#"
                SELECT
                    federation_id,
                    config_digest,
                    config_json
                FROM federation_config
                WHERE federation_id = :federation_id
                "#,
            )?
            .bind("federation_id", config.federation_id.to_string())
            .fetch_all(&self.inner)
            .await?
            .into_iter();

            if let Some(existing) = rows
                .next()
                .map(sql_row_to_federation_config_snapshot)
                .transpose()?
            {
                if is_dkg_finalization_config_update(&existing, config) {
                    query(
                        r#"
                        UPDATE federation_config
                        SET
                            config_digest = :config_digest,
                            config_json = :config_json
                        WHERE federation_id = :federation_id
                        "#,
                    )?
                    .bind("federation_id", config.federation_id.to_string())
                    .bind("config_digest", config.config_digest.to_string())
                    .bind("config_json", config_json)
                    .execute(&self.inner)
                    .await?;
                    return Ok(());
                }
            }

            return Err(federation_config_conflict_error(config));
        }

        Ok(())
    }

    async fn add_federation_journal_entry(
        &mut self,
        entry: &FederationJournalEntry,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_journal_entries
            (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, consensus_evidence_json, rejection_json, application_digest, status)
            VALUES
            (:operation_index, :operation_id, :envelope_json, :conflict_keys_json, :previous_order_digest, :order_digest, :consensus_evidence_json, :rejection_json, NULL, :status)
            "#,
        )?
        .bind("operation_index", entry.index as i64)
        .bind("operation_id", entry.operation_id.to_string())
        .bind("envelope_json", serde_json::to_string(&entry.envelope)?)
        .bind(
            "conflict_keys_json",
            serde_json::to_string(&entry.conflict_keys)?,
        )
        .bind(
            "previous_order_digest",
            entry.previous_order_digest.to_string(),
        )
        .bind("order_digest", entry.order_digest.to_string())
        .bind(
            "consensus_evidence_json",
            entry
                .consensus_evidence
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(
            "rejection_json",
            entry
                .rejection()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind("status", status_to_str(entry.status()))
        .execute(&self.inner)
        .await?;

        advance_federation_journal_frontier(self).await?;
        advance_federation_application_frontier(self).await?;

        Ok(())
    }

    async fn set_federation_operation_application_state(
        &mut self,
        operation_id: FederationOperationId,
        application: &FederationApplicationState,
    ) -> Result<(), Self::Err> {
        let existing = query(
            r#"
            SELECT operation_index, status, rejection_json, application_digest
            FROM federation_journal_entries
            WHERE operation_id = :operation_id
            FOR UPDATE
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .ok_or_else(|| {
            Error::Internal(format!(
                "Cannot update missing federation operation {operation_id}"
            ))
        })?;
        unpack_into!(let (operation_index, status, rejection_json, application_digest) = existing);
        let operation_index: u64 = column_as_number!(operation_index);
        let existing_application = FederationApplicationState::from_parts(
            operation_index,
            status_from_str(&column_as_string!(&status))?,
            optional_json(&rejection_json, "journal rejection")?,
        )
        .map_err(|e| Error::Internal(format!("Invalid federation application state: {e}")))?;
        let already_digest_committed = column_as_nullable_string!(&application_digest).is_some();
        if (already_digest_committed || existing_application.is_terminal())
            && existing_application != *application
        {
            return Err(Error::Internal(format!(
                "Cannot change terminal federation application state at index {operation_index}"
            )));
        }

        if matches!(application, FederationApplicationState::Applied {}) {
            let entry = query(federation_journal_entry_select!(
                "WHERE operation_id = :operation_id FOR UPDATE"
            ))?
            .bind("operation_id", operation_id.to_string())
            .fetch_one(&self.inner)
            .await?
            .map(sql_row_to_federation_journal_entry)
            .transpose()?
            .ok_or_else(|| {
                Error::Internal(format!(
                    "Cannot index missing federation operation {operation_id}"
                ))
            })?;
            record_federation_materialized_state_keys(self, &entry).await?;
        }

        query(
            r#"
            UPDATE federation_journal_entries
            SET status = :status, rejection_json = :rejection_json
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("status", status_to_str(application.status()))
        .bind(
            "rejection_json",
            application
                .rejection()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .execute(&self.inner)
        .await?;

        advance_federation_application_frontier(self).await?;

        Ok(())
    }

    async fn set_federation_dkg_member_sequence(
        &mut self,
        member_id: MemberId,
        sequence: u64,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_dkg_member_sequences
            (member_id, last_sequence)
            VALUES
            (:member_id, :last_sequence)
            ON CONFLICT (member_id)
            DO UPDATE SET last_sequence = EXCLUDED.last_sequence
            WHERE federation_dkg_member_sequences.last_sequence < EXCLUDED.last_sequence
            "#,
        )?
        .bind("member_id", i64::from(member_id.get()))
        .bind("last_sequence", sequence as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_checkpoint_candidate(
        &mut self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<(), Self::Err> {
        let frontier = ensure_checkpoint_not_ahead_of_local_state(self, checkpoint).await?;
        if checkpoint.journal_next_index != frontier.journal_next_index
            || checkpoint.journal_next_index != frontier.application_next_index
            || checkpoint.finalized_next_item_index != frontier.finalized_next_item_index
        {
            return Err(Error::Internal(
                "Federation checkpoint candidate does not equal the current durable frontier"
                    .to_string(),
            ));
        }
        let checkpoint_id = insert_federation_checkpoint(self, checkpoint).await?;
        query(
            r#"
            UPDATE federation_checkpoint_state
            SET
                latest_audited_checkpoint_id = :checkpoint_id,
                latest_materialized_state_digest = :materialized_state_digest
            WHERE state_key = :state_key
            "#,
        )?
        .bind("checkpoint_id", checkpoint_id.to_string())
        .bind(
            "materialized_state_digest",
            checkpoint.materialized_state_digest.to_string(),
        )
        .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
        .execute(&self.inner)
        .await?;
        Ok(())
    }

    async fn add_federation_checkpoint_signature(
        &mut self,
        signature: &FederationCheckpointSignature,
    ) -> Result<(), Self::Err> {
        let checkpoint_exists = query(
            r#"
            SELECT checkpoint_id
            FROM federation_checkpoints
            WHERE checkpoint_id = :checkpoint_id
            "#,
        )?
        .bind("checkpoint_id", signature.checkpoint_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .is_some();
        if !checkpoint_exists {
            return Err(Error::Internal(format!(
                "Cannot store a signature for unknown federation checkpoint {}",
                signature.checkpoint_id
            )));
        }

        let signature_json = serde_json::to_string(signature)?;
        let rows_affected = query(
            r#"
            INSERT INTO federation_checkpoint_signatures
            (checkpoint_id, signer_member_id, signature_json)
            VALUES
            (:checkpoint_id, :signer_member_id, :signature_json)
            ON CONFLICT (checkpoint_id, signer_member_id)
            DO UPDATE SET signature_json = EXCLUDED.signature_json
            WHERE federation_checkpoint_signatures.signature_json = EXCLUDED.signature_json
            "#,
        )?
        .bind("checkpoint_id", signature.checkpoint_id.to_string())
        .bind("signer_member_id", i64::from(signature.signer.get()))
        .bind("signature_json", signature_json)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(Error::Internal(format!(
                "Conflicting federation checkpoint signature from member {} for {}",
                signature.signer, signature.checkpoint_id
            )));
        }
        Ok(())
    }

    async fn add_federation_checkpoint_quorum(
        &mut self,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<(), Self::Err> {
        let checkpoint_id = insert_federation_checkpoint(self, &quorum.checkpoint).await?;
        for signature in &quorum.signatures {
            if signature.checkpoint_id != checkpoint_id {
                return Err(Error::Internal(
                    "Federation checkpoint quorum contains a mismatched signature".to_string(),
                ));
            }
            self.add_federation_checkpoint_signature(signature).await?;
        }

        let signatures = query(
            r#"
            SELECT checkpoint_id, signer_member_id, signature_json
            FROM federation_checkpoint_signatures
            WHERE checkpoint_id = :checkpoint_id
            ORDER BY signer_member_id ASC
            "#,
        )?
        .bind("checkpoint_id", checkpoint_id.to_string())
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_federation_checkpoint_signature)
        .collect::<Result<Vec<_>, _>>()?;
        let stored_quorum = FederationCheckpointQuorum::new(quorum.checkpoint.clone(), signatures);
        query(
            r#"
            INSERT INTO federation_checkpoint_quorums
            (checkpoint_id, quorum_json)
            VALUES
            (:checkpoint_id, :quorum_json)
            ON CONFLICT (checkpoint_id)
            DO UPDATE SET quorum_json = EXCLUDED.quorum_json
            "#,
        )?
        .bind("checkpoint_id", checkpoint_id.to_string())
        .bind("quorum_json", serde_json::to_string(&stored_quorum)?)
        .execute(&self.inner)
        .await?;
        Ok(())
    }

    async fn promote_verified_federation_checkpoint(
        &mut self,
        quorum: &FederationCheckpointQuorum,
    ) -> Result<(), Self::Err> {
        self.add_federation_checkpoint_quorum(quorum).await?;
        let frontier = ensure_checkpoint_not_ahead_of_local_state(self, &quorum.checkpoint).await?;
        let current = match frontier.trusted_checkpoint_id {
            Some(checkpoint_id) => query(
                r#"
                SELECT checkpoint_id, checkpoint_json
                FROM federation_checkpoints
                WHERE checkpoint_id = :checkpoint_id
                "#,
            )?
            .bind("checkpoint_id", checkpoint_id.to_string())
            .fetch_one(&self.inner)
            .await?
            .map(sql_row_to_federation_checkpoint)
            .transpose()?
            .ok_or_else(|| {
                Error::Internal(format!(
                    "Trusted federation checkpoint {checkpoint_id} is missing"
                ))
            })?,
            None => {
                query(
                    r#"
                    UPDATE federation_checkpoint_state
                    SET trusted_checkpoint_id = :trusted_checkpoint_id
                    WHERE state_key = :state_key
                    "#,
                )?
                .bind(
                    "trusted_checkpoint_id",
                    quorum
                        .checkpoint
                        .id()
                        .map_err(|e| {
                            Error::Internal(format!("Invalid federation checkpoint: {e}"))
                        })?
                        .to_string(),
                )
                .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
                .execute(&self.inner)
                .await?;
                return Ok(());
            }
        };
        quorum
            .checkpoint
            .validate_promotion_from(Some(&current))
            .map_err(|e| Error::Internal(format!("Invalid checkpoint promotion: {e}")))?;
        query(
            r#"
            UPDATE federation_checkpoint_state
            SET trusted_checkpoint_id = :trusted_checkpoint_id
            WHERE state_key = :state_key
            "#,
        )?
        .bind(
            "trusted_checkpoint_id",
            quorum
                .checkpoint
                .id()
                .map_err(|e| Error::Internal(format!("Invalid federation checkpoint: {e}")))?
                .to_string(),
        )
        .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
        .execute(&self.inner)
        .await?;
        Ok(())
    }

    async fn latch_federation_materialized_state_audit_mismatch(
        &mut self,
    ) -> Result<(), Self::Err> {
        ensure_federation_checkpoint_state(self).await?;
        query(
            r#"
            UPDATE federation_checkpoint_state
            SET materialized_state_audit_mismatch = 1
            WHERE state_key = :state_key
            "#,
        )?
        .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
        .execute(&self.inner)
        .await?;
        Ok(())
    }

    async fn add_federation_catch_up_certificate(
        &mut self,
        certificate: &FederationCatchUpCertificate,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_catch_up_certificates
            (from_index, signer_member_id, certificate_json)
            VALUES
            (:from_index, :signer_member_id, :certificate_json)
            ON CONFLICT (from_index, signer_member_id)
            DO UPDATE SET
                certificate_json = EXCLUDED.certificate_json
            "#,
        )?
        .bind("from_index", certificate.from_index as i64)
        .bind("signer_member_id", i64::from(certificate.signer.get()))
        .bind("certificate_json", serde_json::to_string(certificate)?)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_aleph_bft_finalized_item(
        &mut self,
        item: &FederationAlephBftFinalizedItem,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_aleph_bft_finalized_items
            (
                accepted_item_index,
                session,
                unit_finalization_index,
                unit_submitting_member,
                unit_index,
                item_index_in_unit,
                operation_id,
                status,
                unit_body_hash,
                item_json
            )
            VALUES
            (
                :accepted_item_index,
                :session,
                :unit_finalization_index,
                :unit_submitting_member,
                :unit_index,
                :item_index_in_unit,
                :operation_id,
                :status,
                :unit_body_hash,
                :item_json
            )
            ON CONFLICT (accepted_item_index)
            DO UPDATE SET
                session = EXCLUDED.session,
                unit_finalization_index = EXCLUDED.unit_finalization_index,
                unit_submitting_member = EXCLUDED.unit_submitting_member,
                unit_index = EXCLUDED.unit_index,
                item_index_in_unit = EXCLUDED.item_index_in_unit,
                operation_id = EXCLUDED.operation_id,
                status = EXCLUDED.status,
                unit_body_hash = EXCLUDED.unit_body_hash,
                item_json = EXCLUDED.item_json
            WHERE federation_aleph_bft_finalized_items.session = EXCLUDED.session
              AND federation_aleph_bft_finalized_items.unit_finalization_index = EXCLUDED.unit_finalization_index
              AND federation_aleph_bft_finalized_items.unit_submitting_member = EXCLUDED.unit_submitting_member
              AND federation_aleph_bft_finalized_items.unit_index = EXCLUDED.unit_index
              AND federation_aleph_bft_finalized_items.item_index_in_unit = EXCLUDED.item_index_in_unit
              AND (
                    federation_aleph_bft_finalized_items.operation_id = EXCLUDED.operation_id
                    OR (
                        federation_aleph_bft_finalized_items.operation_id IS NULL
                        AND EXCLUDED.operation_id IS NULL
                    )
                  )
              AND federation_aleph_bft_finalized_items.status = EXCLUDED.status
              AND federation_aleph_bft_finalized_items.unit_body_hash = EXCLUDED.unit_body_hash
              AND federation_aleph_bft_finalized_items.item_json = EXCLUDED.item_json
            "#,
        )?
        .bind("accepted_item_index", item.accepted_item_index as i64)
        .bind("session", item.session as i64)
        .bind(
            "unit_finalization_index",
            item.unit_finalization_index as i64,
        )
        .bind(
            "unit_submitting_member",
            i64::from(item.unit_submitting_member.get()),
        )
        .bind("unit_index", item.unit_index as i64)
        .bind("item_index_in_unit", item.item_index_in_unit as i64)
        .bind(
            "operation_id",
            item.operation_id.map(|operation_id| operation_id.to_string()),
        )
        .bind("status", aleph_bft_finalized_item_status_to_str(item.status))
        .bind("unit_body_hash", item.unit_body_hash.to_hex())
        .bind("item_json", serde_json::to_string(item)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_aleph_bft_finalized_item_conflict_error(item));
        }

        advance_federation_finalized_item_frontier(self).await?;

        Ok(())
    }

    async fn append_federation_aleph_bft_backup_bytes(
        &mut self,
        session: u64,
        bytes: &[u8],
    ) -> Result<(), Self::Err> {
        if bytes.is_empty() {
            return Ok(());
        }

        query(
            r#"
            INSERT INTO federation_aleph_bft_backups
            (session, chunk_index, backup_bytes)
            SELECT
                :session,
                COALESCE(MAX(chunk_index), -1) + 1,
                :backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session
            "#,
        )?
        .bind("session", session as i64)
        .bind("backup_bytes", bytes)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn replace_federation_aleph_bft_backup_chunks(
        &mut self,
        session: u64,
        chunks: &[Vec<u8>],
    ) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM federation_aleph_bft_backups
            WHERE session = :session
            "#,
        )?
        .bind("session", session as i64)
        .execute(&self.inner)
        .await?;

        for chunk in chunks {
            self.append_federation_aleph_bft_backup_bytes(session, chunk)
                .await?;
        }

        Ok(())
    }

    async fn set_federation_aleph_bft_active_session(
        &mut self,
        session: u64,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_aleph_bft_session_state
            (session_key, active_session)
            VALUES
            (:session_key, :active_session)
            ON CONFLICT (session_key)
            DO UPDATE SET active_session =
                CASE
                    WHEN federation_aleph_bft_session_state.active_session < EXCLUDED.active_session
                    THEN EXCLUDED.active_session
                    ELSE federation_aleph_bft_session_state.active_session
                END
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .bind("active_session", session as i64)
        .execute(&self.inner)
        .await?;

        let active_session_row = query(
            r#"
            SELECT active_session
            FROM federation_aleph_bft_session_state
            WHERE session_key = :session_key
            FOR UPDATE
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .fetch_one(&self.inner)
        .await?
        .ok_or_else(|| {
            Error::Internal(
                "Federation AlephBFT active session disappeared during update".to_string(),
            )
        })?;
        let active_session_column = active_session_row.into_iter().next().ok_or_else(|| {
            Error::Internal(
                "Federation AlephBFT active session row had no value during update".to_string(),
            )
        })?;
        let active_session: u64 = column_as_number!(active_session_column);

        let pending_operations = query(
            r#"
            SELECT
                operation_id,
                session,
                envelope_json
            FROM federation_aleph_bft_pending_operations
            ORDER BY operation_id ASC
            FOR UPDATE
            "#,
        )?
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_pending_operation)
        .collect::<Result<Vec<_>, _>>()?;
        if let Some(future) = pending_operations
            .iter()
            .find(|operation| operation.session_id > active_session)
        {
            return Err(Error::Internal(format!(
                "Federation AlephBFT pending operation {} belongs to future session {} while active session is {}",
                future.operation_id, future.session_id, active_session
            )));
        }

        query(
            r#"
            UPDATE federation_aleph_bft_pending_operations
            SET session = :active_session
            WHERE session < :active_session
            "#,
        )?
        .bind("active_session", active_session as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_aleph_bft_pending_operation(
        &mut self,
        operation: &FederationAlephBftPendingOperationRecord,
    ) -> Result<(), Self::Err> {
        let computed_operation_id = operation.envelope.operation_id().map_err(|err| {
            Error::Internal(format!(
                "Invalid federation AlephBFT pending operation envelope: {err}"
            ))
        })?;
        if computed_operation_id != operation.operation_id {
            return Err(Error::Internal(format!(
                "Federation AlephBFT pending operation ID mismatch: stored {}, envelope {}",
                operation.operation_id, computed_operation_id
            )));
        }

        let active_session_row = query(
            r#"
            SELECT active_session
            FROM federation_aleph_bft_session_state
            WHERE session_key = :session_key
            FOR UPDATE
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .fetch_one(&self.inner)
        .await?;
        let active_session = match active_session_row {
            Some(row) => {
                let column = row.into_iter().next().ok_or_else(|| {
                    Error::Internal(
                        "Federation AlephBFT active session row had no value during pending insert"
                            .to_string(),
                    )
                })?;
                Some(column_as_number!(column))
            }
            None => None,
        };
        if active_session.is_some_and(|active| operation.session_id < active) {
            return Err(Error::Internal(format!(
                "Federation AlephBFT pending operation {} belongs to stale session {}",
                operation.operation_id, operation.session_id
            )));
        }

        query(
            r#"
            INSERT INTO federation_aleph_bft_pending_operations
            (operation_id, session, envelope_json)
            VALUES
            (:operation_id, :session, :envelope_json)
            ON CONFLICT (operation_id)
            DO UPDATE SET
                session = EXCLUDED.session,
                envelope_json = EXCLUDED.envelope_json
            "#,
        )?
        .bind("operation_id", operation.operation_id.to_string())
        .bind("session", operation.session_id as i64)
        .bind("envelope_json", serde_json::to_string(&operation.envelope)?)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn remove_federation_aleph_bft_pending_operation(
        &mut self,
        operation_id: FederationOperationId,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM federation_aleph_bft_pending_operations
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> FederationJournalDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_federation_config(
        &self,
        federation_id: FederationId,
    ) -> Result<Option<FederationConfigSnapshot>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let mut rows = query(
            r#"
            SELECT
                federation_id,
                config_digest,
                config_json
            FROM federation_config
            WHERE federation_id = :federation_id
            "#,
        )?
        .bind("federation_id", federation_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter();
        match rows.next() {
            Some(row) => sql_row_to_federation_config_snapshot(row).map(Some),
            None => Ok(None),
        }
    }

    async fn get_federation_journal_entries(
        &self,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_journal_entry_select!(
            "ORDER BY operation_index ASC"
        ))?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_journal_entry)
        .collect()
    }

    async fn get_federation_journal_tip(&self) -> Result<Option<FederationJournalTip>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT operation_index, order_digest, status
            FROM federation_journal_entries
            ORDER BY operation_index DESC
            LIMIT 1
            "#,
        )?
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_journal_tip)
        .transpose()
    }

    async fn get_federation_journal_entry_by_operation_id(
        &self,
        operation_id: FederationOperationId,
    ) -> Result<Option<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_journal_entry_select!(
            "WHERE operation_id = :operation_id"
        ))?
        .bind("operation_id", operation_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_journal_entry)
        .transpose()
    }

    async fn get_federation_journal_entries_from(
        &self,
        next_index: u64,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_journal_entry_select!(
            "WHERE operation_index >= :next_index ORDER BY operation_index ASC"
        ))?
        .bind("next_index", next_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_journal_entry)
        .collect()
    }

    async fn get_federation_dkg_member_sequences(
        &self,
    ) -> Result<std::collections::BTreeMap<MemberId, u64>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = query(
            r#"
            SELECT member_id, last_sequence
            FROM federation_dkg_member_sequences
            ORDER BY member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?;

        let mut sequences = std::collections::BTreeMap::new();
        for row in rows {
            unpack_into!(let (member_id, last_sequence) = row);
            let member_id_raw: u64 = column_as_number!(member_id);
            let member_id = MemberId::new(member_id_raw as u16)
                .map_err(|e| Error::Internal(format!("Invalid federation member id: {e}")))?;
            let last_sequence: u64 = column_as_number!(last_sequence);
            sequences.insert(member_id, last_sequence);
        }

        Ok(sequences)
    }

    async fn get_federation_checkpoint_frontier(
        &self,
    ) -> Result<FederationCheckpointFrontier, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                journal_next_index,
                journal_order_digest,
                finalized_next_item_index,
                finalized_item_digest,
                application_next_index,
                application_digest,
                trusted_checkpoint_id,
                latest_audited_checkpoint_id,
                latest_materialized_state_digest,
                materialized_state_audit_mismatch
            FROM federation_checkpoint_state
            WHERE state_key = :state_key
            "#,
        )?
        .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_checkpoint_frontier)
        .transpose()
        .map(|frontier| frontier.unwrap_or_else(FederationCheckpointFrontier::genesis))
    }

    async fn get_federation_checkpoint(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Option<FederationCheckpoint>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT checkpoint_id, checkpoint_json
            FROM federation_checkpoints
            WHERE checkpoint_id = :checkpoint_id
            "#,
        )?
        .bind("checkpoint_id", checkpoint_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_checkpoint)
        .transpose()
    }

    async fn get_federation_checkpoints_after(
        &self,
        finalized_next_item_index: u64,
    ) -> Result<Vec<FederationCheckpoint>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT checkpoint_id, checkpoint_json
            FROM federation_checkpoints
            WHERE finalized_next_item_index > :finalized_next_item_index
            ORDER BY
                finalized_next_item_index ASC,
                journal_next_index ASC,
                scheduled_checkpoint_sequence ASC,
                checkpoint_kind DESC,
                checkpoint_id ASC
            "#,
        )?
        .bind(
            "finalized_next_item_index",
            finalized_next_item_index as i64,
        )
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_checkpoint)
        .collect()
    }

    async fn get_federation_checkpoint_signatures(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Vec<FederationCheckpointSignature>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT checkpoint_id, signer_member_id, signature_json
            FROM federation_checkpoint_signatures
            WHERE checkpoint_id = :checkpoint_id
            ORDER BY signer_member_id ASC
            "#,
        )?
        .bind("checkpoint_id", checkpoint_id.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_checkpoint_signature)
        .collect()
    }

    async fn get_federation_checkpoint_quorum(
        &self,
        checkpoint_id: FederationCheckpointId,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT checkpoint_id, quorum_json
            FROM federation_checkpoint_quorums
            WHERE checkpoint_id = :checkpoint_id
            "#,
        )?
        .bind("checkpoint_id", checkpoint_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_checkpoint_quorum)
        .transpose()
    }

    async fn get_latest_federation_checkpoint_quorum(
        &self,
        kind: FederationCheckpointKind,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let quorum = query(
            r#"
            SELECT q.checkpoint_id, q.quorum_json
            FROM federation_checkpoint_quorums AS q
            INNER JOIN federation_checkpoints AS c
                ON c.checkpoint_id = q.checkpoint_id
            WHERE c.checkpoint_kind = :checkpoint_kind
            ORDER BY
                c.scheduled_checkpoint_sequence DESC,
                c.finalized_next_item_index DESC,
                c.journal_next_index DESC,
                c.checkpoint_id DESC
            LIMIT 1
            "#,
        )?
        .bind("checkpoint_kind", checkpoint_kind_to_str(kind))
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_checkpoint_quorum)
        .transpose()?;
        if quorum
            .as_ref()
            .is_some_and(|quorum| quorum.checkpoint.kind != kind)
        {
            return Err(Error::Internal(
                "Federation checkpoint kind index does not match the stored quorum".to_string(),
            ));
        }
        Ok(quorum)
    }

    async fn get_trusted_federation_checkpoint_quorum(
        &self,
    ) -> Result<Option<FederationCheckpointQuorum>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT q.checkpoint_id, q.quorum_json
            FROM federation_checkpoint_state AS s
            INNER JOIN federation_checkpoint_quorums AS q
                ON q.checkpoint_id = s.trusted_checkpoint_id
            WHERE s.state_key = :state_key
            "#,
        )?
        .bind("state_key", FEDERATION_CHECKPOINT_STATE_KEY)
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_federation_checkpoint_quorum)
        .transpose()
    }

    async fn get_federation_catch_up_certificates(
        &self,
    ) -> Result<Vec<FederationCatchUpCertificate>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT certificate_json
            FROM federation_catch_up_certificates
            ORDER BY from_index ASC, signer_member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_catch_up_certificate)
        .collect()
    }

    async fn get_federation_catch_up_certificates_from(
        &self,
        from_index: u64,
    ) -> Result<Vec<FederationCatchUpCertificate>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT certificate_json
            FROM federation_catch_up_certificates
            WHERE from_index >= :from_index
            ORDER BY from_index ASC, signer_member_id ASC
            "#,
        )?
        .bind("from_index", from_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_catch_up_certificate)
        .collect()
    }

    async fn get_federation_catch_up_certificates_at(
        &self,
        from_index: u64,
    ) -> Result<Vec<FederationCatchUpCertificate>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT certificate_json
            FROM federation_catch_up_certificates
            WHERE from_index = :from_index
            ORDER BY signer_member_id ASC
            "#,
        )?
        .bind("from_index", from_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_catch_up_certificate)
        .collect()
    }

    async fn get_federation_aleph_bft_finalized_items(
        &self,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_aleph_bft_finalized_item_select!(
            "ORDER BY accepted_item_index ASC"
        ))?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_finalized_item)
        .collect()
    }

    async fn get_federation_aleph_bft_finalized_items_from(
        &self,
        accepted_item_index: u64,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(federation_aleph_bft_finalized_item_select!(
            "WHERE accepted_item_index >= :accepted_item_index ORDER BY accepted_item_index ASC"
        ))?
        .bind("accepted_item_index", accepted_item_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_finalized_item)
        .collect()
    }

    async fn get_federation_aleph_bft_backup_bytes(
        &self,
        session: u64,
    ) -> Result<Option<Vec<u8>>, Self::Err> {
        let chunks = self.get_federation_aleph_bft_backup_chunks(session).await?;
        if chunks.is_empty() {
            return Ok(None);
        }

        Ok(Some(chunks.concat()))
    }

    async fn get_federation_aleph_bft_backup_chunks(
        &self,
        session: u64,
    ) -> Result<Vec<Vec<u8>>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = query(
            r#"
            SELECT backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session
            ORDER BY chunk_index ASC
            "#,
        )?
        .bind("session", session as i64)
        .fetch_all(&*conn)
        .await?;

        let mut chunks = Vec::with_capacity(rows.len());
        for mut row in rows {
            let bytes = row.remove(0);
            chunks.push(column_as_binary!(&bytes).to_vec());
        }

        Ok(chunks)
    }

    async fn get_federation_aleph_bft_backup_chunks_from(
        &self,
        session: u64,
        from_chunk: u64,
        limit: usize,
    ) -> Result<Vec<Vec<u8>>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let rows = query(
            r#"
            SELECT backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session AND chunk_index >= :from_chunk
            ORDER BY chunk_index ASC
            LIMIT :limit
            "#,
        )?
        .bind("session", session as i64)
        .bind("from_chunk", from_chunk as i64)
        .bind("limit", limit as i64)
        .fetch_all(&*conn)
        .await?;

        let mut chunks = Vec::with_capacity(rows.len());
        for mut row in rows {
            let bytes = row.remove(0);
            chunks.push(column_as_binary!(&bytes).to_vec());
        }

        Ok(chunks)
    }

    async fn get_federation_aleph_bft_active_session(&self) -> Result<Option<u64>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT active_session
            FROM federation_aleph_bft_session_state
            WHERE session_key = :session_key
            "#,
        )?
        .bind("session_key", FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY)
        .fetch_one(&*conn)
        .await?
        .map(|mut row| {
            let active_session = row.remove(0);
            Ok(column_as_number!(active_session))
        })
        .transpose()
    }

    async fn get_federation_aleph_bft_pending_operations(
        &self,
        session: u64,
    ) -> Result<Vec<FederationAlephBftPendingOperationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                operation_id,
                session,
                envelope_json
            FROM federation_aleph_bft_pending_operations
            WHERE session = :session
            ORDER BY operation_id ASC
            "#,
        )?
        .bind("session", session as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_aleph_bft_pending_operation)
        .collect()
    }
}
