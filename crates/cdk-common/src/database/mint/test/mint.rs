//! Payments

use crate::database::mint::test::unique_string;
use crate::database::mint::{Database, Error, KeysDatabase};
use crate::mint::MintQuote;
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
