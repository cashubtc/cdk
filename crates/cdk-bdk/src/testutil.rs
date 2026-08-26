//! Deterministic KV interleaving harness for concurrency tests.
//!
//! [`GatedKvStore`] is an in-memory read-committed KV store that lets a
//! test pause one actor at a specific read — inside or outside a
//! transaction, before or after the value is sampled — while another
//! actor commits, then release the pause and observe the outcome.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use bdk_wallet::bitcoin::Network;
use bdk_wallet::keys::bip39::Mnemonic;
use cdk_common::common::FeeReserve;
use cdk_common::database::{
    DbTransactionFinalizer, Error as DatabaseError, KVStore, KVStoreDatabase, KVStoreTransaction,
};
use cdk_common::{Amount, CurrencyUnit};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::send::batch_transaction::record::{
    BatchOutputAssignment, SendBatchRecord, SendBatchState,
};
use crate::storage::BdkStorage;
use crate::types::BatchConfig;
use crate::{CdkBdk, ChainSource, EsploraConfig};

/// Build a `CdkBdk` test instance backed by the given KV store, pointed
/// at a bogus Esplora URL. The sync loop is never started, so the
/// unreachable URL is harmless.
pub(crate) async fn build_test_backend(
    kv: Arc<dyn KVStore<Err = DatabaseError> + Send + Sync>,
    batch_config: Option<BatchConfig>,
) -> CdkBdk {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.keep();
    let mnemonic = Mnemonic::from_str(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .expect("mnemonic");

    let chain_source = ChainSource::Esplora(EsploraConfig {
        url: "http://127.0.0.1:1".to_string(),
        parallel_requests: 1,
    });

    let fee_reserve = FeeReserve {
        min_fee_reserve: Amount::new(1, CurrencyUnit::Sat).into(),
        percent_fee_reserve: 0.02,
    };

    CdkBdk::new(
        mnemonic,
        Network::Regtest,
        chain_source,
        path.to_string_lossy().into_owned(),
        fee_reserve,
        kv,
        batch_config,
        1,
        0,
        546,
        60,
        Some(5),
        None,
    )
    .expect("build CdkBdk test instance")
}

/// Persist a minimal signed batch listing each supplied intent once.
pub(crate) async fn store_test_signed_batch(
    storage: &BdkStorage,
    batch_id: Uuid,
    intent_ids: &[Uuid],
) {
    let mut assignments = Vec::with_capacity(intent_ids.len());
    for (vout, intent_id) in intent_ids.iter().enumerate() {
        let intent = storage
            .get_send_intent(intent_id)
            .await
            .expect("load test batch intent")
            .expect("test batch intent exists");
        assignments.push(BatchOutputAssignment {
            intent_id: *intent_id,
            attempt_id: intent.attempt_id,
            vout: u32::try_from(vout).expect("test batch output count fits in u32"),
            fee_contribution_sat: 0,
        });
    }
    storage
        .store_send_batch(&SendBatchRecord {
            batch_id,
            state: SendBatchState::Signed {
                tx_bytes: vec![0x01],
                assignments,
                fee_sat: 0,
            },
        })
        .await
        .expect("store test signed batch");
}

type KvKey = (String, String, String);

/// Buffered write inside a transaction: the key and the value to write
/// (`None` removes the key).
type TxWrite = (KvKey, Option<Vec<u8>>);

/// Where a read gate pauses relative to sampling the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PausePoint {
    /// Block before the value is sampled, so the read observes commits
    /// that land while it is paused.
    BeforeRead,
    /// Block after the value is sampled, so the reader holds a stale
    /// snapshot while another actor commits.
    AfterRead,
}

/// Which read path a gate applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadPath {
    /// Reads outside a transaction (`KVStoreDatabase::kv_read`).
    Direct,
    /// Reads inside a transaction (`KVStoreTransaction::kv_read`).
    Transaction,
}

