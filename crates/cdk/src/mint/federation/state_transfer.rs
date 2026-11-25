//! Materialized federation state: the digest rows and their frozen snapshot
//! at a scheduled checkpoint.
//!
//! One builder turns the stored rows of [`FederationMaterializedStateSource`]
//! into digest rows. The live digest, the snapshot a scheduled checkpoint
//! freezes, and the state-transfer installer's check inside its transaction
//! (`state_install`) all use it, so a transferred state is verified with
//! exactly the encoding peers signed.

use std::collections::{BTreeSet, HashMap};

use bitcoin::hashes::{sha256, Hash};
use cdk_common::database::mint::{
    DynMintDatabase, DynMintTransaction, FederationMaterializedStateSource,
    FederationMintQuoteState, FederationStoredBlindSignature, FederationStoredPaymentObservation,
    FederationStoredProof,
};
use cdk_common::error::Error;
use cdk_common::federation::{
    federation_state_rows_at_floor, load_federation_state_rows, read_federation_state_rows,
    sort_federation_state_snapshot_rows, FederatedKeyset, FederationAddressAllocationState,
    FederationChainObservationState, FederationCheckpoint, FederationCheckpointError,
    FederationError, FederationMaterializedStateKey, FederationMaterializedStateRow,
    FederationMaterializedStateSnapshot, FederationOperationId, FederationSignatureShareRecord,
    FederationStateSnapshotRow, FederationStateTable, FederationTransactionState, MemberId,
};
use cdk_common::mint::{MeltQuote, Operation};
use cdk_common::{Amount, CurrencyUnit};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::{federation_bark_application, FederationBarkRecoveryState};
use tracing::instrument;
use uuid::Uuid;

use super::FederationRuntimeConfig;
use crate::mint::Mint;

pub(super) const MINT_QUOTES: &str = "mint_quotes";
pub(super) const MELT_QUOTES: &str = "melt_quotes";
pub(super) const COMPLETED_OPERATIONS: &str = "completed_operations";
pub(super) const FEDERATION_KEYSETS: &str = "federation_keysets";
pub(super) const PROOF_STATES: &str = "proof_states";
pub(super) const BLIND_SIGNATURES: &str = "blind_signatures";
pub(super) const PAYMENT_OBSERVATIONS: &str = "payment_observations";
pub(super) const FROST_ROOT_PUBLIC_CONFIG: &str = "federation_frost_root_public_config";
pub(super) const FROST_APPLICATION_CONFIG: &str = "federation_frost_application_config";
pub(super) const BARK_TABLE_PREFIX: &str = "federation_bark_";

pub(super) fn transfer_invalid(reason: &'static str) -> Error {
    FederationError::from(FederationCheckpointError::StateTransferInvalid { reason }).into()
}

/// Where the builder reads the key-value projections from.
enum StateReader<'a> {
    Database(&'a DynMintDatabase),
    Transaction(&'a mut DynMintTransaction),
}

impl StateReader<'_> {
    /// Digest rows of one projection's stored rows at `floor`.
    async fn projection_rows<S>(
        &mut self,
        floor: u64,
    ) -> Result<Vec<FederationMaterializedStateRow>, Error>
    where
        S: FederationStateTable,
    {
        let rows = match self {
            Self::Database(db) => read_federation_state_rows::<S>(db).await?,
            Self::Transaction(tx) => load_federation_state_rows::<S>(tx).await?,
        };
        Ok(federation_state_rows_at_floor::<S>(rows, floor)?)
    }
}

impl Mint {
    /// Build the deterministic federation materialized-state snapshot of the
    /// current state, under the retention floor of the latest scheduled
    /// checkpoint (see [`Self::federation_current_materialized_state_floor`]).
    #[instrument(skip_all)]
    pub async fn federation_materialized_state_snapshot(
        &self,
    ) -> Result<FederationMaterializedStateSnapshot, Error> {
        let floor = self.federation_current_materialized_state_floor().await?;
        self.federation_materialized_state_snapshot_at(floor).await
    }

