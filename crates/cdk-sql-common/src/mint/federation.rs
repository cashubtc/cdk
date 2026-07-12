//! Federation journal database implementation.

use async_trait::async_trait;
use cdk_common::database::mint::{FederationJournalDatabase, FederationJournalTransaction};
use cdk_common::database::Error;
use cdk_common::federation::{
    FederationAlephBftFinalizedItem, FederationAlephBftFinalizedItemStatus,
    FederationAlephBftPendingOperationRecord, FederationCatchUpCertificate, FederationConfigDigest,
    FederationConfigSnapshot, FederationDkgActivationRequest, FederationDkgCeremonyId,
    FederationDkgCeremonyRecord, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgParticipantState, FederationDkgSecretShareRequest, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignatureRequest, FederationId, FederationJournalConsensusEvidence,
    FederationJournalDigest, FederationJournalEntry, FederationJournalTip,
    FederationMemberStatusKind, FederationMemberStatusSnapshot, FederationOperationEnvelope,
    FederationOperationId, FederationOperationStatus, FederationPaymentObservation,
    FederationPaymentObservationContext, FederationPaymentObservationRecord,
    FederationSignatureShareRecord, MemberId,
};
use cdk_common::PublicKey;

use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{
    column_as_binary, column_as_nullable_number, column_as_nullable_string, column_as_number,
    column_as_string, unpack_into,
};

