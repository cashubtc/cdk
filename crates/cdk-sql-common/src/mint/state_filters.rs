//! State filter database implementation

use async_trait::async_trait;
use cdk_common::database::mint::{StateFilterConfig, StateFilterDatabase, StateFilterTransaction};
use cdk_common::database::Error;
use cdk_common::nuts::{Filter, FilterElement};

use super::{SQLMintDatabase, SQLTransaction};
use crate::pool::DatabasePool;
use crate::stmt::{query, Column};
use crate::{column_as_binary, column_as_number, unpack_into};

fn sql_row_to_element(row: Vec<Column>) -> Result<FilterElement, Error> {
    unpack_into!(let (element) = row);

    let bytes: Vec<u8> = column_as_binary!(element);
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| Error::Internal("Filter element is not 32 bytes".to_string()))?;

    Ok(FilterElement::from_bytes(bytes))
}

fn sql_row_to_filter(row: Vec<Column>) -> Result<Filter, Error> {
    unpack_into!(let (start_time, end_time, data) = row);

    let data: Vec<u8> = column_as_binary!(data);

    Ok(Filter {
        start: column_as_number!(start_time),
        end: column_as_number!(end_time),
        data: cdk_common::util::hex::encode(data),
    })
}

#[async_trait]
impl<RM> StateFilterTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn set_state_filter_config(
        &mut self,
        config: &StateFilterConfig,
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO state_filter_config (id, genesis, epoch_seconds, p, page_size)
            VALUES (0, :genesis, :epoch_seconds, :p, :page_size)
            ON CONFLICT (id) DO NOTHING
            "#,
        )?
        .bind("genesis", config.genesis as i64)
        .bind("epoch_seconds", config.epoch_seconds as i64)
        .bind("p", i64::from(config.p))
        .bind("page_size", config.page_size as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }

    async fn add_filter_elements(
        &mut self,
        epoch: u64,
        elements: &[FilterElement],
    ) -> Result<(), Self::Err> {
        for element in elements {
            query(
                r#"
                INSERT INTO state_filter_element (epoch, element)
                VALUES (:epoch, :element)
                ON CONFLICT (epoch, element) DO NOTHING
                "#,
            )?
            .bind("epoch", epoch as i64)
            .bind("element", element.as_bytes().to_vec())
            .execute(&self.inner)
            .await?;
        }

        Ok(())
    }

    async fn take_filter_elements(&mut self, epoch: u64) -> Result<Vec<FilterElement>, Self::Err> {
        let elements = query(
            r#"SELECT element FROM state_filter_element WHERE epoch = :epoch ORDER BY element"#,
        )?
        .bind("epoch", epoch as i64)
        .fetch_all(&self.inner)
        .await?
        .into_iter()
        .map(sql_row_to_element)
        .collect::<Result<Vec<_>, _>>()?;

        query(r#"DELETE FROM state_filter_element WHERE epoch = :epoch"#)?
            .bind("epoch", epoch as i64)
            .execute(&self.inner)
            .await?;

        Ok(elements)
    }

    async fn add_filter(
        &mut self,
        epoch: u64,
        start: u64,
        end: u64,
        data: &[u8],
    ) -> Result<(), Self::Err> {
        query(
            r#"
            INSERT INTO state_filter (epoch, start_time, end_time, data)
            VALUES (:epoch, :start_time, :end_time, :data)
            ON CONFLICT (epoch) DO NOTHING
            "#,
        )?
        .bind("epoch", epoch as i64)
        .bind("start_time", start as i64)
        .bind("end_time", end as i64)
        .bind("data", data.to_vec())
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl<RM> StateFilterDatabase for SQLMintDatabase<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn get_state_filter_config(&self) -> Result<Option<StateFilterConfig>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"SELECT genesis, epoch_seconds, p, page_size FROM state_filter_config WHERE id = 0"#,
        )?
        .fetch_one(&*conn)
        .await?
        .map(|row| {
            unpack_into!(let (genesis, epoch_seconds, p, page_size) = row);

            let p: u64 = column_as_number!(p);
            let p = u8::try_from(p).map_err(|_| {
                Error::Internal("Stored filter parameter is out of range".to_string())
            })?;

            Ok(StateFilterConfig {
                genesis: column_as_number!(genesis),
                epoch_seconds: column_as_number!(epoch_seconds),
                p,
                page_size: column_as_number!(page_size),
            })
        })
        .transpose()
    }

    async fn latest_built_epoch(&self) -> Result<Option<u64>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"SELECT MAX(epoch) FROM state_filter"#)?
            .fetch_one(&*conn)
            .await?
            .map(|row| {
                unpack_into!(let (epoch) = row);
                Ok(match epoch {
                    Column::Null => None,
                    other => Some(column_as_number!(other)),
                })
            })
            .transpose()
            .map(Option::flatten)
    }

    async fn get_filters(&self, first_epoch: u64, limit: u64) -> Result<Vec<Filter>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"
            SELECT start_time, end_time, data
            FROM state_filter
            WHERE epoch >= :first_epoch
            ORDER BY epoch ASC
            LIMIT :limit
            "#,
        )?
        .bind("first_epoch", first_epoch as i64)
        .bind("limit", limit as i64)
        .fetch_all(&*conn)
        .await?
        .into_iter()
        .map(sql_row_to_filter)
        .collect()
    }

    async fn get_filter_elements(&self, epoch: u64) -> Result<Vec<FilterElement>, Self::Err> {
        let conn = self
            .pool
            .get()
            .await
            .map_err(|e| Error::Database(Box::new(e)))?;

        query(r#"SELECT element FROM state_filter_element WHERE epoch = :epoch ORDER BY element"#)?
            .bind("epoch", epoch as i64)
            .fetch_all(&*conn)
            .await?
            .into_iter()
            .map(sql_row_to_element)
            .collect()
    }
}