    /// The materialized-state floor of the latest scheduled checkpoint, or
    /// zero before the first one.
    ///
    /// A member deletes terminal entities only below a floor at or below
    /// this one, so members in the same state compute the same digest at it
    /// whatever each of them already deleted.
    pub async fn federation_current_materialized_state_floor(&self) -> Result<u64, Error> {
        let db = self.localstore();
        match db
            .get_latest_federation_checkpoint(
                cdk_common::federation::FederationCheckpointKind::Scheduled,
            )
            .await?
        {
            Some(checkpoint) => Ok(db
                .get_federation_materialized_state_floor(&checkpoint)
                .await?),
            None => Ok(0),
        }
    }

    /// Build the materialized-state snapshot a checkpoint whose retention
    /// floor is `floor` covers: terminal entities retired below `floor` are
    /// left out, whether or not this member already deleted them.
    #[instrument(skip_all)]
    pub async fn federation_materialized_state_snapshot_at(
        &self,
        floor: u64,
    ) -> Result<FederationMaterializedStateSnapshot, Error> {
        Ok(FederationMaterializedStateSnapshot::from_rows(
            self.federation_state_snapshot_rows_at(floor)
                .await?
                .into_iter()
                .map(|row| row.row),
        ))
    }

    /// The current materialized state as frozen snapshot rows, sorted, with
    /// the attachments a state transfer needs, under the retention floor of
    /// the latest scheduled checkpoint.
    #[instrument(skip_all)]
    pub async fn federation_state_snapshot_rows(
        &self,
    ) -> Result<Vec<FederationStateSnapshotRow>, Error> {
        let floor = self.federation_current_materialized_state_floor().await?;
        self.federation_state_snapshot_rows_at(floor).await
    }

    /// The current materialized state as frozen snapshot rows for a
    /// checkpoint whose retention floor is `floor`.
    ///
    /// Their digest is the materialized-state digest that checkpoint signs.
    #[instrument(skip_all)]
    pub async fn federation_state_snapshot_rows_at(
        &self,
        floor: u64,
    ) -> Result<Vec<FederationStateSnapshotRow>, Error> {
        let db = self.localstore();
        let source = db.get_federation_materialized_state_source().await?;
        self.state_rows_from_source(StateReader::Database(&db), source, floor)
            .await
    }

    /// The materialized-state rows `checkpoint` covers, from the current
    /// state: the checkpoint's own retention floor decides which terminal
    /// entities are left out.
    #[instrument(skip_all)]
    pub async fn federation_state_snapshot_rows_for(
        &self,
        checkpoint: &FederationCheckpoint,
    ) -> Result<Vec<FederationStateSnapshotRow>, Error> {
        let floor = self
            .localstore()
            .get_federation_materialized_state_floor(checkpoint)
            .await?;
        self.federation_state_snapshot_rows_at(floor).await
    }

    /// Rows a state-transfer install wrote, inside its transaction. Installed
    /// rows hold nothing below the transferred checkpoint's floor, so none is
    /// left out.
    pub(super) async fn state_rows_in_transaction(
        &self,
        tx: &mut DynMintTransaction,
    ) -> Result<Vec<FederationStateSnapshotRow>, Error> {
        let source = tx.get_federation_materialized_state_source().await?;
        self.state_rows_from_source(StateReader::Transaction(tx), source, 0)
            .await
    }

