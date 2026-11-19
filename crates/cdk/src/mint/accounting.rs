//! Background aggregation of durable keyset accounting.

use std::time::Duration;

use cdk_common::database::DynMintDatabase;
use tokio::sync::oneshot;

use crate::error::Error;

/// Cadence and maximum batch size for keyset accounting aggregation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeysetAccountingConfig {
    /// Delay between aggregation passes. Must be positive.
    pub interval: Duration,
    /// Maximum events applied per transaction. Must be positive and fit in i64.
    pub batch_size: usize,
}

impl Default for KeysetAccountingConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            batch_size: 1000,
        }
    }
}

impl KeysetAccountingConfig {
    /// Validate the aggregation interval and batch size.
    pub fn validate(&self) -> Result<(), Error> {
        if self.interval.is_zero()
            || self.batch_size == 0
            || i64::try_from(self.batch_size).is_err()
        {
            return Err(Error::Custom("Keyset accounting requires a positive interval and a positive batch size fitting in i64".to_owned()));
        }
        Ok(())
    }
}

pub(super) async fn run(
    db: DynMintDatabase,
    config: KeysetAccountingConfig,
    mut shutdown: oneshot::Receiver<()>,
) {
    loop {
        let result = tokio::select! {
            biased;
            _ = &mut shutdown => break,
            result = db.flush_keyset_accounting(config.batch_size) => result,
        };
        match result {
            Ok(count) if count == config.batch_size => continue,
            Ok(_) => {}
            Err(err) => tracing::warn!("Keyset accounting aggregation failed: {}", err),
        }
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            _ = tokio::time::sleep(config.interval) => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_config() {
        assert!(KeysetAccountingConfig::default().validate().is_ok());
        assert!(KeysetAccountingConfig {
            interval: Duration::ZERO,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(KeysetAccountingConfig {
            batch_size: 0,
            ..Default::default()
        }
        .validate()
        .is_err());
    }

    #[tokio::test]
    async fn mint_lifecycle_drains_batches_and_restarts_accounting() {
        use std::sync::Arc;

        use cdk_common::database::MintDatabase;
        use cdk_sql_common::pool::Pool;
        use cdk_sql_common::stmt::{query, Column};
        use cdk_sqlite::{MintSqliteDatabase, SqliteConnectionManager};

        use crate::mint::MintBuilder;

        let path = std::env::temp_dir().join(format!(
            "cdk-accounting-worker-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(MintSqliteDatabase::new(path.clone()).await.unwrap());
        let pool = Pool::<SqliteConnectionManager>::new(path.clone().into());
        let conn = pool.get().await.unwrap();
        let mint = MintBuilder::new(db.clone())
            .with_keyset_accounting(KeysetAccountingConfig {
                interval: Duration::from_secs(60),
                batch_size: 2,
            })
            .build_with_seed(db.clone(), &[42; 64])
            .await
            .unwrap();
        query("INSERT INTO keyset_accounting (event_id, keyset_id, total_issued)
            VALUES ('1', 'keyset', 1), ('2', 'keyset', 2), ('3', 'keyset', 4), ('4', 'keyset', 8), ('5', 'keyset', 16)")
            .unwrap().execute(&*conn).await.unwrap();
        mint.start().await.unwrap();
        for expected in [31, 63] {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if query("SELECT total_issued FROM keyset_amounts WHERE keyset_id = 'keyset'")
                        .unwrap()
                        .pluck(&*conn)
                        .await
                        .unwrap()
                        == Some(Column::Integer(expected))
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("startup must drain every full batch immediately");
            assert_eq!(
                query("SELECT COUNT(*) FROM keyset_accounting")
                    .unwrap()
                    .pluck(&*conn)
                    .await
                    .unwrap(),
                Some(Column::Integer(0))
            );
            tokio::time::timeout(Duration::from_secs(5), mint.stop())
                .await
                .unwrap()
                .unwrap();
            if expected == 31 {
                query("INSERT INTO keyset_accounting (event_id, keyset_id, total_issued) VALUES ('6', 'keyset', 32)").unwrap().execute(&*conn).await.unwrap();
                mint.start().await.unwrap();
            }
        }
        db.close().await.unwrap();
        drop(conn);
        pool.close().await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
