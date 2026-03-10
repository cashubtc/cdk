//! Payments

use std::ops::Deref;
use std::str::FromStr;

use cashu::nut00::KnownMethod;
use cashu::quote_id::QuoteId;
use cashu::{Amount, BlindSignature, CurrencyUnit, Id, SecretKey};

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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote).await.unwrap();

    let p1 = unique_string();
    let p2 = unique_string();

    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            p1.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();

    assert_eq!(mint_quote.amount_paid().value(), 100);

    mint_quote
        .add_payment(
            Amount::from(250).with_unit(CurrencyUnit::Sat),
            p2.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();

    assert_eq!(mint_quote.amount_paid().value(), 350);

    tx.commit().await.unwrap();

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_quote_from_db");
    assert_eq!(mint_quote_from_db.amount_paid().value(), 350);
    assert_eq!(
        mint_quote_from_db
            .payments
            .iter()
            .map(|x| (x.payment_id.clone(), x.amount.clone()))
            .collect::<Vec<_>>(),
        vec![
            (p1, Amount::from(100).with_unit(CurrencyUnit::Sat)),
            (p2, Amount::from(250).with_unit(CurrencyUnit::Sat))
        ]
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let p1 = unique_string();
    let p2 = unique_string();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            p1.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();

    assert_eq!(mint_quote.amount_paid().value(), 100);

    mint_quote
        .add_payment(
            Amount::from(250).with_unit(CurrencyUnit::Sat),
            p2.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 350);
    tx.commit().await.unwrap();

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_quote_from_db");
    assert_eq!(mint_quote_from_db.amount_paid().value(), 350);
    assert_eq!(
        mint_quote_from_db
            .payments
            .iter()
            .map(|x| (x.payment_id.clone(), x.amount.clone()))
            .collect::<Vec<_>>(),
        vec![
            (p1, Amount::from(100).with_unit(CurrencyUnit::Sat)),
            (p2, Amount::from(250).with_unit(CurrencyUnit::Sat))
        ]
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mint_quote_from_tx = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_quote_from_tx");
    assert_eq!(mint_quote_from_db, mint_quote_from_tx.deref().to_owned());
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let p1 = unique_string();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            p1.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();

    // Duplicate payment should fail
    assert!(mint_quote
        .add_payment(Amount::from(100).with_unit(CurrencyUnit::Sat), p1, None)
        .is_err());
    tx.commit().await.unwrap();

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_from_db");
    assert_eq!(mint_quote_from_db.amount_paid(), mint_quote.amount_paid());
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            p1.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("no error")
        .expect("quote");
    // Duplicate payment should fail
    assert!(mint_quote
        .add_payment(Amount::from(100).with_unit(CurrencyUnit::Sat), p1, None)
        .is_err());
    tx.commit().await.unwrap(); // although in theory nothing has changed, let's try it out

    let mint_quote_from_db = db
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("mint_from_db");
    assert_eq!(mint_quote_from_db.amount_paid(), mint_quote.amount_paid());
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    // Trying to issue without any payment should fail (over-issue)
    assert!(mint_quote
        .add_issuance(Amount::from(100).with_unit(CurrencyUnit::Sat))
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("no error")
        .expect("quote");
    // Trying to issue without any payment should fail (over-issue)
    assert!(mint_quote
        .add_issuance(Amount::from(100).with_unit(CurrencyUnit::Sat))
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let p1 = unique_string();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            p1.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    // Trying to issue more than paid should fail (over-issue)
    assert!(mint_quote
        .add_issuance(Amount::from(101).with_unit(CurrencyUnit::Sat))
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
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        vec![],
        vec![],
        None,
    );

    let p1 = unique_string();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx.add_mint_quote(mint_quote).await.unwrap();
    let quote_id = mint_quote.id.clone();
    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            p1.clone(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&quote_id)
        .await
        .expect("no error")
        .expect("quote");
    // Trying to issue more than paid should fail (over-issue)
    assert!(mint_quote
        .add_issuance(Amount::from(101).with_unit(CurrencyUnit::Sat))
        .is_err());
}
/// Successful melt with unique blinded messages
pub async fn add_melt_request_unique_blinded_messages<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let inputs_amount = Amount::new(100, CurrencyUnit::Sat);
    let inputs_fee = Amount::new(1, CurrencyUnit::Sat);
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
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11));
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_melt_request(&quote.id, inputs_amount.clone(), inputs_fee.clone())
        .await
        .unwrap();
    tx.add_blinded_messages(
        Some(&quote.id),
        &blinded_messages,
        &Operation::new_melt(
            Amount::ZERO,
            Amount::ZERO,
            cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        ),
    )
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
    let inputs_amount = Amount::new(100, CurrencyUnit::Sat);
    let inputs_fee = Amount::new(1, CurrencyUnit::Sat);
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
    let quote2 = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11));
    tx.add_melt_quote(quote2.clone()).await.unwrap();
    tx.add_melt_request(&quote2.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    let result = tx
        .add_blinded_messages(
            Some(&quote2.id),
            &blinded_messages,
            &Operation::new_melt(
                Amount::ZERO,
                Amount::ZERO,
                cashu::PaymentMethod::Known(KnownMethod::Bolt11),
            ),
        )
        .await;
    assert!(result.is_err() && matches!(result.unwrap_err(), Error::Duplicate));
    tx.rollback().await.unwrap(); // Rollback to avoid partial state
}