    async fn state_rows_from_source(
        &self,
        mut reader: StateReader<'_>,
        source: FederationMaterializedStateSource,
        floor: u64,
    ) -> Result<Vec<FederationStateSnapshotRow>, Error> {
        let member_id = self
            .federation_config()
            .map(FederationRuntimeConfig::member_id);
        let mut rows = Vec::new();
        for root in &source.frost_root_public_configs {
            rows.push(FederationStateSnapshotRow::new(
                FederationMaterializedStateRow::new(
                    FROST_ROOT_PUBLIC_CONFIG,
                    root.root_key_epoch.to_string(),
                )
                .with_field("payload_json", serde_json::to_string(root)?),
            ));
        }
        for application in &source.frost_application_configs {
            rows.push(FederationStateSnapshotRow::new(
                FederationMaterializedStateRow::new(
                    FROST_APPLICATION_CONFIG,
                    format!(
                        "{}:{}",
                        application.application_id, application.application_key_epoch
                    ),
                )
                .with_field("payload_json", serde_json::to_string(application)?),
            ));
        }
        let mut projection_rows = reader
            .projection_rows::<FederationAddressAllocationState>(floor)
            .await?;
        projection_rows.extend(
            reader
                .projection_rows::<FederationChainObservationState>(floor)
                .await?,
        );
        projection_rows.extend(
            reader
                .projection_rows::<FederationTransactionState>(floor)
                .await?,
        );
        #[cfg(feature = "federation-bark")]
        if let Some(runtime) = self.federation_config() {
            if federation_bark_application(runtime.public_config())?.is_some() {
                projection_rows.extend(bark_rows_at_floor(&source.application_rows, floor)?);
                projection_rows.extend(
                    reader
                        .projection_rows::<FederationBarkRecoveryState>(floor)
                        .await?,
                );
            }
        }
        rows.extend(
            projection_rows
                .into_iter()
                .map(FederationStateSnapshotRow::new),
        );
        rows.extend(materialized_entity_rows(&source, member_id, floor)?);
        sort_federation_state_snapshot_rows(&mut rows);
        Ok(rows)
    }
}

/// Bark projection rows a checkpoint with retention floor `floor` covers:
/// the transitions and observations of an action retired below the floor are
/// left out. The action itself stays; it guards against replays.
#[cfg(feature = "federation-bark")]
fn bark_rows_at_floor(
    rows: &[FederationMaterializedStateRow],
    floor: u64,
) -> Result<Vec<FederationMaterializedStateRow>, Error> {
    use cdk_federation_bark::{
        FederationBarkActionRecord, FederationBarkObservationRecord, FederationBarkTransitionRecord,
    };

    fn payload<T>(row: &FederationMaterializedStateRow) -> Result<T, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let json = row
            .fields
            .get("payload_json")
            .ok_or_else(|| transfer_invalid("Bark row has no payload"))?;
        serde_json::from_str(json).map_err(|_| transfer_invalid("Bark row payload does not decode"))
    }

    let mut retired = HashMap::new();
    for row in rows
        .iter()
        .filter(|row| row.table == "federation_bark_actions")
    {
        let action: FederationBarkActionRecord = payload(row)?;
        if let Some(index) = action
            .retired_operation_index()
            .filter(|index| *index < floor)
        {
            retired.insert(action.action_id, index);
        }
    }
    let mut kept = Vec::with_capacity(rows.len());
    for row in rows {
        let action_id = match row.table.as_str() {
            "federation_bark_transitions" => {
                Some(payload::<FederationBarkTransitionRecord>(row)?.action_id)
            }
            "federation_bark_observations" => Some(
                payload::<FederationBarkObservationRecord>(row)?
                    .operation
                    .action_id(),
            ),
            _ => None,
        };
        if action_id.is_none_or(|action_id| !retired.contains_key(&action_id)) {
            kept.push(row.clone());
        }
    }
    Ok(kept)
}

