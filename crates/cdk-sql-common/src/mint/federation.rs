//! Federation journal database implementation.

use async_trait::async_trait;
use cdk_common::database::mint::{FederationJournalDatabase, FederationJournalTransaction};
use cdk_common::database::Error;
use cdk_common::federation::{
    FederationJournalEntry, FederationOperationConflictKey, FederationOperationId,
    FederationOperationStatus, FederationSessionOutcome, MemberId,
};

use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_number, column_as_string, unpack_into};

fn status_to_str(status: FederationOperationStatus) -> &'static str {
    match status {
        FederationOperationStatus::Accepted => "accepted",
        FederationOperationStatus::Applying => "applying",
        FederationOperationStatus::Applied => "applied",
        FederationOperationStatus::Rejected => "rejected",
        FederationOperationStatus::Failed => "failed",
    }
}

fn status_from_str(status: &str) -> Result<FederationOperationStatus, Error> {
    match status {
        "accepted" => Ok(FederationOperationStatus::Accepted),
        "applying" => Ok(FederationOperationStatus::Applying),
        "applied" => Ok(FederationOperationStatus::Applied),
        "rejected" => Ok(FederationOperationStatus::Rejected),
        "failed" => Ok(FederationOperationStatus::Failed),
        other => Err(Error::Internal(format!(
            "Invalid federation operation status: {other}"
        ))),
    }
}

fn optional_json<T>(column: &Column, label: &str) -> Result<Option<T>, Error>
where
    T: serde::de::DeserializeOwned,
{
    match column {
        Column::Text(value) if !value.is_empty() => serde_json::from_str(value)
            .map(Some)
            .map_err(|e| Error::Internal(format!("Invalid federation {label} JSON: {e}"))),
        Column::Text(_) | Column::Null => Ok(None),
        _ => Err(Error::Internal(format!(
            "Invalid federation {label} column type"
        ))),
    }
}

fn sql_row_to_federation_journal_entry(row: Vec<Column>) -> Result<FederationJournalEntry, Error> {
    unpack_into!(
        let (
            operation_index,
            operation_id,
            envelope_json,
            conflict_key_json,
            conflict_keys_json,
            previous_order_digest,
            order_digest,
            status
        ) = row
    );

    let index: u64 = column_as_number!(operation_index);
    let operation_id = FederationOperationId::from_hex(column_as_string!(&operation_id))
        .map_err(|e| Error::Internal(format!("Invalid federation operation id: {e}")))?;
    let envelope = serde_json::from_str(&column_as_string!(&envelope_json))
        .map_err(|e| Error::Internal(format!("Invalid federation envelope JSON: {e}")))?;
    let conflict_key =
        optional_json::<FederationOperationConflictKey>(&conflict_key_json, "conflict key")?;
    let conflict_keys = serde_json::from_str(&column_as_string!(&conflict_keys_json))
        .map_err(|e| Error::Internal(format!("Invalid federation conflict keys JSON: {e}")))?;
    let previous_order_digest = cdk_common::federation::FederationJournalDigest::from_hex(
        column_as_string!(&previous_order_digest),
    )
    .map_err(|e| Error::Internal(format!("Invalid federation previous digest: {e}")))?;
    let order_digest =
        cdk_common::federation::FederationJournalDigest::from_hex(column_as_string!(&order_digest))
            .map_err(|e| Error::Internal(format!("Invalid federation order digest: {e}")))?;
    let status = status_from_str(&column_as_string!(&status))?;

    Ok(FederationJournalEntry {
        index,
        operation_id,
        envelope,
        conflict_key,
        conflict_keys,
        previous_order_digest,
        order_digest,
        status,
    })
}

