//! Cancellation at actual KV operations, including ambiguous commit completion.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cdk_common::database::{
    DbTransactionFinalizer, Error as DbError, KVStore, KVStoreDatabase, KVStoreTransaction,
};
use tokio::sync::Notify;

use super::*;

pub(super) struct Cut {
    target: usize,
    exit_process: bool,
    count: AtomicUsize,
    pub reached: Notify,
    pub trace: Mutex<Vec<String>>,
}

impl Cut {
    pub fn new(target: usize) -> Arc<Self> {
        Arc::new(Self {
            target,
            exit_process: false,
            count: AtomicUsize::new(0),
            reached: Notify::new(),
            trace: Mutex::new(Vec::new()),
        })
    }

    async fn checkpoint(&self, operation: &str, namespace: &str) {
        self.trace
            .lock()
            .unwrap()
            .push(format!("{operation} {namespace}"));
        if self.count.fetch_add(1, Ordering::SeqCst) + 1 == self.target {
            if self.exit_process {
                // No unwinding or transaction/database destructors. The parent
                // recovers only the files left by this process.
                std::process::exit(86);
            }
            self.reached.notify_one();
            std::future::pending::<()>().await;
        }
    }
}

pub(super) fn instrument(backend: &mut CdkBdk, cut: Arc<Cut>) {
    backend.storage = crate::BdkStorage::new(Arc::new(InterruptedStore {
        inner: backend.storage.kv_store.clone(),
        cut,
    }));
}

struct InterruptedStore {
    inner: Arc<dyn KVStore<Err = DbError> + Send + Sync>,
    cut: Arc<Cut>,
}

struct InterruptedTransaction {
    inner: Box<dyn KVStoreTransaction<DbError> + Send + Sync>,
    cut: Arc<Cut>,
}

#[async_trait::async_trait]
impl KVStoreDatabase for InterruptedStore {
    type Err = DbError;
    async fn kv_read(&self, p: &str, s: &str, k: &str) -> Result<Option<Vec<u8>>, DbError> {
        self.inner.kv_read(p, s, k).await
    }
    async fn kv_list(&self, p: &str, s: &str) -> Result<Vec<String>, DbError> {
        self.inner.kv_list(p, s).await
    }
}

#[async_trait::async_trait]
impl KVStore for InterruptedStore {
    async fn begin_transaction(
        &self,
    ) -> Result<Box<dyn KVStoreTransaction<DbError> + Send + Sync>, DbError> {
        Ok(Box::new(InterruptedTransaction {
            inner: self.inner.begin_transaction().await?,
            cut: self.cut.clone(),
        }))
    }
}

#[async_trait::async_trait]
impl DbTransactionFinalizer for InterruptedTransaction {
    type Err = DbError;
    async fn commit(self: Box<Self>) -> Result<(), DbError> {
        self.cut.checkpoint("before commit", "").await;
        self.inner.commit().await?;
        // The durable commit succeeded but the caller has not observed it yet.
        self.cut.checkpoint("after commit", "").await;
        Ok(())
    }
    async fn rollback(self: Box<Self>) -> Result<(), DbError> {
        self.inner.rollback().await
    }
}

#[async_trait::async_trait]
impl KVStoreTransaction<DbError> for InterruptedTransaction {
    async fn kv_read(&mut self, p: &str, s: &str, k: &str) -> Result<Option<Vec<u8>>, DbError> {
        self.inner.kv_read(p, s, k).await
    }
    async fn kv_list(&mut self, p: &str, s: &str) -> Result<Vec<String>, DbError> {
        self.inner.kv_list(p, s).await
    }
    async fn kv_write(&mut self, p: &str, s: &str, k: &str, v: &[u8]) -> Result<(), DbError> {
        self.cut.checkpoint("before write", s).await;
        self.inner.kv_write(p, s, k, v).await?;
        self.cut.checkpoint("after write", s).await;
        Ok(())
    }
    async fn kv_write_if_absent(
        &mut self,
        p: &str,
        s: &str,
        k: &str,
        v: &[u8],
    ) -> Result<bool, DbError> {
        self.cut.checkpoint("before insert", s).await;
        let inserted = self.inner.kv_write_if_absent(p, s, k, v).await?;
        self.cut.checkpoint("after insert", s).await;
        Ok(inserted)
    }
    async fn kv_write_if_equals(
        &mut self,
        p: &str,
        s: &str,
        k: &str,
        expected: &[u8],
        v: &[u8],
    ) -> Result<bool, DbError> {
        self.cut.checkpoint("before compare-and-set", s).await;
        let replaced = self.inner.kv_write_if_equals(p, s, k, expected, v).await?;
        self.cut.checkpoint("after compare-and-set", s).await;
        Ok(replaced)
    }
    async fn kv_remove(&mut self, p: &str, s: &str, k: &str) -> Result<(), DbError> {
        self.cut.checkpoint("before remove", s).await;
        self.inner.kv_remove(p, s, k).await?;
        self.cut.checkpoint("after remove", s).await;
        Ok(())
    }
}

