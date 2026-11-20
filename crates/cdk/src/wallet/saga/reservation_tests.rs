//! Exercise preparation cancellation through the real SQLite wallet database.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdk_common::database::{Error, WalletDatabase};
use cdk_common::wallet::KeysetLoadPolicy;
use cdk_sql_common::database::{DatabaseExecutor, SqlBackend, SqlConnection, SqlTransaction};
use cdk_sql_common::stmt::{Column, Statement};
use cdk_sql_common::SQLWalletDatabase;
use cdk_sqlite::{Config as SqliteConfig, SqliteBackend, SqliteConnection, SqliteTransaction};
use tokio::sync::oneshot;

use crate::wallet::test_utils::{
    create_test_wallet_with_mock_seed, test_keyset, test_keyset_id, test_melt_quote, test_mint_url,
    test_proof_info, MockMintConnector,
};
use crate::Amount;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boundary {
    SagaSaved,
    BeforeCommit,
    AfterCommit,
}

#[derive(Debug)]
struct Gate {
    boundary: Boundary,
    entered: oneshot::Sender<()>,
}

type SharedGate = Arc<Mutex<Option<Gate>>>;

async fn pause(gate: &SharedGate, boundary: Boundary) {
    let gate = {
        let mut gate = gate.lock().unwrap();
        match gate.as_ref() {
            Some(armed) if armed.boundary == boundary => gate.take(),
            _ => None,
        }
    };
    if let Some(gate) = gate {
        gate.entered.send(()).unwrap();
        std::future::pending::<()>().await;
    }
}

#[derive(Debug, Clone)]
struct Config {
    sqlite: SqliteConfig,
    gate: SharedGate,
}

/// SQLite backend that can suspend at chosen proof-reservation boundaries.
#[derive(Debug, Clone)]
struct GatedBackend {
    inner: SqliteBackend,
    gate: SharedGate,
}

#[async_trait::async_trait]
impl SqlBackend for GatedBackend {
    type Config = Config;
    type Connection = Connection;
    type Transaction = Transaction;

    fn new(config: Config) -> Result<Self, Error> {
        Ok(Self {
            inner: SqliteBackend::new(config.sqlite)?,
            gate: config.gate,
        })
    }

    async fn acquire(&self) -> Result<Connection, Error> {
        Ok(Connection {
            inner: self.inner.acquire().await?,
            gate: self.gate.clone(),
        })
    }

    async fn close(&self) -> Result<(), Error> {
        self.inner.close().await
    }
}

#[derive(Debug)]
struct Connection {
    inner: SqliteConnection,
    gate: SharedGate,
}

#[async_trait::async_trait]
impl SqlConnection for Connection {
    type Transaction = Transaction;

    async fn begin_transaction(self) -> Result<Transaction, Error> {
        Ok(Transaction {
            inner: self.inner.begin_transaction().await?,
            gate: self.gate,
            writes_proofs: AtomicBool::new(false),
        })
    }
}

#[derive(Debug)]
struct Transaction {
    inner: SqliteTransaction,
    gate: SharedGate,
    writes_proofs: AtomicBool,
}

#[async_trait::async_trait]
impl SqlTransaction for Transaction {
    async fn commit(self) -> Result<(), Error> {
        let writes_proofs = self.writes_proofs.load(Ordering::SeqCst);
        if writes_proofs {
            pause(&self.gate, Boundary::BeforeCommit).await;
        }
        let gate = self.gate.clone();
        self.inner.commit().await?;
        if writes_proofs {
            // Withhold the acknowledgement after the actual SQLite COMMIT.
            // Cancellation must not rely on rolling this write back.
            pause(&gate, Boundary::AfterCommit).await;
        }
        Ok(())
    }

    async fn rollback(self) -> Result<(), Error> {
        self.inner.rollback().await
    }
}

fn normalized_sql(statement: &Statement) -> Result<String, Error> {
    let mut copy = Statement::default();
    copy.parts = statement.parts.clone();
    Ok(copy
        .to_sql()?
        .0
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

macro_rules! gated_executor {
    ($type:ty, $writes_proofs:expr) => {
        #[async_trait::async_trait]
        impl DatabaseExecutor for $type {
            fn name() -> &'static str {
                SqliteConnection::name()
            }

            async fn execute(&self, statement: Statement) -> Result<usize, Error> {
                let sql = normalized_sql(&statement)?;
                if sql.starts_with("UPDATE proof SET state = 'RESERVED'")
                    || sql.starts_with("INSERT INTO proof ")
                {
                    ($writes_proofs)(self);
                }
                let result = self.inner.execute(statement).await?;
                if sql.starts_with("INSERT INTO wallet_sagas ") {
                    pause(&self.gate, Boundary::SagaSaved).await;
                }
                Ok(result)
            }

            async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
                self.inner.fetch_one(statement).await
            }

            async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
                self.inner.fetch_all(statement).await
            }

            async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
                self.inner.pluck(statement).await
            }

            async fn batch(&self, statement: Statement) -> Result<(), Error> {
                self.inner.batch(statement).await
            }
        }
    };
}

