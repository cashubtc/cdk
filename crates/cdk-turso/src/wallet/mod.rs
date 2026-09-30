//! Wallet storage using the native Turso engine.

use std::path::Path;

use cdk_common::database::Error;
use cdk_sql_common::SQLWalletDatabase;

use crate::{Config, TursoConnectionManager};

/// Wallet database backed by Turso.
pub type WalletTursoDatabase = SQLWalletDatabase<TursoConnectionManager>;

/// Open and migrate a local wallet database, or `:memory:`.
pub async fn open<P>(path: P) -> Result<WalletTursoDatabase, Error>
where
    P: AsRef<Path>,
{
    WalletTursoDatabase::new(Config::new(path).await?).await
}

#[cfg(test)]
mod tests {
    use cdk_common::wallet_db_test;

    async fn provide_db(_test_name: String) -> super::WalletTursoDatabase {
        super::open(":memory:").await.expect("wallet database")
    }

    wallet_db_test!(provide_db);

    #[tokio::test]
    async fn persists_across_reopen() {
        use cdk_common::database::WalletDatabase;
        use cdk_common::mint_url::MintUrl;
        use cdk_common::MintInfo;

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("wallet.db");
        let url: MintUrl = "https://example.com".parse().expect("mint URL");
        let info = MintInfo::new().description("persistent wallet");
        {
            let db = super::open(&path).await.expect("open");
            db.add_mint(url.clone(), Some(info.clone()))
                .await
                .expect("add mint");
        }
        let db = super::open(&path).await.expect("reopen");
        assert_eq!(db.get_mint(url).await.expect("get mint"), Some(info));
    }
}
