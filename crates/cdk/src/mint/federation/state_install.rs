//! Install a scheduled checkpoint's materialized state from its frozen
//! snapshot rows (checkpoint state transfer).
//!
//! The rows are checked against the checkpoint's quorum-signed digest before
//! anything is written, written in one transaction through the same storage
//! calls the apply path uses, and checked again by recomputing the digest
//! from what was written inside that transaction.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use cdk_common::database::mint::{Acquired, DynMintTransaction};
use cdk_common::error::Error;
use cdk_common::federation::{
    ensure_federation_state_snapshot_rows_sorted, federation_state_snapshot_digest,
    FederatedKeyset, FederationAddressAllocationState, FederationChainObservationState,
    FederationCheckpointError, FederationCheckpointKind, FederationCheckpointQuorum,
    FederationError, FederationKeysetRotationOperation, FederationMaterializedStateKey,
    FederationMaterializedStateKeyRecord, FederationMaterializedStateRow, FederationOperationId,
    FederationPaymentObservation, FederationPaymentObservationContext,
    FederationPaymentObservationRecord, FederationSignatureShareRecord, FederationStateRows,
    FederationStateSnapshotRow, FederationStateTable, FederationTransactionState,
    FEDERATION_STATE_PRIMARY_NAMESPACE,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote, MintQuote, Operation, OperationKind};
use cdk_common::nuts::{BlindedMessage, MeltQuoteState, PaymentMethod, State};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::{Amount, CurrencyUnit, Id, Proof, PublicKey};
#[cfg(feature = "federation-bark")]
use cdk_federation_bark::FederationBarkRecoveryState;
use tracing::instrument;
use uuid::Uuid;

use super::state_transfer::{
    payment_observation_row, transfer_invalid, BARK_TABLE_PREFIX, BLIND_SIGNATURES,
    COMPLETED_OPERATIONS, FEDERATION_KEYSETS, FROST_APPLICATION_CONFIG, FROST_ROOT_PUBLIC_CONFIG,
    MELT_QUOTES, MINT_QUOTES, PAYMENT_OBSERVATIONS, PROOF_STATES,
};
use super::FederationRuntimeConfig;
use crate::mint::{Mint, QuoteId};

/// Rows of a verified snapshot, grouped by what installs them.
#[derive(Default)]
struct InstallPlan {
    frost_root_public_configs: Vec<cdk_common::federation::FederationFrostRootPublicConfig>,
    frost_application_configs: Vec<cdk_common::federation::FederationFrostApplicationConfig>,
    projections: BTreeMap<String, FederationStateRows>,
    application_rows: Vec<FederationMaterializedStateRow>,
    completed_operations: Vec<(FederationOperationId, Option<Operation>)>,
    keysets: Vec<(u64, FederationOperationId, FederatedKeyset)>,
    mint_quotes: Vec<(String, Option<MintQuoteInstall>)>,
    melt_quotes: Vec<(String, Option<MeltQuote>)>,
    payment_observations: Vec<FederationPaymentObservationRecord>,
    proofs: Vec<(PublicKey, Option<ProofInstall>)>,
    blind_signatures: Vec<(PublicKey, Option<BlindSignatureInstall>)>,
}

struct MintQuoteInstall {
    quote: MintQuote,
    amount_issued: Amount<CurrencyUnit>,
}

struct ProofInstall {
    proof: Proof,
    quote_id: Option<QuoteId>,
    operation: Operation,
}

struct BlindSignatureInstall {
    output: BlindedMessage,
    quote_id: Option<QuoteId>,
    operation_id: FederationOperationId,
    output_index: u32,
    accepted_operation_index: Option<u64>,
}

fn field<'a>(row: &'a FederationMaterializedStateRow, name: &str) -> Result<&'a str, Error> {
    row.fields
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| transfer_invalid("snapshot row lacks a field"))
}

fn parsed<T>(row: &FederationMaterializedStateRow, name: &str) -> Result<T, Error>
where
    T: FromStr,
{
    field(row, name)?
        .parse()
        .map_err(|_| transfer_invalid("snapshot row field does not parse"))
}