gated_executor!(Connection, |_: &Connection| ());
gated_executor!(Transaction, |tx: &Transaction| tx
    .writes_proofs
    .store(true, Ordering::SeqCst));

#[derive(Debug, Clone, Copy)]
enum Preparation {
    Send,
    Melt,
    MeltWithProofs,
}

async fn check_cancellation(preparation: Preparation, proof_amount: u64) {
    for boundary in [
        Boundary::SagaSaved,
        Boundary::BeforeCommit,
        Boundary::AfterCommit,
    ] {
        let gate = Arc::new(Mutex::new(None));
        let db = Arc::new(
            SQLWalletDatabase::<GatedBackend>::new(Config {
                sqlite: SqliteConfig::from(":memory:"),
                gate: gate.clone(),
            })
            .await
            .unwrap(),
        );
        let proof = test_proof_info(test_keyset_id(), proof_amount, test_mint_url());
        db.update_proofs(vec![proof.clone()], vec![]).await.unwrap();
        let mut quote = test_melt_quote();
        quote.amount = Amount::from(2);
        quote.fee_reserve = Amount::ZERO;
        db.add_melt_quote(quote.clone()).await.unwrap();

        let client = Arc::new(MockMintConnector::new());
        client.reset_default_mint_state();
        let mut keyset = test_keyset();
        keyset.input_fee_ppk = 0;
        client.set_active_keyset(keyset);
        let wallet = create_test_wallet_with_mock_seed(db.clone(), client.clone(), [7; 64]).await;
        wallet.keysets(KeysetLoadPolicy::default()).await.unwrap();

        let (entered, waiting) = oneshot::channel();
        *gate.lock().unwrap() = Some(Gate { boundary, entered });
        let quote_id = quote.id.clone();
        let worker = tokio::spawn(async move {
            match preparation {
                Preparation::Send => {
                    let _ = wallet
                        .prepare_send(Amount::from(2), Default::default())
                        .await
                        .unwrap();
                }
                Preparation::Melt => {
                    let _ = wallet
                        .prepare_melt(&quote_id, HashMap::new())
                        .await
                        .unwrap();
                }
                Preparation::MeltWithProofs => {
                    let _ = wallet
                        .prepare_melt_proofs(&quote_id, vec![proof.proof], HashMap::new())
                        .await
                        .unwrap();
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());

        // Reading the single-connection database also waits for drop rollback.
        let sagas = db.get_incomplete_sagas().await.unwrap();
        assert_eq!(sagas.len(), 1, "{preparation:?} at {boundary:?}");
        let operation_id = sagas[0].id;
        let reserved = db.get_reserved_proofs(&operation_id).await.unwrap();
        assert_eq!(
            reserved.len(),
            usize::from(boundary == Boundary::AfterCommit)
        );

        // A newly constructed wallet must recover solely from persisted state.
        let reopened = create_test_wallet_with_mock_seed(db.clone(), client, [7; 64]).await;
        let report = reopened.recover_incomplete_sagas().await.unwrap();
        assert_eq!(report.compensated, 1);
        assert_eq!(report.failed, 0);
        assert!(db.get_incomplete_sagas().await.unwrap().is_empty());
        assert!(db
            .get_reserved_proofs(&operation_id)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            reopened.total_balance().await.unwrap(),
            Amount::from(proof_amount)
        );
        assert!(db
            .get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap()
            .used_by_operation
            .is_none());
        assert!(reopened
            .recover_incomplete_sagas()
            .await
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn cancelled_exact_send_preparation_is_recoverable() {
    check_cancellation(Preparation::Send, 2).await;
}

#[tokio::test]
async fn cancelled_swap_send_preparation_is_recoverable() {
    check_cancellation(Preparation::Send, 4).await;
}

#[tokio::test]
async fn cancelled_exact_melt_preparation_is_recoverable() {
    check_cancellation(Preparation::Melt, 2).await;
}

#[tokio::test]
async fn cancelled_swap_melt_preparation_is_recoverable() {
    check_cancellation(Preparation::Melt, 4).await;
}

#[tokio::test]
async fn cancelled_explicit_melt_preparation_is_recoverable() {
    check_cancellation(Preparation::MeltWithProofs, 2).await;
}
