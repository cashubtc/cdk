//! Member-local Bark wallet runtime and bounded readiness reporting.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bark::actions::lightning::receive::LightningReceiveAntiDosMode;
use bark::ark::lightning::{Invoice, PaymentHash, Preimage};
use bark::ark::mailbox::MailboxIdentifier;
use bark::ark::ProtocolEncoding;
use bark::custody::{KeySourceDescriptor, KeySourceKind, VtxoCustody, VtxoKeyLocator};
use bark::federation::{
    FederationActionId, FederationExitEvidence, FederationExitPhase, FederationExitPlan,
    FederationLightningReceiveClaimExecution, FederationLightningReceiveClaimPlan,
    FederationLightningReceiveIntent, FederationLightningReceivePhase,
    FederationLightningReceivePlan, FederationLightningReceiveStatus,
    FederationLightningSendCandidate, FederationLightningSendHtlcEvidence,
    FederationLightningSendPlan, FederationLightningSendRevocationEvidence,
    FederationLightningSendRevocationPlan, FederationLightningSendStatus,
    FederationMaintenanceInput, FederationRefreshEvidence, FederationRefreshExecution,
    FederationRefreshPlan, FederationRequestCommitment, FederationSendInput, FederationVtxoImport,
    FederationVtxoState as BarkFederationVtxoState, FederationWallet,
};
use bark::{OpenWalletArgs, Wallet, WalletProperties};
use bitcoin::consensus::{
    deserialize as deserialize_transaction, serialize as serialize_transaction,
};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::PublicKey;
use bitcoin::{FeeRate, ScriptBuf, Transaction};
use cdk_common::federation::{
    FederationAllocatedAddressRef, FederationConfig, FederationJournalEntry, FederationOperationId,
    FederationOperationStatus, MemberId, FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};
use cdk_common::{CurrencyUnit, MeltQuoteState};
use cdk_federation_bark::{
    ark_server_commitment, federation_bark_exit_allocation_quote,
    federation_bark_receive_htlc_package_hash, federation_bark_receive_request_hash,
    federation_bark_refresh_candidates, federation_bark_signer_set_for_attempt,
    validate_federation_bark_lightning_send_expiry, validate_federation_bark_operation,
    FederationBarkActionId, FederationBarkActionKind, FederationBarkActionPlan,
    FederationBarkActionRecord, FederationBarkActionStatus, FederationBarkActionTransition,
    FederationBarkArtifact, FederationBarkArtifactKind, FederationBarkCertifiedReceiveHtlcs,
    FederationBarkDigest, FederationBarkExternalId, FederationBarkFrostApplicationConfig,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkOperatorStatus, FederationBarkPolicyConfig, FederationBarkProjectionSnapshot,
    FederationBarkProtocolStatus, FederationBarkReceiveIntent, FederationBarkReceivePrepared,
    FederationBarkReceiveSecretBinding, FederationBarkReceiveSecretCoverage,
    FederationBarkSemanticPhase, FederationBarkVtxoId, FederationBarkVtxoInput,
    FederationBarkVtxoLocator, FederationBarkVtxoOutput, FederationBarkVtxoRecord,
    FederationBarkVtxoState, FederationBarkVtxoStatusCount, FederationBarkVtxoTransition,
    FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU, FEDERATION_BARK_OPERATION_VERSION,
    FEDERATION_BARK_OPERATOR_STATUS_VERSION, FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkCustodyClient, FederatedBarkLimits,
    FederatedBarkLocalConfig, FederatedBarkMaintenanceControls, FederatedBarkReceiveSecretStore,
    FederatedBarkScope, ValidatedFederatedBarkConfig,
};

/// Whether member-local Bark state can serve an accepted frontier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedBarkReadiness {
    /// Local projection is valid and at or beyond the requested frontier.
    Ready,
    /// Local projection is valid but has not replayed the requested frontier.
    BehindConsensus,
    /// Local state or a required dependency is unavailable.
    Degraded,
}

impl FederatedBarkReadiness {
    /// Stable operator and metrics label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::BehindConsensus => "behind_consensus",
            Self::Degraded => "degraded",
        }
    }
}

/// Bounded reason for degraded operation; never an external error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederatedBarkDegradedReason {
    /// Bark's member-local execution wallet is unavailable.
    WalletUnavailable,
    /// Deterministic accepted-state projection is invalid.
    ProjectionInvalid,
    /// The selected participant set cannot satisfy the operation.
    ParticipantsUnavailable,
    /// Failed outgoing value requires accepted revocation or emergency exit.
    RecoveryRequired,
}

impl FederatedBarkDegradedReason {
    /// Stable redacted operator label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WalletUnavailable => "wallet_unavailable",
            Self::ProjectionInvalid => "projection_invalid",
            Self::ParticipantsUnavailable => "participants_unavailable",
            Self::RecoveryRequired => "recovery_required",
        }
    }
}

/// Public, redacted runtime status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedBarkRuntimeStatus {
    /// Current readiness for the requested frontier.
    pub readiness: FederatedBarkReadiness,
    /// Highest locally replayed accepted frontier.
    pub local_frontier: u64,
    /// Bounded degraded reason, if any.
    pub degraded_reason: Option<FederatedBarkDegradedReason>,
}

/// Consensus transitions produced by one accepted incoming claim execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederatedBarkReceiveClaimOutcome {
    /// The preimage boundary and spendable output were both proven locally.
    Claimed {
        /// Point-of-no-return transition for the accepted inputs.
        preimage_disclosed: FederationBarkActionTransition,
        /// Exact locally verified result finalized after the disclosure
        /// transition itself is accepted.
        result: Box<FederatedBarkReceiveClaimResult>,
    },
    /// The point of no return is durable but the Ark result is ambiguous.
    RecoveryRequired {
        /// Transition that prevents the receive from reverting to unpaid.
        preimage_disclosed: FederationBarkActionTransition,
    },
}

/// Consensus-selected execution fields for an exact receive-claim proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedBarkReceiveClaimProposal {
    /// Member authorized to execute the accepted attempt.
    pub actor: MemberId,
    /// Sorted eligible FROST signer set.
    pub signer_set: Vec<MemberId>,
    /// Non-zero custody attempt.
    pub attempt: u32,
    /// Consensus-allocated locator for the claimed output.
    pub output_locator: FederationBarkVtxoLocator,
}

/// Consensus-selected non-economic fields for a pure outgoing plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedBarkSendProposal {
    /// Member authorized to execute the accepted first attempt.
    pub actor: MemberId,
    /// Sorted eligible FROST signer set.
    pub signer_set: Vec<MemberId>,
    /// Non-zero custody attempt.
    pub attempt: u32,
    /// Absolute outgoing HTLC expiry derived from Bark's live Ark view.
    pub expires_at: u32,
}

/// One selected member's terminal check for an accepted outgoing Lightning payment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederatedBarkSendTerminalStatus {
    /// Bark has not observed a terminal result yet.
    Pending,
    /// The Ark server returned typed `NotFound`, so a selected fallback may
    /// submit the exact idempotent request fixed by accepted HTLC state.
    Missing,
    /// Bark verified the invoice preimage and constructed the exact success
    /// transition authorized by the accepted send plan.
    Succeeded(Box<FederationBarkActionTransition>),
    /// Bark reported terminal failure and constructed this member's independent
    /// failure observation. Federation failure still requires threshold
    /// matching observations.
    Failed(FederationBarkMemberObservation),
}

/// Result of driving one already accepted refresh plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederatedBarkRefreshOutcome {
    /// The Ark round remains unconfirmed and must be retried by the same plan.
    Pending,
    /// Exact confirmed replacement value ready for consensus admission.
    Completed(Box<FederationBarkActionTransition>),
}

/// Result of driving one already accepted emergency-exit plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FederatedBarkExitOutcome {
    /// Registration or publication is still pending.
    Pending,
    /// Exact final-drain publication ready for consensus admission.
    Published(Box<FederationBarkActionTransition>),
}

/// Exact cryptographic claim result retained between the two ordered result
/// transitions. Fields remain private so callers cannot forge terminal value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedBarkReceiveClaimResult {
    plan_operation_id: FederationOperationId,
    action_id: FederationBarkActionId,
    quote: String,
    payment_hash: FederationBarkDigest,
    attempt: u32,
    signer_set: Vec<MemberId>,
    input_transitions: Vec<FederationBarkVtxoTransition>,
    output_transitions: Vec<FederationBarkVtxoTransition>,
    artifacts: Vec<FederationBarkArtifact>,
    result_hash: FederationBarkDigest,
    amount: cdk_common::Amount,
}

impl FederatedBarkReceiveClaimResult {
    /// Exact output value already validated by Bark while executing the claim.
    #[must_use]
    pub const fn amount(&self) -> cdk_common::Amount {
        self.amount
    }
}

#[derive(Debug, Clone)]
struct AcceptedCustodyAttempt {
    entry: FederationJournalEntry,
    attempt: u32,
    signer_set: Vec<MemberId>,
}

struct ExitTransitionAuthorization<'a> {
    phase: FederationBarkSemanticPhase,
    attempt: u32,
    signer_set: &'a [MemberId],
}

#[derive(Debug, Clone, Copy)]
struct RuntimeHealth {
    local_frontier: u64,
    degraded_reason: Option<FederatedBarkDegradedReason>,
}

/// Restricted member-local Bark execution runtime.
pub struct FederatedBarkRuntime {
    scope: FederatedBarkScope,
    member_id: MemberId,
    federation: FederationConfig,
    application: FederationBarkFrostApplicationConfig,
    limits: FederatedBarkLimits,
    maintenance: FederatedBarkMaintenanceControls,
    wallet: FederationWallet,
    custody: Arc<CdkFrostVtxoCustody>,
    receive_secrets: Arc<FederatedBarkReceiveSecretStore>,
    health: RwLock<RuntimeHealth>,
}

impl fmt::Debug for FederatedBarkRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkRuntime")
            .field("scope", &self.scope)
            .field("limits", &self.limits)
            .field("wallet", &"<member-local Bark projection>")
            .field("receive_secrets", &"<encrypted member-local store>")
            .field("health", &"<bounded runtime status>")
            .finish()
    }
}

impl FederatedBarkRuntime {
    /// Validate all bindings and open a fresh or existing FROST-custodied Bark wallet.
    pub async fn open(
        federation_enabled: bool,
        federation: &FederationConfig,
        member_id: MemberId,
        local: FederatedBarkLocalConfig,
        custody_client: Arc<dyn FederatedBarkCustodyClient>,
    ) -> Result<Arc<Self>, Error> {
        let validated =
            ValidatedFederatedBarkConfig::validate(federation_enabled, federation, &local)?;
        if !federation.members.contains_key(&member_id) {
            return Err(Error::InvalidPublicConfig);
        }
        let receive_secrets = Arc::new(FederatedBarkReceiveSecretStore::open(
            &local.receive_secret_store.database_path,
            Arc::clone(&local.receive_secret_store.encryption_key),
        )?);
        let custody = Arc::new(CdkFrostVtxoCustody::with_client(
            validated.clone(),
            federation.clone(),
            custody_client,
        ));
        validated.validate_descriptor(&custody.descriptor())?;
        let wallet_custody: Arc<dyn VtxoCustody> = custody.clone();
        let expected_operational_fingerprint = local.operational_seed.fingerprint();
        let expected_descriptor = custody.descriptor();
        let expected_server = validated.ark_server_public_key();
        let limits = local.limits;
        let maintenance = local.maintenance;
        let wallet = Wallet::open_with_custody(
            validated.network(),
            local.operational_seed,
            wallet_custody,
            local.bark,
            OpenWalletArgs {
                run_daemon: false,
                datadir: Some(local.data_directory),
                create_if_not_exists: local.create_if_missing,
                create_without_server: false,
                ..OpenWalletArgs::default()
            },
        )
        .await
        .map_err(|_| Error::WalletOpenFailed)?;
        let properties = wallet
            .properties()
            .await
            .map_err(|_| Error::WalletPropertiesUnavailable)?;
        validate_wallet_properties(
            &properties,
            validated.network(),
            expected_operational_fingerprint,
            &expected_descriptor,
            expected_server,
        )?;
        let ark_info = wallet
            .require_ark_info()
            .await
            .map_err(|_| Error::ArkCapabilityMismatch)?;
        let minimum_outgoing_fee_bound = validated
            .policy()
            .maximum_outgoing_fee(cdk_common::Amount::from(limits.minimum_sat()))
            .map_err(|_| Error::ArkCapabilityMismatch)?;
        if ark_info.network != validated.network()
            || ark_info.server_pubkey != expected_server
            || ark_info.ln_receive_anti_dos_required
            || ark_info.fees.lightning_receive.base_fee != bitcoin::Amount::ZERO
            || ark_info.fees.lightning_receive.ppm.0 != 0
            || u32::from(ark_info.max_user_invoice_cltv_delta)
                < validated.policy().minimum_incoming_cltv_delta
            || u32::from(ark_info.htlc_send_expiry_delta)
                < validated.policy().minimum_outgoing_cltv_delta
            || ark_info.fees.lightning_send.base_fee.to_sat()
                > validated.policy().maximum_outgoing_base_fee_sat
            || ark_info.fees.lightning_send.min_fee.to_sat() > u64::from(minimum_outgoing_fee_bound)
            || ark_info
                .fees
                .lightning_send
                .ppm_expiry_table
                .iter()
                .any(|entry| entry.ppm.0 > u64::from(validated.policy().maximum_outgoing_fee_ppm))
            || ark_info
                .max_vtxo_amount
                .is_some_and(|maximum| maximum.to_sat() < limits.maximum_sat())
        {
            return Err(Error::ArkCapabilityMismatch);
        }
        let wallet = FederationWallet::new(&wallet)
            .await
            .map_err(|_| Error::FederationFacadeUnavailable)?;
        Ok(Arc::new(Self {
            scope: validated.scope(),
            member_id,
            federation: federation.clone(),
            application: validated.application().clone(),
            limits,
            maintenance,
            wallet,
            custody,
            receive_secrets,
            health: RwLock::new(RuntimeHealth {
                local_frontier: 0,
                degraded_reason: None,
            }),
        }))
    }

    /// Immutable accepted runtime scope.
    #[must_use]
    pub const fn scope(&self) -> FederatedBarkScope {
        self.scope
    }

    /// Startup-validated satoshi payment limits.
    #[must_use]
    pub const fn limits(&self) -> FederatedBarkLimits {
        self.limits
    }

    /// Member-local automation switches; consensus safety bounds remain fixed.
    #[must_use]
    pub const fn maintenance_controls(&self) -> FederatedBarkMaintenanceControls {
        self.maintenance
    }

    /// Accepted Bitcoin network for all Bark invoices and VTXOs.
    #[must_use]
    pub fn network(&self) -> bitcoin::Network {
        self.bark_application().network
    }

    /// Accepted conservative Bark fee and expiry policy.
    #[must_use]
    pub fn policy(&self) -> &FederationBarkPolicyConfig {
        &self.bark_application().policy
    }

    fn bark_application(&self) -> &FederationBarkFrostApplicationConfig {
        &self.application
    }

    /// Local federation member represented by this runtime.
    #[must_use]
    pub const fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Construct the sole deterministic consensus transition that can replace
    /// an unavailable custody signer set without changing the accepted Bark
    /// action or its external execution identity.
    pub fn advance_signing_attempt(
        &self,
        action: &FederationBarkActionRecord,
        accepted_plan: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_action_plan(accepted_plan)?;
        if action.action_id != plan.action_id
            || action.actor != plan.actor
            || plan.actor != self.member_id
            || action.plan_operation_id != Some(accepted_plan.operation_id)
        {
            return Err(Error::InvalidCustodyAuthorization);
        }
        let attempt = action
            .attempt
            .checked_add(1)
            .ok_or(Error::InvalidCustodyAuthorization)?;
        let signer_set = federation_bark_signer_set_for_attempt(&self.federation, attempt)
            .map_err(|_| Error::InvalidCustodyAuthorization)?;
        FederationBarkActionTransition::signing_attempt_advanced(
            accepted_plan.operation_id,
            action.latest_operation_id,
            &plan,
            attempt,
            signer_set,
        )
        .map_err(|_| Error::InvalidCustodyAuthorization)
    }

    /// Encrypted member-local receive-secret store.
    #[must_use]
    pub fn receive_secret_store(&self) -> Arc<FederatedBarkReceiveSecretStore> {
        Arc::clone(&self.receive_secrets)
    }

    /// Read the member-local chain height without exposing source details.
    pub async fn chain_tip_height(&self) -> Result<u32, Error> {
        self.wallet
            .chain_tip_height()
            .await
            .map_err(|_| Error::InvalidChainSource)
    }

    /// Read Bark's standard outgoing expiry from the current chain tip and
    /// the Ark server's advertised send delta.
    pub async fn lightning_send_expiry_height(&self) -> Result<u32, Error> {
        self.wallet
            .lightning_send_expiry_height()
            .await
            .map_err(|_| Error::InvalidChainSource)
    }

    /// Purely derive one accepted locator without database or network I/O.
    pub fn derive_public_key(&self, index: u32) -> Result<PublicKey, Error> {
        self.wallet
            .derive_locator(VtxoKeyLocator { index })
            .map(|derived| derived.public_key)
            .map_err(|_| Error::PublicKeyDerivationFailed)
    }