fn optional_parsed<T>(row: &FederationMaterializedStateRow, name: &str) -> Result<Option<T>, Error>
where
    T: FromStr,
{
    match field(row, name)? {
        "" => Ok(None),
        value => value
            .parse()
            .map(Some)
            .map_err(|_| transfer_invalid("snapshot row field does not parse")),
    }
}

fn optional_string(
    row: &FederationMaterializedStateRow,
    name: &str,
) -> Result<Option<String>, Error> {
    Ok(match field(row, name)? {
        "" => None,
        value => Some(value.to_string()),
    })
}

fn json_field<T>(row: &FederationMaterializedStateRow, name: &str) -> Result<T, Error>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(field(row, name)?)
        .map_err(|_| transfer_invalid("snapshot row JSON field does not decode"))
}

fn is_missing(row: &FederationMaterializedStateRow) -> bool {
    row.fields.get("missing").map(String::as_str) == Some("true")
}

fn quote_id(value: &str) -> Result<QuoteId, Error> {
    QuoteId::from_str(value).map_err(|_| transfer_invalid("snapshot row has an invalid quote id"))
}

impl InstallPlan {
    fn from_rows(rows: &[FederationStateSnapshotRow]) -> Result<Self, Error> {
        let mut plan = Self::default();
        for snapshot_row in rows {
            let row = &snapshot_row.row;
            if snapshot_row.attachment.is_some() && row.table != PAYMENT_OBSERVATIONS {
                return Err(transfer_invalid(
                    "only payment observations carry attachments",
                ));
            }
            match row.table.as_str() {
                FROST_ROOT_PUBLIC_CONFIG => plan
                    .frost_root_public_configs
                    .push(json_field(row, "payload_json")?),
                FROST_APPLICATION_CONFIG => plan
                    .frost_application_configs
                    .push(json_field(row, "payload_json")?),
                table
                    if table == FederationAddressAllocationState::SECONDARY_NAMESPACE
                        || table == FederationChainObservationState::SECONDARY_NAMESPACE
                        || table == FederationTransactionState::SECONDARY_NAMESPACE
                        || is_bark_recovery_namespace(table) =>
                {
                    plan.projections
                        .entry(table.to_string())
                        .or_default()
                        .insert(row.key.clone(), field(row, "record")?.as_bytes().to_vec());
                }
                table if table.starts_with(BARK_TABLE_PREFIX) => {
                    plan.application_rows.push(row.clone())
                }
                COMPLETED_OPERATIONS => plan.completed_operations.push(completed_operation(row)?),
                FEDERATION_KEYSETS => plan.keysets.push(keyset(row)?),
                MINT_QUOTES => plan.mint_quotes.push(mint_quote(row)?),
                MELT_QUOTES => plan.melt_quotes.push(melt_quote(row)?),
                PAYMENT_OBSERVATIONS => plan
                    .payment_observations
                    .push(payment_observation(snapshot_row)?),
                PROOF_STATES => plan.proofs.push(proof(row)?),
                BLIND_SIGNATURES => plan.blind_signatures.push(blind_signature(row)?),
                _ => return Err(transfer_invalid("snapshot row names an unknown table")),
            }
        }
        // Every projection must load as a valid whole.
        for (namespace, rows) in &plan.projections {
            validate_projection_rows(namespace, rows)?;
        }
        Ok(plan)
    }
}

#[cfg(feature = "federation-bark")]
fn is_bark_recovery_namespace(table: &str) -> bool {
    table == FederationBarkRecoveryState::SECONDARY_NAMESPACE
}

#[cfg(not(feature = "federation-bark"))]
fn is_bark_recovery_namespace(_table: &str) -> bool {
    false
}

fn validate_projection_rows(namespace: &str, rows: &FederationStateRows) -> Result<(), Error> {
    if namespace == FederationAddressAllocationState::SECONDARY_NAMESPACE {
        FederationAddressAllocationState::from_rows(rows)?;
    } else if namespace == FederationChainObservationState::SECONDARY_NAMESPACE {
        FederationChainObservationState::from_rows(rows)?;
    } else if namespace == FederationTransactionState::SECONDARY_NAMESPACE {
        FederationTransactionState::from_rows(rows)?;
    } else {
        #[cfg(feature = "federation-bark")]
        if namespace == FederationBarkRecoveryState::SECONDARY_NAMESPACE {
            FederationBarkRecoveryState::from_rows(rows)?;
            return Ok(());
        }
        return Err(transfer_invalid("snapshot row names an unknown projection"));
    }
    Ok(())
}