const FEDERATION_DKG_COMMITMENT_PHASE: &str = "commitment";
const FEDERATION_DKG_REVEAL_PHASE: &str = "reveal";
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

    Ok(FederationJournalEntry {
        index,
        operation_id,
        envelope,
        conflict_keys,
        previous_order_digest,
        order_digest,
        consensus_evidence,
        rejection,
        status,
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

fn sql_optional_member_id(column: Column) -> Result<Option<MemberId>, Error> {
    let member_id_raw: Option<u64> = column_as_nullable_number!(column);
    let Some(member_id_raw) = member_id_raw else {
        return Ok(None);
    };
    MemberId::new(member_id_raw as u16)
        .map(Some)
        .map_err(|e| Error::Internal(format!("Invalid federation member id: {e}")))
}

fn sql_row_to_federation_member_status(
    row: Vec<Column>,
) -> Result<FederationMemberStatusSnapshot, Error> {
    unpack_into!(
        let (
            member_id,
            status,
            peer_member_id,
            local_operation_count,
            peer_operation_count,
            operation_count,
            message,
            updated_time
        ) = row
    );

    let status_label = column_as_string!(&status);
    let status = FederationMemberStatusKind::from_label(&status_label).ok_or_else(|| {
        Error::Internal(format!("Invalid federation member status: {status_label}"))
    })?;

    Ok(FederationMemberStatusSnapshot {
        member_id: sql_member_id(member_id)?,
        status,
        peer: sql_optional_member_id(peer_member_id)?,
        local_operation_count: column_as_nullable_number!(local_operation_count),
        peer_operation_count: column_as_nullable_number!(peer_operation_count),
        operation_count: column_as_nullable_number!(operation_count),
        message: column_as_nullable_string!(message),
        updated_time: column_as_number!(updated_time),
    })
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

#[async_trait]
impl<RM> FederationJournalTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_federation_dkg_ceremony(
        &mut self,
        ceremony: &FederationDkgCeremonyRecord,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_ceremonies
            (ceremony_id, payload_json)
            VALUES
            (:ceremony_id, :payload_json)
            ON CONFLICT (ceremony_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_ceremonies.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", ceremony.ceremony_id.to_string())
        .bind("payload_json", serde_json::to_string(ceremony)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_ceremony_conflict_error(ceremony.ceremony_id));
        }
        Ok(())
    }

    async fn transition_federation_dkg_ceremony(
        &mut self,
        expected: &FederationDkgCeremonyRecord,
        updated: &FederationDkgCeremonyRecord,
    ) -> Result<(), Self::Err> {
        if expected.ceremony_id != updated.ceremony_id {
            return Err(Error::Internal(
                "Federation DKG ceremony transition cannot change ceremony ID".to_string(),
            ));
        }
        let rows_affected = query(
            r#"
            UPDATE federation_dkg_ceremonies
            SET payload_json = :updated_payload_json
            WHERE ceremony_id = :ceremony_id
              AND (
                payload_json = :expected_payload_json
                OR payload_json = :updated_payload_json
              )
            "#,
        )?
        .bind("ceremony_id", expected.ceremony_id.to_string())
        .bind("expected_payload_json", serde_json::to_string(expected)?)
        .bind("updated_payload_json", serde_json::to_string(updated)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_ceremony_conflict_error(expected.ceremony_id));
        }
        Ok(())
    }

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
            (operation_index, operation_id, envelope_json, conflict_keys_json, previous_order_digest, order_digest, consensus_evidence_json, rejection_json, status)
            VALUES
            (:operation_index, :operation_id, :envelope_json, :conflict_keys_json, :previous_order_digest, :order_digest, :consensus_evidence_json, :rejection_json, :status)
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
                .rejection
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        )
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
            SET status = :status, rejection_json = NULL
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("status", status_to_str(status))
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn reject_federation_operation(
        &mut self,
        operation_id: FederationOperationId,
        rejection: &cdk_common::error::ErrorResponse,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            UPDATE federation_journal_entries
            SET status = :status, rejection_json = :rejection_json
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .bind("status", status_to_str(FederationOperationStatus::Rejected))
        .bind("rejection_json", serde_json::to_string(rejection)?)
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

    async fn set_federation_member_status(
        &mut self,
        status: &FederationMemberStatusSnapshot,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO federation_member_status
            (
                member_id,
                status,
                peer_member_id,
                local_operation_count,
                peer_operation_count,
                operation_count,
                message,
                updated_time
            )
            VALUES
            (
                :member_id,
                :status,
                :peer_member_id,
                :local_operation_count,
                :peer_operation_count,
                :operation_count,
                :message,
                :updated_time
            )
            ON CONFLICT (member_id)
            DO UPDATE SET
                status = EXCLUDED.status,
                peer_member_id = EXCLUDED.peer_member_id,
                local_operation_count = EXCLUDED.local_operation_count,
                peer_operation_count = EXCLUDED.peer_operation_count,
                operation_count = EXCLUDED.operation_count,
                message = EXCLUDED.message,
                updated_time = EXCLUDED.updated_time
            "#,
        )?
        .bind("member_id", i64::from(status.member_id.get()))
        .bind("status", status.status.as_str())
        .bind(
            "peer_member_id",
            status.peer.map(|peer| i64::from(peer.get())),
        )
        .bind(
            "local_operation_count",
            status.local_operation_count.map(|count| count as i64),
        )
        .bind(
            "peer_operation_count",
            status.peer_operation_count.map(|count| count as i64),
        )
        .bind(
            "operation_count",
            status.operation_count.map(|count| count as i64),
        )
        .bind("message", status.message.clone())
        .bind("updated_time", status.updated_time as i64)
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
            (session, backup_bytes)
            VALUES
            (:session, :backup_bytes)
            ON CONFLICT (session)
            DO UPDATE SET backup_bytes = federation_aleph_bft_backups.backup_bytes || EXCLUDED.backup_bytes
            "#,
        )?
        .bind("session", session as i64)
        .bind("backup_bytes", bytes)
        .execute(&self.inner)
        .await?;

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

        Ok(())
    }

    async fn add_federation_aleph_bft_pending_operation(
        &mut self,
        operation: &FederationAlephBftPendingOperationRecord,
    ) -> Result<(), Self::Err> {
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
            (ceremony_id, phase, member_id, payload_json)
            VALUES
            (:ceremony_id, :phase, :member_id, :payload_json)
            ON CONFLICT (ceremony_id, phase, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_setup_messages.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", commitment.ceremony_id.to_string())
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
            (ceremony_id, phase, member_id, payload_json)
            VALUES
            (:ceremony_id, :phase, :member_id, :payload_json)
            ON CONFLICT (ceremony_id, phase, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_setup_messages.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", reveal.ceremony_id.to_string())
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
            (ceremony_id, sender_id, receiver_id, payload_json)
            VALUES
            (:ceremony_id, :sender_id, :receiver_id, :payload_json)
            ON CONFLICT (ceremony_id, sender_id, receiver_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_secret_share_contributions.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", contribution.ceremony_id.to_string())
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

    async fn add_federation_dkg_participant_state(
        &mut self,
        state: &FederationDkgParticipantState,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_participant_states
            (ceremony_id, member_id, payload_json)
            VALUES
            (:ceremony_id, :member_id, :payload_json)
            ON CONFLICT (ceremony_id)
            DO UPDATE SET
                member_id = EXCLUDED.member_id,
                payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_participant_states.member_id = EXCLUDED.member_id
              AND federation_dkg_participant_states.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", state.ceremony_id().to_string())
        .bind("member_id", i64::from(state.member_id().get()))
        .bind("payload_json", serde_json::to_string(state)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_participant_state_conflict_error(state));
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

    async fn add_federation_dkg_activation(
        &mut self,
        activation: &FederationDkgActivationRequest,
    ) -> Result<(), Self::Err> {
        let rows_affected = query(
            r#"
            INSERT INTO federation_dkg_activations
            (ceremony_id, member_id, payload_json)
            VALUES
            (:ceremony_id, :member_id, :payload_json)
            ON CONFLICT (ceremony_id, member_id)
            DO UPDATE SET payload_json = EXCLUDED.payload_json
            WHERE federation_dkg_activations.payload_json = EXCLUDED.payload_json
            "#,
        )?
        .bind("ceremony_id", activation.ceremony_id.to_string())
        .bind("member_id", i64::from(activation.signer.get()))
        .bind("payload_json", serde_json::to_string(activation)?)
        .execute(&self.inner)
        .await?;
        if rows_affected == 0 {
            return Err(federation_dkg_activation_conflict_error(activation));
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
        query(
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
            ORDER BY operation_index ASC
            "#,
        )?
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
            SELECT operation_index, order_digest
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
        query(
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
            WHERE operation_id = :operation_id
            "#,
        )?
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
        query(
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

    async fn get_federation_member_status(
        &self,
        member_id: MemberId,
    ) -> Result<Option<FederationMemberStatusSnapshot>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        let mut rows = query(
            r#"
            SELECT
                member_id,
                status,
                peer_member_id,
                local_operation_count,
                peer_operation_count,
                operation_count,
                message,
                updated_time
            FROM federation_member_status
            WHERE member_id = :member_id
            "#,
        )?
        .bind("member_id", i64::from(member_id.get()))
        .fetch_all(&*conn)
        .await?
        .into_iter();
        match rows.next() {
            Some(row) => sql_row_to_federation_member_status(row).map(Some),
            None => Ok(None),
        }
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

    async fn get_federation_aleph_bft_finalized_items(
        &self,
    ) -> Result<Vec<FederationAlephBftFinalizedItem>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
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
            ORDER BY accepted_item_index ASC
            "#,
        )?
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
        query(
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
            WHERE accepted_item_index >= :accepted_item_index
            ORDER BY accepted_item_index ASC
            "#,
        )?
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
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT backup_bytes
            FROM federation_aleph_bft_backups
            WHERE session = :session
            "#,
        )?
        .bind("session", session as i64)
        .fetch_one(&*conn)
        .await?
        .map(|mut row| {
            let bytes = row.remove(0);
            Ok(column_as_binary!(&bytes))
        })
        .transpose()
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

    async fn get_federation_dkg_ceremonies(
        &self,
    ) -> Result<Vec<FederationDkgCeremonyRecord>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT ceremony_id, payload_json
            FROM federation_dkg_ceremonies
            ORDER BY ceremony_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_ceremony)
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
            ORDER BY ceremony_id ASC, member_id ASC
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
            ORDER BY ceremony_id ASC, member_id ASC
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
            ORDER BY ceremony_id ASC, sender_id ASC, receiver_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_secret_share)
        .collect()
    }

    async fn get_federation_dkg_participant_states(
        &self,
    ) -> Result<Vec<FederationDkgParticipantState>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT ceremony_id, member_id, payload_json
            FROM federation_dkg_participant_states
            ORDER BY ceremony_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_participant_state)
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

    async fn get_federation_dkg_activations(
        &self,
    ) -> Result<Vec<FederationDkgActivationRequest>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;
        query(
            r#"
            SELECT ceremony_id, member_id, payload_json
            FROM federation_dkg_activations
            ORDER BY ceremony_id ASC, member_id ASC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_federation_dkg_activation)
        .collect()
    }
}

fn sql_row_to_federation_catch_up_certificate(
    row: Vec<Column>,
) -> Result<FederationCatchUpCertificate, Error> {
    unpack_into!(let (certificate_json) = row);
    serde_json::from_str(&column_as_string!(&certificate_json))
        .map_err(|e| Error::Internal(format!("Invalid federation catch-up certificate JSON: {e}")))
}
