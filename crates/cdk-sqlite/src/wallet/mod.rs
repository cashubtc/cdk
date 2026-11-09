//! SQLite Wallet Database

use cdk_sql_common::SQLWalletDatabase;

use crate::common::SqliteConnectionManager;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type WalletSqliteDatabase = SQLWalletDatabase<SqliteConnectionManager>;

#[cfg(test)]
mod tests {
    use cdk_common::wallet_db_test;

    use super::memory;

    async fn provide_db(_test_name: String) -> super::WalletSqliteDatabase {
        memory::empty().await.unwrap()
    }

    wallet_db_test!(provide_db);
    use std::str::FromStr;

    use cdk_common::database::WalletDatabase;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{ProofDleq, State};
    use cdk_common::secret::Secret;

    use crate::WalletSqliteDatabase;

    #[tokio::test]
    async fn transaction_points_and_metadata_roll_back_together() {
        use cdk_common::wallet::{Transaction, TransactionDirection, TransactionStatus};
        use cdk_common::{Amount, CurrencyUnit, SecretKey};

        let db = memory::empty().await.unwrap();
        let original = Transaction {
            mint_url: "https://mint.example".parse().unwrap(),
            direction: TransactionDirection::Incoming,
            amount: Amount::from(1),
            fee: Amount::ZERO,
            unit: CurrencyUnit::Sat,
            ys: vec![SecretKey::generate().public_key()],
            timestamp: 123,
            memo: Some("original".to_owned()),
            metadata: Default::default(),
            quote_id: None,
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(uuid::Uuid::new_v4()),
            status: TransactionStatus::Pending,
        };
        db.add_transaction(original.clone()).await.unwrap();
        let mut invalid = original.clone();
        invalid.memo = Some("must roll back".to_owned());
        // Insert a valid point before a G2 mint key, which is not a proof identifier.
        invalid.ys.push(SecretKey::generate_bls().public_key());
        assert!(db.add_transaction(invalid).await.is_err());
        let stored = db.get_transaction(original.id()).await.unwrap().unwrap();
        assert_eq!(stored.ys, original.ys);
        assert_eq!(stored.memo, original.memo);
    }

    #[test]
    fn transaction_ys_migration_preserves_legacy_points() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../../cdk-sql-common/src/wallet/migrations/sqlite/20250401120000_add_transactions_table.sql")).unwrap();
        let first = cdk_common::SecretKey::generate().public_key().to_bytes();
        let second = cdk_common::SecretKey::generate().public_key().to_bytes();
        let points = [first.clone(), second.clone(), first.clone()];
        for (id, ys) in [(vec![1u8], points.concat()), (vec![2u8], Vec::new())] {
            conn.execute("INSERT INTO transactions (id, mint_url, direction, amount, fee, unit, ys, timestamp, memo) VALUES (?1, 'https://mint.example', 'Incoming', 1, 0, 'sat', ?2, 123, 'retained')", rusqlite::params![id, ys]).unwrap();
        }
        conn.execute_batch(include_str!("../../../cdk-sql-common/src/wallet/migrations/sqlite/20260924000000_normalize_transaction_ys.sql")).unwrap();
        let mut stmt = conn
            .prepare("SELECT y FROM transaction_ys WHERE transaction_id = ?1 ORDER BY position")
            .unwrap();
        let stored = stmt
            .query_map([vec![1u8]], |row| row.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(stored, points);
        let empty_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM transaction_ys WHERE transaction_id = ?1",
                [vec![2u8]],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(empty_count, 0);
        let retained: i64 = conn
            .query_row(
                "SELECT count(*) FROM transactions WHERE memo = 'retained' AND timestamp = 123",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, 2);
        assert!(conn.prepare("SELECT ys FROM transactions").is_err());
    }

    #[test]
    fn transaction_ys_migration_rejects_truncated_points_atomically() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../../../cdk-sql-common/src/wallet/migrations/sqlite/20250401120000_add_transactions_table.sql")).unwrap();
        conn.execute("INSERT INTO transactions (id, mint_url, direction, amount, fee, unit, ys, timestamp) VALUES (?1, 'https://mint.example', 'Incoming', 1, 0, 'sat', ?2, 123)", rusqlite::params![vec![1u8], vec![2u8; 34]]).unwrap();
        let tx = conn.transaction().unwrap();
        assert!(tx.execute_batch(include_str!("../../../cdk-sql-common/src/wallet/migrations/sqlite/20260924000000_normalize_transaction_ys.sql")).is_err());
        tx.rollback().unwrap();
        let bytes: Vec<u8> = conn
            .query_row("SELECT ys FROM transactions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(bytes, vec![2u8; 34]);
        assert!(conn.prepare("SELECT * FROM transaction_ys").is_err());
    }

