//! Completed operations database implementation

use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::mint::{
    CompletedOperationsDatabase, CompletedOperationsTransaction, OperationFilter,
    OperationListResult, OperationRecord,
};
use cdk_common::database::Error;
use cdk_common::util::unix_time;
use cdk_common::{mint, Amount, PaymentMethod};

use super::filters::{
    apply_pagination_peek_ahead, bind_date_range, bind_operations, bind_units,
    build_pagination_clause, build_where_clause, order_direction,
};
use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{
    column_as_nullable_number, column_as_nullable_string, column_as_number, column_as_string,
    unpack_into,
};

fn sql_row_to_completed_operation(row: Vec<Column>) -> Result<mint::Operation, Error> {
    unpack_into!(
        let (
            operation_id,
            operation_kind,
            completed_at,
            total_issued,
            total_redeemed,
            fee_collected,
            payment_method
        ) = row
    );

    let operation_id_str = column_as_string!(&operation_id);
    let operation_id = uuid::Uuid::parse_str(&operation_id_str)
        .map_err(|e| Error::Internal(format!("Invalid operation_id UUID: {e}")))?;

    let operation_kind_str = column_as_string!(&operation_kind);
    let operation_kind = mint::OperationKind::from_str(&operation_kind_str)
        .map_err(|e| Error::Internal(format!("Invalid operation kind: {e}")))?;

    let completed_at: u64 = column_as_number!(completed_at);
    let total_issued_u64: u64 = column_as_number!(total_issued);
    let total_redeemed_u64: u64 = column_as_number!(total_redeemed);
    let fee_collected_u64: u64 = column_as_number!(fee_collected);

    let total_issued = Amount::from(total_issued_u64);
    let total_redeemed = Amount::from(total_redeemed_u64);
    let fee_collected = Amount::from(fee_collected_u64);

    let payment_method = column_as_nullable_string!(payment_method)
        .map(|s| PaymentMethod::from_str(&s))
        .transpose()
        .map_err(|e| Error::Internal(format!("Invalid payment method: {e}")))?;

    Ok(mint::Operation::new(
        operation_id,
        operation_kind,
        total_issued,
        total_redeemed,
        fee_collected,
        Some(completed_at),
        payment_method,
    ))
}

fn sql_row_to_operation_record(row: Vec<Column>) -> Result<OperationRecord, Error> {
    unpack_into!(
        let (
            operation_id, operation_kind, completed_at, total_issued, total_redeemed,
            fee_collected, payment_amount, payment_fee, payment_method, unit
        ) = row
    );

    let total_issued: u64 = column_as_number!(total_issued);
    let total_redeemed: u64 = column_as_number!(total_redeemed);
    let fee_collected: u64 = column_as_number!(fee_collected);
    let completed_time: u64 = column_as_number!(completed_at);
    let payment_amount_val: Option<u64> = column_as_nullable_number!(payment_amount);
    let payment_fee_val: Option<u64> = column_as_nullable_number!(payment_fee);

    Ok(OperationRecord {
        operation_id: column_as_string!(operation_id),
        operation_kind: column_as_string!(operation_kind),
        completed_time,
        total_issued: Amount::from(total_issued),
        total_redeemed: Amount::from(total_redeemed),
        fee_collected: Amount::from(fee_collected),
        payment_amount: payment_amount_val.map(Amount::from),
        payment_fee: payment_fee_val.map(Amount::from),
        payment_method: column_as_nullable_string!(payment_method),
        unit: column_as_nullable_string!(unit),
    })
}

