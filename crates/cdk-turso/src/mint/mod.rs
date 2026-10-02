//! Mint and authentication storage using the native Turso engine.

use std::path::Path;

use cdk_common::database::Error;
use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::SQLMintDatabase;

use crate::{Config, TursoConnectionManager};

/// Mint database backed by Turso.
pub type MintTursoDatabase = SQLMintDatabase<TursoConnectionManager>;

/// Mint authentication database backed by Turso.
pub type MintTursoAuthDatabase = SQLMintAuthDatabase<TursoConnectionManager>;

/// Open and migrate a local mint database, or `:memory:`.
pub async fn open<P>(path: P) -> Result<MintTursoDatabase, Error>
where
    P: AsRef<Path>,
{
    MintTursoDatabase::new(Config::new(path).await?).await
}

/// Open and migrate a separate mint authentication database, or `:memory:`.
pub async fn open_auth<P>(path: P) -> Result<MintTursoAuthDatabase, Error>
where
    P: AsRef<Path>,
{
    MintTursoAuthDatabase::new(Config::new(path).await?).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::database::{self, MintAuthDatabase};
    use cdk_common::secret::Secret;
    use cdk_common::{mint_db_test, AuthProof, Id, SecretKey, State};

    async fn provide_db(_test_name: String) -> super::MintTursoDatabase {
        super::open(":memory:").await.expect("mint database")
    }

    mint_db_test!(provide_db);

    #[tokio::test]
    async fn kvstore_compare_and_swap() {
        database::mint::test::kvstore_compare_and_swap(provide_db("kv".to_owned()).await).await;
    }

    #[tokio::test]
    async fn concurrent_configuration_updates_have_one_winner() {
        use cdk_common::database::KVStoreCompareAndSwap;

        let db = Arc::new(provide_db("concurrent".to_owned()).await);
        let (first, second) = tokio::join!(
            db.kv_compare_and_swap("settings", "mint", "name", None, b"first"),
            db.kv_compare_and_swap("settings", "mint", "name", None, b"second"),
        );
        assert_ne!(first.expect("first update"), second.expect("second update"));
    }

    #[tokio::test]
    async fn persists_across_reopen() {
        use cdk_common::database::{KVStore, KVStoreDatabase};

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("mint.db");
        {
            let db = super::open(&path).await.expect("open");
            let mut tx = KVStore::begin_transaction(&db).await.expect("transaction");
            tx.kv_write("settings", "mint", "name", b"persistent mint")
                .await
                .expect("write");
            tx.commit().await.expect("commit");
        }
        let db = super::open(&path).await.expect("reopen");
        assert_eq!(
            db.kv_read("settings", "mint", "name").await.expect("read"),
            Some(b"persistent mint".to_vec())
        );
    }

    async fn add_auth_proof(
        db: Arc<super::MintTursoAuthDatabase>,
        proof: AuthProof,
    ) -> Result<(), database::Error> {
        let mut tx = db.begin_transaction().await?;
        tx.add_proof(proof).await?;
        tx.commit().await
    }

    #[tokio::test]
    async fn auth_proof_is_atomic_and_persistent() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("auth.db");
        let proof = AuthProof {
            keyset_id: "00916bbf7ef91a36".parse::<Id>().expect("keyset id"),
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            dleq: None,
        };
        let y = proof.y().expect("proof Y");
        {
            let db = Arc::new(super::open_auth(&path).await.expect("auth database"));
            let (first, second) = tokio::join!(
                add_auth_proof(db.clone(), proof.clone()),
                add_auth_proof(db, proof)
            );
            assert!(matches!(
                (&first, &second),
                (Ok(()), Err(database::Error::Duplicate))
                    | (Err(database::Error::Duplicate), Ok(()))
            ));
        }
        let db = super::open_auth(&path).await.expect("reopen");
        assert_eq!(
            db.get_proofs_states(&[y]).await.expect("proof state"),
            vec![Some(State::Spent)]
        );
    }
}