/// Digest rows of the mint entities named by the key registry, and of every
/// payment observation.
fn materialized_entity_rows(
    source: &FederationMaterializedStateSource,
    member_id: Option<MemberId>,
    floor: u64,
) -> Result<Vec<FederationStateSnapshotRow>, Error> {
    let mut mint_quotes = BTreeSet::new();
    let mut melt_quotes = BTreeSet::new();
    let mut completed_operations = BTreeSet::new();
    let mut proof_ys = BTreeSet::new();
    let mut blinded_messages = BTreeSet::new();
    let mut keyset_rotations = Vec::new();
    for record in &source.state_keys {
        match &record.key {
            FederationMaterializedStateKey::MintQuote { quote } => {
                mint_quotes.insert(quote.clone());
            }
            FederationMaterializedStateKey::MeltQuote { quote } => {
                melt_quotes.insert(quote.clone());
            }
            FederationMaterializedStateKey::CompletedOperation => {
                completed_operations.insert(record.operation_id);
            }
            FederationMaterializedStateKey::ProofState { y } => {
                proof_ys.insert(*y);
            }
            FederationMaterializedStateKey::BlindSignature { blinded_message } => {
                blinded_messages.insert(*blinded_message);
            }
            FederationMaterializedStateKey::KeysetRotation { keyset } => {
                keyset_rotations.push((record.operation_index, record.operation_id, keyset));
            }
            FederationMaterializedStateKey::Application { .. } => {}
        }
    }

    let mut rows = Vec::new();
    let mut operations_by_id = source
        .completed_operations
        .iter()
        .map(|operation| (*operation.id(), operation))
        .collect::<HashMap<_, _>>();
    for operation_id in completed_operations {
        let operation_uuid = operation_id.mint_operation_uuid();
        let row = match operations_by_id.remove(&operation_uuid) {
            Some(operation) => completed_operation_row(operation_id, operation_uuid, operation),
            None => FederationMaterializedStateRow::new(
                COMPLETED_OPERATIONS,
                operation_uuid.to_string(),
            )
            .with_field("federation_operation_id", operation_id.to_string())
            .with_field("missing", "true"),
        };
        rows.push(FederationStateSnapshotRow::new(row));
    }

    for (operation_index, operation_id, keyset) in keyset_rotations {
        rows.push(FederationStateSnapshotRow::new(keyset_row(
            operation_index,
            operation_id,
            keyset,
        )?));
    }

    let mut stored_mint_quotes = source
        .mint_quotes
        .iter()
        .map(|quote| (quote.id.to_string(), quote))
        .collect::<HashMap<_, _>>();
    for quote in mint_quotes {
        let row = match stored_mint_quotes.remove(&quote) {
            Some(stored) => mint_quote_row(stored)?,
            None => FederationMaterializedStateRow::new(MINT_QUOTES, quote)
                .with_field("missing", "true"),
        };
        rows.push(FederationStateSnapshotRow::new(row));
    }

    let mut stored_melt_quotes = source
        .melt_quotes
        .iter()
        .map(|quote| (quote.id.to_string(), quote))
        .collect::<HashMap<_, _>>();
    for quote in melt_quotes {
        let row = match stored_melt_quotes.remove(&quote) {
            Some(stored) => melt_quote_row(stored)?,
            None => FederationMaterializedStateRow::new(MELT_QUOTES, quote)
                .with_field("missing", "true"),
        };
        rows.push(FederationStateSnapshotRow::new(row));
    }

    // Observations retired below the floor are left out: their quote was
    // already terminal there.
    for stored in source.payment_observations.iter().filter(|stored| {
        stored
            .terminal_operation_index
            .is_none_or(|terminal| terminal >= floor)
    }) {
        rows.push(FederationStateSnapshotRow::with_attachment(
            payment_observation_row(stored)?,
            serde_json::to_string(&stored.record.observation)?,
        ));
    }

    let mut stored_proofs = source
        .proofs
        .iter()
        .map(|proof| (proof.y, proof))
        .collect::<HashMap<_, _>>();
    for y in proof_ys {
        let row = match stored_proofs.remove(&y) {
            Some(stored) => proof_row(stored)?,
            None => FederationMaterializedStateRow::new(PROOF_STATES, y.to_hex())
                .with_field("missing", "true"),
        };
        rows.push(FederationStateSnapshotRow::new(row));
    }

    let mut stored_signatures = source
        .blind_signatures
        .iter()
        .map(|signature| (signature.blinded_message, signature))
        .collect::<HashMap<_, _>>();
    let own_shares = source
        .signature_shares
        .iter()
        .filter(|share| Some(share.member_id) == member_id)
        .map(|share| (share.blinded_secret, share))
        .collect::<HashMap<_, _>>();
    for blinded_message in blinded_messages {
        let row = match stored_signatures.remove(&blinded_message) {
            Some(stored) => blind_signature_row(stored, own_shares.get(&blinded_message).copied()),
            None => FederationMaterializedStateRow::new(BLIND_SIGNATURES, blinded_message.to_hex())
                .with_field("missing", "true"),
        };
        rows.push(FederationStateSnapshotRow::new(row));
    }
    Ok(rows)
}

