//! Federation persistence implementation.

macro_rules! federation_journal_entry_select {
    ($suffix:literal) => {
        concat!(
            r#"
            SELECT
                operation_index,
                operation_id,
                envelope_json,
                conflict_keys_json,
                previous_order_digest,
                order_digest,
                consensus_evidence_json,
                rejection_json,
                status
            FROM federation_journal_entries
            "#,
            $suffix
        )
    };
}

macro_rules! federation_aleph_bft_finalized_item_select {
    ($suffix:literal) => {
        concat!(
            r#"
            SELECT
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
            FROM federation_aleph_bft_finalized_items
            "#,
            $suffix
        )
    };
}

mod application;
mod dkg;
mod frost_wallet;
mod journal;

use async_trait::async_trait;
use cdk_common::database::mint::{
    FederationApplicationDatabase, FederationApplicationTransaction, FederationDkgDatabase,
    FederationDkgTransaction, FederationFrostWalletDatabase, FederationFrostWalletTransaction,
    FederationJournalDatabase, FederationJournalTransaction, FederationMintQuoteState,
};
use cdk_common::database::Error;
use cdk_common::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus,
    FederationAlephBftPendingOperationRecord, FederationApplicationState,
    FederationCatchUpCertificate, FederationConfigDigest, FederationConfigSnapshot,
    FederationDkgActivationRequest, FederationDkgCeremonyId, FederationDkgCeremonyRecord,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgParticipantState,
    FederationDkgSecretShareRequest, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignatureRequest, FederationFrostActiveEpochs,
    FederationFrostApplicationConfig, FederationFrostConsumerRegistration,
    FederationFrostDkgConfirmationRequest, FederationFrostDkgLocalState,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Record,
    FederationFrostRootPrivateConfig, FederationFrostRootPublicConfig, FederationFrostRootRecord,
    FederationId, FederationJournalConsensusEvidence, FederationJournalDigest,
    FederationJournalEntry, FederationJournalTip, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationSignatureShareRecord, MemberId,
};
use cdk_common::quote_id::QuoteId;
use cdk_common::PublicKey;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::completed_operations::get_completed_operations_by_ids_inner;
use super::quotes::{get_federation_melt_quotes_inner, get_federation_mint_quote_states_inner};
use super::{SQLMintDatabase, SQLTransaction};
use crate::database::DatabaseExecutor;
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{
    column_as_binary, column_as_nullable_string, column_as_number, column_as_string, unpack_into,
};

const FEDERATION_DKG_COMMITMENT_PHASE: &str = "commitment";
const FEDERATION_DKG_REVEAL_PHASE: &str = "reveal";
const FEDERATION_FROST_DKG_ROUND1_RECORD: &str = "round1";
const FEDERATION_FROST_DKG_ROUND2_RECORD: &str = "round2";
const FEDERATION_FROST_DKG_LOCAL_ROUND1_RECORD: &str = "local_round1";
const FEDERATION_FROST_DKG_LOCAL_ROUND2_RECORD: &str = "local_round2";
const FEDERATION_FROST_DKG_ROOT_RECORD: &str = "root";
const FEDERATION_FROST_DKG_CONFIRMATION_RECORD: &str = "confirmation";
const FEDERATION_ALEPH_BFT_ACTIVE_SESSION_KEY: &str = "active";

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