fn completed_operation(
    row: &FederationMaterializedStateRow,
) -> Result<(FederationOperationId, Option<Operation>), Error> {
    let operation_id: FederationOperationId = parsed(row, "federation_operation_id")?;
    let operation_uuid = operation_id.mint_operation_uuid();
    if row.key != operation_uuid.to_string() {
        return Err(transfer_invalid(
            "completed operation key does not match its operation",
        ));
    }
    if is_missing(row) {
        return Ok((operation_id, None));
    }
    let mut operation = Operation::new(
        operation_uuid,
        parsed::<OperationKind>(row, "kind")?,
        Amount::from(parsed::<u64>(row, "total_issued")?),
        Amount::from(parsed::<u64>(row, "total_redeemed")?),
        Amount::from(parsed::<u64>(row, "fee_collected")?),
        optional_parsed(row, "completed_at")?,
        optional_parsed::<PaymentMethod>(row, "payment_method")?,
    );
    match (
        optional_parsed::<u64>(row, "payment_amount")?,
        optional_parsed::<u64>(row, "payment_fee")?,
    ) {
        (Some(amount), Some(fee)) => {
            operation.set_payment_details(Amount::from(amount), Amount::from(fee))
        }
        (None, None) => {}
        _ => {
            return Err(transfer_invalid(
                "completed operation has half its payment details",
            ))
        }
    }
    Ok((operation_id, Some(operation)))
}

fn keyset(
    row: &FederationMaterializedStateRow,
) -> Result<(u64, FederationOperationId, FederatedKeyset), Error> {
    let keyset = FederatedKeyset {
        id: Id::from_str(&row.key).map_err(|_| transfer_invalid("keyset row has an invalid id"))?,
        unit: parsed(row, "unit")?,
        active: parsed(row, "active")?,
        input_fee_ppk: parsed(row, "input_fee_ppk")?,
        final_expiry: optional_parsed(row, "final_expiry")?,
        aggregate_keys: json_field(row, "aggregate_keys")?,
        public_shares: json_field(row, "public_shares")?,
    };
    Ok((
        parsed(row, "operation_index")?,
        parsed(row, "operation_id")?,
        keyset,
    ))
}

fn mint_quote(
    row: &FederationMaterializedStateRow,
) -> Result<(String, Option<MintQuoteInstall>), Error> {
    if is_missing(row) {
        return Ok((row.key.clone(), None));
    }
    let unit: CurrencyUnit = parsed(row, "unit")?;
    let request_lookup_id: PaymentIdentifier = json_field(row, "request_lookup_id")?;
    let quote = MintQuote::new(
        Some(quote_id(&row.key)?),
        field(row, "request")?.to_string(),
        unit.clone(),
        optional_parsed::<u64>(row, "amount")?.map(|amount| Amount::new(amount, unit.clone())),
        parsed(row, "expiry")?,
        request_lookup_id,
        optional_parsed::<PublicKey>(row, "pubkey")?,
        Amount::new(0, unit.clone()),
        Amount::new(0, unit.clone()),
        parsed(row, "payment_method")?,
        // Creation time and extra fields can differ on the member that
        // created the quote, so they are not part of the signed state.
        0,
        0,
        Vec::new(),
        Vec::new(),
        None,
    );
    let amount_issued = Amount::new(parsed(row, "amount_issued")?, unit);
    Ok((
        row.key.clone(),
        Some(MintQuoteInstall {
            quote,
            amount_issued,
        }),
    ))
}

