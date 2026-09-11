//! A [`DatabaseExecutor`] that records what it was asked to run instead of running it.
//!
//! It exists so the migration runner's ordering and skipping can be tested without a database. It
//! answers only the two queries the runner itself issues; everything else is recorded and ignored.

use std::collections::HashSet;
use std::sync::Mutex;

use async_trait::async_trait;
use cdk_common::database::Error;

use crate::database::DatabaseExecutor;
use crate::stmt::{Column, Statement};
use crate::value::Value;

const APPLIED_PROBE: &str = "SELECT name FROM migrations WHERE name";
const RECORD_APPLIED: &str = "INSERT INTO migrations";

/// Records every statement, and tracks the `migrations` table in memory.
#[derive(Debug, Default)]
pub struct FakeExecutor {
    executed: Mutex<Vec<String>>,
    applied: Mutex<HashSet<String>>,
}

impl FakeExecutor {
    /// A fake whose `migrations` table already holds `names`.
    pub fn with_applied(names: &[&str]) -> Self {
        Self {
            executed: Mutex::new(Vec::new()),
            applied: Mutex::new(names.iter().map(|name| (*name).to_owned()).collect()),
        }
    }

    /// Every statement the runner issued, in order.
    pub fn executed(&self) -> Vec<String> {
        self.executed.lock().expect("executed lock").clone()
    }

    /// The names the runner recorded as applied, in the order it recorded them.
    pub fn recorded_names(&self) -> Vec<String> {
        self.executed()
            .iter()
            .filter_map(|entry| entry.strip_prefix("recorded:").map(str::to_owned))
            .collect()
    }

    /// Splits a statement into its SQL and bound values, and files it under `executed`.
    fn record(&self, statement: Statement) -> Result<(String, Vec<Value>), Error> {
        let (sql, values) = statement.to_sql()?;
        self.executed
            .lock()
            .expect("executed lock")
            .push(sql.clone());
        Ok((sql, values))
    }

    fn first_text(values: &[Value]) -> Option<String> {
        values.iter().find_map(|value| match value {
            Value::Text(text) => Some(text.clone()),
            _ => None,
        })
    }
}

#[async_trait]
impl DatabaseExecutor for FakeExecutor {
    fn name() -> &'static str {
        "sqlite"
    }

    async fn execute(&self, statement: Statement) -> Result<usize, Error> {
        let (sql, values) = self.record(statement)?;

        if sql.contains(RECORD_APPLIED) {
            if let Some(name) = Self::first_text(&values) {
                self.applied
                    .lock()
                    .expect("applied lock")
                    .insert(name.clone());
                self.executed
                    .lock()
                    .expect("executed lock")
                    .push(format!("recorded:{name}"));
            }
        }

        Ok(1)
    }

    async fn fetch_one(&self, statement: Statement) -> Result<Option<Vec<Column>>, Error> {
        self.record(statement)?;
        Ok(None)
    }

    async fn fetch_all(&self, statement: Statement) -> Result<Vec<Vec<Column>>, Error> {
        self.record(statement)?;
        Ok(Vec::new())
    }

    async fn pluck(&self, statement: Statement) -> Result<Option<Column>, Error> {
        let (sql, values) = self.record(statement)?;

        if !sql.contains(APPLIED_PROBE) {
            return Ok(None);
        }

        let Some(name) = Self::first_text(&values) else {
            return Ok(None);
        };

        Ok(self
            .applied
            .lock()
            .expect("applied lock")
            .contains(&name)
            .then(|| Value::Text(name)))
    }

    async fn batch(&self, statement: Statement) -> Result<(), Error> {
        self.record(statement)?;
        Ok(())
    }
}
