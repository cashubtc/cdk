//! SQLite Wallet Database

use cdk_sql_common::SQLWalletDatabase;

use crate::common::SqliteConnectionManager;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type WalletSqliteDatabase = SQLWalletDatabase<SqliteConnectionManager>;

#[cfg(test)]
mod tests {
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

        db.add_mint(mint_url.clone(), Some(mint_info.clone()))
            .await
            .unwrap();

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
            db.add_mint_quote(quote.clone()).await.unwrap();

            // Retrieve and verify
            let retrieved = db.get_mint_quote(&quote.id).await.unwrap().unwrap();
            assert_eq!(retrieved.payment_method, *payment_method);
            assert_eq!(retrieved.amount_issued, Amount::from(0));
            assert_eq!(retrieved.amount_paid, Amount::from(0));
        }
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
        db.update_proofs(proof_infos.clone(), vec![]).await.unwrap();

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
}