fn melt_quote(row: &FederationMaterializedStateRow) -> Result<(String, Option<MeltQuote>), Error> {
    if is_missing(row) {
        return Ok((row.key.clone(), None));
    }
    let state: MeltQuoteState = parsed(row, "state")?;
    if state == MeltQuoteState::Pending {
        return Err(transfer_invalid(
            "snapshot holds an in-flight melt, which a state transfer cannot install yet",
        ));
    }
    let request: MeltPaymentRequest = json_field(row, "request_json")?;
    let quote = MeltQuote::from_db(
        quote_id(&row.key)?,
        parsed(row, "unit")?,
        request,
        parsed(row, "amount")?,
        parsed(row, "fee_reserve")?,
        state,
        parsed(row, "expiry")?,
        optional_string(row, "payment_proof")?,
        // The paying member's processor rewrites the lookup id, so it is not
        // part of the signed state.
        None,
        json_field(row, "options")?,
        parsed(row, "created_time")?,
        None,
        parsed(row, "payment_method")?,
        json_field(row, "extra_json")?,
        optional_parsed(row, "estimated_blocks")?,
        json_field(row, "fee_options")?,
        optional_parsed(row, "selected_fee_index")?,
    )?;
    Ok((row.key.clone(), Some(quote)))
}

fn payment_observation(
    snapshot_row: &FederationStateSnapshotRow,
) -> Result<FederationPaymentObservationRecord, Error> {
    let attachment = snapshot_row
        .attachment
        .as_deref()
        .ok_or_else(|| transfer_invalid("payment observation row has no observation"))?;
    let observation: FederationPaymentObservation = serde_json::from_str(attachment)
        .map_err(|_| transfer_invalid("payment observation attachment does not decode"))?;
    let record = FederationPaymentObservationRecord {
        operation_id: parsed(&snapshot_row.row, "operation_id")?,
        operation_index: parsed(&snapshot_row.row, "operation_index")?,
        observation,
    };
    // Re-encoding checks the attachment against the row's evidence hash and
    // every other field the row carries.
    if payment_observation_row(&record)? != snapshot_row.row {
        return Err(transfer_invalid(
            "payment observation attachment does not match its row",
        ));
    }
    Ok(record)
}

fn proof(row: &FederationMaterializedStateRow) -> Result<(PublicKey, Option<ProofInstall>), Error> {
    let y = PublicKey::from_hex(&row.key)
        .map_err(|_| transfer_invalid("proof row has an invalid Y"))?;
    if is_missing(row) {
        return Ok((y, None));
    }
    let state: State = parsed(row, "state")?;
    if state != State::Spent {
        return Err(transfer_invalid(
            "snapshot holds a proof that is not spent, which a state transfer cannot install yet",
        ));
    }
    let proof: Proof = json_field(row, "proof")?;
    if proof.y()? != y {
        return Err(transfer_invalid("proof row key is not the proof's Y"));
    }
    let operation_uuid = Uuid::from_str(field(row, "operation_uuid")?)
        .map_err(|_| transfer_invalid("proof row has an invalid operation"))?;
    let operation = Operation::new(
        operation_uuid,
        parsed(row, "operation_kind")?,
        Amount::ZERO,
        Amount::ZERO,
        Amount::ZERO,
        None,
        None,
    );
    Ok((
        y,
        Some(ProofInstall {
            proof,
            quote_id: optional_string(row, "quote_id")?
                .map(|quote| quote_id(&quote))
                .transpose()?,
            operation,
        }),
    ))
}

fn blind_signature(
    row: &FederationMaterializedStateRow,
) -> Result<(PublicKey, Option<BlindSignatureInstall>), Error> {
    let blinded_message = PublicKey::from_hex(&row.key)
        .map_err(|_| transfer_invalid("blind signature row has an invalid blinded message"))?;
    if is_missing(row) {
        return Ok((blinded_message, None));
    }
    Ok((
        blinded_message,
        Some(BlindSignatureInstall {
            output: BlindedMessage::new(
                parsed(row, "amount")?,
                parsed(row, "keyset_id")?,
                blinded_message,
            ),
            quote_id: optional_string(row, "quote_id")?
                .map(|quote| quote_id(&quote))
                .transpose()?,
            operation_id: parsed(row, "operation_id")?,
            output_index: parsed(row, "output_index")?,
            accepted_operation_index: optional_parsed(row, "accepted_operation_index")?,
        }),
    ))
}

