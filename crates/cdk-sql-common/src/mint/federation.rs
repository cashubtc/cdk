//! Federation journal database implementation.

use async_trait::async_trait;
use cdk_common::database::mint::{FederationJournalDatabase, FederationJournalTransaction};
use cdk_common::database::Error;
use cdk_common::federation::{
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgSecretShareRequest,
    FederationDkgTranscriptHash, FederationDkgTranscriptSignatureRequest, FederationJournalEntry,
    FederationOperationConflictKey, FederationOperationId, FederationOperationStatus,
    FederationPaymentObservation, FederationPaymentObservationContext,
    FederationPaymentObservationRecord, FederationSessionOutcome, FederationSignatureShareRecord,
    MemberId,
};
use cdk_common::PublicKey;

use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_number, column_as_string, unpack_into};

const FEDERATION_DKG_COMMITMENT_PHASE: &str = "commitment";
const FEDERATION_DKG_REVEAL_PHASE: &str = "reveal";

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

fn payment_observation_context_to_str(
    context: FederationPaymentObservationContext,
) -> &'static str {
    match context {
        FederationPaymentObservationContext::MintQuote => "mint_quote",
        FederationPaymentObservationContext::MeltQuote => "melt_quote",
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

fn sql_row_to_federation_payment_observation_record(
    row: Vec<Column>,
) -> Result<FederationPaymentObservationRecord, Error> {
    unpack_into!(let (operation_id, operation_index, observation_json) = row);

    let operation_id = FederationOperationId::from_hex(column_as_string!(&operation_id))
        .map_err(|e| Error::Internal(format!("Invalid federation operation id: {e}")))?;
    let operation_index: u64 = column_as_number!(operation_index);
    let observation =
        serde_json::from_str::<FederationPaymentObservation>(&column_as_string!(&observation_json))
            .map_err(|e| {
                Error::Internal(format!("Invalid federation payment observation JSON: {e}"))
            })?;

    Ok(FederationPaymentObservationRecord {
        operation_id,
        operation_index,
        observation,
    })
}

fn sql_row_to_federation_signature_share_record(
    row: Vec<Column>,
) -> Result<FederationSignatureShareRecord, Error> {
    unpack_into!(let (share_json) = row);
    serde_json::from_str::<FederationSignatureShareRecord>(&column_as_string!(&share_json))
        .map_err(|e| Error::Internal(format!("Invalid federation signature share JSON: {e}")))
}

fn sql_member_id(column: Column) -> Result<MemberId, Error> {
    let member_id_raw: u64 = column_as_number!(column);
    MemberId::new(member_id_raw as u16)
        .map_err(|e| Error::Internal(format!("Invalid federation member id: {e}")))
}

fn sql_row_to_federation_dkg_commitment(
    row: Vec<Column>,
) -> Result<FederationDkgParticipantCommitment, Error> {
    unpack_into!(let (member_id, payload_json) = row);
    let member_id = sql_member_id(member_id)?;
    let commitment = serde_json::from_str::<FederationDkgParticipantCommitment>(
        &column_as_string!(&payload_json),
    )
    .map_err(|e| Error::Internal(format!("Invalid federation DKG commitment JSON: {e}")))?;
    if commitment.member_id != member_id {
        return Err(Error::Internal(format!(
            "Federation DKG commitment member mismatch: expected={member_id}, found={}",
            commitment.member_id
        )));
    }

    Ok(commitment)
}

fn sql_row_to_federation_dkg_reveal(
    row: Vec<Column>,
) -> Result<FederationDkgParticipantCommitmentReveal, Error> {
    unpack_into!(let (member_id, payload_json) = row);
    let member_id = sql_member_id(member_id)?;
    let reveal = serde_json::from_str::<FederationDkgParticipantCommitmentReveal>(
        &column_as_string!(&payload_json),
    )
    .map_err(|e| Error::Internal(format!("Invalid federation DKG reveal JSON: {e}")))?;
    if reveal.member_id != member_id {
        return Err(Error::Internal(format!(
            "Federation DKG reveal member mismatch: expected={member_id}, found={}",
            reveal.member_id
        )));
    }

    Ok(reveal)
}

fn sql_row_to_federation_dkg_secret_share(
    row: Vec<Column>,
) -> Result<FederationDkgSecretShareRequest, Error> {
    unpack_into!(let (sender_id, receiver_id, payload_json) = row);
    let sender_id = sql_member_id(sender_id)?;
    let receiver_id = sql_member_id(receiver_id)?;
    let secret_share =
        serde_json::from_str::<FederationDkgSecretShareRequest>(&column_as_string!(&payload_json))
            .map_err(|e| {
                Error::Internal(format!("Invalid federation DKG secret share JSON: {e}"))
            })?;
    if secret_share.sender != sender_id {
        return Err(Error::Internal(format!(
            "Federation DKG secret share sender mismatch: expected={sender_id}, found={}",
            secret_share.sender
        )));
    }
    if secret_share.receiver != receiver_id {
        return Err(Error::Internal(format!(
            "Federation DKG secret share receiver mismatch: expected={receiver_id}, found={}",
            secret_share.receiver
        )));
    }

    Ok(secret_share)
}

fn sql_row_to_federation_dkg_member_keyset_share(
    row: Vec<Column>,
) -> Result<FederationDkgMemberKeysetShare, Error> {
    unpack_into!(let (keyset_id, member_id, payload_json) = row);
    let keyset_id = column_as_string!(&keyset_id);
    let member_id = sql_member_id(member_id)?;
    let share = serde_json::from_str::<FederationDkgMemberKeysetShare>(&column_as_string!(
        &payload_json
    ))
    .map_err(|e| Error::Internal(format!("Invalid federation DKG keyset share JSON: {e}")))?;
    if share.keyset.id.to_string() != keyset_id {
        return Err(Error::Internal(format!(
            "Federation DKG keyset share id mismatch: expected={keyset_id}, found={}",
            share.keyset.id
        )));
    }
    if share.member_id != member_id {
        return Err(Error::Internal(format!(
            "Federation DKG keyset share member mismatch: expected={member_id}, found={}",
            share.member_id
        )));
    }

    Ok(share)
}

fn sql_row_to_federation_dkg_transcript_signature(
    row: Vec<Column>,
) -> Result<FederationDkgTranscriptSignatureRequest, Error> {
    unpack_into!(let (transcript_hash, member_id, payload_json) = row);
    let transcript_hash =
        FederationDkgTranscriptHash::from_hex(column_as_string!(&transcript_hash))
            .map_err(|e| Error::Internal(format!("Invalid federation DKG transcript hash: {e}")))?;
    let member_id = sql_member_id(member_id)?;
    let signature = serde_json::from_str::<FederationDkgTranscriptSignatureRequest>(
        &column_as_string!(&payload_json),
    )
    .map_err(|e| {
        Error::Internal(format!(
            "Invalid federation DKG transcript signature JSON: {e}"
        ))
    })?;
    if signature.transcript_hash != transcript_hash {
        return Err(Error::Internal(format!(
            "Federation DKG transcript signature hash mismatch: expected={transcript_hash}, found={}",
            signature.transcript_hash
        )));
    }
    if signature.signer != member_id {
        return Err(Error::Internal(format!(
            "Federation DKG transcript signature member mismatch: expected={member_id}, found={}",
            signature.signer
        )));
    }

    Ok(signature)
}

fn federation_dkg_conflict_error(phase: &str, member_id: MemberId) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG {phase} for member {member_id}"
    ))
}

