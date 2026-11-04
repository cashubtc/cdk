//! Payments

use std::cmp::Reverse;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cashu::nut00::KnownMethod;
use cashu::quote_id::QuoteId;
use cashu::{Amount, BlindSignature, CurrencyUnit, Id, SecretKey};

use crate::database::mint::{Acquired, Database, DynMintDatabase, Error, KeysDatabase};
use crate::database::MintSignaturesDatabase;
use crate::mint::{MeltPaymentRequest, MeltQuote, MintQuote, Operation};
use crate::payment::PaymentIdentifier;

fn unique_string() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Add a mint quote
pub async fn add_mint_quote<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
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

    // Use reverse key order to verify physical lock ordering does not change
    // the logical order returned to the client.
    let mut blinded_messages = vec![
        cashu::BlindedMessage {
            blinded_secret: SecretKey::generate().public_key(),
            keyset_id,
            amount: Amount::from(100u64),
            witness: None,
        },
        cashu::BlindedMessage {
            blinded_secret: SecretKey::generate().public_key(),
            keyset_id,
            amount: Amount::from(200u64),
            witness: None,
        },
    ];
    blinded_messages.sort_unstable_by_key(|message| Reverse(message.blinded_secret.to_bytes()));

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let quote = MeltQuote::new(None,MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11), None, None);
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
    assert_eq!(retrieved.change_outputs.len(), 2);
    assert_eq!(retrieved.change_outputs, blinded_messages);
    tx.commit().await.unwrap();
}

/// Reject melt with duplicate blinded message (already signed)
pub async fn reject_melt_duplicate_blinded_signature<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error> + MintSignaturesDatabase<Err = Error>,
{
    let quote_id1 = QuoteId::new();
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
    let quote2 = MeltQuote::new(None,MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat),Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11), None, None);
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
    let quote = MeltQuote::new(None,MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11), None, None);
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
    let quote = MeltQuote::new(None,MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11), None, None);
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

/// Payment details of a completed melt survive reading the operation back
pub async fn completed_melt_operation_keeps_payment_details<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mut melt = Operation::new_melt(
        Amount::from(1_000u64),
        Amount::from(2u64),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );
    melt.set_payment_details(Amount::from(990u64), Amount::from(8u64));
    let swap = Operation::new_swap(Amount::from(10u64), Amount::from(10u64), Amount::ZERO);

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_completed_operation(&melt, &std::collections::HashMap::new())
        .await
        .unwrap();
    tx.add_completed_operation(&swap, &std::collections::HashMap::new())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let by_id = db
        .get_completed_operation(melt.id())
        .await
        .unwrap()
        .unwrap();
    let by_kind = db
        .get_completed_operations_by_kind(melt.kind())
        .await
        .unwrap()
        .into_iter()
        .find(|operation| operation.id() == melt.id())
        .unwrap();
    let all = db.get_completed_operations().await.unwrap();
    let from_all = all
        .iter()
        .find(|operation| operation.id() == melt.id())
        .unwrap();

    for operation in [&by_id, &by_kind, from_all] {
        assert_eq!(operation.payment_amount(), Some(Amount::from(990u64)));
        assert_eq!(operation.payment_fee(), Some(Amount::from(8u64)));
    }

    // Operations without payment details still read back without them
    let stored_swap = all
        .iter()
        .find(|operation| operation.id() == swap.id())
        .unwrap();
    assert_eq!(stored_swap.payment_amount(), None);
    assert_eq!(stored_swap.payment_fee(), None);
}

/// Reusing a blinded message that is stored but not signed yet reports it as
/// pending, not as a duplicate of an already-signed output
pub async fn reject_pending_blinded_message_as_pending<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let blinded_messages = vec![cashu::BlindedMessage {
        blinded_secret: SecretKey::generate().public_key(),
        keyset_id: Id::from_str("001711afb1de20cb").unwrap(),
        amount: Amount::from(100u64),
        witness: None,
    }];
    let operation = Operation::new_mint(
        Amount::ZERO,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );

    // Reserve the output without signing it (c stays NULL)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blinded_messages(None, &blinded_messages, &operation)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx
        .add_blinded_messages(None, &blinded_messages, &operation)
        .await;
    assert!(
        matches!(result, Err(Error::BlindedMessagePending)),
        "unexpected result: {result:?}"
    );
    tx.rollback().await.unwrap();
}

