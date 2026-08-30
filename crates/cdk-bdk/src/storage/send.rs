use std::str::FromStr;

use uuid::Uuid;

use super::{
    outpoint_to_key, BdkStorage, BroadcastRejectionRecord, FailedSendAttemptRecord,
    FinalizedSendIntentRecord, BDK_NAMESPACE, BROADCAST_REJECTION_NAMESPACE,
    FAILED_SEND_ATTEMPT_NAMESPACE, FINALIZED_INTENT_NAMESPACE,
    FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE, SEND_BATCH_NAMESPACE, SEND_INTENT_NAMESPACE,
    SEND_INTENT_QUOTE_ID_NAMESPACE, SEND_OUTPOINT_QUOTE_ID_BACKFILL_KEY,
    SEND_OUTPOINT_QUOTE_ID_NAMESPACE, STORAGE_MIGRATION_NAMESPACE,
};
use crate::error::Error;
use crate::send::batch_transaction::record::{SendBatchRecord, SendBatchState};
use crate::send::payment_intent::record::{SendIntentRecord, SendIntentState};

impl BdkStorage {
    // ── Send Intent storage ──────────────────────────────────────────

    /// Store a new send intent and quote-id index atomically.
    pub async fn create_send_intent_if_absent(
        &self,
        intent: &SendIntentRecord,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let active = tx
            .kv_read(
                BDK_NAMESPACE,
                SEND_INTENT_QUOTE_ID_NAMESPACE,
                &intent.quote_id,
            )
            .await
            .map_err(Error::from)?;

        if active.is_some() {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
        }

        let finalized = tx
            .kv_read(
                BDK_NAMESPACE,
                FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE,
                &intent.quote_id,
            )
            .await
            .map_err(Error::from)?;

        if finalized.is_some() {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
        }

        let serialized = serde_json::to_vec(intent)?;
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            &intent.intent_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        // Reserve the quote id atomically with the record write.
        let reserved = tx
            .kv_write_if_absent(
                BDK_NAMESPACE,
                SEND_INTENT_QUOTE_ID_NAMESPACE,
                &intent.quote_id,
                intent.intent_id.to_string().as_bytes(),
            )
            .await
            .map_err(Error::from)?;
        if !reserved {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
        }
        if let SendIntentState::AwaitingConfirmation { outpoint, .. } = &intent.state {
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(outpoint),
                intent.quote_id.as_bytes(),
            )
            .await
            .map_err(Error::from)?;
        }
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Store a new send intent, or re-queue an existing failed intent with
    /// the same quote id.
    ///
    /// Both paths are atomic against concurrent lifecycle changes:
    ///
    /// - The retry path rewrites the existing record with a
    ///   compare-and-set on the exact bytes read, so a stale retry cannot
    ///   rewind an intent another worker already advanced or finalized.
    /// - The create path reserves the quote id with `kv_write_if_absent`
    ///   and only then re-checks the finalized tombstone index, so a retry
    ///   racing [`Self::finalize_send_intent`] can never leave both a
    ///   tombstone and a live intent for the same quote.
    pub async fn create_or_retry_failed_send_intent(
        &self,
        intent: &SendIntentRecord,
    ) -> Result<SendIntentRecord, Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let active = tx
            .kv_read(
                BDK_NAMESPACE,
                SEND_INTENT_QUOTE_ID_NAMESPACE,
                &intent.quote_id,
            )
            .await
            .map_err(Error::from)?;