struct ReadGate {
    primary: String,
    secondary: String,
    path: ReadPath,
    pause: PausePoint,
    remaining: usize,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

/// Handle for an armed read gate: await entry, then release the reader.
pub(crate) struct GateHandle {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl GateHandle {
    /// Wait until the gated read has been reached.
    pub(crate) async fn wait_entered(&self) {
        self.entered.notified().await;
    }

    /// Allow the gated read to continue.
    pub(crate) fn release(&self) {
        self.release.notify_one();
    }
}

/// In-memory KV store with read gates.
///
/// `kv_write_if_absent` reserves its key across transactions (mirroring
/// the atomic conditional insert of the SQL backend), and
/// `kv_write_if_equals` compares against the latest committed value at
/// write time (mirroring the atomic conditional update of the SQL
/// backend), so compare-and-set behavior under test matches production.
#[derive(Clone, Default)]
pub(crate) struct GatedKvStore {
    data: Arc<StdMutex<HashMap<KvKey, Vec<u8>>>>,
    reservations: Arc<StdMutex<HashSet<KvKey>>>,
    gates: Arc<StdMutex<Vec<ReadGate>>>,
}

impl GatedKvStore {
    /// Pause the `occurrence`-th matching read (1-based, counted from
    /// registration) until [`GateHandle::release`] is called.
    pub(crate) fn gate_read(
        &self,
        path: ReadPath,
        pause: PausePoint,
        primary: &str,
        secondary: &str,
        occurrence: usize,
    ) -> GateHandle {
        assert!(occurrence >= 1, "gate occurrences are 1-based");
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        self.gates.lock().expect("lock gates").push(ReadGate {
            primary: primary.to_string(),
            secondary: secondary.to_string(),
            path,
            pause,
            remaining: occurrence,
            entered: entered.clone(),
            release: release.clone(),
        });
        GateHandle { entered, release }
    }

    /// Consume the gate matching this read, if any.
    fn take_gate(
        &self,
        path: ReadPath,
        primary: &str,
        secondary: &str,
    ) -> Option<(PausePoint, Arc<Notify>, Arc<Notify>)> {
        let mut gates = self.gates.lock().expect("lock gates");
        let index = gates.iter().position(|gate| {
            gate.path == path && gate.primary == primary && gate.secondary == secondary
        })?;
        let gate = &mut gates[index];
        gate.remaining -= 1;
        if gate.remaining > 0 {
            return None;
        }
        let gate = gates.remove(index);
        Some((gate.pause, gate.entered, gate.release))
    }

    /// Read `key` from the committed data, honoring a registered gate.
    async fn gated_read(
        &self,
        path: ReadPath,
        primary: &str,
        secondary: &str,
        key: &str,
        tx_writes: Option<&[TxWrite]>,
    ) -> Option<Vec<u8>> {
        let gate = self.take_gate(path, primary, secondary);
        let mut pause_after = None;
        if let Some((pause, entered, release)) = gate {
            match pause {
                PausePoint::BeforeRead => {
                    entered.notify_one();
                    release.notified().await;
                }
                PausePoint::AfterRead => {
                    pause_after = Some((entered, release));
                }
            }
        }

        let map_key = (primary.to_string(), secondary.to_string(), key.to_string());
        let value = tx_writes
            .and_then(|writes| {
                writes
                    .iter()
                    .rev()
                    .find(|(candidate, _)| candidate == &map_key)
                    .map(|(_, value)| value.clone())
            })
            .unwrap_or_else(|| {
                self.data
                    .lock()
                    .expect("lock gated kv store")
                    .get(&map_key)
                    .cloned()
            });

        if let Some((entered, release)) = pause_after {
            entered.notify_one();
            release.notified().await;
        }

        value
    }
}

struct GatedTransaction {
    store: GatedKvStore,
    writes: Vec<TxWrite>,
    reserved: Vec<KvKey>,
}

#[async_trait]
impl DbTransactionFinalizer for GatedTransaction {
    type Err = DatabaseError;

