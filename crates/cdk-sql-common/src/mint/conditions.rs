//! Conditions database implementation (NUT-CTF)

use std::collections::HashMap;

use async_trait::async_trait;
use cdk_common::database::mint::ConditionsDatabase;
use cdk_common::database::Error;
use cdk_common::mint::{MintKeySetInfo, StoredCondition, StoredPartition};
use cdk_common::nuts::nut_ctf::ConditionalKeySetInfo;
use cdk_common::nuts::Id;

use super::SQLMintDatabase;
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_number, column_as_string, unpack_into};

fn sql_row_to_stored_condition(row: Vec<Column>) -> Result<StoredCondition, Error> {
    unpack_into!(
        let (
            condition_id,
            threshold,
            tags_json,
            announcements_json,
            attestation_status,
            winning_outcome,
            attested_at,
            created_at,
            condition_type,
            lo_bound,
            hi_bound,
            precision
        ) = row
    );

    let winning_outcome = match &winning_outcome {
        Column::Text(s) => Some(s.clone()),
        _ => None,
    };

    let attested_at: Option<u64> = match &attested_at {
        Column::Integer(n) => Some(*n as u64),
        _ => None,
    };

    let threshold_val: u64 = column_as_number!(threshold);
    let created_at_val: u64 = column_as_number!(created_at);

    let condition_type_str = match &condition_type {
        Column::Text(s) => s.clone(),
        _ => "enum".to_string(),
    };

    let lo_bound_val: Option<i64> = match &lo_bound {
        Column::Integer(n) => Some(*n),
        _ => None,
    };

    let hi_bound_val: Option<i64> = match &hi_bound {
        Column::Integer(n) => Some(*n),
        _ => None,
    };

    let precision_val: Option<i32> = match &precision {
        Column::Integer(n) => Some(*n as i32),
        _ => None,
    };

    Ok(StoredCondition {
        condition_id: column_as_string!(&condition_id),
        threshold: threshold_val as u32,
        tags_json: column_as_string!(&tags_json),
        announcements_json: column_as_string!(&announcements_json),
        attestation_status: column_as_string!(&attestation_status),
        winning_outcome,
        attested_at,
        created_at: created_at_val,
        condition_type: condition_type_str,
        lo_bound: lo_bound_val,
        hi_bound: hi_bound_val,
        precision: precision_val,
    })
}

fn sql_row_to_stored_partition(row: Vec<Column>) -> Result<StoredPartition, Error> {
    unpack_into!(
        let (
            condition_id,
            partition_json,
            collateral,
            parent_collection_id,
            created_at
        ) = row
    );

    let created_at_val: u64 = column_as_number!(created_at);

    Ok(StoredPartition {
        condition_id: column_as_string!(&condition_id),
        partition_json: column_as_string!(&partition_json),
        collateral: column_as_string!(&collateral),
        parent_collection_id: column_as_string!(&parent_collection_id),
        created_at: created_at_val,
    })
}

fn sql_row_to_keyset_mapping(row: Vec<Column>) -> Result<(String, Id), Error> {
    unpack_into!(
        let (
            outcome_collection,
            keyset_id
        ) = row
    );

    let oc = column_as_string!(&outcome_collection);
    let kid_str = column_as_string!(&keyset_id);
    let kid: Id = kid_str
        .parse()
        .map_err(|e| Error::Internal(format!("Invalid keyset id: {e}")))?;

    Ok((oc, kid))
}

/// Columns selected by every `conditional_keyset` read path. The first 10
/// columns match `sql_row_to_keyset_info` exactly so the base parser can be
/// reused; the last 4 are the conditional-specific fields.
pub(crate) const CONDITIONAL_KEYSET_COLUMNS: &str =
    "id, unit, active, valid_from, valid_to, \
     derivation_path, derivation_path_index, amounts, input_fee_ppk, issuer_version, \
     condition_id, outcome_collection, outcome_collection_id, created_at";

pub(crate) fn sql_row_to_conditional_mint_keyset_info(
    mut row: Vec<Column>,
) -> Result<(MintKeySetInfo, u64), Error> {
    if row.len() != 14 {
        return Err(Error::Internal(format!(
            "expected 14 columns for conditional_keyset, got {}",
            row.len()
        )));
    }

    // Split off the trailing 4 conditional-specific columns, leaving the
    // first 10 to be parsed by the shared base parser.
    let tail: Vec<Column> = row.split_off(10);
    let mut info = super::keys::sql_row_to_keyset_info(row)?;

    let mut tail_iter = tail.into_iter();
    let condition_id = tail_iter.next().expect("length checked above");
    let outcome_collection = tail_iter.next().expect("length checked above");
    let outcome_collection_id = tail_iter.next().expect("length checked above");
    let created_at = tail_iter.next().expect("length checked above");

    info.condition_id = Some(column_as_string!(&condition_id));
    info.outcome_collection = Some(column_as_string!(&outcome_collection));
    info.outcome_collection_id = Some(column_as_string!(&outcome_collection_id));

    let created_at_val: u64 = column_as_number!(created_at);
    Ok((info, created_at_val))
}

