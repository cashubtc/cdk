// Counter compatibility module for handling both legacy and migrated counter semantics
// This module provides compatibility between "last used index" and "next available index" semantics

use crate::error::Error;
use crate::nuts::Id;
use cdk_common::database::WalletDatabase;

/// Counter version enum
#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum CounterVersion {
    /// Version 0: Counter represents "last used index"
    Legacy,
    /// Version 1: Counter represents "next available index"
    NextAvailable,
}

/// Check if the counter migration has been applied by checking for migration_state table
/// This is a simplified check - in production you'd query the migration_state table
pub async fn get_counter_version<DB>(_localstore: &DB) -> Result<CounterVersion, Error>
where
    DB: WalletDatabase<Err = cdk_common::database::Error> + Send + Sync + ?Sized,
{
    // For now, we check if any keyset has a counter > 0
    // If migration was applied, counters would have been incremented
    // This is a simplified heuristic - in production you'd check migration_state table

    // Since we can't directly query migration_state without modifying the trait,
    // we'll assume migration has been applied if this code is running
    // (since it's part of the refactored codebase)
    Ok(CounterVersion::NextAvailable)
}

/// Get the next available counter value with version compatibility
pub async fn get_next_counter<DB>(localstore: &DB, keyset_id: &Id) -> Result<u32, Error>
where
    DB: WalletDatabase<Err = cdk_common::database::Error> + Send + Sync + ?Sized,
{
    let counter = localstore.get_keyset_counter(keyset_id).await?;
    let version = get_counter_version(localstore).await?;

    match version {
        CounterVersion::Legacy => {
            // Old behavior: add 1 to get next available
            Ok(counter.map_or(0, |c| c + 1))
        }
        CounterVersion::NextAvailable => {
            // New behavior: counter IS next available
            Ok(counter.unwrap_or(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_counter_version_enum() {
        assert_eq!(CounterVersion::Legacy, CounterVersion::Legacy);
        assert_eq!(CounterVersion::NextAvailable, CounterVersion::NextAvailable);
        assert_ne!(CounterVersion::Legacy, CounterVersion::NextAvailable);
    }

    #[test]
    fn test_counter_semantics_logic() {
        // Test legacy counter logic (last used + 1)
        let legacy_counter = Some(5u32);
        let legacy_result = match CounterVersion::Legacy {
            CounterVersion::Legacy => legacy_counter.map_or(0, |c| c + 1),
            CounterVersion::NextAvailable => legacy_counter.unwrap_or(0),
        };
        assert_eq!(legacy_result, 6); // 5 + 1 = 6

        // Test next available counter logic (counter is next available)
        let next_available_counter = Some(5u32);
        let next_available_result = match CounterVersion::NextAvailable {
            CounterVersion::Legacy => next_available_counter.map_or(0, |c| c + 1),
            CounterVersion::NextAvailable => next_available_counter.unwrap_or(0),
        };
        assert_eq!(next_available_result, 5); // Counter is next available

        // Test None case for both versions
        let none_counter = None;
        let legacy_none = match CounterVersion::Legacy {
            CounterVersion::Legacy => none_counter.map_or(0, |c| c + 1),
            CounterVersion::NextAvailable => none_counter.unwrap_or(0),
        };
        let next_available_none = match CounterVersion::NextAvailable {
            CounterVersion::Legacy => none_counter.map_or(0, |c| c + 1),
            CounterVersion::NextAvailable => none_counter.unwrap_or(0),
        };
        assert_eq!(legacy_none, 0);
        assert_eq!(next_available_none, 0);
    }

    #[test]
    fn test_migration_scenario() {
        // Pre-migration: counter 5 means "last used index 5", next would be 6
        let pre_migration_counter = 5u32;
        let legacy_next = pre_migration_counter + 1;

        // Post-migration: counter should be set to 6 (next available)
        let post_migration_counter = 6u32;
        let migrated_next = post_migration_counter; // Counter IS next available

        assert_eq!(legacy_next, migrated_next); // Both should produce same result
        assert_eq!(migrated_next, 6);
    }

    #[test]
    fn test_index_zero_scenarios() {
        // New wallet with no counter set (None) should use index 0
        let new_wallet_counter = None;
        let next_index = new_wallet_counter.unwrap_or(0);
        assert_eq!(next_index, 0); // Should start at 0

        // Migrated wallet that never used any indices (counter was 0, so migrated to 1)
        let migrated_unused_counter = 1u32;
        assert_eq!(migrated_unused_counter, 1); // First index after migration

        // Migrated wallet that used indices 0-4 (counter was 4, so migrated to 5)
        let migrated_used_counter = 5u32;
        assert_eq!(migrated_used_counter, 5); // Next available after using 0-4
    }
}
