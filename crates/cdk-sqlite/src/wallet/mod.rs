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

    use cdk_sql_common::database::ConnectionWithTransaction;
    use cdk_sql_common::migrate;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;
    use cdk_sql_common::value::Value;
    use cdk_sql_common::wallet::migrations::MIGRATIONS;

    use crate::common::SqliteConnectionManager;
    use crate::WalletSqliteDatabase;

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
        db.add_mint(mint_url.clone(), None).await.unwrap();
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
        db.add_mint(mint_url.clone(), None).await.unwrap();
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
        db.add_mint(mint_url.clone(), None).await.unwrap();

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
        db.add_mint(mint_url.clone(), None).await.unwrap();

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

    /// Checks the `mint_url` to `mint_id` conversion over real rows.
    ///
    /// An empty Rust registry stops the runner exactly at the schema before the migration under
    /// test, so the seed below needs no checked-in dump. It covers the two cases the migration
    /// exists for: a row whose mint was never added through `add_mint`, and the nullable
    /// `melt_quote.mint_url`.
    #[tokio::test]
    async fn migrates_mint_url_to_mint_id() {
        let path = std::env::temp_dir()
            .join(format!("cdk-test-mint-id-{}.sqlite", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();

        let known = "https://known.example.com/";
        let orphan = "https://orphan.example.com/";

        {
            let pool = Pool::<SqliteConnectionManager>::new(path.as_str().into());
            let conn = pool.get().await.expect("connection");
            let tx = ConnectionWithTransaction::new(conn).await.expect("transaction");

            migrate(&tx, "sqlite", MIGRATIONS, Vec::new())
                .await
                .expect("pre-migration schema");

            for statement in [
                format!("INSERT INTO mint (mint_url) VALUES ('{known}')"),
                format!(
                    "INSERT INTO keyset (id, mint_url, unit, active) \
                     VALUES ('ks', '{known}', 'sat', 1)"
                ),
                format!(
                    "INSERT INTO proof (y, mint_url, state, unit, amount, keyset_id, secret, c) \
                     VALUES (X'01', '{orphan}', 'UNSPENT', 'sat', 1, 'ks', 's', X'02')"
                ),
                format!(
                    "INSERT INTO mint_quote (id, mint_url, unit, request, state, expiry) \
                     VALUES ('mq', '{known}', 'sat', 'r', 'UNPAID', 0)"
                ),
                "INSERT INTO melt_quote (id, unit, amount, request, fee_reserve, expiry, mint_url) \
                 VALUES ('no_mint', 'sat', 1, 'r', 0, 0, NULL)"
                    .to_owned(),
                format!(
                    "INSERT INTO transactions \
                     (id, mint_url, direction, amount, fee, unit, ys, timestamp) \
                     VALUES (X'03', '{orphan}', 'Incoming', 1, 0, 'sat', X'04', 0)"
                ),
                format!(
                    "INSERT INTO wallet_sagas \
                     (id, kind, state, amount, mint_url, unit, created_at, updated_at, data) \
                     VALUES ('sg', 'send', 'st', 1, '{known}', 'sat', 0, 0, '{{}}')"
                ),
            ] {
                query(&statement)
                    .expect("statement")
                    .batch(&tx)
                    .await
                    .expect("seed");
            }

            tx.commit().await.expect("commit");
        }

        let db = WalletSqliteDatabase::new(path.as_str())
            .await
            .expect("migration applies");

        let pool = Pool::<SqliteConnectionManager>::new(path.as_str().into());
        let conn = pool.get().await.expect("connection");

        let url_for = |table: &str, id: &str| {
            format!(
                "SELECT m.mint_url FROM {table} t JOIN mint m ON m.id = t.mint_id WHERE t.id = {id}"
            )
        };

        for (sql, expected) in [
            (url_for("keyset", "'ks'"), known),
            (url_for("mint_quote", "'mq'"), known),
            (url_for("wallet_sagas", "'sg'"), known),
            (url_for("transactions", "X'03'"), orphan),
            (
                "SELECT m.mint_url FROM proof p JOIN mint m ON m.id = p.mint_id".to_owned(),
                orphan,
            ),
        ] {
            let found = query(&sql)
                .expect("statement")
                .pluck(&*conn)
                .await
                .expect("query")
                .unwrap_or_else(|| panic!("row lost by the migration: {sql}"));

            assert_eq!(found, Value::Text(expected.to_owned()), "{sql}");
        }

        let mints = query("SELECT COUNT(*) FROM mint")
            .expect("statement")
            .pluck(&*conn)
            .await
            .expect("query");
        assert_eq!(mints, Some(Value::Integer(2)));

        let melt = query("SELECT mint_id FROM melt_quote WHERE id = 'no_mint'")
            .expect("statement")
            .pluck(&*conn)
            .await
            .expect("query");
        assert_eq!(melt, Some(Value::Null));

        drop(db);
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }
}