fn mint_keyset_info_to_conditional_keyset_info(
    info: &MintKeySetInfo,
    created_at: u64,
) -> Result<ConditionalKeySetInfo, Error> {
    let condition_id = info
        .condition_id
        .clone()
        .ok_or_else(|| Error::Internal("condition_id missing on conditional keyset".to_string()))?;
    let outcome_collection = info.outcome_collection.clone().ok_or_else(|| {
        Error::Internal("outcome_collection missing on conditional keyset".to_string())
    })?;
    let outcome_collection_id = info.outcome_collection_id.clone().ok_or_else(|| {
        Error::Internal("outcome_collection_id missing on conditional keyset".to_string())
    })?;

    Ok(ConditionalKeySetInfo {
        id: info.id,
        unit: info.unit.to_string(),
        active: info.active,
        input_fee_ppk: Some(info.input_fee_ppk),
        final_expiry: info.final_expiry,
        condition_id,
        outcome_collection,
        outcome_collection_id,
        registered_at: created_at,
    })
}

impl<RM> SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    /// Query the `conditional_keyset` table with optional cursor pagination
    /// (`since` is strictly greater than), `limit`, and active filter. This
    /// is the shared path for both the public NUT-CTF listing endpoint and
    /// the internal `reload_keys_from_db` bootstrap.
    pub(crate) async fn query_conditional_keysets(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        active: Option<bool>,
    ) -> Result<Vec<(MintKeySetInfo, u64)>, Error> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let mut sql = format!(
            "SELECT {} FROM conditional_keyset WHERE 1=1",
            CONDITIONAL_KEYSET_COLUMNS
        );

        if since.is_some() {
            // Cursor pagination: strictly greater than the last-seen timestamp.
            sql.push_str(" AND created_at > :since");
        }

        if active.is_some() {
            sql.push_str(" AND active = :active");
        }

        sql.push_str(" ORDER BY created_at ASC");

        if limit.is_some() {
            sql.push_str(" LIMIT :limit");
        }

        let mut stmt = query(&sql)?;

        if let Some(since_ts) = since {
            stmt = stmt.bind("since", since_ts as i64);
        }

        if let Some(active_val) = active {
            stmt = stmt.bind("active", active_val as i64);
        }

        if let Some(limit_val) = limit {
            stmt = stmt.bind("limit", limit_val as i64);
        }

        stmt.fetch_all(&*conn)
            .await?
            .into_iter()
            .map(sql_row_to_conditional_mint_keyset_info)
            .collect()
    }
}

