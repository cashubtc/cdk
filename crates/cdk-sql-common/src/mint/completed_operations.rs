//! Completed operations database implementation

use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::mint::{CompletedOperationsDatabase, CompletedOperationsTransaction};
use cdk_common::database::Error;
use cdk_common::util::unix_time;
use cdk_common::{mint, Amount, PaymentMethod};

use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_nullable_string, column_as_number, column_as_string, unpack_into};

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
}
