#![cfg(test)]
//! Test helper utilities for CDK unit tests
//!
//! This module provides shared test utilities for creating test mints, wallets,
//! and test data without external dependencies (Lightning nodes, databases).
//!
//! These helpers are only compiled when running tests.

#[cfg(feature = "mint")]
pub mod mint;
