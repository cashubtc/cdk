//! Reads of the stored rows the materialized-state digest covers, the
//! checkpoint snapshot store, and the writes a state transfer adds on top of
//! the ordinary apply path.

use std::collections::BTreeSet;
use std::str::FromStr;

use cdk_common::database::mint::{
    FederationMaterializedStateSource, FederationStoredBlindSignature, FederationStoredProof,
};
use cdk_common::federation::{
    FederationMaterializedStateKey, FederationMaterializedStateRow, FederationStateSnapshotInfo,
    FederationStateSnapshotRow, FEDERATION_STATE_SNAPSHOT_RETAINED,
};
use cdk_common::State;

use super::*;
use crate::mint::proofs::sql_row_to_proof;
use crate::mint::signatures::sql_row_to_blind_signature;

/// Rows per `IN (...)` query, so a large registry never builds one huge statement.
const SOURCE_QUERY_BATCH_SIZE: usize = 500;

/// Read every stored row the materialized-state digest covers through one
/// connection or transaction.
pub(super) async fn get_materialized_state_source<C>(
    connection: &C,
) -> Result<FederationMaterializedStateSource, Error>
where
    C: DatabaseExecutor,
{
    let state_keys = query(
        r#"
        SELECT state_kind, state_key, operation_index, operation_id, record_json
        FROM federation_materialized_state_keys
        ORDER BY state_kind ASC, state_key ASC
        "#,
    )?
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(sql_row_to_federation_materialized_state_key)
    .collect::<Result<Vec<_>, _>>()?;

    let mut mint_quotes = BTreeSet::new();
    let mut melt_quotes = BTreeSet::new();
    let mut completed_operations = BTreeSet::new();
    let mut proof_ys = BTreeSet::new();
    let mut blinded_messages = BTreeSet::new();
    for record in &state_keys {
        match &record.key {
            FederationMaterializedStateKey::MintQuote { quote } => {
                mint_quotes.insert(quote.clone());
            }
            FederationMaterializedStateKey::MeltQuote { quote } => {
                melt_quotes.insert(quote.clone());
            }
            FederationMaterializedStateKey::CompletedOperation => {
                completed_operations.insert(record.operation_id.mint_operation_uuid());
            }
            FederationMaterializedStateKey::ProofState { y } => {
                proof_ys.insert(*y);
            }
            FederationMaterializedStateKey::BlindSignature { blinded_message } => {
                blinded_messages.insert(*blinded_message);
            }
            FederationMaterializedStateKey::KeysetRotation { .. }
            | FederationMaterializedStateKey::Application { .. } => {}
        }
    }

    let mint_quote_ids = mint_quotes
        .iter()
        .map(|quote| QuoteId::from_str(quote))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Error::Internal(format!("Invalid federation mint quote id: {e}")))?;
    let mut stored_mint_quotes = Vec::new();
    for chunk in mint_quote_ids.chunks(SOURCE_QUERY_BATCH_SIZE) {
        stored_mint_quotes.extend(
            get_federation_mint_quote_states_inner(connection, chunk)
                .await?
                .into_iter()
                .flatten(),
        );
    }

    let melt_quote_ids = melt_quotes
        .iter()
        .map(|quote| QuoteId::from_str(quote))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Error::Internal(format!("Invalid federation melt quote id: {e}")))?;
    let mut stored_melt_quotes = Vec::new();
    for chunk in melt_quote_ids.chunks(SOURCE_QUERY_BATCH_SIZE) {
        stored_melt_quotes.extend(get_federation_melt_quotes_inner(connection, chunk).await?);
    }

    let completed_operations = completed_operations.into_iter().collect::<Vec<_>>();
    let mut stored_operations = Vec::new();
    for chunk in completed_operations.chunks(SOURCE_QUERY_BATCH_SIZE) {
        stored_operations.extend(get_completed_operations_by_ids_inner(connection, chunk).await?);
    }

    let proof_ys = proof_ys.into_iter().collect::<Vec<_>>();
    let mut proofs = Vec::new();
    for chunk in proof_ys.chunks(SOURCE_QUERY_BATCH_SIZE) {
        proofs.extend(get_stored_proofs(connection, chunk).await?);
    }

    let blinded_messages = blinded_messages.into_iter().collect::<Vec<_>>();
    let mut blind_signatures = Vec::new();
    let mut signature_shares = Vec::new();
    for chunk in blinded_messages.chunks(SOURCE_QUERY_BATCH_SIZE) {
        blind_signatures.extend(get_stored_blind_signatures(connection, chunk).await?);
        signature_shares.extend(get_signature_shares(connection, chunk).await?);
    }

    let payment_observations = query(
        r#"
        SELECT operation_id, operation_index, observation_json
        FROM federation_payment_observations
        ORDER BY operation_index ASC, observer_member_id ASC
        "#,
    )?
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(sql_row_to_federation_payment_observation_record)
    .collect::<Result<Vec<_>, _>>()?;

    Ok(FederationMaterializedStateSource {
        frost_root_public_configs: super::frost_wallet::fetch_root_public_configs(connection)
            .await?,
        frost_application_configs: super::frost_wallet::fetch_application_configs(connection)
            .await?,
        state_keys,
        completed_operations: stored_operations,
        mint_quotes: stored_mint_quotes,
        melt_quotes: stored_melt_quotes,
        payment_observations,
        proofs,
        blind_signatures,
        signature_shares,
        application_rows: get_application_rows(connection).await?,
    })
}

