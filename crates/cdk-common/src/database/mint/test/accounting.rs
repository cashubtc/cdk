//! Accounting contracts shared by storage backends.

use std::collections::HashMap;
use std::str::FromStr;

use cashu::secret::Secret;
use cashu::{Amount, BlindSignature, Id, Proof, SecretKey, State};

use crate::database::mint::{Database, Error};
use crate::mint::Operation;

/// Committed totals include unapplied events, and rolled back operations never count.
pub async fn accounting_survives_flush_and_rollback<DB>(db: DB)
where
    DB: Database<Error> + Sync,
{
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let message = SecretKey::generate().public_key();
    let signature = BlindSignature {
        amount: Amount::from(1_u64 << 20),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };
    let proof = Proof {
        amount: Amount::from(128),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    };
    let operation = Operation::new_swap(signature.amount, proof.amount, Amount::from(1));
    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_blind_signatures(&[message], std::slice::from_ref(&signature), None)
        .await
        .unwrap();
    let mut proofs = tx
        .add_proofs(vec![proof.clone()], None, &operation)
        .await
        .unwrap();
    tx.update_proofs_state(&mut proofs, State::Spent)
        .await
        .unwrap();
    tx.add_completed_operation(&operation, &HashMap::from([(keyset_id, Amount::from(1))]))
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(db.get_total_issued().await.unwrap().is_empty());
    assert!(db.get_total_redeemed().await.unwrap().is_empty());
    assert_eq!(db.flush_keyset_accounting(1).await.unwrap(), 0);

    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_blind_signatures(&[message], std::slice::from_ref(&signature), None)
        .await
        .unwrap();
    let mut proofs = tx
        .add_proofs(vec![proof.clone()], None, &operation)
        .await
        .unwrap();
    tx.update_proofs_state(&mut proofs, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        db.get_total_issued().await.unwrap()[&keyset_id],
        signature.amount
    );
    assert_eq!(
        db.get_total_redeemed()
            .await
            .unwrap()
            .get(&keyset_id)
            .copied()
            .unwrap_or_default(),
        Amount::ZERO
    );

    let mut tx = db.begin_transaction().await.unwrap();
    let mut proofs = tx.get_proofs(&[proof.y().unwrap()]).await.unwrap();
    tx.update_proofs_state(&mut proofs, State::Spent)
        .await
        .unwrap();
    tx.add_completed_operation(&operation, &HashMap::from([(keyset_id, Amount::from(1))]))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        db.get_total_redeemed().await.unwrap()[&keyset_id],
        proof.amount
    );
    while db.flush_keyset_accounting(1).await.unwrap() > 0 {
        assert_eq!(
            db.get_total_issued().await.unwrap()[&keyset_id],
            signature.amount
        );
        assert_eq!(
            db.get_total_redeemed().await.unwrap()[&keyset_id],
            proof.amount
        );
    }
    assert_eq!(
        db.get_total_issued().await.unwrap()[&keyset_id],
        signature.amount
    );
    assert_eq!(
        db.get_total_redeemed().await.unwrap()[&keyset_id],
        proof.amount
    );
    assert_eq!(db.flush_keyset_accounting(10).await.unwrap(), 0);
}