async fn reopen(path: &std::path::Path) -> CdkBdk {
    use bdk_wallet::keys::bip39::Mnemonic;
    use cdk_common::{Amount as CdkAmount, CurrencyUnit};

    let storage = Arc::new(
        cdk_sqlite::MintSqliteDatabase::new(path.join("sessions.sqlite").to_str().unwrap())
            .await
            .unwrap(),
    );
    CdkBdk::new(
        Mnemonic::from_str("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about").unwrap(),
        Network::Regtest,
        crate::ChainSource::Esplora(crate::EsploraConfig { url: "http://127.0.0.1:1".to_string(), parallel_requests: 1 }),
        path.to_str().unwrap().to_string(),
        cdk_common::common::FeeReserve { min_fee_reserve: CdkAmount::new(1, CurrencyUnit::Sat).into(), percent_fee_reserve: 0.02 },
        storage, None, 1, 0, 1, 60, None, None,
    ).unwrap().with_payjoin(PayjoinConfig { relay_url: "https://127.0.0.1:1".to_string(), directory_url: None }).unwrap()
}

#[tokio::test]
async fn every_handoff_write_and_commit_can_be_cancelled_and_reopened() {
    handoff_interruption_matrix(false).await;
}

#[tokio::test]
async fn every_handoff_write_and_commit_survives_process_exit() {
    handoff_interruption_matrix(true).await;
}

#[tokio::test]
#[ignore = "subprocess helper invoked by the crash matrix"]
async fn crash_child() {
    let Some(path) = std::env::var_os("CDK_PAYJOIN_CRASH_PATH") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let target = std::env::var("CDK_PAYJOIN_CRASH_POINT")
        .unwrap()
        .parse()
        .unwrap();
    let mut backend = reopen(&path).await;
    let intent = backend
        .storage
        .get_pending_send_intents()
        .await
        .unwrap()
        .pop()
        .unwrap();
    let mut cut = Cut::new(target);
    Arc::get_mut(&mut cut).unwrap().exit_process = true;
    instrument(&mut backend, cut);
    backend.process_payjoin_send(&intent).await.unwrap();
    panic!("crash boundary was not reached");
}