fn aleph_bft_finalized_item_status_to_str(
    status: FederationAlephBftFinalizedItemStatus,
) -> &'static str {
    match status {
        FederationAlephBftFinalizedItemStatus::Accepted => "accepted",
        FederationAlephBftFinalizedItemStatus::DuplicateOperation => "duplicate_operation",
        FederationAlephBftFinalizedItemStatus::RejectedInvalidOperation => {
            "rejected_invalid_operation"
        }
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

fn sql_row_to_federation_config_snapshot(
    row: Vec<Column>,
) -> Result<FederationConfigSnapshot, Error> {
    unpack_into!(let (federation_id, config_digest, config_json) = row);

    let federation_id = FederationId::from_hex(column_as_string!(&federation_id))
        .map_err(|e| Error::Internal(format!("Invalid federation id: {e}")))?;
    let config_digest = FederationConfigDigest::from_hex(column_as_string!(&config_digest))
        .map_err(|e| Error::Internal(format!("Invalid federation config digest: {e}")))?;
    let config = serde_json::from_str(&column_as_string!(&config_json))
        .map_err(|e| Error::Internal(format!("Invalid federation config JSON: {e}")))?;
    let snapshot = FederationConfigSnapshot::new(config)
        .map_err(|e| Error::Internal(format!("Invalid federation config snapshot: {e}")))?;
    if snapshot.federation_id != federation_id {
        return Err(Error::Internal(format!(
            "Federation config row id mismatch: expected={federation_id}, found={}",
            snapshot.federation_id
        )));
    }
    if snapshot.config_digest != config_digest {
        return Err(Error::Internal(format!(
            "Federation config row digest mismatch: expected={config_digest}, found={}",
            snapshot.config_digest
        )));
    }

    Ok(snapshot)
}

fn sql_row_to_federation_journal_entry(row: Vec<Column>) -> Result<FederationJournalEntry, Error> {
    unpack_into!(
        let (
            operation_index,
            operation_id,
            envelope_json,
            conflict_keys_json,
            previous_order_digest,
            order_digest,
            consensus_evidence_json,
            rejection_json,
            status
        ) = row
    );

    let index: u64 = column_as_number!(operation_index);
    let operation_id = FederationOperationId::from_hex(column_as_string!(&operation_id))
        .map_err(|e| Error::Internal(format!("Invalid federation operation id: {e}")))?;
    let envelope = serde_json::from_str(&column_as_string!(&envelope_json))
        .map_err(|e| Error::Internal(format!("Invalid federation envelope JSON: {e}")))?;
    let conflict_keys = serde_json::from_str(&column_as_string!(&conflict_keys_json))
        .map_err(|e| Error::Internal(format!("Invalid federation conflict keys JSON: {e}")))?;
    let previous_order_digest = cdk_common::federation::FederationJournalDigest::from_hex(
        column_as_string!(&previous_order_digest),
    )
    .map_err(|e| Error::Internal(format!("Invalid federation previous digest: {e}")))?;
    let order_digest =
        cdk_common::federation::FederationJournalDigest::from_hex(column_as_string!(&order_digest))
            .map_err(|e| Error::Internal(format!("Invalid federation order digest: {e}")))?;
    let consensus_evidence = optional_json::<FederationJournalConsensusEvidence>(
        &consensus_evidence_json,
        "journal consensus evidence",
    )?;
    let rejection = optional_json::<cdk_common::error::ErrorResponse>(
        &rejection_json,
        "federation operation rejection",
    )?;
    let status = status_from_str(&column_as_string!(&status))?;
    let application = FederationApplicationState::from_parts(index, status, rejection)
        .map_err(|error| Error::Internal(error.to_string()))?;

    Ok(FederationJournalEntry {
        index,
        operation_id,
        envelope,
        conflict_keys,
        previous_order_digest,
        order_digest,
        consensus_evidence,
        application,
    })
}

fn sql_row_to_federation_journal_tip(row: Vec<Column>) -> Result<FederationJournalTip, Error> {
    unpack_into!(let (operation_index, order_digest) = row);

    let index: u64 = column_as_number!(operation_index);
    let order_digest = FederationJournalDigest::from_hex(column_as_string!(&order_digest))
        .map_err(|e| Error::Internal(format!("Invalid federation order digest: {e}")))?;
    Ok(FederationJournalTip {
        index,
        order_digest,
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

fn sql_row_to_federation_aleph_bft_finalized_item(
    row: Vec<Column>,
) -> Result<FederationAlephBftFinalizedItem, Error> {
    unpack_into!(
        let (
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
        ) = row
    );

    let accepted_item_index: u64 = column_as_number!(accepted_item_index);
    let session: u64 = column_as_number!(session);
    let unit_finalization_index: u64 = column_as_number!(unit_finalization_index);
    let unit_submitting_member = sql_member_id(unit_submitting_member)?;
    let unit_index: u64 = column_as_number!(unit_index);
    let item_index_in_unit: u64 = column_as_number!(item_index_in_unit);
    let operation_id = match column_as_nullable_string!(&operation_id) {
        Some(value) => Some(
            FederationOperationId::from_hex(value)
                .map_err(|e| Error::Internal(format!("Invalid federation operation id: {e}")))?,
        ),
        None => None,
    };
    let item =
        serde_json::from_str::<FederationAlephBftFinalizedItem>(&column_as_string!(&item_json))
            .map_err(|e| {
                Error::Internal(format!(
                    "Invalid federation AlephBFT finalized item JSON: {e}"
                ))
            })?;
    if item.accepted_item_index != accepted_item_index {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item index mismatch: expected={accepted_item_index}, found={}",
            item.accepted_item_index
        )));
    }
    if item.session != session {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item session mismatch: expected={session}, found={}",
            item.session
        )));
    }
    if item.unit_finalization_index != unit_finalization_index {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item finalization index mismatch: expected={unit_finalization_index}, found={}",
            item.unit_finalization_index
        )));
    }
    if item.unit_submitting_member != unit_submitting_member {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item member mismatch: expected={unit_submitting_member}, found={}",
            item.unit_submitting_member
        )));
    }
    if item.unit_index != unit_index {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item unit index mismatch: expected={unit_index}, found={}",
            item.unit_index
        )));
    }
    if item.item_index_in_unit != item_index_in_unit {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item item index mismatch: expected={item_index_in_unit}, found={}",
            item.item_index_in_unit
        )));
    }
    if item.operation_id != operation_id {
        return Err(Error::Internal(
            "Federation AlephBFT finalized item operation id mismatch".to_string(),
        ));
    }
    let status = column_as_string!(&status);
    if aleph_bft_finalized_item_status_to_str(item.status) != status {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item status mismatch: expected={status}, found={}",
            aleph_bft_finalized_item_status_to_str(item.status)
        )));
    }
    let unit_body_hash = column_as_string!(&unit_body_hash);
    if item.unit_body_hash.to_hex() != unit_body_hash {
        return Err(Error::Internal(format!(
            "Federation AlephBFT finalized item unit body hash mismatch: expected={unit_body_hash}, found={}",
            item.unit_body_hash
        )));
    }

    Ok(item)
}