/// A request that reuses both a signed and a pending output reports the
/// signed one, even when the pending output is reached first
pub async fn reject_signed_and_pending_blinded_messages_as_signed<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("001711afb1de20cb").unwrap();
    let operation = Operation::new_mint(
        Amount::ZERO,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
    );
    let blinded_message = || cashu::BlindedMessage {
        blinded_secret: SecretKey::generate().public_key(),
        keyset_id,
        amount: Amount::from(100u64),
        witness: None,
    };

    // Outputs are stored in blinded-secret order. Make the pending output
    // sort first so its conflict is the one hit first; an implementation that
    // only looks at the first conflict would then report it as pending.
    let (mut pending, mut signed) = (blinded_message(), blinded_message());
    if pending.blinded_secret.to_bytes() > signed.blinded_secret.to_bytes() {
        std::mem::swap(&mut pending, &mut signed);
    }

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_blinded_messages(None, &[pending.clone(), signed.clone()], &operation)
        .await
        .unwrap();
    tx.add_blind_signatures(
        &[signed.blinded_secret],
        &[cashu::BlindSignature {
            amount: Amount::from(100u64),
            keyset_id,
            c: SecretKey::generate().public_key(),
            dleq: None,
        }],
        None,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let result = tx
        .add_blinded_messages(None, &[pending, signed], &operation)
        .await;
    assert!(
        matches!(result, Err(Error::Duplicate)),
        "unexpected result: {result:?}"
    );
    tx.rollback().await.unwrap();
}

/// A request that reuses an output while another transaction is still
/// reserving it waits for that transaction, then reports the output as
/// pending rather than already signed.
///
/// `second_insert_waiting` must resolve only once the second request is
/// blocked on the first transaction's uncommitted reservation, so the first
/// transaction commits while the second one is waiting.
pub async fn concurrent_reservation_of_output_reports_pending<F>(
    db: DynMintDatabase,
    second_insert_waiting: F,
) where
    F: std::future::Future<Output = ()>,
{
    let blinded_messages = vec![cashu::BlindedMessage {
        blinded_secret: SecretKey::generate().public_key(),
        keyset_id: Id::from_str("001711afb1de20cb").unwrap(),
        amount: Amount::from(100u64),
        witness: None,
    }];
    let operation = || {
        Operation::new_mint(
            Amount::ZERO,
            cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        )
    };

    let mut first = db.begin_transaction().await.unwrap();
    first
        .add_blinded_messages(None, &blinded_messages, &operation())
        .await
        .unwrap();

    let second_messages = blinded_messages.clone();
    let second = tokio::spawn(async move {
        let mut tx = db.begin_transaction().await?;
        let result = tx
            .add_blinded_messages(None, &second_messages, &operation())
            .await;
        tx.rollback().await?;
        Ok::<_, Error>(result)
    });

    second_insert_waiting.await;
    assert!(
        !second.is_finished(),
        "second request should wait for the reservation"
    );
    first.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("second request should finish once the reservation commits")
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(Error::BlindedMessagePending)),
        "unexpected result: {result:?}"
    );
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
    let quote = MeltQuote::new(None,MeltPaymentRequest::Bolt11 { bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap() }, cashu::CurrencyUnit::Sat, Amount::new(33, cashu::CurrencyUnit::Sat), Amount::new(0, cashu::CurrencyUnit::Sat), 0, None, None, cashu::PaymentMethod::Known(KnownMethod::Bolt11), None, None);
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
        None,
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
        Some(serde_json::json!({"processor": "metadata"})),
        None    );

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
    assert_eq!(retrieved.extra_json, melt_quote.extra_json);
}

