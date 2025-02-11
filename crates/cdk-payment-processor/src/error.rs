//! Errors

use thiserror::Error;

/// CDK Payment processor error
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid ID
    #[error("Invalid id")]
    InvalidId,
    /// NUT00 Error
    #[error(transparent)]
    NUT00(#[from] cdk_common::nuts::nut00::Error),
    /// NUT05 error
    #[error(transparent)]
    NUT05(#[from] cdk_common::nuts::nut05::Error),
    /// Parse invoice error
    #[error(transparent)]
    Invoice(#[from] lightning_invoice::ParseOrSemanticError),
}