/// Reject duplicate blinded message insert via DB constraint (different quotes)
pub async fn reject_duplicate_blinded_message_db_constraint<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let inputs_amount = Amount::new(100, CurrencyUnit::Sat);
    let inputs_fee = Amount::new(1, CurrencyUnit::Sat);
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
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11));
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_melt_request(&quote.id, inputs_amount.clone(), inputs_fee.clone())
        .await
        .unwrap();
    assert!(tx
        .add_blinded_messages(
            Some(&quote.id),
            &blinded_messages,
            &Operation::new_melt(
                Amount::ZERO,
                Amount::ZERO,
                cashu::PaymentMethod::Known(KnownMethod::Bolt11)
            )
        )
        .await
        .is_ok());
    tx.commit().await.unwrap();

    // Second insert with same blinded_message but different quote_id should fail due to unique constraint on blinded_message
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11));
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.add_melt_request(&quote.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    let result = tx
        .add_blinded_messages(
            Some(&quote.id),
            &blinded_messages,
            &Operation::new_melt(
                Amount::ZERO,
                Amount::ZERO,
                cashu::PaymentMethod::Known(KnownMethod::Bolt11),
            ),
        )
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
    let inputs_amount = Amount::new(100, CurrencyUnit::Sat);
    let inputs_fee = Amount::new(1, CurrencyUnit::Sat);
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
    let quote = MeltQuote::new(MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11));
    tx1.add_melt_quote(quote.clone()).await.unwrap();
    tx1.add_melt_request(&quote.id, inputs_amount, inputs_fee)
        .await
        .unwrap();
    tx1.add_blinded_messages(
        Some(&quote.id),
        &blinded_messages,
        &Operation::new_melt(
            Amount::ZERO,
            Amount::ZERO,
            cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        ),
    )
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

/// Test adding and retrieving melt quotes
pub async fn add_and_get_melt_quote<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        None,
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Add melt quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_melt_quote(melt_quote.clone()).await.is_ok());
    tx.commit().await.unwrap();

    // Retrieve melt quote
    let retrieved = db.get_melt_quote(&melt_quote.id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, melt_quote.id);
    assert_eq!(retrieved.amount(), melt_quote.amount());
    assert_eq!(retrieved.fee_reserve(), melt_quote.fee_reserve());
}

/// Test adding duplicate melt quotes fails
pub async fn add_melt_quote_only_once<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        None,
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Add first melt quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_melt_quote(melt_quote.clone()).await.is_ok());
    tx.commit().await.unwrap();

    // Try to add duplicate - should fail
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_melt_quote(melt_quote).await.is_err());
    tx.rollback().await.unwrap();
}

