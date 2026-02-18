//! Conditions database implementation (NUT-28)

use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::mint::ConditionsDatabase;
use cdk_common::database::Error;
use cdk_common::mint::{StoredCondition, StoredPartition};
use cdk_common::nuts::nut28::ConditionalKeySetInfo;
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
            description,
            announcements_json,
            attestation_status,
            winning_outcome,
            attested_at,
            created_at
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

    Ok(StoredCondition {
        condition_id: column_as_string!(&condition_id),
        threshold: threshold_val as u32,
        description: column_as_string!(&description),
        announcements_json: column_as_string!(&announcements_json),
        attestation_status: column_as_string!(&attestation_status),
        winning_outcome,
        attested_at,
        created_at: created_at_val,
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

fn sql_row_to_conditional_keyset_info(row: Vec<Column>) -> Result<ConditionalKeySetInfo, Error> {
    unpack_into!(
        let (
            keyset_id,
            unit,
            active,
            input_fee_ppk,
            final_expiry,
            condition_id,
            outcome_collection,
            outcome_collection_id
        ) = row
    );

    let kid_str = column_as_string!(&keyset_id);
    let kid: Id = Id::from_str(&kid_str)
        .map_err(|e| Error::Internal(format!("Invalid keyset id: {e}")))?;

    let active_val: i64 = column_as_number!(active);

    let fee: Option<u64> = match &input_fee_ppk {
        Column::Integer(n) => Some(*n as u64),
        _ => None,
    };

    let expiry: Option<u64> = match &final_expiry {
        Column::Integer(n) => Some(*n as u64),
        _ => None,
    };

    Ok(ConditionalKeySetInfo {
        id: kid,
        unit: column_as_string!(&unit),
        active: active_val != 0,
        input_fee_ppk: fee,
        final_expiry: expiry,
        condition_id: column_as_string!(&condition_id),
        outcome_collection: column_as_string!(&outcome_collection),
        outcome_collection_id: column_as_string!(&outcome_collection_id),
    })
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
                condition_id, threshold, description, announcements_json,
                attestation_status, winning_outcome, attested_at, created_at
            ) VALUES (
                :condition_id, :threshold, :description, :announcements_json,
                :attestation_status, :winning_outcome, :attested_at, :created_at
            )
            "#,
        )?
        .bind("condition_id", condition.condition_id)
        .bind("threshold", condition.threshold as i64)
        .bind("description", condition.description)
        .bind("announcements_json", condition.announcements_json)
        .bind("attestation_status", condition.attestation_status)
        .bind("winning_outcome", condition.winning_outcome)
        .bind("attested_at", condition.attested_at.map(|a| a as i64))
        .bind("created_at", condition.created_at as i64)
        .execute(&*conn)
        .await?;

        Ok(())
    }

    async fn get_condition(&self, condition_id: &str) -> Result<Option<StoredCondition>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let row = query(
            r#"
            SELECT condition_id, threshold, description, announcements_json,
                   attestation_status, winning_outcome, attested_at, created_at
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

    async fn get_conditions(&self) -> Result<Vec<StoredCondition>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT condition_id, threshold, description, announcements_json,
                   attestation_status, winning_outcome, attested_at, created_at
            FROM conditions
            ORDER BY created_at DESC
            "#,
        )?
        .fetch_all(&*conn)
        .await?;

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

    async fn add_conditional_keyset_info(
        &self,
        condition_id: &str,
        outcome_collection: &str,
        outcome_collection_id: &str,
        keyset_id: &Id,
    ) -> Result<(), Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            INSERT INTO conditional_keysets (condition_id, outcome_collection, outcome_collection_id, keyset_id)
            VALUES (:condition_id, :outcome_collection, :outcome_collection_id, :keyset_id)
            "#,
        )?
        .bind("condition_id", condition_id.to_string())
        .bind("outcome_collection", outcome_collection.to_string())
        .bind("outcome_collection_id", outcome_collection_id.to_string())
        .bind("keyset_id", keyset_id.to_string())
        .execute(&*conn)
        .await?;

        Ok(())
    }

    async fn get_conditional_keysets_for_condition(
        &self,
        condition_id: &str,
    ) -> Result<HashMap<String, Id>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT outcome_collection, keyset_id
            FROM conditional_keysets
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
    ) -> Result<Vec<ConditionalKeySetInfo>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        let rows = query(
            r#"
            SELECT ck.keyset_id, ki.unit, ki.active, ki.input_fee_ppk, ki.valid_to,
                   ck.condition_id, ck.outcome_collection, ck.outcome_collection_id
            FROM conditional_keysets ck
            JOIN keyset ki ON ck.keyset_id = ki.id
            "#,
        )?
        .fetch_all(&*conn)
        .await?;

        rows.into_iter()
            .map(sql_row_to_conditional_keyset_info)
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
            FROM conditional_keysets
            WHERE keyset_id = :keyset_id
            "#,
        )?
        .bind("keyset_id", keyset_id.to_string())
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
