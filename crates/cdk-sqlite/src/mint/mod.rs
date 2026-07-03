//! SQLite Mint

use cdk_sql_common::mint::SQLMintAuthDatabase;
use cdk_sql_common::SQLMintDatabase;

use crate::common::SqliteConnectionManager;

pub mod memory;

/// Mint SQLite implementation with rusqlite
pub type MintSqliteDatabase = SQLMintDatabase<SqliteConnectionManager>;

/// Mint Auth database with rusqlite
pub type MintSqliteAuthDatabase = SQLMintAuthDatabase<SqliteConnectionManager>;

#[cfg(test)]
mod test {
    use std::fs::remove_file;
    use std::time::Duration;

    use cdk_common::mint_db_test;
    use cdk_sql_common::pool::Pool;
    use cdk_sql_common::stmt::query;

    use super::*;
    use crate::common::Config;

    async fn provide_db(_test_name: String) -> MintSqliteDatabase {
        memory::empty().await.unwrap()
    }

    mint_db_test!(provide_db);

    #[tokio::test]
    async fn bug_opening_relative_path() {
        let config: Config = "test.db".into();

        let pool = Pool::<SqliteConnectionManager>::new(config);
        let db = pool.get().await;
        assert!(db.is_ok());
        let _ = remove_file("test.db");
    }

    #[tokio::test]
    async fn exhausted_in_memory_pool_times_out() {
        let config: Config = ":memory:".into();
        let pool = Pool::<SqliteConnectionManager>::new(config);

        let _conn = pool.get().await.expect("valid connection");
        let result = pool.get_timeout(Duration::from_millis(10)).await;

        assert!(matches!(result, Err(cdk_sql_common::pool::Error::Timeout)));
    }

    #[tokio::test]
    async fn open_legacy_and_migrate() {
        let file = format!(
            "{}/db.sqlite",
            std::env::temp_dir().to_str().unwrap_or_default()
        );

        {
            let _ = remove_file(&file);
            #[cfg(not(feature = "sqlcipher"))]
            let config: Config = file.as_str().into();
            #[cfg(feature = "sqlcipher")]
            let config: Config = (file.as_str(), "test").into();

            let pool = Pool::<SqliteConnectionManager>::new(config);

            let conn = pool.get().await.expect("valid connection");

            query(include_str!("../../tests/legacy-sqlx.sql"))
                .expect("query")
                .execute(&*conn)
                .await
                .expect("create former db failed");
        }

        #[cfg(not(feature = "sqlcipher"))]
        let conn = MintSqliteDatabase::new(file.as_str()).await;

        #[cfg(feature = "sqlcipher")]
        let conn = MintSqliteDatabase::new((file.as_str(), "test")).await;

        assert!(conn.is_ok(), "Failed with {:?}", conn.unwrap_err());

        let _ = remove_file(&file);
    }

