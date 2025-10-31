//! Payments

use std::str::FromStr;

use cashu::quote_id::QuoteId;
use cashu::{Amount, Id, SecretKey};

use crate::database::mint::test::unique_string;
use crate::database::mint::{Database, Error, KeysDatabase};
use crate::database::MintSignaturesDatabase;
use crate::mint::{MeltPaymentRequest, MeltQuote, MintQuote, Operation};
use crate::payment::PaymentIdentifier;

/// Add a mint quote
pub async fn add_mint_quote<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_mint_quote(mint_quote.clone()).await.is_ok());
    tx.commit().await.unwrap();
}

/// Dup mint quotes fails
pub async fn add_mint_quote_only_once<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_mint_quote(mint_quote.clone()).await.is_ok());
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_mint_quote(mint_quote).await.is_err());
    tx.commit().await.unwrap();
}

/// Register payments
pub async fn register_payments<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_mint_quote(mint_quote.clone()).await.is_ok());

    let p1 = unique_string();
    let p2 = unique_string();

    let new_paid_amount = tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1.clone())
        .await
        .unwrap();

    assert_eq!(new_paid_amount, 100.into());

    let new_paid_amount = tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 250.into(), p2.clone())
        .await
        .unwrap();

    assert_eq!(new_paid_amount, 350.into());

    tx.commit().await.unwrap();

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_quote_from_db");
    assert_eq!(mint_quote_from_db.amount_paid(), 350.into());
    assert_eq!(
        mint_quote_from_db
            .payments
            .iter()
            .map(|x| (x.payment_id.clone(), x.amount))
            .collect::<Vec<_>>(),
        vec![(p1, 100.into()), (p2, 250.into())]
    );
}

/// Read mint and payments from db and tx objects
pub async fn read_mint_from_db_and_tx<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let p1 = unique_string();
    let p2 = unique_string();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    let new_paid_amount = tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1.clone())
        .await
        .unwrap();

    assert_eq!(new_paid_amount, 100.into());

    let new_paid_amount = tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 250.into(), p2.clone())
        .await
        .unwrap();
    assert_eq!(new_paid_amount, 350.into());
    tx.commit().await.unwrap();

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_quote_from_db");
    assert_eq!(mint_quote_from_db.amount_paid(), 350.into());
    assert_eq!(
        mint_quote_from_db
            .payments
            .iter()
            .map(|x| (x.payment_id.clone(), x.amount))
            .collect::<Vec<_>>(),
        vec![(p1, 100.into()), (p2, 250.into())]
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mint_quote_from_tx = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_quote_from_tx");
    assert_eq!(mint_quote_from_db, mint_quote_from_tx);
}

/// Reject duplicate payments in the same txs
pub async fn reject_duplicate_payments_same_tx<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let p1 = unique_string();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    let amount_paid = tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1.clone())
        .await
        .unwrap();

    assert!(tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1)
        .await
        .is_err());
    tx.commit().await.unwrap();

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_from_db");
    assert_eq!(mint_quote_from_db.amount_paid(), amount_paid);
    assert_eq!(mint_quote_from_db.payments.len(), 1);
}

/// Reject duplicate payments in different txs
pub async fn reject_duplicate_payments_diff_tx<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let p1 = unique_string();

    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    let amount_paid = tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx
        .increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1)
        .await
        .is_err());
    tx.commit().await.unwrap(); // although in theory nothing has changed, let's try it out

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_from_db");
    assert_eq!(mint_quote_from_db.amount_paid(), amount_paid);
    assert_eq!(mint_quote_from_db.payments.len(), 1);
}

/// Reject over issue in same tx
pub async fn reject_over_issue_same_tx<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    assert!(tx
        .increment_mint_quote_amount_issued(&mint_quote.id, 100.into())
        .await
        .is_err());
}

/// Reject over issue
pub async fn reject_over_issue_different_tx<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx
        .increment_mint_quote_amount_issued(&mint_quote.id, 100.into())
        .await
        .is_err());
}

/// Reject over issue with payment
pub async fn reject_over_issue_with_payment<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let p1 = unique_string();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1.clone())
        .await
        .unwrap();
    assert!(tx
        .increment_mint_quote_amount_issued(&mint_quote.id, 101.into())
        .await
        .is_err());
}

/// Reject over issue with payment
pub async fn reject_over_issue_with_payment_different_tx<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        0.into(),
        0.into(),
        cashu::PaymentMethod::Bolt12,
        0,
        vec![],
        vec![],
    );

    let p1 = unique_string();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.increment_mint_quote_amount_paid(&mint_quote.id, 100.into(), p1.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx
        .increment_mint_quote_amount_issued(&mint_quote.id, 101.into())
        .await
        .is_err());
}
/// Successful melt with unique blinded messages
pub async fn add_melt_request_unique_blinded_messages<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let inputs_amount = Amount::from(100u64);
    let inputs_fee = Amount::from(1u64);
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

    // Create a dummy blinded message
    let blinded_secret = SecretKey::generate().public_key();
    let blinded_message = cashu::BlindedMessage {
        blinded_secret,
        keyset_id,
        amount: Amount::from(100u64),
        witness: None,
    };
    let blinded_messages = vec![blinded_message];

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, 33.into(), Amount::ZERO, 0, None, None, cashu::PaymentMethod::Bolt11);
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_melt_request(&quote.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    tx.add_blinded_messages(Some(&quote.id), &blinded_messages, &Operation::new_melt())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify retrieval
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx
        .get_melt_request_and_blinded_messages(&quote.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.inputs_amount, inputs_amount);
    assert_eq!(retrieved.inputs_fee, inputs_fee);
    assert_eq!(retrieved.change_outputs.len(), 1);
    assert_eq!(retrieved.change_outputs[0].amount, Amount::from(100u64));
    tx.commit().await.unwrap();
}

