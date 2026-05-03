use std::str::FromStr;

use uuid::Uuid;

use super::{
    BdkStorage, FinalizedSendIntentRecord, BDK_NAMESPACE, FINALIZED_INTENT_NAMESPACE,
    FINALIZED_SEND_INTENT_QUOTE_ID_NAMESPACE, SEND_INTENT_NAMESPACE,
    SEND_INTENT_QUOTE_ID_NAMESPACE,
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
}
