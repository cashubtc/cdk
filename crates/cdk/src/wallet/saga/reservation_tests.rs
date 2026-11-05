//! Exercise preparation cancellation through the real SQLite wallet database.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cdk_common::database::{Error, WalletDatabase};
use cdk_common::wallet::KeysetLoadPolicy;
use cdk_sql_common::database::{DatabaseConnector, DatabaseExecutor, DatabaseTransaction};
use cdk_sql_common::pool::{self, DatabaseConfig, DatabasePool};
use cdk_sql_common::stmt::{Column, Statement};
use cdk_sql_common::SQLWalletDatabase;
use cdk_sqlite::SqliteConnectionManager;
use tokio::sync::oneshot;

use crate::wallet::test_utils::{
    create_test_wallet_with_mock_seed, test_keyset, test_keyset_id, test_melt_quote, test_mint_url,
    test_proof_info, MockMintConnector,
};
use crate::Amount;

type Sqlite = <SqliteConnectionManager as DatabasePool>::Connection;
type SqliteConfig = <SqliteConnectionManager as DatabasePool>::Config;
type SqliteTransaction = <Sqlite as DatabaseConnector>::Transaction;

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

#[derive(Debug, Clone)]
struct Config {
    sqlite: Arc<SqliteConfig>,
    gate: Arc<Mutex<Option<Gate>>>,
}

impl DatabaseConfig for Config {
    fn max_size(&self) -> usize {
        1
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(5)
    }
}

#[derive(Debug)]
struct Manager;

impl DatabasePool for Manager {
    type Config = Config;
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn new_resource(
        config: &Config,
        stale: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<Connection, pool::Error<Self::Error>> {
        Ok(Connection {
            inner: SqliteConnectionManager::new_resource(&config.sqlite, stale, timeout)?,
            gate: config.gate.clone(),
            writes_proofs: AtomicBool::new(false),
        })
    }
}

#[derive(Debug)]
struct Connection {
    inner: Sqlite,
    gate: Arc<Mutex<Option<Gate>>>,
    writes_proofs: AtomicBool,
}

impl Connection {
    async fn pause(&self, boundary: Boundary) {
        let gate = {
            let mut gate = self.gate.lock().unwrap();
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
}

impl DatabaseConnector for Connection {
    type Transaction = Self;
}

#[async_trait::async_trait]
impl DatabaseTransaction<Self> for Connection {
    async fn begin(conn: &mut Self) -> Result<(), Error> {
        conn.writes_proofs.store(false, Ordering::SeqCst);
        SqliteTransaction::begin(&mut conn.inner).await
    }

    async fn commit(conn: &mut Self) -> Result<(), Error> {
        let writes_proofs = conn.writes_proofs.load(Ordering::SeqCst);
        if writes_proofs {
            conn.pause(Boundary::BeforeCommit).await;
        }
        SqliteTransaction::commit(&mut conn.inner).await?;
        if writes_proofs {
            // Withhold the acknowledgement after the actual SQLite COMMIT.
            // Cancellation must not rely on rolling this write back.
            conn.pause(Boundary::AfterCommit).await;
        }
        Ok(())
    }

    async fn rollback(conn: &mut Self) -> Result<(), Error> {
        SqliteTransaction::rollback(&mut conn.inner).await
    }
}

#[async_trait::async_trait]
impl DatabaseExecutor for Connection {
    fn name() -> &'static str {
        Sqlite::name()
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        let mut copy = Statement::default();
        copy.parts = statement.parts.clone();
        let sql = copy
            .to_sql()?
            .0
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if sql.starts_with("UPDATE proof SET state = 'RESERVED'")
            || sql.starts_with("INSERT INTO proof ")
        {
            self.writes_proofs.store(true, Ordering::SeqCst);
        }
        let result = self.inner.execute(statement).await?;
        if sql.starts_with("INSERT INTO wallet_sagas ") {
            self.pause(Boundary::SagaSaved).await;
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
            SQLWalletDatabase::<Manager>::new(Config {
                sqlite: Arc::new(SqliteConfig::from(":memory:")),
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
