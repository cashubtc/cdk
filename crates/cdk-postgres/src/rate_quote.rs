//! Postgres storage for rate-quoted payment terms.

use std::sync::Arc;

use async_trait::async_trait;
use cdk_common::database::Error;
use cdk_common::nuts::CurrencyUnit;
use cdk_common::payment::PaymentIdentifier;
use cdk_exchange_rate::{
    ParkedPaymentRecord, RateQuoteRecord, RateQuoteStore, RateQuoteStoreError,
};
use cdk_sql_common::pool::Pool;
use cdk_sql_common::stmt::query;

use crate::{PgConfig, PgConnectionPool};

const MIGRATION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS rate_quote_terms (
    payment_lookup_id TEXT PRIMARY KEY,
    fiat_unit TEXT NOT NULL,
    fiat_subunits BIGINT NOT NULL,
    fiat_fee_subunits BIGINT NOT NULL DEFAULT 0,
    snapshot_json TEXT NOT NULL,
    sats_invoiced BIGINT NOT NULL,
    expiry_unix BIGINT NOT NULL
);

ALTER TABLE rate_quote_terms
    ADD COLUMN IF NOT EXISTS fiat_fee_subunits BIGINT NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS parked_payments (
    payment_lookup_id TEXT NOT NULL,
    bolt11_payment_hash TEXT NOT NULL,
    received_sats BIGINT NOT NULL,
    observed_at BIGINT NOT NULL,
    resolution_status TEXT NOT NULL,
    PRIMARY KEY (payment_lookup_id, bolt11_payment_hash)
);
"#;

/// Postgres-backed [`RateQuoteStore`] colocated with the mint database schema.
#[derive(Debug, Clone)]
pub struct PostgresRateQuoteStore {
    pool: Arc<Pool<PgConnectionPool>>,
}

impl PostgresRateQuoteStore {
    /// Create the store and apply its companion-table migration.
    pub async fn new(conn_str: &str) -> Result<Self, Error> {
        let pool = Pool::<PgConnectionPool>::new(PgConfig::from(conn_str));
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<(), Error> {
        let conn = self
            .pool
            .get()
            .map_err(|error| Error::Database(Box::new(error)))?;
        query(MIGRATION_SQL)?.batch(&*conn).await
    }
}

#[async_trait]
impl RateQuoteStore for PostgresRateQuoteStore {
    async fn insert(&self, record: RateQuoteRecord) -> Result<(), RateQuoteStoreError> {
        let conn = self
            .pool
            .get()
            .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;

        query(
            r#"
            INSERT INTO rate_quote_terms
                (payment_lookup_id, fiat_unit, fiat_subunits, fiat_fee_subunits, snapshot_json, sats_invoiced, expiry_unix)
            VALUES
                (:payment_lookup_id, :fiat_unit, :fiat_subunits, :fiat_fee_subunits, :snapshot_json, :sats_invoiced, :expiry_unix)
            "#,
        )
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?
        .bind("payment_lookup_id", record.payment_lookup_id.to_string())
        .bind("fiat_unit", record.fiat_unit.to_string())
        .bind("fiat_subunits", checked_i64(record.fiat_subunits, "fiat_subunits")?)
        .bind(
            "fiat_fee_subunits",
            checked_i64(record.fiat_fee_subunits, "fiat_fee_subunits")?,
        )
        .bind("snapshot_json", record.snapshot_json.to_string())
        .bind("sats_invoiced", checked_i64(record.sats_invoiced, "sats_invoiced")?)
        .bind("expiry_unix", checked_i64(record.expiry_unix, "expiry_unix")?)
        .execute(&*conn)
        .await
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;

        Ok(())
    }

    async fn get_by_lookup_id(
        &self,
        payment_lookup_id: &PaymentIdentifier,
    ) -> Result<Option<RateQuoteRecord>, RateQuoteStoreError> {
        let conn = self
            .pool
            .get()
            .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;

        let Some(row) = query(
            r#"
            SELECT payment_lookup_id, fiat_unit, fiat_subunits, fiat_fee_subunits, snapshot_json, sats_invoiced, expiry_unix
            FROM rate_quote_terms
            WHERE payment_lookup_id = :payment_lookup_id
            "#,
        )
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?
        .bind("payment_lookup_id", payment_lookup_id.to_string())
        .fetch_one(&*conn)
        .await
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?
        else {
            return Ok(None);
        };

        Ok(Some(row_to_record(row)?))
    }

    async fn insert_parked(&self, record: ParkedPaymentRecord) -> Result<(), RateQuoteStoreError> {
        let conn = self
            .pool
            .get()
            .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;

        query(
            r#"
            INSERT INTO parked_payments
                (payment_lookup_id, bolt11_payment_hash, received_sats, observed_at, resolution_status)
            VALUES
                (:payment_lookup_id, :bolt11_payment_hash, :received_sats, :observed_at, :resolution_status)
            ON CONFLICT (payment_lookup_id, bolt11_payment_hash)
            DO UPDATE SET
                received_sats = EXCLUDED.received_sats,
                observed_at = EXCLUDED.observed_at,
                resolution_status = EXCLUDED.resolution_status
            "#,
        )
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?
        .bind("payment_lookup_id", record.payment_lookup_id.to_string())
        .bind("bolt11_payment_hash", record.bolt11_payment_hash)
        .bind("received_sats", checked_i64(record.received_sats, "received_sats")?)
        .bind("observed_at", checked_i64(record.observed_at, "observed_at")?)
        .bind("resolution_status", record.resolution_status)
        .execute(&*conn)
        .await
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;

        Ok(())
    }
}

fn row_to_record(
    row: Vec<cdk_sql_common::stmt::Column>,
) -> Result<RateQuoteRecord, RateQuoteStoreError> {
    if row.len() < 7 {
        return Err(RateQuoteStoreError::Storage(
            "rate quote row had too few columns".to_string(),
        ));
    }

    let payment_lookup_id = text_col(&row[0])?;
    let fiat_unit = text_col(&row[1])?
        .parse::<CurrencyUnit>()
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;
    let snapshot_json = serde_json::from_str(&text_col(&row[4])?)
        .map_err(|error| RateQuoteStoreError::Storage(error.to_string()))?;

    Ok(RateQuoteRecord {
        payment_lookup_id: PaymentIdentifier::CustomId(payment_lookup_id),
        fiat_unit,
        fiat_subunits: int_col(&row[2])?,
        fiat_fee_subunits: int_col(&row[3])?,
        snapshot_json,
        sats_invoiced: int_col(&row[5])?,
        expiry_unix: int_col(&row[6])?,
    })
}

fn checked_i64(value: u64, column: &str) -> Result<i64, RateQuoteStoreError> {
    i64::try_from(value).map_err(|_| {
        RateQuoteStoreError::Storage(format!("{column} value {value} exceeds postgres BIGINT"))
    })
}

fn text_col(value: &cdk_sql_common::stmt::Column) -> Result<String, RateQuoteStoreError> {
    match value {
        cdk_sql_common::value::Value::Text(text) => Ok(text.clone()),
        other => Err(RateQuoteStoreError::Storage(format!(
            "expected text column, got {other:?}"
        ))),
    }
}

fn int_col(value: &cdk_sql_common::stmt::Column) -> Result<u64, RateQuoteStoreError> {
    match value {
        cdk_sql_common::value::Value::Integer(value) => {
            u64::try_from(*value).map_err(|error| RateQuoteStoreError::Storage(error.to_string()))
        }
        other => Err(RateQuoteStoreError::Storage(format!(
            "expected integer column, got {other:?}"
        ))),
    }
}