/// Test updating melt quote state
pub async fn update_melt_quote_state_transition<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::MeltQuoteState;

    let melt_quote = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        None,
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Add melt quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(melt_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Update to Pending state
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut quote = tx.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
    let old_state = tx
        .update_melt_quote_state(&mut quote, MeltQuoteState::Pending, None)
        .await
        .unwrap();
    assert_eq!(old_state, MeltQuoteState::Unpaid);
    assert_eq!(quote.state, MeltQuoteState::Pending);
    tx.commit().await.unwrap();

    // Update to Paid state with payment proof
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut quote = tx.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
    let payment_proof = "payment_proof_123".to_string();
    let old_state = tx
        .update_melt_quote_state(
            &mut quote,
            MeltQuoteState::Paid,
            Some(payment_proof.clone()),
        )
        .await
        .unwrap();
    assert_eq!(old_state, MeltQuoteState::Pending);
    assert_eq!(quote.state, MeltQuoteState::Paid);
    // The payment proof is stored in the melt quote (verification depends on implementation)
    tx.commit().await.unwrap();
}

/// Test updating melt quote request lookup id
pub async fn update_melt_quote_request_lookup_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        Some(PaymentIdentifier::CustomId("old_lookup_id".to_string())),
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Add melt quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(melt_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Update request lookup id
    let new_lookup_id = PaymentIdentifier::CustomId("new_lookup_id".to_string());
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut quote = tx.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
    tx.update_melt_quote_request_lookup_id(&mut quote, &new_lookup_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Verify the update
    let retrieved = db.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.request_lookup_id, Some(new_lookup_id));
}

/// Test getting all mint quotes
pub async fn get_all_mint_quotes<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let quote1 = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    let quote2 = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(200, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quotes
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(quote1.clone()).await.unwrap();
    tx.add_mint_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get all quotes
    let all_quotes = db.get_mint_quotes().await.unwrap();
    assert!(all_quotes.len() >= 2);
    assert!(all_quotes.iter().any(|q| q.id == quote1.id));
    assert!(all_quotes.iter().any(|q| q.id == quote2.id));
}

/// Test getting all melt quotes
pub async fn get_all_melt_quotes<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let quote1 = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        None,
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    let quote2 = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(200, cashu::CurrencyUnit::Sat),
        Amount::new(20, cashu::CurrencyUnit::Sat),
        0,
        None,
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Add quotes
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote1.clone()).await.unwrap();
    tx.add_melt_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get all quotes
    let all_quotes = db.get_melt_quotes().await.unwrap();
    assert!(all_quotes.len() >= 2);
    assert!(all_quotes.iter().any(|q| q.id == quote1.id));
    assert!(all_quotes.iter().any(|q| q.id == quote2.id));
}

/// Test getting mint quote by request
pub async fn get_mint_quote_by_request<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let request = unique_string();
    let mint_quote = MintQuote::new(
        None,
        request.clone(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get by request
    let retrieved = db.get_mint_quote_by_request(&request).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, mint_quote.id);
    assert_eq!(retrieved.request, request);
}

/// Test getting mint quote by request lookup id
pub async fn get_mint_quote_by_request_lookup_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let lookup_id = PaymentIdentifier::CustomId(unique_string());
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        lookup_id.clone(),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get by request lookup id
    let retrieved = db
        .get_mint_quote_by_request_lookup_id(&lookup_id)
        .await
        .unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, mint_quote.id);
    assert_eq!(retrieved.request_lookup_id, lookup_id);
}

/// Test deleting blinded messages
pub async fn delete_blinded_messages<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();

    // Create blinded messages
    let blinded_secret1 = SecretKey::generate().public_key();
    let blinded_secret2 = SecretKey::generate().public_key();

    let blinded_message1 = cashu::BlindedMessage {
        blinded_secret: blinded_secret1,
        keyset_id,
        amount: Amount::from(100u64),
        witness: None,
    };

    let blinded_message2 = cashu::BlindedMessage {
        blinded_secret: blinded_secret2,
        keyset_id,
        amount: Amount::from(200u64),
        witness: None,
    };

    let blinded_messages = vec![blinded_message1.clone(), blinded_message2.clone()];

    // Add blinded messages
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blinded_messages(
        None,
        &blinded_messages,
        &Operation::new_mint(
            Amount::ZERO,
            cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        ),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Delete one blinded message
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.delete_blinded_messages(&[blinded_secret1])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Try to add same blinded messages again - first should succeed, second should fail
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx
        .add_blinded_messages(
            None,
            &[blinded_message1],
            &Operation::new_mint(
                Amount::ZERO,
                cashu::PaymentMethod::Known(KnownMethod::Bolt11)
            )
        )
        .await
        .is_ok());
    assert!(tx
        .add_blinded_messages(
            None,
            &[blinded_message2],
            &Operation::new_mint(
                Amount::ZERO,
                cashu::PaymentMethod::Known(KnownMethod::Bolt11)
            )
        )
        .await
        .is_err());
    tx.rollback().await.unwrap();
}