    /// Select exact spendable VTXOs and construct a complete outgoing plan
    /// using only accepted public state. This performs no database mutation,
    /// chain lookup, custody operation, clock read, or Ark request.
    pub fn propose_lightning_send(
        &self,
        quote: &MeltQuote,
        predecessor_operation_id: FederationOperationId,
        projection: &FederationBarkProjectionSnapshot,
        mut proposal: FederatedBarkSendProposal,
    ) -> Result<FederationBarkActionPlan, Error> {
        if quote.state != MeltQuoteState::Pending {
            return Err(Error::SendPlanInvalid);
        }
        let (invoice, amount, fee) = self.validate_send_quote(quote)?;
        if proposal.attempt == 0 || !self.federation.members.contains_key(&proposal.actor) {
            return Err(Error::SendPlanInvalid);
        }
        proposal.signer_set.sort();
        proposal.signer_set.dedup();
        let application = self.bark_application();
        if proposal.signer_set.len() < usize::from(application.threshold)
            || proposal
                .signer_set
                .iter()
                .any(|member| !self.federation.members.contains_key(member))
        {
            return Err(Error::SendPlanInvalid);
        }

        let target = amount.checked_add(fee).ok_or(Error::SendPlanInvalid)?;
        let mut selected = Vec::new();
        let mut selected_amount = cdk_common::Amount::ZERO;
        let mut minimum_expiry = u32::MAX;
        for vtxo in projection
            .vtxos
            .iter()
            .filter(|vtxo| vtxo.state == FederationBarkVtxoState::Spendable)
        {
            selected.push(FederationBarkVtxoInput {
                vtxo_id: vtxo.vtxo_id,
                locator: vtxo.locator,
                amount: vtxo.amount,
            });
            selected_amount = selected_amount
                .checked_add(vtxo.amount)
                .ok_or(Error::SendPlanInvalid)?;
            minimum_expiry = minimum_expiry.min(vtxo.expiry_height);
            if selected_amount >= target {
                break;
            }
        }
        if selected_amount < target || selected.is_empty() {
            return Err(Error::InsufficientAcceptedBalance);
        }
        selected.sort();
        validate_federation_bark_lightning_send_expiry(minimum_expiry, proposal.expires_at)
            .map_err(|_| Error::SendPlanInvalid)?;
        let output_index = projection
            .derivation_allocations
            .iter()
            .map(|allocation| allocation.locator.index)
            .max()
            .map_or(Ok(0), |index| {
                index.checked_add(1).ok_or(Error::SendPlanInvalid)
            })?;
        let revocation_index = output_index
            .checked_add(1)
            .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
            .ok_or(Error::SendPlanInvalid)?;
        let output_locator = FederationBarkVtxoLocator::vtxo_custody(output_index);
        let revocation_locator = FederationBarkVtxoLocator::vtxo_custody(revocation_index);
        let action_id = FederationBarkActionId::derive(
            self.scope.federation_id(),
            predecessor_operation_id,
            FederationBarkSemanticPhase::SendPlanned,
            0,
        )
        .map_err(|_| Error::SendPlanInvalid)?;
        let external_id = FederationBarkExternalId::derive_lightning_send(
            action_id,
            predecessor_operation_id,
            proposal.attempt,
        )
        .map_err(|_| Error::SendPlanInvalid)?;
        let payment_hash =
            FederationBarkDigest::from_bytes(*invoice.payment_hash().as_byte_array())
                .map_err(|_| Error::SendPlanInvalid)?;
        let change = selected_amount
            .checked_sub(target)
            .ok_or(Error::SendPlanInvalid)?;
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id,
            quote: Some(quote.id.to_string()),
            kind: FederationBarkActionKind::LightningSend,
            actor: proposal.actor,
            predecessor_operation_id,
            payment_hash: Some(payment_hash),
            inputs: selected,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount: change,
                artifact_hash: FederationBarkDigest::from_bytes([1; 32])
                    .map_err(|_| Error::SendPlanInvalid)?,
            }],
            revocation_locator: Some(revocation_locator),
            external_amount: amount,
            fee_amount: fee,
            fee_bound: fee,
            certified_receive_htlcs: None,
            artifacts: Vec::new(),
            external_id,
            signer_set: proposal.signer_set,
            attempt: proposal.attempt,
            expires_at: u64::from(proposal.expires_at),
        };
        plan.bind_lightning_send_artifacts(invoice)
            .map_err(|_| Error::SendPlanInvalid)?;
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::SendPlanInvalid)?;
        Ok(plan)
    }

    /// Reconstruct Bark's exact member-local checkpoint from one accepted
    /// outgoing plan. Inputs may still be spendable during proposal validation
    /// or already reserved by this action during replay.
    pub fn validate_lightning_send_plan(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningSendPlan, Error> {
        let (invoice, amount, fee) = self.validate_send_quote(quote)?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        operation.validate().map_err(|_| Error::SendPlanInvalid)?;
        if plan.kind != FederationBarkActionKind::LightningSend
            || plan.quote.as_deref() != Some(quote.id.to_string().as_str())
            || plan.external_amount != amount
            || plan.fee_amount != fee
            || plan.fee_bound != fee
            || plan.payment_hash
                != Some(
                    FederationBarkDigest::from_bytes(*invoice.payment_hash().as_byte_array())
                        .map_err(|_| Error::SendPlanInvalid)?,
                )
        {
            return Err(Error::SendPlanInvalid);
        }
        let output = plan.outputs.first().ok_or(Error::SendPlanInvalid)?;
        let revocation_locator = plan.revocation_locator.ok_or(Error::SendPlanInvalid)?;
        let mut inputs = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let accepted = projection
                .vtxos
                .iter()
                .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                .ok_or(Error::SendPlanInvalid)?;
            if accepted.amount != input.amount
                || accepted.locator != input.locator
                || !matches!(
                    accepted.state,
                    FederationBarkVtxoState::Spendable
                        | FederationBarkVtxoState::Reserved
                        | FederationBarkVtxoState::PendingSpend
                        | FederationBarkVtxoState::Spent
                )
                || (matches!(
                    accepted.state,
                    FederationBarkVtxoState::Reserved | FederationBarkVtxoState::PendingSpend
                ) && accepted.reservation_action_id != Some(plan.action_id))
            {
                return Err(Error::SendPlanInvalid);
            }
            inputs.push(FederationSendInput {
                vtxo_id: bark::ark::VtxoId::from_slice(&input.vtxo_id.to_bytes())
                    .map_err(|_| Error::SendPlanInvalid)?,
                amount: bitcoin::Amount::from_sat(u64::from(input.amount)),
                locator: VtxoKeyLocator {
                    index: input.locator.index,
                },
            });
        }
        let action_id = FederationActionId::from_bytes(plan.action_id.to_bytes())
            .map_err(|_| Error::SendPlanInvalid)?;
        let request_commitment = FederationRequestCommitment::from_bytes(
            operation
                .commitment()
                .map_err(|_| Error::SendPlanInvalid)?
                .to_bytes(),
        )
        .map_err(|_| Error::SendPlanInvalid)?;
        let bark_invoice = Invoice::Bolt11(invoice.clone());
        let candidate = FederationLightningSendCandidate {
            action_id,
            request_commitment,
            invoice: bark_invoice.clone(),
            original_payment_method: bark_invoice.into(),
            inputs,
            payment_amount: bitcoin::Amount::from_sat(u64::from(amount)),
            fee: bitcoin::Amount::from_sat(u64::from(fee)),
            htlc_locator: VtxoKeyLocator {
                index: output.locator.index,
            },
            revocation_locator: VtxoKeyLocator {
                index: revocation_locator.index,
            },
            htlc_expiry: u32::try_from(plan.expires_at).map_err(|_| Error::SendPlanInvalid)?,
            custody_attempt: plan.attempt,
            accepted_at_unix_seconds: i64::try_from(quote.created_time)
                .map_err(|_| Error::SendPlanInvalid)?,
        };
        let bark_plan = self
            .wallet
            .plan_lightning_send(candidate)
            .map_err(|_| Error::SendPlanInvalid)?;
        self.wallet
            .validate_lightning_send_plan(&bark_plan)
            .map_err(|_| Error::SendPlanInvalid)?;
        Ok(bark_plan)
    }

    /// Apply or replay an accepted outgoing plan into Bark without starting
    /// its executor or making an Ark request.
    pub async fn apply_lightning_send_plan(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        for input in &plan.inputs {
            let accepted = projection
                .vtxos
                .iter()
                .find(|vtxo| vtxo.vtxo_id == input.vtxo_id)
                .ok_or(Error::SendProjectionFailed)?;
            let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(
                &accepted.artifact.bytes,
            )
            .map_err(|_| Error::SendProjectionFailed)?;
            if vtxo.serialize() != accepted.artifact.bytes
                || vtxo.id().to_bytes() != accepted.vtxo_id.to_bytes()
                || vtxo.amount().to_sat() != u64::from(accepted.amount)
                || vtxo.expiry_height() != accepted.expiry_height
                || vtxo.server_pubkey().serialize() != self.bark_application().ark_server_public_key
            {
                return Err(Error::SendProjectionFailed);
            }
            self.wallet
                .import_vtxo(&FederationVtxoImport {
                    locator: VtxoKeyLocator {
                        index: accepted.locator.index,
                    },
                    vtxo,
                    // Import the accepted origin before applying its accepted
                    // reservation. `apply_lightning_send` performs the exact
                    // spendable -> action-lock transition atomically in Bark.
                    state: BarkFederationVtxoState::Spendable,
                })
                .await
                .map_err(|_| Error::SendProjectionFailed)?;
        }
        self.wallet
            .apply_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendProjectionFailed)?;
        Ok(())
    }

    /// Execute only the HTLC-cosign phase of one applied outgoing plan and
    /// return the exact semantic transition candidate.
    pub async fn execute_lightning_send_htlcs(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        if plan.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        let plan_operation = FederationBarkOperation::ActionPlan(plan.clone());
        let plan_commitment = plan_operation
            .commitment()
            .map_err(|_| Error::SendExecutionFailed)?;
        let custody_attempt = accepted_custody_attempt(accepted_plan, accepted_authorization)?;
        self.custody
            .activate_send(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                plan_commitment.to_bytes().to_vec(),
            )
            .map_err(|_| Error::SendExecutionFailed)?;
        let result = self.wallet.execute_lightning_send_htlcs(&bark_plan).await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_send();
        let evidence =
            result.map_err(|error| classify_send_execution_error(error, signer_unavailable))?;
        send_htlc_transition(
            self.member_id,
            accepted_plan,
            &plan,
            projection,
            &evidence,
            plan_commitment,
            &custody_attempt,
        )
    }

    /// Replay one accepted HTLC result into member-local Bark state without a
    /// second Ark cosign request.
    pub async fn apply_lightning_send_htlcs(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let transition = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &transition)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        let evidence = send_htlc_evidence_from_transition(&plan, &transition)?;
        self.wallet
            .apply_lightning_send_htlcs(&bark_plan, &evidence)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(())
    }

    /// Initiate the exact already-cosigned outgoing payment. The coordinator
    /// selects the accepted actor first and may later authorize a fallback;
    /// Ark's persisted identity makes concurrent or delayed retries exact.
    pub async fn initiate_lightning_send(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .initiate_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendExecutionFailed)?;
        Ok(())
    }

    /// Poll one selected member's accepted payment exactly once and preserve
    /// Bark's typed terminal result. A success carries an objective preimage
    /// bound to the accepted invoice, so a deterministic fallback may publish
    /// it after actor loss without receiving initiation authority. A failed
    /// result creates only one member observation; threshold matching evidence
    /// remains required before the federation can authorize revocation.
    pub async fn observe_lightning_send_terminal(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<FederatedBarkSendTerminalStatus, Error> {
        if observation_epoch == 0 {
            return Err(Error::SendReconciliationFailed);
        }
        let plan = accepted_send_plan(accepted_plan)?;
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        let status = self
            .wallet
            .poll_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        match status {
            FederationLightningSendStatus::Pending => Ok(FederatedBarkSendTerminalStatus::Pending),
            FederationLightningSendStatus::Missing => Ok(FederatedBarkSendTerminalStatus::Missing),
            FederationLightningSendStatus::Succeeded(preimage) => Ok(
                FederatedBarkSendTerminalStatus::Succeeded(Box::new(payment_succeeded_transition(
                    plan.actor,
                    accepted_plan,
                    accepted_htlcs,
                    &plan,
                    &htlcs,
                    projection,
                    preimage,
                )?)),
            ),
            FederationLightningSendStatus::Failed => Ok(FederatedBarkSendTerminalStatus::Failed(
                self.lightning_send_failure_observation(&plan, &htlcs, observation_epoch)?,
            )),
        }
    }

    /// Apply an accepted success proof locally and construct this member's
    /// independently bound success observation for the existing melt saga.
    pub async fn apply_lightning_send_succeeded(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_success: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        if observation_epoch == 0 {
            return Err(Error::SendReconciliationFailed);
        }
        let plan = accepted_send_plan(accepted_plan)?;
        let success = accepted_send_transition(
            accepted_success,
            FederationBarkSemanticPhase::PaymentSucceeded,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &success)?;
        let preimage = send_success_preimage(&plan, &success)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .apply_lightning_send_succeeded(&bark_plan, preimage)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: plan.payment_hash,
            status: FederationBarkObservationStatus::Succeeded,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash: success.result_hash,
        })
    }

    fn lightning_send_failure_observation(
        &self,
        plan: &FederationBarkActionPlan,
        htlcs: &FederationBarkActionTransition,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        let (_, result_hash) = payment_failed_result(plan, htlcs.result_hash)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: plan.payment_hash,
            status: FederationBarkObservationStatus::Failed,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash,
        })
    }

    /// Construct the terminal-payment-failure transition after the accepted
    /// distinct-member evidence threshold is present.
    pub fn finalize_lightning_send_failure(
        &self,
        accepted_plan: &FederationJournalEntry,
        accepted_htlcs: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        if plan.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let htlcs = accepted_send_transition(
            accepted_htlcs,
            FederationBarkSemanticPhase::SendHtlcsCosigned,
        )?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &htlcs)?;
        let (artifact, result_hash) = payment_failed_result(&plan, htlcs.result_hash)?;
        Ok(FederationBarkActionTransition {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: plan.quote.clone(),
            phase: FederationBarkSemanticPhase::PaymentFailed,
            actor: self.member_id,
            plan_operation_id: accepted_plan.operation_id,
            predecessor_operation_id: accepted_htlcs.operation_id,
            payment_hash: plan.payment_hash,
            attempt: htlcs.attempt,
            signer_set: htlcs.signer_set.clone(),
            request_hash: htlcs.result_hash,
            result_hash,
            evidence_certificate: None,
            external_id: plan.external_id,
            artifacts: vec![artifact],
            vtxo_transitions: Vec::new(),
        })
    }

    /// Replay accepted terminal failure into Bark while retaining all HTLC
    /// value in the revocation-required checkpoint.
    pub async fn apply_lightning_send_failed(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_failed: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let failed =
            accepted_send_transition(accepted_failed, FederationBarkSemanticPhase::PaymentFailed)?;
        validate_send_transition_binding(&self.federation, accepted_plan, &plan, &failed)?;
        validate_payment_failed_result(&plan, &failed)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, &plan)?;
        self.wallet
            .apply_lightning_send_failed(&bark_plan)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(())
    }

    /// Build the one exact recovery plan for the accepted failed payment.
    pub fn propose_failed_send_revocation(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        send_plan_entry: &FederationJournalEntry,
        accepted_failed: &FederationJournalEntry,
    ) -> Result<FederationBarkActionPlan, Error> {
        let send = accepted_send_plan(send_plan_entry)?;
        if send.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let failed =
            accepted_send_transition(accepted_failed, FederationBarkSemanticPhase::PaymentFailed)?;
        validate_send_transition_binding(&self.federation, send_plan_entry, &send, &failed)?;
        validate_payment_failed_result(&send, &failed)?;
        let mut records = projection
            .vtxos
            .iter()
            .filter(|record| {
                record.state == FederationBarkVtxoState::PendingSpend
                    && record.reservation_action_id == Some(send.action_id)
            })
            .collect::<Vec<_>>();
        records.sort_by_key(|record| record.vtxo_id);
        if records.is_empty() {
            return Err(Error::SendExecutionFailed);
        }
        let inputs = records
            .iter()
            .map(|record| FederationBarkVtxoInput {
                vtxo_id: record.vtxo_id,
                locator: record.locator,
                amount: record.amount,
            })
            .collect::<Vec<_>>();
        let amount = inputs
            .iter()
            .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                sum.checked_add(input.amount)
                    .ok_or(Error::SendExecutionFailed)
            })?;
        if amount
            != send
                .external_amount
                .checked_add(send.fee_amount)
                .ok_or(Error::SendExecutionFailed)?
        {
            return Err(Error::SendExecutionFailed);
        }
        let output_locator = send.revocation_locator.ok_or(Error::SendExecutionFailed)?;
        let request_bytes = failed_send_revocation_request_bytes(
            send.action_id,
            accepted_failed.operation_id,
            send.payment_hash.ok_or(Error::SendExecutionFailed)?,
            output_locator,
            &inputs,
        )?;
        let request_artifact =
            bark_artifact(FederationBarkArtifactKind::ArkRequest, request_bytes)?;
        let mut artifacts = records
            .iter()
            .map(|record| record.artifact.clone())
            .collect::<Vec<_>>();
        artifacts.push(request_artifact.clone());
        artifacts.sort();
        artifacts.dedup();
        let plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: send.action_id,
            quote: send.quote.clone(),
            kind: FederationBarkActionKind::FailedSendRevocation,
            actor: send.actor,
            predecessor_operation_id: accepted_failed.operation_id,
            payment_hash: send.payment_hash,
            inputs,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount,
                artifact_hash: request_artifact.hash,
            }],
            revocation_locator: None,
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: cdk_common::Amount::ZERO,
            fee_bound: cdk_common::Amount::ZERO,
            certified_receive_htlcs: None,
            artifacts,
            external_id: FederationBarkExternalId::derive_failed_send_revocation(
                send.action_id,
                accepted_failed.operation_id,
                failed.attempt,
            )
            .map_err(|_| Error::SendExecutionFailed)?,
            signer_set: failed.signer_set.clone(),
            attempt: failed.attempt,
            expires_at: send.expires_at,
        };
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::SendExecutionFailed)?;
        Ok(plan)
    }

    /// Execute the accepted revocation plan once and return its exact semantic
    /// result candidate.
    pub async fn execute_failed_send_revocation(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        send_plan_entry: &FederationJournalEntry,
        revocation_plan_entry: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let send = accepted_send_plan(send_plan_entry)?;
        let revocation = accepted_revocation_plan(revocation_plan_entry)?;
        if revocation.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let bark_send = self.validate_lightning_send_plan(quote, projection, &send)?;
        let bark_revocation = self.bark_revocation_plan(&bark_send, &revocation)?;
        let operation = FederationBarkOperation::ActionPlan(revocation.clone());
        let commitment = operation
            .commitment()
            .map_err(|_| Error::SendExecutionFailed)?;
        let custody_attempt =
            accepted_custody_attempt(revocation_plan_entry, accepted_authorization)?;
        self.custody
            .activate_revocation(
                revocation_plan_entry.clone(),
                custody_attempt.entry.clone(),
                commitment.to_bytes().to_vec(),
            )
            .map_err(|_| Error::SendExecutionFailed)?;
        let result = self
            .wallet
            .execute_lightning_send_revocation(&bark_send, &bark_revocation)
            .await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_revocation();
        let evidence =
            result.map_err(|error| classify_send_execution_error(error, signer_unavailable))?;
        revocation_completed_transition(
            self.member_id,
            revocation_plan_entry,
            &revocation,
            projection,
            &evidence,
            commitment,
            &custody_attempt,
        )
    }

    /// Apply accepted recovered value locally and return the independently
    /// validated failure observation that may safely release Cashu proofs.
    pub async fn apply_failed_send_revocation(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        send_plan_entry: &FederationJournalEntry,
        revocation_plan_entry: &FederationJournalEntry,
        accepted_completed: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<FederationBarkMemberObservation, Error> {
        if observation_epoch == 0 {
            return Err(Error::SendReconciliationFailed);
        }
        let send = accepted_send_plan(send_plan_entry)?;
        let revocation = accepted_revocation_plan(revocation_plan_entry)?;
        let completed = accepted_send_transition(
            accepted_completed,
            FederationBarkSemanticPhase::RevocationCompleted,
        )?;
        if completed.plan_operation_id != revocation_plan_entry.operation_id
            || completed.action_id != revocation.action_id
            || completed.quote != revocation.quote
            || completed.payment_hash != revocation.payment_hash
            || completed.predecessor_operation_id != revocation_plan_entry.operation_id
        {
            return Err(Error::SendReconciliationFailed);
        }
        let bark_send = self.validate_lightning_send_plan(quote, projection, &send)?;
        let bark_revocation = self.bark_revocation_plan(&bark_send, &revocation)?;
        let evidence = revocation_evidence_from_transition(&revocation, &completed)?;
        self.wallet
            .apply_lightning_send_revocation(&bark_send, &bark_revocation, &evidence)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: send.action_id,
            quote: send.quote.clone(),
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::SendReconciliationFailed)?,
            payment_hash: send.payment_hash,
            status: FederationBarkObservationStatus::Failed,
            amount: send.external_amount,
            fee: send.fee_amount,
            result_hash: completed.result_hash,
        })
    }

    fn bark_revocation_plan(
        &self,
        send_plan: &FederationLightningSendPlan,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningSendRevocationPlan, Error> {
        if plan.kind != FederationBarkActionKind::FailedSendRevocation
            || plan.action_id.to_bytes() != send_plan.action_id.to_bytes()
            || plan
                .payment_hash
                .map(|payment_hash| payment_hash.to_bytes().to_vec())
                != Some(send_plan.invoice.payment_hash().to_vec())
            || plan.outputs.len() != 1
        {
            return Err(Error::SendPlanInvalid);
        }
        let output = plan.outputs.first().ok_or(Error::SendPlanInvalid)?;
        if output.locator.index != send_plan.revocation_key.locator.index {
            return Err(Error::SendPlanInvalid);
        }
        let mut input_amount = cdk_common::Amount::ZERO;
        let mut input_vtxo_ids = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            input_amount = input_amount
                .checked_add(input.amount)
                .ok_or(Error::SendPlanInvalid)?;
            input_vtxo_ids.push(
                bark::ark::VtxoId::from_slice(&input.vtxo_id.to_bytes())
                    .map_err(|_| Error::SendPlanInvalid)?,
            );
        }
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let commitment = operation.commitment().map_err(|_| Error::SendPlanInvalid)?;
        let mut requests = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkRequest);
        let request = requests.next().ok_or(Error::SendPlanInvalid)?;
        let expected_request = failed_send_revocation_request_bytes(
            plan.action_id,
            plan.predecessor_operation_id,
            plan.payment_hash.ok_or(Error::SendPlanInvalid)?,
            output.locator,
            &plan.inputs,
        )?;
        if requests.next().is_some()
            || request.bytes != expected_request
            || request.hash != bark_digest(&expected_request)?
            || output.artifact_hash != request.hash
            || output.amount != input_amount
        {
            return Err(Error::SendPlanInvalid);
        }
        Ok(FederationLightningSendRevocationPlan {
            action_id: send_plan.action_id,
            request_commitment: FederationRequestCommitment::from_bytes(commitment.to_bytes())
                .map_err(|_| Error::SendPlanInvalid)?,
            payment_hash: send_plan.invoice.payment_hash(),
            input_vtxo_ids,
            input_amount: bitcoin::Amount::from_sat(u64::from(input_amount)),
            output_key: send_plan.revocation_key,
            custody_attempt: plan.attempt,
            expires_at: u32::try_from(plan.expires_at).map_err(|_| Error::SendPlanInvalid)?,
        })
    }

    /// Build the deterministic earliest-expiry refresh plan once the local
    /// chain tip reaches its accepted refresh margin.
    pub async fn propose_refresh(
        &self,
        projection: &FederationBarkProjectionSnapshot,
    ) -> Result<Option<FederationBarkActionPlan>, Error> {
        let minimum_expiry = match projection
            .vtxos
            .iter()
            .filter(|record| record.state == FederationBarkVtxoState::Spendable)
            .map(|record| record.expiry_height)
            .min()
        {
            Some(expiry) => expiry,
            None => return Ok(None),
        };
        let records = federation_bark_refresh_candidates(projection);
        if records.is_empty() {
            let actual = projection
                .vtxos
                .iter()
                .filter(|record| {
                    record.state == FederationBarkVtxoState::Spendable
                        && record.expiry_height == minimum_expiry
                })
                .min_by_key(|record| record.vtxo_id)
                .map_or(0, |record| record.artifact.bytes.len());
            return Err(Error::MaintenanceAuthorizationTooLarge {
                actual,
                maximum: FEDERATION_MAX_BARK_REFRESH_INPUT_ARTIFACT_BYTES,
            });
        }
        let predecessor_operation_id = records
            .first()
            .ok_or(Error::MaintenancePlanInvalid)?
            .latest_operation_id;
        let application = self.bark_application();
        let trigger_height = minimum_expiry
            .checked_sub(application.policy.refresh_before_expiry_blocks)
            .filter(|height| *height != 0)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let emergency_height = minimum_expiry
            .checked_sub(application.policy.emergency_exit_before_expiry_blocks)
            .filter(|height| *height > trigger_height)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let action_id = FederationBarkActionId::derive(
            self.scope.federation_id(),
            predecessor_operation_id,
            FederationBarkSemanticPhase::RefreshPlanned,
            records
                .first()
                .ok_or(Error::MaintenancePlanInvalid)?
                .locator
                .index,
        )
        .map_err(|_| Error::MaintenancePlanInvalid)?;
        let bark_action_id = FederationActionId::from_bytes(action_id.to_bytes())
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let output_index = next_locator_index(projection)?;
        let output_locator = FederationBarkVtxoLocator::vtxo_custody(output_index);
        let output_key = self
            .custody
            .derive_public_key(VtxoKeyLocator {
                index: output_locator.index,
            })
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let bark_inputs = self.maintenance_inputs_from_records(&records)?;
        let preview = self
            .wallet
            .plan_refresh(
                bark_action_id,
                FederationRequestCommitment::from_bytes([1; 32])
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
                bark_inputs,
                output_key.locator,
                trigger_height,
                emergency_height,
                1,
            )
            .await
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let observation = self
            .wallet
            .observe_refresh_expiry(&preview)
            .await
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if observation.tip_height < trigger_height {
            return Ok(None);
        }
        let inputs = records
            .iter()
            .map(|record| FederationBarkVtxoInput {
                vtxo_id: record.vtxo_id,
                locator: record.locator,
                amount: record.amount,
            })
            .collect::<Vec<_>>();
        let input_amount = cdk_common::Amount::from(preview.input_amount.to_sat());
        let fee = cdk_common::Amount::from(preview.fee.to_sat());
        let fee_bound = application
            .policy
            .maximum_outgoing_fee(input_amount)
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if fee > fee_bound {
            return Err(Error::MaintenancePlanInvalid);
        }
        let actor = self
            .federation
            .members
            .keys()
            .next()
            .copied()
            .ok_or(Error::MaintenancePlanInvalid)?;
        let signer_set = self
            .federation
            .members
            .keys()
            .copied()
            .take(usize::from(application.threshold))
            .collect::<Vec<_>>();
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id,
            quote: None,
            kind: FederationBarkActionKind::Refresh,
            actor,
            predecessor_operation_id,
            payment_hash: None,
            inputs,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount: cdk_common::Amount::from(preview.output_amount.to_sat()),
                artifact_hash: FederationBarkDigest::from_bytes([1; 32])
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
            }],
            revocation_locator: None,
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: fee,
            fee_bound,
            certified_receive_htlcs: None,
            artifacts: Vec::new(),
            external_id: FederationBarkExternalId::derive_refresh(
                action_id,
                predecessor_operation_id,
                1,
            )
            .map_err(|_| Error::MaintenancePlanInvalid)?,
            signer_set,
            attempt: 1,
            expires_at: u64::from(emergency_height),
        };
        plan.bind_refresh_artifacts(
            trigger_height,
            minimum_expiry,
            records.iter().map(|record| record.artifact.clone()),
        )
        .map_err(|_| Error::MaintenancePlanInvalid)?;
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(Some(plan))
    }

    /// Report that this member's chain view has crossed an accepted refresh or
    /// exit trigger. The exact plan commitment is the quorum result hash.
    pub async fn observe_maintenance_trigger(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<Option<FederationBarkMemberObservation>, Error> {
        if observation_epoch == 0 {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(plan_entry) else {
            return Err(Error::MaintenancePlanInvalid);
        };
        let (tip_height, trigger_height, amount) = match plan.kind {
            FederationBarkActionKind::Refresh => {
                let bark_plan = self.bark_refresh_plan(projection, &plan)?;
                let observation = self
                    .wallet
                    .observe_refresh_expiry(&bark_plan)
                    .await
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?;
                let amount =
                    plan.inputs
                        .iter()
                        .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                            sum.checked_add(input.amount)
                                .ok_or(Error::MaintenanceReconciliationFailed)
                        })?;
                (observation.tip_height, observation.trigger_height, amount)
            }
            FederationBarkActionKind::EmergencyExit => {
                let bark_plan = self.bark_exit_plan(projection, &plan)?;
                let observation = self
                    .wallet
                    .observe_exit_expiry(&bark_plan)
                    .await
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?;
                (
                    observation.tip_height,
                    observation.trigger_height,
                    plan.external_amount,
                )
            }
            _ => return Err(Error::MaintenancePlanInvalid),
        };
        if tip_height < trigger_height {
            return Ok(None);
        }
        let commitment = FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        Ok(Some(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: None,
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            payment_hash: None,
            status: FederationBarkObservationStatus::Accepted,
            amount,
            fee: plan.fee_amount,
            result_hash: commitment,
        }))
    }

    /// Return whether an accepted refresh has crossed its immutable emergency
    /// escalation height in this member's chain view.
    pub async fn emergency_exit_due(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        refresh_plan_entry: &FederationJournalEntry,
    ) -> Result<bool, Error> {
        let plan =
            accepted_maintenance_plan(refresh_plan_entry, FederationBarkActionKind::Refresh)?;
        let bark_plan = self.bark_refresh_plan(projection, &plan)?;
        let observation = self
            .wallet
            .observe_refresh_expiry(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        Ok(observation.tip_height >= observation.emergency_height)
    }

    /// Apply or replay an accepted refresh plan without entering a round.
    pub async fn apply_refresh_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        self.apply_refresh_plan_for_attempt(projection, plan, plan.attempt)
            .await
            .map(|_| ())
    }

    async fn apply_refresh_plan_for_attempt(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
        custody_attempt: u32,
    ) -> Result<FederationRefreshPlan, Error> {
        let mut bark_plan = self.bark_refresh_plan(projection, plan)?;
        bark_plan.custody_attempt = custody_attempt;
        for input in &bark_plan.inputs {
            self.wallet
                .import_vtxo(&FederationVtxoImport {
                    locator: input.locator,
                    vtxo: input.vtxo.clone(),
                    state: BarkFederationVtxoState::Spendable,
                })
                .await
                .map_err(|_| Error::MaintenanceProjectionFailed)?;
        }
        self.wallet
            .apply_refresh_plan(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceProjectionFailed)?;
        Ok(bark_plan)
    }

    /// Execute only the exact accepted refresh round.
    pub async fn execute_refresh(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederatedBarkRefreshOutcome, Error> {
        let plan = accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::Refresh)?;
        if plan.actor != self.member_id {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let custody_attempt = accepted_custody_attempt(accepted_plan, accepted_authorization)?;
        let bark_plan = self
            .apply_refresh_plan_for_attempt(projection, &plan, custody_attempt.attempt)
            .await?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let commitment = operation
            .commitment()
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        self.custody
            .activate_refresh(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                commitment.to_bytes().to_vec(),
            )
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let execution = self.wallet.execute_refresh(&bark_plan).await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_refresh();
        match execution
            .map_err(|error| classify_maintenance_execution_error(error, signer_unavailable))?
        {
            FederationRefreshExecution::Pending { .. } => Ok(FederatedBarkRefreshOutcome::Pending),
            FederationRefreshExecution::Completed(evidence) => {
                let transition = refresh_completed_transition(
                    self.member_id,
                    accepted_plan,
                    &plan,
                    projection,
                    &evidence,
                    commitment,
                    &custody_attempt,
                )?;
                Ok(FederatedBarkRefreshOutcome::Completed(Box::new(transition)))
            }
        }
    }

    /// Apply a consensus-accepted confirmed refresh result locally.
    pub async fn apply_refresh_completed(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        completed_entry: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::Refresh)?;
        let transition = accepted_send_transition(
            completed_entry,
            FederationBarkSemanticPhase::RefreshCompleted,
        )?;
        validate_maintenance_transition_binding(&self.federation, plan_entry, &plan, &transition)?;
        let bark_plan = self.bark_refresh_plan(projection, &plan)?;
        let evidence = refresh_evidence_from_transition(&plan, &transition)?;
        self.wallet
            .apply_refresh_result(&bark_plan, &evidence)
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)
    }

    fn bark_refresh_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationRefreshPlan, Error> {
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if plan.kind != FederationBarkActionKind::Refresh || plan.outputs.len() != 1 {
            return Err(Error::MaintenancePlanInvalid);
        }
        let request = plan
            .refresh_request()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let mut records = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let record = projection
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or(Error::MaintenancePlanInvalid)?;
            if record.locator != input.locator
                || record.amount != input.amount
                || !matches!(
                    record.state,
                    FederationBarkVtxoState::Spendable
                        | FederationBarkVtxoState::Reserved
                        | FederationBarkVtxoState::Spent
                )
                || (record.state == FederationBarkVtxoState::Reserved
                    && record.reservation_action_id != Some(plan.action_id))
            {
                return Err(Error::MaintenancePlanInvalid);
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.vtxo_id);
        let inputs = self.maintenance_inputs_from_records(&records)?;
        let output = plan.outputs.first().ok_or(Error::MaintenancePlanInvalid)?;
        let output_key = self
            .custody
            .derive_public_key(VtxoKeyLocator {
                index: output.locator.index,
            })
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let commitment = FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(FederationRefreshPlan {
            action_id: FederationActionId::from_bytes(plan.action_id.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            request_commitment: FederationRequestCommitment::from_bytes(commitment.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            inputs,
            input_amount: bitcoin::Amount::from_sat(
                plan.inputs
                    .iter()
                    .map(|input| u64::from(input.amount))
                    .try_fold(0_u64, u64::checked_add)
                    .ok_or(Error::MaintenancePlanInvalid)?,
            ),
            output_key,
            output_amount: bitcoin::Amount::from_sat(u64::from(output.amount)),
            fee: bitcoin::Amount::from_sat(u64::from(plan.fee_amount)),
            trigger_height: request.trigger_height,
            emergency_height: request.emergency_height,
            minimum_expiry: request.minimum_expiry,
            custody_attempt: plan.attempt,
        })
    }

    /// Escalate one accepted refresh reservation into an exact federated BDK
    /// emergency-exit plan. No exit registration or signing occurs here.
    pub fn propose_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        action: &FederationBarkActionRecord,
        refresh_plan_entry: &FederationJournalEntry,
        destination: FederationAllocatedAddressRef,
        destination_script: Vec<u8>,
        minimum_confirmations: u32,
    ) -> Result<FederationBarkActionPlan, Error> {
        let refresh =
            accepted_maintenance_plan(refresh_plan_entry, FederationBarkActionKind::Refresh)?;
        let effective_phase = action.signing_retry_phase.unwrap_or(action.phase);
        let signer_set = federation_bark_signer_set_for_attempt(&self.federation, action.attempt)
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if action.action_id != refresh.action_id
            || action.action_kind != Some(FederationBarkActionKind::Refresh)
            || effective_phase != FederationBarkSemanticPhase::RefreshPlanned
            || action.plan_operation_id != Some(refresh_plan_entry.operation_id)
            || action.actor != refresh.actor
            || action.attempt < refresh.attempt
            || destination.quote != federation_bark_exit_allocation_quote(refresh.action_id)
            || minimum_confirmations == 0
        {
            return Err(Error::MaintenancePlanInvalid);
        }
        let mut records = Vec::with_capacity(refresh.inputs.len());
        for input in &refresh.inputs {
            let record = projection
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or(Error::MaintenancePlanInvalid)?;
            if record.state != FederationBarkVtxoState::Reserved
                || record.reservation_action_id != Some(refresh.action_id)
                || record.locator != input.locator
                || record.amount != input.amount
            {
                return Err(Error::MaintenancePlanInvalid);
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.vtxo_id);
        let bark_inputs = self.maintenance_inputs_from_records(&records)?;
        let input_amount =
            refresh
                .inputs
                .iter()
                .try_fold(cdk_common::Amount::ZERO, |sum, input| {
                    sum.checked_add(input.amount)
                        .ok_or(Error::MaintenancePlanInvalid)
                })?;
        let fee_bound = self
            .bark_application()
            .policy
            .maximum_outgoing_fee(input_amount)
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let minimum_recovered = input_amount
            .checked_sub(fee_bound)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let minimum_expiry = records
            .iter()
            .map(|record| record.expiry_height)
            .min()
            .ok_or(Error::MaintenancePlanInvalid)?;
        let trigger_height = minimum_expiry
            .checked_sub(
                self.bark_application()
                    .policy
                    .emergency_exit_before_expiry_blocks,
            )
            .filter(|height| *height != 0)
            .ok_or(Error::MaintenancePlanInvalid)?;
        let fee_rate = FeeRate::from_sat_per_kwu(FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU);
        let bark_plan = self
            .wallet
            .plan_exit(
                FederationActionId::from_bytes(refresh.action_id.to_bytes())
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
                FederationRequestCommitment::from_bytes([1; 32])
                    .map_err(|_| Error::MaintenancePlanInvalid)?,
                bark_inputs.clone(),
                ScriptBuf::from_bytes(destination_script.clone()),
                fee_rate,
                bitcoin::Amount::from_sat(u64::from(fee_bound)),
                trigger_height,
                minimum_confirmations,
                action.attempt,
            )
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if bark_plan.minimum_recovered.to_sat() != u64::from(minimum_recovered)
            || bark_plan.minimum_expiry != minimum_expiry
        {
            return Err(Error::MaintenancePlanInvalid);
        }
        let mut exit_transactions = Vec::new();
        for input in &bark_inputs {
            exit_transactions.extend(
                input
                    .vtxo
                    .transactions()
                    .map(|transaction| serialize_transaction(&transaction.tx)),
            );
        }
        let mut plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: refresh.action_id,
            quote: None,
            kind: FederationBarkActionKind::EmergencyExit,
            actor: action.actor,
            predecessor_operation_id: action.latest_operation_id,
            payment_hash: None,
            inputs: refresh.inputs.clone(),
            outputs: Vec::new(),
            revocation_locator: None,
            external_amount: minimum_recovered,
            fee_amount: fee_bound,
            fee_bound,
            certified_receive_htlcs: None,
            artifacts: Vec::new(),
            external_id: FederationBarkExternalId::derive_emergency_exit(
                refresh.action_id,
                action.latest_operation_id,
                action.attempt,
            )
            .map_err(|_| Error::MaintenancePlanInvalid)?,
            signer_set,
            attempt: action.attempt,
            expires_at: u64::from(minimum_expiry),
        };
        plan.bind_emergency_exit_artifacts(
            destination,
            destination_script,
            self.network(),
            FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU,
            trigger_height,
            minimum_confirmations,
            records.iter().map(|record| record.artifact.clone()),
            exit_transactions,
        )
        .map_err(|_| Error::MaintenancePlanInvalid)?;
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(plan)
    }

    /// Apply or replay an accepted emergency-exit plan without registration,
    /// signing, or publication.
    pub async fn apply_emergency_exit_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<(), Error> {
        let bark_plan = self.bark_exit_plan(projection, plan)?;
        for input in &bark_plan.inputs {
            self.wallet
                .import_vtxo(&FederationVtxoImport {
                    locator: input.locator,
                    vtxo: input.vtxo.clone(),
                    state: BarkFederationVtxoState::Spendable,
                })
                .await
                .map_err(|_| Error::MaintenanceProjectionFailed)?;
        }
        self.wallet
            .apply_exit_plan(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceProjectionFailed)?;
        Ok(())
    }

    /// Register the exact VTXOs from an accepted emergency-exit plan.
    pub async fn register_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan =
            accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::EmergencyExit)?;
        if plan.actor != self.member_id {
            return Err(Error::MaintenanceExecutionFailed);
        }
        self.apply_emergency_exit_plan(projection, &plan).await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let mut evidence = self
            .wallet
            .register_exit(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        if !matches!(
            evidence.phase,
            FederationExitPhase::Registered
                | FederationExitPhase::Published
                | FederationExitPhase::Claimable
                | FederationExitPhase::DrainPublished
                | FederationExitPhase::Confirmed
        ) {
            return Err(Error::MaintenanceExecutionFailed);
        }
        evidence.phase = FederationExitPhase::Registered;
        evidence.drain_transaction = None;
        evidence.confirmed_in = None;
        exit_transition(
            self.member_id,
            accepted_plan,
            accepted_plan.operation_id,
            &plan,
            projection,
            &evidence,
            ExitTransitionAuthorization {
                phase: FederationBarkSemanticPhase::ExitRegistered,
                attempt: plan.attempt,
                signer_set: &plan.signer_set,
            },
        )
    }

    /// Replay an accepted exit registration into member-local tracking without
    /// progressing or publishing any transaction.
    pub async fn apply_emergency_exit_registered(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::EmergencyExit)?;
        let registered = accepted_send_transition(
            registered_entry,
            FederationBarkSemanticPhase::ExitRegistered,
        )?;
        validate_maintenance_transition_binding(&self.federation, plan_entry, &plan, &registered)?;
        self.apply_emergency_exit_plan(projection, &plan).await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        self.wallet
            .register_exit(&bark_plan)
            .await
            .map_err(|_| Error::MaintenanceProjectionFailed)?;
        Ok(())
    }

    /// Progress an accepted registered exit and publish its deterministic
    /// final drain once every exact input is claimable.
    pub async fn publish_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
        accepted_authorization: &FederationJournalEntry,
    ) -> Result<FederatedBarkExitOutcome, Error> {
        let plan =
            accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::EmergencyExit)?;
        if plan.actor != self.member_id {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let registered = accepted_send_transition(
            registered_entry,
            FederationBarkSemanticPhase::ExitRegistered,
        )?;
        validate_maintenance_transition_binding(
            &self.federation,
            accepted_plan,
            &plan,
            &registered,
        )?;
        self.apply_emergency_exit_registered(projection, accepted_plan, registered_entry)
            .await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let custody_attempt = accepted_custody_attempt(accepted_plan, accepted_authorization)?;
        self.custody
            .activate_exit(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                registered.request_hash.to_bytes().to_vec(),
            )
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let result = async {
            let evidence = self.wallet.progress_exit(&bark_plan).await?;
            match evidence.phase {
                FederationExitPhase::Claimable
                | FederationExitPhase::DrainPublished
                | FederationExitPhase::Confirmed => {
                    let drain = self.wallet.build_exit_drain(&bark_plan).await?;
                    self.wallet.publish_exit_drain(&bark_plan, &drain).await
                }
                FederationExitPhase::Planned
                | FederationExitPhase::Registered
                | FederationExitPhase::Published => Ok(evidence),
            }
        }
        .await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        self.custody.deactivate_exit();
        let evidence = result
            .map_err(|error| classify_maintenance_execution_error(error, signer_unavailable))?;
        if !matches!(
            evidence.phase,
            FederationExitPhase::DrainPublished | FederationExitPhase::Confirmed
        ) {
            return Ok(FederatedBarkExitOutcome::Pending);
        }
        let transition = exit_transition(
            self.member_id,
            accepted_plan,
            if custody_attempt.entry.operation_id == accepted_plan.operation_id {
                registered_entry.operation_id
            } else {
                custody_attempt.entry.operation_id
            },
            &plan,
            projection,
            &evidence,
            ExitTransitionAuthorization {
                phase: FederationBarkSemanticPhase::ExitPublished,
                attempt: custody_attempt.attempt,
                signer_set: &custody_attempt.signer_set,
            },
        )?;
        Ok(FederatedBarkExitOutcome::Published(Box::new(transition)))
    }

    /// Observe confirmation of the exact accepted drain transaction.
    pub async fn observe_emergency_exit_confirmation(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
        published_entry: &FederationJournalEntry,
        observation_epoch: u64,
    ) -> Result<Option<FederationBarkMemberObservation>, Error> {
        if observation_epoch == 0 {
            return Err(Error::MaintenanceReconciliationFailed);
        }
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::EmergencyExit)?;
        let published =
            accepted_send_transition(published_entry, FederationBarkSemanticPhase::ExitPublished)?;
        validate_maintenance_transition_binding(&self.federation, plan_entry, &plan, &published)?;
        let accepted_evidence = exit_evidence_from_transition(&plan, &published)?;
        let drain = accepted_evidence
            .drain_transaction
            .as_ref()
            .ok_or(Error::MaintenanceReconciliationFailed)?;
        self.apply_emergency_exit_registered(projection, plan_entry, registered_entry)
            .await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let evidence = self
            .wallet
            .observe_exit(&bark_plan, Some(drain))
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        if evidence.phase != FederationExitPhase::Confirmed {
            return Ok(None);
        }
        let result_hash = bark_digest(&exit_result_bytes(&evidence)?)?;
        Ok(Some(FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: plan.action_id,
            quote: None,
            observer: self.member_id,
            observation_epoch,
            source_scope: ark_server_commitment(&self.bark_application().ark_server_public_key)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            payment_hash: None,
            status: FederationBarkObservationStatus::Succeeded,
            amount: plan.external_amount,
            fee: plan.fee_amount,
            result_hash,
        }))
    }

    /// Build the terminal exit transition after confirmation observations have
    /// reached consensus threshold.
    pub async fn confirm_emergency_exit(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan_entry: &FederationJournalEntry,
        registered_entry: &FederationJournalEntry,
        published_entry: &FederationJournalEntry,
    ) -> Result<Option<FederationBarkActionTransition>, Error> {
        let plan = accepted_maintenance_plan(plan_entry, FederationBarkActionKind::EmergencyExit)?;
        let published =
            accepted_send_transition(published_entry, FederationBarkSemanticPhase::ExitPublished)?;
        let accepted_evidence = exit_evidence_from_transition(&plan, &published)?;
        let drain = accepted_evidence
            .drain_transaction
            .as_ref()
            .ok_or(Error::MaintenanceReconciliationFailed)?;
        self.apply_emergency_exit_registered(projection, plan_entry, registered_entry)
            .await?;
        let bark_plan = self.bark_exit_plan(projection, &plan)?;
        let evidence = self
            .wallet
            .observe_exit(&bark_plan, Some(drain))
            .await
            .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        if evidence.phase != FederationExitPhase::Confirmed {
            return Ok(None);
        }
        exit_transition(
            self.member_id,
            plan_entry,
            published_entry.operation_id,
            &plan,
            projection,
            &evidence,
            ExitTransitionAuthorization {
                phase: FederationBarkSemanticPhase::ExitConfirmed,
                attempt: published.attempt,
                signer_set: &published.signer_set,
            },
        )
        .map(Some)
    }

    fn bark_exit_plan(
        &self,
        projection: &FederationBarkProjectionSnapshot,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationExitPlan, Error> {
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        if plan.kind != FederationBarkActionKind::EmergencyExit {
            return Err(Error::MaintenancePlanInvalid);
        }
        let request = plan
            .emergency_exit_request()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        let mut records = Vec::with_capacity(plan.inputs.len());
        for input in &plan.inputs {
            let record = projection
                .vtxos
                .iter()
                .find(|record| record.vtxo_id == input.vtxo_id)
                .ok_or(Error::MaintenancePlanInvalid)?;
            if record.locator != input.locator
                || record.amount != input.amount
                || !matches!(
                    record.state,
                    FederationBarkVtxoState::Reserved
                        | FederationBarkVtxoState::Exiting
                        | FederationBarkVtxoState::Exited
                )
                || record
                    .reservation_action_id
                    .is_some_and(|action| action != plan.action_id)
            {
                return Err(Error::MaintenancePlanInvalid);
            }
            records.push(record);
        }
        records.sort_by_key(|record| record.vtxo_id);
        let inputs = self.maintenance_inputs_from_records(&records)?;
        let commitment = FederationBarkOperation::ActionPlan(plan.clone())
            .commitment()
            .map_err(|_| Error::MaintenancePlanInvalid)?;
        Ok(FederationExitPlan {
            action_id: FederationActionId::from_bytes(plan.action_id.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            request_commitment: FederationRequestCommitment::from_bytes(commitment.to_bytes())
                .map_err(|_| Error::MaintenancePlanInvalid)?,
            inputs,
            input_amount: bitcoin::Amount::from_sat(
                plan.inputs
                    .iter()
                    .map(|input| u64::from(input.amount))
                    .try_fold(0_u64, u64::checked_add)
                    .ok_or(Error::MaintenancePlanInvalid)?,
            ),
            destination_script: ScriptBuf::from_bytes(request.destination_script),
            network: request.network,
            fee_rate: FeeRate::from_sat_per_kwu(request.fee_rate_sat_per_kwu),
            fee_bound: bitcoin::Amount::from_sat(u64::from(request.fee_bound)),
            minimum_recovered: bitcoin::Amount::from_sat(u64::from(request.minimum_recovered)),
            trigger_height: request.trigger_height,
            minimum_expiry: request.minimum_expiry,
            minimum_confirmations: request.minimum_confirmations,
            custody_attempt: plan.attempt,
        })
    }

    fn maintenance_inputs_from_records(
        &self,
        records: &[&FederationBarkVtxoRecord],
    ) -> Result<Vec<FederationMaintenanceInput>, Error> {
        records
            .iter()
            .map(|record| {
                let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(
                    &record.artifact.bytes,
                )
                .map_err(|_| Error::MaintenancePlanInvalid)?;
                if vtxo.serialize() != record.artifact.bytes
                    || vtxo.id().to_bytes() != record.vtxo_id.to_bytes()
                    || vtxo.amount().to_sat() != u64::from(record.amount)
                    || vtxo.expiry_height() != record.expiry_height
                    || vtxo.server_pubkey().serialize()
                        != self.bark_application().ark_server_public_key
                {
                    return Err(Error::MaintenancePlanInvalid);
                }
                Ok(FederationMaintenanceInput {
                    locator: VtxoKeyLocator {
                        index: record.locator.index,
                    },
                    vtxo,
                })
            })
            .collect()
    }

    fn validate_send_quote<'a>(
        &self,
        quote: &'a MeltQuote,
    ) -> Result<
        (
            &'a cdk_common::Bolt11Invoice,
            cdk_common::Amount,
            cdk_common::Amount,
        ),
        Error,
    > {
        let MeltPaymentRequest::Bolt11 { bolt11 } = &quote.request else {
            return Err(Error::SendPlanInvalid);
        };
        if quote.unit != CurrencyUnit::Sat || !send_quote_state_is_replayable(quote.state) {
            return Err(Error::SendPlanInvalid);
        }
        bolt11
            .check_signature()
            .map_err(|_| Error::SendPlanInvalid)?;
        if bolt11.network() != self.network() {
            return Err(Error::SendPlanInvalid);
        }
        let amount = cdk_common::Amount::from(quote.amount().value());
        let fee = cdk_common::Amount::from(quote.fee_reserve().value());
        let expected_msat = u64::from(amount)
            .checked_mul(1_000)
            .ok_or(Error::SendPlanInvalid)?;
        if amount == cdk_common::Amount::ZERO
            || bolt11.amount_milli_satoshis() != Some(expected_msat)
            || fee
                != self
                    .policy()
                    .maximum_outgoing_fee(amount)
                    .map_err(|_| Error::SendPlanInvalid)?
        {
            return Err(Error::SendPlanInvalid);
        }
        Ok((bolt11, amount, fee))
    }

    /// Perform the sole Ark invoice RPC for an already accepted receive intent.
    ///
    /// The protected preimage is loaded only after the caller supplies the
    /// accepted intent operation ID. Bark receives no locally generated
    /// identity, secret, locator, mailbox, amount, or policy value.
    pub async fn prepare_receive_invoice(
        &self,
        execution_actor: MemberId,
        intent_operation_id: FederationOperationId,
        intent: &FederationBarkReceiveIntent,
    ) -> Result<FederationBarkReceivePrepared, Error> {
        let binding =
            FederationBarkReceiveSecretBinding::from_intent(self.scope.federation_id(), intent);
        let secret = self.receive_secrets.load_secret(&binding)?;
        let bark_intent =
            self.bark_receive_intent(intent_operation_id, intent, *secret.as_bytes())?;
        let plan = self
            .wallet
            .start_lightning_receive(&bark_intent)
            .await
            .map_err(|_| Error::InvoicePreparationFailed)?;
        FederationBarkReceivePrepared::from_invoice(
            execution_actor,
            intent_operation_id,
            intent,
            plan.invoice,
        )
        .map_err(|_| Error::InvoicePreparationFailed)
    }

    /// Materialize an accepted receive plan without another Ark RPC.
    pub async fn apply_receive_prepared(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<(), Error> {
        prepared
            .validate_against_intent(intent)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        let plan = self.receive_plan(intent, prepared)?;
        self.wallet
            .validate_lightning_receive_plan(&plan)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        self.wallet
            .apply_lightning_receive(&plan)
            .await
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        Ok(())
    }

    fn receive_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<FederationLightningReceivePlan, Error> {
        prepared
            .validate_against_intent(intent)
            .map_err(|_| Error::ReceiveProjectionFailed)?;
        let binding =
            FederationBarkReceiveSecretBinding::from_intent(self.scope.federation_id(), intent);
        let secret = self.receive_secrets.load_secret(&binding)?;
        let bark_intent =
            self.bark_receive_intent(prepared.intent_operation_id, intent, *secret.as_bytes())?;
        Ok(FederationLightningReceivePlan {
            action_id: bark_intent.action_id,
            request_commitment: bark_intent.request_commitment,
            payment_hash: bark_intent.payment_hash,
            cashu_amount: bark_intent.cashu_amount,
            gross_amount: bark_intent.gross_amount,
            receive_fee_bound: bark_intent.receive_fee_bound,
            minimum_net_amount: bark_intent.minimum_net_amount,
            description: bark_intent.description,
            invoice: prepared
                .invoice
                .parse()
                .map_err(|_| Error::ReceiveProjectionFailed)?,
            payment_preimage: bark_intent.payment_preimage,
            locator: bark_intent.locator,
            mailbox_id: bark_intent.mailbox_id,
            htlc_recv_cltv_delta: bark_intent.htlc_recv_cltv_delta,
            anti_dos: bark_intent.anti_dos,
            policy_version: bark_intent.policy_version,
            custody_attempt: bark_intent.custody_attempt,
        })
    }

    /// Replay an accepted HTLC-ready transition from its exact VTXO artifacts
    /// without polling Ark again.
    pub async fn apply_receive_htlcs_ready(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        accepted_ready: &FederationJournalEntry,
    ) -> Result<(), Error> {
        if accepted_ready.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionTransition(transition)) =
            bark_operation(accepted_ready)
        else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        if transition.phase != FederationBarkSemanticPhase::ReceiveHtlcsReady
            || transition.action_id != intent.action_id
            || transition.quote.as_deref() != Some(intent.quote.as_str())
            || transition.plan_operation_id != prepared_operation_id
            || transition.predecessor_operation_id != prepared_operation_id
            || transition.payment_hash != Some(intent.payment_hash)
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let evidence =
            receive_htlc_evidence_from_transition(&self.wallet, &receive, intent, &transition)?;
        self.wallet
            .apply_lightning_receive_htlc_replay(&receive, &evidence.vtxos)
            .await
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark accepted receive evidence replay failed"
                );
                Error::ReceiveClaimRecoveryRequired
            })
    }

    /// Poll and validate exact incoming HTLC evidence. The returned member
    /// observation is safe to order immediately; the transition candidate must
    /// wait until the federation has accepted the required matching evidence.
    /// Only the current receive-worker lease holder may prepare an accepted
    /// payment; every member can validate an already-ready package read-only.
    pub async fn observe_receive_htlcs(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        observation_epoch: u64,
        prepare_if_accepted: bool,
    ) -> Result<
        Option<(
            FederationBarkMemberObservation,
            FederationBarkActionTransition,
        )>,
        Error,
    > {
        if observation_epoch == 0 {
            return Err(Error::ReceiveObservationFailed);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let observation = self
            .wallet
            .poll_lightning_receive(&receive)
            .await
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark receive status poll failed"
                );
                Error::ReceiveObservationFailed
            })?;
        let evidence = match observation.status {
            FederationLightningReceiveStatus::AwaitingPayment
            | FederationLightningReceiveStatus::Canceled => return Ok(None),
            FederationLightningReceiveStatus::Settled => {
                return Err(Error::ReceiveClaimRecoveryRequired)
            }
            FederationLightningReceiveStatus::Accepted if prepare_if_accepted => self
                .wallet
                .prepare_lightning_receive_htlcs(&receive)
                .await
                .map_err(|error| {
                    tracing::warn!(
                        action_id = %intent.action_id,
                        error = %error,
                        "Federated Bark accepted receive preparation failed"
                    );
                    Error::ReceiveObservationFailed
                })?,
            FederationLightningReceiveStatus::Accepted => return Ok(None),
            // The original Ark status response contains VTXO IDs, not full
            // VTXOs. Once ready, this endpoint is idempotent and returns the
            // same complete evidence to every federation member.
            FederationLightningReceiveStatus::HtlcsReady => self
                .wallet
                .hydrate_lightning_receive_htlcs(&receive)
                .await
                .map_err(|error| {
                    tracing::warn!(
                        action_id = %intent.action_id,
                        error = %error,
                        "Federated Bark ready receive hydration failed"
                    );
                    Error::ReceiveObservationFailed
                })?,
        };
        let transition =
            receive_htlc_transition(self.member_id, prepared_operation_id, intent, &evidence)?;
        let member_observation = FederationBarkMemberObservation {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: intent.action_id,
            quote: Some(intent.quote.clone()),
            observer: self.member_id,
            observation_epoch,
            source_scope: intent.ark_server,
            payment_hash: Some(intent.payment_hash),
            status: FederationBarkObservationStatus::Accepted,
            amount: cdk_common::Amount::from(evidence.net_amount.to_sat()),
            fee: cdk_common::Amount::from(evidence.fee.to_sat()),
            result_hash: transition.result_hash,
        };
        Ok(Some((member_observation, transition)))
    }

    /// Rebuild an HTLC-ready transition from this member's durable evidence.
    ///
    /// A member observation is submitted only after Bark persists the exact
    /// HTLC package, so the receive actor can continue from an accepted local
    /// observation without polling Ark again while it waits for the remaining
    /// federation observations.
    pub async fn receive_htlc_transition_from_accepted_observation(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        accepted: &FederationBarkMemberObservation,
    ) -> Result<FederationBarkActionTransition, Error> {
        if accepted.action_id != intent.action_id
            || accepted.quote.as_deref() != Some(intent.quote.as_str())
            || accepted.observer != self.member_id
            || accepted.source_scope != intent.ark_server
            || accepted.payment_hash != Some(intent.payment_hash)
            || accepted.status != FederationBarkObservationStatus::Accepted
        {
            return Err(Error::ReceiveObservationFailed);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark accepted receive evidence reload failed"
                );
                Error::ReceiveObservationFailed
            })?;
        let transition =
            receive_htlc_transition(self.member_id, prepared_operation_id, intent, &evidence)?;
        if accepted.amount != cdk_common::Amount::from(evidence.net_amount.to_sat())
            || accepted.fee != cdk_common::Amount::from(evidence.fee.to_sat())
            || accepted.result_hash != transition.result_hash
        {
            return Err(Error::ReceiveObservationFailed);
        }
        Ok(transition)
    }

    /// Construct one exact incoming-claim action plan from the local copy of
    /// already accepted HTLC evidence. This performs no custody operation and
    /// does not disclose the preimage.
    pub async fn propose_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        ready_operation_id: FederationOperationId,
        proposal: FederatedBarkReceiveClaimProposal,
    ) -> Result<FederationBarkActionPlan, Error> {
        let receive = self.receive_plan(intent, prepared)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark receive claim evidence load failed"
                );
                Error::ReceiveClaimPlanInvalid
            })?;
        let claim = self
            .wallet
            .plan_lightning_receive_claim(
                evidence,
                VtxoKeyLocator {
                    index: proposal.output_locator.index,
                },
                proposal.attempt,
            )
            .map_err(|error| {
                tracing::warn!(
                    action_id = %intent.action_id,
                    error = %error,
                    "Federated Bark receive claim planning failed"
                );
                Error::ReceiveClaimPlanInvalid
            })?;
        self.claim_action_plan(
            intent,
            ready_operation_id,
            proposal.actor,
            proposal.signer_set,
            proposal.output_locator,
            &claim,
        )
    }

    /// Combine threshold-certified HTLC evidence with the exact incoming
    /// claim reservation. The certificate still authorizes only readiness;
    /// the outer plan separately commits the custody inputs, output, and
    /// signer set before any preimage disclosure.
    pub async fn propose_certified_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        certified_ready: FederationBarkActionTransition,
        proposal: FederatedBarkReceiveClaimProposal,
    ) -> Result<FederationBarkActionPlan, Error> {
        let certificate = certified_ready
            .evidence_certificate
            .as_deref()
            .ok_or(Error::ReceiveClaimPlanInvalid)?
            .clone();
        if certified_ready.phase != FederationBarkSemanticPhase::ReceiveHtlcsReady
            || certified_ready.action_id != intent.action_id
            || certified_ready.quote.as_deref() != Some(intent.quote.as_str())
            || certified_ready.plan_operation_id != certified_ready.predecessor_operation_id
            || certified_ready.payment_hash != Some(intent.payment_hash)
            || certificate.binding.evidence_digest != certified_ready.result_hash
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let certified_evidence = receive_htlc_evidence_from_transition(
            &self.wallet,
            &receive,
            intent,
            &certified_ready,
        )?;
        let predecessor_operation_id = certified_ready.predecessor_operation_id;
        let mut plan = self
            .propose_receive_claim(intent, prepared, predecessor_operation_id, proposal)
            .await?;
        let ready_artifacts = certified_ready
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .cloned()
            .collect::<Vec<_>>();
        let plan_artifacts = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .cloned()
            .collect::<Vec<_>>();
        if ready_artifacts != plan_artifacts {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        plan.certified_receive_htlcs = Some(Box::new(FederationBarkCertifiedReceiveHtlcs {
            version: FEDERATION_BARK_OPERATION_VERSION,
            minimum_expiry: certified_evidence.minimum_expiry,
            request_hash: certified_ready.request_hash,
            vtxo_transitions: certified_ready.vtxo_transitions,
            certificate,
        }));
        FederationBarkOperation::ActionPlan(plan.clone())
            .validate()
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        Ok(plan)
    }

    /// Reconstruct and compare every field of an incoming claim plan before it
    /// can authorize FROST or virtual MuSig2.
    pub async fn validate_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningReceiveClaimPlan, Error> {
        let (receive, accepted_claim) = self.accepted_receive_claim_plan(intent, prepared, plan)?;
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let output = plan.outputs.first().ok_or(Error::ReceiveClaimPlanInvalid)?;
        let claim = self
            .wallet
            .plan_lightning_receive_claim(
                evidence,
                VtxoKeyLocator {
                    index: output.locator.index,
                },
                plan.attempt,
            )
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        if claim != accepted_claim {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        Ok(accepted_claim)
    }

    fn accepted_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        plan: &FederationBarkActionPlan,
    ) -> Result<
        (
            bark::federation::FederationLightningReceivePlan,
            FederationLightningReceiveClaimPlan,
        ),
        Error,
    > {
        if plan.kind != FederationBarkActionKind::IncomingClaim
            || plan.action_id != intent.action_id
            || plan.quote.as_deref() != Some(intent.quote.as_str())
            || plan.payment_hash != Some(intent.payment_hash)
            || plan.outputs.len() != 1
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
        let mut input_vtxos = plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .map(|artifact| {
                let vtxo =
                    bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(&artifact.bytes)
                        .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
                if vtxo.serialize() != artifact.bytes {
                    return Err(Error::ReceiveClaimPlanInvalid);
                }
                Ok(vtxo)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        input_vtxos.sort_by_key(bark::ark::vtxo::Vtxo::id);
        if input_vtxos.len() != plan.inputs.len()
            || plan.inputs.iter().zip(&input_vtxos).any(|(input, vtxo)| {
                input.vtxo_id.to_bytes() != vtxo.id().to_bytes()
                    || u64::from(input.amount) != vtxo.amount().to_sat()
                    || input.locator != intent.destination
            })
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let output = plan.outputs.first().ok_or(Error::ReceiveClaimPlanInvalid)?;
        let destination_locator = VtxoKeyLocator {
            index: output.locator.index,
        };
        let destination = self
            .custody
            .derive_public_key(destination_locator)
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let claim = FederationLightningReceiveClaimPlan {
            action_id: FederationActionId::from_bytes(plan.action_id.to_bytes())
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
            payment_hash: receive.payment_hash,
            destination,
            amount: bitcoin::Amount::from_sat(u64::from(output.amount)),
            minimum_expiry: u32::try_from(plan.expires_at)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
            custody_attempt: plan.attempt,
            input_vtxos,
        };
        self.wallet
            .validate_lightning_receive_claim_plan(&receive, &claim)
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        if let Some(certified_ready) = certified_receive_htlc_transition(plan)? {
            receive_htlc_evidence_from_transition(
                &self.wallet,
                &receive,
                intent,
                &certified_ready,
            )?;
        }
        let mut expected = self.claim_action_plan(
            intent,
            plan.predecessor_operation_id,
            plan.actor,
            plan.signer_set.clone(),
            output.locator,
            &claim,
        )?;
        expected.certified_receive_htlcs = plan.certified_receive_htlcs.clone();
        if &expected != plan {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        Ok((receive, claim))
    }

    /// Replay an accepted incoming-claim plan without observing Ark or
    /// releasing custody material. This reconstructs the exact HTLC inputs
    /// from accepted artifacts and stops before preimage disclosure.
    pub async fn apply_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        accepted_plan: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_claim_plan(accepted_plan)?;
        let (receive, claim) = self.accepted_receive_claim_plan(intent, prepared, &plan)?;
        if let Some(certified_ready) = certified_receive_htlc_transition(&plan)? {
            let evidence = receive_htlc_evidence_from_transition(
                &self.wallet,
                &receive,
                intent,
                &certified_ready,
            )?;
            self.wallet
                .apply_lightning_receive_htlc_replay(&receive, &evidence.vtxos)
                .await
                .map_err(|_| Error::ReceiveClaimRecoveryRequired)?;
        }
        self.wallet
            .apply_lightning_receive_claim_replay(
                &receive,
                &claim,
                FederationLightningReceivePhase::HtlcsReady,
                &[],
            )
            .await
            .map_err(|_| Error::ReceiveClaimRecoveryRequired)
    }

    /// Replay an accepted preimage-disclosure or terminal claim transition
    /// without signing or external I/O.
    pub async fn apply_receive_claim_transition(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        accepted_plan: &FederationJournalEntry,
        accepted_transition: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_claim_plan(accepted_plan)?;
        if accepted_transition.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionTransition(transition)) =
            bark_operation(accepted_transition)
        else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        let expected_signers =
            federation_bark_signer_set_for_attempt(&self.federation, transition.attempt)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        if transition.action_id != plan.action_id
            || transition.quote.as_deref() != plan.quote.as_deref()
            || transition.plan_operation_id != accepted_plan.operation_id
            || transition.payment_hash != plan.payment_hash
            || transition.attempt < plan.attempt
            || transition.signer_set != expected_signers
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let phase = match transition.phase {
            FederationBarkSemanticPhase::ReceivePreimageDisclosed => {
                FederationLightningReceivePhase::PreimageRevealed
            }
            FederationBarkSemanticPhase::ReceiveClaimed => FederationLightningReceivePhase::Claimed,
            _ => return Err(Error::ReceiveClaimPlanInvalid),
        };
        let (receive, claim) = self.accepted_receive_claim_plan(intent, prepared, &plan)?;
        let outputs = if phase == FederationLightningReceivePhase::Claimed {
            receive_claim_outputs(&transition, &claim)?
        } else {
            Vec::new()
        };
        self.wallet
            .apply_lightning_receive_claim_replay(&receive, &claim, phase, &outputs)
            .await
            .map_err(|_| Error::ReceiveClaimRecoveryRequired)?;
        if phase == FederationLightningReceivePhase::Claimed {
            self.wallet
                .acknowledge_lightning_receive_claim(&receive)
                .await
                .map_err(|_| Error::ReceiveClaimRecoveryRequired)?;
        }
        Ok(())
    }

    /// Execute an already applied claim plan through threshold custody and
    /// return only semantic transition candidates. An error after Bark's
    /// durable preimage checkpoint is converted to `RecoveryRequired` so the
    /// caller cannot reinterpret the quote as unpaid.
    pub async fn execute_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        accepted_plan: FederationJournalEntry,
        accepted_authorization: FederationJournalEntry,
    ) -> Result<FederatedBarkReceiveClaimOutcome, Error> {
        if accepted_plan.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(&accepted_plan) else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        let claim = self
            .validate_receive_claim_plan(intent, prepared, &plan)
            .await?;
        let receive = self.receive_plan(intent, prepared)?;
        let operation = FederationBarkOperation::ActionPlan(plan.clone());
        let plan_commitment = operation
            .commitment()
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let action_id = FederationActionId::from_bytes(plan.action_id.to_bytes())
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let request_commitment =
            FederationRequestCommitment::from_bytes(plan_commitment.to_bytes())
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let authorization_context = plan_commitment.to_bytes().to_vec();
        let custody_attempt = accepted_custody_attempt(&accepted_plan, &accepted_authorization)?;
        self.custody
            .activate_claim(
                accepted_plan.clone(),
                custody_attempt.entry.clone(),
                intent.destination,
                authorization_context.clone(),
            )
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let execution = FederationLightningReceiveClaimExecution {
            external_action_id: action_id,
            request_commitment,
            authorization_context,
        };
        let result = self
            .wallet
            .execute_lightning_receive_claim(&receive, &claim, &execution)
            .await;
        let signer_unavailable = self.custody.take_signer_unavailable();
        let disclosed = match &result {
            Ok(_) => true,
            Err(_) => matches!(
                self.wallet.lightning_receive_phase(&receive).await,
                Ok(FederationLightningReceivePhase::PreimageRevealed)
            ),
        };
        self.custody.deactivate_claim();
        if !disclosed {
            return match result {
                Err(error) => Err(classify_receive_claim_error(&error, signer_unavailable)),
                Ok(_) => Err(Error::ReceiveClaimPlanInvalid),
            };
        }
        let preimage_disclosed = receive_preimage_disclosed_transition(
            self.member_id,
            &accepted_plan,
            intent,
            &plan,
            &claim,
            plan_commitment,
            &custody_attempt,
        )?;
        match result {
            Ok(outputs) => Ok(FederatedBarkReceiveClaimOutcome::Claimed {
                preimage_disclosed,
                result: Box::new(receive_claim_result(
                    &accepted_plan,
                    intent,
                    &plan,
                    &claim,
                    outputs,
                    plan_commitment,
                    &custody_attempt,
                )?),
            }),
            Err(_) => Ok(FederatedBarkReceiveClaimOutcome::RecoveryRequired { preimage_disclosed }),
        }
    }

    /// Bind a verified local claim result to the now-applied disclosure
    /// transition. This enforces the ordered `Reserved -> PendingSpend ->
    /// Spent` projection rather than skipping the point-of-no-return record.
    pub fn finalize_receive_claim(
        &self,
        accepted_preimage: &FederationJournalEntry,
        result: FederatedBarkReceiveClaimResult,
    ) -> Result<FederationBarkActionTransition, Error> {
        if accepted_preimage.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let Some(FederationBarkOperation::ActionTransition(disclosed)) =
            bark_operation(accepted_preimage)
        else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        if disclosed.phase != FederationBarkSemanticPhase::ReceivePreimageDisclosed
            || disclosed.action_id != result.action_id
            || disclosed.plan_operation_id != result.plan_operation_id
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let mut transitions = result.input_transitions;
        transitions.extend(result.output_transitions);
        transitions.sort();
        Ok(FederationBarkActionTransition {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: result.action_id,
            quote: Some(result.quote),
            phase: FederationBarkSemanticPhase::ReceiveClaimed,
            actor: self.member_id,
            plan_operation_id: result.plan_operation_id,
            predecessor_operation_id: accepted_preimage.operation_id,
            payment_hash: Some(result.payment_hash),
            attempt: result.attempt,
            signer_set: result.signer_set,
            request_hash: disclosed.result_hash,
            result_hash: result.result_hash,
            evidence_certificate: None,
            external_id: receive_external_id(
                b"cdk/bark/receive-claimed-external/v1",
                result.action_id,
                accepted_preimage.operation_id,
                result.attempt,
            )?,
            artifacts: result.artifacts,
            vtxo_transitions: transitions,
        })
    }

    /// Acknowledge that the terminal claim transition is durably accepted and
    /// allow Bark to remove its successful-result retry checkpoint.
    pub async fn acknowledge_receive_claim(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
    ) -> Result<(), Error> {
        let receive = self.receive_plan(intent, prepared)?;
        self.wallet
            .acknowledge_lightning_receive_claim(&receive)
            .await
            .map_err(|_| Error::ReceiveClaimRecoveryRequired)
    }

    /// Independently validate every spendable VTXO in an accepted claim
    /// candidate before this member attests to the result. The transition's
    /// opaque bytes are not trusted: they must canonically decode to the exact
    /// ID, amount, and locator in the semantic mutation, use the configured
    /// Ark key, and pass Bark's complete signed-chain validation.
    pub async fn validate_receive_claim_result(
        &self,
        transition: &FederationBarkActionTransition,
    ) -> Result<cdk_common::Amount, Error> {
        if transition.phase != FederationBarkSemanticPhase::ReceiveClaimed {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let outputs = transition
            .vtxo_transitions
            .iter()
            .filter(|mutation| {
                mutation.previous_state.is_none()
                    && mutation.new_state == FederationBarkVtxoState::Spendable
            })
            .collect::<Vec<_>>();
        if outputs.is_empty() {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let mut ids = BTreeSet::new();
        let mut amount = cdk_common::Amount::ZERO;
        for output in outputs {
            if !ids.insert(output.vtxo_id) {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let mut artifacts = transition.artifacts.iter().filter(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo
                    && artifact.hash == output.artifact_hash
            });
            let artifact = artifacts.next().ok_or(Error::ReceiveClaimPlanInvalid)?;
            if artifacts.next().is_some() || artifact.hash != bark_digest(&artifact.bytes)? {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(&artifact.bytes)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
            if vtxo.serialize() != artifact.bytes
                || vtxo.id().to_bytes() != output.vtxo_id.to_bytes()
                || vtxo.amount().to_sat() != u64::from(output.amount)
            {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            self.wallet
                .validate_claimed_vtxo(
                    VtxoKeyLocator {
                        index: output.locator.index,
                    },
                    &vtxo,
                )
                .await
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
            amount = amount
                .checked_add(output.amount)
                .ok_or(Error::ReceiveClaimPlanInvalid)?;
        }
        Ok(amount)
    }

    fn claim_action_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        ready_operation_id: FederationOperationId,
        actor: MemberId,
        mut signer_set: Vec<MemberId>,
        output_locator: FederationBarkVtxoLocator,
        claim: &FederationLightningReceiveClaimPlan,
    ) -> Result<FederationBarkActionPlan, Error> {
        if output_locator.role != 1 || output_locator.version != 1 {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        signer_set.sort();
        signer_set.dedup();
        if signer_set.len() < usize::from(self.application.threshold) {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let mut inputs = claim
            .input_vtxos
            .iter()
            .map(|vtxo| {
                Ok(FederationBarkVtxoInput {
                    vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                        .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
                    locator: intent.destination,
                    amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        inputs.sort();
        let request_bytes = receive_claim_request_bytes(claim);
        let request_artifact =
            bark_artifact(FederationBarkArtifactKind::ArkRequest, request_bytes)?;
        let mut artifacts = claim
            .input_vtxos
            .iter()
            .map(|vtxo| bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize()))
            .collect::<Result<Vec<_>, _>>()?;
        artifacts.push(request_artifact.clone());
        artifacts.sort();
        Ok(FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id: intent.action_id,
            quote: Some(intent.quote.clone()),
            kind: FederationBarkActionKind::IncomingClaim,
            actor,
            predecessor_operation_id: ready_operation_id,
            payment_hash: Some(intent.payment_hash),
            inputs,
            outputs: vec![FederationBarkVtxoOutput {
                locator: output_locator,
                amount: cdk_common::Amount::from(claim.amount.to_sat()),
                artifact_hash: request_artifact.hash,
            }],
            revocation_locator: None,
            external_amount: cdk_common::Amount::ZERO,
            fee_amount: cdk_common::Amount::ZERO,
            fee_bound: cdk_common::Amount::ZERO,
            certified_receive_htlcs: None,
            artifacts,
            external_id: receive_external_id(
                b"cdk/bark/receive-claim-external/v1",
                intent.action_id,
                ready_operation_id,
                claim.custody_attempt,
            )?,
            signer_set,
            attempt: claim.custody_attempt,
            expires_at: u64::from(claim.minimum_expiry),
        })
    }

    fn bark_receive_intent(
        &self,
        intent_operation_id: FederationOperationId,
        intent: &FederationBarkReceiveIntent,
        preimage: [u8; 32],
    ) -> Result<FederationLightningReceiveIntent, Error> {
        let action_id = FederationActionId::from_bytes(intent.action_id.to_bytes())
            .map_err(|_| Error::InvoicePreparationFailed)?;
        let request_commitment = FederationRequestCommitment::from_bytes(
            federation_bark_receive_request_hash(intent_operation_id, intent)
                .map_err(|_| Error::InvoicePreparationFailed)?
                .to_bytes(),
        )
        .map_err(|_| Error::InvoicePreparationFailed)?;
        let mailbox_public_key = PublicKey::from_slice(&intent.mailbox_id)
            .map_err(|_| Error::InvoicePreparationFailed)?;
        let htlc_recv_cltv_delta = u16::try_from(intent.minimum_cltv_delta)
            .map_err(|_| Error::InvoicePreparationFailed)?;
        Ok(FederationLightningReceiveIntent {
            action_id,
            request_commitment,
            payment_hash: PaymentHash::from(intent.payment_hash.to_bytes()),
            cashu_amount: bitcoin::Amount::from_sat(u64::from(intent.cashu_amount)),
            gross_amount: bitcoin::Amount::from_sat(u64::from(intent.gross_amount)),
            receive_fee_bound: bitcoin::Amount::from_sat(u64::from(intent.receive_fee)),
            minimum_net_amount: bitcoin::Amount::from_sat(u64::from(intent.minimum_net_amount)),
            description: intent.description.clone(),
            payment_preimage: Preimage::from(preimage),
            locator: VtxoKeyLocator {
                index: intent.destination.index,
            },
            mailbox_id: MailboxIdentifier::from_pubkey(mailbox_public_key),
            htlc_recv_cltv_delta,
            anti_dos: LightningReceiveAntiDosMode::NoProof,
            policy_version: intent.fee_policy_version,
            custody_attempt: 1,
        })
    }

    /// Return readiness relative to one accepted frontier.
    pub async fn status(&self, required_frontier: u64) -> FederatedBarkRuntimeStatus {
        let health = *self.health.read().await;
        let readiness = match health.degraded_reason {
            Some(_) => FederatedBarkReadiness::Degraded,
            None if health.local_frontier < required_frontier => {
                FederatedBarkReadiness::BehindConsensus
            }
            None => FederatedBarkReadiness::Ready,
        };
        FederatedBarkRuntimeStatus {
            readiness,
            local_frontier: health.local_frontier,
            degraded_reason: health.degraded_reason,
        }
    }

    /// Build a bounded operator view from accepted state and local ledgers.
    ///
    /// The result deliberately excludes invoices, payment hashes, protocol
    /// record identifiers, cryptographic material, endpoints, and paths.
    pub async fn operator_status(
        &self,
        accepted_frontier: u64,
        projection: &FederationBarkProjectionSnapshot,
        protocols: FederationBarkProtocolStatus,
        chain_tip_height: Option<u32>,
    ) -> Result<FederationBarkOperatorStatus, Error> {
        let runtime = self.status(accepted_frontier).await;
        let application = self.bark_application();
        let refresh_margin_blocks = application.policy.refresh_before_expiry_blocks;
        let ark_server_scope = ark_server_commitment(&application.ark_server_public_key)
            .map_err(|_| Error::InvalidPublicConfig)?;

        let states = [
            FederationBarkVtxoState::Observed,
            FederationBarkVtxoState::Spendable,
            FederationBarkVtxoState::Reserved,
            FederationBarkVtxoState::PendingSpend,
            FederationBarkVtxoState::Spent,
            FederationBarkVtxoState::Exiting,
            FederationBarkVtxoState::Exited,
        ];
        let vtxos = states
            .into_iter()
            .map(|state| {
                let mut count = 0_u64;
                let mut value_sat = 0_u64;
                for vtxo in projection.vtxos.iter().filter(|vtxo| vtxo.state == state) {
                    count = count.saturating_add(1);
                    value_sat = value_sat.saturating_add(u64::from(vtxo.amount));
                }
                FederationBarkVtxoStatusCount {
                    state,
                    count,
                    value_sat,
                }
            })
            .collect();

        let live_vtxos = projection
            .vtxos
            .iter()
            .filter(|vtxo| !bark_vtxo_state_is_terminal(vtxo.state))
            .collect::<Vec<_>>();
        let earliest_expiry_height = live_vtxos.iter().map(|vtxo| vtxo.expiry_height).min();
        let (expiring_vtxo_count, expiring_value_sat) = match chain_tip_height {
            Some(tip) => {
                let deadline = tip.saturating_add(refresh_margin_blocks);
                let mut count = 0_u64;
                let mut value_sat = 0_u64;
                for vtxo in live_vtxos
                    .iter()
                    .filter(|vtxo| vtxo.expiry_height <= deadline)
                {
                    count = count.saturating_add(1);
                    value_sat = value_sat.saturating_add(u64::from(vtxo.amount));
                }
                (Some(count), Some(value_sat))
            }
            None => (None, None),
        };

        let action_values = projection.vtxos.iter().fold(
            BTreeMap::<FederationBarkActionId, u64>::new(),
            |mut values, vtxo| {
                if !bark_vtxo_state_is_terminal(vtxo.state) {
                    if let Some(action_id) = vtxo.reservation_action_id {
                        let value = values.entry(action_id).or_default();
                        *value = value.saturating_add(u64::from(vtxo.amount));
                    }
                }
                values
            },
        );
        let mut ambiguous_action_count = 0_u64;
        let mut ambiguous_value_sat = 0_u64;
        let mut recovery_required_action_count = 0_u64;
        let mut recovery_required_value_sat = 0_u64;
        let mut signer_retry_action_count = 0_u64;
        let mut actions = Vec::new();
        for action in projection
            .actions
            .iter()
            .filter(|action| !bark_action_phase_is_terminal(action.phase))
        {
            let value_sat = action_values.get(&action.action_id).copied().unwrap_or(0);
            let effective_phase = action.signing_retry_phase.unwrap_or(action.phase);
            if bark_action_phase_is_ambiguous(effective_phase) {
                ambiguous_action_count = ambiguous_action_count.saturating_add(1);
                ambiguous_value_sat = ambiguous_value_sat.saturating_add(value_sat);
            }
            if bark_action_phase_requires_recovery(effective_phase) {
                recovery_required_action_count = recovery_required_action_count.saturating_add(1);
                recovery_required_value_sat = recovery_required_value_sat.saturating_add(value_sat);
            }
            if action.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced {
                signer_retry_action_count = signer_retry_action_count.saturating_add(1);
            }
            actions.push(FederationBarkActionStatus {
                action_id: action.action_id,
                kind: action.action_kind,
                phase: action.phase,
                signing_retry_phase: action.signing_retry_phase,
                actor: action.actor,
                local_actor: action.actor == self.member_id,
                attempt: action.attempt,
                age_operations: accepted_frontier.saturating_sub(action.origin_operation_index),
                value_sat,
            });
        }
        actions.sort_by_key(|action| action.action_id);

        let mut receive_intents = BTreeMap::new();
        for transition in &projection.transitions {
            if let FederationBarkOperation::ReceiveIntent(intent) = &transition.operation {
                receive_intents.insert(intent.action_id, intent);
            }
        }
        let mut receive_secrets = FederationBarkReceiveSecretCoverage {
            minimum_acknowledgements: if receive_intents.is_empty() {
                0
            } else {
                u64::MAX
            },
            ..Default::default()
        };
        for (action_id, intent) in receive_intents {
            receive_secrets.accepted_actions = receive_secrets.accepted_actions.saturating_add(1);
            let acknowledgement_count =
                usize_to_u64_saturating(intent.secret_acknowledgements.len());
            receive_secrets.minimum_acknowledgements = receive_secrets
                .minimum_acknowledgements
                .min(acknowledgement_count);
            receive_secrets.maximum_acknowledgements = receive_secrets
                .maximum_acknowledgements
                .max(acknowledgement_count);
            let local_acknowledged = intent
                .secret_acknowledgements
                .iter()
                .any(|acknowledgement| acknowledgement.member == self.member_id);
            if local_acknowledged {
                receive_secrets.acknowledged_by_local_member = receive_secrets
                    .acknowledged_by_local_member
                    .saturating_add(1);
            }
            let retained = self.receive_secrets.status(action_id)?.is_some();
            if retained {
                receive_secrets.locally_retained =
                    receive_secrets.locally_retained.saturating_add(1);
            }
            let terminal = projection
                .actions
                .iter()
                .find(|action| action.action_id == action_id)
                .is_some_and(|action| bark_action_phase_is_terminal(action.phase));
            if local_acknowledged && !terminal && !retained {
                receive_secrets.missing_acknowledged_nonterminal = receive_secrets
                    .missing_acknowledged_nonterminal
                    .saturating_add(1);
            }
        }

        Ok(FederationBarkOperatorStatus {
            version: FEDERATION_BARK_OPERATOR_STATUS_VERSION,
            readiness: runtime.readiness.as_str().to_string(),
            degraded_reason: runtime
                .degraded_reason
                .map(FederatedBarkDegradedReason::as_str)
                .map(str::to_string),
            accepted_frontier,
            local_frontier: runtime.local_frontier,
            projection_lag_operations: accepted_frontier.saturating_sub(runtime.local_frontier),
            root_key_epoch: self.scope.root_key_epoch(),
            application_key_epoch: self.scope.application_key_epoch(),
            network: self.network().to_string(),
            ark_server_scope,
            ark_source_scope: "shared_protocol_server".to_string(),
            ark_source_correlated_warning: true,
            chain_source_scope: "undeclared".to_string(),
            chain_source_correlated_warning: true,
            chain_tip_height,
            refresh_margin_blocks,
            vtxos,
            expiring_vtxo_count,
            expiring_value_sat,
            earliest_expiry_height,
            ambiguous_action_count,
            ambiguous_value_sat,
            recovery_required_action_count,
            recovery_required_value_sat,
            signer_retry_action_count,
            actions,
            protocols,
            receive_secrets,
        })
    }

    /// Fail closed unless this member can serve the required accepted frontier.
    pub async fn ensure_ready(&self, required_frontier: u64) -> Result<(), Error> {
        match self.status(required_frontier).await.readiness {
            FederatedBarkReadiness::Ready => Ok(()),
            FederatedBarkReadiness::BehindConsensus | FederatedBarkReadiness::Degraded => {
                Err(Error::NotReady)
            }
        }
    }

    /// Return the current bounded degradation reason.
    pub async fn degraded_reason(&self) -> Option<FederatedBarkDegradedReason> {
        self.health.read().await.degraded_reason
    }

    /// Advance the member-local replay frontier monotonically.
    pub async fn record_projection_frontier(&self, frontier: u64) {
        let mut health = self.health.write().await;
        health.local_frontier = health.local_frontier.max(frontier);
    }

    /// Mark the runtime degraded with a bounded, non-sensitive reason.
    pub async fn mark_degraded(&self, reason: FederatedBarkDegradedReason) {
        self.health.write().await.degraded_reason = Some(reason);
    }

    /// Clear one recovery condition without masking a different degradation.
    pub async fn clear_degraded(&self, reason: FederatedBarkDegradedReason) {
        let mut health = self.health.write().await;
        if health.degraded_reason == Some(reason) {
            health.degraded_reason = None;
        }
    }
}

const fn send_quote_state_is_replayable(state: MeltQuoteState) -> bool {
    matches!(
        state,
        MeltQuoteState::Unpaid
            | MeltQuoteState::Pending
            | MeltQuoteState::Paid
            | MeltQuoteState::Failed
    )
}

const fn bark_vtxo_state_is_terminal(state: FederationBarkVtxoState) -> bool {
    matches!(
        state,
        FederationBarkVtxoState::Spent | FederationBarkVtxoState::Exited
    )
}

const fn bark_action_phase_is_terminal(phase: FederationBarkSemanticPhase) -> bool {
    matches!(
        phase,
        FederationBarkSemanticPhase::ReceiveClaimed
            | FederationBarkSemanticPhase::PaymentSucceeded
            | FederationBarkSemanticPhase::RevocationCompleted
            | FederationBarkSemanticPhase::RefreshCompleted
            | FederationBarkSemanticPhase::ExitConfirmed
            | FederationBarkSemanticPhase::Observation
    )
}

const fn bark_action_phase_is_ambiguous(phase: FederationBarkSemanticPhase) -> bool {
    matches!(
        phase,
        FederationBarkSemanticPhase::ReceivePreimageDisclosed
            | FederationBarkSemanticPhase::SendHtlcsCosigned
    )
}

const fn bark_action_phase_requires_recovery(phase: FederationBarkSemanticPhase) -> bool {
    matches!(
        phase,
        FederationBarkSemanticPhase::PaymentFailed
            | FederationBarkSemanticPhase::RevocationPlanned
            | FederationBarkSemanticPhase::ExitPlanned
            | FederationBarkSemanticPhase::ExitRegistered
            | FederationBarkSemanticPhase::ExitPublished
    )
}

fn usize_to_u64_saturating(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn bark_operation(entry: &FederationJournalEntry) -> Option<FederationBarkOperation> {
    validate_federation_bark_operation(&entry.envelope.operation).ok()
}

fn accepted_send_plan(entry: &FederationJournalEntry) -> Result<FederationBarkActionPlan, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::SendReconciliationFailed);
    }
    let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(entry) else {
        return Err(Error::SendReconciliationFailed);
    };
    if plan.kind != FederationBarkActionKind::LightningSend {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(plan)
}

fn accepted_revocation_plan(
    entry: &FederationJournalEntry,
) -> Result<FederationBarkActionPlan, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::SendReconciliationFailed);
    }
    let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(entry) else {
        return Err(Error::SendReconciliationFailed);
    };
    if plan.kind != FederationBarkActionKind::FailedSendRevocation {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(plan)
}

fn accepted_claim_plan(entry: &FederationJournalEntry) -> Result<FederationBarkActionPlan, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(entry) else {
        return Err(Error::ReceiveClaimPlanInvalid);
    };
    if plan.kind != FederationBarkActionKind::IncomingClaim {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    Ok(plan)
}

fn receive_claim_outputs(
    transition: &FederationBarkActionTransition,
    claim: &FederationLightningReceiveClaimPlan,
) -> Result<Vec<bark::ark::vtxo::Vtxo<bark::ark::vtxo::Full>>, Error> {
    let mut outputs = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state.is_none()
                && mutation.new_state == FederationBarkVtxoState::Spendable
        })
        .map(|mutation| {
            if mutation.locator.index != claim.destination.locator.index {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let mut artifacts = transition.artifacts.iter().filter(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo
                    && artifact.hash == mutation.artifact_hash
            });
            let artifact = artifacts.next().ok_or(Error::ReceiveClaimPlanInvalid)?;
            if artifacts.next().is_some() {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(&artifact.bytes)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
            if vtxo.serialize() != artifact.bytes
                || vtxo.id().to_bytes() != mutation.vtxo_id.to_bytes()
                || vtxo.amount().to_sat() != u64::from(mutation.amount)
            {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            Ok(vtxo)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    outputs.sort_by_key(bark::ark::vtxo::Vtxo::id);
    if outputs.is_empty() || outputs.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    Ok(outputs)
}

fn accepted_maintenance_plan(
    entry: &FederationJournalEntry,
    kind: FederationBarkActionKind,
) -> Result<FederationBarkActionPlan, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(entry) else {
        return Err(Error::MaintenanceReconciliationFailed);
    };
    if plan.kind != kind {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    Ok(plan)
}

fn accepted_action_plan(entry: &FederationJournalEntry) -> Result<FederationBarkActionPlan, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::InvalidCustodyAuthorization);
    }
    let Some(FederationBarkOperation::ActionPlan(plan)) = bark_operation(entry) else {
        return Err(Error::InvalidCustodyAuthorization);
    };
    Ok(plan)
}

fn classify_send_execution_error(error: anyhow::Error, signer_unavailable: bool) -> Error {
    if signer_unavailable
        || matches!(
            error.downcast_ref::<Error>(),
            Some(Error::CustodySignerUnavailable)
        )
    {
        Error::CustodySignerUnavailable
    } else {
        Error::SendExecutionFailed
    }
}

fn classify_receive_claim_error(error: &anyhow::Error, signer_unavailable: bool) -> Error {
    if signer_unavailable
        || matches!(
            error.downcast_ref::<Error>(),
            Some(Error::CustodySignerUnavailable)
        )
    {
        Error::CustodySignerUnavailable
    } else {
        Error::ReceiveClaimPlanInvalid
    }
}

fn classify_maintenance_execution_error(error: anyhow::Error, signer_unavailable: bool) -> Error {
    if signer_unavailable
        || matches!(
            error.downcast_ref::<Error>(),
            Some(Error::CustodySignerUnavailable)
        )
    {
        Error::CustodySignerUnavailable
    } else {
        Error::MaintenanceExecutionFailed
    }
}

fn accepted_custody_attempt(
    plan_entry: &FederationJournalEntry,
    authorization_entry: &FederationJournalEntry,
) -> Result<AcceptedCustodyAttempt, Error> {
    let plan = accepted_action_plan(plan_entry)?;
    if authorization_entry.status() != FederationOperationStatus::Applied {
        return Err(Error::InvalidCustodyAuthorization);
    }
    let Some(operation) = bark_operation(authorization_entry) else {
        return Err(Error::InvalidCustodyAuthorization);
    };
    let (attempt, signer_set) = match &operation {
        FederationBarkOperation::ActionPlan(authorized_plan)
            if authorization_entry.operation_id == plan_entry.operation_id
                && authorization_entry.index == plan_entry.index
                && authorized_plan == &plan =>
        {
            (plan.attempt, plan.signer_set.clone())
        }
        FederationBarkOperation::ActionTransition(transition)
            if transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced
                && transition.action_id == plan.action_id
                && transition.quote == plan.quote
                && transition.actor == plan.actor
                && transition.plan_operation_id == plan_entry.operation_id
                && transition.payment_hash == plan.payment_hash
                && transition.external_id == plan.external_id =>
        {
            (transition.attempt, transition.signer_set.clone())
        }
        _ => return Err(Error::InvalidCustodyAuthorization),
    };
    Ok(AcceptedCustodyAttempt {
        entry: authorization_entry.clone(),
        attempt,
        signer_set,
    })
}

fn validate_maintenance_transition_binding(
    federation: &FederationConfig,
    plan_entry: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let expected_signers = federation_bark_signer_set_for_attempt(federation, transition.attempt)
        .map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .map_err(|_| Error::MaintenanceReconciliationFailed)?;
    if transition.action_id != plan.action_id
        || transition.quote.is_some()
        || transition.plan_operation_id != plan_entry.operation_id
        || transition.payment_hash.is_some()
        || transition.actor != plan.actor
        || transition.attempt < plan.attempt
        || transition.signer_set != expected_signers
        || transition.external_id != plan.external_id
        || transition.request_hash != commitment
    {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    Ok(())
}

fn accepted_send_transition(
    entry: &FederationJournalEntry,
    phase: FederationBarkSemanticPhase,
) -> Result<FederationBarkActionTransition, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::SendReconciliationFailed);
    }
    let Some(FederationBarkOperation::ActionTransition(transition)) = bark_operation(entry) else {
        return Err(Error::SendReconciliationFailed);
    };
    if transition.phase != phase {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(transition)
}

fn validate_send_transition_binding(
    federation: &FederationConfig,
    accepted_plan: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let expected_signers = federation_bark_signer_set_for_attempt(federation, transition.attempt)
        .map_err(|_| Error::SendReconciliationFailed)?;
    if transition.action_id != plan.action_id
        || transition.quote != plan.quote
        || transition.plan_operation_id != accepted_plan.operation_id
        || transition.payment_hash != plan.payment_hash
        || transition.actor != plan.actor
        || transition.attempt < plan.attempt
        || transition.signer_set != expected_signers
        || transition.external_id != plan.external_id
    {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(())
}

fn send_htlc_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    projection: &FederationBarkProjectionSnapshot,
    evidence: &FederationLightningSendHtlcEvidence,
    plan_commitment: FederationBarkDigest,
    custody_attempt: &AcceptedCustodyAttempt,
) -> Result<FederationBarkActionTransition, Error> {
    let mut artifacts = Vec::new();
    let mut vtxo_transitions = Vec::new();
    for input in &plan.inputs {
        let record = projection
            .vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or(Error::SendExecutionFailed)?;
        if record.locator != input.locator
            || record.amount != input.amount
            || record.state != FederationBarkVtxoState::Reserved
            || record.reservation_action_id != Some(plan.action_id)
        {
            return Err(Error::SendExecutionFailed);
        }
        artifacts.push(record.artifact.clone());
        vtxo_transitions.push(FederationBarkVtxoTransition {
            vtxo_id: input.vtxo_id,
            locator: input.locator,
            amount: input.amount,
            expiry_height: record.expiry_height,
            previous_state: Some(FederationBarkVtxoState::Reserved),
            new_state: FederationBarkVtxoState::Spent,
            artifact_hash: record.artifact.hash,
        });
    }
    for vtxo in evidence
        .htlc_vtxos
        .iter()
        .chain(evidence.change_vtxos.iter())
    {
        artifacts.push(bark_artifact(
            FederationBarkArtifactKind::Vtxo,
            vtxo.serialize(),
        )?);
    }
    let output = plan.outputs.first().ok_or(Error::SendExecutionFailed)?;
    for vtxo in &evidence.htlc_vtxos {
        let artifact = bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize())?;
        vtxo_transitions.push(FederationBarkVtxoTransition {
            vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                .map_err(|_| Error::SendExecutionFailed)?,
            locator: output.locator,
            amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
            expiry_height: vtxo.expiry_height(),
            previous_state: None,
            new_state: FederationBarkVtxoState::PendingSpend,
            artifact_hash: artifact.hash,
        });
    }
    for vtxo in &evidence.change_vtxos {
        let artifact = bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize())?;
        vtxo_transitions.push(FederationBarkVtxoTransition {
            vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                .map_err(|_| Error::SendExecutionFailed)?,
            locator: output.locator,
            amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
            expiry_height: vtxo.expiry_height(),
            previous_state: None,
            new_state: FederationBarkVtxoState::Spendable,
            artifact_hash: artifact.hash,
        });
    }
    let result_artifact = bark_artifact(
        FederationBarkArtifactKind::ArkResult,
        send_htlc_result_bytes(evidence)?,
    )?;
    let result_hash = result_artifact.hash;
    artifacts.push(result_artifact);
    artifacts.sort();
    artifacts.dedup();
    vtxo_transitions.sort();
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: plan.quote.clone(),
        phase: FederationBarkSemanticPhase::SendHtlcsCosigned,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: custody_attempt.entry.operation_id,
        payment_hash: plan.payment_hash,
        attempt: custody_attempt.attempt,
        signer_set: custody_attempt.signer_set.clone(),
        request_hash: plan_commitment,
        result_hash,
        evidence_certificate: None,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions,
    })
}

