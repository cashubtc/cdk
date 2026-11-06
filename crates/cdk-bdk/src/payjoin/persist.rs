//! Durable PDK event logs. Closed logs are retained for transaction recovery.

use std::marker::PhantomData;

use bdk_wallet::bitcoin::OutPoint;
use payjoin::persist::AsyncSessionPersister;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::storage::{BdkStorage, BDK_NAMESPACE};
use crate::Error;

pub(super) const NAMESPACE: &str = "payjoin_session";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecord {
    pub events: Vec<serde_json::Value>,
    pub closed: bool,
    pub inputs: Vec<OutPoint>,
    pub expires_at: u64,
    pub completed: bool,
    pub address: Option<String>,
    pub input_value_sat: u64,
}

pub(super) struct Persister<E> {
    pub storage: BdkStorage,
    pub id: String,
    marker: PhantomData<E>,
}

impl<E> Persister<E> {
    pub fn new(storage: BdkStorage, id: String) -> Self {
        Self {
            storage,
            id,
            marker: PhantomData,
        }
    }

    pub async fn record(&self) -> Result<SessionRecord, Error> {
        let bytes = self
            .storage
            .kv_store
            .kv_read(BDK_NAMESPACE, NAMESPACE, &self.id)
            .await?
            .ok_or_else(|| Error::Payjoin("Missing session".to_string()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn insert(&self, record: &SessionRecord) -> Result<(), Error> {
        let mut tx = self.storage.kv_store.begin_transaction().await?;
        if !tx
            .kv_write_if_absent(
                BDK_NAMESPACE,
                NAMESPACE,
                &self.id,
                &serde_json::to_vec(record)?,
            )
            .await?
        {
            return Err(Error::Payjoin("Session already exists".to_string()));
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn write(&self, record: &SessionRecord) -> Result<(), Error> {
        let mut tx = self.storage.kv_store.begin_transaction().await?;
        tx.kv_write(
            BDK_NAMESPACE,
            NAMESPACE,
            &self.id,
            &serde_json::to_vec(record)?,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

impl<E> AsyncSessionPersister for Persister<E>
where
    E: Serialize + DeserializeOwned + Send + Sync + 'static,
{
    type SessionEvent = E;
    type InternalStorageError = Error;

    async fn save_event(&self, event: E) -> Result<(), Error> {
        let mut tx = self.storage.kv_store.begin_transaction().await?;
        let bytes = tx
            .kv_read(BDK_NAMESPACE, NAMESPACE, &self.id)
            .await?
            .ok_or_else(|| Error::Payjoin("Missing session".to_string()))?;
        let mut record: SessionRecord = serde_json::from_slice(&bytes)?;
        if record.closed {
            return Err(Error::Payjoin(
                "Cannot append to a closed session".to_string(),
            ));
        }
        record.events.push(serde_json::to_value(event)?);
        if !tx
            .kv_write_if_equals(
                BDK_NAMESPACE,
                NAMESPACE,
                &self.id,
                &bytes,
                &serde_json::to_vec(&record)?,
            )
            .await?
        {
            return Err(Error::Payjoin("Concurrent session update".to_string()));
        }
        tx.commit().await?;
        Ok(())
    }

    async fn load(&self) -> Result<Box<dyn Iterator<Item = E> + Send>, Error> {
        let events = self
            .record()
            .await?
            .events
            .into_iter()
            // PDK uses zero-field tuple variants. Deserialize from JSON bytes,
            // since serde_json's Value deserializer treats empty tuples as unit.
            .map(|event| serde_json::from_slice(&serde_json::to_vec(&event)?))
            .collect::<Result<Vec<E>, serde_json::Error>>()?;
        Ok(Box::new(events.into_iter()))
    }

    async fn close(&self) -> Result<(), Error> {
        let mut record = self.record().await?;
        record.closed = true;
        self.write(&record).await
    }
}

impl BdkStorage {
    pub(crate) async fn payjoin_records(&self) -> Result<Vec<(String, SessionRecord)>, Error> {
        let mut records = Vec::new();
        for id in self.kv_store.kv_list(BDK_NAMESPACE, NAMESPACE).await? {
            let record = Persister::<serde_json::Value>::new(self.clone(), id.clone())
                .record()
                .await?;
            records.push((id, record));
        }
        Ok(records)
    }

    pub(crate) async fn payjoin_reserved_inputs(&self) -> Result<Vec<OutPoint>, Error> {
        let mut inputs = Vec::new();
        for id in self.kv_store.kv_list(BDK_NAMESPACE, NAMESPACE).await? {
            let record = Persister::<serde_json::Value>::new(self.clone(), id)
                .record()
                .await?;
            if !record.completed {
                inputs.extend(record.inputs);
            }
        }
        Ok(inputs)
    }
}
