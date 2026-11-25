//! Federation mode support for CDK mints.
//!
//! This module applies accepted federation operations to local mint state,
//! validates aggregate BLS keysets, and produces member signature shares while
//! preserving standalone mint behavior outside federation mode.

mod address_allocation;
mod chain_observation;
#[cfg(feature = "federation-bark")]
mod projection;
mod retention;
mod state_install;
mod state_transfer;
mod transaction_policy;
mod transaction_state;

mod application;
mod configuration_and_admission;
mod helpers;

pub(super) use self::configuration_and_admission::FederationProofVerificationContext;
pub use self::configuration_and_admission::{FederationMode, FederationRuntimeConfig};
pub(super) use self::helpers::{federation_operation_uuid, federation_share_signatures};
pub use self::transaction_policy::{
    FederationCpfpPolicy, FederationFeeRateSource, FederationOnchainTransactionPolicy,
};
pub use cdk_common::federation::FederationTransactionPolicyAudit;

use crate::Error;

/// Whether an apply error is a deterministic rejection of an ordered operation.
///
/// Every member applies the same accepted order to the same state, so such an
/// error is recorded as the operation's rejection and application continues
/// past it. Any other error leaves the operation `Failed` for retry.
///
/// Wallet proof-release policy is deliberately conservative for spent or
/// reserved inputs. At ordered application, those states instead identify a
/// deterministic losing operation: its transaction rolled back, and replay
/// records the rejection and continues beyond it.
pub fn is_definitive_federation_apply_rejection(error: &Error) -> bool {
    error.is_definitive_failure()
        || matches!(
            error,
            Error::PendingQuote | Error::TokenAlreadySpent | Error::TokenPending
        )
}

#[cfg(test)]
mod tests;
