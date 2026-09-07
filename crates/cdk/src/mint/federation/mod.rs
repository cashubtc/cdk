//! Federation mode support for CDK mints.
//!
//! This module applies accepted federation operations to local mint state,
//! validates aggregate BLS keysets, and produces member signature shares while
//! preserving standalone mint behavior outside federation mode.

mod address_allocation;
mod chain_observation;
mod projection;
mod transaction_policy;
mod transaction_state;

mod application;
mod configuration_and_admission;
mod helpers;

pub(super) use self::configuration_and_admission::FederationProofVerificationContext;
pub use self::configuration_and_admission::{FederationMode, FederationRuntimeConfig};
pub(super) use self::helpers::federation_operation_uuid;
pub use self::transaction_policy::{
    FederationCpfpPolicy, FederationFeeRateSource, FederationOnchainTransactionPolicy,
};
pub use cdk_common::federation::FederationTransactionPolicyAudit;

#[cfg(test)]
mod tests;