async fn get_stored_proofs<C>(
    connection: &C,
    ys: &[PublicKey],
) -> Result<Vec<FederationStoredProof>, Error>
where
    C: DatabaseExecutor,
{
    if ys.is_empty() {
        return Ok(Vec::new());
    }
    query(
        r#"
        SELECT amount, keyset_id, secret, c, witness, y, state, quote_id, operation_kind, operation_id
        FROM proof
        WHERE y IN (:ys)
        ORDER BY y ASC
        "#,
    )?
    .bind_vec("ys", ys.iter().map(|y| y.to_bytes().to_vec()).collect())?
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(|mut row| {
        let operation_id = column_as_nullable_string!(row.pop().ok_or(Error::InvalidDbResponse)?);
        let operation_kind =
            column_as_nullable_string!(row.pop().ok_or(Error::InvalidDbResponse)?);
        let quote_id = column_as_nullable_string!(row.pop().ok_or(Error::InvalidDbResponse)?);
        let state = column_as_string!(row.pop().ok_or(Error::InvalidDbResponse)?, State::from_str);
        let y = column_as_string!(
            row.pop().ok_or(Error::InvalidDbResponse)?,
            PublicKey::from_hex,
            PublicKey::from_slice
        );
        let proof = sql_row_to_proof(row)?;
        Ok(FederationStoredProof {
            y,
            proof,
            state,
            quote_id,
            operation_kind,
            operation_id,
        })
    })
    .collect()
}

async fn get_stored_blind_signatures<C>(
    connection: &C,
    blinded_messages: &[PublicKey],
) -> Result<Vec<FederationStoredBlindSignature>, Error>
where
    C: DatabaseExecutor,
{
    if blinded_messages.is_empty() {
        return Ok(Vec::new());
    }
    query(
        r#"
        SELECT keyset_id, amount, c, dleq_e, dleq_s, blinded_message, quote_id
        FROM blind_signature
        WHERE blinded_message IN (:blinded_messages) AND c IS NOT NULL
        ORDER BY blinded_message ASC
        "#,
    )?
    .bind_vec(
        "blinded_messages",
        blinded_messages
            .iter()
            .map(|message| message.to_bytes().to_vec())
            .collect(),
    )?
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(|mut row| {
        let quote_id = column_as_nullable_string!(row.pop().ok_or(Error::InvalidDbResponse)?);
        let blinded_message = column_as_string!(
            row.pop().ok_or(Error::InvalidDbResponse)?,
            PublicKey::from_hex,
            PublicKey::from_slice
        );
        Ok(FederationStoredBlindSignature {
            blinded_message,
            signature: sql_row_to_blind_signature(row)?,
            quote_id,
        })
    })
    .collect()
}

async fn get_signature_shares<C>(
    connection: &C,
    blinded_messages: &[PublicKey],
) -> Result<Vec<FederationSignatureShareRecord>, Error>
where
    C: DatabaseExecutor,
{
    if blinded_messages.is_empty() {
        return Ok(Vec::new());
    }
    query(
        r#"
        SELECT share_json
        FROM federation_signature_shares
        WHERE blinded_message IN (:blinded_messages)
        ORDER BY operation_id ASC, output_index ASC, member_id ASC
        "#,
    )?
    .bind_vec(
        "blinded_messages",
        blinded_messages
            .iter()
            .map(|message| message.to_bytes().to_vec())
            .collect(),
    )?
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(sql_row_to_federation_signature_share_record)
    .collect()
}