#[async_trait]
impl<RM> CompletedOperationsTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_completed_operation(
        &mut self,
        operation: &mint::Operation,
        fee_by_keyset: &std::collections::HashMap<cdk_common::nuts::Id, cdk_common::Amount>,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO completed_operations
            (operation_id, operation_kind, completed_at, total_issued, total_redeemed, fee_collected, payment_amount, payment_fee, payment_method)
            VALUES
            (:operation_id, :operation_kind, :completed_at, :total_issued, :total_redeemed, :fee_collected, :payment_amount, :payment_fee, :payment_method)
            "#,
        )?
        .bind("operation_id", operation.id().to_string())
        .bind("operation_kind", operation.kind().to_string())
        .bind("completed_at", operation.completed_at().unwrap_or(unix_time()) as i64)
        .bind("total_issued", operation.total_issued().to_u64() as i64)
        .bind("total_redeemed", operation.total_redeemed().to_u64() as i64)
        .bind("fee_collected", operation.fee_collected().to_u64() as i64)
        .bind("payment_amount", operation.payment_amount().map(|a| a.to_u64() as i64))
        .bind("payment_fee", operation.payment_fee().map(|a| a.to_u64() as i64))
        .bind("payment_method", operation.payment_method().map(|m| m.to_string()))
        .execute(&self.inner)
        .await?;

        // Update keyset_amounts with fee_collected from the breakdown
        for (keyset_id, fee) in fee_by_keyset {
            if fee.to_u64() > 0 {
                query(
                    r#"
                    INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
                    VALUES (:keyset_id, 0, 0, :fee)
                    ON CONFLICT (keyset_id)
                    DO UPDATE SET fee_collected = keyset_amounts.fee_collected + EXCLUDED.fee_collected
                    "#,
                )?
                .bind("keyset_id", keyset_id.to_string())
                .bind("fee", fee.to_u64() as i64)
                .execute(&self.inner)
                .await?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl<RM> CompletedOperationsDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_completed_operation(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Option<mint::Operation>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                completed_at,
                total_issued,
                total_redeemed,
                fee_collected,
                payment_method
            FROM
                completed_operations
            WHERE
                operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_one(&*conn)
        .await?
        .map(sql_row_to_completed_operation)
        .transpose()?)
    }

    async fn get_completed_operations_by_kind(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Operation>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                completed_at,
                total_issued,
                total_redeemed,
                fee_collected,
                payment_method
            FROM
                completed_operations
            WHERE
                operation_kind = :operation_kind
            ORDER BY completed_at DESC
            "#,
        )?
        .bind("operation_kind", operation_kind.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_completed_operation)
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_completed_operations(&self) -> Result<Vec<mint::Operation>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                completed_at,
                total_issued,
                total_redeemed,
                fee_collected,
                payment_method
            FROM
                completed_operations
            ORDER BY completed_at DESC
            "#,
        )?
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_completed_operation)
        .collect::<Result<Vec<_>, _>>()?)
    }

    async fn list_operations_filtered(
        &self,
        filter: OperationFilter,
    ) -> Result<OperationListResult, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;

        // Build dynamic WHERE clauses
        let mut where_clauses: Vec<String> = Vec::new();
        let needs_unit_join = !filter.units.is_empty();

        if filter.creation_date_start.is_some() {
            where_clauses.push("co.completed_at >= :creation_date_start".into());
        }
        if filter.creation_date_end.is_some() {
            where_clauses.push("co.completed_at <= :creation_date_end".into());
        }
        if !filter.operations.is_empty() {
            where_clauses.push("co.operation_kind IN (:operations)".into());
        }
        if needs_unit_join {
            where_clauses.push("ou.unit IN (:units)".into());
        }

        let where_clause = build_where_clause(&where_clauses);
        let (limit_clause, requested_limit) = build_pagination_clause(filter.limit, filter.offset);
        let order = order_direction(filter.reversed);

        // Different query strategies based on whether we need unit filtering
        let query_str = if needs_unit_join {
            // JOIN approach with CTE when filtering by unit
            format!(
                r#"
                WITH operation_units AS (
                    SELECT p.operation_id, k.unit
                    FROM proof p
                    JOIN keyset k ON p.keyset_id = k.id
                    WHERE p.operation_id IS NOT NULL
                    UNION
                    SELECT bs.operation_id, k.unit
                    FROM blind_signature bs
                    JOIN keyset k ON bs.keyset_id = k.id
                    WHERE bs.operation_id IS NOT NULL
                )
                SELECT co.operation_id, co.operation_kind, co.completed_at,
                       co.total_issued, co.total_redeemed, co.fee_collected,
                       co.payment_amount, co.payment_fee, co.payment_method,
                       MIN(ou.unit) as unit
                FROM completed_operations co
                JOIN operation_units ou ON co.operation_id = ou.operation_id
                {where_clause}
                GROUP BY co.operation_id, co.operation_kind, co.completed_at,
                         co.total_issued, co.total_redeemed, co.fee_collected,
                         co.payment_amount, co.payment_fee, co.payment_method
                ORDER BY co.completed_at {order}
                {limit_clause}
                "#,
                where_clause = where_clause,
                order = order,
                limit_clause = limit_clause,
            )
        } else {
            // Subquery approach when not filtering by unit (more efficient)
            format!(
                r#"
                SELECT co.operation_id, co.operation_kind, co.completed_at,
                       co.total_issued, co.total_redeemed, co.fee_collected,
                       co.payment_amount, co.payment_fee, co.payment_method,
                       COALESCE(
                           (SELECT k.unit FROM proof p
                            JOIN keyset k ON p.keyset_id = k.id
                            WHERE p.operation_id = co.operation_id LIMIT 1),
                           (SELECT k.unit FROM blind_signature bs
                            JOIN keyset k ON bs.keyset_id = k.id
                            WHERE bs.operation_id = co.operation_id LIMIT 1)
                       ) as unit
                FROM completed_operations co
                {where_clause}
                ORDER BY co.completed_at {order}
                {limit_clause}
                "#,
                where_clause = where_clause,
                order = order,
                limit_clause = limit_clause,
            )
        };

        let stmt = query(&query_str)?;
        let stmt = bind_date_range(stmt, filter.creation_date_start, filter.creation_date_end);
        let stmt = bind_operations(stmt, &filter.operations);
        let stmt = bind_units(stmt, &filter.units);

        let mut operations = stmt
            .fetch_all(&*conn)
            .await?
            .into_iter()
            .map(sql_row_to_operation_record)
            .collect::<Result<Vec<_>, _>>()?;

        let has_more = apply_pagination_peek_ahead(&mut operations, requested_limit);

        Ok(OperationListResult {
            operations,
            has_more,
        })
    }
}
