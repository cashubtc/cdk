use std::time::Duration;

use cdk_common::mint::MeltPaymentRequest;
use cdk_common::nut00::KnownMethod;
use cdk_common::payment::PaymentIdentifier;
use cdk_common::{
    Amount, CurrencyUnit, MeltQuoteBolt11Request, MeltQuoteRequest, MeltQuoteState, MeltRequest,
    PaymentMethod,
};
use cdk_fake_wallet::create_fake_invoice;

use super::*;
use crate::mint::melt::melt_saga::{MeltSaga, PaymentOutcome};
use crate::test_helpers::mint::{
    create_test_blinded_messages, create_test_mint, mint_test_proofs, set_fail_for, should_fail_for,
};

async fn pending_quote(db: &DynMintDatabase) -> MeltQuote {
    let invoice = create_fake_invoice(10_000, String::new());
    let mut quote = MeltQuote::new(
        None,
        MeltPaymentRequest::Bolt11 {
            bolt11: invoice.clone(),
        },
        CurrencyUnit::Sat,
        Amount::new(10, CurrencyUnit::Sat),
        Amount::new(1, CurrencyUnit::Sat),
        0,
        Some(PaymentIdentifier::PaymentHash(
            *invoice.payment_hash().as_ref(),
        )),
        None,
        PaymentMethod::Known(KnownMethod::Bolt11),
        None,
        None,
    );
    quote.state = MeltQuoteState::Pending;
    let mut tx = db.begin_transaction().await.unwrap();
    tx.add_melt_quote(quote.clone()).await.unwrap();
    tx.commit().await.unwrap();
    quote
}

#[tokio::test]
async fn expired_takeover_fences_old_owner() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let mut quote = pending_quote(&db).await;
    let mut owner = MeltLease::claim(&db, &mut quote).await.unwrap().unwrap();
    let stale = quote.clone();
    assert!(MeltLease::claim(&db, &mut quote).await.unwrap().is_none());
    let mut tx = db.begin_transaction().await.unwrap();
    assert!(tx
        .renew_melt_quote_lease(&quote.id, &quote.melt_lock, 0)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    let mut replacement = MeltLease::claim(&db, &mut quote).await.unwrap().unwrap();
    assert_ne!(quote.melt_lock, stale.melt_lock);
    let mut tx = db.begin_transaction().await.unwrap();
    let current = tx.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(matches!(
        check_melt_lease(&mut tx, &current, &stale.melt_lock).await,
        Err(Error::MeltQuoteLocked)
    ));
    tx.rollback().await.unwrap();
    owner.release().await;
    assert_eq!(
        db.get_melt_quote(&quote.id)
            .await
            .unwrap()
            .unwrap()
            .melt_lock,
        quote.melt_lock
    );
    replacement.release().await;
}