/// Test adding duplicate melt quotes fails
pub async fn add_melt_quote_only_once<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        None,
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
        None,
        None    );

    // Add first melt quote
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_melt_quote(melt_quote.clone()).await.is_ok());
    tx.commit().await.unwrap();

    // Try to add duplicate - should fail
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.add_melt_quote(melt_quote).await.is_err());
    tx.rollback().await.unwrap();
}

/// An unpaid melt quote, already stored in the database.
async fn stored_melt_quote<DB>(db: &DB) -> MeltQuote
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        None,
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        Some(PaymentIdentifier::CustomId(unique_string())),
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        None,
        None,
    );

    let mut tx = Database::begin_transaction(db).await.unwrap();
    tx.add_melt_quote(melt_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    melt_quote
}

/// Moves a stored melt quote to `new_state` in a transaction of its own.
async fn move_melt_quote<DB>(db: &DB, quote_id: &QuoteId, new_state: cashu::MeltQuoteState)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mut tx = Database::begin_transaction(db).await.unwrap();
    let mut quote = tx.get_melt_quote(quote_id).await.unwrap().unwrap();
    tx.update_melt_quote_state(&mut quote, new_state, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// Loads a melt quote and ends the transaction, so the copy outlives its row lock.
async fn stale_melt_quote<DB>(db: &DB, quote_id: &QuoteId) -> Acquired<MeltQuote>
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mut tx = Database::begin_transaction(db).await.unwrap();
    let quote = tx.get_melt_quote(quote_id).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    quote
}

/// Test updating melt quote state
pub async fn update_melt_quote_state_transition<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::MeltQuoteState;

    let melt_quote = MeltQuote::new(
        None,
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
        Some(serde_json::json!({"state": "init"})),
        None,
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
    assert_eq!(quote.extra_json, Some(serde_json::json!({"state": "init"})));
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

/// A melt quote kept past its transaction can't move a paid quote back to unpaid.
pub async fn update_melt_quote_state_refuses_stale_state<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::MeltQuoteState;

    let quote_id = stored_melt_quote(&db).await.id;
    move_melt_quote(&db, &quote_id, MeltQuoteState::Pending).await;
    let mut stale = stale_melt_quote(&db, &quote_id).await;
    move_melt_quote(&db, &quote_id, MeltQuoteState::Paid).await;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx
        .update_melt_quote_state(&mut stale, MeltQuoteState::Unpaid, None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let stored = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
    assert_eq!(stored.state, MeltQuoteState::Paid);
}

/// A melt quote kept past its transaction can't mark a quote paid after it was rolled back.
pub async fn update_melt_quote_state_to_paid_refuses_stale_state<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::MeltQuoteState;

    let quote_id = stored_melt_quote(&db).await.id;
    move_melt_quote(&db, &quote_id, MeltQuoteState::Pending).await;
    let mut stale = stale_melt_quote(&db, &quote_id).await;
    move_melt_quote(&db, &quote_id, MeltQuoteState::Unpaid).await;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx
        .update_melt_quote_state(
            &mut stale,
            MeltQuoteState::Paid,
            Some("payment_proof".to_string()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let stored = db.get_melt_quote(&quote_id).await.unwrap().unwrap();
    assert_eq!(stored.state, MeltQuoteState::Unpaid);
    assert_eq!(stored.paid_time, None);
    assert_eq!(stored.payment_proof, None);
}

/// Test updating melt quote request lookup id
pub async fn update_melt_quote_request_lookup_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        None,
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
        Some(serde_json::json!({"lookup": "test"})),
        None,
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
    assert_eq!(
        retrieved.extra_json,
        Some(serde_json::json!({"lookup": "test"}))
    );
}

/// A melt quote kept past its transaction can't change the lookup id of a quote that moved on.
pub async fn update_melt_quote_request_lookup_id_refuses_stale_state<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = stored_melt_quote(&db).await;
    let mut stale = stale_melt_quote(&db, &melt_quote.id).await;
    move_melt_quote(&db, &melt_quote.id, cashu::MeltQuoteState::Pending).await;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx
        .update_melt_quote_request_lookup_id(
            &mut stale,
            &PaymentIdentifier::CustomId(unique_string()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let stored = db.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
    assert_eq!(stored.request_lookup_id, melt_quote.request_lookup_id);
}

/// Test getting all mint quotes
pub async fn get_all_mint_quotes<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
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
        None,
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
        Some(serde_json::json!({"processor": "metadata-1"})),
        None    );

    let quote2 = MeltQuote::new(
        None,
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
        Some(serde_json::json!({"processor": "metadata-2"})),
        None    );

    // Add quotes
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote1.clone()).await.unwrap();
    tx.add_melt_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    // Get all quotes
    let all_quotes = db.get_melt_quotes().await.unwrap();
    assert!(all_quotes.len() >= 2);

    let retrieved_quote1 = all_quotes.iter().find(|q| q.id == quote1.id).unwrap();
    let retrieved_quote2 = all_quotes.iter().find(|q| q.id == quote2.id).unwrap();

    assert_eq!(retrieved_quote1.extra_json, quote1.extra_json);
    assert_eq!(retrieved_quote1.payment_method, quote1.payment_method);
    assert_eq!(retrieved_quote2.extra_json, quote2.extra_json);
    assert_eq!(retrieved_quote2.payment_method, quote2.payment_method);
}