fn payment_failed_result(
    plan: &FederationBarkActionPlan,
    htlc_result_hash: FederationBarkDigest,
) -> Result<(FederationBarkArtifact, FederationBarkDigest), Error> {
    let mut result_bytes = b"cdk/bark/send-payment-failed/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(&htlc_result_hash.to_bytes());
    result_bytes.extend_from_slice(
        &plan
            .payment_hash
            .ok_or(Error::SendExecutionFailed)?
            .to_bytes(),
    );
    let artifact = bark_artifact(FederationBarkArtifactKind::ArkResult, result_bytes)?;
    let result_hash = artifact.hash;
    Ok((artifact, result_hash))
}

fn validate_payment_failed_result(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let (expected, result_hash) = payment_failed_result(plan, transition.request_hash)?;
    let mut results = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
    if results.next() != Some(&expected)
        || results.next().is_some()
        || transition.result_hash != result_hash
    {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(())
}

fn failed_send_revocation_request_bytes(
    action_id: FederationBarkActionId,
    failed_operation_id: FederationOperationId,
    payment_hash: FederationBarkDigest,
    output_locator: FederationBarkVtxoLocator,
    inputs: &[FederationBarkVtxoInput],
) -> Result<Vec<u8>, Error> {
    let mut bytes = b"cdk/bark/send-revocation-request/v1".to_vec();
    bytes.extend_from_slice(&action_id.to_bytes());
    bytes.extend_from_slice(&failed_operation_id.to_bytes());
    bytes.extend_from_slice(&payment_hash.to_bytes());
    append_locator(&mut bytes, output_locator);
    bytes.extend_from_slice(
        &u32::try_from(inputs.len())
            .map_err(|_| Error::SendPlanInvalid)?
            .to_be_bytes(),
    );
    for input in inputs {
        bytes.extend_from_slice(&input.vtxo_id.to_bytes());
        append_locator(&mut bytes, input.locator);
        bytes.extend_from_slice(&u64::from(input.amount).to_be_bytes());
    }
    Ok(bytes)
}

fn append_locator(bytes: &mut Vec<u8>, locator: FederationBarkVtxoLocator) {
    bytes.push(locator.role);
    bytes.extend_from_slice(&locator.index.to_be_bytes());
    bytes.extend_from_slice(&locator.version.to_be_bytes());
}

fn payment_succeeded_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    accepted_htlcs: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    htlcs: &FederationBarkActionTransition,
    projection: &FederationBarkProjectionSnapshot,
    preimage: Preimage,
) -> Result<FederationBarkActionTransition, Error> {
    let proof = bark_artifact(
        FederationBarkArtifactKind::Proof,
        preimage.as_ref().to_vec(),
    )?;
    let mut result_bytes = b"cdk/bark/send-payment-succeeded/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(&htlcs.result_hash.to_bytes());
    result_bytes.extend_from_slice(preimage.as_ref());
    let result_hash = bark_digest(&result_bytes)?;
    let mut artifacts = vec![proof];
    let pending = projection
        .vtxos
        .iter()
        .filter(|record| {
            record.state == FederationBarkVtxoState::PendingSpend
                && record.reservation_action_id == Some(plan.action_id)
        })
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Err(Error::SendExecutionFailed);
    }
    let mut pending_amount = cdk_common::Amount::ZERO;
    let mut vtxo_transitions = Vec::with_capacity(pending.len());
    for record in pending {
        pending_amount = pending_amount
            .checked_add(record.amount)
            .ok_or(Error::SendExecutionFailed)?;
        artifacts.push(record.artifact.clone());
        vtxo_transitions.push(FederationBarkVtxoTransition {
            vtxo_id: record.vtxo_id,
            locator: record.locator,
            amount: record.amount,
            expiry_height: record.expiry_height,
            previous_state: Some(FederationBarkVtxoState::PendingSpend),
            new_state: FederationBarkVtxoState::Spent,
            artifact_hash: record.artifact.hash,
        });
    }
    if pending_amount
        != plan
            .external_amount
            .checked_add(plan.fee_amount)
            .ok_or(Error::SendExecutionFailed)?
    {
        return Err(Error::SendExecutionFailed);
    }
    artifacts.sort();
    artifacts.dedup();
    vtxo_transitions.sort();
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: plan.quote.clone(),
        phase: FederationBarkSemanticPhase::PaymentSucceeded,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: accepted_htlcs.operation_id,
        payment_hash: plan.payment_hash,
        attempt: htlcs.attempt,
        signer_set: htlcs.signer_set.clone(),
        request_hash: htlcs.result_hash,
        result_hash,
        evidence_certificate: None,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions,
    })
}

