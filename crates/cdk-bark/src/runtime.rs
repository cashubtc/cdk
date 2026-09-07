//! Member-local Bark wallet runtime and bounded readiness reporting.

use core::fmt;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bark::actions::lightning::receive::minimum_lightning_receive_cltv_delta;
use bark::ark::lightning::{PaymentHash, Preimage};
use bark::ark::mailbox::MailboxIdentifier;
use bark::ark::ProtocolEncoding;
use bark::custody::{KeySourceDescriptor, KeySourceKind, VtxoCustody, VtxoKeyLocator};
use bark::federation::{
    FederationActionId, FederationExitEvidence, FederationExitPhase,
    FederationLightningReceiveClaimPlan, FederationLightningReceivePlan,
    FederationLightningSendHtlcEvidence, FederationLightningSendRevocationEvidence,
    FederationRefreshEvidence, FederationRequestCommitment, FederationWallet,
};
use bark::{OpenWalletArgs, Wallet, WalletProperties};
use bitcoin::consensus::{
    deserialize as deserialize_transaction, serialize as serialize_transaction,
};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::PublicKey;
use bitcoin::Transaction;
use cdk_bitcoin_wallet::{
    FederatedDescriptorWallet, FederatedDescriptorWalletConfig, FederatedDescriptorWalletScope,
};
use cdk_common::federation::{
    FederationConfig, FederationJournalEntry, FederationOperationId, FederationOperationStatus,
    MemberId, FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};
use cdk_common::{CurrencyUnit, MeltQuoteState};
use cdk_federation_bark::{
    ark_server_commitment, federation_bark_next_actor, federation_bark_receive_htlc_package_hash,
    federation_bark_recovery_application, federation_bark_signer_set_for_attempt,
    validate_federation_bark_operation, FederationBarkActionId, FederationBarkActionKind,
    FederationBarkActionPlan, FederationBarkActionRecord, FederationBarkActionStatus,
    FederationBarkActionTransition, FederationBarkArtifact, FederationBarkArtifactKind,
    FederationBarkDigest, FederationBarkExternalId, FederationBarkFrostApplicationConfig,
    FederationBarkMemberObservation, FederationBarkOperation, FederationBarkOperatorStatus,
    FederationBarkPolicyConfig, FederationBarkProjectionSnapshot, FederationBarkProtocolStatus,
    FederationBarkReceiveIntent, FederationBarkReceiveSecretCoverage, FederationBarkSemanticPhase,
    FederationBarkVtxoId, FederationBarkVtxoInput, FederationBarkVtxoLocator,
    FederationBarkVtxoState, FederationBarkVtxoStatusCount, FederationBarkVtxoTransition,
    FEDERATION_BARK_OPERATION_VERSION, FEDERATION_BARK_OPERATOR_STATUS_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, RwLock};

mod incoming;
mod maintenance;
mod onchain_recovery;
mod outgoing;

use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkCustodyClient, FederatedBarkLimits,
    FederatedBarkLocalConfig, FederatedBarkMaintenanceControls, FederatedBarkReceiveSecretStore,
    FederatedBarkScope, ValidatedFederatedBarkConfig,
};

/// Keep one long-poll below the payment observer's durable attempt lease.
///
/// The Ark server normally wakes this immediately from its payment-update
/// channel. Bounding it prevents abandoned invoices or stalled server calls
/// from monopolizing a federation worker; the coordinator reconnects quickly.
const BARK_PAYMENT_UPDATE_WAIT: Duration = Duration::from_secs(4);

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
    /// Bark's independently scoped on-chain fee reserve cannot fund recovery.
    RecoveryFeeReserveInsufficient,
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
            Self::RecoveryFeeReserveInsufficient => "recovery_fee_reserve_insufficient",
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
    actor: MemberId,
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
    bark_wallet: Wallet,
    wallet: FederationWallet,
    custody: Arc<CdkFrostVtxoCustody>,
    recovery_signer: Arc<dyn FederatedBarkCustodyClient>,
    receive_secrets: Arc<FederatedBarkReceiveSecretStore>,
    recovery_wallet: Arc<FederatedDescriptorWallet>,
    pub(crate) recovery_advance: Mutex<()>,
    health: RwLock<RuntimeHealth>,
}