/// Test melt execution lock storage and release semantics
pub async fn melt_quote_execution_lock<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    use cashu::MeltQuoteState;

    let new_melt_quote = || {
        MeltQuote::new(
            None,
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
            None,
            None,
        )
    };

    async fn arm_lock<DB>(db: &DB, quote: &MeltQuote, token: &str)
    where
        DB: Database<Error> + KeysDatabase<Err = Error>,
    {
        let mut tx = Database::begin_transaction(db).await.unwrap();
        let mut stored = tx.get_melt_quote(&quote.id).await.unwrap().unwrap();
        stored.melt_lock = token.to_string();
        stored.melt_lock_expires_at = tx.melt_lease_time().await.unwrap() + 60;
        tx.update_melt_quote_state(&mut stored, MeltQuoteState::Pending, None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    // A fresh quote is unlocked.
    let quote = new_melt_quote();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let retrieved = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(!retrieved.is_locked());
    assert_eq!(retrieved.melt_lock, "");

    // Arming persists the lock token.
    arm_lock(&db, &quote, "token-a").await;
    let retrieved = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(retrieved.is_locked());
    assert_eq!(retrieved.melt_lock, "token-a");

    // A non-matching token cannot release the lock.
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(!tx.unlock_melt_quote(&quote.id, "token-b").await.unwrap());
    tx.commit().await.unwrap();
    let retrieved = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.melt_lock, "token-a");

    // The token holder releases the lock; a second release is a no-op.
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.unlock_melt_quote(&quote.id, "token-a").await.unwrap());
    assert!(!tx.unlock_melt_quote(&quote.id, "token-a").await.unwrap());
    tx.commit().await.unwrap();
    let retrieved = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(!retrieved.is_locked());

    // Force unlock releases a lock without knowing the token.
    let quote = new_melt_quote();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();
    arm_lock(&db, &quote, "token-c").await;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx.force_unlock_melt_quote(&quote.id).await.unwrap());
    assert!(!tx.force_unlock_melt_quote(&quote.id).await.unwrap());
    tx.commit().await.unwrap();
    let retrieved = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(!retrieved.is_locked());

    // Only an unexpired owner can renew. Releasing or replacing it fences it.
    let quote = new_melt_quote();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();
    arm_lock(&db, &quote, "old-owner").await;
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx
        .renew_melt_quote_lease(&quote.id, "old-owner", 60)
        .await
        .unwrap());
    assert!(!tx
        .renew_melt_quote_lease(&quote.id, "other-owner", 60)
        .await
        .unwrap());
    assert!(tx
        .renew_melt_quote_lease(&quote.id, "old-owner", 0)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(!tx
        .renew_melt_quote_lease(&quote.id, "old-owner", 60)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(tx
        .claim_melt_quote_lease(&quote.id, "new-owner", 60)
        .await
        .unwrap()
        .is_some());
    assert!(tx
        .claim_melt_quote_lease(&quote.id, "another-owner", 60)
        .await
        .unwrap()
        .is_none());
    tx.commit().await.unwrap();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    assert!(!tx.unlock_melt_quote(&quote.id, "old-owner").await.unwrap());
    assert!(!tx
        .renew_melt_quote_lease(&quote.id, "old-owner", 60)
        .await
        .unwrap());
    assert!(tx.unlock_melt_quote(&quote.id, "new-owner").await.unwrap());
    tx.commit().await.unwrap();
    let retrieved = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(!retrieved.is_locked());
    assert_eq!(retrieved.melt_lock_expires_at, 0);

    // Concurrent recovery claimants must have exactly one winner.
    let claim = |token: &'static str| async {
        let mut tx = Database::begin_transaction(&db).await.unwrap();
        let won = tx
            .claim_melt_quote_lease(&quote.id, token, 60)
            .await
            .unwrap()
            .is_some();
        tx.commit().await.unwrap();
        won
    };
    let (first, second) = tokio::join!(claim("first"), claim("second"));
    assert_ne!(first, second);
}

