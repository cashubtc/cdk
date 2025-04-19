//! Macro with default tests
//!
//! This set is generic and checks the default and expected behaviour for a mint database
//! implementation
use std::fmt::Debug;
use std::str::FromStr;

use cashu::secret::Secret;
use cashu::{Amount, CurrencyUnit, SecretKey};

use super::*;
use crate::mint::MintKeySetInfo;

#[inline]
async fn setup_keyset<E: Debug, DB: Database<E> + KeysDatabase<Err = E>>(db: &DB) -> Id {
    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let keyset_info = MintKeySetInfo {
        id: keyset_id,
        unit: CurrencyUnit::Sat,
        active: true,
        valid_from: 0,
        valid_to: None,
        derivation_path: bitcoin::bip32::DerivationPath::from_str("m/0'/0'/0'").unwrap(),
        derivation_path_index: Some(0),
        max_order: 32,
        input_fee_ppk: 0,
    };
    db.add_keyset_info(keyset_info).await.unwrap();
    keyset_id
}

/// State transition test
pub async fn state_transition<E: Debug, DB: Database<E> + KeysDatabase<Err = E>>(db: DB) {
    let keyset_id = setup_keyset(&db).await;

    let proofs = vec![
        Proof {
            amount: Amount::from(100),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
        Proof {
            amount: Amount::from(200),
            keyset_id,
            secret: Secret::generate(),
            c: SecretKey::generate().public_key(),
            witness: None,
            dleq: None,
        },
    ];

    // Add proofs to database
    db.add_proofs(proofs.clone(), None).await.unwrap();

    // Mark one proof as `pending`
    assert!(db
        .update_proofs_states(&[proofs[0].y().unwrap()], State::Pending)
        .await
        .is_ok());

    // Attempt to select the `pending` proof, as `pending` again (which should fail)
    assert!(db
        .update_proofs_states(&[proofs[0].y().unwrap()], State::Pending)
        .await
        .is_err());
}

/// Unit test that is expected to be passed for a correct database implementation
#[macro_export]
macro_rules! mint_db_test {
    ($make_db_fn:ident) => {
        mint_db_test!(state_transition, $make_db_fn);
    };
    ($name:ident, $make_db_fn:ident) => {
        #[tokio::test]
        async fn $name() {
            cdk_common::database::mint::test::$name($make_db_fn().await).await;
        }
    };
}