/// Test incrementing mint quote amount paid
pub async fn increment_mint_quote_amount_paid<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mint_quote = tx.add_mint_quote(mint_quote).await.unwrap();
    tx.commit().await.unwrap();

    // Add payment first time
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote
        .add_payment(
            Amount::from(300).with_unit(CurrencyUnit::Sat),
            "payment_1".to_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 300);
    tx.commit().await.unwrap();

    // Add payment second time
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote
        .add_payment(
            Amount::from(200).with_unit(CurrencyUnit::Sat),
            "payment_2".to_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 500);
    tx.commit().await.unwrap();

    // Verify final state
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_paid().value(), 500);
}

/// Test incrementing mint quote amount issued
pub async fn increment_mint_quote_amount_issued<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // First add payment to allow issuing
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote
        .add_payment(
            Amount::from(1000).with_unit(CurrencyUnit::Sat),
            "payment_1".to_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    tx.commit().await.unwrap();

    // Add issuance first time
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote
        .add_issuance(Amount::from(400).with_unit(CurrencyUnit::Sat))
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_issued().value(), 400);
    tx.commit().await.unwrap();

    // Add issuance second time
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote
        .add_issuance(Amount::from(300).with_unit(CurrencyUnit::Sat))
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_issued().value(), 700);
    tx.commit().await.unwrap();

    // Verify final state
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_issued().value(), 700);
}

/// Test getting mint quote within transaction (with lock)
pub async fn get_mint_quote_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let mint_quote = MintQuote::new(
        None,
        "test_request".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get quote within transaction
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_mint_quote(&mint_quote.id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, mint_quote.id);
    assert_eq!(retrieved.request, "test_request");
    tx.commit().await.unwrap();
}

/// Test getting melt quote within transaction (with lock)
pub async fn get_melt_quote_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        None,
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(melt_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get quote within transaction
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_melt_quote(&melt_quote.id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, melt_quote.id);
    assert_eq!(retrieved.amount(), melt_quote.amount());
    tx.commit().await.unwrap();
}

/// Test get mint quote by request within transaction
pub async fn get_mint_quote_by_request_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let request = unique_string();
    let mint_quote = MintQuote::new(
        None,
        request.clone(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get by request within transaction
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_mint_quote_by_request(&request).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, mint_quote.id);
    assert_eq!(retrieved.request, request);
    tx.commit().await.unwrap();
}

/// Test get mint quote by request lookup id within transaction
pub async fn get_mint_quote_by_request_lookup_id_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let lookup_id = PaymentIdentifier::CustomId(unique_string());
    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        lookup_id.clone(),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get by request lookup id within transaction
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx
        .get_mint_quote_by_request_lookup_id(&lookup_id)
        .await
        .unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.id, mint_quote.id);
    assert_eq!(retrieved.request_lookup_id, lookup_id);
    tx.commit().await.unwrap();
}

/// Test getting blind signatures within transaction
pub async fn get_blind_signatures_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use std::str::FromStr;

    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
    let blinded_message = SecretKey::generate().public_key();

    let sig = BlindSignature {
        amount: Amount::from(100u64),
        keyset_id,
        c: SecretKey::generate().public_key(),
        dleq: None,
    };

    // Add blind signature
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blind_signatures(&[blinded_message], std::slice::from_ref(&sig), None)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Get blind signature within transaction
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx.get_blind_signatures(&[blinded_message]).await.unwrap();
    assert_eq!(retrieved.len(), 1);
    assert!(retrieved[0].is_some());
    let retrieved_sig = retrieved[0].as_ref().unwrap();
    assert_eq!(retrieved_sig.amount, sig.amount);
    assert_eq!(retrieved_sig.c, sig.c);
    tx.commit().await.unwrap();
}