/// Reject melt with duplicate blinded message (already signed)
pub async fn reject_melt_duplicate_blinded_signature<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let quote_id1 = QuoteId::new_uuid();
    let inputs_amount = Amount::from(100u64);
    let inputs_fee = Amount::from(1u64);
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

    // Create a dummy blinded message
    let blinded_secret = SecretKey::generate().public_key();
    let blinded_message = cashu::BlindedMessage {
        blinded_secret,
        keyset_id,
        amount: Amount::from(100u64),
        witness: None,
    };
    let blinded_messages = vec![blinded_message.clone()];

    // First, "sign" it by adding to blind_signature (simulate successful mint)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let c = SecretKey::generate().public_key();
    let blind_sig = cashu::BlindSignature {
        amount: Amount::from(100u64),
        keyset_id,
        c,
        dleq: None,
    };
    let blinded_secrets = vec![blinded_message.blinded_secret];
    tx.add_blind_signatures(&blinded_secrets, &[blind_sig], Some(quote_id1))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Now try to add melt request with the same blinded message - should fail due to constraint
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let quote2 = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, 33.into(), Amount::ZERO, 0, None, None, cashu::PaymentMethod::Bolt11);
    tx.add_melt_quote(quote2.clone()).await.unwrap();
    tx.add_melt_request(&quote2.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    let result = tx
        .add_blinded_messages(Some(&quote2.id), &blinded_messages, &Operation::new_melt())
        .await;
    assert!(result.is_err() && matches!(result.unwrap_err(), Error::Duplicate));
    tx.rollback().await.unwrap(); // Rollback to avoid partial state
}

/// Reject duplicate blinded message insert via DB constraint (different quotes)
pub async fn reject_duplicate_blinded_message_db_constraint<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let inputs_amount = Amount::from(100u64);
    let inputs_fee = Amount::from(1u64);
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

    // Create a dummy blinded message
    let blinded_secret = SecretKey::generate().public_key();
    let blinded_message = cashu::BlindedMessage {
        blinded_secret,
        keyset_id,
        amount: Amount::from(100u64),
        witness: None,
    };
    let blinded_messages = vec![blinded_message];

    // First insert succeeds
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, 33.into(), Amount::ZERO, 0, None, None, cashu::PaymentMethod::Bolt11);
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_melt_request(&quote.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    assert!(tx
        .add_blinded_messages(Some(&quote.id), &blinded_messages, &Operation::new_melt())
        .await
        .is_ok());
    tx.commit().await.unwrap();

    // Second insert with same blinded_message but different quote_id should fail due to unique constraint on blinded_message
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, 33.into(), Amount::ZERO, 0, None, None, cashu::PaymentMethod::Bolt11);
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_melt_request(&quote.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    let result = tx
        .add_blinded_messages(Some(&quote.id), &blinded_messages, &Operation::new_melt())
        .await;
    // Expect a database error due to unique violation
    assert!(result.is_err()); // Specific error might be DB-specific, e.g., SqliteError or PostgresError
    tx.rollback().await.unwrap();
}

/// Cleanup of melt request after processing
pub async fn cleanup_melt_request_after_processing<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let inputs_amount = Amount::from(100u64);
    let inputs_fee = Amount::from(1u64);
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

    // Create dummy blinded message
    let blinded_secret = SecretKey::generate().public_key();
    let blinded_message = cashu::BlindedMessage {
        blinded_secret,
        keyset_id,
        amount: Amount::from(100u64),
        witness: None,
    };
    let blinded_messages = vec![blinded_message];

    // Insert melt request
    let mut tx1 = Database::begin_transaction(&db).await.unwrap();
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, 33.into(), Amount::ZERO, 0, None, None, cashu::PaymentMethod::Bolt11);
    tx1.add_melt_quote(quote.clone()).await.unwrap();
    tx1.add_melt_request(&quote.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    tx1.add_blinded_messages(Some(&quote.id), &blinded_messages, &Operation::new_melt())
        .await
        .unwrap();
    tx1.commit().await.unwrap();

    // Simulate processing: get and delete
    let mut tx2 = Database::begin_transaction(&db).await.unwrap();
    let _retrieved = tx2
        .get_melt_request_and_blinded_messages(&quote.id)
        .await
        .unwrap()
        .unwrap();
    tx2.delete_melt_request(&quote.id).await.unwrap();
    tx2.commit().await.unwrap();

    // Verify melt_request is deleted
    let mut tx3 = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx3
        .get_melt_request_and_blinded_messages(&quote.id)
        .await
        .unwrap();
    assert!(retrieved.is_none());
    tx3.commit().await.unwrap();
}