#[cfg(feature = "federation-bark")]
async fn get_application_rows<C>(
    connection: &C,
) -> Result<Vec<FederationMaterializedStateRow>, Error>
where
    C: DatabaseExecutor,
{
    super::bark_projection::get_bark_projection_snapshot(connection)
        .await?
        .materialized_rows()
        .map_err(|e| Error::Internal(format!("Invalid Bark projection rows: {e}")))
}

#[cfg(not(feature = "federation-bark"))]
async fn get_application_rows<C>(
    _connection: &C,
) -> Result<Vec<FederationMaterializedStateRow>, Error>
where
    C: DatabaseExecutor,
{
    Ok(Vec::new())
}

/// Write key registry records exactly as given.
pub(super) async fn add_materialized_state_keys<C>(
    connection: &C,
    records: &[FederationMaterializedStateKeyRecord],
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    for record in records {
        query(
            r#"
            INSERT INTO federation_materialized_state_keys
            (state_kind, state_key, operation_index, operation_id, record_json)
            VALUES
            (:state_kind, :state_key, :operation_index, :operation_id, :record_json)
            "#,
        )?
        .bind("state_kind", record.kind_label())
        .bind("state_key", record.row_key())
        .bind("operation_index", record.operation_index as i64)
        .bind("operation_id", record.operation_id.to_string())
        .bind("record_json", serde_json::to_string(record)?)
        .execute(connection)
        .await?;
    }
    Ok(())
}

#[cfg(feature = "federation-bark")]
pub(super) async fn install_application_rows<C>(
    connection: &C,
    rows: &[FederationMaterializedStateRow],
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    super::bark_projection::install_bark_projection_rows(connection, rows).await
}

#[cfg(not(feature = "federation-bark"))]
pub(super) async fn install_application_rows<C>(
    _connection: &C,
    rows: &[FederationMaterializedStateRow],
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    if rows.is_empty() {
        return Ok(());
    }
    Err(Error::Internal(
        "Federation application rows need application support in this build".to_string(),
    ))
}

/// Store a scheduled checkpoint's frozen rows and keep only the latest
/// [`FEDERATION_STATE_SNAPSHOT_RETAINED`] scheduled snapshots.
pub(super) async fn add_state_snapshot<C>(
    connection: &C,
    info: &FederationStateSnapshotInfo,
    rows: &[FederationStateSnapshotRow],
) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let rows_affected = query(
        r#"
        INSERT INTO federation_state_snapshots
        (checkpoint_id, scheduled_checkpoint_sequence, row_count, materialized_state_digest)
        VALUES
        (:checkpoint_id, :scheduled_checkpoint_sequence, :row_count, :materialized_state_digest)
        ON CONFLICT (checkpoint_id) DO NOTHING
        "#,
    )?
    .bind("checkpoint_id", info.checkpoint_id.to_string())
    .bind(
        "scheduled_checkpoint_sequence",
        info.scheduled_checkpoint_sequence as i64,
    )
    .bind("row_count", info.row_count as i64)
    .bind(
        "materialized_state_digest",
        info.materialized_state_digest.to_string(),
    )
    .execute(connection)
    .await?;
    if rows_affected == 0 {
        // A recovered capture stores the same checkpoint again; the rows
        // already stored hash to the same signed digest.
        let stored = get_state_snapshot_info(connection, info.checkpoint_id).await?;
        if stored.as_ref() != Some(info) {
            return Err(Error::Internal(
                "Federation state snapshot conflicts with the stored one".to_string(),
            ));
        }
        return Ok(());
    }
    for (index, row) in rows.iter().enumerate() {
        query(
            r#"
            INSERT INTO federation_state_snapshot_rows (checkpoint_id, row_index, row_json)
            VALUES (:checkpoint_id, :row_index, :row_json)
            "#,
        )?
        .bind("checkpoint_id", info.checkpoint_id.to_string())
        .bind("row_index", index as i64)
        .bind("row_json", serde_json::to_string(row)?)
        .execute(connection)
        .await?;
    }

    let stale = query(
        r#"
        SELECT checkpoint_id
        FROM federation_state_snapshots
        ORDER BY scheduled_checkpoint_sequence DESC
        "#,
    )?
    .fetch_all(connection)
    .await?
    .into_iter()
    .skip(FEDERATION_STATE_SNAPSHOT_RETAINED as usize)
    .map(|row| {
        unpack_into!(let (checkpoint_id) = row);
        Ok(column_as_string!(&checkpoint_id))
    })
    .collect::<Result<Vec<_>, Error>>()?;
    for checkpoint_id in stale {
        query(
            r#"DELETE FROM federation_state_snapshot_rows WHERE checkpoint_id = :checkpoint_id"#,
        )?
        .bind("checkpoint_id", checkpoint_id.clone())
        .execute(connection)
        .await?;
        query(r#"DELETE FROM federation_state_snapshots WHERE checkpoint_id = :checkpoint_id"#)?
            .bind("checkpoint_id", checkpoint_id)
            .execute(connection)
            .await?;
    }
    Ok(())
}