impl Mint {
    /// Install the materialized state of a scheduled checkpoint from its
    /// frozen snapshot rows, and make the checkpoint this member's trusted
    /// frontier.
    ///
    /// The member must hold no federation state yet; its key material stays.
    /// The rows must hash to the quorum-signed materialized-state digest. They
    /// are written in one transaction, the digest is recomputed from what was
    /// written inside that transaction, and the transaction commits only if it
    /// equals the signed digest. The caller then restarts the federation
    /// runtime, which starts from the checkpoint, and catches up the journal
    /// suffix with the ordinary protocol.
    #[instrument(skip_all)]
    pub async fn install_federation_state_transfer(
        &self,
        quorum: &FederationCheckpointQuorum,
        rows: &[FederationStateSnapshotRow],
    ) -> Result<(), Error> {
        let runtime = self.federation_config().ok_or(Error::Internal)?;
        let verified = quorum.verify(runtime.public_config())?;
        let checkpoint = verified.checkpoint();
        if checkpoint.kind != FederationCheckpointKind::Scheduled {
            return Err(transfer_invalid(
                "state transfers install scheduled checkpoints only",
            ));
        }
        ensure_federation_state_snapshot_rows_sorted(rows)?;
        let transferred = federation_state_snapshot_digest(rows)?;
        if transferred != checkpoint.materialized_state_digest {
            return Err(FederationError::from(
                FederationCheckpointError::StateTransferDigestMismatch {
                    expected: checkpoint.materialized_state_digest,
                    found: transferred,
                },
            )
            .into());
        }
        let plan = InstallPlan::from_rows(rows)?;
        let effective = effective_config_with_rotations(runtime, &plan.keysets)?;
        let checkpoint_id = checkpoint.id()?;
        // The operation that last wrote a quote, proof or signature lies
        // below the checkpoint and is not part of the signed state. Installed
        // registry entries name the checkpoint position instead.
        let base_operation_index = checkpoint.journal_next_index.saturating_sub(1);
        let base_operation_id = FederationOperationId::from_bytes(checkpoint_id.to_bytes());

        let mut tx = self.localstore.begin_transaction().await?;
        let outcome = async {
            self.ensure_empty_federation_state(&mut tx).await?;
            let state_keys = self
                .write_install_plan(
                    &mut tx,
                    &plan,
                    &effective,
                    base_operation_index,
                    base_operation_id,
                )
                .await?;
            tx.add_federation_materialized_state_keys(&state_keys)
                .await?;

            let installed = self.state_rows_in_transaction(&mut tx).await?;
            let installed_digest = federation_state_snapshot_digest(&installed)?;
            if installed_digest != checkpoint.materialized_state_digest {
                return Err(Error::from(FederationError::from(
                    FederationCheckpointError::StateTransferDigestMismatch {
                        expected: checkpoint.materialized_state_digest,
                        found: installed_digest,
                    },
                )));
            }
            tx.install_federation_state_transfer_frontier(&verified)
                .await?;
            Ok(())
        }
        .await;
        match outcome {
            Ok(()) => tx.commit().await?,
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        }
        self.rebuild_effective_federation_config(true).await?;
        tracing::info!(
            checkpoint = %checkpoint_id,
            journal_next_index = checkpoint.journal_next_index,
            rows = rows.len(),
            "Installed federation state transfer"
        );
        Ok(())
    }

    async fn ensure_empty_federation_state(
        &self,
        tx: &mut DynMintTransaction,
    ) -> Result<(), Error> {
        let source = tx.get_federation_materialized_state_source().await?;
        let mut empty = source.state_keys.is_empty()
            && source.payment_observations.is_empty()
            && source.application_rows.is_empty();
        for namespace in [
            FederationAddressAllocationState::SECONDARY_NAMESPACE,
            FederationChainObservationState::SECONDARY_NAMESPACE,
            FederationTransactionState::SECONDARY_NAMESPACE,
        ] {
            empty &= tx
                .kv_entries(FEDERATION_STATE_PRIMARY_NAMESPACE, namespace)
                .await?
                .is_empty();
        }
        #[cfg(feature = "federation-bark")]
        {
            empty &= tx
                .kv_entries(
                    FEDERATION_STATE_PRIMARY_NAMESPACE,
                    FederationBarkRecoveryState::SECONDARY_NAMESPACE,
                )
                .await?
                .is_empty();
        }
        if !empty {
            return Err(transfer_invalid(
                "a state transfer installs into a member without federation state",
            ));
        }
        Ok(())
    }

