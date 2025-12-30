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
    use cdk_common::nuts::{ProofDleq, State};
    use cdk_common::secret::Secret;

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

        let mut tx = db.begin_db_transaction().await.expect("tx");

        tx.add_mint(mint_url.clone(), Some(mint_info.clone()))
            .await
            .unwrap();

        tx.commit().await.expect("commit");

        let res = db.get_mint(mint_url).await.unwrap();
        assert_eq!(mint_info, res.clone().unwrap());
        assert_eq!("test", &res.unwrap().description.unwrap());
    }

    #[tokio::test]
    async fn test_proof_with_dleq() {
        use cdk_common::common::ProofInfo;
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, Id, Proof, PublicKey, SecretKey};
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

        let mut tx = db.begin_db_transaction().await.expect("tx");

        // Store the proof in the database
        tx.update_proofs(vec![proof_info.clone()], vec![])
            .await
            .unwrap();

        tx.commit().await.expect("commit");

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
        assert_eq!(retrieved_dleq.e.to_string(), e.to_string());
        assert_eq!(retrieved_dleq.s.to_string(), s.to_string());
        assert_eq!(retrieved_dleq.r.to_string(), r.to_string());
    }

    #[tokio::test]
    async fn test_mint_quote_payment_method_read_and_write() {
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
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
        let payment_methods = [
            PaymentMethod::Bolt11,
            PaymentMethod::Bolt12,
            PaymentMethod::Custom("custom".to_string()),
        ];

        let mut tx = db.begin_db_transaction().await.expect("begin");

        for (i, payment_method) in payment_methods.iter().enumerate() {
            let quote = MintQuote {
                id: format!("test_quote_{}", i),
                mint_url: mint_url.clone(),
                amount: Some(Amount::from(100)),
                unit: CurrencyUnit::Sat,
                request: "test_request".to_string(),
                state: MintQuoteState::Unpaid,
                expiry: 1000000000,
                secret_key: None,
                payment_method: payment_method.clone(),
                amount_issued: Amount::from(0),
                amount_paid: Amount::from(0),
            };

            // Store the quote
            tx.add_mint_quote(quote.clone()).await.unwrap();

            // Retrieve and verify
            let retrieved = tx.get_mint_quote(&quote.id).await.unwrap().unwrap();
            assert_eq!(retrieved.payment_method, *payment_method);
            assert_eq!(retrieved.amount_issued, Amount::from(0));
            assert_eq!(retrieved.amount_paid, Amount::from(0));
        }
        tx.commit().await.expect("commit");
    }

    #[tokio::test]
    async fn test_get_proofs_by_ys() {
        use cdk_common::common::ProofInfo;
        use cdk_common::mint_url::MintUrl;
        use cdk_common::nuts::{CurrencyUnit, Id, Proof, SecretKey};
        use cdk_common::Amount;

        // Create a temporary database
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

        // Create multiple proofs
        let keyset_id = Id::from_str("00deadbeef123456").unwrap();
        let mint_url = MintUrl::from_str("https://example.com").unwrap();

        let mut proof_infos = vec![];
        let mut expected_ys = vec![];

        // Generate valid public keys using SecretKey
        for _i in 0..5 {
            let secret = Secret::generate();

            // Generate a valid public key from a secret key
            let secret_key = SecretKey::generate();
            let c = secret_key.public_key();

            let proof = Proof::new(Amount::from(64), keyset_id, secret, c);

            let proof_info =
                ProofInfo::new(proof, mint_url.clone(), State::Unspent, CurrencyUnit::Sat).unwrap();

            expected_ys.push(proof_info.y);
            proof_infos.push(proof_info);
        }

        // Store all proofs in the database
        let mut tx = db.begin_db_transaction().await.unwrap();
        tx.update_proofs(proof_infos.clone(), vec![]).await.unwrap();
        tx.commit().await.unwrap();

        // Test 1: Retrieve all proofs by their Y values
        let retrieved_proofs = db.get_proofs_by_ys(expected_ys.clone()).await.unwrap();

        assert_eq!(retrieved_proofs.len(), 5);
        for retrieved_proof in &retrieved_proofs {
            assert!(expected_ys.contains(&retrieved_proof.y));
        }

        // Test 2: Retrieve subset of proofs (first 3)
        let subset_ys = expected_ys[0..3].to_vec();
        let subset_proofs = db.get_proofs_by_ys(subset_ys.clone()).await.unwrap();

        assert_eq!(subset_proofs.len(), 3);
        for retrieved_proof in &subset_proofs {
            assert!(subset_ys.contains(&retrieved_proof.y));
        }

        // Test 3: Retrieve with non-existent Y values
        let non_existent_secret_key = SecretKey::generate();
        let non_existent_y = non_existent_secret_key.public_key();
        let mixed_ys = vec![expected_ys[0], non_existent_y, expected_ys[1]];
        let mixed_proofs = db.get_proofs_by_ys(mixed_ys).await.unwrap();

        // Should only return the 2 that exist
        assert_eq!(mixed_proofs.len(), 2);

        // Test 4: Empty input returns empty result
        let empty_result = db.get_proofs_by_ys(vec![]).await.unwrap();
        assert_eq!(empty_result.len(), 0);

        // Test 5: Verify retrieved proof data matches original
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
            payment_method: PaymentMethod::Bolt11,
            amount_issued: Amount::from(100),
            amount_paid: Amount::from(100),
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
            payment_method: PaymentMethod::Bolt11,
            amount_issued: Amount::from(0),
            amount_paid: Amount::from(100),
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
            payment_method: PaymentMethod::Bolt12,
            amount_issued: Amount::from(0),
            amount_paid: Amount::from(0),
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
            payment_method: PaymentMethod::Bolt11,
            amount_issued: Amount::from(0),
            amount_paid: Amount::from(0),
        };

        {
            let mut tx = db.begin_db_transaction().await.unwrap();

            // Add all quotes to the database
            tx.add_mint_quote(quote1).await.unwrap();
            tx.add_mint_quote(quote2.clone()).await.unwrap();
            tx.add_mint_quote(quote3.clone()).await.unwrap();
            tx.add_mint_quote(quote4.clone()).await.unwrap();

            tx.commit().await.unwrap();
        }

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