impl fmt::Debug for FederatedBarkRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkRuntime")
            .field("scope", &self.scope)
            .field("limits", &self.limits)
            .field("bark_wallet", &"<member-local Bark wallet>")
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
        let recovery_application = federation_bark_recovery_application(federation)
            .map_err(|_| Error::InvalidPublicConfig)?
            .ok_or(Error::InvalidPublicConfig)?;
        if recovery_application.network() != validated.network() {
            return Err(Error::InvalidPublicConfig);
        }
        let frost_wallet = federation
            .frost_wallet
            .as_ref()
            .ok_or(Error::InvalidPublicConfig)?;
        let descriptors = recovery_application
            .descriptors(federation, &frost_wallet.root)
            .map_err(|_| Error::InvalidPublicConfig)?;
        let recovery_scope = FederatedDescriptorWalletScope::new(
            federation.federation_id.to_bytes(),
            recovery_application.application_id().to_bytes(),
            recovery_application.root_key_epoch(),
            recovery_application.application_key_epoch(),
            recovery_application
                .descriptor_commitment(federation, &frost_wallet.root)
                .map_err(|_| Error::InvalidPublicConfig)?,
        )
        .map_err(|_| Error::InvalidPublicConfig)?;
        let recovery_config = FederatedDescriptorWalletConfig::new(
            recovery_scope,
            descriptors.external,
            descriptors.internal,
            recovery_application.network(),
        )
        .map_err(|_| Error::InvalidPublicConfig)?;
        let recovery_wallet = Arc::new(
            FederatedDescriptorWallet::open_with_observation_source(
                recovery_config,
                local.recovery.chain_source.clone(),
                &local.recovery.storage_root,
                local.recovery.observation_source.clone(),
            )
            .map_err(|_| Error::WalletOpenFailed)?,
        );
        let recovery_signer = Arc::clone(&custody_client);
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
        let safe_receive_cltv_delta = minimum_lightning_receive_cltv_delta(
            ark_info.vtxo_exit_delta,
            ark_info.htlc_expiry_delta,
            wallet.config(),
        )
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
                < validated
                    .policy()
                    .minimum_incoming_cltv_delta
                    .max(u32::from(safe_receive_cltv_delta))
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
        let bark_wallet = wallet.clone();
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
            bark_wallet,
            wallet,
            custody,
            recovery_signer,
            receive_secrets,
            recovery_wallet,
            recovery_advance: Mutex::new(()),
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

    /// Advance the signer window while retaining this accepted execution actor.
    pub fn advance_signing_attempt(
        &self,
        action: &FederationBarkActionRecord,
        accepted_plan: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_action_plan(accepted_plan)?;
        if action.action_id != plan.action_id
            || action.actor != self.member_id
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
            self.member_id,
            attempt,
            signer_set,
        )
        .map_err(|_| Error::InvalidCustodyAuthorization)
    }

    /// Transfer execution to the next canonical actor and signer attempt.
    ///
    /// The transition retains the accepted plan and external identity. The
    /// predecessor conflict key ensures that actor and signer retries cannot
    /// both fork the same action head.
    pub fn advance_actor_attempt(
        &self,
        action: &FederationBarkActionRecord,
        accepted_plan: &FederationJournalEntry,
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_action_plan(accepted_plan)?;
        let next_actor = federation_bark_next_actor(&self.federation, action.actor)
            .map_err(|_| Error::InvalidCustodyAuthorization)?;
        if action.action_id != plan.action_id
            || next_actor != self.member_id
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
            self.member_id,
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

    /// Bark-owned threshold recovery wallet; never the CDK payment wallet.
    #[must_use]
    pub fn recovery_wallet(&self) -> Arc<FederatedDescriptorWallet> {
        Arc::clone(&self.recovery_wallet)
    }

    /// Deterministic member selected to originate routine Bark maintenance.
    pub fn maintenance_actor(&self) -> Result<MemberId, Error> {
        self.federation
            .members
            .keys()
            .next()
            .copied()
            .ok_or(Error::InvalidPublicConfig)
    }

    /// Accepted custody signature threshold for maintenance observations.
    #[must_use]
    pub const fn signature_threshold(&self) -> u16 {
        self.federation.threshold_params.signature_threshold
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
    let (actor, attempt, signer_set) = match &operation {
        FederationBarkOperation::ActionPlan(authorized_plan)
            if authorization_entry.operation_id == plan_entry.operation_id
                && authorization_entry.index == plan_entry.index
                && authorized_plan == &plan =>
        {
            (plan.actor, plan.attempt, plan.signer_set.clone())
        }
        FederationBarkOperation::ActionTransition(transition)
            if transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced
                && transition.action_id == plan.action_id
                && transition.quote == plan.quote
                && transition.plan_operation_id == plan_entry.operation_id
                && transition.payment_hash == plan.payment_hash
                && transition.external_id == plan.external_id =>
        {
            (
                transition.actor,
                transition.attempt,
                transition.signer_set.clone(),
            )
        }
        _ => return Err(Error::InvalidCustodyAuthorization),
    };
    Ok(AcceptedCustodyAttempt {
        entry: authorization_entry.clone(),
        actor,
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
    if transition.result_hash != current_result_hash
        || transition.request_hash != bark_digest(&request)?
    {
        return Err(Error::ReceiveClaimPlanInvalid);
    }
    Ok(evidence)
}

fn certified_receive_htlc_transition(
    plan: &FederationBarkActionPlan,
) -> Result<FederationBarkActionTransition, Error> {
    let certified = plan
        .certified_receive_htlcs
        .as_deref()
        .ok_or(Error::ReceiveClaimPlanInvalid)?;
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
    Ok(transition)
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
    use cdk_federation_bark::FederationBarkObservationStatus;

    #[test]
    fn accepted_send_quotes_replay_before_and_after_terminal_resolution() {
        assert!(send_quote_state_is_replayable(MeltQuoteState::Unpaid));
        assert!(send_quote_state_is_replayable(MeltQuoteState::Pending));
        assert!(send_quote_state_is_replayable(MeltQuoteState::Paid));
        assert!(send_quote_state_is_replayable(MeltQuoteState::Failed));
        assert!(!send_quote_state_is_replayable(MeltQuoteState::Unknown));
    }

    #[test]
    fn current_external_application_entries_replay_in_bark_runtime() {
        let operation =
            FederationBarkOperation::MemberObservation(FederationBarkMemberObservation {
                version: FEDERATION_BARK_OPERATION_VERSION,
                action_id: FederationBarkActionId::from_bytes([1; 32]).expect("action ID"),
                quote: Some("current-quote".to_string()),
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
            .expect("current Bark wire operation");
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
