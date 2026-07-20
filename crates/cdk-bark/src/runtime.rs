//! Member-local Bark wallet runtime and bounded readiness reporting.

use core::fmt;
use std::collections::BTreeSet;
use std::sync::Arc;

use bark::actions::lightning::receive::LightningReceiveAntiDosMode;
use bark::ark::lightning::{Invoice, PaymentHash, Preimage};
use bark::ark::mailbox::MailboxIdentifier;
use bark::ark::ProtocolEncoding;
use bark::custody::{KeySourceDescriptor, KeySourceKind, VtxoCustody, VtxoKeyLocator};
use bark::federation::{
    FederationActionId, FederationLightningReceiveClaimExecution,
    FederationLightningReceiveClaimPlan, FederationLightningReceiveIntent,
    FederationLightningReceivePhase, FederationLightningReceivePlan,
    FederationLightningReceiveStatus, FederationLightningSendCandidate,
    FederationLightningSendHtlcEvidence, FederationLightningSendPlan,
    FederationLightningSendStatus, FederationRequestCommitment, FederationSendInput,
    FederationVtxoImport, FederationVtxoState as BarkFederationVtxoState, FederationWallet,
};
use bark::{OpenWalletArgs, Wallet, WalletProperties};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::PublicKey;
use cdk_common::federation::{
    ark_server_commitment, federation_bark_receive_request_hash, FederationBarkActionKind,
    FederationBarkActionPlan, FederationBarkActionTransition, FederationBarkArtifact,
    FederationBarkArtifactKind, FederationBarkDigest, FederationBarkExternalId,
    FederationBarkMemberObservation, FederationBarkObservationStatus, FederationBarkOperation,
    FederationBarkPolicyConfig, FederationBarkProjectionSnapshot, FederationBarkReceiveIntent,
    FederationBarkReceivePrepared, FederationBarkReceiveSecretBinding, FederationBarkSemanticPhase,
    FederationBarkVtxoId, FederationBarkVtxoInput, FederationBarkVtxoLocator,
    FederationBarkVtxoOutput, FederationBarkVtxoState, FederationBarkVtxoTransition,
    FederationConfig, FederationJournalEntry, FederationOperation, FederationOperationId,
    FederationOperationStatus, MemberId, FEDERATION_BARK_OPERATION_VERSION,
    FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote};