fn revocation_completed_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    projection: &FederationBarkProjectionSnapshot,
    evidence: &FederationLightningSendRevocationEvidence,
    plan_commitment: FederationBarkDigest,
    custody_attempt: &AcceptedCustodyAttempt,
) -> Result<FederationBarkActionTransition, Error> {
    let expected_commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .map_err(|_| Error::SendExecutionFailed)?;
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    let evidence_inputs = evidence
        .input_vtxo_ids
        .iter()
        .map(|vtxo_id| vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    if evidence.action_id.to_bytes() != plan.action_id.to_bytes()
        || evidence.request_commitment.to_bytes() != plan_commitment.to_bytes()
        || expected_commitment != plan_commitment
        || evidence.payment_hash.to_vec()
            != plan
                .payment_hash
                .ok_or(Error::SendExecutionFailed)?
                .to_bytes()
        || evidence_inputs != expected_inputs
        || evidence.output_vtxos.is_empty()
    {
        return Err(Error::SendExecutionFailed);
    }
    let mut artifacts = Vec::new();
    let mut transitions = Vec::new();
    for input in &plan.inputs {
        let record = projection
            .vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or(Error::SendExecutionFailed)?;
        if record.state != FederationBarkVtxoState::Reserved
            || record.reservation_action_id != Some(plan.action_id)
            || record.locator != input.locator
            || record.amount != input.amount
        {
            return Err(Error::SendExecutionFailed);
        }
        artifacts.push(record.artifact.clone());
        transitions.push(FederationBarkVtxoTransition {
            vtxo_id: input.vtxo_id,
            locator: input.locator,
            amount: input.amount,
            expiry_height: record.expiry_height,
            previous_state: Some(FederationBarkVtxoState::Reserved),
            new_state: FederationBarkVtxoState::Spent,
            artifact_hash: record.artifact.hash,
        });
    }
    let output = plan.outputs.first().ok_or(Error::SendExecutionFailed)?;
    let mut recovered = cdk_common::Amount::ZERO;
    for vtxo in &evidence.output_vtxos {
        let artifact = bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize())?;
        let amount = cdk_common::Amount::from(vtxo.amount().to_sat());
        recovered = recovered
            .checked_add(amount)
            .ok_or(Error::SendExecutionFailed)?;
        transitions.push(FederationBarkVtxoTransition {
            vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                .map_err(|_| Error::SendExecutionFailed)?,
            locator: output.locator,
            amount,
            expiry_height: vtxo.expiry_height(),
            previous_state: None,
            new_state: FederationBarkVtxoState::Spendable,
            artifact_hash: artifact.hash,
        });
        artifacts.push(artifact);
    }
    if recovered != output.amount {
        return Err(Error::SendExecutionFailed);
    }
    let result_artifact = bark_artifact(
        FederationBarkArtifactKind::ArkResult,
        revocation_result_bytes(evidence)?,
    )?;
    let result_hash = result_artifact.hash;
    artifacts.push(result_artifact);
    artifacts.sort();
    artifacts.dedup();
    transitions.sort();
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: plan.quote.clone(),
        phase: FederationBarkSemanticPhase::RevocationCompleted,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: custody_attempt.entry.operation_id,
        payment_hash: plan.payment_hash,
        attempt: custody_attempt.attempt,
        signer_set: custody_attempt.signer_set.clone(),
        request_hash: plan_commitment,
        result_hash,
        evidence_certificate: None,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions: transitions,
    })
}

