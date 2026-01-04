//! Saga database implementation

use std::str::FromStr;

use async_trait::async_trait;
use cdk_common::database::mint::{SagaDatabase, SagaTransaction};
use cdk_common::database::Error;
use cdk_common::mint;
use cdk_common::util::unix_time;

use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_number, column_as_string, unpack_into};

fn sql_row_to_saga(row: Vec<Column>) -> Result<mint::Saga, Error> {
    unpack_into!(
        let (
            operation_id,
            operation_kind,
            state,
            quote_id,
            created_at,
            updated_at
        ) = row
    );

    let operation_id_str = column_as_string!(&operation_id);
    let operation_id = uuid::Uuid::parse_str(&operation_id_str)
        .map_err(|e| Error::Internal(format!("Invalid operation_id UUID: {e}")))?;

    let operation_kind_str = column_as_string!(&operation_kind);
    let operation_kind = mint::OperationKind::from_str(&operation_kind_str)
        .map_err(|e| Error::Internal(format!("Invalid operation kind: {e}")))?;

    let state_str = column_as_string!(&state);
    let state = mint::SagaStateEnum::new(operation_kind, &state_str)
        .map_err(|e| Error::Internal(format!("Invalid saga state: {e}")))?;

    let quote_id = match &quote_id {
        Column::Text(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Column::Null => None,
        _ => None,
    };

    let created_at: u64 = column_as_number!(created_at);
    let updated_at: u64 = column_as_number!(updated_at);

    Ok(mint::Saga {
        operation_id,
        operation_kind,
        state,
        quote_id,
        created_at,
        updated_at,
    })
}

#[async_trait]
impl<RM> SagaTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_saga(
        &mut self,
        operation_id: &uuid::Uuid,
    ) -> Result<Option<mint::Saga>, Self::Err> {
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                state,
                quote_id,
                created_at,
                updated_at
            FROM
                saga_state
            WHERE
                operation_id = :operation_id
            FOR UPDATE
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .fetch_one(&self.inner)
        .await?
        .map(sql_row_to_saga)
        .transpose()?)
    }

    async fn add_saga(&mut self, saga: &mint::Saga) -> Result<(), Self::Err> {
        let current_time = unix_time();

        query(
            r#"
            INSERT INTO saga_state
            (operation_id, operation_kind, state, quote_id, created_at, updated_at)
            VALUES
            (:operation_id, :operation_kind, :state, :quote_id, :created_at, :updated_at)
            "#,
        )?
        .bind("operation_id", saga.operation_id.to_string())
        .bind("operation_kind", saga.operation_kind.to_string())
        .bind("state", saga.state.state())
        .bind("quote_id", saga.quote_id.as_deref())
        .bind("created_at", saga.created_at as i64)
        .bind("updated_at", current_time as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn update_saga(
        &mut self,
        operation_id: &uuid::Uuid,
        new_state: mint::SagaStateEnum,
    ) -> Result<(), Self::Err> {
        let current_time = unix_time();

        query(
            r#"
            UPDATE saga_state
            SET state = :state, updated_at = :updated_at
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("state", new_state.state())
        .bind("updated_at", current_time as i64)
        .bind("operation_id", operation_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn delete_saga(&mut self, operation_id: &uuid::Uuid) -> Result<(), Self::Err> {
        query(
            r#"
            DELETE FROM saga_state
            WHERE operation_id = :operation_id
            "#,
        )?
        .bind("operation_id", operation_id.to_string())
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> SagaDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_incomplete_sagas(
        &self,
        operation_kind: mint::OperationKind,
    ) -> Result<Vec<mint::Saga>, Self::Err> {
        let conn = self.pool.get().map_err(|e| Error::Database(Box::new(e)))?;
        Ok(query(
            r#"
            SELECT
                operation_id,
                operation_kind,
                state,
                quote_id,
                created_at,
                updated_at
            FROM
                saga_state
            WHERE
                operation_kind = :operation_kind
            ORDER BY created_at ASC
            "#,
        )?
        .bind("operation_kind", operation_kind.to_string())
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_saga)
        .collect::<Result<Vec<_>, _>>()?)
    }
}