        let record = if let Some(intent_id_bytes) = active {
            let intent_id_str = std::str::from_utf8(&intent_id_bytes)
                .map_err(|e| Error::Wallet(format!("Invalid quote-id index entry: {}", e)))?;
            let intent_id = Uuid::from_str(intent_id_str)
                .map_err(|e| Error::Wallet(format!("Invalid indexed intent id: {}", e)))?;
            let key = intent_id.to_string();
            let intent_bytes = tx
                .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
                .await
                .map_err(Error::from)?
                .ok_or(Error::SendIntentNotFound(intent_id))?;
            let existing: SendIntentRecord = serde_json::from_slice(&intent_bytes)?;

            if !matches!(existing.state, SendIntentState::Failed { .. }) {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
            }

            let batch_keys = tx
                .kv_list(BDK_NAMESPACE, SEND_BATCH_NAMESPACE)
                .await
                .map_err(Error::from)?;
            for batch_key in batch_keys {
                let Some(batch_bytes) = tx
                    .kv_read(BDK_NAMESPACE, SEND_BATCH_NAMESPACE, &batch_key)
                    .await
                    .map_err(Error::from)?
                else {
                    continue;
                };
                let batch: SendBatchRecord =
                    serde_json::from_slice(&batch_bytes).map_err(|error| {
                        Error::Wallet(format!(
                            "Failed to deserialize record in namespace {} for key {}: {}",
                            SEND_BATCH_NAMESPACE, batch_key, error
                        ))
                    })?;
                if matches!(
                    batch.state,
                    SendBatchState::Broadcast { ref assignments, .. }
                        if assignments
                            .iter()
                            .any(|assignment| assignment.intent_id == intent_id)
                ) {
                    tx.rollback().await.map_err(Error::from)?;
                    return Err(Error::SendIntentStateConflict {
                        intent_id,
                        expected: "Failed without durable Broadcast evidence",
                    });
                }
            }

            let record = SendIntentRecord {
                intent_id,
                attempt_id: intent.attempt_id,
                quote_id: intent.quote_id.clone(),
                address: intent.address.clone(),
                amount_sat: intent.amount_sat,
                max_fee_amount_sat: intent.max_fee_amount_sat,
                tier: intent.tier,
                metadata: intent.metadata.clone(),
                state: intent.state.clone(),
            };
            let serialized = serde_json::to_vec(&record)?;
            let rewritten = tx
                .kv_write_if_equals(
                    BDK_NAMESPACE,
                    SEND_INTENT_NAMESPACE,
                    &key,
                    &intent_bytes,
                    &serialized,
                )
                .await
                .map_err(Error::from)?;
            if !rewritten {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
            }
            record
        } else {
            // Reserve the quote id atomically with the record write.
            let reserved = tx
                .kv_write_if_absent(
                    BDK_NAMESPACE,
                    SEND_INTENT_QUOTE_ID_NAMESPACE,
                    &intent.quote_id,
                    intent.intent_id.to_string().as_bytes(),
                )
                .await
                .map_err(Error::from)?;
            if !reserved {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
            }
            let finalized = tx
                .kv_read(
                    BDK_NAMESPACE,
                    FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE,
                    &intent.quote_id,
                )
                .await
                .map_err(Error::from)?;
            if finalized.is_some() {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
            }
            let serialized = serde_json::to_vec(intent)?;
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_INTENT_NAMESPACE,
                &intent.intent_id.to_string(),
                &serialized,
            )
            .await
            .map_err(Error::from)?;
            intent.clone()
        };

        if let SendIntentState::AwaitingConfirmation { outpoint, .. } = &record.state {
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(outpoint),
                record.quote_id.as_bytes(),
            )
            .await
            .map_err(Error::from)?;
        }
        tx.commit().await.map_err(Error::from)?;
        Ok(record)
    }

    /// Get a send intent by ID
    pub async fn get_send_intent(
        &self,
        intent_id: &Uuid,
    ) -> Result<Option<SendIntentRecord>, Error> {
        self.get_record::<SendIntentRecord>(&intent_id.to_string())
            .await
    }

    /// Update a send intent's state
    pub async fn update_send_intent(
        &self,
        intent_id: &Uuid,
        new_state: &SendIntentState,
    ) -> Result<(), Error> {
        let Some(mut intent) = self.get_send_intent(intent_id).await? else {
            return Err(Error::SendIntentNotFound(*intent_id));
        };
        let previous_outpoint = match &intent.state {
            SendIntentState::AwaitingConfirmation { outpoint, .. } => Some(outpoint.clone()),
            _ => None,
        };
        let new_outpoint = match new_state {
            SendIntentState::AwaitingConfirmation { outpoint, .. } => Some(outpoint.clone()),
            _ => None,
        };
        intent.state = new_state.clone();

        let serialized = serde_json::to_vec(&intent)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            &intent_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        if let Some(outpoint) =
            previous_outpoint.filter(|outpoint| Some(outpoint) != new_outpoint.as_ref())
        {
            tx.kv_remove(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(&outpoint),
            )
            .await
            .map_err(Error::from)?;
        }
        if let Some(outpoint) = new_outpoint {
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(&outpoint),
                intent.quote_id.as_bytes(),
            )
            .await
            .map_err(Error::from)?;
        }
        tx.commit().await.map_err(Error::from)
    }

    /// Atomically transition a send intent from `expected_state` to
    /// `next_state` using a compare-and-set against the durable record.
    ///
    /// Returns `Ok(true)` when the transition was applied. Returns
    /// `Ok(false)` — without writing anything — when the durable record no
    /// longer matches `expected_attempt_id` and `expected_state`, meaning
    /// another worker advanced, rewound, or retried it concurrently.
    ///
    /// The outpoint quote-id index is maintained in the same transaction,
    /// mirroring [`Self::update_send_intent`].
    pub async fn transition_send_intent(
        &self,
        intent_id: &Uuid,
        expected_attempt_id: &Uuid,
        expected_state: &SendIntentState,
        next_state: &SendIntentState,
    ) -> Result<bool, Error> {
        let key = intent_id.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let Some(expected_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendIntentNotFound(*intent_id));
        };

        let mut intent: SendIntentRecord = serde_json::from_slice(&expected_bytes)?;
        if &intent.attempt_id != expected_attempt_id || &intent.state != expected_state {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        let previous_outpoint = match &intent.state {
            SendIntentState::AwaitingConfirmation { outpoint, .. } => Some(outpoint.clone()),
            _ => None,
        };
        let new_outpoint = match next_state {
            SendIntentState::AwaitingConfirmation { outpoint, .. } => Some(outpoint.clone()),
            _ => None,
        };
        intent.state = next_state.clone();

        let replacement = serde_json::to_vec(&intent)?;
        let written = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_INTENT_NAMESPACE,
                &key,
                &expected_bytes,
                &replacement,
            )
            .await
            .map_err(Error::from)?;
        if !written {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }
        if let SendIntentState::Failed {
            reason, failed_at, ..
        } = next_state
        {
            let failed_attempt = FailedSendAttemptRecord {
                attempt_id: *expected_attempt_id,
                intent_id: *intent_id,
                quote_id: intent.quote_id.clone(),
                reason: reason.clone(),
                failed_at: *failed_at,
            };
            let serialized = serde_json::to_vec(&failed_attempt)?;
            tx.kv_write(
                BDK_NAMESPACE,
                FAILED_SEND_ATTEMPT_NAMESPACE,
                &expected_attempt_id.to_string(),
                &serialized,
            )
            .await
            .map_err(Error::from)?;
        }
        if let Some(outpoint) =
            previous_outpoint.filter(|outpoint| Some(outpoint) != new_outpoint.as_ref())
        {
            tx.kv_remove(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(&outpoint),
            )
            .await
            .map_err(Error::from)?;
        }
        if let Some(outpoint) = new_outpoint {
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(&outpoint),
                intent.quote_id.as_bytes(),
            )
            .await
            .map_err(Error::from)?;
        }
        tx.commit().await.map_err(Error::from)?;
        Ok(true)
    }

    /// Atomically claim a pending intent for a batch that is still `Signed`.
    ///
    /// The no-op batch compare-and-set locks the exact signed record before
    /// the intent is advanced. Cancellation therefore either happens first
    /// and rejects the claim, or waits for the claim and compensates it.
    pub(crate) async fn claim_send_intent_for_signed_batch(
        &self,
        intent_id: &Uuid,
        expected_attempt_id: &Uuid,
        created_at: u64,
        batch_id: &Uuid,
    ) -> Result<(), Error> {
        let batch_key = batch_id.to_string();
        let intent_key = intent_id.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let Some(batch_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_BATCH_NAMESPACE, &batch_key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendBatchStateConflict {
                batch_id: *batch_id,
                expected: "Signed",
            });
        };
        let batch: SendBatchRecord = serde_json::from_slice(&batch_bytes)?;
        let lists_exact_attempt_once = match &batch.state {
            SendBatchState::Signed { assignments, .. } if !expected_attempt_id.is_nil() => {
                let mut matching_intent = assignments
                    .iter()
                    .filter(|assignment| assignment.intent_id == *intent_id);
                matches!(
                    (matching_intent.next(), matching_intent.next()),
                    (Some(assignment), None)
                        if assignment.attempt_id == *expected_attempt_id
                )
            }
            _ => false,
        };
        if !lists_exact_attempt_once {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendBatchStateConflict {
                batch_id: *batch_id,
                expected: "Signed with one assignment for exact intent attempt",
            });
        }

        let batch_locked = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_BATCH_NAMESPACE,
                &batch_key,
                &batch_bytes,
                &batch_bytes,
            )
            .await
            .map_err(Error::from)?;
        if !batch_locked {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendBatchStateConflict {
                batch_id: *batch_id,
                expected: "Signed",
            });
        }

        let Some(intent_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &intent_key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendIntentNotFound(*intent_id));
        };
        let mut intent: SendIntentRecord = serde_json::from_slice(&intent_bytes)?;
        let expected_state = SendIntentState::Pending { created_at };
        if &intent.attempt_id != expected_attempt_id || intent.state != expected_state {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendIntentStateConflict {
                intent_id: *intent_id,
                expected: "Pending",
            });
        }

        intent.state = SendIntentState::Batched {
            batch_id: *batch_id,
            created_at,
        };
        let replacement = serde_json::to_vec(&intent)?;
        let intent_claimed = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_INTENT_NAMESPACE,
                &intent_key,
                &intent_bytes,
                &replacement,
            )
            .await
            .map_err(Error::from)?;
        if !intent_claimed {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::SendIntentStateConflict {
                intent_id: *intent_id,
                expected: "Pending",
            });
        }

        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Delete a send intent
    pub async fn delete_send_intent(&self, intent_id: &Uuid) -> Result<(), Error> {
        let Some(intent) = self.get_send_intent(intent_id).await? else {
            return Ok(());
        };

        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_remove(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &intent_id.to_string())
            .await
            .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            SEND_INTENT_QUOTE_ID_NAMESPACE,
            &intent.quote_id,
        )
        .await
        .map_err(Error::from)?;
        if let SendIntentState::AwaitingConfirmation { outpoint, .. } = intent.state {
            tx.kv_remove(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(&outpoint),
            )
            .await
            .map_err(Error::from)?;
        }
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Get all send intents
    pub async fn get_all_send_intents(&self) -> Result<Vec<SendIntentRecord>, Error> {
        self.list_records::<SendIntentRecord>().await
    }

    /// Get all pending send intents (filtering by state)
    pub async fn get_pending_send_intents(&self) -> Result<Vec<SendIntentRecord>, Error> {
        let all = self.get_all_send_intents().await?;
        Ok(all
            .into_iter()
            .filter(|i| matches!(i.state, SendIntentState::Pending { .. }))
            .collect())
    }

    /// Atomically assign fresh attempt IDs to legacy pending send intents.
    ///
    /// Records written before attempt generations were introduced deserialize
    /// with a nil attempt ID. All such pending records are rewritten in one
    /// transaction. A concurrent change causes the whole normalization to roll
    /// back rather than overwriting the newer record.
    pub async fn normalize_legacy_pending_attempt_ids(&self) -> Result<usize, Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let keys = tx
            .kv_list(BDK_NAMESPACE, SEND_INTENT_NAMESPACE)
            .await
            .map_err(Error::from)?;
        let mut normalized = 0;

        for key in keys {
            let Some(bytes) = tx
                .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
                .await
                .map_err(Error::from)?
            else {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::Wallet(format!(
                    "Send intent disappeared while normalizing legacy attempts for key {}",
                    key
                )));
            };
            let mut intent: SendIntentRecord = serde_json::from_slice(&bytes).map_err(|error| {
                Error::Wallet(format!(
                    "Failed to deserialize record in namespace {} for key {}: {}",
                    SEND_INTENT_NAMESPACE, key, error
                ))
            })?;
            if !intent.attempt_id.is_nil()
                || !matches!(intent.state, SendIntentState::Pending { .. })
            {
                continue;
            }

            intent.attempt_id = Uuid::new_v4();
            let replacement = serde_json::to_vec(&intent)?;
            let rewritten = tx
                .kv_write_if_equals(
                    BDK_NAMESPACE,
                    SEND_INTENT_NAMESPACE,
                    &key,
                    &bytes,
                    &replacement,
                )
                .await
                .map_err(Error::from)?;
            if !rewritten {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::Wallet(format!(
                    "Send intent changed while normalizing legacy attempt for key {}",
                    key
                )));
            }
            normalized += 1;
        }

        tx.commit().await.map_err(Error::from)?;
        Ok(normalized)
    }

    /// Write-once binding of pre-attempt-generation Broadcast batches to
    /// their member intents.
    ///
    /// Broadcast records written before attempt generations were introduced
    /// deserialize with nil assignment attempt IDs, and the rebroadcast
    /// eligibility gate fences such batches permanently. When every member
    /// intent is still bound to this exact batch (state, batch ID, computed
    /// txid, outpoint, and fee all match), each nil assignment is bound to
    /// the member's current attempt, assigning a fresh attempt ID to legacy
    /// members first. Batches with missing, replaced, or mismatched members
    /// are left untouched and stay fenced for operator review.
    ///
    /// `txid` must be the transaction ID computed from the batch's persisted
    /// transaction bytes, not the record's informational `txid` field.
    ///
    /// All rewrites for one batch commit atomically. A concurrent
    /// modification rolls the transaction back and returns `Ok(false)`; the
    /// next processor cycle retries.
    pub async fn normalize_legacy_broadcast_attempt_ids(
        &self,
        batch_id: &Uuid,
        txid: &str,
    ) -> Result<bool, Error> {
        let batch_key = batch_id.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let Some(batch_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_BATCH_NAMESPACE, &batch_key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        };
        let mut batch: SendBatchRecord = serde_json::from_slice(&batch_bytes).map_err(|error| {
            Error::Wallet(format!(
                "Failed to deserialize record in namespace {} for key {}: {}",
                SEND_BATCH_NAMESPACE, batch_key, error
            ))
        })?;
        let SendBatchState::Broadcast {
            txid: stored_txid,
            tx_bytes,
            assignments,
            fee_sat,
        } = batch.state
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        };
        if !assignments.iter().any(|a| a.attempt_id.is_nil()) {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        // Validate every member's binding to this exact batch and plan the
        // rewrites before writing anything: a batch is bound only as a
        // whole.
        let mut member_rewrites: Vec<(String, Vec<u8>, Vec<u8>)> = Vec::new();
        let mut bound_assignments = assignments.clone();
        for (index, assignment) in assignments.iter().enumerate() {
            let intent_key = assignment.intent_id.to_string();
            let Some(intent_bytes) = tx
                .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &intent_key)
                .await
                .map_err(Error::from)?
            else {
                // Missing member: leave the batch fenced.
                tx.rollback().await.map_err(Error::from)?;
                return Ok(false);
            };
            let mut intent: SendIntentRecord =
                serde_json::from_slice(&intent_bytes).map_err(|error| {
                    Error::Wallet(format!(
                        "Failed to deserialize record in namespace {} for key {}: {}",
                        SEND_INTENT_NAMESPACE, intent_key, error
                    ))
                })?;
            let bound_to_batch = match &intent.state {
                SendIntentState::AwaitingConfirmation {
                    batch_id: member_batch_id,
                    txid: member_txid,
                    outpoint,
                    fee_contribution_sat,
                    ..
                } => {
                    *member_batch_id == *batch_id
                        && member_txid == txid
                        && *outpoint == format!("{txid}:{}", assignment.vout)
                        && *fee_contribution_sat == assignment.fee_contribution_sat
                }
                // A Batched member has not been advanced to its final
                // broadcast fields yet; the eligibility gate repairs it.
                SendIntentState::Batched {
                    batch_id: member_batch_id,
                    ..
                } => *member_batch_id == *batch_id,
                _ => false,
            };
            if !bound_to_batch {
                tx.rollback().await.map_err(Error::from)?;
                return Ok(false);
            }

            let target_attempt = if intent.attempt_id.is_nil() {
                let fresh = Uuid::new_v4();
                intent.attempt_id = fresh;
                let replacement = serde_json::to_vec(&intent)?;
                member_rewrites.push((intent_key, intent_bytes, replacement));
                fresh
            } else {
                intent.attempt_id
            };
            bound_assignments[index].attempt_id = target_attempt;
        }

        for (intent_key, intent_bytes, replacement) in member_rewrites {
            let rewritten = tx
                .kv_write_if_equals(
                    BDK_NAMESPACE,
                    SEND_INTENT_NAMESPACE,
                    &intent_key,
                    &intent_bytes,
                    &replacement,
                )
                .await
                .map_err(Error::from)?;
            if !rewritten {
                tx.rollback().await.map_err(Error::from)?;
                return Ok(false);
            }
        }

        batch.state = SendBatchState::Broadcast {
            txid: stored_txid,
            tx_bytes,
            assignments: bound_assignments,
            fee_sat,
        };
        let batch_replacement = serde_json::to_vec(&batch)?;
        let rewritten = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_BATCH_NAMESPACE,
                &batch_key,
                &batch_bytes,
                &batch_replacement,
            )
            .await
            .map_err(Error::from)?;
        if !rewritten {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        tx.commit().await.map_err(Error::from)?;
        Ok(true)
    }

    /// Record a deterministic broadcast rejection for a batch, returning the
    /// consecutive rejection count.
    ///
    /// Only rejections the backend will never reconsider may be recorded
    /// here; transient or ambiguous outcomes must not increment the counter.
    /// A concurrent modification rolls the transaction back and returns an
    /// error; the caller logs it and the next cycle retries.
    pub async fn record_broadcast_rejection(
        &self,
        batch_id: &Uuid,
        txid: &str,
        error: &str,
    ) -> Result<u32, Error> {
        let key = batch_id.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let existing = tx
            .kv_read(BDK_NAMESPACE, BROADCAST_REJECTION_NAMESPACE, &key)
            .await
            .map_err(Error::from)?;

        let mut record = match &existing {
            Some(bytes) => serde_json::from_slice::<BroadcastRejectionRecord>(bytes)?,
            None => BroadcastRejectionRecord {
                batch_id: *batch_id,
                txid: txid.to_string(),
                consecutive_rejections: 0,
                last_error: String::new(),
                last_rejected_at: 0,
            },
        };
        record.consecutive_rejections = record.consecutive_rejections.saturating_add(1);
        record.txid = txid.to_string();
        record.last_error = error.to_string();
        record.last_rejected_at = crate::util::unix_now();
        let replacement = serde_json::to_vec(&record)?;

        let written = match &existing {
            Some(bytes) => tx
                .kv_write_if_equals(
                    BDK_NAMESPACE,
                    BROADCAST_REJECTION_NAMESPACE,
                    &key,
                    bytes,
                    &replacement,
                )
                .await
                .map_err(Error::from)?,
            None => tx
                .kv_write_if_absent(
                    BDK_NAMESPACE,
                    BROADCAST_REJECTION_NAMESPACE,
                    &key,
                    &replacement,
                )
                .await
                .map_err(Error::from)?,
        };
        if !written {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::Wallet(format!(
                "Broadcast rejection record changed while updating batch {key}"
            )));
        }

        tx.commit().await.map_err(Error::from)?;
        Ok(record.consecutive_rejections)
    }

    /// Clear a batch's rejection record after an accepted or already-known
    /// broadcast outcome, or when the batch record is deleted. Idempotent.
    pub async fn clear_broadcast_rejection(&self, batch_id: &Uuid) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            BROADCAST_REJECTION_NAMESPACE,
            &batch_id.to_string(),
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Read a batch's broadcast rejection record, if any.
    pub async fn get_broadcast_rejection(
        &self,
        batch_id: &Uuid,
    ) -> Result<Option<BroadcastRejectionRecord>, Error> {
        self.get_record(&batch_id.to_string()).await
    }

    /// Store a failed pre-sign send attempt tombstone.
    pub async fn add_failed_send_attempt(
        &self,
        record: &FailedSendAttemptRecord,
    ) -> Result<(), Error> {
        // A tombstone belongs permanently to the attempt supplied by the
        // caller. In particular, never retarget a delayed write to whichever
        // retry currently occupies the active intent record.
        let key = record.attempt_id.to_string();
        let bytes = serde_json::to_vec(record)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let inserted = tx
            .kv_write_if_absent(BDK_NAMESPACE, FAILED_SEND_ATTEMPT_NAMESPACE, &key, &bytes)
            .await
            .map_err(Error::from)?;
        if inserted {
            tx.commit().await.map_err(Error::from)?;
            return Ok(());
        }

        let existing = tx
            .kv_read(BDK_NAMESPACE, FAILED_SEND_ATTEMPT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?;
        tx.rollback().await.map_err(Error::from)?;
        if existing.as_deref() == Some(bytes.as_slice()) {
            Ok(())
        } else {
            Err(Error::Wallet(format!(
                "Failed send attempt {key} already exists with different contents"
            )))
        }
    }

    /// List failed pre-sign send attempts for a quote id.
    pub async fn get_failed_send_attempts_by_quote_id(
        &self,
        quote_id: &str,
    ) -> Result<Vec<FailedSendAttemptRecord>, Error> {
        let all = self.list_records::<FailedSendAttemptRecord>().await?;
        Ok(all
            .into_iter()
            .filter(|record| record.quote_id == quote_id)
            .collect())
    }

    // ── Send Batch storage ───────────────────────────────────────────

    /// Store a new send batch
    pub async fn store_send_batch(&self, batch: &SendBatchRecord) -> Result<(), Error> {
        self.put_record(batch).await
    }

    /// Get a send batch by ID
    pub async fn get_send_batch(&self, batch_id: &Uuid) -> Result<Option<SendBatchRecord>, Error> {
        self.get_record::<SendBatchRecord>(&batch_id.to_string())
            .await
    }

    /// Update a send batch's state
    pub async fn update_send_batch(
        &self,
        batch_id: &Uuid,
        new_state: &SendBatchState,
    ) -> Result<(), Error> {
        let key = batch_id.to_string();
        if self.get_send_batch(batch_id).await?.is_none() {
            return Err(Error::SendBatchNotFound(*batch_id));
        }

        self.update_record_state::<SendBatchRecord, SendBatchState>(&key, new_state)
            .await
    }

    /// Atomically transition a send batch from `expected_state` to
    /// `next_state` using a compare-and-set against the durable record.
    ///
    /// Returns `Ok(true)` when the transition was applied and `Ok(false)`
    /// when another worker changed or removed the batch first.
    pub async fn transition_send_batch(
        &self,
        batch_id: &Uuid,
        expected_state: &SendBatchState,
        next_state: &SendBatchState,
    ) -> Result<bool, Error> {
        let key = batch_id.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let Some(expected_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_BATCH_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        };

        let mut batch: SendBatchRecord = serde_json::from_slice(&expected_bytes)?;
        if &batch.state != expected_state {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }
        batch.state = next_state.clone();

        let replacement = serde_json::to_vec(&batch)?;
        let written = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_BATCH_NAMESPACE,
                &key,
                &expected_bytes,
                &replacement,
            )
            .await
            .map_err(Error::from)?;
        if !written {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        tx.commit().await.map_err(Error::from)?;
        Ok(true)
    }

    /// Delete a send batch only while it still matches `expected_state`.
    ///
    /// The no-op conditional update locks the matching record for the rest of
    /// the transaction before deletion, so a concurrent promotion to
    /// `Broadcast` and compensation cannot both succeed.
    pub async fn delete_send_batch_if_state(
        &self,
        batch_id: &Uuid,
        expected_state: &SendBatchState,
    ) -> Result<bool, Error> {
        let key = batch_id.to_string();
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let Some(expected_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_BATCH_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        };

        let batch: SendBatchRecord = serde_json::from_slice(&expected_bytes)?;
        if &batch.state != expected_state {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        let locked = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_BATCH_NAMESPACE,
                &key,
                &expected_bytes,
                &expected_bytes,
            )
            .await
            .map_err(Error::from)?;
        if !locked {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        tx.kv_remove(BDK_NAMESPACE, SEND_BATCH_NAMESPACE, &key)
            .await
            .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(true)
    }

    /// Delete a send batch
    pub async fn delete_send_batch(&self, batch_id: &Uuid) -> Result<(), Error> {
        self.delete_record::<SendBatchRecord>(&batch_id.to_string())
            .await
    }

    /// Get all send batches
    pub async fn get_all_send_batches(&self) -> Result<Vec<SendBatchRecord>, Error> {
        self.list_records::<SendBatchRecord>().await
    }

    // ── Finalized Intent storage (tombstones) ────────────────────────

    /// Look up a finalized intent tombstone by intent ID.
    pub async fn get_finalized_intent(
        &self,
        intent_id: &Uuid,
    ) -> Result<Option<FinalizedSendIntentRecord>, Error> {
        self.get_record::<FinalizedSendIntentRecord>(&intent_id.to_string())
            .await
    }

    /// Get all finalized send intent tombstones.
    pub async fn get_all_finalized_send_intents(
        &self,
    ) -> Result<Vec<FinalizedSendIntentRecord>, Error> {
        self.list_records::<FinalizedSendIntentRecord>().await
    }

    /// Look up a quote ID by the transaction output assigned to a send intent.
    pub async fn get_quote_id_by_send_outpoint(
        &self,
        outpoint: &str,
    ) -> Result<Option<String>, Error> {
        let quote_id_bytes = self
            .kv_store
            .kv_read(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(outpoint),
            )
            .await
            .map_err(Error::from)?;

        match quote_id_bytes {
            Some(quote_id_bytes) => String::from_utf8(quote_id_bytes)
                .map(Some)
                .map_err(|e| Error::Wallet(format!("Invalid quote-id index entry: {}", e))),
            None => Ok(None),
        }
    }

    /// Populate the send output quote index for records written by older versions.
    pub(crate) async fn ensure_send_outpoint_quote_id_index(&self) -> Result<(), Error> {
        if self
            .kv_store
            .kv_read(
                BDK_NAMESPACE,
                STORAGE_MIGRATION_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_BACKFILL_KEY,
            )
            .await
            .map_err(Error::from)?
            .is_some()
        {
            return Ok(());
        }

        let (send_intents, finalized_send_intents) = tokio::try_join!(
            self.get_all_send_intents(),
            self.get_all_finalized_send_intents(),
        )?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        for intent in send_intents {
            if let SendIntentState::AwaitingConfirmation { outpoint, .. } = intent.state {
                tx.kv_write(
                    BDK_NAMESPACE,
                    SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                    &outpoint_to_key(&outpoint),
                    intent.quote_id.as_bytes(),
                )
                .await
                .map_err(Error::from)?;
            }
        }
        for intent in finalized_send_intents {
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
                &outpoint_to_key(&intent.outpoint),
                intent.quote_id.as_bytes(),
            )
            .await
            .map_err(Error::from)?;
        }
        tx.kv_write(
            BDK_NAMESPACE,
            STORAGE_MIGRATION_NAMESPACE,
            SEND_OUTPOINT_QUOTE_ID_BACKFILL_KEY,
            b"complete",
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)
    }

    /// Look up a finalized intent tombstone by quote ID.
    pub async fn get_finalized_intent_by_quote_id(
        &self,
        quote_id: &str,
    ) -> Result<Option<FinalizedSendIntentRecord>, Error> {
        let Some(intent_id_bytes) = self
            .kv_store
            .kv_read(
                BDK_NAMESPACE,
                FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE,
                quote_id,
            )
            .await
            .map_err(Error::from)?
        else {
            return Ok(None);
        };

        let intent_id_str = std::str::from_utf8(&intent_id_bytes)
            .map_err(|e| Error::Wallet(format!("Invalid intent-id index entry: {}", e)))?;
        let intent_id = Uuid::from_str(intent_id_str)
            .map_err(|e| Error::Wallet(format!("Invalid indexed intent id: {}", e)))?;

        self.get_record::<FinalizedSendIntentRecord>(&intent_id.to_string())
            .await
    }

    /// Look up a send intent by quote ID.
    ///
    /// Scans all active intents and returns the first match.
    pub async fn get_send_intent_by_quote_id(
        &self,
        quote_id: &str,
    ) -> Result<Option<SendIntentRecord>, Error> {
        let Some(intent_id_bytes) = self
            .kv_store
            .kv_read(BDK_NAMESPACE, SEND_INTENT_QUOTE_ID_NAMESPACE, quote_id)
            .await
            .map_err(Error::from)?
        else {
            return Ok(None);
        };

        let intent_id = std::str::from_utf8(&intent_id_bytes)
            .map_err(|e| Error::Wallet(format!("Invalid quote-id index entry: {}", e)))?;
        let intent_id = Uuid::from_str(intent_id)
            .map_err(|e| Error::Wallet(format!("Invalid indexed intent id: {}", e)))?;

        self.get_send_intent(&intent_id).await
    }

    /// Atomically finalize an active send intent and create a tombstone.
    pub async fn finalize_send_intent(
        &self,
        expected: &SendIntentRecord,
        record: &FinalizedSendIntentRecord,
    ) -> Result<bool, Error> {
        if record.intent_id != expected.intent_id || record.quote_id != expected.quote_id {
            return Err(Error::SendIntentStateConflict {
                intent_id: expected.intent_id,
                expected: "matching finalized intent identity",
            });
        }

        let key = expected.intent_id.to_string();
        let serialized = serde_json::to_vec(record)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let Some(active_bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        };
        let intent: SendIntentRecord = serde_json::from_slice(&active_bytes)?;
        // Compare records semantically, not by raw bytes: the caller's
        // expected record round-tripped through storage and a fresh
        // serialization, so identity must not depend on the exact byte
        // representation.
        if intent != *expected {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        // Take ownership of this exact active record before creating the
        // tombstone. Writing back the bytes just read is intentional: the
        // conditional update locks the row in SQL stores and gives all
        // backends one atomic winner, while preserving the record until the
        // transaction removes it. The condition is the stored bytes, not a
        // re-serialization, so the lock does not depend on how a fresh
        // serialization would represent the record.
        let won = tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                SEND_INTENT_NAMESPACE,
                &key,
                &active_bytes,
                &active_bytes,
            )
            .await
            .map_err(Error::from)?;
        if !won {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }
        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_INTENT_NAMESPACE,
            &record.intent_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE,
            &intent.quote_id,
            record.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_OUTPOINT_QUOTE_ID_NAMESPACE,
            &outpoint_to_key(&record.outpoint),
            record.quote_id.as_bytes(),
        )
        .await
        .map_err(Error::from)?;
        tx.kv_remove(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            SEND_INTENT_QUOTE_ID_NAMESPACE,
            &intent.quote_id,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::send::batch_transaction::record::BatchOutputAssignment;
    use crate::send::payment_intent::SendIntent;
    use crate::testutil::{store_test_signed_batch, GatedKvStore, PausePoint, ReadPath};
    use crate::types::{PaymentMetadata, PaymentTier};

    const ADDR: &str = "bcrt1qw508d6qejxtdg4y5r3zarvary0c5xw7kygt080";

    #[tokio::test]
    async fn failed_attempt_insert_never_retargets_or_overwrites() {
        let storage = BdkStorage::new(Arc::new(GatedKvStore::default()));
        let intent_id = Uuid::new_v4();
        let first = FailedSendAttemptRecord {
            attempt_id: Uuid::new_v4(),
            intent_id,
            quote_id: "failed-attempt-history".to_string(),
            reason: "first failure".to_string(),
            failed_at: 1,
        };
        let second = FailedSendAttemptRecord {
            attempt_id: Uuid::new_v4(),
            intent_id,
            quote_id: first.quote_id.clone(),
            reason: "second failure".to_string(),
            failed_at: 2,
        };
        storage
            .add_failed_send_attempt(&first)
            .await
            .expect("store first attempt");
        storage
            .add_failed_send_attempt(&second)
            .await
            .expect("store second attempt");
        storage
            .add_failed_send_attempt(&first)
            .await
            .expect("identical delayed write is idempotent");

        let conflicting = FailedSendAttemptRecord {
            reason: "retargeted contents".to_string(),
            ..second.clone()
        };
        assert!(storage.add_failed_send_attempt(&conflicting).await.is_err());

        let records = storage
            .get_failed_send_attempts_by_quote_id(&first.quote_id)
            .await
            .expect("read attempt history");
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|record| {
            record.attempt_id == first.attempt_id && record.reason == first.reason
        }));
        assert!(records.iter().any(|record| {
            record.attempt_id == second.attempt_id && record.reason == second.reason
        }));
    }

    /// A retry that is paused before reading the active quote-id index
    /// while `finalize_send_intent` commits must be rejected: the quote
    /// ends with exactly one finalized tombstone and no live intent.
    #[tokio::test]
    async fn concurrent_retry_cannot_reactivate_finalized_quote() {
        let kv = GatedKvStore::default();
        let storage = BdkStorage::new(Arc::new(kv.clone()));
        let quote_id = "quote-finalize-race".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            ADDR.to_string(),
            20_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[pending.intent_id]).await;
        let awaiting = pending
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("assign")
            .mark_broadcast(
                &storage,
                "txid-race".to_string(),
                "txid-race:0".to_string(),
                250,
            )
            .await
            .expect("mark broadcast");
        let intent_id = awaiting.intent_id;
        let tombstone = FinalizedSendIntentRecord {
            intent_id,
            quote_id: quote_id.clone(),
            total_spent_sat: 20_250,
            outpoint: "txid-race:0".to_string(),
            finalized_at: 1_700_000_000,
        };

        let gate = kv.gate_read(
            ReadPath::Transaction,
            PausePoint::BeforeRead,
            BDK_NAMESPACE,
            SEND_INTENT_QUOTE_ID_NAMESPACE,
            1,
        );

        let retry_storage = storage.clone();
        let retry_quote = quote_id.clone();
        let retry = tokio::spawn(async move {
            SendIntent::new(
                &retry_storage,
                retry_quote,
                ADDR.to_string(),
                20_000,
                1_000,
                PaymentTier::Immediate,
                PaymentMetadata::default(),
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
            .await
            .expect("retry reached the gated read");
        storage
            .finalize_send_intent(
                &storage
                    .get_send_intent(&intent_id)
                    .await
                    .expect("read intent")
                    .expect("intent exists"),
                &tombstone,
            )
            .await
            .expect("finalize");
        gate.release();

        let result = retry.await.expect("join retry task");
        assert!(
            matches!(result, Err(Error::DuplicateQuoteId(ref id)) if *id == quote_id),
            "retry must be rejected with DuplicateQuoteId, got {:?}",
            result
        );

        assert!(storage
            .get_finalized_intent(&intent_id)
            .await
            .expect("get tombstone")
            .is_some());
        assert!(storage
            .get_all_send_intents()
            .await
            .expect("list send intents")
            .is_empty());
        assert!(storage
            .get_send_intent_by_quote_id(&quote_id)
            .await
            .expect("lookup by quote id")
            .is_none());
    }

    /// A retry that snapshots a `Failed` record and then loses the race
    /// against another worker requeuing and batching the intent must not
    /// rewind the record back to `Pending`.
    #[tokio::test]
    async fn stale_failed_retry_cannot_rewind_batched_intent() {
        let kv = GatedKvStore::default();
        let storage = BdkStorage::new(Arc::new(kv.clone()));
        let quote_id = "quote-stale-retry".to_string();

        let pending = SendIntent::new(
            &storage,
            quote_id.clone(),
            ADDR.to_string(),
            20_000,
            1_000,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("new");
        let intent_id = pending.intent_id;
        pending
            .fail(&storage, "fee too high".to_string())
            .await
            .expect("fail");

        let gate = kv.gate_read(
            ReadPath::Transaction,
            PausePoint::AfterRead,
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            1,
        );
        let retry_a_storage = storage.clone();
        let retry_a_quote = quote_id.clone();
        let retry_a = tokio::spawn(async move {
            SendIntent::new(
                &retry_a_storage,
                retry_a_quote,
                ADDR.to_string(),
                20_000,
                1_500,
                PaymentTier::Immediate,
                PaymentMetadata::default(),
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
            .await
            .expect("retry A reached the gated read");

        let retried_b = SendIntent::new(
            &storage,
            quote_id.clone(),
            ADDR.to_string(),
            20_000,
            1_500,
            PaymentTier::Immediate,
            PaymentMetadata::default(),
        )
        .await
        .expect("retry B requeues the failed intent");
        assert_eq!(retried_b.intent_id, intent_id);
        let batch_id = Uuid::new_v4();
        store_test_signed_batch(&storage, batch_id, &[intent_id]).await;
        retried_b
            .assign_to_batch(&storage, batch_id)
            .await
            .expect("batch intent for retry B");

        gate.release();
        let result_a = retry_a.await.expect("join retry A");
        assert!(
            matches!(result_a, Err(Error::DuplicateQuoteId(ref id)) if *id == quote_id),
            "stale retry must be rejected with DuplicateQuoteId, got {:?}",
            result_a
        );

        let persisted = storage
            .get_send_intent(&intent_id)
            .await
            .expect("get intent")
            .expect("intent present");
        assert!(
            matches!(persisted.state, SendIntentState::Batched { batch_id: b, .. } if b == batch_id),
            "stale retry must not rewind the batched intent, got {:?}",
            persisted.state
        );
    }

    /// Compensation that snapshots a `Signed` batch must not delete it after
    /// another worker promotes the durable record to `Broadcast`.
    #[tokio::test]
    async fn stale_signed_batch_delete_cannot_remove_broadcast_batch() {
        let kv = GatedKvStore::default();
        let storage = BdkStorage::new(Arc::new(kv.clone()));
        let batch_id = Uuid::new_v4();
        let signed_state = SendBatchState::Signed {
            tx_bytes: vec![1, 2, 3],
            assignments: Vec::new(),
            fee_sat: 500,
        };
        let broadcast_state = SendBatchState::Broadcast {
            txid: "broadcast-won".to_string(),
            tx_bytes: vec![1, 2, 3],
            assignments: Vec::new(),
            fee_sat: 500,
        };
        storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: signed_state.clone(),
            })
            .await
            .expect("store signed batch");

        let gate = kv.gate_read(
            ReadPath::Transaction,
            PausePoint::AfterRead,
            BDK_NAMESPACE,
            SEND_BATCH_NAMESPACE,
            1,
        );
        let stale_storage = storage.clone();
        let stale_signed_state = signed_state.clone();
        let stale_delete = tokio::spawn(async move {
            stale_storage
                .delete_send_batch_if_state(&batch_id, &stale_signed_state)
                .await
        });

        tokio::time::timeout(Duration::from_secs(5), gate.wait_entered())
            .await
            .expect("conditional delete reached the gated read");
        assert!(storage
            .transition_send_batch(&batch_id, &signed_state, &broadcast_state)
            .await
            .expect("promote batch"));
        gate.release();

        assert!(!stale_delete
            .await
            .expect("join stale delete")
            .expect("stale delete should not error"));
        let persisted = storage
            .get_send_batch(&batch_id)
            .await
            .expect("get batch")
            .expect("broadcast batch remains");
        assert_eq!(persisted.state, broadcast_state);
    }

    /// Regression test: finalizing an intent whose expected record
    /// round-tripped through storage must not be mistaken for a concurrent
    /// modification. The identity comparison must be semantic, never based
    /// on the serialized byte representation of the record.
    #[tokio::test]
    async fn finalize_send_intent_with_multi_entry_metadata() {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory database");
        let storage = BdkStorage::new(Arc::new(db));

        for _ in 0..25 {
            let intent_id = Uuid::new_v4();
            let txid = format!("txid-{intent_id}");
            let intent = SendIntentRecord {
                intent_id,
                attempt_id: Uuid::new_v4(),
                quote_id: format!("quote-{intent_id}"),
                address: ADDR.to_string(),
                amount_sat: 20_000,
                max_fee_amount_sat: 1_000,
                tier: PaymentTier::Immediate,
                metadata: PaymentMetadata::from_optional_json(Some(
                    r#"{"key1": "value1", "key2": "value2", "key3": "value3"}"#,
                )),
                state: SendIntentState::AwaitingConfirmation {
                    batch_id: Uuid::new_v4(),
                    txid: txid.clone(),
                    outpoint: format!("{txid}:0"),
                    fee_contribution_sat: 250,
                    created_at: 1_700_000_000,
                },
            };
            storage
                .create_send_intent_if_absent(&intent)
                .await
                .expect("store intent");

            // Mirror the confirmation flow: the record handed to
            // `finalize_send_intent` is deserialized from storage, not the
            // originally inserted value.
            let active = storage
                .get_send_intent(&intent_id)
                .await
                .expect("read intent")
                .expect("intent exists");
            let tombstone = FinalizedSendIntentRecord {
                intent_id,
                quote_id: active.quote_id.clone(),
                total_spent_sat: 20_250,
                outpoint: format!("{txid}:0"),
                finalized_at: 1_700_000_001,
            };
            assert!(
                storage
                    .finalize_send_intent(&active, &tombstone)
                    .await
                    .expect("finalize"),
                "finalization must not be rejected for multi-entry metadata"
            );
            assert!(storage
                .get_finalized_intent(&intent_id)
                .await
                .expect("get tombstone")
                .is_some());
        }
    }

    /// Pre-attempt-generation Broadcast batches are bound to their member
    /// intents' current attempts in one atomic write, so the rebroadcast
    /// eligibility gate can evaluate them. Covers both an advanced
    /// AwaitingConfirmation member and a crash-leftover Batched member.
    #[tokio::test]
    async fn normalize_legacy_broadcast_attempt_ids_binds_matching_members() {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory database");
        let storage = BdkStorage::new(Arc::new(db));
        let batch_id = Uuid::new_v4();
        let txid = "aa".repeat(32);
        let awaiting_id = Uuid::new_v4();
        let batched_id = Uuid::new_v4();

        let awaiting = SendIntentRecord {
            intent_id: awaiting_id,
            attempt_id: Uuid::nil(),
            quote_id: format!("quote-{awaiting_id}"),
            address: ADDR.to_string(),
            amount_sat: 20_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id,
                txid: txid.clone(),
                outpoint: format!("{txid}:0"),
                fee_contribution_sat: 250,
                created_at: 1_700_000_000,
            },
        };
        let batched = SendIntentRecord {
            intent_id: batched_id,
            attempt_id: Uuid::nil(),
            quote_id: format!("quote-{batched_id}"),
            address: ADDR.to_string(),
            amount_sat: 10_000,
            max_fee_amount_sat: 500,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::Batched {
                batch_id,
                created_at: 1_700_000_000,
            },
        };
        for intent in [&awaiting, &batched] {
            storage
                .create_send_intent_if_absent(intent)
                .await
                .expect("store member");
        }
        storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: txid.clone(),
                    tx_bytes: vec![0x01],
                    assignments: vec![
                        BatchOutputAssignment {
                            intent_id: awaiting_id,
                            attempt_id: Uuid::nil(),
                            vout: 0,
                            fee_contribution_sat: 250,
                        },
                        BatchOutputAssignment {
                            intent_id: batched_id,
                            attempt_id: Uuid::nil(),
                            vout: 1,
                            fee_contribution_sat: 250,
                        },
                    ],
                    fee_sat: 500,
                },
            })
            .await
            .expect("store legacy broadcast batch");

        assert!(storage
            .normalize_legacy_broadcast_attempt_ids(&batch_id, &txid)
            .await
            .expect("normalize"));

        let awaiting_after = storage
            .get_send_intent(&awaiting_id)
            .await
            .expect("read awaiting member")
            .expect("awaiting member exists");
        let batched_after = storage
            .get_send_intent(&batched_id)
            .await
            .expect("read batched member")
            .expect("batched member exists");
        assert!(!awaiting_after.attempt_id.is_nil());
        assert!(!batched_after.attempt_id.is_nil());
        let batch_after = storage
            .get_send_batch(&batch_id)
            .await
            .expect("read batch")
            .expect("batch exists");
        match batch_after.state {
            SendBatchState::Broadcast { assignments, .. } => {
                assert_eq!(assignments[0].attempt_id, awaiting_after.attempt_id);
                assert_eq!(assignments[1].attempt_id, batched_after.attempt_id);
            }
            other => panic!("expected Broadcast, got {:?}", other),
        }

        // Idempotent: nothing left to normalize.
        assert!(!storage
            .normalize_legacy_broadcast_attempt_ids(&batch_id, &txid)
            .await
            .expect("normalize again"));
    }

    /// A batch whose member no longer matches its assignment must stay
    /// untouched and fenced for operator review.
    #[tokio::test]
    async fn normalize_legacy_broadcast_attempt_ids_leaves_mismatched_batch_fenced() {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory database");
        let storage = BdkStorage::new(Arc::new(db));
        let batch_id = Uuid::new_v4();
        let txid = "bb".repeat(32);
        let intent_id = Uuid::new_v4();

        // The member's outpoint does not match the assignment's vout: the
        // batch cannot be proven to belong to this intent.
        let member = SendIntentRecord {
            intent_id,
            attempt_id: Uuid::nil(),
            quote_id: format!("quote-{intent_id}"),
            address: ADDR.to_string(),
            amount_sat: 20_000,
            max_fee_amount_sat: 1_000,
            tier: PaymentTier::Immediate,
            metadata: PaymentMetadata::default(),
            state: SendIntentState::AwaitingConfirmation {
                batch_id,
                txid: txid.clone(),
                outpoint: format!("{txid}:1"),
                fee_contribution_sat: 250,
                created_at: 1_700_000_000,
            },
        };
        storage
            .create_send_intent_if_absent(&member)
            .await
            .expect("store member");
        storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: SendBatchState::Broadcast {
                    txid: txid.clone(),
                    tx_bytes: vec![0x01],
                    assignments: vec![BatchOutputAssignment {
                        intent_id,
                        attempt_id: Uuid::nil(),
                        vout: 0,
                        fee_contribution_sat: 250,
                    }],
                    fee_sat: 250,
                },
            })
            .await
            .expect("store legacy broadcast batch");

        assert!(!storage
            .normalize_legacy_broadcast_attempt_ids(&batch_id, &txid)
            .await
            .expect("normalize must not error"));

        // Neither record was rewritten.
        let member_after = storage
            .get_send_intent(&intent_id)
            .await
            .expect("read member")
            .expect("member exists");
        assert_eq!(member_after.attempt_id, Uuid::nil());
        let batch_after = storage
            .get_send_batch(&batch_id)
            .await
            .expect("read batch")
            .expect("batch exists");
        match batch_after.state {
            SendBatchState::Broadcast { assignments, .. } => {
                assert_eq!(assignments[0].attempt_id, Uuid::nil());
            }
            other => panic!("expected Broadcast, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn broadcast_rejection_record_counts_and_clears() {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory database");
        let storage = BdkStorage::new(Arc::new(db));
        let batch_id = Uuid::new_v4();

        assert!(storage
            .get_broadcast_rejection(&batch_id)
            .await
            .expect("read")
            .is_none());

        for expected in 1..=3_u32 {
            let count = storage
                .record_broadcast_rejection(&batch_id, "txid", "bad-txns-inputs-missingorspent")
                .await
                .expect("record rejection");
            assert_eq!(count, expected);
        }
        let record = storage
            .get_broadcast_rejection(&batch_id)
            .await
            .expect("read")
            .expect("record exists");
        assert_eq!(record.consecutive_rejections, 3);
        assert_eq!(record.last_error, "bad-txns-inputs-missingorspent");

        storage
            .clear_broadcast_rejection(&batch_id)
            .await
            .expect("clear");
        assert!(storage
            .get_broadcast_rejection(&batch_id)
            .await
            .expect("read")
            .is_none());

        // Clearing is idempotent.
        storage
            .clear_broadcast_rejection(&batch_id)
            .await
            .expect("clear again");
    }

    #[tokio::test]
    async fn conditional_signed_batch_delete_succeeds_on_sqlite() {
        let db = cdk_sqlite::mint::memory::empty()
            .await
            .expect("in-memory database");
        let storage = BdkStorage::new(Arc::new(db));
        let batch_id = Uuid::new_v4();
        let signed_state = SendBatchState::Signed {
            tx_bytes: vec![1, 2, 3],
            assignments: Vec::new(),
            fee_sat: 500,
        };
        storage
            .store_send_batch(&SendBatchRecord {
                batch_id,
                state: signed_state.clone(),
            })
            .await
            .expect("store signed batch");

        assert!(storage
            .delete_send_batch_if_state(&batch_id, &signed_state)
            .await
            .expect("delete signed batch"));
        assert!(storage
            .get_send_batch(&batch_id)
            .await
            .expect("get deleted batch")
            .is_none());
    }
}