/// Test getting mint quote by request
pub async fn get_mint_quote_by_request<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
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
    let lookup_id = PaymentIdentifier::CustomId(unique_string());
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        lookup_id.clone(),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
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
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
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
            Some(100),
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 300);
    let first_updated_at = mint_quote.updated_at();
    assert!(first_updated_at > 0);
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
            Some(100),
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 500);
    let second_updated_at = mint_quote.updated_at();
    assert!(second_updated_at > first_updated_at);
    tx.commit().await.unwrap();

    // Verify final state
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_paid().value(), 500);
    assert_eq!(retrieved.updated_at(), second_updated_at);

    // Even if the in-memory quote carries a stale timestamp, persistence must
    // bump from the database value instead of regressing.
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut mint_quote = tx
        .get_mint_quote(&mint_quote.id)
        .await
        .expect("valid quote")
        .expect("valid result");
    mint_quote.set_updated_at(0);
    mint_quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            "payment_3".to_string(),
            Some(100),
        )
        .unwrap();
    tx.update_mint_quote(&mut mint_quote).await.unwrap();
    assert_eq!(mint_quote.amount_paid().value(), 600);
    let stale_update_updated_at = mint_quote.updated_at();
    assert!(stale_update_updated_at > second_updated_at);
    tx.commit().await.unwrap();

    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_paid().value(), 600);
    assert_eq!(retrieved.updated_at(), stale_update_updated_at);
}

/// Test incrementing mint quote amount issued
pub async fn increment_mint_quote_amount_issued<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
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
    let paid_updated_at = mint_quote.updated_at();
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
    let first_issuance_updated_at = mint_quote.updated_at();
    assert!(first_issuance_updated_at > paid_updated_at);
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
    let second_issuance_updated_at = mint_quote.updated_at();
    assert!(second_issuance_updated_at > first_issuance_updated_at);
    tx.commit().await.unwrap();

    // Verify final state
    let retrieved = db.get_mint_quote(&mint_quote.id).await.unwrap().unwrap();
    assert_eq!(retrieved.amount_issued().value(), 700);
    assert_eq!(retrieved.updated_at(), second_issuance_updated_at);
}

/// Test getting mint quote within transaction (with lock)
pub async fn get_mint_quote_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
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
    assert_eq!(retrieved.request, mint_quote.request);
    tx.commit().await.unwrap();
}