    async fn commit(self: Box<Self>) -> Result<(), Self::Err> {
        {
            let mut data = self.store.data.lock().expect("lock gated kv store");
            for (key, value) in self.writes {
                if let Some(value) = value {
                    data.insert(key, value);
                } else {
                    data.remove(&key);
                }
            }
        }
        let mut reservations = self.store.reservations.lock().expect("lock reservations");
        for key in &self.reserved {
            reservations.remove(key);
        }
        Ok(())
    }

    async fn rollback(self: Box<Self>) -> Result<(), Self::Err> {
        let mut reservations = self.store.reservations.lock().expect("lock reservations");
        for key in &self.reserved {
            reservations.remove(key);
        }
        Ok(())
    }
}

#[async_trait]
impl KVStoreTransaction<DatabaseError> for GatedTransaction {
    async fn kv_read(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DatabaseError> {
        Ok(self
            .store
            .gated_read(
                ReadPath::Transaction,
                primary_namespace,
                secondary_namespace,
                key,
                Some(&self.writes),
            )
            .await)
    }

    async fn kv_write(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), DatabaseError> {
        self.writes.push((
            (
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            ),
            Some(value.to_vec()),
        ));
        Ok(())
    }

    async fn kv_write_if_absent(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<bool, DatabaseError> {
        let map_key = (
            primary_namespace.to_string(),
            secondary_namespace.to_string(),
            key.to_string(),
        );

        let exists = match self
            .writes
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == &map_key)
        {
            Some((_, pending)) => pending.is_some(),
            None => {
                let data = self.store.data.lock().expect("lock gated kv store");
                let reservations = self.store.reservations.lock().expect("lock reservations");
                data.contains_key(&map_key) || reservations.contains(&map_key)
            }
        };

        if exists {
            return Ok(false);
        }

        self.store
            .reservations
            .lock()
            .expect("lock reservations")
            .insert(map_key.clone());
        self.reserved.push(map_key.clone());
        self.writes.push((map_key, Some(value.to_vec())));

        Ok(true)
    }

    async fn kv_remove(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), DatabaseError> {
        self.writes.push((
            (
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            ),
            None,
        ));
        Ok(())
    }

    async fn kv_write_if_equals(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        expected: &[u8],
        replacement: &[u8],
    ) -> Result<bool, DatabaseError> {
        let map_key = (
            primary_namespace.to_string(),
            secondary_namespace.to_string(),
            key.to_string(),
        );

        let current = self
            .writes
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == &map_key)
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| {
                self.store
                    .data
                    .lock()
                    .expect("lock gated kv store")
                    .get(&map_key)
                    .cloned()
            });

        match current {
            Some(current) if current == expected => {
                self.writes.push((map_key, Some(replacement.to_vec())));
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn kv_list(
        &mut self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, DatabaseError> {
        self.store
            .kv_list(primary_namespace, secondary_namespace)
            .await
    }
}

#[async_trait]
impl KVStoreDatabase for GatedKvStore {
    type Err = DatabaseError;

    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, Self::Err> {
        Ok(self
            .gated_read(
                ReadPath::Direct,
                primary_namespace,
                secondary_namespace,
                key,
                None,
            )
            .await)
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, Self::Err> {
        Ok(self
            .data
            .lock()
            .expect("lock gated kv store")
            .keys()
            .filter(|(primary, secondary, _)| {
                primary == primary_namespace && secondary == secondary_namespace
            })
            .map(|(_, _, key)| key.clone())
            .collect())
    }
}

#[async_trait]
impl KVStore for GatedKvStore {
    async fn begin_transaction(
        &self,
    ) -> Result<Box<dyn KVStoreTransaction<Self::Err> + Send + Sync>, DatabaseError> {
        Ok(Box::new(GatedTransaction {
            store: self.clone(),
            writes: Vec::new(),
            reserved: Vec::new(),
        }))
    }
}
