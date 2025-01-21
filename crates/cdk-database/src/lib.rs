//! CDK-Database instance
//!
//! This crate will create a database instance based on the provided engine.
use std::path::PathBuf;
use std::sync::Arc;

use cdk_redb::MintRedbDatabase;
use cdk_sqlite::MintSqliteDatabase;
use serde::{Deserialize, Serialize};

/// Database engine definition
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseEngine {
    #[default]
    Sqlite,
    Redb,
}

impl std::str::FromStr for DatabaseEngine {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sqlite" => Ok(DatabaseEngine::Sqlite),
            "redb" => Ok(DatabaseEngine::Redb),
            _ => Err(format!("Unknown database engine: {}", s)),
        }
    }
}

impl DatabaseEngine {
    /// Convert the database instance into a mint database
    pub async fn mint<P: Into<PathBuf>>(
        self,
        work_dir: P,
    ) -> Result<
        Arc<
            dyn cdk_common::database::MintDatabase<Err = cdk_common::database::Error>
                + Sync
                + Send
                + 'static,
        >,
        cdk_common::database::Error,
    > {
        match self {
            DatabaseEngine::Sqlite => {
                let sql_db_path = work_dir.into().join("cdk-mintd.sqlite");
                let db = MintSqliteDatabase::new(&sql_db_path).await?;
                db.migrate().await;
                Ok(Arc::new(db))
            }
            DatabaseEngine::Redb => {
                let redb_path = work_dir.into().join("cdk-mintd.redb");
                Ok(Arc::new(MintRedbDatabase::new(&redb_path)?))
            }
        }
    }
}