/// Test getting melt quote within transaction (with lock)
pub async fn get_melt_quote_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let melt_quote = MeltQuote::new(
        None,
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
        Some(serde_json::json!({"tx": true})),
        None,
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
    assert_eq!(retrieved.extra_json, Some(serde_json::json!({"tx": true})));
    tx.commit().await.unwrap();
}

/// Test get mint quote by request within transaction
pub async fn get_mint_quote_by_request_in_transaction<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
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
    let lookup_id = PaymentIdentifier::CustomId(unique_string());
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        lookup_id.clone(),
        None,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
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
    tx.commit().await.unwrap();
}

/// Test getting melt quotes by request lookup id
pub async fn get_melt_quotes_by_request_lookup_id<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let lookup_id = PaymentIdentifier::CustomId(unique_string());

    let quote1 = MeltQuote::new(
        None,
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        Some(lookup_id.clone()),
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        Some(serde_json::json!({"item": 1})),
        None,
    );

    let quote2 = MeltQuote::new(
        None,
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(200, cashu::CurrencyUnit::Sat),
        Amount::new(20, cashu::CurrencyUnit::Sat),
        0,
        Some(lookup_id.clone()),
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        Some(serde_json::json!({"item": 2})),
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote1.clone()).await.unwrap();
    tx.add_melt_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let retrieved = tx
        .get_melt_quotes_by_request_lookup_id(&lookup_id)
        .await
        .unwrap();

    assert_eq!(retrieved.len(), 2);

    let retrieved_quote1 = retrieved.iter().find(|q| q.id == quote1.id).unwrap();
    let retrieved_quote2 = retrieved.iter().find(|q| q.id == quote2.id).unwrap();

    assert_eq!(
        retrieved_quote1.extra_json,
        Some(serde_json::json!({"item": 1}))
    );
    assert_eq!(
        retrieved_quote2.extra_json,
        Some(serde_json::json!({"item": 2}))
    );

    tx.commit().await.unwrap();
}

/// Test lock melt quote and related
pub async fn lock_melt_quote_and_related<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let lookup_id = PaymentIdentifier::CustomId(unique_string());

    let quote1 = MeltQuote::new(
        None,
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(100, cashu::CurrencyUnit::Sat),
        Amount::new(10, cashu::CurrencyUnit::Sat),
        0,
        Some(lookup_id.clone()),
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        Some(serde_json::json!({"target": true})),
        None,
    );

    let quote2 = MeltQuote::new(
        None,
        MeltPaymentRequest::Bolt11 {
            bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap()
        },
        cashu::CurrencyUnit::Sat,
        Amount::new(200, cashu::CurrencyUnit::Sat),
        Amount::new(20, cashu::CurrencyUnit::Sat),
        0,
        Some(lookup_id.clone()),
        None,
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        Some(serde_json::json!({"target": false})),
        None,
    );

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    tx.add_melt_quote(quote1.clone()).await.unwrap();
    tx.add_melt_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let locked = tx.lock_melt_quote_and_related(&quote1.id).await.unwrap();

    assert!(locked.target.is_some());
    let target = locked.target.unwrap();
    assert_eq!(target.id, quote1.id);
    assert_eq!(target.extra_json, Some(serde_json::json!({"target": true})));

    assert_eq!(locked.all_related.len(), 2);

    let retrieved_quote1 = locked
        .all_related
        .iter()
        .find(|q| q.id == quote1.id)
        .unwrap();
    let retrieved_quote2 = locked
        .all_related
        .iter()
        .find(|q| q.id == quote2.id)
        .unwrap();

    assert_eq!(
        retrieved_quote1.extra_json,
        Some(serde_json::json!({"target": true}))
    );
    assert_eq!(
        retrieved_quote2.extra_json,
        Some(serde_json::json!({"target": false}))
    );

    tx.commit().await.unwrap();
}