async fn handoff_interruption_matrix(hard_exit: bool) {
    let mut boundaries = 0;
    // First enumerate the real persistence operations, then interrupt each one.
    let mut target = 0;
    loop {
        let temp = tempfile::tempdir().unwrap();
        let mut backend = reopen(temp.path()).await;
        crate::tests::fund_backend_wallet_transactions(&backend, &[100_000]).await;
        let intent = intent(&backend, destination(Duration::from_secs(600))).await;
        let (persister, original) = persist_sender(&backend, &intent).await;
        persister
            .save_event(SessionEvent::Closed(SessionOutcome::Aborted))
            .await
            .unwrap();
        drop(persister);
        let expected = original.extract_tx().unwrap();
        if hard_exit && target > 0 {
            drop(backend);
            let path = temp.path().to_owned();
            let output = tokio::task::spawn_blocking(move || {
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "payjoin::tests::faults::crash_child",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("CDK_PAYJOIN_CRASH_PATH", path)
                    .env("CDK_PAYJOIN_CRASH_POINT", target.to_string())
                    .output()
                    .unwrap()
            })
            .await
            .unwrap();
            assert_eq!(
                output.status.code(),
                Some(86),
                "boundary {target}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        } else {
            let mut events = backend.payment_sender.subscribe();
            let cut = Cut::new(target);
            instrument(&mut backend, cut.clone());
            let task_backend = backend.clone();
            let task_intent = intent.clone();
            let task =
                tokio::spawn(async move { task_backend.process_payjoin_send(&task_intent).await });
            if target == 0 {
                task.await.unwrap().unwrap();
                boundaries = cut.trace.lock().unwrap().len();
                assert!(boundaries >= 12, "must exercise the complete handoff");
                eprintln!(
                    "Payjoin handoff: {boundaries} write/commit boundaries, hard_exit={hard_exit}"
                );
            } else {
                tokio::time::timeout(Duration::from_secs(10), cut.reached.notified())
                    .await
                    .unwrap_or_else(|_| {
                        panic!(
                            "boundary {target} not reached: {:?}",
                            cut.trace.lock().unwrap()
                        )
                    });
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
            }
            assert!(
                events.try_recv().is_err(),
                "interruption must not emit a refund"
            );
            drop(backend);
        }
        let reopened = reopen(temp.path()).await;
        let stored_intent = reopened
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap();
        let batch = reopened
            .storage
            .get_send_batch(&intent.intent_id)
            .await
            .unwrap();
        // The new handoff commits both records or neither, including when commit
        // completed just before cancellation prevented the caller observing it.
        assert_eq!(
            batch.is_none(),
            matches!(stored_intent.state, SendIntentState::Pending { .. })
        );
        assert!(!reopened
            .storage
            .payjoin_reserved_inputs()
            .await
            .unwrap()
            .is_empty());
        reopened.process_payjoin_sessions().await.unwrap();
        let batch = reopened
            .storage
            .get_send_batch(&intent.intent_id)
            .await
            .unwrap()
            .unwrap();
        let SendBatchState::Broadcast {
            tx_bytes,
            assignments,
            ..
        } = batch.state
        else {
            panic!("handoff did not resume at boundary {target}");
        };
        assert_eq!(
            tx_bytes,
            bdk_wallet::bitcoin::consensus::serialize(&expected)
        );
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].intent_id, intent.intent_id);
        assert!(matches!(
            reopened
                .storage
                .get_send_intent(&intent.intent_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            SendIntentState::AwaitingConfirmation { .. }
        ));
        assert!(reopened
            .storage
            .get_finalized_intent(&intent.intent_id)
            .await
            .unwrap()
            .is_none());
        target += 1;
        if target > boundaries {
            break;
        }
    }
}

#[tokio::test]
async fn typed_handoff_rejects_invalid_assignments_and_stale_intents() {
    let (backend, _temp) = backend().await;
    let intent = intent(&backend, destination(Duration::from_secs(600))).await;
    let (_, psbt) = persist_sender(&backend, &intent).await;
    let transaction = psbt.extract_tx().unwrap();
    let (fee, vout) = backend
        .payjoin_sender_assignment(&intent, &transaction)
        .await
        .unwrap();
    let SendIntentAny::Pending(pending) = payment_intent::from_record(&intent) else {
        panic!("pending intent");
    };
    assert!(SendBatch::from_payjoin(
        &backend.storage,
        pending.clone(),
        &transaction,
        u32::MAX,
        fee
    )
    .await
    .is_err());
    assert!(SendBatch::from_payjoin(
        &backend.storage,
        pending.clone(),
        &transaction,
        vout,
        pending.max_fee_amount + 1
    )
    .await
    .is_err());
    assert!(backend
        .storage
        .get_send_batch(&intent.intent_id)
        .await
        .unwrap()
        .is_none());
    SendBatch::from_payjoin(&backend.storage, pending.clone(), &transaction, vout, fee)
        .await
        .unwrap();
    let mut different_transaction = transaction.clone();
    different_transaction.lock_time = absolute::LockTime::from_consensus(1);
    assert!(
        SendBatch::from_payjoin(&backend.storage, pending, &different_transaction, vout, fee)
            .await
            .is_err()
    );
    let batch = backend
        .storage
        .get_send_batch(&intent.intent_id)
        .await
        .unwrap()
        .unwrap();
    let SendBatchState::Signed { tx_bytes, .. } = batch.state else {
        panic!("signed batch");
    };
    assert_eq!(
        tx_bytes,
        bdk_wallet::bitcoin::consensus::serialize(&transaction)
    );
    assert!(matches!(
        backend
            .storage
            .get_send_intent(&intent.intent_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        SendIntentState::Batched { .. }
    ));
}
