//! Helpers for exercising the wallet migrations over real rows.
//!
//! They live here rather than in each backend crate because the two dialects need the same seed and
//! the same assertions, and because building the schema as of a point in the migration history
//! needs the embedded `MIGRATIONS` array, which is otherwise private.

#![allow(clippy::unwrap_used, clippy::missing_panics_doc)]

use cdk_common::database::Error;

use crate::common::migration_order_key;
use crate::database::DatabaseExecutor;
use crate::migrate;
use crate::migration::RegisteredRustMigration;
use crate::stmt::query;
use crate::value::Value;
use crate::wallet::migrations::MIGRATIONS;
use crate::wallet::rust_migrations::MINT_INTERNAL_ID_NAME;

/// A mint URL added to `mint` before the migration runs.
pub const KNOWN_MINT_URL: &str = "https://known.example.com/";

/// A mint URL rows reference but that was never added to `mint`, which is the case the migration's
/// orphan backfill exists for.
pub const ORPHAN_MINT_URL: &str = "https://orphan.example.com/";

/// Builds the wallet schema as of just before `mint_internal_id` and seeds one row in every table
/// that migration converts.
///
/// The SQL list is cut by order key rather than assumed to end at the migration under test, so a
/// migration added later does not silently seed the wrong schema.
pub async fn seed_pre_mint_id<C>(conn: &C, dialect: &str) -> Result<(), Error>
where
    C: DatabaseExecutor + 'static,
{
    let cutoff = migration_order_key(MINT_INTERNAL_ID_NAME);
    let earlier = MIGRATIONS
        .iter()
        .filter(|(_, name, _)| migration_order_key(name) < cutoff)
        .copied()
        .collect::<Vec<_>>();

    migrate(
        conn,
        dialect,
        &earlier,
        Vec::<RegisteredRustMigration<C>>::new(),
    )
    .await?;

    query("INSERT INTO mint (mint_url) VALUES (:mint_url)")?
        .bind("mint_url", KNOWN_MINT_URL)
        .execute(conn)
        .await?;

    query(
        "INSERT INTO keyset (id, mint_url, unit, active) \
         VALUES (:id, :mint_url, :unit, :active)",
    )?
    .bind("id", "ks")
    .bind("mint_url", KNOWN_MINT_URL)
    .bind("unit", "sat")
    .bind("active", true)
    .execute(conn)
    .await?;

    query(
        "INSERT INTO proof (y, mint_url, state, unit, amount, keyset_id, secret, c) \
         VALUES (:y, :mint_url, :state, :unit, :amount, :keyset_id, :secret, :c)",
    )?
    .bind("y", vec![1u8])
    .bind("mint_url", ORPHAN_MINT_URL)
    .bind("state", "UNSPENT")
    .bind("unit", "sat")
    .bind("amount", 1i64)
    .bind("keyset_id", "ks")
    .bind("secret", "s")
    .bind("c", vec![2u8])
    .execute(conn)
    .await?;

    query(
        "INSERT INTO mint_quote (id, mint_url, unit, request, state, expiry) \
         VALUES (:id, :mint_url, :unit, :request, :state, :expiry)",
    )?
    .bind("id", "mq")
    .bind("mint_url", KNOWN_MINT_URL)
    .bind("unit", "sat")
    .bind("request", "r")
    .bind("state", "UNPAID")
    .bind("expiry", 0i64)
    .execute(conn)
    .await?;

    query(
        "INSERT INTO melt_quote (id, unit, amount, request, fee_reserve, expiry, mint_url) \
         VALUES (:id, :unit, :amount, :request, :fee_reserve, :expiry, NULL)",
    )?
    .bind("id", "no_mint")
    .bind("unit", "sat")
    .bind("amount", 1i64)
    .bind("request", "r")
    .bind("fee_reserve", 0i64)
    .bind("expiry", 0i64)
    .execute(conn)
    .await?;

    query(
        "INSERT INTO transactions (id, mint_url, direction, amount, fee, unit, ys, timestamp) \
         VALUES (:id, :mint_url, :direction, :amount, :fee, :unit, :ys, :timestamp)",
    )?
    .bind("id", vec![3u8])
    .bind("mint_url", ORPHAN_MINT_URL)
    .bind("direction", "Incoming")
    .bind("amount", 1i64)
    .bind("fee", 0i64)
    .bind("unit", "sat")
    .bind("ys", vec![4u8])
    .bind("timestamp", 0i64)
    .execute(conn)
    .await?;

    query(
        "INSERT INTO wallet_sagas \
         (id, kind, state, amount, mint_url, unit, created_at, updated_at, data) \
         VALUES (:id, :kind, :state, :amount, :mint_url, :unit, :created_at, :updated_at, :data)",
    )?
    .bind("id", "sg")
    .bind("kind", "send")
    .bind("state", "st")
    .bind("amount", 1i64)
    .bind("mint_url", KNOWN_MINT_URL)
    .bind("unit", "sat")
    .bind("created_at", 0i64)
    .bind("updated_at", 0i64)
    .bind("data", "{}")
    .execute(conn)
    .await?;

    Ok(())
}

/// Asserts the rows [`seed_pre_mint_id`] wrote all survived the conversion and point at the mint
/// they started on.
pub async fn assert_mint_id_migrated<C>(conn: &C) -> Result<(), Error>
where
    C: DatabaseExecutor,
{
    let known = Some(Value::Text(KNOWN_MINT_URL.to_owned()));
    let orphan = Some(Value::Text(ORPHAN_MINT_URL.to_owned()));

    assert_eq!(
        joined_mint_url(conn, "keyset", Value::Text("ks".to_owned())).await?,
        known,
        "keyset"
    );
    assert_eq!(
        joined_mint_url(conn, "mint_quote", Value::Text("mq".to_owned())).await?,
        known,
        "mint_quote"
    );
    assert_eq!(
        joined_mint_url(conn, "wallet_sagas", Value::Text("sg".to_owned())).await?,
        known,
        "wallet_sagas"
    );
    assert_eq!(
        joined_mint_url(conn, "transactions", Value::Blob(vec![3u8])).await?,
        orphan,
        "transactions"
    );

    let proof_url = query("SELECT m.mint_url FROM proof p JOIN mint m ON m.id = p.mint_id")?
        .pluck(conn)
        .await?;
    assert_eq!(proof_url, orphan, "proof");

    let mints = query("SELECT COUNT(*) FROM mint")?.pluck(conn).await?;
    assert_eq!(
        mints,
        Some(Value::Integer(2)),
        "the orphan URL should have gained its own mint row"
    );

    let melt = query("SELECT mint_id FROM melt_quote WHERE id = :id")?
        .bind("id", "no_mint")
        .pluck(conn)
        .await?;
    assert_eq!(
        melt,
        Some(Value::Null),
        "melt_quote.mint_url was nullable, so its mint_id stays NULL"
    );

    Ok(())
}

/// The `mint_url` a converted row now reaches through its `mint_id`.
///
/// The table name is interpolated because identifiers cannot be bound; every caller passes a
/// literal from this file.
async fn joined_mint_url<C>(conn: &C, table: &str, id: Value) -> Result<Option<Value>, Error>
where
    C: DatabaseExecutor,
{
    query(&format!(
        "SELECT m.mint_url FROM {table} t JOIN mint m ON m.id = t.mint_id WHERE t.id = :id"
    ))?
    .bind("id", id)
    .pluck(conn)
    .await
}