    /// Creations and mutations of the tracked entities land in the append-only
    /// `journal`, and replaying a record's events reconstructs its state.
    #[cfg(not(feature = "sqlcipher"))]
    #[tokio::test]
    async fn journal_records_and_replays_entities() {
        use std::str::FromStr;

        use bitcoin::bip32::DerivationPath;
        use cdk_common::common::IssuerVersion;
        use cdk_common::database::event_log::{Delta, Entity, Event, Snapshot};
        use cdk_common::database::{MintDatabase, MintKeysDatabase};
        use cdk_common::mint::{MeltPaymentRequest, MeltQuote, MintKeySetInfo};
        use cdk_common::nut00::KnownMethod;
        use cdk_common::nuts::{CurrencyUnit, Id, MeltQuoteState, PaymentMethod};
        use cdk_common::Amount;
        use cdk_sql_common::stmt::Column;

        let file = format!(
            "{}/cdk_journal_replay_test.sqlite",
            std::env::temp_dir().display()
        );
        let _ = remove_file(&file);

        let amounts: Vec<u64> = (0..8).map(|n| 2u64.pow(n)).collect();
        let keyset_a = MintKeySetInfo {
            id: Id::from_str("00916bbf7ef91a36").unwrap(),
            unit: CurrencyUnit::Sat,
            active: true,
            valid_from: 0,
            final_expiry: None,
            derivation_path: DerivationPath::from_str("m/0'/0'/0'").unwrap(),
            derivation_path_index: Some(0),
            input_fee_ppk: 0,
            amounts,
            issuer_version: IssuerVersion::from_str("cdk/0.1.0").ok(),
        };
        let keyset_b = MintKeySetInfo {
            id: Id::from_str("009a1f293253e41e").unwrap(),
            active: false,
            derivation_path: DerivationPath::from_str("m/0'/0'/1'").unwrap(),
            derivation_path_index: Some(1),
            ..keyset_a.clone()
        };

        let melt_quote = MeltQuote::new(
            None,
            MeltPaymentRequest::Bolt11 {
                bolt11: "lnbc330n1p5d85skpp5344v3ktclujsjl3h09wgsfm7zytumr7h7zhrl857f5w8nv0a52zqdqqcqzzsxqyz5vqrzjqvueefmrckfdwyyu39m0lf24sqzcr9vcrmxrvgfn6empxz7phrjxvrttncqq0lcqqyqqqqlgqqqqqqgq2qsp5j3rrg8kvpemqxtf86j8tjm90wq77c7ende4e5qmrerq4xsg02vhq9qxpqysgqjltywgyk6uc5qcgwh8xnzmawl2tjlhz8d28tgp3yx8xwtz76x0jqkfh6mmq70hervjxs0keun7ur0spldgll29l0dnz3md50d65sfqqqwrwpsu".parse().unwrap(),
            },
            CurrencyUnit::Sat,
            Amount::new(100, CurrencyUnit::Sat),
            Amount::new(10, CurrencyUnit::Sat),
            0,
            None,
            None,
            PaymentMethod::Known(KnownMethod::Bolt11),
            Some(serde_json::json!({ "state": "init" })),
            None,
        );
        let payment_proof = "payment_proof_123".to_string();

        // Journaling is now orchestrated by the caller: each mutation is paired
        // with an explicit `add_journal` in the same transaction, exactly as the
        // mint layer does. This drives the real DB writes and the journal writes
        // together, then verifies the journal round-trips and replays.
        {
            let db = MintSqliteDatabase::new(file.as_str()).await.unwrap();
            let keyset_a_record = keyset_a.id.to_string();
            let keyset_b_record = keyset_b.id.to_string();
            let quote_record = melt_quote.id.to_string();

            // Keyset lifecycle: create both, then switch the active one.
            let mut tx = MintKeysDatabase::begin_transaction(&db).await.unwrap();
            tx.add_keyset_info(keyset_a.clone()).await.unwrap();
            tx.add_journal(
                keyset_a_record.clone(),
                Event::Snapshot(Box::new(Snapshot::Keyset(keyset_a.clone()))),
            )
            .await
            .unwrap();
            tx.add_keyset_info(keyset_b.clone()).await.unwrap();
            tx.add_journal(
                keyset_b_record.clone(),
                Event::Snapshot(Box::new(Snapshot::Keyset(keyset_b.clone()))),
            )
            .await
            .unwrap();
            tx.set_active_keyset(CurrencyUnit::Sat, keyset_b.id)
                .await
                .unwrap();
            tx.add_journal(
                keyset_a_record.clone(),
                Event::Delta(Delta::KeysetActive(false)),
            )
            .await
            .unwrap();
            tx.add_journal(
                keyset_b_record.clone(),
                Event::Delta(Delta::KeysetActive(true)),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();

            // Melt quote lifecycle: create, then Unpaid -> Pending -> Paid.
            let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
            tx.add_melt_quote(melt_quote.clone()).await.unwrap();
            tx.add_journal(
                quote_record.clone(),
                Event::Snapshot(Box::new(Snapshot::MeltQuote(melt_quote.clone()))),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();

            let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
            let mut quote = tx.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
            tx.update_melt_quote_state(&mut quote, MeltQuoteState::Pending, None)
                .await
                .unwrap();
            tx.add_journal(
                quote_record.clone(),
                Event::Delta(Delta::MeltQuoteState(MeltQuoteState::Pending)),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();

            let mut tx = MintDatabase::begin_transaction(&db).await.unwrap();
            let mut quote = tx.get_melt_quote(&melt_quote.id).await.unwrap().unwrap();
            tx.update_melt_quote_state(
                &mut quote,
                MeltQuoteState::Paid,
                Some(payment_proof.clone()),
            )
            .await
            .unwrap();
            tx.add_journal(
                quote_record.clone(),
                Event::Delta(Delta::MeltQuoteState(MeltQuoteState::Paid)),
            )
            .await
            .unwrap();
            tx.add_journal(
                quote_record.clone(),
                Event::Delta(Delta::MeltQuotePaymentProof(Some(payment_proof.clone()))),
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
        }

        // Read the journal through an independent connection to the same file.
        let config: Config = file.as_str().into();
        let pool = Pool::<SqliteConnectionManager>::new(config);
        let conn = pool.get().await.expect("valid connection");
        let rows = query("SELECT entity, record, event FROM journal ORDER BY id")
            .expect("query")
            .fetch_all(&*conn)
            .await
            .expect("read journal");

        let events: Vec<(Entity, String, Event)> = rows
            .into_iter()
            .map(|row| {
                let entity = match &row[0] {
                    Column::Integer(i) => Entity::try_from(*i as u8).expect("known entity"),
                    other => panic!("entity must be an integer, got {other:?}"),
                };
                let record = match &row[1] {
                    Column::Text(s) => s.clone(),
                    other => panic!("record must be text, got {other:?}"),
                };
                let bytes = match &row[2] {
                    Column::Blob(b) => b.clone(),
                    other => panic!("event must be a blob, got {other:?}"),
                };
                (
                    entity,
                    record,
                    Event::from_bytes(&bytes).expect("decode event"),
                )
            })
            .collect();

        // Every row's stored entity must match the entity derived from its event.
        for (entity, _, event) in &events {
            assert_eq!(*entity, event.entity());
        }

        let for_record = |ent: Entity, rec: &str| -> Vec<Event> {
            events
                .iter()
                .filter(|(e, r, _)| *e == ent && r == rec)
                .map(|(_, _, e)| e.clone())
                .collect()
        };

        // Keyset A: created active, then deactivated when B took over.
        let a = for_record(Entity::Keyset, &keyset_a.id.to_string());
        assert_eq!(
            a[0],
            Event::Snapshot(Box::new(Snapshot::Keyset(keyset_a.clone())))
        );
        assert!(a.contains(&Event::Delta(Delta::KeysetActive(false))));

        // Keyset B: created inactive, then activated.
        let b = for_record(Entity::Keyset, &keyset_b.id.to_string());
        assert_eq!(
            b[0],
            Event::Snapshot(Box::new(Snapshot::Keyset(keyset_b.clone())))
        );
        assert!(b.contains(&Event::Delta(Delta::KeysetActive(true))));

        // Melt quote: the full-object snapshot round-trips exactly (exercises the
        // typed-amount serde helper), followed by its state deltas.
        let mq = for_record(Entity::MeltQuote, &melt_quote.id.to_string());
        assert_eq!(
            mq[0],
            Event::Snapshot(Box::new(Snapshot::MeltQuote(melt_quote.clone())))
        );
        assert!(mq.contains(&Event::Delta(Delta::MeltQuoteState(
            MeltQuoteState::Pending
        ))));
        assert!(mq.contains(&Event::Delta(Delta::MeltQuoteState(MeltQuoteState::Paid))));
        assert!(mq.contains(&Event::Delta(Delta::MeltQuotePaymentProof(Some(
            payment_proof.clone()
        )))));

        // Replay: base snapshot state plus ordered state deltas => current state.
        let mut replayed_state = match &mq[0] {
            Event::Snapshot(snapshot) => match snapshot.as_ref() {
                Snapshot::MeltQuote(q) => q.state,
                other => panic!("first melt_quote event must be a melt quote, got {other:?}"),
            },
            other => panic!("first melt_quote event must be a snapshot, got {other:?}"),
        };
        for event in &mq {
            if let Event::Delta(Delta::MeltQuoteState(state)) = event {
                replayed_state = *state;
            }
        }
        assert_eq!(replayed_state, MeltQuoteState::Paid);

        let _ = remove_file(&file);
    }
}
