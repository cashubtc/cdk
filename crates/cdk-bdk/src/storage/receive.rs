use uuid::Uuid;

use super::{
    outpoint_to_key, BdkStorage, FinalizedReceiveIntentRecord, BDK_NAMESPACE,
    FINALIZED_RECEIVE_INTENT_NAMESPACE, FINALIZED_RECEIVE_INTENT_OUTPOINT_NAMESPACE,
    FINALIZED_RECEIVE_INTENT_QUOTE_ID_NAMESPACE, RECEIVE_ADDRESS_QUOTE_ID_NAMESPACE,
    RECEIVE_INTENT_NAMESPACE, RECEIVE_INTENT_OUTPOINT_NAMESPACE,
};
use crate::error::Error;
use crate::receive::receive_intent::record::ReceiveIntentRecord;

impl BdkStorage {
    // ── Receive address index storage ────────────────────────────────

    /// Track a generated receive address by quote ID.
    pub async fn track_receive_address(&self, address: &str, quote_id: &str) -> Result<(), Error> {
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        tx.kv_write(
            BDK_NAMESPACE,
            RECEIVE_ADDRESS_QUOTE_ID_NAMESPACE,
            address,
            quote_id.as_bytes(),
        )
        .await
        .map_err(Error::from)?;

        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Get the quote ID for a tracked receive address.
    pub async fn get_quote_id_by_receive_address(
        &self,
        address: &str,
    ) -> Result<Option<String>, Error> {
        let quote_id_bytes = self
            .kv_store
            .kv_read(BDK_NAMESPACE, RECEIVE_ADDRESS_QUOTE_ID_NAMESPACE, address)
            .await
            .map_err(Error::from)?;

        let Some(quote_id_bytes) = quote_id_bytes else {
            return Ok(None);
        };

        let quote_id = String::from_utf8(quote_id_bytes)
            .map_err(|e| Error::Wallet(format!("Invalid quote-id index entry: {}", e)))?;
        Ok(Some(quote_id))
    }

    /// Get all tracked receive addresses.
    pub async fn get_tracked_receive_addresses(&self) -> Result<Vec<String>, Error> {
        self.kv_store
            .kv_list(BDK_NAMESPACE, RECEIVE_ADDRESS_QUOTE_ID_NAMESPACE)
            .await
            .map_err(Error::from)
    }

    // ── Receive Intent storage ───────────────────────────────────────

    /// Store a new receive intent if no intent already tracks the same outpoint.
    ///
    /// Uses the outpoint as a secondary index key to ensure idempotent
    /// detection. Returns `true` if the intent was created, `false` if a
    /// duplicate outpoint was found (silently skipped).
    pub async fn create_receive_intent_if_absent(
        &self,
        intent: &ReceiveIntentRecord,
    ) -> Result<bool, Error> {
        let outpoint = match &intent.state {
            crate::receive::receive_intent::record::ReceiveIntentState::Detected {
                outpoint,
                ..
            } => outpoint.clone(),
        };

        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        // Check outpoint index for duplicates (active and finalized)
        let outpoint_key = outpoint_to_key(&outpoint);
        let active = tx
            .kv_read(
                BDK_NAMESPACE,
                RECEIVE_INTENT_OUTPOINT_NAMESPACE,
                &outpoint_key,
            )
            .await
            .map_err(Error::from)?;

        if active.is_some() {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        let finalized = tx
            .kv_read(
                BDK_NAMESPACE,
                FINALIZED_RECEIVE_INTENT_OUTPOINT_NAMESPACE,
                &outpoint_key,
            )
            .await
            .map_err(Error::from)?;

        if finalized.is_some() {
            tx.rollback().await.map_err(Error::from)?;
            return Ok(false);
        }

        let serialized = serde_json::to_vec(intent)?;
        tx.kv_write(
            BDK_NAMESPACE,
            RECEIVE_INTENT_NAMESPACE,
            &intent.intent_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            RECEIVE_INTENT_OUTPOINT_NAMESPACE,
            &outpoint_key,
            intent.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(true)
    }

    /// Get a receive intent by ID.
    pub async fn get_receive_intent(
        &self,
        intent_id: &Uuid,
    ) -> Result<Option<ReceiveIntentRecord>, Error> {
        self.get_record::<ReceiveIntentRecord>(&intent_id.to_string())
            .await
    }

    /// Get all active receive intents.
    pub async fn get_all_receive_intents(&self) -> Result<Vec<ReceiveIntentRecord>, Error> {
        self.list_records::<ReceiveIntentRecord>().await
    }

    /// Delete an active receive intent.
    #[cfg(test)]
    pub async fn delete_receive_intent(&self, intent_id: &Uuid) -> Result<(), Error> {
        let Some(intent) = self.get_receive_intent(intent_id).await? else {
            return Ok(());
        };

        let outpoint_key = match &intent.state {
            crate::receive::receive_intent::record::ReceiveIntentState::Detected {
                outpoint,
                ..
            } => outpoint_to_key(outpoint),
        };

        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            RECEIVE_INTENT_NAMESPACE,
            &intent_id.to_string(),
        )
        .await
        .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            RECEIVE_INTENT_OUTPOINT_NAMESPACE,
            &outpoint_key,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    // ── Finalized Receive Intent storage (tombstones) ────────────────

    /// Atomically finalize an active receive intent and create a tombstone.
    pub async fn finalize_receive_intent(
        &self,
        intent_id: &Uuid,
        record: &FinalizedReceiveIntentRecord,
    ) -> Result<(), Error> {
        let Some(intent) = self.get_receive_intent(intent_id).await? else {
            return Err(Error::ReceiveIntentNotFound(*intent_id));
        };

        let outpoint_key = match &intent.state {
            crate::receive::receive_intent::record::ReceiveIntentState::Detected {
                outpoint,
                ..
            } => outpoint_to_key(outpoint),
        };

        let serialized = serde_json::to_vec(record)?;
        let mut tx = self
            .kv_store
            .begin_transaction()
            .await
            .map_err(Error::from)?;

        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_RECEIVE_INTENT_NAMESPACE,
            &record.intent_id.to_string(),
            &serialized,
        )
        .await
        .map_err(Error::from)?;
        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_RECEIVE_INTENT_OUTPOINT_NAMESPACE,
            &outpoint_key,
            record.intent_id.to_string().as_bytes(),
        )
        .await
        .map_err(Error::from)?;

        let quote_id_index: Vec<Uuid> = tx
            .kv_read(
                BDK_NAMESPACE,
                FINALIZED_RECEIVE_INTENT_QUOTE_ID_NAMESPACE,
                &record.quote_id,
            )
            .await
            .map_err(Error::from)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .map_err(Error::from)?
            .unwrap_or_default();
        let mut quote_id_index = quote_id_index;
        quote_id_index.push(record.intent_id);
        tx.kv_write(
            BDK_NAMESPACE,
            FINALIZED_RECEIVE_INTENT_QUOTE_ID_NAMESPACE,
            &record.quote_id,
            &serde_json::to_vec(&quote_id_index)?,
        )
        .await
        .map_err(Error::from)?;

        tx.kv_remove(
            BDK_NAMESPACE,
            RECEIVE_INTENT_NAMESPACE,
            &intent_id.to_string(),
        )
        .await
        .map_err(Error::from)?;
        tx.kv_remove(
            BDK_NAMESPACE,
            RECEIVE_INTENT_OUTPOINT_NAMESPACE,
            &outpoint_key,
        )
        .await
        .map_err(Error::from)?;
        tx.commit().await.map_err(Error::from)?;
        Ok(())
    }

    /// Look up a finalized receive intent tombstone by intent ID.
    #[cfg(test)]
    pub async fn get_finalized_receive_intent(
        &self,
        intent_id: &Uuid,
    ) -> Result<Option<FinalizedReceiveIntentRecord>, Error> {
        self.get_record::<FinalizedReceiveIntentRecord>(&intent_id.to_string())
            .await
    }

    /// Look up finalized receive intent tombstones by quote ID.
    pub async fn get_finalized_receive_intents_by_quote_id(
        &self,
        quote_id: &str,
    ) -> Result<Vec<FinalizedReceiveIntentRecord>, Error> {
        let intent_ids: Vec<Uuid> = self
            .kv_store
            .kv_read(
                BDK_NAMESPACE,
                FINALIZED_RECEIVE_INTENT_QUOTE_ID_NAMESPACE,
                quote_id,
            )
            .await
            .map_err(Error::from)?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .map_err(Error::from)?
            .unwrap_or_default();

        let mut results = Vec::new();
        for intent_id in intent_ids {
            if let Some(record) = self
                .get_record::<FinalizedReceiveIntentRecord>(&intent_id.to_string())
                .await?
            {
                results.push(record);
            }
        }
        Ok(results)
    }
}