fn revocation_result_bytes(
    evidence: &FederationLightningSendRevocationEvidence,
) -> Result<Vec<u8>, Error> {
    let mut bytes = b"cdk/bark/send-revocation-result/v1".to_vec();
    bytes.extend_from_slice(&evidence.action_id.to_bytes());
    bytes.extend_from_slice(&evidence.request_commitment.to_bytes());
    bytes.extend_from_slice(&evidence.payment_hash.to_vec());
    bytes.extend_from_slice(
        &u32::try_from(evidence.input_vtxo_ids.len())
            .map_err(|_| Error::SendExecutionFailed)?
            .to_be_bytes(),
    );
    for vtxo_id in &evidence.input_vtxo_ids {
        bytes.extend_from_slice(&vtxo_id.to_bytes());
    }
    append_send_vtxos(&mut bytes, &evidence.output_vtxos)?;
    Ok(bytes)
}

fn revocation_evidence_from_transition(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<FederationLightningSendRevocationEvidence, Error> {
    let mut results = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
    let result = results.next().ok_or(Error::SendReconciliationFailed)?;
    if results.next().is_some()
        || result.hash != transition.result_hash
        || result.hash != bark_digest(&result.bytes)?
    {
        return Err(Error::SendReconciliationFailed);
    }
    let mut bytes = result.bytes.as_slice();
    take_send_bytes(&mut bytes, b"cdk/bark/send-revocation-result/v1".len())
        .filter(|domain| *domain == b"cdk/bark/send-revocation-result/v1")
        .ok_or(Error::SendReconciliationFailed)?;
    let action_id = take_send_array::<32>(&mut bytes)?;
    let request_commitment = take_send_array::<32>(&mut bytes)?;
    let payment_hash = take_send_array::<32>(&mut bytes)?;
    let input_count = u32::from_be_bytes(take_send_array::<4>(&mut bytes)?);
    if input_count > 1_024 {
        return Err(Error::SendReconciliationFailed);
    }
    let input_vtxo_ids = (0..input_count)
        .map(|_| {
            bark::ark::VtxoId::from_slice(&take_send_array::<36>(&mut bytes)?)
                .map_err(|_| Error::SendReconciliationFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output_vtxos = take_send_vtxos(&mut bytes)?;
    let expected_commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .map_err(|_| Error::SendReconciliationFailed)?;
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    let decoded_inputs = input_vtxo_ids
        .iter()
        .map(|vtxo_id| vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    if !bytes.is_empty()
        || action_id != plan.action_id.to_bytes()
        || request_commitment != expected_commitment.to_bytes()
        || request_commitment != transition.request_hash.to_bytes()
        || payment_hash
            != plan
                .payment_hash
                .ok_or(Error::SendReconciliationFailed)?
                .to_bytes()
        || decoded_inputs != expected_inputs
        || output_vtxos.is_empty()
    {
        return Err(Error::SendReconciliationFailed);
    }
    let input_mutations = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state == Some(FederationBarkVtxoState::Reserved)
                && mutation.new_state == FederationBarkVtxoState::Spent
        })
        .map(|mutation| mutation.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    if input_mutations != expected_inputs {
        return Err(Error::SendReconciliationFailed);
    }
    let output = plan
        .outputs
        .first()
        .ok_or(Error::SendReconciliationFailed)?;
    let output_mutations = transition
        .vtxo_transitions
        .iter()
        .filter(|mutation| {
            mutation.previous_state.is_none()
                && mutation.new_state == FederationBarkVtxoState::Spendable
        })
        .collect::<Vec<_>>();
    if output_mutations.len() != output_vtxos.len()
        || transition.vtxo_transitions.len() != plan.inputs.len() + output_vtxos.len()
    {
        return Err(Error::SendReconciliationFailed);
    }
    let mut recovered = cdk_common::Amount::ZERO;
    for vtxo in &output_vtxos {
        let encoded = vtxo.serialize();
        let artifact = transition.artifacts.iter().filter(|artifact| {
            artifact.kind == FederationBarkArtifactKind::Vtxo && artifact.bytes == encoded
        });
        if artifact.count() != 1 {
            return Err(Error::SendReconciliationFailed);
        }
        let vtxo_id = FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
            .map_err(|_| Error::SendReconciliationFailed)?;
        let amount = cdk_common::Amount::from(vtxo.amount().to_sat());
        let mutation = output_mutations
            .iter()
            .find(|mutation| mutation.vtxo_id == vtxo_id)
            .ok_or(Error::SendReconciliationFailed)?;
        if mutation.locator != output.locator
            || mutation.amount != amount
            || mutation.expiry_height != vtxo.expiry_height()
            || mutation.artifact_hash != bark_digest(&encoded)?
        {
            return Err(Error::SendReconciliationFailed);
        }
        recovered = recovered
            .checked_add(amount)
            .ok_or(Error::SendReconciliationFailed)?;
    }
    if recovered != output.amount {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(FederationLightningSendRevocationEvidence {
        action_id: FederationActionId::from_bytes(action_id)
            .map_err(|_| Error::SendReconciliationFailed)?,
        request_commitment: FederationRequestCommitment::from_bytes(request_commitment)
            .map_err(|_| Error::SendReconciliationFailed)?,
        payment_hash: PaymentHash::from(payment_hash),
        input_vtxo_ids,
        output_vtxos,
    })
}

fn refresh_completed_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    projection: &FederationBarkProjectionSnapshot,
    evidence: &FederationRefreshEvidence,
    plan_commitment: FederationBarkDigest,
    custody_attempt: &AcceptedCustodyAttempt,
) -> Result<FederationBarkActionTransition, Error> {
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    let evidence_inputs = evidence
        .input_vtxo_ids
        .iter()
        .map(|vtxo_id| vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    if evidence.action_id.to_bytes() != plan.action_id.to_bytes()
        || evidence.request_commitment.to_bytes() != plan_commitment.to_bytes()
        || evidence_inputs != expected_inputs
        || evidence.output_vtxos.is_empty()
    {
        return Err(Error::MaintenanceExecutionFailed);
    }
    let mut artifacts = Vec::new();
    let mut transitions = Vec::new();
    for input in &plan.inputs {
        let record = projection
            .vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or(Error::MaintenanceExecutionFailed)?;
        if record.state != FederationBarkVtxoState::Reserved
            || record.reservation_action_id != Some(plan.action_id)
            || record.locator != input.locator
            || record.amount != input.amount
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        artifacts.push(record.artifact.clone());
        transitions.push(FederationBarkVtxoTransition {
            vtxo_id: input.vtxo_id,
            locator: input.locator,
            amount: input.amount,
            expiry_height: record.expiry_height,
            previous_state: Some(FederationBarkVtxoState::Reserved),
            new_state: FederationBarkVtxoState::Spent,
            artifact_hash: record.artifact.hash,
        });
    }
    let output = plan
        .outputs
        .first()
        .ok_or(Error::MaintenanceExecutionFailed)?;
    let mut output_amount = cdk_common::Amount::ZERO;
    for vtxo in &evidence.output_vtxos {
        let encoded = vtxo.serialize();
        let artifact = bark_artifact(FederationBarkArtifactKind::Vtxo, encoded)?;
        let amount = cdk_common::Amount::from(vtxo.amount().to_sat());
        output_amount = output_amount
            .checked_add(amount)
            .ok_or(Error::MaintenanceExecutionFailed)?;
        transitions.push(FederationBarkVtxoTransition {
            vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                .map_err(|_| Error::MaintenanceExecutionFailed)?,
            locator: output.locator,
            amount,
            expiry_height: vtxo.expiry_height(),
            previous_state: None,
            new_state: FederationBarkVtxoState::Spendable,
            artifact_hash: artifact.hash,
        });
        artifacts.push(artifact);
    }
    if output_amount != output.amount {
        return Err(Error::MaintenanceExecutionFailed);
    }
    let result = bark_artifact(
        FederationBarkArtifactKind::ArkResult,
        refresh_result_bytes(evidence)?,
    )?;
    let result_hash = result.hash;
    artifacts.push(result);
    artifacts.sort();
    artifacts.dedup();
    transitions.sort();
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: None,
        phase: FederationBarkSemanticPhase::RefreshCompleted,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: custody_attempt.entry.operation_id,
        payment_hash: None,
        attempt: custody_attempt.attempt,
        signer_set: custody_attempt.signer_set.clone(),
        request_hash: plan_commitment,
        result_hash,
        evidence_certificate: None,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions: transitions,
    })
}

fn refresh_result_bytes(evidence: &FederationRefreshEvidence) -> Result<Vec<u8>, Error> {
    let mut bytes = b"cdk/bark/refresh-result/v1".to_vec();
    bytes.extend_from_slice(&evidence.action_id.to_bytes());
    bytes.extend_from_slice(&evidence.request_commitment.to_bytes());
    bytes.extend_from_slice(
        &u32::try_from(evidence.input_vtxo_ids.len())
            .map_err(|_| Error::MaintenanceExecutionFailed)?
            .to_be_bytes(),
    );
    for vtxo_id in &evidence.input_vtxo_ids {
        bytes.extend_from_slice(&vtxo_id.to_bytes());
    }
    bytes.extend_from_slice(&evidence.funding_txid.to_byte_array());
    bytes.extend_from_slice(
        &u32::try_from(evidence.output_vtxos.len())
            .map_err(|_| Error::MaintenanceExecutionFailed)?
            .to_be_bytes(),
    );
    for vtxo in &evidence.output_vtxos {
        append_bounded_bytes(&mut bytes, &vtxo.serialize())?;
    }
    Ok(bytes)
}

fn refresh_evidence_from_transition(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<FederationRefreshEvidence, Error> {
    let result = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
        .collect::<Vec<_>>();
    if result.len() != 1
        || result[0].hash != transition.result_hash
        || bark_digest(&result[0].bytes)? != result[0].hash
    {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    let mut bytes = result[0].bytes.as_slice();
    take_send_bytes(&mut bytes, b"cdk/bark/refresh-result/v1".len())
        .filter(|domain| *domain == b"cdk/bark/refresh-result/v1")
        .ok_or(Error::MaintenanceReconciliationFailed)?;
    let action_id =
        take_send_array::<32>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let request_commitment =
        take_send_array::<32>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let input_count = u32::from_be_bytes(
        take_send_array::<4>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?,
    );
    let input_vtxo_ids = (0..input_count)
        .map(|_| {
            let id = take_send_array::<36>(&mut bytes)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?;
            bark::ark::VtxoId::from_slice(&id).map_err(|_| Error::MaintenanceReconciliationFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let funding_txid = bitcoin::Txid::from_byte_array(
        take_send_array::<32>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?,
    );
    let output_count = u32::from_be_bytes(
        take_send_array::<4>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?,
    );
    let output_vtxos = (0..output_count)
        .map(|_| {
            let encoded = take_send_bounded(&mut bytes)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?;
            bark::ark::vtxo::Vtxo::deserialize(encoded)
                .map_err(|_| Error::MaintenanceReconciliationFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected_commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    if !bytes.is_empty()
        || action_id != plan.action_id.to_bytes()
        || request_commitment != expected_commitment.to_bytes()
        || input_vtxo_ids
            .iter()
            .map(|vtxo_id| vtxo_id.to_bytes())
            .collect::<Vec<_>>()
            != expected_inputs
        || output_vtxos.is_empty()
    {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    Ok(FederationRefreshEvidence {
        action_id: FederationActionId::from_bytes(action_id)
            .map_err(|_| Error::MaintenanceReconciliationFailed)?,
        request_commitment: FederationRequestCommitment::from_bytes(request_commitment)
            .map_err(|_| Error::MaintenanceReconciliationFailed)?,
        input_vtxo_ids,
        funding_txid,
        output_vtxos,
    })
}

fn exit_transition(
    actor: MemberId,
    plan_entry: &FederationJournalEntry,
    predecessor_operation_id: FederationOperationId,
    plan: &FederationBarkActionPlan,
    projection: &FederationBarkProjectionSnapshot,
    evidence: &FederationExitEvidence,
    authorization: ExitTransitionAuthorization<'_>,
) -> Result<FederationBarkActionTransition, Error> {
    let plan_commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .map_err(|_| Error::MaintenanceExecutionFailed)?;
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    let evidence_inputs = evidence
        .input_vtxo_ids
        .iter()
        .map(|vtxo_id| vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    let valid_phase = match authorization.phase {
        FederationBarkSemanticPhase::ExitRegistered => {
            evidence.phase == FederationExitPhase::Registered
        }
        FederationBarkSemanticPhase::ExitPublished => {
            matches!(
                evidence.phase,
                FederationExitPhase::DrainPublished | FederationExitPhase::Confirmed
            ) && evidence.drain_transaction.is_some()
        }
        FederationBarkSemanticPhase::ExitConfirmed => {
            evidence.phase == FederationExitPhase::Confirmed
                && evidence.drain_transaction.is_some()
                && evidence.confirmed_in.is_some()
        }
        _ => false,
    };
    if !valid_phase
        || evidence.action_id.to_bytes() != plan.action_id.to_bytes()
        || evidence.request_commitment.to_bytes() != plan_commitment.to_bytes()
        || evidence_inputs != expected_inputs
        || evidence.exit_transactions.is_empty()
    {
        return Err(Error::MaintenanceExecutionFailed);
    }
    let mut artifacts = Vec::new();
    let mut transitions = Vec::new();
    for input in &plan.inputs {
        let record = projection
            .vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or(Error::MaintenanceExecutionFailed)?;
        let state_pair = match authorization.phase {
            FederationBarkSemanticPhase::ExitRegistered => (
                FederationBarkVtxoState::Reserved,
                FederationBarkVtxoState::Exiting,
            ),
            FederationBarkSemanticPhase::ExitConfirmed => (
                FederationBarkVtxoState::Exiting,
                FederationBarkVtxoState::Exited,
            ),
            FederationBarkSemanticPhase::ExitPublished => continue,
            _ => return Err(Error::MaintenanceExecutionFailed),
        };
        if record.state != state_pair.0
            || record.reservation_action_id != Some(plan.action_id)
            || record.locator != input.locator
            || record.amount != input.amount
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        artifacts.push(record.artifact.clone());
        transitions.push(FederationBarkVtxoTransition {
            vtxo_id: input.vtxo_id,
            locator: input.locator,
            amount: input.amount,
            expiry_height: record.expiry_height,
            previous_state: Some(state_pair.0),
            new_state: state_pair.1,
            artifact_hash: record.artifact.hash,
        });
    }
    for transaction in evidence
        .exit_transactions
        .iter()
        .chain(evidence.drain_transaction.iter())
    {
        artifacts.push(bark_artifact(
            FederationBarkArtifactKind::Transaction,
            serialize_transaction(transaction),
        )?);
    }
    let result = bark_artifact(
        FederationBarkArtifactKind::ArkResult,
        exit_result_bytes(evidence)?,
    )?;
    let result_hash = result.hash;
    artifacts.push(result);
    artifacts.sort();
    artifacts.dedup();
    transitions.sort();
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: None,
        phase: authorization.phase,
        actor,
        plan_operation_id: plan_entry.operation_id,
        predecessor_operation_id,
        payment_hash: None,
        attempt: authorization.attempt,
        signer_set: authorization.signer_set.to_vec(),
        request_hash: plan_commitment,
        result_hash,
        evidence_certificate: None,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions: transitions,
    })
}

fn exit_result_bytes(evidence: &FederationExitEvidence) -> Result<Vec<u8>, Error> {
    let mut bytes = b"cdk/bark/exit-result/v1".to_vec();
    bytes.extend_from_slice(&evidence.action_id.to_bytes());
    bytes.extend_from_slice(&evidence.request_commitment.to_bytes());
    bytes.push(exit_phase_tag(evidence.phase));
    bytes.extend_from_slice(
        &u32::try_from(evidence.input_vtxo_ids.len())
            .map_err(|_| Error::MaintenanceExecutionFailed)?
            .to_be_bytes(),
    );
    for vtxo_id in &evidence.input_vtxo_ids {
        bytes.extend_from_slice(&vtxo_id.to_bytes());
    }
    bytes.extend_from_slice(
        &u32::try_from(evidence.exit_transactions.len())
            .map_err(|_| Error::MaintenanceExecutionFailed)?
            .to_be_bytes(),
    );
    for transaction in &evidence.exit_transactions {
        append_bounded_bytes(&mut bytes, &serialize_transaction(transaction))?;
    }
    match &evidence.drain_transaction {
        Some(transaction) => {
            bytes.push(1);
            append_bounded_bytes(&mut bytes, &serialize_transaction(transaction))?;
        }
        None => bytes.push(0),
    }
    match evidence.confirmed_in {
        Some(block) => {
            bytes.push(1);
            bytes.extend_from_slice(&block.height.to_be_bytes());
            bytes.extend_from_slice(&block.hash.to_byte_array());
        }
        None => bytes.push(0),
    }
    Ok(bytes)
}

fn exit_evidence_from_transition(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<FederationExitEvidence, Error> {
    let result = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult)
        .collect::<Vec<_>>();
    if result.len() != 1
        || result[0].hash != transition.result_hash
        || bark_digest(&result[0].bytes)? != result[0].hash
    {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    let mut bytes = result[0].bytes.as_slice();
    take_send_bytes(&mut bytes, b"cdk/bark/exit-result/v1".len())
        .filter(|domain| *domain == b"cdk/bark/exit-result/v1")
        .ok_or(Error::MaintenanceReconciliationFailed)?;
    let action_id =
        take_send_array::<32>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let request_commitment =
        take_send_array::<32>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let phase = take_send_bytes(&mut bytes, 1)
        .and_then(|tag| tag.first().copied())
        .and_then(exit_phase_from_tag)
        .ok_or(Error::MaintenanceReconciliationFailed)?;
    let input_count = u32::from_be_bytes(
        take_send_array::<4>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?,
    );
    let input_vtxo_ids = (0..input_count)
        .map(|_| {
            bark::ark::VtxoId::from_slice(
                &take_send_array::<36>(&mut bytes)
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            )
            .map_err(|_| Error::MaintenanceReconciliationFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let transaction_count = u32::from_be_bytes(
        take_send_array::<4>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?,
    );
    let exit_transactions = (0..transaction_count)
        .map(|_| {
            deserialize_transaction::<Transaction>(
                take_send_bounded(&mut bytes)
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            )
            .map_err(|_| Error::MaintenanceReconciliationFailed)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let drain_transaction = match take_send_bytes(&mut bytes, 1) {
        Some([0]) => None,
        Some([1]) => Some(
            deserialize_transaction::<Transaction>(
                take_send_bounded(&mut bytes)
                    .map_err(|_| Error::MaintenanceReconciliationFailed)?,
            )
            .map_err(|_| Error::MaintenanceReconciliationFailed)?,
        ),
        _ => return Err(Error::MaintenanceReconciliationFailed),
    };
    match take_send_bytes(&mut bytes, 1) {
        Some([0]) => {}
        Some([1]) => {
            take_send_array::<4>(&mut bytes).map_err(|_| Error::MaintenanceReconciliationFailed)?;
            take_send_array::<32>(&mut bytes)
                .map_err(|_| Error::MaintenanceReconciliationFailed)?;
        }
        _ => return Err(Error::MaintenanceReconciliationFailed),
    }
    let expected_commitment = FederationBarkOperation::ActionPlan(plan.clone())
        .commitment()
        .map_err(|_| Error::MaintenanceReconciliationFailed)?;
    let expected_inputs = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id.to_bytes())
        .collect::<Vec<_>>();
    if !bytes.is_empty()
        || action_id != plan.action_id.to_bytes()
        || request_commitment != expected_commitment.to_bytes()
        || input_vtxo_ids
            .iter()
            .map(|vtxo_id| vtxo_id.to_bytes())
            .collect::<Vec<_>>()
            != expected_inputs
        || exit_transactions.is_empty()
    {
        return Err(Error::MaintenanceReconciliationFailed);
    }
    Ok(FederationExitEvidence {
        action_id: FederationActionId::from_bytes(action_id)
            .map_err(|_| Error::MaintenanceReconciliationFailed)?,
        request_commitment: FederationRequestCommitment::from_bytes(request_commitment)
            .map_err(|_| Error::MaintenanceReconciliationFailed)?,
        input_vtxo_ids,
        phase,
        tip_height: 0,
        exit_transactions,
        drain_transaction,
        confirmed_in: None,
    })
}

const fn exit_phase_tag(phase: FederationExitPhase) -> u8 {
    match phase {
        FederationExitPhase::Planned => 1,
        FederationExitPhase::Registered => 2,
        FederationExitPhase::Published => 3,
        FederationExitPhase::Claimable => 4,
        FederationExitPhase::DrainPublished => 5,
        FederationExitPhase::Confirmed => 6,
    }
}

const fn exit_phase_from_tag(tag: u8) -> Option<FederationExitPhase> {
    match tag {
        1 => Some(FederationExitPhase::Planned),
        2 => Some(FederationExitPhase::Registered),
        3 => Some(FederationExitPhase::Published),
        4 => Some(FederationExitPhase::Claimable),
        5 => Some(FederationExitPhase::DrainPublished),
        6 => Some(FederationExitPhase::Confirmed),
        _ => None,
    }
}

fn send_htlc_result_bytes(
    evidence: &FederationLightningSendHtlcEvidence,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"cdk/bark/send-htlc-result/v1");
    bytes.extend_from_slice(&evidence.action_id.to_bytes());
    bytes.extend_from_slice(&evidence.request_commitment.to_bytes());
    bytes.extend_from_slice(&evidence.payment_hash.to_vec());
    append_bounded_bytes(&mut bytes, &evidence.mailbox_id.serialize())?;
    append_send_vtxos(&mut bytes, &evidence.htlc_vtxos)?;
    append_send_vtxos(&mut bytes, &evidence.change_vtxos)?;
    Ok(bytes)
}

fn append_send_vtxos(
    bytes: &mut Vec<u8>,
    vtxos: &[bark::ark::vtxo::Vtxo<bark::ark::vtxo::Full>],
) -> Result<(), Error> {
    bytes.extend_from_slice(
        &u32::try_from(vtxos.len())
            .map_err(|_| Error::SendExecutionFailed)?
            .to_be_bytes(),
    );
    for vtxo in vtxos {
        append_bounded_bytes(bytes, &vtxo.serialize())?;
    }
    Ok(())
}

fn send_htlc_evidence_from_transition(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<FederationLightningSendHtlcEvidence, Error> {
    let mut result_artifacts = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
    let result = result_artifacts
        .next()
        .ok_or(Error::SendReconciliationFailed)?;
    if result_artifacts.next().is_some()
        || result.hash != transition.result_hash
        || result.hash != bark_digest(&result.bytes)?
    {
        return Err(Error::SendReconciliationFailed);
    }
    let mut bytes = result.bytes.as_slice();
    take_send_bytes(&mut bytes, b"cdk/bark/send-htlc-result/v1".len())
        .filter(|domain| *domain == b"cdk/bark/send-htlc-result/v1")
        .ok_or(Error::SendReconciliationFailed)?;
    let action_id = take_send_array::<32>(&mut bytes)?;
    let request_commitment = take_send_array::<32>(&mut bytes)?;
    let payment_hash = take_send_array::<32>(&mut bytes)?;
    if action_id != plan.action_id.to_bytes()
        || request_commitment
            != FederationBarkOperation::ActionPlan(plan.clone())
                .commitment()
                .map_err(|_| Error::SendReconciliationFailed)?
                .to_bytes()
        || payment_hash
            != plan
                .payment_hash
                .ok_or(Error::SendReconciliationFailed)?
                .to_bytes()
    {
        return Err(Error::SendReconciliationFailed);
    }
    let mailbox_bytes = take_send_bounded(&mut bytes)?;
    let mailbox_id = MailboxIdentifier::deserialize(mailbox_bytes)
        .map_err(|_| Error::SendReconciliationFailed)?;
    let htlc_vtxos = take_send_vtxos(&mut bytes)?;
    let change_vtxos = take_send_vtxos(&mut bytes)?;
    if !bytes.is_empty() {
        return Err(Error::SendReconciliationFailed);
    }
    for vtxo in htlc_vtxos.iter().chain(change_vtxos.iter()) {
        let encoded = vtxo.serialize();
        let matches = transition
            .artifacts
            .iter()
            .filter(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo && artifact.bytes == encoded
            })
            .count();
        if matches != 1 {
            return Err(Error::SendReconciliationFailed);
        }
    }
    Ok(FederationLightningSendHtlcEvidence {
        action_id: FederationActionId::from_bytes(action_id)
            .map_err(|_| Error::SendReconciliationFailed)?,
        request_commitment: FederationRequestCommitment::from_bytes(request_commitment)
            .map_err(|_| Error::SendReconciliationFailed)?,
        payment_hash: PaymentHash::from(payment_hash),
        mailbox_id,
        htlc_vtxos,
        change_vtxos,
    })
}

fn take_send_vtxos(
    bytes: &mut &[u8],
) -> Result<Vec<bark::ark::vtxo::Vtxo<bark::ark::vtxo::Full>>, Error> {
    let count = u32::from_be_bytes(take_send_array::<4>(bytes)?);
    if count > 1_024 {
        return Err(Error::SendReconciliationFailed);
    }
    (0..count)
        .map(|_| {
            let encoded = take_send_bounded(bytes)?;
            let vtxo = bark::ark::vtxo::Vtxo::deserialize(encoded)
                .map_err(|_| Error::SendReconciliationFailed)?;
            if vtxo.serialize() != encoded {
                return Err(Error::SendReconciliationFailed);
            }
            Ok(vtxo)
        })
        .collect()
}

fn take_send_bounded<'a>(bytes: &mut &'a [u8]) -> Result<&'a [u8], Error> {
    let length = u64::from_be_bytes(take_send_array::<8>(bytes)?);
    let length = usize::try_from(length).map_err(|_| Error::SendReconciliationFailed)?;
    take_send_bytes(bytes, length).ok_or(Error::SendReconciliationFailed)
}

fn take_send_array<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], Error> {
    take_send_bytes(bytes, N)
        .and_then(|value| value.try_into().ok())
        .ok_or(Error::SendReconciliationFailed)
}

fn take_send_bytes<'a>(bytes: &mut &'a [u8], length: usize) -> Option<&'a [u8]> {
    if bytes.len() < length {
        return None;
    }
    let (value, remaining) = bytes.split_at(length);
    *bytes = remaining;
    Some(value)
}

fn send_success_preimage(
    plan: &FederationBarkActionPlan,
    success: &FederationBarkActionTransition,
) -> Result<Preimage, Error> {
    let mut proofs = success
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Proof);
    let proof = proofs.next().ok_or(Error::SendReconciliationFailed)?;
    let preimage_bytes: [u8; 32] = proof
        .bytes
        .as_slice()
        .try_into()
        .map_err(|_| Error::SendReconciliationFailed)?;
    let preimage = Preimage::from(preimage_bytes);
    let payment_hash = plan.payment_hash.ok_or(Error::SendReconciliationFailed)?;
    let mut result_bytes = b"cdk/bark/send-payment-succeeded/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(&success.request_hash.to_bytes());
    result_bytes.extend_from_slice(&preimage_bytes);
    if proofs.next().is_some()
        || proof.hash != bark_digest(&proof.bytes)?
        || preimage.compute_payment_hash().to_vec() != payment_hash.to_bytes()
        || success.result_hash != bark_digest(&result_bytes)?
    {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(preimage)
}

fn receive_htlc_evidence_from_transition(
    wallet: &FederationWallet,
    plan: &FederationLightningReceivePlan,
    intent: &FederationBarkReceiveIntent,
    transition: &FederationBarkActionTransition,
) -> Result<bark::federation::FederationLightningReceiveHtlcEvidence, Error> {
    let mut vtxos = transition
        .vtxo_transitions
        .iter()
        .map(|mutation| {
            if mutation.previous_state.is_some()
                || mutation.new_state != FederationBarkVtxoState::Observed
                || mutation.locator != intent.destination
            {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let mut artifacts = transition.artifacts.iter().filter(|artifact| {
                artifact.kind == FederationBarkArtifactKind::Vtxo
                    && artifact.hash == mutation.artifact_hash
            });
            let artifact = artifacts.next().ok_or(Error::ReceiveClaimPlanInvalid)?;
            if artifacts.next().is_some() || artifact.hash != bark_digest(&artifact.bytes)? {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            let vtxo = bark::ark::vtxo::Vtxo::<bark::ark::vtxo::Full>::deserialize(&artifact.bytes)
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
            if vtxo.serialize() != artifact.bytes
                || vtxo.id().to_bytes() != mutation.vtxo_id.to_bytes()
                || vtxo.amount().to_sat() != u64::from(mutation.amount)
                || vtxo.expiry_height() != mutation.expiry_height
            {
                return Err(Error::ReceiveClaimPlanInvalid);
            }
            Ok(vtxo)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    vtxos.sort_by_key(bark::ark::vtxo::Vtxo::id);
    if vtxos.is_empty() || vtxos.windows(2).any(|pair| pair[0].id() == pair[1].id()) {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    let evidence = wallet
        .reconstruct_lightning_receive_htlc_evidence(plan, vtxos)
        .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
    let mut request = b"cdk/bark/receive-htlc-request/v1".to_vec();
    request.extend_from_slice(&transition.plan_operation_id.to_bytes());
    request.extend_from_slice(&intent.action_id.to_bytes());
    request.extend_from_slice(&intent.destination.index.to_be_bytes());
    request.extend_from_slice(&evidence.minimum_expiry.to_be_bytes());
    let current_result_hash = federation_bark_receive_htlc_package_hash(
        evidence.minimum_expiry,
        transition.request_hash,
        &transition.vtxo_transitions,
        &transition.artifacts,
    )
    .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
    let legacy_result_hash = legacy_receive_htlc_evidence_hash(&evidence)?;
    if !receive_htlc_result_hash_matches(
        transition.result_hash,
        current_result_hash,
        legacy_result_hash,
        transition.evidence_certificate.is_none(),
    ) || transition.request_hash != bark_digest(&request)?
    {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    Ok(evidence)
}

fn legacy_receive_htlc_evidence_hash(
    evidence: &bark::federation::FederationLightningReceiveHtlcEvidence,
) -> Result<FederationBarkDigest, Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"cdk/bark/receive-htlc-evidence/v1");
    bytes.extend_from_slice(&evidence.action_id.to_bytes());
    bytes.extend_from_slice(&evidence.payment_hash.to_vec());
    bytes.extend_from_slice(&evidence.destination.index.to_be_bytes());
    bytes.extend_from_slice(&evidence.gross_amount.to_sat().to_be_bytes());
    bytes.extend_from_slice(&evidence.net_amount.to_sat().to_be_bytes());
    bytes.extend_from_slice(&evidence.fee.to_sat().to_be_bytes());
    bytes.extend_from_slice(&evidence.minimum_expiry.to_be_bytes());
    for vtxo in &evidence.vtxos {
        append_bounded_bytes(&mut bytes, &vtxo.serialize())?;
    }
    bark_digest(&bytes)
}

fn receive_htlc_result_hash_matches(
    stored: FederationBarkDigest,
    current: FederationBarkDigest,
    legacy: FederationBarkDigest,
    legacy_compatible: bool,
) -> bool {
    stored == current || (legacy_compatible && stored == legacy)
}

fn certified_receive_htlc_transition(
    plan: &FederationBarkActionPlan,
) -> Result<Option<FederationBarkActionTransition>, Error> {
    let Some(certified) = plan.certified_receive_htlcs.as_deref() else {
        return Ok(None);
    };
    let transition = FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: plan.quote.clone(),
        phase: FederationBarkSemanticPhase::ReceiveHtlcsReady,
        actor: plan.actor,
        plan_operation_id: plan.predecessor_operation_id,
        predecessor_operation_id: plan.predecessor_operation_id,
        payment_hash: plan.payment_hash,
        attempt: plan.attempt,
        signer_set: Vec::new(),
        request_hash: certified.request_hash,
        result_hash: certified.certificate.binding.evidence_digest,
        evidence_certificate: Some(Box::new(certified.certificate.clone())),
        external_id: receive_external_id(
            b"cdk/bark/receive-htlc-external/v1",
            plan.action_id,
            plan.predecessor_operation_id,
            plan.attempt,
        )?,
        artifacts: plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Vtxo)
            .cloned()
            .collect(),
        vtxo_transitions: certified.vtxo_transitions.clone(),
    };
    FederationBarkOperation::ActionTransition(transition.clone())
        .validate()
        .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
    Ok(Some(transition))
}

fn receive_htlc_transition(
    actor: MemberId,
    prepared_operation_id: FederationOperationId,
    intent: &FederationBarkReceiveIntent,
    evidence: &bark::federation::FederationLightningReceiveHtlcEvidence,
) -> Result<FederationBarkActionTransition, Error> {
    let mut artifacts = evidence
        .vtxos
        .iter()
        .map(|vtxo| bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize()))
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort();
    let mut vtxo_transitions = evidence
        .vtxos
        .iter()
        .map(|vtxo| {
            let bytes = vtxo.serialize();
            Ok(FederationBarkVtxoTransition {
                vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                    .map_err(|_| Error::ReceiveObservationFailed)?,
                locator: intent.destination,
                amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
                expiry_height: vtxo.expiry_height(),
                previous_state: None,
                new_state: FederationBarkVtxoState::Observed,
                artifact_hash: bark_digest(&bytes)?,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    vtxo_transitions.sort();
    let mut request = Vec::new();
    request.extend_from_slice(b"cdk/bark/receive-htlc-request/v1");
    request.extend_from_slice(&prepared_operation_id.to_bytes());
    request.extend_from_slice(&intent.action_id.to_bytes());
    request.extend_from_slice(&intent.destination.index.to_be_bytes());
    request.extend_from_slice(&evidence.minimum_expiry.to_be_bytes());
    let request_hash = bark_digest(&request)?;
    let result_hash = federation_bark_receive_htlc_package_hash(
        evidence.minimum_expiry,
        request_hash,
        &vtxo_transitions,
        &artifacts,
    )
    .map_err(|_| Error::ReceiveObservationFailed)?;
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: intent.action_id,
        quote: Some(intent.quote.clone()),
        phase: FederationBarkSemanticPhase::ReceiveHtlcsReady,
        actor,
        plan_operation_id: prepared_operation_id,
        predecessor_operation_id: prepared_operation_id,
        payment_hash: Some(intent.payment_hash),
        attempt: 1,
        signer_set: Vec::new(),
        request_hash,
        result_hash,
        evidence_certificate: None,
        external_id: receive_external_id(
            b"cdk/bark/receive-htlc-external/v1",
            intent.action_id,
            prepared_operation_id,
            1,
        )?,
        artifacts,
        vtxo_transitions,
    })
}

fn receive_preimage_disclosed_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    intent: &FederationBarkReceiveIntent,
    plan: &FederationBarkActionPlan,
    claim: &FederationLightningReceiveClaimPlan,
    plan_commitment: FederationBarkDigest,
    custody_attempt: &AcceptedCustodyAttempt,
) -> Result<FederationBarkActionTransition, Error> {
    let mut artifacts = claim
        .input_vtxos
        .iter()
        .map(|vtxo| bark_artifact(FederationBarkArtifactKind::Vtxo, vtxo.serialize()))
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort();
    let mut vtxo_transitions = claim
        .input_vtxos
        .iter()
        .map(|vtxo| {
            let bytes = vtxo.serialize();
            Ok(FederationBarkVtxoTransition {
                vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                    .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
                locator: intent.destination,
                amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
                expiry_height: vtxo.expiry_height(),
                previous_state: Some(FederationBarkVtxoState::Reserved),
                new_state: FederationBarkVtxoState::PendingSpend,
                artifact_hash: bark_digest(&bytes)?,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    vtxo_transitions.sort();
    let mut result = Vec::new();
    result.extend_from_slice(b"cdk/bark/receive-preimage-disclosed/v1");
    result.extend_from_slice(&plan.action_id.to_bytes());
    result.extend_from_slice(&accepted_plan.operation_id.to_bytes());
    result.extend_from_slice(&intent.payment_hash.to_bytes());
    result.extend_from_slice(&custody_attempt.attempt.to_be_bytes());
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: Some(intent.quote.clone()),
        phase: FederationBarkSemanticPhase::ReceivePreimageDisclosed,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: custody_attempt.entry.operation_id,
        payment_hash: Some(intent.payment_hash),
        attempt: custody_attempt.attempt,
        signer_set: custody_attempt.signer_set.clone(),
        request_hash: plan_commitment,
        result_hash: bark_digest(&result)?,
        evidence_certificate: None,
        external_id: receive_external_id(
            b"cdk/bark/receive-preimage-external/v1",
            plan.action_id,
            accepted_plan.operation_id,
            plan.attempt,
        )?,
        artifacts,
        vtxo_transitions,
    })
}

fn receive_claim_result(
    accepted_plan: &FederationJournalEntry,
    intent: &FederationBarkReceiveIntent,
    plan: &FederationBarkActionPlan,
    claim: &FederationLightningReceiveClaimPlan,
    outputs: Vec<bark::ark::vtxo::Vtxo<bark::ark::vtxo::Full>>,
    plan_commitment: FederationBarkDigest,
    custody_attempt: &AcceptedCustodyAttempt,
) -> Result<FederatedBarkReceiveClaimResult, Error> {
    let output = plan.outputs.first().ok_or(Error::ReceiveClaimPlanInvalid)?;
    let mut artifacts = Vec::new();
    let mut input_transitions = Vec::new();
    for vtxo in &claim.input_vtxos {
        let bytes = vtxo.serialize();
        artifacts.push(bark_artifact(
            FederationBarkArtifactKind::Vtxo,
            bytes.clone(),
        )?);
        input_transitions.push(FederationBarkVtxoTransition {
            vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
            locator: intent.destination,
            amount: cdk_common::Amount::from(vtxo.amount().to_sat()),
            expiry_height: vtxo.expiry_height(),
            previous_state: Some(FederationBarkVtxoState::PendingSpend),
            new_state: FederationBarkVtxoState::Spent,
            artifact_hash: bark_digest(&bytes)?,
        });
    }
    let mut output_transitions = Vec::new();
    let mut materialized_amount_sat = 0_u64;
    let mut result_bytes = Vec::new();
    result_bytes.extend_from_slice(b"cdk/bark/receive-claimed-result/v1");
    result_bytes.extend_from_slice(&plan_commitment.to_bytes());
    for vtxo in outputs {
        let bytes = vtxo.serialize();
        let amount = cdk_common::Amount::from(vtxo.amount().to_sat());
        materialized_amount_sat = materialized_amount_sat
            .checked_add(u64::from(amount))
            .ok_or(Error::ReceiveClaimPlanInvalid)?;
        append_bounded_bytes(&mut result_bytes, &bytes)?;
        artifacts.push(bark_artifact(
            FederationBarkArtifactKind::Vtxo,
            bytes.clone(),
        )?);
        output_transitions.push(FederationBarkVtxoTransition {
            vtxo_id: FederationBarkVtxoId::from_bytes(vtxo.id().to_bytes())
                .map_err(|_| Error::ReceiveClaimPlanInvalid)?,
            locator: output.locator,
            amount,
            expiry_height: vtxo.expiry_height(),
            previous_state: None,
            new_state: FederationBarkVtxoState::Spendable,
            artifact_hash: bark_digest(&bytes)?,
        });
    }
    if output_transitions.is_empty()
        || materialized_amount_sat < u64::from(intent.cashu_amount)
        || materialized_amount_sat > u64::from(output.amount)
    {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    artifacts.sort();
    artifacts.dedup();
    input_transitions.sort();
    output_transitions.sort();
    Ok(FederatedBarkReceiveClaimResult {
        plan_operation_id: accepted_plan.operation_id,
        action_id: plan.action_id,
        quote: intent.quote.clone(),
        payment_hash: intent.payment_hash,
        attempt: custody_attempt.attempt,
        signer_set: custody_attempt.signer_set.clone(),
        input_transitions,
        output_transitions,
        artifacts,
        result_hash: bark_digest(&result_bytes)?,
        amount: cdk_common::Amount::from(materialized_amount_sat),
    })
}

fn receive_claim_request_bytes(claim: &FederationLightningReceiveClaimPlan) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"cdk/bark/receive-claim-request/v1");
    bytes.extend_from_slice(&claim.action_id.to_bytes());
    bytes.extend_from_slice(&claim.payment_hash.to_vec());
    bytes.extend_from_slice(&claim.destination.locator.index.to_be_bytes());
    bytes.extend_from_slice(&claim.destination.public_key.serialize());
    bytes.extend_from_slice(&claim.amount.to_sat().to_be_bytes());
    bytes.extend_from_slice(&claim.minimum_expiry.to_be_bytes());
    bytes.extend_from_slice(&claim.custody_attempt.to_be_bytes());
    for vtxo in &claim.input_vtxos {
        let encoded = vtxo.serialize();
        bytes.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&encoded);
    }
    bytes
}

fn next_locator_index(projection: &FederationBarkProjectionSnapshot) -> Result<u32, Error> {
    projection
        .derivation_allocations
        .iter()
        .map(|allocation| allocation.locator.index)
        .max()
        .map_or(Ok(0), |index| {
            index
                .checked_add(1)
                .filter(|index| *index <= FEDERATION_MAX_DERIVATION_INDEX)
                .ok_or(Error::MaintenancePlanInvalid)
        })
}

fn bark_artifact(
    kind: FederationBarkArtifactKind,
    bytes: Vec<u8>,
) -> Result<FederationBarkArtifact, Error> {
    Ok(FederationBarkArtifact {
        kind,
        hash: bark_digest(&bytes)?,
        bytes,
    })
}

fn bark_digest(bytes: &[u8]) -> Result<FederationBarkDigest, Error> {
    FederationBarkDigest::from_bytes(Sha256::digest(bytes).into())
        .map_err(|_| Error::ReceiveClaimPlanInvalid)
}

fn receive_external_id(
    domain: &[u8],
    action_id: FederationBarkActionId,
    predecessor: FederationOperationId,
    attempt: u32,
) -> Result<FederationBarkExternalId, Error> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(&action_id.to_bytes());
    bytes.extend_from_slice(&predecessor.to_bytes());
    bytes.extend_from_slice(&attempt.to_be_bytes());
    FederationBarkExternalId::from_bytes(Sha256::digest(bytes).into())
        .map_err(|_| Error::ReceiveClaimPlanInvalid)
}

fn append_bounded_bytes(target: &mut Vec<u8>, bytes: &[u8]) -> Result<(), Error> {
    let length = u64::try_from(bytes.len()).map_err(|_| Error::ReceiveObservationFailed)?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(bytes);
    Ok(())
}

pub(crate) fn validate_wallet_properties(
    properties: &WalletProperties,
    expected_network: bitcoin::Network,
    expected_operational_fingerprint: bitcoin::bip32::Fingerprint,
    expected_descriptor: &KeySourceDescriptor,
    expected_server: PublicKey,
) -> Result<(), Error> {
    if properties.network != expected_network {
        return Err(Error::ConfigMismatch {
            field: "wallet_network",
        });
    }
    if properties.fingerprint != expected_operational_fingerprint {
        return Err(Error::ConfigMismatch {
            field: "operational_seed",
        });
    }
    let descriptor = properties
        .key_source
        .as_ref()
        .ok_or(Error::StandaloneCustodyForbidden)?;
    if descriptor.kind() != KeySourceKind::Frost {
        return Err(Error::StandaloneCustodyForbidden);
    }
    if descriptor != expected_descriptor {
        return Err(Error::CustodyDescriptorMismatch);
    }
    if properties.server_pubkey != Some(expected_server) {
        return Err(Error::ConfigMismatch {
            field: "ark_server_public_key",
        });
    }
    if properties.server_mailbox_pubkey.is_none() {
        return Err(Error::ConfigMismatch {
            field: "ark_server_mailbox_public_key",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_send_quotes_replay_before_and_after_terminal_resolution() {
        assert!(send_quote_state_is_replayable(MeltQuoteState::Unpaid));
        assert!(send_quote_state_is_replayable(MeltQuoteState::Pending));
        assert!(send_quote_state_is_replayable(MeltQuoteState::Paid));
        assert!(send_quote_state_is_replayable(MeltQuoteState::Failed));
        assert!(!send_quote_state_is_replayable(MeltQuoteState::Unknown));
    }

    #[test]
    fn historical_external_application_entries_replay_in_bark_runtime() {
        let operation =
            FederationBarkOperation::MemberObservation(FederationBarkMemberObservation {
                version: FEDERATION_BARK_OPERATION_VERSION,
                action_id: FederationBarkActionId::from_bytes([1; 32]).expect("action ID"),
                quote: Some("historical-quote".to_string()),
                observer: MemberId::new(1).expect("member ID"),
                observation_epoch: 1,
                source_scope: FederationBarkDigest::from_bytes([2; 32]).expect("source scope"),
                payment_hash: Some(
                    FederationBarkDigest::from_bytes([3; 32]).expect("payment hash"),
                ),
                status: FederationBarkObservationStatus::Accepted,
                amount: cdk_common::Amount::from(1),
                fee: cdk_common::Amount::ZERO,
                result_hash: FederationBarkDigest::from_bytes([4; 32]).expect("result hash"),
            });
        let wrapped = cdk_federation_bark::federation_bark_application_operation(operation.clone())
            .expect("historical Bark wire operation");
        assert!(matches!(
            wrapped,
            cdk_common::federation::FederationOperation::ExternalApplication(_)
        ));
        let entry = FederationJournalEntry {
            index: 0,
            operation_id: FederationOperationId::from_bytes([5; 32]),
            envelope: cdk_common::federation::FederationOperationEnvelope::new(
                cdk_common::federation::FederationId::from_bytes([6; 32]),
                wrapped,
            ),
            conflict_keys: Vec::new(),
            previous_order_digest: cdk_common::federation::FederationJournalDigest::genesis(),
            order_digest: cdk_common::federation::FederationJournalDigest::from_bytes([7; 32]),
            consensus_evidence: None,
            application: cdk_common::federation::FederationApplicationState::Applied {},
        };

        assert_eq!(bark_operation(&entry), Some(operation));
    }

    #[test]
    fn legacy_receive_hash_is_accepted_only_without_a_certificate() {
        let evidence = bark::federation::FederationLightningReceiveHtlcEvidence {
            action_id: FederationActionId::from_bytes([1; 32]).expect("action id"),
            payment_hash: PaymentHash::from([2; 32]),
            destination: VtxoKeyLocator { index: 3 },
            gross_amount: bitcoin::Amount::from_sat(1_000),
            net_amount: bitcoin::Amount::from_sat(1_000),
            fee: bitcoin::Amount::ZERO,
            minimum_expiry: 42,
            vtxos: Vec::new(),
        };
        let stored = legacy_receive_htlc_evidence_hash(&evidence).expect("legacy digest");
        let current = bark_digest(b"current").expect("current digest");
        let unrelated = bark_digest(b"unrelated").expect("unrelated digest");

        assert!(receive_htlc_result_hash_matches(
            stored, current, stored, true
        ));
        assert!(!receive_htlc_result_hash_matches(
            stored, current, stored, false
        ));
        assert!(receive_htlc_result_hash_matches(
            current, current, stored, false
        ));
        assert!(!receive_htlc_result_hash_matches(
            unrelated, current, stored, true
        ));
    }

    #[test]
    fn custody_unavailability_survives_backend_error_wrapping() {
        let opaque_error = anyhow::anyhow!("opaque Bark execution failure");

        assert!(matches!(
            classify_send_execution_error(opaque_error, true),
            Error::CustodySignerUnavailable
        ));
        assert!(matches!(
            classify_receive_claim_error(&anyhow::anyhow!("opaque Bark claim failure"), true),
            Error::CustodySignerUnavailable
        ));
        assert!(matches!(
            classify_maintenance_execution_error(
                anyhow::anyhow!("opaque Bark maintenance failure"),
                true,
            ),
            Error::CustodySignerUnavailable
        ));
    }
}