fn completed_operation_row(
    operation_id: FederationOperationId,
    operation_uuid: Uuid,
    operation: &Operation,
) -> FederationMaterializedStateRow {
    FederationMaterializedStateRow::new(COMPLETED_OPERATIONS, operation_uuid.to_string())
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
        .with_field(
            "payment_amount",
            operation
                .payment_amount()
                .map(|amount| amount.to_u64().to_string())
                .unwrap_or_default(),
        )
        .with_field(
            "payment_fee",
            operation
                .payment_fee()
                .map(|amount| amount.to_u64().to_string())
                .unwrap_or_default(),
        )
}

fn keyset_row(
    operation_index: u64,
    operation_id: FederationOperationId,
    keyset: &FederatedKeyset,
) -> Result<FederationMaterializedStateRow, Error> {
    Ok(
        FederationMaterializedStateRow::new(FEDERATION_KEYSETS, keyset.id.to_string())
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

fn mint_quote_row(
    quote: &FederationMintQuoteState,
) -> Result<FederationMaterializedStateRow, Error> {
    Ok(
        FederationMaterializedStateRow::new(MINT_QUOTES, quote.id.to_string())
            .with_field("amount", optional_amount_value(quote.amount.as_ref()))
            .with_field("unit", quote.unit.to_string())
            .with_field("request", quote.request.clone())
            .with_field("expiry", quote.expiry.to_string())
            .with_field("amount_paid", quote.amount_paid.value().to_string())
            .with_field("amount_issued", quote.amount_issued.value().to_string())
            .with_field("state", quote.state.to_string())
            .with_field("payment_method", quote.payment_method.to_string())
            .with_field(
                "request_lookup_id",
                canonical_json_field(&quote.request_lookup_id)?,
            )
            .with_field(
                "pubkey",
                quote
                    .pubkey
                    .map(|pubkey| pubkey.to_hex())
                    .unwrap_or_default(),
            ),
    )
}

fn melt_quote_row(quote: &MeltQuote) -> Result<FederationMaterializedStateRow, Error> {
    Ok(
        FederationMaterializedStateRow::new(MELT_QUOTES, quote.id.to_string())
            .with_field("amount", quote.amount().value().to_string())
            .with_field("fee_reserve", quote.fee_reserve().value().to_string())
            .with_field("unit", quote.unit.to_string())
            .with_field("request", quote.request.to_string())
            .with_field("request_json", canonical_json_field(&quote.request)?)
            .with_field("state", quote.state.to_string())
            .with_field("payment_method", quote.payment_method.to_string())
            .with_field("expiry", quote.expiry.to_string())
            .with_field("options", canonical_json_field(&quote.options)?)
            .with_field("created_time", quote.created_time.to_string())
            .with_field("extra_json", canonical_json_field(&quote.extra_json)?)
            .with_field("fee_options", canonical_json_field(quote.fee_options())?)
            .with_field(
                "estimated_blocks",
                quote
                    .estimated_blocks
                    .map(|blocks| blocks.to_string())
                    .unwrap_or_default(),
            )
            .with_field(
                "selected_fee_index",
                quote
                    .selected_fee_index
                    .map(|index| index.to_string())
                    .unwrap_or_default(),
            )
            .with_field(
                "payment_proof",
                quote.payment_proof.clone().unwrap_or_default(),
            ),
    )
}

pub(super) fn payment_observation_row(
    stored: &FederationStoredPaymentObservation,
) -> Result<FederationMaterializedStateRow, Error> {
    let record = &stored.record;
    let key = format!(
        "{}:{}:{}:{}",
        record.observation.context(),
        record.observation.quote(),
        record.operation_index,
        record.operation_id
    );
    let canonical = serde_json::to_vec(&record.observation)?;
    Ok(
        FederationMaterializedStateRow::new(PAYMENT_OBSERVATIONS, key)
            .with_field("context", record.observation.context().to_string())
            .with_field("quote", record.observation.quote().to_string())
            .with_field("operation_index", record.operation_index.to_string())
            .with_field("operation_id", record.operation_id.to_string())
            .with_field("observer", record.observation.observer().to_string())
            .with_field("state", record.observation.state_label())
            .with_field("evidence_hash", sha256::Hash::hash(&canonical).to_string())
            .with_field(
                "terminal_operation_index",
                stored
                    .terminal_operation_index
                    .map(|index| index.to_string())
                    .unwrap_or_default(),
            ),
    )
}

// The stored proof comes from the accepted operation's inputs, so every
// member stores the same bytes and a state transfer can install it.
fn proof_row(stored: &FederationStoredProof) -> Result<FederationMaterializedStateRow, Error> {
    Ok(
        FederationMaterializedStateRow::new(PROOF_STATES, stored.y.to_hex())
            .with_field("state", stored.state.to_string())
            .with_field("proof", canonical_json_field(&stored.proof)?)
            .with_field("quote_id", stored.quote_id.clone().unwrap_or_default())
            .with_field(
                "operation_kind",
                stored.operation_kind.clone().unwrap_or_default(),
            )
            .with_field(
                "operation_uuid",
                stored.operation_id.clone().unwrap_or_default(),
            ),
    )
}

// Each member stores its own BLS share, so the signature itself stays out.
// The share's operation and position are consensus-fixed: they let an
// installer recompute its own share from its key share.
fn blind_signature_row(
    stored: &FederationStoredBlindSignature,
    own_share: Option<&FederationSignatureShareRecord>,
) -> FederationMaterializedStateRow {
    FederationMaterializedStateRow::new(BLIND_SIGNATURES, stored.blinded_message.to_hex())
        .with_field("amount", stored.signature.amount.to_string())
        .with_field("keyset_id", stored.signature.keyset_id.to_string())
        .with_field("quote_id", stored.quote_id.clone().unwrap_or_default())
        .with_field(
            "operation_id",
            own_share
                .map(|share| share.operation_id.to_string())
                .unwrap_or_default(),
        )
        .with_field(
            "output_index",
            own_share
                .map(|share| share.output_index.to_string())
                .unwrap_or_default(),
        )
        .with_field(
            "accepted_operation_index",
            own_share
                .and_then(|share| share.accepted_operation_index)
                .map(|index| index.to_string())
                .unwrap_or_default(),
        )
}

/// Canonical JSON of a field set once from the accepted operation, so every
/// member encodes it to the same bytes.
fn canonical_json_field<T>(value: &T) -> Result<String, Error>
where
    T: serde::Serialize + ?Sized,
{
    let bytes = cdk_common::federation::canonical::canonical_json_bytes(value)?;
    String::from_utf8(bytes).map_err(|_| Error::Internal)
}

fn optional_amount_value(amount: Option<&Amount<CurrencyUnit>>) -> String {
    amount
        .map(|amount| amount.value().to_string())
        .unwrap_or_default()
}