#[async_trait]
impl<RM> ConditionsDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_condition(&self, condition: StoredCondition) -> Result<(), Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            INSERT INTO conditions (
                condition_id, threshold, tags_json, announcements_json,
                attestation_status, winning_outcome, attested_at, created_at,
                condition_type, lo_bound, hi_bound, precision
            ) VALUES (
                :condition_id, :threshold, :tags_json, :announcements_json,
                :attestation_status, :winning_outcome, :attested_at, :created_at,
                :condition_type, :lo_bound, :hi_bound, :precision
            )
            "#,
        )?
        .bind("condition_id", condition.condition_id)
        .bind("threshold", condition.threshold as i64)
        .bind("tags_json", condition.tags_json)
        .bind("announcements_json", condition.announcements_json)
        .bind("attestation_status", condition.attestation_status)
        .bind("winning_outcome", condition.winning_outcome)
        .bind("attested_at", condition.attested_at.map(|a| a as i64))
        .bind("created_at", condition.created_at as i64)
        .bind("condition_type", condition.condition_type)
        .bind("lo_bound", condition.lo_bound)
        .bind("hi_bound", condition.hi_bound)
        .bind("precision", condition.precision.map(|p| p as i64))
        .execute(&*conn)
        .await?;

        Ok(())
    }

    async fn get_condition(&self, condition_id: &str) -> Result<Option<StoredCondition>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let row = query(
            r#"
            SELECT condition_id, threshold, tags_json, announcements_json,
                   attestation_status, winning_outcome, attested_at, created_at,
                   condition_type, lo_bound, hi_bound, precision
            FROM conditions
            WHERE condition_id = :condition_id
            "#,
        )?
        .bind("condition_id", condition_id.to_string())
        .fetch_one(&*conn)
        .await?;

        match row {
            Some(r) => Ok(Some(sql_row_to_stored_condition(r)?)),
            None => Ok(None),
        }
    }

    async fn get_conditions(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        status: &[String],
    ) -> Result<Vec<StoredCondition>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        // Build SQL dynamically for status IN clause
        let mut sql = String::from(
            "SELECT condition_id, threshold, tags_json, announcements_json, \
             attestation_status, winning_outcome, attested_at, created_at, \
             condition_type, lo_bound, hi_bound, precision FROM conditions WHERE 1=1",
        );

        if since.is_some() {
            // Cursor pagination: strictly greater, so callers can pass the
            // last-seen `created_at` without re-receiving the boundary row.
            sql.push_str(" AND created_at > :since");
        }

        if !status.is_empty() {
            sql.push_str(" AND attestation_status IN (");
            for (i, _) in status.iter().enumerate() {
                if i > 0 {
                    sql.push(',');
                }
                sql.push_str(&format!(":status_{}", i));
            }
            sql.push(')');
        }

        sql.push_str(" ORDER BY created_at ASC");

        if limit.is_some() {
            sql.push_str(" LIMIT :limit");
        }

        let mut stmt = query(&sql)?;

        if let Some(since_ts) = since {
            stmt = stmt.bind("since", since_ts as i64);
        }

        for (i, s) in status.iter().enumerate() {
            stmt = stmt.bind(&format!("status_{}", i), s.clone());
        }

        if let Some(limit_val) = limit {
            stmt = stmt.bind("limit", limit_val as i64);
        }

        let rows = stmt.fetch_all(&*conn).await?;

        rows.into_iter()
            .map(sql_row_to_stored_condition)
            .collect()
    }

    async fn update_condition_attestation(
        &self,
        condition_id: &str,
        status: &str,
        winning_outcome: Option<&str>,
        attested_at: Option<u64>,
    ) -> Result<bool, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows_affected = query(
            r#"
            UPDATE conditions
            SET attestation_status = :status,
                winning_outcome = :winning_outcome,
                attested_at = :attested_at
            WHERE condition_id = :condition_id
              AND attestation_status = 'pending'
            "#,
        )?
        .bind("status", status.to_string())
        .bind("winning_outcome", winning_outcome.map(|w| w.to_string()))
        .bind("attested_at", attested_at.map(|a| a as i64))
        .bind("condition_id", condition_id.to_string())
        .execute(&*conn)
        .await?;

        Ok(rows_affected > 0)
    }

    async fn get_conditional_keysets_for_condition(
        &self,
        condition_id: &str,
    ) -> Result<HashMap<String, Id>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT outcome_collection, id
            FROM conditional_keyset
            WHERE condition_id = :condition_id
            "#,
        )?
        .bind("condition_id", condition_id.to_string())
        .fetch_all(&*conn)
        .await?;

        let mut map = HashMap::new();
        for row in rows {
            let (oc, kid) = sql_row_to_keyset_mapping(row)?;
            map.insert(oc, kid);
        }

        Ok(map)
    }

    async fn get_all_conditional_keyset_infos(
        &self,
        since: Option<u64>,
        limit: Option<u64>,
        active: Option<bool>,
    ) -> Result<Vec<ConditionalKeySetInfo>, Self::Err> {
        let rows = self
            .query_conditional_keysets(since, limit, active)
            .await?;
        rows.into_iter()
            .map(|(info, created_at)| mint_keyset_info_to_conditional_keyset_info(&info, created_at))
            .collect()
    }

    async fn get_condition_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Option<(String, String, String)>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let row = query(
            r#"
            SELECT condition_id, outcome_collection, outcome_collection_id
            FROM conditional_keyset
            WHERE id = :id
            "#,
        )?
        .bind("id", keyset_id.to_string())
        .fetch_one(&*conn)
        .await?;

        match row {
            Some(r) => {
                unpack_into!(
                    let (condition_id, outcome_collection, outcome_collection_id) = r
                );
                Ok(Some((
                    column_as_string!(&condition_id),
                    column_as_string!(&outcome_collection),
                    column_as_string!(&outcome_collection_id),
                )))
            }
            None => Ok(None),
        }
    }

    async fn add_partition(&self, partition: StoredPartition) -> Result<(), Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            INSERT INTO condition_partitions (
                condition_id, partition_json, collateral, parent_collection_id, created_at
            ) VALUES (
                :condition_id, :partition_json, :collateral, :parent_collection_id, :created_at
            )
            "#,
        )?
        .bind("condition_id", partition.condition_id)
        .bind("partition_json", partition.partition_json)
        .bind("collateral", partition.collateral)
        .bind("parent_collection_id", partition.parent_collection_id)
        .bind("created_at", partition.created_at as i64)
        .execute(&*conn)
        .await?;

        Ok(())
    }

    async fn get_partitions_for_condition(
        &self,
        condition_id: &str,
    ) -> Result<Vec<StoredPartition>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT condition_id, partition_json, collateral, parent_collection_id, created_at
            FROM condition_partitions
            WHERE condition_id = :condition_id
            ORDER BY created_at ASC
            "#,
        )?
        .bind("condition_id", condition_id.to_string())
        .fetch_all(&*conn)
        .await?;

        rows.into_iter()
            .map(sql_row_to_stored_partition)
            .collect()
    }
}