    async fn write_install_plan(
        &self,
        tx: &mut DynMintTransaction,
        plan: &InstallPlan,
        effective: &FederationRuntimeConfig,
        base_operation_index: u64,
        base_operation_id: FederationOperationId,
    ) -> Result<Vec<FederationMaterializedStateKeyRecord>, Error> {
        let base_key = |key| FederationMaterializedStateKeyRecord {
            operation_index: base_operation_index,
            operation_id: base_operation_id,
            key,
        };
        let mut state_keys = Vec::new();

        // Member key material may already hold the FROST configs; inserting an
        // identical config again is a no-op.
        for config in &plan.frost_root_public_configs {
            tx.add_federation_frost_root_public_config(config).await?;
        }
        for config in &plan.frost_application_configs {
            tx.add_federation_frost_application_config(config).await?;
        }
        for (namespace, rows) in &plan.projections {
            for (key, value) in rows {
                tx.kv_write(FEDERATION_STATE_PRIMARY_NAMESPACE, namespace, key, value)
                    .await?;
            }
        }
        if !plan.application_rows.is_empty() {
            tx.install_federation_application_rows(&plan.application_rows)
                .await?;
        }

        let mut operation_kinds = HashMap::new();
        for (operation_id, operation) in &plan.completed_operations {
            if let Some(operation) = operation {
                tx.add_completed_operation(operation, &HashMap::new())
                    .await?;
                operation_kinds.insert(*operation_id, operation.kind());
            }
            state_keys.push(FederationMaterializedStateKeyRecord {
                operation_index: base_operation_index,
                operation_id: *operation_id,
                key: FederationMaterializedStateKey::CompletedOperation,
            });
        }
        for (operation_index, operation_id, keyset) in &plan.keysets {
            state_keys.push(FederationMaterializedStateKeyRecord {
                operation_index: *operation_index,
                operation_id: *operation_id,
                key: FederationMaterializedStateKey::KeysetRotation {
                    keyset: keyset.clone(),
                },
            });
        }

        for record in &plan.payment_observations {
            tx.add_federation_payment_observation(record).await?;
        }

        for (quote, install) in &plan.mint_quotes {
            if let Some(install) = install {
                let mut stored = Acquired::from(install.quote.clone());
                tx.add_mint_quote(install.quote.clone()).await?;
                // Payments are rebuilt from the installed observations with
                // the quorum rule the apply path uses.
                let observations = tx
                    .get_federation_payment_observations(
                        FederationPaymentObservationContext::MintQuote,
                        quote,
                    )
                    .await?;
                self.apply_mint_payment_observations_to_quote(tx, &mut stored, &observations)
                    .await?;
                // Issuing more than the rebuilt payments cover fails here.
                if install.amount_issued.value() > 0 {
                    stored.add_issuance(install.amount_issued.clone())?;
                    tx.update_mint_quote(&mut stored).await?;
                }
            }
            state_keys.push(base_key(FederationMaterializedStateKey::MintQuote {
                quote: quote.clone(),
            }));
        }

        for (quote, install) in &plan.melt_quotes {
            if let Some(install) = install {
                tx.add_melt_quote(install.clone()).await?;
            }
            state_keys.push(base_key(FederationMaterializedStateKey::MeltQuote {
                quote: quote.clone(),
            }));
        }

        // Proofs go in per operation, as the apply path stored them, then
        // move to their spent state.
        let mut proof_groups: BTreeMap<(Uuid, Option<String>), (Operation, Vec<Proof>)> =
            BTreeMap::new();
        for (y, install) in &plan.proofs {
            if let Some(install) = install {
                proof_groups
                    .entry((
                        *install.operation.id(),
                        install.quote_id.as_ref().map(ToString::to_string),
                    ))
                    .or_insert_with(|| {
                        (
                            Operation::new(
                                *install.operation.id(),
                                install.operation.kind(),
                                Amount::ZERO,
                                Amount::ZERO,
                                Amount::ZERO,
                                None,
                                None,
                            ),
                            Vec::new(),
                        )
                    })
                    .1
                    .push(install.proof.clone());
            }
            state_keys.push(base_key(FederationMaterializedStateKey::ProofState {
                y: *y,
            }));
        }
        for ((_, quote), (operation, proofs)) in proof_groups {
            let quote = quote.map(|quote| quote_id(&quote)).transpose()?;
            let mut stored = tx.add_proofs(proofs, quote, &operation).await?;
            Mint::update_proofs_state(tx, &mut stored, State::Spent).await?;
        }

        // Each member stores its own share: recompute it from this member's
        // key share for the output the signed rows name.
        let mut signature_groups: BTreeMap<
            (FederationOperationId, Option<String>),
            Vec<&BlindSignatureInstall>,
        > = BTreeMap::new();
        for (blinded_message, install) in &plan.blind_signatures {
            if let Some(install) = install {
                signature_groups
                    .entry((
                        install.operation_id,
                        install.quote_id.as_ref().map(ToString::to_string),
                    ))
                    .or_default()
                    .push(install);
            }
            state_keys.push(match install {
                Some(install) => FederationMaterializedStateKeyRecord {
                    operation_index: install
                        .accepted_operation_index
                        .unwrap_or(base_operation_index),
                    operation_id: install.operation_id,
                    key: FederationMaterializedStateKey::BlindSignature {
                        blinded_message: *blinded_message,
                    },
                },
                None => base_key(FederationMaterializedStateKey::BlindSignature {
                    blinded_message: *blinded_message,
                }),
            });
        }
        for ((operation_id, quote), mut outputs) in signature_groups {
            outputs.sort_by_key(|install| install.output_index);
            let kind = operation_kinds
                .get(&operation_id)
                .copied()
                .ok_or_else(|| transfer_invalid("signed output names no completed operation"))?;
            let operation = Operation::new(
                operation_id.mint_operation_uuid(),
                kind,
                Amount::ZERO,
                Amount::ZERO,
                Amount::ZERO,
                None,
                None,
            );
            let mut records = Vec::with_capacity(outputs.len());
            for install in &outputs {
                let share_set = effective
                    .private_config()
                    .sign_transferred_blinded_messages(
                        effective.public_config(),
                        operation_id,
                        std::slice::from_ref(&install.output),
                    )?;
                let share = share_set
                    .shares
                    .into_iter()
                    .next()
                    .ok_or_else(|| transfer_invalid("signing an output produced no share"))?;
                records.push(FederationSignatureShareRecord {
                    operation_id,
                    accepted_operation_index: install.accepted_operation_index,
                    consensus_evidence: None,
                    output_index: install.output_index,
                    member_id: share.member_id,
                    keyset_id: share.keyset_id,
                    amount: share.amount,
                    blinded_secret: share.blinded_secret,
                    share: share.share,
                });
            }
            tx.add_federation_signature_shares(&records).await?;
            let quote = quote.map(|quote| quote_id(&quote)).transpose()?;
            let blinded_messages = outputs
                .iter()
                .map(|install| install.output.clone())
                .collect::<Vec<_>>();
            let signatures = records
                .into_iter()
                .map(crate::mint::federation_signature_share_to_blind_signature)
                .collect::<Vec<_>>();
            tx.add_blinded_messages(quote.as_ref(), &blinded_messages, &operation)
                .await?;
            tx.add_blind_signatures(
                &blinded_messages
                    .iter()
                    .map(|output| output.blinded_secret)
                    .collect::<Vec<_>>(),
                &signatures,
                quote,
            )
            .await?;
        }
        Ok(state_keys)
    }
}

/// The configured runtime config with the snapshot's keyset rotations applied
/// in consensus order, as `rebuild_effective_federation_config` builds it.
fn effective_config_with_rotations(
    runtime: &FederationRuntimeConfig,
    keysets: &[(u64, FederationOperationId, FederatedKeyset)],
) -> Result<FederationRuntimeConfig, Error> {
    let mut rotations = keysets.iter().collect::<Vec<_>>();
    rotations.sort_by_key(|(operation_index, operation_id, _)| (*operation_index, *operation_id));
    let mut effective = runtime.clone();
    for (_, _, keyset) in rotations {
        effective = effective.apply_keyset_rotation(&FederationKeysetRotationOperation {
            keyset: keyset.clone(),
        })?;
    }
    Ok(effective)
}
