//! [`JournalTransaction`] implementation: append-only `journal` writes.
//!
//! The mint layer orchestrates which events to emit; this only provides the
//! durable append. Each insert runs on the transaction's own connection, so it
//! joins the caller's transaction and commits or rolls back with it.

use async_trait::async_trait;
use cdk_common::database::event_log::{generate_id, Event};
use cdk_common::database::{Error, JournalTransaction};
use cdk_common::util::unix_time;

use super::SQLTransaction;
use crate::pool::DatabasePool;
use crate::stmt::query;

#[async_trait]
impl<RM> JournalTransaction for SQLTransaction<RM>
where
    RM: DatabasePool + 'static,
{
    type Err = Error;

    async fn add_journal(&mut self, record: String, event: Event) -> Result<(), Error> {
        // The entity is derived from the event, so it is always consistent with
        // the payload and callers only pass the row's bare primary key.
        let entity = i64::from(event.entity().as_u8());
        let bytes = event.to_bytes().map_err(|e| Error::Database(Box::new(e)))?;

        query(
            r#"INSERT INTO journal (id, entity, record, event, created_at)
               VALUES (:id, :entity, :record, :event, :created_at)"#,
        )?
        .bind("id", generate_id())
        .bind("entity", entity)
        .bind("record", record)
        .bind("event", bytes)
        .bind("created_at", unix_time() as i64)
        .execute(&self.inner)
        .await?;

        Ok(())
    }
}
