//! Typed, redacted backend errors.

use cdk_common::payment;
use thiserror::Error;

/// Federated Bark construction and runtime error.
///
/// Variants intentionally carry only bounded field names and public state.
/// External Bark errors are classified at their boundary instead of being
/// retained, because their messages may contain endpoints or local paths.
#[derive(Debug, Error)]
pub enum Error {
    /// The local backend is disabled.
    #[error("federated Bark backend is disabled")]
    Disabled,
    /// The mint was not started in federation mode.
    #[error("federated Bark requires federation mode")]
    FederationRequired,
    /// The public federation config has no Bark application.
    #[error("federation config has no Bark FROST application")]
    MissingApplication,
    /// The public federation configuration is invalid.
    #[error("invalid federated Bark public configuration")]
    InvalidPublicConfig,
    /// A local public binding differs from accepted configuration.
    #[error("federated Bark configuration mismatch: {field}")]
    ConfigMismatch {
        /// Stable non-secret name of the mismatched field.
        field: &'static str,
    },
    /// Production startup encountered a trusted-dealer setup.
    #[error("trusted-dealer federation setup is not allowed for federated Bark")]
    TrustedDealerForbidden,
    /// Durable participant protocol state is unavailable.
    #[error("durable FROST protocol store is unavailable")]
    MissingFrostProtocolStore,
    /// Protected receive-secret state is unavailable.
    #[error("protected receive-secret store is unavailable")]
    MissingReceiveSecretStore,
    /// The configured unit is not satoshis.
    #[error("federated Bark supports only the satoshi unit")]
    UnsupportedUnit,
    /// Payment limits are empty or inverted.
    #[error("invalid federated Bark payment limits")]
    InvalidLimits,
    /// A Bark configuration omitted or ambiguously selected a chain source.
    #[error("invalid Bark chain-source configuration")]
    InvalidChainSource,
    /// A Bark wallet used local seed custody or another custody kind.
    #[error("Bark wallet is not bound to FROST VTXO custody")]
    StandaloneCustodyForbidden,
    /// The persisted Bark custody descriptor differs from accepted FROST state.
    #[error("Bark wallet custody descriptor does not match accepted federation state")]
    CustodyDescriptorMismatch,
    /// Public locator derivation failed.
    #[error("federated Bark public-key derivation failed")]
    PublicKeyDerivationFailed,
    /// Bark wallet construction failed. The source is deliberately discarded.
    #[error("Bark wallet could not be opened")]
    WalletOpenFailed,
    /// Bark wallet properties could not be loaded.
    #[error("Bark wallet properties could not be read")]
    WalletPropertiesUnavailable,
    /// The restricted Bark facade rejected the opened wallet.
    #[error("Bark federation facade could not be opened")]
    FederationFacadeUnavailable,
    /// The runtime has not caught up to the required accepted frontier.
    #[error("federated Bark runtime is not ready")]
    NotReady,
    /// This path is owned by accepted federation operations, not MintPayment.
    #[error("federated Bark payment mutation requires an accepted federation operation")]
    ConsensusOnly,
    /// A generic payment error crossed the adapter boundary.
    #[error("payment adapter rejected the request")]
    Payment,
}

impl From<payment::Error> for Error {
    fn from(_error: payment::Error) -> Self {
        Self::Payment
    }
}

impl From<Error> for payment::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::UnsupportedUnit => Self::UnsupportedUnit,
            other => Self::Lightning(Box::new(other)),
        }
    }
}