/// Test that duplicate payment IDs are rejected
pub async fn reject_duplicate_payment_ids<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // First payment with payment_id "payment_1"
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote
        .add_payment(
            Amount::from(300).with_unit(CurrencyUnit::Sat),
            "payment_1".to_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 300);
    tx.commit().await.unwrap();

    // Try to add the same payment_id again - should fail with DuplicatePaymentId error
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");

    let result = mint_quote.add_payment(
        Amount::from(300).with_unit(CurrencyUnit::Sat),
        "payment_1".to_string(),
        None,
    );

    assert!(
        matches!(result.unwrap_err(), crate::Error::DuplicatePaymentId),
        "Duplicate payment_id should be rejected"
    );
    tx.rollback().await.unwrap();

    // Verify that the amount_paid is still 300 (not 600)
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_paid().value(), 300);

    // A different payment_id should succeed
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");

    mint_quote
        .add_payment(
            Amount::from(200).with_unit(CurrencyUnit::Sat),
            "payment_2".to_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();

    assert_eq!(mint_quote.amount_paid().value(), 500);
    tx.commit().await.unwrap();

    // Verify final state
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_paid().value(), 500);
}

/// Test that loading the quote first allows modifications
pub async fn modify_mint_quote_after_loading_succeeds<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let mint_quote = MintQuote::new(
        None,
        "".to_owned(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Now load the quote first, then modify it
    let mut tx = Database::begin_transaction(&db).await.unwrap();

    // First load the quote (this should lock it)
    let mut loaded_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .unwrap()
        .expect("quote should exist");

    // Now modification should succeed
    loaded_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            unique_string(),
            None,
        )
        .unwrap();
    let result = tx.update_mint_quote(&mut loaded_quote).await;

    assert!(
        result.is_ok(),
        "Modifying after loading should succeed, got: {:?}",
        result.err()
    );

    tx.commit().await.unwrap();

    // Verify the modification was persisted
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_paid().value(), 100);
}

/// Test getting multiple mint quotes by IDs
pub async fn get_mint_quotes_by_ids<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use crate::database::mint::test::unique_string;

    let quote1 = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    let quote2 = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(200, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
        vec![],
        vec![],
        None,
    );

    // Add quotes
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_mint_quote(quote1.clone()).await.unwrap();
    tx.add_mint_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // 1. Test getting both quotes
    let ids = vec![quote1.id.clone(), quote2.id.clone()];
    let quotes = db.get_mint_quotes_by_ids(&ids).await.unwrap();
    assert_eq!(quotes.len(), 2);
    assert!(quotes[0].is_some());
    assert!(quotes[1].is_some());
    assert_eq!(quotes[0].as_ref().unwrap().id, quote1.id);
    assert_eq!(quotes[1].as_ref().unwrap().id, quote2.id);

    // 2. Test getting with missing ID
    let missing_id = QuoteId::new_uuid();
    let ids = vec![quote1.id.clone(), missing_id, quote2.id.clone()];
    let quotes = db.get_mint_quotes_by_ids(&ids).await.unwrap();
    assert_eq!(quotes.len(), 3);
    assert!(quotes[0].is_some());
    assert!(quotes[1].is_none());
    assert!(quotes[2].is_some());
    assert_eq!(quotes[0].as_ref().unwrap().id, quote1.id);
    assert_eq!(quotes[2].as_ref().unwrap().id, quote2.id);

    // 3. Test empty list
    let quotes = db.get_mint_quotes_by_ids(&[]).await.unwrap();
    assert!(quotes.is_empty());

    // 4. Test within transaction (with locking)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let ids = vec![quote2.id.clone(), quote1.id.clone()]; // Reverse order
    let quotes = tx.get_mint_quotes_by_ids(&ids).await.unwrap();
    assert_eq!(quotes.len(), 2);
    assert_eq!(quotes[0].as_ref().unwrap().id, quote2.id);
    assert_eq!(quotes[1].as_ref().unwrap().id, quote1.id);
    tx.commit().await.unwrap();
}