#[async_trait]
impl<RM> FederationJournalTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_federation_journal_entry(
        &mut self,
        entry: &FederationJournalEntry,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_journal_entries
            (operation_index, operation_id, envelope_json, conflict_key_json, conflict_keys_json, previous_order_digest, order_digest, status)
            VALUES
            (:operation_index, :operation_id, :envelope_json, :conflict_key_json, :conflict_keys_json, :previous_order_digest, :order_digest, :status)
            "#,
        )?
        .bind("operation_index", entry.index as i64)
        .bind("operation_id", entry.operation_id.to_string())
        .bind("envelope_json", serde_json::to_string(&entry.envelope)?)
        .bind(
            "conflict_key_json",
            entry
                .conflict_key
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
        .bind(
            "conflict_keys_json",
            serde_json::to_string(&entry.conflict_keys)?,
        )
        .bind(
            "previous_order_digest",
            entry.previous_order_digest.to_string(),
        )
        .bind("order_digest", entry.order_digest.to_string())
        .bind("status", status_to_str(entry.status))
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_federation_operation_status(
        &mut self,
        operation_id: FederationOperationId,
        status: FederationOperationStatus,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            UPDATE federation_journal_entries
            SET status = :status
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("status", status_to_str(status))
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn set_federation_member_sequence(
        &mut self,
        member_id: MemberId,
        sequence: u64,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_member_sequences
            (member_id, last_sequence)
            VALUES
            (:member_id, :last_sequence)
            ON CONFLICT (member_id)
            DO UPDATE SET last_sequence = EXCLUDED.last_sequence
            WHERE federation_member_sequences.last_sequence < EXCLUDED.last_sequence
            "#,
        )?
        .bind("member_id", i64::from(member_id.get()))
        .bind("last_sequence", sequence as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_session_outcome(
        &mut self,
        outcome: &FederationSessionOutcome,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_session_outcomes
            (from_index, signer_member_id, latest_index, operation_count, outcome_json)
            VALUES
            (:from_index, :signer_member_id, :latest_index, :operation_count, :outcome_json)
            ON CONFLICT (from_index, signer_member_id)
            DO UPDATE SET
                latest_index = EXCLUDED.latest_index,
                operation_count = EXCLUDED.operation_count,
                outcome_json = EXCLUDED.outcome_json
            "#,
        )?
        .bind("from_index", outcome.from_index as i64)
        .bind("signer_member_id", i64::from(outcome.signer.get()))
        .bind("latest_index", outcome.latest_index as i64)
        .bind("operation_count", outcome.operation_count as i64)
        .bind("outcome_json", serde_json::to_string(outcome)?)
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

    async fn get_federation_journal_entries(
        &self,
    ) -> Result<Vec<FederationJournalEntry>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT
                operation_index,
                operation_id,
                envelope_json,
                conflict_key_json,
                conflict_keys_json,
                previous_order_digest,
                order_digest,
                status
            FROM federation_journal_entries
            ORDER BY operation_index ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_journal_entry)
        .collect()
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
        query(
            r#"
            SELECT
                operation_index,
                operation_id,
                envelope_json,
                conflict_key_json,
                conflict_keys_json,
                previous_order_digest,
                order_digest,
                status
            FROM federation_journal_entries
            WHERE operation_index >= :next_index
            ORDER BY operation_index ASC
            "#,
        )?
        .bind("next_index", next_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_journal_entry)
        .collect()
    }

    async fn get_federation_member_sequences(
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
            FROM federation_member_sequences
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

    async fn get_federation_session_outcomes(
        &self,
    ) -> Result<Vec<FederationSessionOutcome>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT outcome_json
            FROM federation_session_outcomes
            ORDER BY from_index ASC, signer_member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_session_outcome)
        .collect()
    }

    async fn get_federation_session_outcomes_from(
        &self,
        from_index: u64,
    ) -> Result<Vec<FederationSessionOutcome>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT outcome_json
            FROM federation_session_outcomes
            WHERE from_index >= :from_index
            ORDER BY from_index ASC, signer_member_id ASC
            "#,
        )?
        .bind("from_index", from_index as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_session_outcome)
        .collect()
    }
}

fn sql_row_to_federation_session_outcome(
    row: Vec<Column>,
) -> Result<FederationSessionOutcome, Error> {
    unpack_into!(let (outcome_json) = row);
    serde_json::from_str(&column_as_string!(&outcome_json))
        .map_err(|e| Error::Internal(format!("Invalid federation session outcome JSON: {e}")))
}
