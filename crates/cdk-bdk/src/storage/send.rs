use std::str::FromStr;

use uuid::Uuid;

use super::{
    BdkStorage, FailedSendAttemptRecord, FinalizedReceiveIntentRecord, FinalizedSendIntentRecord,
    PayjoinCutThroughProgress, PayjoinReceiveSessionRecord, BDK_NAMESPACE,
    FINALIZED_INTENT_NAMESPACE, FINALIZED_RECEIVE_INTENT_BY_QUOTE_NAMESPACE_PREFIX,
    FINALIZED_RECEIVE_INTENT_NAMESPACE, FINALIZED_RECEIVE_INTENT_OUTPOINT_NAMESPACE,
    FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE, PAYJOIN_RECEIVE_SESSION_NAMESPACE,
    SEND_INTENT_NAMESPACE, SEND_INTENT_QUOTE_ID_NAMESPACE,
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
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_INTENT_QUOTE_ID_NAMESPACE,
            &intent.quote_id,
            intent.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Store a new send intent, or re-queue an existing failed intent with
    /// the same quote id.
    pub async fn create_or_retry_failed_send_intent(
        &self,
        intent: &SendIntentRecord,
    ) -> Result<SendIntentRecord, Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

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
            let intent_bytes = tx
                .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &intent_id.to_string())
                .await
                .map_err(Error::from)?
                .ok_or(Error::SendIntentNotFound(intent_id))?;
            let existing: SendIntentRecord = serde_json::from_slice(&intent_bytes)?;

            if !matches!(existing.state, SendIntentState::Failed { .. }) {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::DuplicateQuoteId(intent.quote_id.clone()));
            }

            SendIntentRecord {
                intent_id,
                quote_id: intent.quote_id.clone(),
                address: intent.address.clone(),
                amount_sat: intent.amount_sat,
                max_fee_amount_sat: intent.max_fee_amount_sat,
                tier: intent.tier,
                metadata: intent.metadata.clone(),
                state: intent.state.clone(),
            }
        } else {
            tx.kv_write(
                BDK_NAMESPACE,
                SEND_INTENT_QUOTE_ID_NAMESPACE,
                &intent.quote_id,
                intent.intent_id.to_string().as_bytes(),
            )
            .await
            .map_err(Error::from)?;
            intent.clone()
        };

        let serialized = serde_json::to_vec(&record)?;
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            &record.intent_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
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
        let key = intent_id.to_string();
        if self.get_send_intent(intent_id).await?.is_none() {
            return Err(Error::SendIntentNotFound(*intent_id));
        }

        self.update_record_state::<SendIntentRecord, SendIntentState>(&key, new_state)
            .await
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

    /// Atomically claim still-pending send intents for a normal batch.
    ///
    /// Intents that are no longer pending are skipped. The returned records are
    /// the claimed records with `BatchClaimed` state.
    pub async fn claim_pending_send_intents_for_batch(
        &self,
        intent_ids: &[Uuid],
        batch_id: Uuid,
    ) -> Result<Vec<SendIntentRecord>, Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let mut claimed = Vec::new();

        for intent_id in intent_ids {
            let key = intent_id.to_string();
            let Some(bytes) = tx
                .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
                .await
                .map_err(Error::from)?
            else {
                continue;
            };
            let mut record: SendIntentRecord = serde_json::from_slice(&bytes)?;
            let SendIntentState::Pending { created_at } = record.state else {
                continue;
            };

            record.state = SendIntentState::BatchClaimed {
                batch_id,
                created_at,
            };
            let serialized = serde_json::to_vec(&record)?;
            tx.kv_write(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key, &serialized)
                .await
                .map_err(Error::from)?;
            claimed.push(record);
        }

        tx.commit().await.map_err(Error::from)?;
        Ok(claimed)
    }

    /// Conditionally reserve a pending send intent for cut-through.
    pub async fn reserve_pending_send_intent_for_cut_through(
        &self,
        intent_id: &Uuid,
        reservation_id: Uuid,
        receive_quote_id: &str,
        original_receive_amount_sat: u64,
    ) -> Result<Option<SendIntentRecord>, Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        let key = intent_id.to_string();
        let Some(bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(None);
        };
        let mut record: SendIntentRecord = serde_json::from_slice(&bytes)?;
        let SendIntentState::Pending { created_at } = record.state else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(None);
        };

        record.state = SendIntentState::CutThroughReserved {
            reservation_id,
            receive_quote_id: receive_quote_id.to_string(),
            original_receive_amount_sat,
            created_at,
        };
        let intent_bytes = serde_json::to_vec(&record)?;
        tx.kv_write(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key, &intent_bytes)
            .await
            .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(Some(record))
    }

    /// Release a cut-through-reserved intent back to pending.
    pub async fn release_cut_through_reserved_intent(
        &self,
        intent_id: &Uuid,
        reservation_id: Uuid,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let key = intent_id.to_string();
        let Some(bytes) = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(());
        };
        let mut record: SendIntentRecord = serde_json::from_slice(&bytes)?;
        let SendIntentState::CutThroughReserved {
            reservation_id: current,
            created_at,
            ..
        } = record.state
        else {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(());
        };
        if current != reservation_id {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(());
        }

        record.state = SendIntentState::Pending { created_at };
        let serialized = serde_json::to_vec(&record)?;
        tx.kv_write(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key, &serialized)
            .await
            .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Store a failed pre-sign send attempt tombstone.
    pub async fn add_failed_send_attempt(
        &self,
        record: &FailedSendAttemptRecord,
    ) -> Result<(), Error> {
        self.put_record(record).await
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
        intent_id: &Uuid,
        record: &FinalizedSendIntentRecord,
    ) -> Result<(), Error> {
        let Some(intent) = self.get_send_intent(intent_id).await? else {
            return Err(Error::SendIntentNotFound(*intent_id));
        };

        let serialized = serde_json::to_vec(record)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
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
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Atomically persist receive progress and expose the matching reservation.
    pub async fn expose_cut_through(
        &self,
        session: &PayjoinReceiveSessionRecord,
        intent_id: Uuid,
        reservation_id: Uuid,
        exposed_state: &SendIntentState,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let key = intent_id.to_string();
        let bytes = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
            .ok_or(Error::SendIntentNotFound(intent_id))?;
        let mut intent: SendIntentRecord = serde_json::from_slice(&bytes)?;
        let matches = matches!(
            &intent.state,
            SendIntentState::CutThroughReserved {
                reservation_id: current,
                receive_quote_id,
                ..
            } if *current == reservation_id && receive_quote_id == &session.quote_id
        );
        if !matches {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::Payjoin(
                "stale cut-through reservation token".to_string(),
            ));
        }
        let proposal_txid = match exposed_state {
            SendIntentState::CutThroughExposed { proposal_txid, .. } => proposal_txid,
            _ => {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::Payjoin(
                    "cut-through exposure state is invalid".to_string(),
                ));
            }
        };
        if session.proposal_tx_bytes.is_none()
            || !matches!(
                &session.cut_through,
                Some(PayjoinCutThroughProgress::Active {
                    reservation_id: current,
                    send_intent_id,
                    proposal_txid: active_txid,
                }) if *current == reservation_id
                    && *send_intent_id == intent_id
                    && active_txid == proposal_txid
            )
        {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::Payjoin(
                "cut-through receive marker does not match exposure".to_string(),
            ));
        }
        intent.state = exposed_state.clone();
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            &key,
            &serde_json::to_vec(&intent)?,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            PAYJOIN_RECEIVE_SESSION_NAMESPACE,
            &session.quote_id,
            &serde_json::to_vec(session)?,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Atomically finalize both sides of an exposed cut-through proposal.
    pub async fn finalize_cut_through_pair(
        &self,
        receive_record: &FinalizedReceiveIntentRecord,
        send_record: &FinalizedSendIntentRecord,
        reservation_id: Uuid,
        session: &PayjoinReceiveSessionRecord,
    ) -> Result<(), Error> {
        let serialized_receive = serde_json::to_vec(receive_record)?;
        let serialized_send = serde_json::to_vec(send_record)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let intent_bytes = tx
            .kv_read(
                BDK_NAMESPACE,
                SEND_INTENT_NAMESPACE,
                &send_record.intent_id.to_string(),
            )
            .await
            .map_err(Error::from)?
            .ok_or(Error::SendIntentNotFound(send_record.intent_id))?;
        let intent: SendIntentRecord = serde_json::from_slice(&intent_bytes)?;
        let proposal_txid = match &intent.state {
            SendIntentState::CutThroughExposed {
                reservation_id: current,
                proposal_txid,
                receive_quote_id,
                ..
            } if *current == reservation_id
                && receive_quote_id == &session.quote_id
                && receive_quote_id == &receive_record.quote_id =>
            {
                proposal_txid
            }
            _ => {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::Payjoin(
                    "stale cut-through finalization token".to_string(),
                ));
            }
        };
        if !matches!(
            &session.cut_through,
            Some(PayjoinCutThroughProgress::Confirmed { proposal_txid: confirmed })
                if confirmed == proposal_txid
        ) {
            tx.rollback().await.map_err(Error::from)?;
            return Err(Error::Payjoin(
                "invalid cut-through confirmation marker".to_string(),
            ));
        }

        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_RECEIVE_INTENT_NAMESPACE,
            &receive_record.intent_id.to_string(),
            &serialized_receive,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_RECEIVE_INTENT_OUTPOINT_NAMESPACE,
            &super::outpoint_to_key(&receive_record.outpoint),
            receive_record.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;
        let quote_ns = format!(
            "{FINALIZED_RECEIVE_INTENT_BY_QUOTE_NAMESPACE_PREFIX}__{}",
            receive_record.quote_id
        );
        tx.kv_write(
            BDK_NAMESPACE,
            &quote_ns,
            &receive_record.intent_id.to_string(),
            receive_record.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;

        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_INTENT_NAMESPACE,
            &send_record.intent_id.to_string(),
            &serialized_send,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE,
            &intent.quote_id,
            send_record.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            &send_record.intent_id.to_string(),
        )
        .await
        .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            SEND_INTENT_QUOTE_ID_NAMESPACE,
            &intent.quote_id,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            PAYJOIN_RECEIVE_SESSION_NAMESPACE,
            &session.quote_id,
            &serde_json::to_vec(session)?,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Atomically release an exposed intent and mark the proposal abandoned.
    pub async fn abandon_cut_through(
        &self,
        intent_id: Uuid,
        reservation_id: Uuid,
        session: &PayjoinReceiveSessionRecord,
    ) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        let key = intent_id.to_string();
        let bytes = tx
            .kv_read(BDK_NAMESPACE, SEND_INTENT_NAMESPACE, &key)
            .await
            .map_err(Error::from)?
            .ok_or(Error::SendIntentNotFound(intent_id))?;
        let mut intent: SendIntentRecord = serde_json::from_slice(&bytes)?;
        let created_at = match intent.state {
            SendIntentState::CutThroughExposed {
                reservation_id: current,
                ref proposal_txid,
                ref receive_quote_id,
                created_at,
                ..
            } if current == reservation_id
                && receive_quote_id == &session.quote_id
                && matches!(
                    &session.cut_through,
                    Some(PayjoinCutThroughProgress::Abandoned { proposal_txid: abandoned })
                        if abandoned == proposal_txid
                ) =>
            {
                created_at
            }
            _ => {
                tx.rollback().await.map_err(Error::from)?;
                return Err(Error::Payjoin(
                    "stale cut-through abandonment token".to_string(),
                ));
            }
        };
        intent.state = SendIntentState::Pending { created_at };
        tx.kv_write(
            BDK_NAMESPACE,
            SEND_INTENT_NAMESPACE,
            &key,
            &serde_json::to_vec(&intent)?,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            PAYJOIN_RECEIVE_SESSION_NAMESPACE,
            &session.quote_id,
            &serde_json::to_vec(session)?,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }
}
