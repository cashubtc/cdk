//! PostgreSQL proof coordination through the public database API.

#![allow(clippy::unwrap_used)]

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database::{Error, MintDatabase, MintProofsDatabase};
use cdk_common::mint::Operation;
use cdk_common::secret::Secret;
use cdk_common::{Amount, Id, Proof, SecretKey, State};
use tokio::sync::Barrier;
use tokio_postgres::{Client, NoTls};

use crate::MintPgDatabase;

async fn setup() -> (Arc<MintPgDatabase>, Client) {
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .unwrap_or_else(|_| {
            "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                .to_owned()
        });
    let schema = format!("proof_coordination_{}", uuid::Uuid::new_v4().simple());
    let config = format!("{url} application_name={schema} schema={schema}");
    let db = Arc::new(MintPgDatabase::new(config.as_str()).await.unwrap());
    let (client, connection) = tokio_postgres::connect(&url, NoTls).await.unwrap();
    tokio::spawn(connection);
    client
        .batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    (db, client)
}

fn proof() -> Proof {
    Proof {
        amount: Amount::from(128),
        keyset_id: Id::from_str("00916bbf7ef91a36").unwrap(),
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    }
}

fn operation() -> Operation {
    Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO)
}

async fn wait_for_proof_lock(client: &Client, insert: bool) {
    let pattern = if insert {
        "%INSERT INTO proof%"
    } else {
        "%FROM%proof%FOR UPDATE%"
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let n: i64 = client.query_one("SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND pid <> pg_backend_pid() AND application_name = current_schema() AND wait_event_type = 'Lock' AND query LIKE $1", &[&pattern]).await.unwrap().get(0);
            if n > 0 { break; }
            tokio::task::yield_now().await;
        }
    }).await.expect("proof operation must reach its contended database lock");
}

#[tokio::test]
async fn unrelated_proofs_complete_while_another_reservation_is_open() {
    let (db, _) = setup().await;
    let mut holder = db.begin_transaction().await.unwrap();
    holder
        .add_proofs(vec![proof()], None, &operation())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut tx = db.begin_transaction().await.unwrap();
        tx.add_proofs(vec![proof()], None, &operation())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await
    .expect("unrelated proof must not wait for the holder");
    holder.rollback().await.unwrap();
}

#[tokio::test]
async fn same_uncommitted_proof_has_one_reservation_winner() {
    let (db, client) = setup().await;
    let proof = proof();
    let mut holder = db.begin_transaction().await.unwrap();
    holder
        .add_proofs(vec![proof.clone()], None, &operation())
        .await
        .unwrap();
    let contender = tokio::spawn({
        let db = db.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            let result = tx.add_proofs(vec![proof], None, &operation()).await;
            tx.rollback().await.unwrap();
            result
        }
    });
    wait_for_proof_lock(&client, true).await;
    holder.commit().await.unwrap();
    assert!(matches!(contender.await.unwrap(), Err(Error::Duplicate)));
}

#[tokio::test]
async fn rolled_back_reservation_can_be_acquired_by_waiter() {
    let (db, client) = setup().await;
    let proof = proof();
    let y = proof.y().unwrap();
    let mut holder = db.begin_transaction().await.unwrap();
    holder
        .add_proofs(vec![proof.clone()], None, &operation())
        .await
        .unwrap();
    let contender = tokio::spawn({
        let db = db.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(vec![proof], None, &operation())
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });
    wait_for_proof_lock(&client, true).await;
    holder.rollback().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), contender)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        db.get_proofs_states(&[y]).await.unwrap(),
        vec![Some(State::Unspent)]
    );
}

#[tokio::test]
async fn reversed_overlapping_batches_finish_without_deadlock() {
    let (db, _) = setup().await;
    let proofs = vec![proof(), proof(), proof(), proof()];
    let mut reversed = proofs.clone();
    reversed.reverse();
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = Vec::new();
    for batch in [proofs, reversed] {
        let db = db.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let mut tx = db.begin_transaction().await.unwrap();
            barrier.wait().await;
            match tx.add_proofs(batch, None, &operation()).await {
                Ok(_) => {
                    tx.commit().await.unwrap();
                    true
                }
                Err(Error::Duplicate) => {
                    tx.rollback().await.unwrap();
                    false
                }
                Err(err) => panic!("unexpected reservation result: {err}"),
            }
        }));
    }
    let mut winners = 0;
    for task in tasks {
        if tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
        {
            winners += 1;
        }
    }
    assert_eq!(winners, 1);
}