fn sql_member_id(column: Column) -> Result<MemberId, Error> {
    let member_id_raw: u64 = column_as_number!(column);
    MemberId::new(member_id_raw as u16)
        .map_err(|e| Error::Internal(format!("Invalid federation member id: {e}")))
}

fn sql_row_to_federation_aleph_bft_pending_operation(
    row: Vec<Column>,
) -> Result<FederationAlephBftPendingOperationRecord, Error> {
    unpack_into!(let (operation_id, session, envelope_json) = row);

    let operation_id = FederationOperationId::from_hex(column_as_string!(&operation_id))
        .map_err(|e| Error::Internal(format!("Invalid federation operation ID: {e}")))?;
    let envelope: FederationOperationEnvelope =
        serde_json::from_str(&column_as_string!(&envelope_json)).map_err(|e| {
            Error::Internal(format!(
                "Invalid federation AlephBFT pending operation envelope JSON: {e}"
            ))
        })?;
    let computed_operation_id = envelope.operation_id().map_err(|e| {
        Error::Internal(format!(
            "Invalid federation AlephBFT pending operation envelope: {e}"
        ))
    })?;
    if computed_operation_id != operation_id {
        return Err(Error::Internal(format!(
            "Federation AlephBFT pending operation ID mismatch: stored {operation_id}, envelope {computed_operation_id}"
        )));
    }

    Ok(FederationAlephBftPendingOperationRecord {
        operation_id,
        session_id: column_as_number!(session),
        envelope,
    })
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

fn sql_row_to_federation_dkg_ceremony(
    row: Vec<Column>,
) -> Result<FederationDkgCeremonyRecord, Error> {
    unpack_into!(let (ceremony_id, payload_json) = row);
    let ceremony_id = FederationDkgCeremonyId::from_hex(column_as_string!(&ceremony_id))
        .map_err(|e| Error::Internal(format!("Invalid federation DKG ceremony ID: {e}")))?;
    let ceremony =
        serde_json::from_str::<FederationDkgCeremonyRecord>(&column_as_string!(&payload_json))
            .map_err(|e| Error::Internal(format!("Invalid federation DKG ceremony JSON: {e}")))?;
    if ceremony.ceremony_id != ceremony_id {
        return Err(Error::Internal(format!(
            "Federation DKG ceremony ID mismatch: expected={ceremony_id}, found={}",
            ceremony.ceremony_id
        )));
    }
    Ok(ceremony)
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

fn sql_row_to_federation_dkg_participant_state(
    row: Vec<Column>,
) -> Result<FederationDkgParticipantState, Error> {
    unpack_into!(let (ceremony_id, member_id, payload_json) = row);
    let ceremony_id = FederationDkgCeremonyId::from_hex(column_as_string!(&ceremony_id))
        .map_err(|e| Error::Internal(format!("Invalid federation DKG ceremony ID: {e}")))?;
    let member_id = sql_member_id(member_id)?;
    let state =
        serde_json::from_str::<FederationDkgParticipantState>(&column_as_string!(&payload_json))
            .map_err(|e| {
                Error::Internal(format!(
                    "Invalid federation DKG participant state JSON: {e}"
                ))
            })?;
    if state.ceremony_id() != ceremony_id {
        return Err(Error::Internal(format!(
            "Federation DKG participant-state ceremony mismatch: expected={ceremony_id}, found={}",
            state.ceremony_id()
        )));
    }
    if state.member_id() != member_id {
        return Err(Error::Internal(format!(
            "Federation DKG participant-state member mismatch: expected={member_id}, found={}",
            state.member_id()
        )));
    }

    Ok(state)
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

fn sql_row_to_federation_dkg_activation(
    row: Vec<Column>,
) -> Result<FederationDkgActivationRequest, Error> {
    unpack_into!(let (ceremony_id, member_id, payload_json) = row);
    let ceremony_id = FederationDkgCeremonyId::from_hex(column_as_string!(&ceremony_id))
        .map_err(|e| Error::Internal(format!("Invalid federation DKG ceremony ID: {e}")))?;
    let member_id = sql_member_id(member_id)?;
    let activation =
        serde_json::from_str::<FederationDkgActivationRequest>(&column_as_string!(&payload_json))
            .map_err(|e| Error::Internal(format!("Invalid federation DKG activation JSON: {e}")))?;
    if activation.ceremony_id != ceremony_id {
        return Err(Error::Internal(format!(
            "Federation DKG activation ceremony mismatch: expected={ceremony_id}, found={}",
            activation.ceremony_id
        )));
    }
    if activation.signer != member_id {
        return Err(Error::Internal(format!(
            "Federation DKG activation member mismatch: expected={member_id}, found={}",
            activation.signer
        )));
    }

    Ok(activation)
}

fn federation_dkg_conflict_error(phase: &str, member_id: MemberId) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG {phase} for member {member_id}"
    ))
}