/// Test that duplicate payment IDs are rejected
pub async fn reject_duplicate_payment_ids<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
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
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(1000, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt11),
        0,
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
    let missing_id = QuoteId::new();
    let ids = vec![quote1.id.clone(), missing_id, quote2.id.clone()];
    let quotes = db.get_mint_quotes_by_ids(&ids).await.unwrap();
    assert_eq!(quotes.len(), 3);
    assert!(quotes[0].is_some());
    assert!(quotes[1].is_none());
    assert!(quotes[2].is_some());
    assert_eq!(quotes[0].as_ref().unwrap().id, quote1.id);
    assert_eq!(quotes[2].as_ref().unwrap().id, quote2.id);

    // 3. Test empty list
    let quotes = db.get_mint_quotes_by_ids(&[]).await;
    assert!(quotes.is_err());

    // 4. Test within transaction (with locking)
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let ids = vec![quote2.id.clone(), quote1.id.clone()]; // Reverse order
    let quotes = tx.get_mint_quotes_by_ids(&ids).await.unwrap();
    assert_eq!(quotes.len(), 2);
    assert_eq!(quotes[0].as_ref().unwrap().id, quote2.id);
    assert_eq!(quotes[1].as_ref().unwrap().id, quote1.id);
    tx.commit().await.unwrap();
}

/// A mint quote with no payments, already stored in the database.
async fn stored_mint_quote<DB>(db: &DB) -> MintQuote
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = Database::begin_transaction(db).await.unwrap();
    tx.add_mint_quote(mint_quote.clone()).await.unwrap();
    tx.commit().await.unwrap();

    mint_quote
}

/// A mint quote kept past its transaction can't overwrite a payment recorded since.
pub async fn update_mint_quote_refuses_stale_amount_paid<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let quote_id = stored_mint_quote(&db).await.id;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut stale = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut current = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    current
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            unique_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut current).await.unwrap();
    tx.commit().await.unwrap();

    stale
        .add_payment(
            Amount::from(50).with_unit(CurrencyUnit::Sat),
            unique_string(),
            None,
        )
        .unwrap();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx.update_mint_quote(&mut stale).await.unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let stored = db.get_mint_quote(&quote_id).await.unwrap().unwrap();
    assert_eq!(stored.amount_paid().value(), 100);
    assert_eq!(stored.payments.len(), 1);
}

/// A mint quote kept past its transaction can't overwrite an issuance recorded since.
pub async fn update_mint_quote_refuses_stale_amount_issued<DB>(db: DB)
where
    DB: Database<Error> + KeysDatabase<Err = Error>,
{
    let quote_id = stored_mint_quote(&db).await.id;

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut quote = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    quote
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            unique_string(),
            None,
        )
        .unwrap();
    tx.update_mint_quote(&mut quote).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut stale = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let mut current = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    current
        .add_issuance(Amount::from(40).with_unit(CurrencyUnit::Sat))
        .unwrap();
    tx.update_mint_quote(&mut current).await.unwrap();
    tx.commit().await.unwrap();

    stale
        .add_issuance(Amount::from(10).with_unit(CurrencyUnit::Sat))
        .unwrap();
    let mut tx = Database::begin_transaction(&db).await.unwrap();
    let err = tx.update_mint_quote(&mut stale).await.unwrap_err();
    assert!(matches!(err, Error::ConcurrentUpdate), "{err}");
    tx.rollback().await.unwrap();

    let stored = db.get_mint_quote(&quote_id).await.unwrap().unwrap();
    assert_eq!(stored.amount_issued().value(), 40);
    assert_eq!(stored.issuance.len(), 1);
}