#[tokio::test]
async fn finalization_and_release_coordinate_on_the_same_proofs() {
    let (db, client) = setup().await;
    let proof = proof();
    let y = proof.y().unwrap();
    let mut tx = db.begin_transaction().await.unwrap();
    let mut proofs = tx
        .add_proofs(vec![proof], None, &operation())
        .await
        .unwrap();
    tx.update_proofs_state(&mut proofs, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut finalizer = db.begin_transaction().await.unwrap();
    let mut proofs = finalizer.get_proofs(&[y]).await.unwrap();
    let release = tokio::spawn({
        let db = db.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            let result = tx.remove_proofs(&[y], None).await;
            tx.rollback().await.unwrap();
            result
        }
    });
    wait_for_proof_lock(&client, false).await;
    finalizer
        .update_proofs_state(&mut proofs, State::Spent)
        .await
        .unwrap();
    finalizer.commit().await.unwrap();
    assert!(matches!(
        release.await.unwrap(),
        Err(Error::AttemptRemoveSpentProof)
    ));
    assert_eq!(
        db.get_proofs_states(&[y]).await.unwrap(),
        vec![Some(State::Spent)]
    );
}

#[tokio::test]
async fn cancelling_a_reservation_releases_it_for_a_waiter() {
    let (db, client) = setup().await;
    let proof = proof();
    let y = proof.y().unwrap();
    let (ready, acquired) = tokio::sync::oneshot::channel();
    let holder = tokio::spawn({
        let db = db.clone();
        let proof = proof.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(vec![proof], None, &operation())
                .await
                .unwrap();
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
            tx.commit().await.unwrap();
        }
    });
    acquired.await.unwrap();
    let waiter = tokio::spawn({
        let db = db.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(vec![proof], None, &operation())
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });
    wait_for_proof_lock(&client, true).await;
    holder.abort();
    assert!(holder.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        db.get_proofs_states(&[y]).await.unwrap(),
        vec![Some(State::Unspent)]
    );
}

#[tokio::test]
async fn released_proof_can_be_reserved_after_the_release_commits() {
    let (db, client) = setup().await;
    let proof = proof();
    let y = proof.y().unwrap();
    let mut tx = db.begin_transaction().await.unwrap();
    let mut proofs = tx
        .add_proofs(vec![proof.clone()], None, &operation())
        .await
        .unwrap();
    tx.update_proofs_state(&mut proofs, State::Pending)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut release = db.begin_transaction().await.unwrap();
    release.remove_proofs(&[y], None).await.unwrap();
    let waiter = tokio::spawn({
        let db = db.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_proofs(vec![proof], None, &operation())
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    });
    wait_for_proof_lock(&client, false).await;
    release.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        db.get_proofs_states(&[y]).await.unwrap(),
        vec![Some(State::Unspent)]
    );
}

#[tokio::test]
async fn identical_nullifiers_in_separate_schemas_do_not_contend() {
    let (first, _) = setup().await;
    let (second, _) = setup().await;
    let proof = proof();
    let mut holder = first.begin_transaction().await.unwrap();
    holder
        .add_proofs(vec![proof.clone()], None, &operation())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut tx = second.begin_transaction().await.unwrap();
        tx.add_proofs(vec![proof], None, &operation())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await
    .expect("different proof tables must coordinate independently");
    holder.rollback().await.unwrap();
}

#[tokio::test]
async fn auth_proofs_coordinate_by_nullifier_before_the_first_commit() {
    use cdk_common::database::MintAuthDatabase;
    use cdk_common::AuthProof;

    use crate::MintPgAuthDatabase;

    let (_, client) = setup().await;
    let schema: String = client
        .query_one("SELECT current_schema()", &[])
        .await
        .unwrap()
        .get(0);
    let schema = format!("{schema}_auth");
    let url = std::env::var("CDK_MINTD_DATABASE_URL")
        .or_else(|_| std::env::var("PG_DB_URL"))
        .unwrap_or_else(|_| {
            "host=localhost user=cdk_user password=cdk_password dbname=cdk_mint port=5432"
                .to_owned()
        });
    let config = format!("{url} application_name={schema} schema={schema}");
    let db = Arc::new(MintPgAuthDatabase::new(config.as_str()).await.unwrap());
    client
        .batch_execute(&format!("SET search_path TO {schema}"))
        .await
        .unwrap();
    let make_proof = || AuthProof {
        keyset_id: Id::from_str("00916bbf7ef91a36").unwrap(),
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        dleq: None,
    };
    let proof = make_proof();
    let y = proof.y().unwrap();
    let mut holder = db.begin_transaction().await.unwrap();
    holder.add_proof(proof.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut tx = db.begin_transaction().await.unwrap();
        tx.add_proof(make_proof()).await.unwrap();
        tx.commit().await.unwrap();
    })
    .await
    .expect("unrelated auth proof must not wait for the holder");
    let contender = tokio::spawn({
        let db = db.clone();
        async move {
            let mut tx = db.begin_transaction().await.unwrap();
            let result = tx.add_proof(proof).await;
            tx.rollback().await.unwrap();
            result
        }
    });
    wait_for_proof_lock(&client, true).await;
    holder.commit().await.unwrap();
    assert!(matches!(contender.await.unwrap(), Err(Error::Duplicate)));
    assert_eq!(
        db.get_proofs_states(&[y]).await.unwrap(),
        vec![Some(State::Spent)]
    );
}