fn federation_dkg_member_keyset_share_conflict_error(
    share: &FederationDkgMemberKeysetShare,
) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG keyset share for member {} keyset {}",
        share.member_id, share.keyset.id
    ))
}

fn federation_dkg_secret_share_conflict_error(
    contribution: &FederationDkgSecretShareRequest,
) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG secret share from member {} to member {}",
        contribution.sender, contribution.receiver
    ))
}

fn federation_signature_share_conflict_error(share: &FederationSignatureShareRecord) -> Error {
    Error::Internal(format!(
        "Conflicting federation signature share for operation {} output {} member {}",
        share.operation_id, share.output_index, share.member_id
    ))
}

fn federation_dkg_transcript_signature_conflict_error(
    signature: &FederationDkgTranscriptSignatureRequest,
) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG transcript signature for member {} transcript {}",
        signature.signer, signature.transcript_hash
    ))
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

    async fn add_federation_payment_observation(
        &mut self,
        observation: &FederationPaymentObservationRecord,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_payment_observations
            (operation_id, operation_index, context, quote, observer_member_id, state, observation_json)
            VALUES
            (:operation_id, :operation_index, :context, :quote, :observer_member_id, :state, :observation_json)
            ON CONFLICT (operation_id)
            DO UPDATE SET
                operation_index = EXCLUDED.operation_index,
                context = EXCLUDED.context,
                quote = EXCLUDED.quote,
                observer_member_id = EXCLUDED.observer_member_id,
                state = EXCLUDED.state,
                observation_json = EXCLUDED.observation_json
            "#,
        )?
        .bind("operation_id", observation.operation_id.to_string())
        .bind("operation_index", observation.operation_index as i64)
        .bind(
            "context",
            payment_observation_context_to_str(observation.observation.context()),
        )
        .bind("quote", observation.observation.quote())
        .bind(
            "observer_member_id",
            i64::from(observation.observation.observer().get()),
        )
        .bind("state", observation.observation.state_label())
        .bind("observation_json", serde_json::to_string(&observation.observation)?)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_federation_signature_share(
        &mut self,
        share: &FederationSignatureShareRecord,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_signature_shares
            (operation_id, output_index, member_id, blinded_message, amount, keyset_id, signature_share, share_json)
            VALUES
            (:operation_id, :output_index, :member_id, :blinded_message, :amount, :keyset_id, :signature_share, :share_json)
            ON CONFLICT (operation_id, output_index, member_id)
            DO UPDATE SET
                blinded_message = EXCLUDED.blinded_message,
                amount = EXCLUDED.amount,
                keyset_id = EXCLUDED.keyset_id,
                signature_share = EXCLUDED.signature_share,
                share_json = EXCLUDED.share_json
            WHERE federation_signature_shares.blinded_message = EXCLUDED.blinded_message
              AND federation_signature_shares.amount = EXCLUDED.amount
              AND federation_signature_shares.keyset_id = EXCLUDED.keyset_id
              AND federation_signature_shares.signature_share = EXCLUDED.signature_share
              AND federation_signature_shares.share_json = EXCLUDED.share_json
            "#,
        )?
        .bind("operation_id", share.operation_id.to_string())
        .bind("output_index", i64::from(share.output_index))
        .bind("member_id", i64::from(share.member_id.get()))
        .bind("blinded_message", share.blinded_secret.to_bytes())
        .bind("amount", u64::from(share.amount) as i64)
        .bind("keyset_id", share.keyset_id.to_string())
        .bind("signature_share", share.share.to_bytes().to_vec())
        .bind("share_json", serde_json::to_string(share)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_signature_share_conflict_error(share));
        }

        Ok(())
    }

    async fn add_federation_dkg_commitment(
        &mut self,
        commitment: &FederationDkgParticipantCommitment,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_setup_messages
            (phase, member_id, payload_json)
            VALUES
            (:phase, :member_id, :payload_json)
            ON CONFLICT (phase, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_setup_messages.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("phase", FEDERATION_DKG_COMMITMENT_PHASE)
        .bind("member_id", i64::from(commitment.member_id.get()))
        .bind("payload_json", serde_json::to_string(commitment)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_conflict_error(
                FEDERATION_DKG_COMMITMENT_PHASE,
                commitment.member_id,
            ));
        }

        Ok(())
    }

    async fn add_federation_dkg_reveal(
        &mut self,
        reveal: &FederationDkgParticipantCommitmentReveal,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_setup_messages
            (phase, member_id, payload_json)
            VALUES
            (:phase, :member_id, :payload_json)
            ON CONFLICT (phase, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_setup_messages.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("phase", FEDERATION_DKG_REVEAL_PHASE)
        .bind("member_id", i64::from(reveal.member_id.get()))
        .bind("payload_json", serde_json::to_string(reveal)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_conflict_error(
                FEDERATION_DKG_REVEAL_PHASE,
                reveal.member_id,
            ));
        }

        Ok(())
    }

    async fn add_federation_dkg_secret_share_contribution(
        &mut self,
        contribution: &FederationDkgSecretShareRequest,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_secret_share_contributions
            (sender_id, receiver_id, payload_json)
            VALUES
            (:sender_id, :receiver_id, :payload_json)
            ON CONFLICT (sender_id, receiver_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_secret_share_contributions.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("sender_id", i64::from(contribution.sender.get()))
        .bind("receiver_id", i64::from(contribution.receiver.get()))
        .bind("payload_json", serde_json::to_string(contribution)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_secret_share_conflict_error(contribution));
        }

        Ok(())
    }

    async fn add_federation_dkg_member_keyset_share(
        &mut self,
        share: &FederationDkgMemberKeysetShare,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_member_keyset_shares
            (keyset_id, member_id, payload_json)
            VALUES
            (:keyset_id, :member_id, :payload_json)
            ON CONFLICT (keyset_id)
            DO UPDATE SET
                member_id = EXCLUDED.member_id,
                payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_member_keyset_shares.member_id = EXCLUDED.member_id
              AND federation_dkg_member_keyset_shares.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("keyset_id", share.keyset.id.to_string())
        .bind("member_id", i64::from(share.member_id.get()))
        .bind("payload_json", serde_json::to_string(share)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_member_keyset_share_conflict_error(share));
        }

        Ok(())
    }

    async fn add_federation_dkg_transcript_signature(
        &mut self,
        signature: &FederationDkgTranscriptSignatureRequest,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_transcript_signatures
            (transcript_hash, member_id, payload_json)
            VALUES
            (:transcript_hash, :member_id, :payload_json)
            ON CONFLICT (transcript_hash, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_transcript_signatures.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("transcript_hash", signature.transcript_hash.to_string())
        .bind("member_id", i64::from(signature.signer.get()))
        .bind("payload_json", serde_json::to_string(signature)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_transcript_signature_conflict_error(
                signature,
            ));
        }

        Ok(())
    }

    async fn get_federation_payment_observations(
        &mut self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err> {
        query(
            r#"
            SELECT operation_id, operation_index, observation_json
            FROM federation_payment_observations
            WHERE context = :context
              AND quote = :quote
            ORDER BY operation_index ASC, observer_member_id ASC
            "#,
        )?
        .bind("context", payment_observation_context_to_str(context))
        .bind("quote", quote)
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_federation_payment_observation_record)
        .collect()
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

    async fn get_federation_payment_observations(
        &self,
        context: FederationPaymentObservationContext,
        quote: &str,
    ) -> Result<Vec<FederationPaymentObservationRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT operation_id, operation_index, observation_json
            FROM federation_payment_observations
            WHERE context = :context
              AND quote = :quote
            ORDER BY operation_index ASC, observer_member_id ASC
            "#,
        )?
        .bind("context", payment_observation_context_to_str(context))
        .bind("quote", quote)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_payment_observation_record)
        .collect()
    }

    async fn get_federation_signature_shares_for_output(
        &self,
        operation_id: FederationOperationId,
        output_index: u32,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT share_json
            FROM federation_signature_shares
            WHERE operation_id = :operation_id
              AND output_index = :output_index
            ORDER BY member_id ASC
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("output_index", i64::from(output_index))
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_signature_share_record)
        .collect()
    }

    async fn get_federation_signature_shares_for_blinded_message(
        &self,
        blinded_message: PublicKey,
    ) -> Result<Vec<FederationSignatureShareRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT share_json
            FROM federation_signature_shares
            WHERE blinded_message = :blinded_message
            ORDER BY operation_id ASC, output_index ASC, member_id ASC
            "#,
        )?
        .bind("blinded_message", blinded_message.to_bytes())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_signature_share_record)
        .collect()
    }

    async fn get_federation_dkg_commitments(
        &self,
    ) -> Result<Vec<FederationDkgParticipantCommitment>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT member_id, payload_json
            FROM federation_dkg_setup_messages
            WHERE phase = :phase
            ORDER BY member_id ASC
            "#,
        )?
        .bind("phase", FEDERATION_DKG_COMMITMENT_PHASE)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_commitment)
        .collect()
    }

    async fn get_federation_dkg_reveals(
        &self,
    ) -> Result<Vec<FederationDkgParticipantCommitmentReveal>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT member_id, payload_json
            FROM federation_dkg_setup_messages
            WHERE phase = :phase
            ORDER BY member_id ASC
            "#,
        )?
        .bind("phase", FEDERATION_DKG_REVEAL_PHASE)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_reveal)
        .collect()
    }

    async fn get_federation_dkg_secret_share_contributions(
        &self,
    ) -> Result<Vec<FederationDkgSecretShareRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT sender_id, receiver_id, payload_json
            FROM federation_dkg_secret_share_contributions
            ORDER BY sender_id ASC, receiver_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_secret_share)
        .collect()
    }

    async fn get_federation_dkg_member_keyset_shares(
        &self,
    ) -> Result<Vec<FederationDkgMemberKeysetShare>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT keyset_id, member_id, payload_json
            FROM federation_dkg_member_keyset_shares
            ORDER BY member_id ASC, keyset_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_member_keyset_share)
        .collect()
    }

    async fn get_federation_dkg_transcript_signatures(
        &self,
    ) -> Result<Vec<FederationDkgTranscriptSignatureRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT transcript_hash, member_id, payload_json
            FROM federation_dkg_transcript_signatures
            ORDER BY transcript_hash ASC, member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_transcript_signature)
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
