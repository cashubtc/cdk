//! Query builder helpers for filtered list methods

use cdk_common::{CurrencyUnit, Id};

use crate::stmt::Statement;

/// Build pagination clause with peek-ahead for has_more detection.
///
/// Queries for limit+1 rows to determine if more results exist.
///
/// # Arguments
/// * `limit` - maximum number of items to return
/// * `offset` - number of items to skip
///
/// # Returns
/// Tuple of SQL clause and requested limit for truncation
pub fn build_pagination_clause(limit: Option<u64>, offset: u64) -> (String, Option<usize>) {
    match limit {
        Some(limit) => (
            format!("LIMIT {} OFFSET {}", limit + 1, offset),
            Some(limit as usize),
        ),
        None if offset > 0 => (format!("OFFSET {}", offset), None),
        None => (String::new(), None),
    }
}

/// Get ORDER BY direction string based on reversed flag.
///
/// # Arguments
/// * `reversed` - if true, sort descending; otherwise ascending
///
/// # Returns
/// "DESC" or "ASC"
pub fn order_direction(reversed: bool) -> &'static str {
    if reversed {
        "DESC"
    } else {
        "ASC"
    }
}

/// Apply peek-ahead truncation and return has_more flag.
///
/// If items exceed requested_limit, truncate and return true.
///
/// # Arguments
/// * `items` - mutable reference to items vector
/// * `requested_limit` - the original limit before peek-ahead
///
/// # Returns
/// True if there are more items beyond the limit
pub fn apply_pagination_peek_ahead<T>(items: &mut Vec<T>, requested_limit: Option<usize>) -> bool {
    match requested_limit {
        Some(limit) if items.len() > limit => {
            items.truncate(limit);
            true
        }
        _ => false,
    }
}

/// Join WHERE clauses with AND, returning empty string if no clauses.
///
/// # Arguments
/// * `clauses` - slice of WHERE clause strings
///
/// # Returns
/// Complete WHERE clause or empty string
pub fn build_where_clause(clauses: &[String]) -> String {
    if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    }
}

/// Bind date range parameters to statement if present.
///
/// # Arguments
/// * `stmt` - the SQL statement
/// * `start` - optional start timestamp
/// * `end` - optional end timestamp
///
/// # Returns
/// Statement with bound parameters
pub fn bind_date_range(stmt: Statement, start: Option<u64>, end: Option<u64>) -> Statement {
    let stmt = match start {
        Some(s) => stmt.bind("creation_date_start", s as i64),
        None => stmt,
    };
    match end {
        Some(e) => stmt.bind("creation_date_end", e as i64),
        None => stmt,
    }
}

/// Bind unit filter parameters to statement if present.
///
/// # Arguments
/// * `stmt` - the SQL statement
/// * `units` - slice of currency units to filter by
///
/// # Returns
/// Statement with bound parameters
pub fn bind_units(stmt: Statement, units: &[CurrencyUnit]) -> Statement {
    if units.is_empty() {
        stmt
    } else {
        stmt.bind_vec("units", units.iter().map(|u| u.to_string()).collect())
    }
}

/// Bind keyset ID filter parameters to statement if present.
///
/// # Arguments
/// * `stmt` - the SQL statement
/// * `keyset_ids` - slice of keyset IDs to filter by
///
/// # Returns
/// Statement with bound parameters
pub fn bind_keyset_ids(stmt: Statement, keyset_ids: &[Id]) -> Statement {
    if keyset_ids.is_empty() {
        stmt
    } else {
        stmt.bind_vec(
            "keyset_ids",
            keyset_ids.iter().map(|id| id.to_string()).collect(),
        )
    }
}

/// Bind operation filter parameters to statement if present.
///
/// # Arguments
/// * `stmt` - the SQL statement
/// * `operations` - slice of operation kinds to filter by
///
/// # Returns
/// Statement with bound parameters
pub fn bind_operations(stmt: Statement, operations: &[String]) -> Statement {
    if operations.is_empty() {
        stmt
    } else {
        stmt.bind_vec("operations", operations.to_vec())
    }
}