    #[tokio::test]
    #[cfg(feature = "sqlcipher")]
    async fn test_sqlcipher() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::MintInfo;

        use super::*;
        let path = std::env::temp_dir()
            .to_path_buf()
            .join(format!("cdk-test-{}.sqlite", uuid::Uuid::new_v4()));
        let db = WalletSqliteDatabase::new((path, "password".to_string()))
            .await
            .unwrap();

        let mint_info = MintInfo::new().description("test");
        let mint_url = MintUrl::from_str("https://mint.xyz").unwrap();

        db.add_mint(mint_url.clone(), Some(mint_info.clone()))
            .await
            .unwrap();

        let res = db.get_mint(mint_url).await.unwrap();
        assert_eq!(mint_info, res.clone().unwrap());
        assert_eq!("test", &res.unwrap().description.unwrap());
    }

    #[tokio::test]
    async fn test_proof_with_dleq() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, Id, Proof, PublicKey, SecretKey};
        use cdk_common::wallet::ProofInfo;
        use cdk_common::Amount;

        // Create a temporary database
        let path = std::env::temp_dir()
            .to_path_buf()
            .join(format!("cdk-test-dleq-{}.sqlite", uuid::Uuid::new_v4()));

        #[cfg(feature = "sqlcipher")]
        let db = WalletSqliteDatabase::new((path, "password".to_string()))
            .await
            .unwrap();

        #[cfg(not(feature = "sqlcipher"))]
        let db = WalletSqliteDatabase::new(path).await.unwrap();

        // Create a proof with DLEQ
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let secret = Secret::new("test_secret_for_dleq");

        // Create DLEQ components
        let e = SecretKey::generate();
        let s = SecretKey::generate();
        let r = SecretKey::generate();

        let dleq = ProofDleq::new(e.clone(), s.clone(), r.clone());

        let mut proof = Proof::new(
            Amount::from(64),
            keyset_id,
            secret,
            PublicKey::from_hex(
                "02deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            )
            .unwrap(),
        );

        // Add DLEQ to the proof
        proof.dleq = Some(dleq);

        // Create ProofInfo
        let proof_info =
            ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat).unwrap();

        // Store the proof in the database
        db.update_proofs(vec![proof_info.clone()], vec![])
            .await
            .unwrap();

        // Retrieve the proof from the database
        let retrieved_proofs = db
            .get_proofs(
                Some(mint_url),
                Some(CurrencyUnit::Sat),
                Some(vec![State::Unspent]),
                None,
            )
            .await
            .unwrap();

        // Verify we got back exactly one proof
        assert_eq!(retrieved_proofs.len(), 1);

        // Verify the DLEQ data was preserved
        let retrieved_proof = &retrieved_proofs[0];
        assert!(retrieved_proof.proof.dleq.is_some());

        let retrieved_dleq = retrieved_proof.proof.dleq.as_ref().unwrap();

        // Verify DLEQ components match what we stored
        assert_eq!(retrieved_dleq.e.to_secret_hex(), e.to_secret_hex());
        assert_eq!(retrieved_dleq.s.to_secret_hex(), s.to_secret_hex());
        assert_eq!(retrieved_dleq.r.to_secret_hex(), r.to_secret_hex());
    }

    #[tokio::test]
    async fn test_mint_quote_payment_method_read_and_write() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod, SecretKey};
        use cdk_common::wallet::MintQuote;
        use cdk_common::Amount;

        // Create a temporary database
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-migration-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = WalletSqliteDatabase::new((path, "password".to_string()))
            .await
            .unwrap();

        #[cfg(not(feature = "sqlcipher"))]
        let db = WalletSqliteDatabase::new(path).await.unwrap();

        // Test PaymentMethod variants
        let mint_url = MintUrl::from_str("https://example.com").unwrap();
        let quote_signing_key = SecretKey::generate();
        let payment_methods = [
            PaymentMethod::Known(KnownMethod::Bolt11),
            PaymentMethod::Known(KnownMethod::Bolt11),
            PaymentMethod::Custom("custom".to_string()),
        ];

        for (i, payment_method) in payment_methods.iter().enumerate() {
            let quote = MintQuote {
                id: format!("test_quote_{}", i),
                mint_url: mint_url.clone(),
                amount: Some(Amount::from(100)),
                unit: CurrencyUnit::Sat,
                request: "test_request".to_string(),
                state: MintQuoteState::Unpaid,
                expiry: 1000000000,
                secret_key: Some(quote_signing_key.clone()),
                payment_method: payment_method.clone(),
                amount_issued: Amount::from(0),
                amount_paid: Amount::from(0),
                updated_at: 0,
                estimated_blocks: None,
                used_by_operation: None,
                version: 0,
            };

            // Store the quote
            db.add_mint_quote(quote.clone()).await.unwrap();

            // Retrieve and verify
            let retrieved = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
            assert_eq!(retrieved.payment_method, *payment_method);
            assert_eq!(retrieved.secret_key, Some(quote_signing_key.clone()));
            assert_eq!(retrieved.amount_issued, Amount::from(0));
            assert_eq!(retrieved.amount_paid, Amount::from(0));
        }
    }

    #[tokio::test]
    async fn test_get_proofs_by_ys_empty_errors() {
        use cdk_common::database::Error;

        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-proofs-by-ys-empty-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = WalletSqliteDatabase::new((path, "password".to_string()))
            .await
            .unwrap();

        #[cfg(not(feature = "sqlcipher"))]
        let db = WalletSqliteDatabase::new(path).await.unwrap();

        let result = db.get_proofs_by_ys(vec![]).await;
        assert!(matches!(result, Err(Error::EmptyInClause(_))));
    }

    #[tokio::test]
    async fn test_get_proofs_by_ys() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, Id, Proof, SecretKey};
        use cdk_common::wallet::ProofInfo;
        use cdk_common::Amount;

        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-proofs-by-ys-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = WalletSqliteDatabase::new((path, "password".to_string()))
            .await
            .unwrap();

        #[cfg(not(feature = "sqlcipher"))]
        let db = WalletSqliteDatabase::new(path).await.unwrap();

        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let mint_url = MintUrl::from_str("https://example.com").unwrap();

        let mut proof_infos = vec![];
        let mut expected_ys = vec![];

        for _i in 0..5 {
            let secret = Secret::generate();
            let secret_key = SecretKey::generate();
            let c = secret_key.public_key();
            let proof = Proof::new(Amount::from(64), keyset_id, secret, c);
            let proof_info =
                ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat).unwrap();

            expected_ys.push(proof_info.y);
            proof_infos.push(proof_info);
        }

        db.update_proofs(proof_infos.clone(), vec![]).await.unwrap();

        // Retrieve all proofs by their Y values
        let retrieved_proofs = db.get_proofs_by_ys(expected_ys.clone()).await.unwrap();
        assert_eq!(retrieved_proofs.len(), 5);
        for retrieved_proof in &retrieved_proofs {
            assert!(expected_ys.contains(&retrieved_proof.y));
        }

        // Retrieve subset of proofs (first 3)
        let subset_ys = expected_ys[0..3].to_vec();
        let subset_proofs = db.get_proofs_by_ys(subset_ys.clone()).await.unwrap();
        assert_eq!(subset_proofs.len(), 3);
        for retrieved_proof in &subset_proofs {
            assert!(subset_ys.contains(&retrieved_proof.y));
        }

        // Retrieve with non-existent Y values returns only existing ones
        let non_existent_secret_key = SecretKey::generate();
        let non_existent_y = non_existent_secret_key.public_key();
        let mixed_ys = vec![expected_ys[0], non_existent_y, expected_ys[1]];
        let mixed_proofs = db.get_proofs_by_ys(mixed_ys).await.unwrap();
        assert_eq!(mixed_proofs.len(), 2);

        // Verify retrieved proof data matches original
        let single_y = vec![expected_ys[2]];
        let single_proof = db.get_proofs_by_ys(single_y).await.unwrap();
        assert_eq!(single_proof.len(), 1);
        assert_eq!(single_proof[0].y, proof_infos[2].y);
        assert_eq!(single_proof[0].proof.amount, proof_infos[2].proof.amount);
        assert_eq!(single_proof[0].mint_url, proof_infos[2].mint_url);
        assert_eq!(single_proof[0].state, proof_infos[2].state);
    }

    #[tokio::test]
    async fn test_get_unissued_mint_quotes() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
        use cdk_common::wallet::MintQuote;
        use cdk_common::Amount;

        // Create a temporary database
        let path = std::env::temp_dir().to_path_buf().join(format!(
            "cdk-test-unpaid-quotes-{}.sqlite",
            uuid::Uuid::new_v4()
        ));

        #[cfg(feature = "sqlcipher")]
        let db = WalletSqliteDatabase::new((path, "password".to_string()))
            .await
            .unwrap();

        #[cfg(not(feature = "sqlcipher"))]
        let db = WalletSqliteDatabase::new(path).await.unwrap();

        let mint_url = MintUrl::from_str("https://example.com").unwrap();

        // Quote 1: Fully paid and issued (should NOT be returned)
        let quote1 = MintQuote {
            id: "quote_fully_paid".to_string(),
            mint_url: mint_url.clone(),
            amount: Some(Amount::from(100)),
            unit: CurrencyUnit::Sat,
            request: "test_request_1".to_string(),
            state: MintQuoteState::Paid,
            expiry: 1000000000,
            secret_key: None,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_issued: Amount::from(100),
            amount_paid: Amount::from(100),
            updated_at: 0,
            estimated_blocks: None,
            used_by_operation: None,
            version: 0,
        };

        // Quote 2: Paid but not yet issued (should be returned - has pending balance)
        let quote2 = MintQuote {
            id: "quote_pending_balance".to_string(),
            mint_url: mint_url.clone(),
            amount: Some(Amount::from(100)),
            unit: CurrencyUnit::Sat,
            request: "test_request_2".to_string(),
            state: MintQuoteState::Paid,
            expiry: 1000000000,
            secret_key: None,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_issued: Amount::from(0),
            amount_paid: Amount::from(100),
            updated_at: 0,
            estimated_blocks: None,
            used_by_operation: None,
            version: 0,
        };

        // Quote 3: Bolt12 quote with no balance (should be returned - bolt12 is reusable)
        let quote3 = MintQuote {
            id: "quote_bolt12".to_string(),
            mint_url: mint_url.clone(),
            amount: Some(Amount::from(100)),
            unit: CurrencyUnit::Sat,
            request: "test_request_3".to_string(),
            state: MintQuoteState::Unpaid,
            expiry: 1000000000,
            secret_key: None,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt12),
            amount_issued: Amount::from(0),
            amount_paid: Amount::from(0),
            updated_at: 0,
            estimated_blocks: None,
            used_by_operation: None,
            version: 0,
        };

        // Quote 4: Unpaid bolt11 quote (should be returned - wallet needs to check with mint)
        let quote4 = MintQuote {
            id: "quote_unpaid".to_string(),
            mint_url: mint_url.clone(),
            amount: Some(Amount::from(100)),
            unit: CurrencyUnit::Sat,
            request: "test_request_4".to_string(),
            state: MintQuoteState::Unpaid,
            expiry: 1000000000,
            secret_key: None,
            payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
            amount_issued: Amount::from(0),
            amount_paid: Amount::from(0),
            updated_at: 0,
            estimated_blocks: None,
            used_by_operation: None,
            version: 0,
        };

        // Add all quotes to the database
        db.add_mint_quote(quote1).await.unwrap();
        db.add_mint_quote(quote2.clone()).await.unwrap();
        db.add_mint_quote(quote3.clone()).await.unwrap();
        db.add_mint_quote(quote4.clone()).await.unwrap();

        // Get unissued mint quotes
        let unissued_quotes = db.get_unissued_mint_quotes().await.unwrap();

        // Should return 3 quotes: quote2, quote3, and quote4
        // - quote2: bolt11 with amount_issued = 0 (needs minting)
        // - quote3: bolt12 (always returned, reusable)
        // - quote4: bolt11 with amount_issued = 0 (check with mint if paid)
        assert_eq!(unissued_quotes.len(), 3);

        // Verify the returned quotes are the expected ones
        let quote_ids: Vec<&str> = unissued_quotes.iter().map(|q| q.id.as_str()).collect();
        assert!(quote_ids.contains(&"quote_pending_balance"));
        assert!(quote_ids.contains(&"quote_bolt12"));
        assert!(quote_ids.contains(&"quote_unpaid"));

        // Verify that fully paid and issued quote is not returned
        assert!(!quote_ids.contains(&"quote_fully_paid"));
    }
}