fn sql_row_to_state_snapshot_info(row: Vec<Column>) -> Result<FederationStateSnapshotInfo, Error> {
    unpack_into!(
        let (checkpoint_id, scheduled_checkpoint_sequence, row_count, materialized_state_digest) = row
    );
    Ok(FederationStateSnapshotInfo {
        checkpoint_id: FederationCheckpointId::from_hex(column_as_string!(&checkpoint_id))
            .map_err(|e| Error::Internal(format!("Invalid federation checkpoint ID: {e}")))?,
        scheduled_checkpoint_sequence: column_as_number!(scheduled_checkpoint_sequence),
        row_count: column_as_number!(row_count),
        materialized_state_digest: FederationMaterializedStateDigest::from_hex(column_as_string!(
            &materialized_state_digest
        ))
        .map_err(|e| Error::Internal(format!("Invalid materialized-state digest: {e}")))?,
    })
}

async fn get_state_snapshot_info<C>(
    connection: &C,
    checkpoint_id: FederationCheckpointId,
) -> Result<Option<FederationStateSnapshotInfo>, Error>
where
    C: DatabaseExecutor,
{
    query(
        r#"
        SELECT checkpoint_id, scheduled_checkpoint_sequence, row_count, materialized_state_digest
        FROM federation_state_snapshots
        WHERE checkpoint_id = :checkpoint_id
        "#,
    )?
    .bind("checkpoint_id", checkpoint_id.to_string())
    .fetch_one(connection)
    .await?
    .map(sql_row_to_state_snapshot_info)
    .transpose()
}

pub(super) async fn get_state_snapshots<C>(
    connection: &C,
) -> Result<Vec<FederationStateSnapshotInfo>, Error>
where
    C: DatabaseExecutor,
{
    query(
        r#"
        SELECT checkpoint_id, scheduled_checkpoint_sequence, row_count, materialized_state_digest
        FROM federation_state_snapshots
        ORDER BY scheduled_checkpoint_sequence ASC
        "#,
    )?
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(sql_row_to_state_snapshot_info)
    .collect()
}

pub(super) async fn get_state_snapshot_rows<C>(
    connection: &C,
    checkpoint_id: FederationCheckpointId,
    from_row_index: u64,
    limit: usize,
) -> Result<Vec<FederationStateSnapshotRow>, Error>
where
    C: DatabaseExecutor,
{
    let end = from_row_index.saturating_add(limit as u64);
    query(
        r#"
        SELECT row_json
        FROM federation_state_snapshot_rows
        WHERE checkpoint_id = :checkpoint_id
          AND row_index >= :from_row_index
          AND row_index < :end_row_index
        ORDER BY row_index ASC
        "#,
    )?
    .bind("checkpoint_id", checkpoint_id.to_string())
    .bind("from_row_index", from_row_index as i64)
    .bind("end_row_index", i64::try_from(end).unwrap_or(i64::MAX))
    .fetch_all(connection)
    .await?
    .into_iter()
    .map(|row| {
        unpack_into!(let (row_json) = row);
        serde_json::from_str::<FederationStateSnapshotRow>(&column_as_string!(&row_json))
            .map_err(|e| Error::Internal(format!("Invalid federation state snapshot row: {e}")))
    })
    .collect()
}