#[tokio::test]
async fn heartbeat_renews_and_release_stops_it() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let quote = pending_quote(&db).await;
    let mut tx = db.begin_transaction().await.unwrap();
    let quote = tx
        .claim_melt_quote_lease(&quote.id, "owner", 2)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut lease = MeltLease::with_timing(
        db.clone(),
        &quote,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let current = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
            if current.melt_lock_expires_at > quote.melt_lock_expires_at {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    lease.release().await;
    assert!(lease.heartbeat.is_finished());
    let current = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert!(!current.is_locked());
    assert_eq!(current.state, MeltQuoteState::Pending);
}

#[tokio::test]
async fn transient_renewal_error_is_retried() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let quote = pending_quote(&db).await;
    let mut tx = db.begin_transaction().await.unwrap();
    let quote = tx
        .claim_melt_quote_lease(&quote.id, "owner", 2)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    set_fail_for("MELT_LEASE_RENEW");
    let mut lease = MeltLease::with_timing(
        db.clone(),
        &quote,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let current = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
            if current.melt_lock_expires_at > quote.melt_lock_expires_at {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("renewal should succeed after the injected transient error");
    assert!(!should_fail_for("MELT_LEASE_RENEW"));
    assert!(!lease.lost.is_cancelled());
    lease.run(async { Ok(()) }).await.unwrap();
    lease.release().await;
}

#[tokio::test]
async fn budget_stops_renewal_and_times_out_work() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let mut quote = pending_quote(&db).await;
    let mut owner = MeltLease::claim(&db, &mut quote).await.unwrap().unwrap();
    owner.release().await;
    let mut tx = db.begin_transaction().await.unwrap();
    let quote = tx
        .claim_melt_quote_lease(&quote.id, "owner", 60)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut lease = MeltLease::with_timing(
        db.clone(),
        &quote,
        Duration::from_millis(10),
        Duration::from_millis(50),
    );
    assert!(matches!(
        lease.run(std::future::pending::<Result<(), Error>>()).await,
        Err(Error::PendingMeltTimeout { .. })
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !lease.heartbeat.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    lease.release().await;
    assert!(!db
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap()
        .is_locked());
}

#[tokio::test]
async fn cancelled_executor_releases_its_owner() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let mut quote = pending_quote(&db).await;
    let lease = MeltLease::claim(&db, &mut quote).await.unwrap().unwrap();
    let task =
        tokio::spawn(async move { lease.run(std::future::pending::<Result<(), Error>>()).await });
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let current = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
            if !current.is_locked() {
                assert_eq!(current.state, MeltQuoteState::Pending);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn lost_lease_stops_work() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let quote = pending_quote(&db).await;
    let mut tx = db.begin_transaction().await.unwrap();
    let quote = tx
        .claim_melt_quote_lease(&quote.id, "owner", 60)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut lease = MeltLease::with_timing(
        db.clone(),
        &quote,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );
    let mut tx = db.begin_transaction().await.unwrap();
    assert!(tx
        .renew_melt_quote_lease(&quote.id, "owner", 0)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(1),
            lease.run(std::future::pending::<Result<(), Error>>())
        )
        .await
        .unwrap(),
        Err(Error::MeltQuoteLocked)
    ));
    lease.release().await;
    assert_eq!(
        db.get_melt_quote(&quote.id).await.unwrap().unwrap().state,
        MeltQuoteState::Pending
    );
}

#[tokio::test]
async fn committed_completion_preserves_executor_result() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let response = mint
        .get_melt_quote(MeltQuoteRequest::Bolt11(MeltQuoteBolt11Request {
            request: create_fake_invoice(100_000, String::new()),
            unit: CurrencyUnit::Sat,
            options: None,
        }))
        .await
        .unwrap();
    let proofs = mint_test_proofs(&mint, Amount::from(1_000)).await.unwrap();
    let (outputs, _) = create_test_blinded_messages(&mint, Amount::from(1_023))
        .await
        .unwrap();
    let request = MeltRequest::new(response.quote().unwrap().clone(), proofs, Some(outputs));
    let verification = mint.verify_inputs(request.inputs()).await.unwrap();
    let setup = MeltSaga::new(Arc::new(mint.clone()), db.clone(), mint.pubsub_manager())
        .setup_melt(
            &request,
            verification,
            PaymentMethod::Known(KnownMethod::Bolt11),
        )
        .await
        .unwrap();
    let quote = setup.lease_quote().clone();
    let mut lease = MeltLease::with_timing(
        db.clone(),
        &quote,
        Duration::from_millis(10),
        Duration::from_secs(10),
    );

    let response = lease
        .run(async {
            let (setup, settlement) = setup.attempt_internal_settlement(&request).await?;
            let PaymentOutcome::Confirmed(confirmed) = setup.make_payment(settlement).await? else {
                panic!("test payment should be confirmed");
            };
            let response = confirmed.finalize().await?;

            tokio::time::timeout(Duration::from_secs(1), async {
                while !lease.heartbeat.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            Ok(response)
        })
        .await
        .unwrap();

    assert_eq!(response.state(), MeltQuoteState::Paid);
    assert!(!response.change().unwrap().is_empty());
    assert!(*lease.committed.lock().await);
    assert!(!lease.lost.is_cancelled());
    lease.release().await;
}

#[tokio::test]
async fn another_owner_completing_recovery_stops_executor() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let quote = pending_quote(&db).await;
    let mut tx = db.begin_transaction().await.unwrap();
    let quote = tx
        .claim_melt_quote_lease(&quote.id, "owner", 60)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut lease = MeltLease::with_timing(
        db.clone(),
        &quote,
        Duration::from_millis(10),
        Duration::from_secs(2),
    );

    let result = lease
        .run(async {
            let mut tx = db.begin_transaction().await?;
            assert!(
                tx.renew_melt_quote_lease(&quote.id, &quote.melt_lock, 0)
                    .await?
            );
            tx.claim_melt_quote_lease(&quote.id, "recovery-owner", 60)
                .await?
                .unwrap();
            let mut recovered = tx.get_melt_quote(&quote.id).await?.unwrap();
            tx.update_melt_quote_state(&mut recovered, MeltQuoteState::Paid, None)
                .await?;
            tx.unlock_melt_quote(&quote.id, "recovery-owner").await?;
            commit_melt_completion(tx, &quote.id, "recovery-owner").await?;
            std::future::pending::<Result<(), Error>>().await
        })
        .await;

    assert!(matches!(result, Err(Error::MeltQuoteLocked)));
    assert!(!*lease.committed.lock().await);
    let current = db.get_melt_quote(&quote.id).await.unwrap().unwrap();
    assert_eq!(current.state, MeltQuoteState::Paid);
    assert!(!current.is_locked());
    lease.release().await;
}

#[tokio::test]
async fn release_failure_can_be_retried_without_replacing_work_error() {
    let mint = create_test_mint().await.unwrap();
    let db = mint.localstore();
    let mut quote = pending_quote(&db).await;
    let mut lease = MeltLease::claim(&db, &mut quote).await.unwrap().unwrap();
    let result = lease.run(async { Err::<(), _>(Error::UnknownQuote) }).await;

    set_fail_for("MELT_LEASE_RELEASE");
    lease.release().await;
    assert!(!should_fail_for("MELT_LEASE_RELEASE"));
    assert!(matches!(result, Err(Error::UnknownQuote)));
    assert!(!lease.released);
    lease.release().await;
    assert!(lease.released);
    assert!(!db
        .get_melt_quote(&quote.id)
        .await
        .unwrap()
        .unwrap()
        .is_locked());
}