use cdk_common::{CurrencyUnit, MeltQuoteState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::{
    CdkFrostVtxoCustody, Error, FederatedBarkCustodyClient, FederatedBarkLimits,
    FederatedBarkLocalConfig, FederatedBarkReceiveSecretStore, FederatedBarkScope,
    ValidatedFederatedBarkConfig,
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
}

/// Exact cryptographic claim result retained between the two ordered result
/// transitions. Fields remain private so callers cannot forge terminal value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedBarkReceiveClaimResult {
    plan_operation_id: FederationOperationId,
    action_id: cdk_common::federation::FederationBarkActionId,
    quote: String,
    payment_hash: cdk_common::federation::FederationBarkDigest,
    attempt: u32,
    signer_set: Vec<MemberId>,
    input_transitions: Vec<FederationBarkVtxoTransition>,
    output_transitions: Vec<FederationBarkVtxoTransition>,
    artifacts: Vec<FederationBarkArtifact>,
    result_hash: FederationBarkDigest,
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
    limits: FederatedBarkLimits,
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
            || !ark_info.federation_receive_idempotency
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
            limits,
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

    fn bark_application(&self) -> &cdk_common::federation::FederationBarkFrostApplicationConfig {
        self.federation
            .frost_wallet
            .as_ref()
            .and_then(|wallet| wallet.bark_application.as_ref())
            .expect("runtime construction requires an accepted Bark application")
    }

    /// Local federation member represented by this runtime.
    #[must_use]
    pub const fn member_id(&self) -> MemberId {
        self.member_id
    }

    /// Encrypted member-local receive-secret store.
    #[must_use]
    pub fn receive_secret_store(&self) -> Arc<FederatedBarkReceiveSecretStore> {
        Arc::clone(&self.receive_secrets)
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
        let expires_at = minimum_expiry
            .checked_sub(application.policy.emergency_exit_before_expiry_blocks)
            .filter(|height| *height != 0)
            .ok_or(Error::SendPlanInvalid)?;
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
        let action_id = cdk_common::federation::FederationBarkActionId::derive(
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
            artifacts: Vec::new(),
            external_id,
            signer_set: proposal.signer_set,
            attempt: proposal.attempt,
            expires_at: u64::from(expires_at),
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
    ) -> Result<FederationBarkActionTransition, Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        if plan.actor != self.member_id {
            return Err(Error::SendExecutionFailed);
        }
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        let plan_operation = FederationBarkOperation::ActionPlan(plan.clone());
        let plan_commitment = plan_operation
            .commitment()
            .map_err(|_| Error::SendExecutionFailed)?;
        self.custody
            .activate_send(accepted_plan.clone(), plan_commitment.to_bytes().to_vec())
            .map_err(|_| Error::SendExecutionFailed)?;
        let result = self.wallet.execute_lightning_send_htlcs(&bark_plan).await;
        self.custody.deactivate_send();
        let evidence = result.map_err(|_| Error::SendExecutionFailed)?;
        send_htlc_transition(
            self.member_id,
            accepted_plan,
            plan,
            projection,
            &evidence,
            plan_commitment,
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
        validate_send_transition_binding(accepted_plan, plan, transition)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        let evidence = send_htlc_evidence_from_transition(plan, transition)?;
        self.wallet
            .apply_lightning_send_htlcs(&bark_plan, &evidence)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(())
    }

    /// Initiate exactly one already-cosigned outgoing payment. Only the
    /// accepted actor performs the idempotent Ark request.
    pub async fn initiate_lightning_send(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
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
        validate_send_transition_binding(accepted_plan, plan, htlcs)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        self.wallet
            .initiate_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendExecutionFailed)?;
        payment_initiated_transition(self.member_id, accepted_plan, accepted_htlcs, plan, htlcs)
    }

    /// Replay an accepted payment-initiation phase locally without contacting
    /// the Ark server.
    pub async fn apply_lightning_send_initiated(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_initiated: &FederationJournalEntry,
    ) -> Result<(), Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        let transition = accepted_send_transition(
            accepted_initiated,
            FederationBarkSemanticPhase::PaymentInitiated,
        )?;
        validate_send_transition_binding(accepted_plan, plan, transition)?;
        validate_payment_initiated_result(plan, transition)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        self.wallet
            .apply_lightning_send_initiated(&bark_plan)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?;
        Ok(())
    }

    /// Poll one accepted initiated payment and return a success transition only
    /// after Bark verifies the exact preimage. Pending, transport-unknown, and
    /// failure all remain non-terminal here; Milestone 14 owns failure safety.
    pub async fn observe_lightning_send_success(
        &self,
        quote: &MeltQuote,
        projection: &FederationBarkProjectionSnapshot,
        accepted_plan: &FederationJournalEntry,
        accepted_initiated: &FederationJournalEntry,
    ) -> Result<Option<FederationBarkActionTransition>, Error> {
        let plan = accepted_send_plan(accepted_plan)?;
        if plan.actor != self.member_id {
            return Ok(None);
        }
        let initiated = accepted_send_transition(
            accepted_initiated,
            FederationBarkSemanticPhase::PaymentInitiated,
        )?;
        validate_send_transition_binding(accepted_plan, plan, initiated)?;
        validate_payment_initiated_result(plan, initiated)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
        match self
            .wallet
            .poll_lightning_send(&bark_plan)
            .await
            .map_err(|_| Error::SendReconciliationFailed)?
        {
            FederationLightningSendStatus::Succeeded(preimage) => {
                Ok(Some(payment_succeeded_transition(
                    self.member_id,
                    accepted_plan,
                    accepted_initiated,
                    plan,
                    initiated,
                    projection,
                    preimage,
                )?))
            }
            FederationLightningSendStatus::Pending | FederationLightningSendStatus::Failed => {
                Ok(None)
            }
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
        validate_send_transition_binding(accepted_plan, plan, success)?;
        let preimage = send_success_preimage(plan, success)?;
        let bark_plan = self.validate_lightning_send_plan(quote, projection, plan)?;
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
        if quote.unit != CurrencyUnit::Sat || quote.state != MeltQuoteState::Pending {
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

    /// Poll and validate exact incoming HTLC evidence. The returned member
    /// observation is safe to order immediately; the transition candidate must
    /// wait until the federation has accepted the required matching evidence.
    pub async fn observe_receive_htlcs(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared_operation_id: FederationOperationId,
        prepared: &FederationBarkReceivePrepared,
        observation_epoch: u64,
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
            .map_err(|_| Error::ReceiveObservationFailed)?;
        match observation.status {
            FederationLightningReceiveStatus::AwaitingPayment
            | FederationLightningReceiveStatus::Canceled => return Ok(None),
            FederationLightningReceiveStatus::Settled => {
                return Err(Error::ReceiveClaimRecoveryRequired)
            }
            FederationLightningReceiveStatus::Accepted
            | FederationLightningReceiveStatus::HtlcsReady => {}
        }
        let evidence = self
            .wallet
            .prepare_lightning_receive_htlcs(&receive)
            .await
            .map_err(|_| Error::ReceiveObservationFailed)?;
        let result_hash = receive_htlc_evidence_hash(&evidence)?;
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
            result_hash,
        };
        let transition = receive_htlc_transition(
            self.member_id,
            prepared_operation_id,
            intent,
            &evidence,
            result_hash,
        )?;
        Ok(Some((member_observation, transition)))
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
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let claim = self
            .wallet
            .plan_lightning_receive_claim(
                evidence,
                VtxoKeyLocator {
                    index: proposal.output_locator.index,
                },
                proposal.attempt,
            )
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        self.claim_action_plan(
            intent,
            ready_operation_id,
            proposal.actor,
            proposal.signer_set,
            proposal.output_locator,
            &claim,
        )
    }

    /// Reconstruct and compare every field of an incoming claim plan before it
    /// can authorize FROST or virtual MuSig2.
    pub async fn validate_receive_claim_plan(
        &self,
        intent: &FederationBarkReceiveIntent,
        prepared: &FederationBarkReceivePrepared,
        plan: &FederationBarkActionPlan,
    ) -> Result<FederationLightningReceiveClaimPlan, Error> {
        if plan.kind != FederationBarkActionKind::IncomingClaim
            || plan.action_id != intent.action_id
            || plan.quote.as_deref() != Some(intent.quote.as_str())
            || plan.payment_hash != Some(intent.payment_hash)
            || plan.outputs.len() != 1
        {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let receive = self.receive_plan(intent, prepared)?;
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
        self.wallet
            .validate_lightning_receive_claim_plan(&receive, &claim)
            .map_err(|_| Error::ReceiveClaimPlanInvalid)?;
        let expected = self.claim_action_plan(
            intent,
            plan.predecessor_operation_id,
            plan.actor,
            plan.signer_set.clone(),
            output.locator,
            &claim,
        )?;
        if &expected != plan {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        Ok(claim)
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
    ) -> Result<FederatedBarkReceiveClaimOutcome, Error> {
        if accepted_plan.status() != FederationOperationStatus::Applied {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
            &accepted_plan.envelope.operation
        else {
            return Err(Error::ReceiveClaimPlanInvalid);
        };
        let claim = self
            .validate_receive_claim_plan(intent, prepared, plan)
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
        self.custody
            .activate_claim(
                accepted_plan.clone(),
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
        let disclosed = match &result {
            Ok(_) => true,
            Err(_) => matches!(
                self.wallet.lightning_receive_phase(&receive).await,
                Ok(FederationLightningReceivePhase::PreimageRevealed)
            ),
        };
        self.custody.deactivate_claim();
        if !disclosed {
            return Err(Error::ReceiveClaimPlanInvalid);
        }
        let preimage_disclosed = receive_preimage_disclosed_transition(
            self.member_id,
            &accepted_plan,
            intent,
            plan,
            &claim,
            plan_commitment,
        )?;
        match result {
            Ok(outputs) => Ok(FederatedBarkReceiveClaimOutcome::Claimed {
                preimage_disclosed,
                result: Box::new(receive_claim_result(
                    &accepted_plan,
                    intent,
                    plan,
                    &claim,
                    outputs,
                    plan_commitment,
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
        let FederationOperation::Bark(FederationBarkOperation::ActionTransition(disclosed)) =
            &accepted_preimage.envelope.operation
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
            if artifacts.next().is_some() {
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
        if signer_set.len()
            < usize::from(
                self.federation
                    .frost_wallet
                    .as_ref()
                    .and_then(|wallet| wallet.bark_application.as_ref())
                    .ok_or(Error::MissingApplication)?
                    .threshold,
            )
        {
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
}

fn accepted_send_plan(entry: &FederationJournalEntry) -> Result<&FederationBarkActionPlan, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::SendReconciliationFailed);
    }
    let FederationOperation::Bark(FederationBarkOperation::ActionPlan(plan)) =
        &entry.envelope.operation
    else {
        return Err(Error::SendReconciliationFailed);
    };
    if plan.kind != FederationBarkActionKind::LightningSend {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(plan)
}

fn accepted_send_transition(
    entry: &FederationJournalEntry,
    phase: FederationBarkSemanticPhase,
) -> Result<&FederationBarkActionTransition, Error> {
    if entry.status() != FederationOperationStatus::Applied {
        return Err(Error::SendReconciliationFailed);
    }
    let FederationOperation::Bark(FederationBarkOperation::ActionTransition(transition)) =
        &entry.envelope.operation
    else {
        return Err(Error::SendReconciliationFailed);
    };
    if transition.phase != phase {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(transition)
}

fn validate_send_transition_binding(
    accepted_plan: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    if transition.action_id != plan.action_id
        || transition.quote != plan.quote
        || transition.plan_operation_id != accepted_plan.operation_id
        || transition.payment_hash != plan.payment_hash
        || transition.actor != plan.actor
        || transition.attempt != plan.attempt
        || transition.signer_set != plan.signer_set
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
            new_state: FederationBarkVtxoState::PendingSpend,
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
        predecessor_operation_id: accepted_plan.operation_id,
        payment_hash: plan.payment_hash,
        attempt: plan.attempt,
        signer_set: plan.signer_set.clone(),
        request_hash: plan_commitment,
        result_hash,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions,
    })
}

fn payment_initiated_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    accepted_htlcs: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    htlcs: &FederationBarkActionTransition,
) -> Result<FederationBarkActionTransition, Error> {
    let mut result_bytes = Vec::new();
    result_bytes.extend_from_slice(b"cdk/bark/send-payment-initiated/v1");
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(
        &plan
            .payment_hash
            .ok_or(Error::SendExecutionFailed)?
            .to_bytes(),
    );
    result_bytes.extend_from_slice(&htlcs.result_hash.to_bytes());
    result_bytes.extend_from_slice(&plan.external_id.to_bytes());
    let result_artifact = bark_artifact(FederationBarkArtifactKind::ArkResult, result_bytes)?;
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: plan.quote.clone(),
        phase: FederationBarkSemanticPhase::PaymentInitiated,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: accepted_htlcs.operation_id,
        payment_hash: plan.payment_hash,
        attempt: plan.attempt,
        signer_set: plan.signer_set.clone(),
        request_hash: htlcs.result_hash,
        result_hash: result_artifact.hash,
        external_id: plan.external_id,
        artifacts: vec![result_artifact],
        vtxo_transitions: Vec::new(),
    })
}

fn validate_payment_initiated_result(
    plan: &FederationBarkActionPlan,
    transition: &FederationBarkActionTransition,
) -> Result<(), Error> {
    let mut result_bytes = Vec::new();
    result_bytes.extend_from_slice(b"cdk/bark/send-payment-initiated/v1");
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(
        &plan
            .payment_hash
            .ok_or(Error::SendReconciliationFailed)?
            .to_bytes(),
    );
    result_bytes.extend_from_slice(&transition.request_hash.to_bytes());
    result_bytes.extend_from_slice(&plan.external_id.to_bytes());
    let mut artifacts = transition
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == FederationBarkArtifactKind::ArkResult);
    let artifact = artifacts.next().ok_or(Error::SendReconciliationFailed)?;
    if artifacts.next().is_some()
        || artifact.bytes != result_bytes
        || artifact.hash != transition.result_hash
        || artifact.hash != bark_digest(&result_bytes)?
    {
        return Err(Error::SendReconciliationFailed);
    }
    Ok(())
}

fn payment_succeeded_transition(
    actor: MemberId,
    accepted_plan: &FederationJournalEntry,
    accepted_initiated: &FederationJournalEntry,
    plan: &FederationBarkActionPlan,
    initiated: &FederationBarkActionTransition,
    projection: &FederationBarkProjectionSnapshot,
    preimage: Preimage,
) -> Result<FederationBarkActionTransition, Error> {
    let proof = bark_artifact(
        FederationBarkArtifactKind::Proof,
        preimage.as_ref().to_vec(),
    )?;
    let mut result_bytes = b"cdk/bark/send-payment-succeeded/v1".to_vec();
    result_bytes.extend_from_slice(&plan.action_id.to_bytes());
    result_bytes.extend_from_slice(&initiated.result_hash.to_bytes());
    result_bytes.extend_from_slice(preimage.as_ref());
    let result_hash = bark_digest(&result_bytes)?;
    let mut artifacts = vec![proof];
    let mut vtxo_transitions = Vec::with_capacity(plan.inputs.len());
    for input in &plan.inputs {
        let record = projection
            .vtxos
            .iter()
            .find(|record| record.vtxo_id == input.vtxo_id)
            .ok_or(Error::SendExecutionFailed)?;
        if record.locator != input.locator
            || record.amount != input.amount
            || record.state != FederationBarkVtxoState::PendingSpend
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
            previous_state: Some(FederationBarkVtxoState::PendingSpend),
            new_state: FederationBarkVtxoState::Spent,
            artifact_hash: record.artifact.hash,
        });
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
        predecessor_operation_id: accepted_initiated.operation_id,
        payment_hash: plan.payment_hash,
        attempt: plan.attempt,
        signer_set: plan.signer_set.clone(),
        request_hash: initiated.result_hash,
        result_hash,
        external_id: plan.external_id,
        artifacts,
        vtxo_transitions,
    })
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

fn receive_htlc_evidence_hash(
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

fn receive_htlc_transition(
    actor: MemberId,
    prepared_operation_id: FederationOperationId,
    intent: &FederationBarkReceiveIntent,
    evidence: &bark::federation::FederationLightningReceiveHtlcEvidence,
    result_hash: FederationBarkDigest,
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
        request_hash: bark_digest(&request)?,
        result_hash,
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
    result.extend_from_slice(&plan.attempt.to_be_bytes());
    Ok(FederationBarkActionTransition {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: plan.action_id,
        quote: Some(intent.quote.clone()),
        phase: FederationBarkSemanticPhase::ReceivePreimageDisclosed,
        actor,
        plan_operation_id: accepted_plan.operation_id,
        predecessor_operation_id: accepted_plan.operation_id,
        payment_hash: Some(intent.payment_hash),
        attempt: plan.attempt,
        signer_set: plan.signer_set.clone(),
        request_hash: plan_commitment,
        result_hash: bark_digest(&result)?,
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
        attempt: plan.attempt,
        signer_set: plan.signer_set.clone(),
        input_transitions,
        output_transitions,
        artifacts,
        result_hash: bark_digest(&result_bytes)?,
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
    action_id: cdk_common::federation::FederationBarkActionId,
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
