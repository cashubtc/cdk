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
    /// The receive-secret storage key is invalid.
    #[error("protected receive-secret encryption key is invalid")]
    InvalidReceiveSecretEncryptionKey,
    /// The receive-secret database could not be opened.
    #[error("protected receive-secret store could not be opened")]
    ReceiveSecretStoreOpenFailed,
    /// A protected receive-secret write or read failed.
    #[error("protected receive-secret persistence failed")]
    ReceiveSecretPersistence,
    /// A private prepare-secret request failed validation.
    #[error("protected receive-secret request is invalid")]
    ReceiveSecretRequestInvalid,
    /// An existing action is bound to different protected secret state.
    #[error("protected receive-secret record conflicts with existing state")]
    ReceiveSecretConflict,
    /// A protected receive-secret record is missing.
    #[error("protected receive-secret is unavailable")]
    ReceiveSecretUnavailable,
    /// A protected receive-secret record is malformed or unauthenticated.
    #[error("protected receive-secret record is invalid")]
    ReceiveSecretRecordInvalid,
    /// Receive-secret encryption failed.
    #[error("protected receive-secret encryption failed")]
    ReceiveSecretEncryptionFailed,
    /// Receive-secret decryption failed closed.
    #[error("protected receive-secret decryption failed")]
    ReceiveSecretDecryptionFailed,
    /// Durable acknowledgement construction failed.
    #[error("protected receive-secret acknowledgement failed")]
    ReceiveSecretAcknowledgementFailed,
    /// Accepted/terminal lifecycle evidence is inconsistent.
    #[error("protected receive-secret lifecycle transition is invalid")]
    ReceiveSecretLifecycleInvalid,
    /// Retention is below the accepted v1 minimum or overflows.
    #[error("protected receive-secret retention is invalid")]
    ReceiveSecretRetentionInvalid,
    /// Encrypted backup bytes are malformed, oversized, or inconsistent.
    #[error("protected receive-secret backup is invalid")]
    ReceiveSecretBackupInvalid,
    /// A durable evidence-attestation write or read failed.
    #[error("Bark evidence-attestation persistence failed")]
    EvidenceAttestationPersistence,
    /// One signer already stored different evidence for the same action phase.
    #[error("Bark evidence attestation conflicts with existing state")]
    EvidenceAttestationConflict,
    /// A durable evidence attestation is malformed or unauthenticated.
    #[error("Bark evidence attestation is invalid")]
    EvidenceAttestationInvalid,
    /// The configured unit is not satoshis.
    #[error("federated Bark supports only the satoshi unit")]
    UnsupportedUnit,
    /// Payment limits are empty or inverted.
    #[error("invalid federated Bark payment limits")]
    InvalidLimits,
    /// Accepted Bark receive policy is outside the deliberately narrow v1 profile.
    #[error("unsupported federated Bark receive policy")]
    UnsupportedReceivePolicy,
    /// The live Ark server capabilities differ from accepted federation policy.
    #[error("Ark server capabilities do not match accepted federated Bark policy")]
    ArkCapabilityMismatch,
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
    /// A consensus-selected custody signer attempt failed before Bark could
    /// persist the next action checkpoint. Any preceding Ark cosign exchange
    /// retains the accepted action identity and is safe to re-drive.
    #[error("federated Bark custody signer attempt is unavailable")]
    CustodySignerUnavailable,
    /// An accepted custody signer attempt does not match its original action.
    #[error("federated Bark custody authorization is invalid")]
    InvalidCustodyAuthorization,
    /// Bark wallet construction failed. The source is deliberately discarded.
    #[error("Bark wallet could not be opened")]
    WalletOpenFailed,
    /// Bark wallet properties could not be loaded.
    #[error("Bark wallet properties could not be read")]
    WalletPropertiesUnavailable,
    /// The restricted Bark facade rejected the opened wallet.
    #[error("Bark federation facade could not be opened")]
    FederationFacadeUnavailable,
    /// The accepted receive intent could not be mapped to or served by Bark.
    #[error("federated Bark invoice preparation failed")]
    InvoicePreparationFailed,
    /// The accepted receive plan could not be materialized locally.
    #[error("federated Bark receive projection failed")]
    ReceiveProjectionFailed,
    /// Independent incoming status evidence could not be validated.
    #[error("federated Bark receive observation failed")]
    ReceiveObservationFailed,
    /// Exact incoming claim planning failed closed.
    #[error("federated Bark receive claim plan is invalid")]
    ReceiveClaimPlanInvalid,
    /// Accepted public VTXOs cannot cover the outgoing amount and fee.
    #[error("federated Bark accepted balance is insufficient")]
    InsufficientAcceptedBalance,
    /// Exact outgoing planning or validation failed closed.
    #[error("federated Bark send plan is invalid")]
    SendPlanInvalid,
    /// An accepted outgoing plan could not be materialized locally.
    #[error("federated Bark send projection failed")]
    SendProjectionFailed,
    /// The accepted outgoing HTLC or payment-initiation phase failed.
    #[error("federated Bark send execution failed")]
    SendExecutionFailed,
    /// Accepted outgoing evidence could not be validated or replayed.
    #[error("federated Bark send reconciliation failed")]
    SendReconciliationFailed,
    /// Exact refresh or emergency-exit planning failed closed.
    #[error("federated Bark maintenance plan is invalid")]
    MaintenancePlanInvalid,
    /// The next canonical refresh input cannot fit the fixed participant
    /// authorization boundary and therefore cannot be proposed safely.
    #[error("federated Bark refresh input is too large for its authorization budget: {actual} bytes exceeds {maximum} bytes")]
    MaintenanceAuthorizationTooLarge {
        /// Canonical VTXO artifact size.
        actual: usize,
        /// Accepted per-round aggregate input-artifact budget.
        maximum: usize,
    },
    /// An accepted maintenance plan could not be materialized locally.
    #[error("federated Bark maintenance projection failed")]
    MaintenanceProjectionFailed,
    /// The accepted refresh or exit action could not be executed safely.
    #[error("federated Bark maintenance execution failed")]
    MaintenanceExecutionFailed,
    /// Bark's separate recovery application has no accepted spendable fee reserve.
    #[error("federated Bark recovery fee reserve is insufficient")]
    RecoveryFeeReserveInsufficient,
    /// Accepted maintenance evidence could not be validated or replayed.
    #[error("federated Bark maintenance reconciliation failed")]
    MaintenanceReconciliationFailed,
    /// An accepted incoming claim could not be executed or recovered.
    #[error("federated Bark receive claim requires recovery")]
    ReceiveClaimRecoveryRequired,
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

impl Error {
    /// Preserve this adapter's redacted diagnostic with federation operation context.
    ///
    /// This conversion does not classify an uncertain payment as failed or authorize a retry.
    pub fn into_federation_error(
        self,
        phase: &'static str,
        operation_id: Option<cdk_common::federation::FederationOperationId>,
    ) -> cdk_common::federation::FederationError {
        cdk_common::federation::FederationError::BackendFailure {
            backend: "Bark",
            phase,
            operation_id,
            reason: self.to_string(),
        }
    }
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
            other => Self::Backend(Box::new(other)),
        }
    }
}