/// A stale write blocked by another writer's uncommitted update is refused once that writer commits.
///
/// Both writers load the quote in an earlier transaction, so neither holds the lock from that read.
/// Both get past their foreign key checks and the second waits on the guarded `UPDATE`, so a guard
/// that compared the totals before that `UPDATE` would let it through.
///
/// This holds a row lock while another transaction writes, so it needs a database that allows
/// concurrent writers.
pub async fn stale_mint_quote_write_waits_for_lock_then_is_refused(db: DynMintDatabase) {
    let mint_quote = MintQuote::new(
        None,
        unique_string(),
        cashu::CurrencyUnit::Sat,
        None,
        0,
        PaymentIdentifier::CustomId(unique_string()),
        None,
        Amount::new(0, cashu::CurrencyUnit::Sat),
        Amount::new(0, cashu::CurrencyUnit::Sat),
        cashu::PaymentMethod::Known(KnownMethod::Bolt12),
        0,
        0,
        vec![],
        vec![],
        None,
    );
    let quote_id = mint_quote.id.clone();

    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_mint_quote(mint_quote).await.unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin_transaction().await.unwrap();
    let mut stale = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin_transaction().await.unwrap();
    let mut current = tx.get_mint_quote(&quote_id).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    let mut holder = db.begin_transaction().await.unwrap();
    current
        .add_payment(
            Amount::from(100).with_unit(CurrencyUnit::Sat),
            unique_string(),
            None,
        )
        .unwrap();
    holder.update_mint_quote(&mut current).await.unwrap();

    let writer = tokio::spawn({
        let db = db.clone();
        async move {
            stale
                .add_payment(
                    Amount::from(50).with_unit(CurrencyUnit::Sat),
                    unique_string(),
                    None,
                )
                .unwrap();
            let mut tx = db.begin_transaction().await.unwrap();
            let result = tx.update_mint_quote(&mut stale).await;
            tx.rollback().await.unwrap();
            result
        }
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !writer.is_finished(),
        "stale write did not wait for the transaction holding the row lock"
    );

    holder.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .expect("stale write remained blocked")
        .expect("writer task");
    assert!(matches!(result, Err(Error::ConcurrentUpdate)));

    let stored = db.get_mint_quote(&quote_id).await.unwrap().unwrap();
    assert_eq!(stored.amount_paid().value(), 100);
    assert_eq!(stored.payments.len(), 1);
}

/// Test concurrent quote batches acquire row locks in a consistent order.
///
/// This test is invoked explicitly by the PostgreSQL suite because SQLite does
/// not provide row-level `FOR UPDATE` locking.
pub async fn concurrent_mint_quote_batches_use_consistent_lock_order(db: DynMintDatabase) {
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
        0,
        vec![],
        vec![],
        None,
    );

    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_mint_quote(quote1.clone()).await.unwrap();
    tx.add_mint_quote(quote2.clone()).await.unwrap();
    tx.commit().await.unwrap();

    let forward = vec![quote1.id.clone(), quote2.id.clone()];
    let reverse = vec![quote2.id.clone(), quote1.id.clone()];
    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    let first_db = db.clone();
    let first_barrier = Arc::clone(&barrier);
    let first = tokio::spawn(async move {
        let mut tx = first_db.begin_transaction().await?;
        first_barrier.wait().await;
        let quotes = tx.get_mint_quotes_by_ids(&forward).await?;
        tokio::time::sleep(Duration::from_millis(25)).await;
        tx.commit().await?;
        Ok::<_, Error>(quotes)
    });

    let second = tokio::spawn(async move {
        let mut tx = db.begin_transaction().await?;
        barrier.wait().await;
        let quotes = tx.get_mint_quotes_by_ids(&reverse).await?;
        tx.commit().await?;
        Ok::<_, Error>(quotes)
    });

    let (first_result, second_result) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(first, second)
    })
    .await
    .expect("reversed quote batches should not deadlock");

    let first_quotes = first_result.unwrap().unwrap();
    let second_quotes = second_result.unwrap().unwrap();
    assert_eq!(first_quotes[0].as_ref().unwrap().id, quote1.id);
    assert_eq!(first_quotes[1].as_ref().unwrap().id, quote2.id);
    assert_eq!(second_quotes[0].as_ref().unwrap().id, quote2.id);
    assert_eq!(second_quotes[1].as_ref().unwrap().id, quote1.id);
}