fn federation_dkg_ceremony_conflict_error(ceremony_id: FederationDkgCeremonyId) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG ceremony lifecycle record for {ceremony_id}"
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

fn federation_dkg_participant_state_conflict_error(state: &FederationDkgParticipantState) -> Error {
    Error::Internal(format!(
        "Conflicting local federation DKG participant state for member {} ceremony {}",
        state.member_id(),
        state.ceremony_id()
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

fn federation_aleph_bft_finalized_item_conflict_error(
    item: &FederationAlephBftFinalizedItem,
) -> Error {
    Error::Internal(format!(
        "Conflicting federation AlephBFT finalized item at accepted index {}",
        item.accepted_item_index
    ))
}

fn federation_config_conflict_error(config: &FederationConfigSnapshot) -> Error {
    Error::Internal(format!(
        "Conflicting federation config for federation {}",
        config.federation_id
    ))
}

fn is_dkg_finalization_config_update(
    existing: &FederationConfigSnapshot,
    replacement: &FederationConfigSnapshot,
) -> bool {
    if existing.federation_id != replacement.federation_id {
        return false;
    }
    if !existing.config.keysets.is_empty() || existing.config.setup_transcript.is_some() {
        return false;
    }
    if replacement.config.keysets.is_empty() || replacement.config.setup_transcript.is_none() {
        return false;
    }

    let mut stripped_replacement = replacement.config.clone();
    stripped_replacement.keysets.clear();
    stripped_replacement.setup_transcript = None;
    stripped_replacement.frost_wallet = None;
    stripped_replacement == existing.config
}

fn federation_dkg_transcript_signature_conflict_error(
    signature: &FederationDkgTranscriptSignatureRequest,
) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG transcript signature for member {} transcript {}",
        signature.signer, signature.transcript_hash
    ))
}

fn federation_dkg_activation_conflict_error(activation: &FederationDkgActivationRequest) -> Error {
    Error::Internal(format!(
        "Conflicting federation DKG activation for member {} ceremony {}",
        activation.signer, activation.ceremony_id
    ))
}

fn sql_row_to_federation_catch_up_certificate(
    row: Vec<Column>,
) -> Result<FederationCatchUpCertificate, Error> {
    unpack_into!(let (certificate_json) = row);
    serde_json::from_str(&column_as_string!(&certificate_json))
        .map_err(|e| Error::Internal(format!("Invalid federation catch-up certificate JSON: {e}")))
}
