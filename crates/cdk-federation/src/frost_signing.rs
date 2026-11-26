//! Application-aware FROST signing runtime and private REST handlers.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
#[cfg(feature = "federation-bdk")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "federation-bark")]
use cdk_common::database::mint::FederationJournalEntryKey;

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use super::signer_fan_out::{
    concurrency_limit, fan_out, known_signer_members, signer_members, LocalFrostSigner,
};
#[cfg(feature = "federation-bark")]
use cdk::federation::{
    FederationBarkActionKind, FederationBarkCustodyProtocol, FederationBarkFrostAuthorization,
    FederationBarkFrostRegistrationAdapter, FederationBarkProtocolStatus,
    FederationBarkRecoveryFrostAuthorization, FederationBarkRecoveryFrostAuthorizationAdapter,
    FederationBarkSemanticPhase,
};
#[cfg(feature = "federation-bdk")]
use cdk::federation::{
    FederationCdkOnchainFrostAuthorization, FederationCdkOnchainFrostAuthorizationAdapter,
    FederationFrostInputSignatureAudit, FederationSignedTransaction,
    FederationTransactionProposalId,
};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk::federation::{
    FederationFrostAggregateResultNotification, FederationFrostCommitmentRequest,
    FederationFrostSigningAbortRequest, FederationFrostSigningPackageRequest, MemberConfigPublic,
};
use cdk::mint::{FederationRuntimeConfig, Mint};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk_frost::DerivedPublicPackage;
use cdk_frost::{
    AuthorizationRequest, NonceCommitment, ParticipantKeyManager, RedbNonceStore,
    SecretEncryptionKey, SignatureShare, SigningDerivation, SigningSessionContext,
};
#[cfg(feature = "federation-bark")]
use cdk_frost::{
    CustodyProtocol, ThresholdDhRequest, ThresholdDhShare, VirtualMusigNonceCommitment,
    VirtualMusigNonceCommitmentSet, VirtualMusigNonceId, VirtualMusigPartialShare,
    VirtualMusigTranscript,
};
#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
use cdk_frost_bip32::{FrostBip32, WalletBranch, WalletIndex};
use sha2::{Digest, Sha256};

use crate::FederationNode;

const RESULT_CACHE_LIMIT: usize = 1_024;
#[cfg(feature = "federation-bark")]
const BARK_CUSTODY_PROJECTION_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(feature = "federation-bark")]
const BARK_CUSTODY_PROJECTION_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(20);

#[cfg(feature = "federation-bark")]
const fn bark_action_authorizes_output_locator(kind: FederationBarkActionKind) -> bool {
    // A refresh is the only accepted action whose newly allocated output key
    // participates in custody before the result transition exists.
    matches!(kind, FederationBarkActionKind::Refresh)
}

#[cfg(all(test, feature = "federation-bark"))]
mod bark_custody_authorization_tests {
    use cdk::federation::FederationBarkActionKind;

    use super::{bark_action_authorizes_output_locator, bark_custody_inputs_are_authorized};

    fn pending_claim_input() -> (
        cdk::federation::FederationBarkActionPlan,
        cdk::federation::FederationBarkProjectionSnapshot,
    ) {
        use cdk::federation::{
            FederationBarkActionId, FederationBarkActionPlan, FederationBarkArtifact,
            FederationBarkArtifactKind, FederationBarkDigest, FederationBarkExternalId,
            FederationBarkProjectionSnapshot, FederationBarkVtxoId, FederationBarkVtxoInput,
            FederationBarkVtxoLocator, FederationBarkVtxoRecord, FederationBarkVtxoState,
            FederationOperationId, MemberId, FEDERATION_BARK_OPERATION_VERSION,
        };

        // Only projection authorization is under test. Plan/artifact validation
        // and cryptographic authorization are separate, earlier boundaries.
        let action_id = FederationBarkActionId::from_bytes([1; 32]).expect("action");
        let operation_id = FederationOperationId::from_bytes([2; 32]);
        let member = MemberId::new(1).expect("member");
        let input = FederationBarkVtxoInput {
            vtxo_id: FederationBarkVtxoId::from_bytes([3; 36]).expect("VTXO"),
            locator: FederationBarkVtxoLocator::vtxo_custody(7),
            amount: 2_048.into(),
        };
        let plan = FederationBarkActionPlan {
            version: FEDERATION_BARK_OPERATION_VERSION,
            action_id,
            quote: Some("projection-claim".to_string()),
            kind: FederationBarkActionKind::IncomingClaim,
            actor: member,
            predecessor_operation_id: operation_id,
            payment_hash: Some(FederationBarkDigest::from_bytes([4; 32]).expect("payment hash")),
            inputs: vec![input],
            outputs: Vec::new(),
            revocation_locator: None,
            external_amount: 0.into(),
            fee_amount: 0.into(),
            fee_bound: 0.into(),
            artifacts: Vec::new(),
            certified_receive_htlcs: None,
            external_id: FederationBarkExternalId::from_bytes([5; 32]).expect("external ID"),
            signer_set: vec![member],
            attempt: 1,
            expires_at: 1_000,
        };
        let projection = FederationBarkProjectionSnapshot {
            vtxos: vec![FederationBarkVtxoRecord {
                vtxo_id: input.vtxo_id,
                locator: input.locator,
                amount: input.amount,
                expiry_height: 1_000,
                state: FederationBarkVtxoState::PendingSpend,
                artifact: FederationBarkArtifact {
                    kind: FederationBarkArtifactKind::Vtxo,
                    hash: FederationBarkDigest::from_bytes([6; 32]).expect("artifact hash"),
                    bytes: vec![6; 32],
                },
                reservation_action_id: Some(action_id),
                origin_operation_id: operation_id,
                origin_operation_index: 1,
                latest_operation_id: operation_id,
                latest_operation_index: 2,
            }],
            ..Default::default()
        };
        (plan, projection)
    }

    #[test]
    fn bark_custody_pending_claim_input_retains_its_reservation() {
        let (plan, projection) = pending_claim_input();
        assert!(bark_custody_inputs_are_authorized(
            &plan,
            cdk::federation::FederationBarkSemanticPhase::ReceivePreimageDisclosed,
            &projection.vtxos,
        ));
    }

    #[test]
    fn pending_claim_input_requires_the_disclosed_incoming_phase() {
        use cdk::federation::{
            FederationBarkActionKind as Kind, FederationBarkSemanticPhase as Phase,
        };

        let (mut plan, projection) = pending_claim_input();
        for (kind, phase) in [
            (Kind::IncomingClaim, Phase::ClaimPlanned),
            (Kind::IncomingClaim, Phase::ReceiveClaimed),
            (Kind::LightningSend, Phase::SendPlanned),
            (Kind::FailedSendRevocation, Phase::RevocationPlanned),
            (Kind::Refresh, Phase::RefreshPlanned),
            (Kind::EmergencyExit, Phase::ExitRegistered),
        ] {
            plan.kind = kind;
            assert!(!bark_custody_inputs_are_authorized(
                &plan,
                phase,
                &projection.vtxos
            ));
        }
    }

    #[test]
    fn pending_claim_input_preserves_exact_plan_binding() {
        use cdk::federation::{
            FederationBarkActionId, FederationBarkSemanticPhase as Phase, FederationBarkVtxoId,
        };

        let (plan, projection) = pending_claim_input();
        let mut mismatches = Vec::new();
        let mut wrong_id = projection.clone();
        wrong_id.vtxos[0].vtxo_id =
            FederationBarkVtxoId::from_bytes([9; 36]).expect("different VTXO");
        mismatches.push(wrong_id);
        let mut wrong_locator = projection.clone();
        wrong_locator.vtxos[0].locator.index += 1;
        mismatches.push(wrong_locator);
        let mut wrong_amount = projection.clone();
        wrong_amount.vtxos[0].amount = 2_049.into();
        mismatches.push(wrong_amount);
        let mut unreserved = projection.clone();
        unreserved.vtxos[0].reservation_action_id = None;
        mismatches.push(unreserved);
        let mut wrong_owner = projection.clone();
        wrong_owner.vtxos[0].reservation_action_id =
            Some(FederationBarkActionId::from_bytes([9; 32]).expect("different action"));
        mismatches.push(wrong_owner);
        let mut absent = projection;
        absent.vtxos.clear();
        mismatches.push(absent);
        for projection in mismatches {
            assert!(!bark_custody_inputs_are_authorized(
                &plan,
                Phase::ReceivePreimageDisclosed,
                &projection.vtxos,
            ));
        }
    }

    #[test]
    fn custody_input_state_preserves_reservation_and_exit_boundaries() {
        use cdk::federation::{
            FederationBarkActionKind as Kind, FederationBarkSemanticPhase as Phase,
            FederationBarkVtxoState as State,
        };

        let (mut plan, mut projection) = pending_claim_input();
        projection.vtxos[0].state = State::Reserved;
        assert!(bark_custody_inputs_are_authorized(
            &plan,
            Phase::ClaimPlanned,
            &projection.vtxos
        ));
        for state in [
            State::Observed,
            State::Spendable,
            State::Spent,
            State::Exiting,
            State::Exited,
        ] {
            projection.vtxos[0].state = state;
            assert!(!bark_custody_inputs_are_authorized(
                &plan,
                Phase::ReceivePreimageDisclosed,
                &projection.vtxos
            ));
        }
        plan.kind = Kind::EmergencyExit;
        projection.vtxos[0].state = State::Exiting;
        assert!(bark_custody_inputs_are_authorized(
            &plan,
            Phase::ExitRegistered,
            &projection.vtxos
        ));
    }

    #[test]
    fn only_refresh_authorizes_an_output_locator() {
        assert!(bark_action_authorizes_output_locator(
            FederationBarkActionKind::Refresh
        ));
        for kind in [
            FederationBarkActionKind::LightningSend,
            FederationBarkActionKind::IncomingClaim,
            FederationBarkActionKind::FailedSendRevocation,
            FederationBarkActionKind::EmergencyExit,
        ] {
            assert!(!bark_action_authorizes_output_locator(kind));
        }
    }
}

type AggregateResultCache = BTreeMap<[u8; 32], ([u8; 32], [u8; 64])>;

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
struct CoordinatedFrostSignature {
    signature: cdk_frost::AggregateSignature,
    /// Read only by on-chain signing, which records it in the input audit.
    #[cfg(feature = "federation-bdk")]
    package_hash: [u8; 32],
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
#[derive(Debug, Clone)]
/// Correlation metadata for one application-neutral FROST signing request.
pub struct FrostLatencyContext {
    correlation: super::latency::FederationLatencyCorrelation,
    correlation_id: String,
    backend: super::latency::FederationLatencyBackend,
    direction: super::latency::FederationLatencyDirection,
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
impl FrostLatencyContext {
    /// Construct bounded latency metadata without exposing it to consensus.
    #[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
    pub fn new(
        correlation: super::latency::FederationLatencyCorrelation,
        correlation_id: impl Into<String>,
        backend: super::latency::FederationLatencyBackend,
        direction: super::latency::FederationLatencyDirection,
    ) -> Self {
        Self {
            correlation,
            correlation_id: correlation_id.into(),
            backend,
            direction,
        }
    }
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
/// Application-neutral coordinator for one exact selected signer set.
#[derive(Debug)]
pub struct FederationFrostSigningCoordinator<'a, T> {
    client: &'a super::peer_client::FederationRestPeerClient<T>,
    public: DerivedPublicPackage,
    local_signer: Option<LocalFrostSigner<'a>>,
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
impl<'a, T> FederationFrostSigningCoordinator<'a, T>
where
    T: cdk_http_client::Transport,
{
    /// Construct a coordinator without participant secret material.
    pub fn new(
        client: &'a super::peer_client::FederationRestPeerClient<T>,
        public: DerivedPublicPackage,
    ) -> Self {
        Self {
            client,
            public,
            local_signer: None,
        }
    }

    /// Use the member-local signer runtime without routing back through the
    /// member's own private REST endpoint. The local path invokes the same
    /// authorization validator and durable nonce store as the REST handlers.
    pub fn with_local_signer(
        mut self,
        member_id: cdk::federation::MemberId,
        mint: &'a Mint,
        runtime: &'a FederationFrostSigningRuntime,
    ) -> Self {
        self.local_signer = Some(LocalFrostSigner {
            member_id,
            mint,
            runtime,
        });
        self
    }

    /// Run both rounds, verify every response, aggregate, and notify signers.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn sign(
        &self,
        public_config: &cdk::federation::FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<cdk_frost::AggregateSignature> {
        Ok(self
            .sign_with_audit(public_config, context, derivation, authorization, None)
            .await?
            .signature)
    }

    /// Run both rounds while attributing their latency to the calling backend.
    #[cfg(feature = "federation-bark")]
    pub async fn sign_with_latency(
        &self,
        public_config: &cdk::federation::FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        latency: &FrostLatencyContext,
    ) -> anyhow::Result<cdk_frost::AggregateSignature> {
        Ok(self
            .sign_with_audit(
                public_config,
                context,
                derivation,
                authorization,
                Some(latency),
            )
            .await?
            .signature)
    }

    async fn sign_with_audit(
        &self,
        public_config: &cdk::federation::FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        latency: Option<&FrostLatencyContext>,
    ) -> anyhow::Result<CoordinatedFrostSignature> {
        if context.signer_set().len() < usize::from(self.public.threshold()) {
            anyhow::bail!("FROST signer set is below threshold");
        }
        let commitment_request =
            FederationFrostCommitmentRequest::new(context, derivation, authorization);
        let signer_members = signer_members(public_config, context.signer_set())?;
        let limit = concurrency_limit(self.client, signer_members.len());
        let default_correlation_id =
            cdk::federation::FederationOperationId::from_bytes(context.session_id().to_bytes())
                .to_string();
        let (correlation, correlation_id, backend, direction) = latency.map_or(
            (
                super::latency::FederationLatencyCorrelation::Operation,
                default_correlation_id.as_str(),
                super::latency::FederationLatencyBackend::Consensus,
                super::latency::FederationLatencyDirection::Internal,
            ),
            |latency| {
                (
                    latency.correlation,
                    latency.correlation_id.as_str(),
                    latency.backend,
                    latency.direction,
                )
            },
        );
        let commitment_timer = super::latency::FederationLatencyTimer::start(
            correlation,
            correlation_id.to_owned(),
            backend,
            direction,
            super::latency::FederationLatencyPhase::FrostCommitmentRound,
        );
        let commitments = fan_out(
            signer_members
                .iter()
                .map(|(_, member)| (*member, ()))
                .collect(),
            self.local_signer,
            limit,
            |local, ()| async move {
                local
                    .runtime
                    .commit(local.mint, context, derivation, authorization)
                    .await
                    .map_err(anyhow::Error::from)
            },
            |member, ()| self.request_frost_commitment_resilient(member, &commitment_request),
        )
        .await;
        commitment_timer.finish(super::latency::FederationLatencyOutcome::from_success(
            commitments.is_ok(),
        ));
        let commitments = match commitments {
            Ok(commitments) => commitments,
            Err(error) => {
                self.abort_selected(
                    public_config,
                    context,
                    authorization,
                    cdk_frost::NonceBurnReason::AmbiguousResponse,
                )
                .await;
                return Err(error);
            }
        };
        let commitments = cdk_frost::NonceCommitmentSet::new(commitments)?;
        let signing_packages = signer_members
            .iter()
            .map(|(signer, member)| {
                let commitment = commitments
                    .commitments()
                    .get(signer)
                    .ok_or_else(|| anyhow::anyhow!("selected signer commitment is missing"))?;
                let package = FederationFrostSigningPackageRequest::new(
                    context,
                    commitment.nonce_id(),
                    authorization,
                    &commitments,
                )?;
                Ok((*member, package))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let share_timer = super::latency::FederationLatencyTimer::start(
            correlation,
            correlation_id.to_owned(),
            backend,
            direction,
            super::latency::FederationLatencyPhase::FrostSignatureRound,
        );
        let shares = fan_out(
            signing_packages
                .iter()
                .map(|(member, package)| (*member, package))
                .collect(),
            self.local_signer,
            limit,
            |local, package| async move {
                let (decoded_context, nonce_id, decoded_authorization, commitments) =
                    package.decode(public_config.federation_id, local.member_id)?;
                local
                    .runtime
                    .sign(
                        local.mint,
                        &decoded_context,
                        nonce_id,
                        &decoded_authorization,
                        &commitments,
                    )
                    .await
                    .map_err(anyhow::Error::from)
            },
            |member, package| self.request_frost_signature_share_resilient(member, package),
        )
        .await;
        share_timer.finish(super::latency::FederationLatencyOutcome::from_success(
            shares.is_ok(),
        ));
        let shares = match shares {
            Ok(shares) => shares,
            Err(error) => {
                self.abort_selected(
                    public_config,
                    context,
                    authorization,
                    cdk_frost::NonceBurnReason::AmbiguousResponse,
                )
                .await;
                return Err(error);
            }
        };
        let signature = ParticipantKeyManager::aggregate_for_derivation(
            &self.public,
            derivation,
            context.session_id(),
            &context.message_commitment(),
            &commitments,
            shares,
        )?;
        let package_hash = cdk_frost::signing_package_hash(
            context.session_id(),
            &context.message_commitment(),
            &commitments,
        )?;
        let notification = FederationFrostAggregateResultNotification::new(
            context,
            authorization,
            package_hash,
            signature,
        );
        fan_out(
            signer_members
                .iter()
                .map(|(_, member)| (*member, ()))
                .collect(),
            self.local_signer,
            limit,
            |local, ()| async move {
                local
                    .runtime
                    .record_aggregate(local.mint, context, authorization, package_hash, signature)
                    .await
                    .map_err(anyhow::Error::from)
            },
            |member, ()| self.notify_frost_aggregate_result_resilient(member, &notification),
        )
        .await?;
        Ok(CoordinatedFrostSignature {
            signature,
            #[cfg(feature = "federation-bdk")]
            package_hash,
        })
    }

    /// Sign every input, finalize through BDK, then order the signed record in consensus.
    #[cfg(feature = "federation-bdk")]
    pub async fn sign_accepted_transaction(
        &self,
        state: &FederationNode,
        wallet: &cdk_bdk::FederatedBdkWallet,
        proposal_id: FederationTransactionProposalId,
        attempt: u32,
        signer_set: Vec<cdk_frost::MemberId>,
    ) -> anyhow::Result<FederationSignedTransaction> {
        state
            .ensure_federation_ready_for_authoritative_read()
            .await?;
        if let Some(reason) = state
            .federation
            .components
            .wallet
            .bdk_degraded_reason()
            .await
        {
            anyhow::bail!(
                "federation local signing is blocked while the member is BDK-degraded: {reason}"
            );
        }
        let transaction_state = state.mint.federation_transaction_state().await?;
        let existing = transaction_state
            .accepted_transaction(proposal_id)
            .ok_or_else(|| anyhow::anyhow!("accepted transaction is missing"))?;
        if let Some(signed) = &existing.signed {
            state
                .mint
                .validate_federation_signed_transaction_submission(signed)
                .await?;
            wallet
                .engine()
                .apply_accepted_transaction(
                    signed.transaction()?,
                    existing.accepted_operation_index,
                )
                .await?;
            return Ok(signed.clone());
        }
        let accepted = state
            .mint
            .federation_transaction_signing_authorization(proposal_id)
            .await?;
        let latency = FrostLatencyContext::new(
            super::latency::FederationLatencyCorrelation::Proposal,
            proposal_id.to_string(),
            super::latency::FederationLatencyBackend::Bdk,
            super::latency::FederationLatencyDirection::Outgoing,
        );
        let runtime = state
            .mint
            .federation_config()
            .ok_or_else(|| anyhow::anyhow!("federation configuration is unavailable"))?;
        let scope = wallet.scope();
        if scope.root_key_epoch() != accepted.proposal.scope.root_key_epoch
            || scope.application_id() != accepted.proposal.scope.application_id.to_bytes()
            || scope.application_key_epoch() != accepted.proposal.scope.application_key_epoch
        {
            anyhow::bail!("accepted transaction uses a different local BDK wallet scope");
        }
        let adapter = FrostBip32::new(self.public.clone(), accepted.proposal.scope.network)?;
        let mut signatures = Vec::with_capacity(accepted.proposal.selected_inputs.len());
        let mut audits = Vec::with_capacity(accepted.proposal.selected_inputs.len());
        for (input_index, selected) in accepted.proposal.selected_inputs.iter().enumerate() {
            let branch = match selected.allocation.keychain {
                cdk::federation::FederationAddressKeychain::External => WalletBranch::External,
                cdk::federation::FederationAddressKeychain::Internal => WalletBranch::Internal,
            };
            let index = WalletIndex::new(selected.allocation.derivation_index)?;
            let derived = adapter.derive(branch, index)?;
            let typed = FederationCdkOnchainFrostAuthorization::new(
                &accepted,
                input_index,
                derived.signing_derivation(),
            )?;
            let authorization_scope = cdk_frost::SigningAuthorizationScope::new(
                runtime.federation_id().to_bytes(),
                cdk_frost::RootKeyEpoch::new(accepted.proposal.scope.root_key_epoch)?,
                self.public.registration(),
                derived.signing_derivation().binding(),
                typed.authorization_reference,
                typed.message,
            )?;
            let context = SigningSessionContext::new(
                authorization_scope,
                attempt,
                signer_set.iter().copied(),
            )?;
            let authorization = AuthorizationRequest::new(
                self.public.registration().id(),
                self.public.registration().key_epoch(),
                self.public.registration().derivation_version(),
                context.session_id(),
                typed.to_bytes()?,
            )?;
            let result = self
                .sign_with_audit(
                    runtime.public_config(),
                    &context,
                    derived.signing_derivation(),
                    &authorization,
                    Some(&latency),
                )
                .await?;
            signatures.push(result.signature.to_bytes());
            audits.push(FederationFrostInputSignatureAudit {
                input_index: u32::try_from(input_index)?,
                session_id: context.session_id().to_bytes(),
                attempt,
                signer_set: context
                    .signer_set()
                    .iter()
                    .map(|member| member.get())
                    .collect(),
                package_hash: result.package_hash,
                signature: result.signature.to_bytes().to_vec(),
            });
        }
        let transaction = wallet
            .engine()
            .finalize_federated_transaction(
                &accepted.proposal.unsigned_transaction,
                &accepted
                    .proposal
                    .selected_inputs
                    .iter()
                    .map(|input| cdk_bitcoin_wallet::FederatedTransactionInput {
                        outpoint: input.outpoint,
                        previous_output: input.previous_output.clone(),
                        keychain: match input.allocation.keychain {
                            cdk::federation::FederationAddressKeychain::External => {
                                cdk_bitcoin_wallet::FederatedWalletKeychain::External.into()
                            }
                            cdk::federation::FederationAddressKeychain::Internal => {
                                cdk_bitcoin_wallet::FederatedWalletKeychain::Internal.into()
                            }
                        },
                        derivation_index: input.allocation.derivation_index,
                    })
                    .collect::<Vec<_>>(),
                &signatures,
            )
            .await?;
        let signed = FederationSignedTransaction::new(&accepted, &transaction, audits)?;
        signed.verify_signatures(&accepted.proposal)?;
        let envelope = runtime.transaction_signed_operation(signed.clone());
        state
            .submit_federation_envelope(envelope)
            .await?
            .ensure_accepted()?;
        let durable = state
            .mint
            .federation_transaction_state()
            .await?
            .accepted_transaction(proposal_id)
            .and_then(|accepted| accepted.signed.as_ref())
            .ok_or_else(|| anyhow::anyhow!("accepted signed transaction is not materialized"))?
            .clone();
        if durable != signed {
            anyhow::bail!("materialized signed transaction differs from the submitted record");
        }
        Ok(durable)
    }

    async fn abort_selected(
        &self,
        public_config: &cdk::federation::FederationConfig,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        reason: cdk_frost::NonceBurnReason,
    ) {
        let request = FederationFrostSigningAbortRequest::new(context, authorization, reason);
        let members = known_signer_members(public_config, context.signer_set());
        let limit = concurrency_limit(self.client, members.len());
        // Best effort: every selected signer is told, and failures are ignored.
        let _ = fan_out(
            members.into_iter().map(|member| (member, ())).collect(),
            self.local_signer,
            limit,
            |local, ()| async move {
                local
                    .runtime
                    .abort(local.mint, context, authorization, reason)
                    .await
                    .map_err(anyhow::Error::from)
            },
            |member, ()| self.client.abort_frost_signing(member, &request),
        )
        .await;
    }

    async fn request_frost_commitment_resilient(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostCommitmentRequest,
    ) -> anyhow::Result<NonceCommitment> {
        match self.client.request_frost_commitment(member, request).await {
            Err(error) if frost_transport_error_is_retryable(&error) => {
                tracing::debug!(
                    member = %member.member_id,
                    "Retrying the same idempotent FROST commitment session after a transport error"
                );
                self.client.request_frost_commitment(member, request).await
            }
            result => result,
        }
    }

    async fn request_frost_signature_share_resilient(
        &self,
        member: &MemberConfigPublic,
        request: &FederationFrostSigningPackageRequest,
    ) -> anyhow::Result<SignatureShare> {
        match self
            .client
            .request_frost_signature_share(member, request)
            .await
        {
            Err(error) if frost_transport_error_is_retryable(&error) => {
                tracing::debug!(
                    member = %member.member_id,
                    "Retrying the same idempotent FROST signature session after a transport error"
                );
                self.client
                    .request_frost_signature_share(member, request)
                    .await
            }
            result => result,
        }
    }

    async fn notify_frost_aggregate_result_resilient(
        &self,
        member: &MemberConfigPublic,
        notification: &FederationFrostAggregateResultNotification,
    ) -> anyhow::Result<()> {
        match self
            .client
            .notify_frost_aggregate_result(member, notification)
            .await
        {
            Err(error) if frost_transport_error_is_retryable(&error) => {
                tracing::debug!(
                    member = %member.member_id,
                    "Retrying the same FROST aggregate result after a transport error"
                );
                self.client
                    .notify_frost_aggregate_result(member, notification)
                    .await
            }
            result => result,
        }
    }
}

#[cfg(any(feature = "federation-bdk", feature = "federation-bark"))]
fn frost_transport_error_is_retryable(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        if let Some(peer_error) =
            source.downcast_ref::<super::peer_client::FederationRestPeerClientError>()
        {
            return matches!(
                peer_error,
                super::peer_client::FederationRestPeerClientError::RequestTimeout { .. }
            );
        }
        match source.downcast_ref::<cdk_http_client::HttpError>() {
            Some(
                cdk_http_client::HttpError::Connection(_)
                | cdk_http_client::HttpError::Timeout
                | cdk_http_client::HttpError::Proxy(_)
                | cdk_http_client::HttpError::Other(_),
            ) => true,
            Some(cdk_http_client::HttpError::Status { status, .. }) => *status >= 500,
            Some(
                cdk_http_client::HttpError::Serialization(_)
                | cdk_http_client::HttpError::Redirect(_)
                | cdk_http_client::HttpError::ResponseTooLarge { .. }
                | cdk_http_client::HttpError::Build(_),
            )
            | None => false,
        }
    })
}

/// One participant's long-lived, application-neutral FROST key boundary.
#[derive(Clone)]
pub struct FederationFrostSigningRuntime {
    managers: Arc<BTreeMap<u64, Arc<tokio::sync::Mutex<ParticipantKeyManager>>>>,
    aggregate_results: Arc<tokio::sync::Mutex<AggregateResultCache>>,
    authorizer: Arc<dyn SigningAuthorizer>,
}

impl std::fmt::Debug for FederationFrostSigningRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FederationFrostSigningRuntime")
            .field("root_epochs", &self.managers.keys())
            .field("key_material", &"<redacted>")
            .finish()
    }
}

impl FederationFrostSigningRuntime {
    /// Open durable nonce stores, root shares, and reviewed application adapters.
    ///
    /// Nonce stores live in `<storage_root>/frost-nonces/root-<epoch>.redb`,
    /// apart from the mint's SQL database. They must never be restored from a
    /// backup; see `cdk-frost`'s README.
    pub async fn open(mint: &Mint, storage_root: &Path) -> anyhow::Result<Option<Self>> {
        let Some(runtime) = mint.federation_config() else {
            return Ok(None);
        };
        let Some(wallet) = runtime.public_config().frost_wallet.as_ref() else {
            return Ok(None);
        };
        let private_roots = mint
            .localstore()
            .get_federation_frost_root_private_configs()
            .await?;
        let nonce_root = storage_root.join("frost-nonces");
        std::fs::create_dir_all(&nonce_root)?;
        #[cfg(unix)]
        std::fs::set_permissions(&nonce_root, std::fs::Permissions::from_mode(0o700))?;
        let mut managers = BTreeMap::new();
        let public_root = &wallet.root;
        let private_root = private_roots
            .iter()
            .find(|root| root.root_key_epoch == public_root.root_key_epoch)
            .ok_or_else(|| anyhow::anyhow!("local FROST root share is missing"))?;
        let context = cdk::federation::FederationFrostDkgContext::new(
            runtime.public_config(),
            public_root.ceremony_id,
            public_root.root_key_epoch,
        )?;
        let root_key = super::dkg_driver::frost_storage_key(runtime.private_config(), &context)?;
        let root = private_root.open(
            runtime.public_config(),
            public_root,
            runtime.member_id(),
            &root_key,
        )?;
        let nonce_key = nonce_encryption_key(runtime, public_root.root_key_epoch)?;
        let nonce_path = nonce_root.join(format!("root-{}.redb", public_root.root_key_epoch));
        let store = RedbNonceStore::open(&nonce_path)?;
        #[cfg(unix)]
        std::fs::set_permissions(&nonce_path, std::fs::Permissions::from_mode(0o600))?;
        #[allow(unused_mut)]
        let mut manager = ParticipantKeyManager::with_nonce_store(root, nonce_key, store)?;
        #[cfg(feature = "federation-bdk")]
        if let Some(application) = wallet.cdk_onchain_application.as_ref() {
            manager.register_application(FederationCdkOnchainFrostAuthorizationAdapter::new(
                application,
            )?)?;
        }
        #[cfg(feature = "federation-bark")]
        if let Some(bark) = cdk::federation::federation_bark_application(runtime.public_config())? {
            let derived = manager
                .register_application(FederationBarkFrostRegistrationAdapter::new(&bark)?)?;
            if derived.group_key() != bark.derived_group_key
                || derived.verifying_shares().len() != bark.verification_shares.len()
                || derived.verifying_shares().iter().any(|(member, share)| {
                    let Ok(member) = cdk::federation::MemberId::new(member.get()) else {
                        return true;
                    };
                    bark.verification_shares.get(&member) != Some(share)
                })
            {
                return Err(anyhow::anyhow!(
                    "participant Bark application derivation differs from public config"
                ));
            }
        }
        #[cfg(feature = "federation-bark")]
        if let Some(recovery) =
            cdk::federation::federation_bark_recovery_application(runtime.public_config())?
        {
            let derived = manager.register_application(
                FederationBarkRecoveryFrostAuthorizationAdapter::new(&recovery)?,
            )?;
            let expected = recovery.derived_public_package(runtime.public_config(), public_root)?;
            if derived != expected {
                return Err(anyhow::anyhow!(
                    "participant Bark recovery derivation differs from public config"
                ));
            }
        }
        managers.insert(
            public_root.root_key_epoch,
            Arc::new(tokio::sync::Mutex::new(manager)),
        );
        Ok(Some(Self {
            managers: Arc::new(managers),
            aggregate_results: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            authorizer: Arc::new(MintSigningAuthorizer),
        }))
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub fn for_test(
        managers: BTreeMap<u64, Arc<tokio::sync::Mutex<ParticipantKeyManager>>>,
        authorizations: impl IntoIterator<
            Item = (
                SigningSessionContext,
                AuthorizationRequest,
                SigningDerivation,
            ),
        >,
    ) -> Self {
        Self {
            managers: Arc::new(managers),
            aggregate_results: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            authorizer: Arc::new(FixedSigningAuthorizations(
                authorizations
                    .into_iter()
                    .map(|value| (value.0.session_id().to_bytes(), value))
                    .collect(),
            )),
        }
    }

    /// Return bounded nonterminal protocol counts for one public root epoch.
    #[cfg(feature = "federation-bark")]
    pub async fn operator_protocol_status(
        &self,
        root_epoch: u64,
    ) -> anyhow::Result<FederationBarkProtocolStatus> {
        let manager = self.manager(root_epoch)?;
        let manager = manager.lock().await;
        let ordinary_frost_nonterminal =
            super::usize_to_u64_saturating(manager.nonce_store().nonterminal_records()?.len());
        let virtual_musig2_nonterminal = super::usize_to_u64_saturating(
            manager
                .protocol_store()
                .nonterminal_virtual_musig_records()?
                .len(),
        );
        let threshold_dh_nonterminal = super::usize_to_u64_saturating(
            manager
                .protocol_store()
                .nonterminal_threshold_dh_records()?
                .len(),
        );
        drop(manager);
        let aggregate_results_cached =
            super::usize_to_u64_saturating(self.aggregate_results.lock().await.len());
        Ok(FederationBarkProtocolStatus {
            ordinary_frost_nonterminal,
            virtual_musig2_nonterminal,
            threshold_dh_nonterminal,
            aggregate_results_cached,
        })
    }

    pub async fn commit(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> Result<NonceCommitment, cdk::Error> {
        let validated = self
            .validate_signing_authorization(mint, context, authorization)
            .await?;
        if &validated.derivation != derivation {
            return Err(frost_invalid("round-one derivation differs from accepted input").into());
        }
        let manager = self.manager(context.root_epoch().get())?;
        manager
            .lock()
            .await
            .commit_nonce_for_derivation(
                context,
                derivation.clone(),
                &mut cdk_frost::rand_core::OsRng,
            )
            .map_err(frost_boundary_error)
    }

    pub async fn sign(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        nonce_id: cdk_frost::NonceId,
        authorization: &AuthorizationRequest,
        commitments: &cdk_frost::NonceCommitmentSet,
    ) -> Result<SignatureShare, cdk::Error> {
        self.validate_signing_authorization(mint, context, authorization)
            .await?;
        let manager = self.manager(context.root_epoch().get())?;
        manager
            .lock()
            .await
            .sign_share(nonce_id, authorization, commitments)
            .map_err(frost_boundary_error)
    }

    pub async fn record_aggregate(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        package_hash: [u8; 32],
        signature: cdk_frost::AggregateSignature,
    ) -> Result<(), cdk::Error> {
        let validated = self
            .validate_signing_authorization(mint, context, authorization)
            .await?;
        if package_hash == [0; 32] {
            return Err(frost_invalid("aggregate result package hash is empty").into());
        }
        let manager = self.manager(context.root_epoch().get())?;
        let manager = manager.lock().await;
        let public = manager
            .public_package(authorization.application_id())
            .map_err(frost_boundary_error)?;
        ParticipantKeyManager::verify_aggregate_for_derivation(
            public,
            &validated.derivation,
            &context.message_commitment(),
            signature,
        )
        .map_err(frost_boundary_error)?;
        drop(manager);
        let mut results = self.aggregate_results.lock().await;
        if let Some(existing) = results.get(&context.session_id().to_bytes()) {
            if existing != &(package_hash, signature.to_bytes()) {
                return Err(frost_invalid("aggregate result equivocation").into());
            }
            return Ok(());
        }
        while results.len() >= RESULT_CACHE_LIMIT {
            let first = results
                .keys()
                .next()
                .copied()
                .expect("non-empty bounded result cache");
            results.remove(&first);
        }
        results.insert(
            context.session_id().to_bytes(),
            (package_hash, signature.to_bytes()),
        );
        Ok(())
    }

    pub async fn abort(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        reason: cdk_frost::NonceBurnReason,
    ) -> Result<(), cdk::Error> {
        self.validate_signing_authorization(mint, context, authorization)
            .await?;
        self.manager(context.root_epoch().get())?
            .lock()
            .await
            .burn_session(context.session_id(), reason)
            .map_err(frost_boundary_error)
    }

    #[cfg(feature = "federation-bark")]
    pub async fn commit_bark_virtual_musig(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> Result<VirtualMusigNonceCommitment, cdk::Error> {
        let validated = self.validate_bark(mint, context, authorization).await?;
        if context.protocol() != CustodyProtocol::VirtualMusig2
            || &validated.derivation != derivation
        {
            return Err(frost_invalid(
                "Bark virtual MuSig2 derivation differs from accepted state",
            )
            .into());
        }
        self.manager(context.root_epoch().get())?
            .lock()
            .await
            .commit_virtual_musig_nonce(
                context,
                derivation.clone(),
                &mut cdk_frost::rand_core::OsRng,
            )
            .map_err(frost_boundary_error)
    }

    #[cfg(feature = "federation-bark")]
    pub async fn sign_bark_virtual_musig(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        nonce_id: VirtualMusigNonceId,
        authorization: &AuthorizationRequest,
        commitments: &VirtualMusigNonceCommitmentSet,
        transcript: &VirtualMusigTranscript,
    ) -> Result<VirtualMusigPartialShare, cdk::Error> {
        self.validate_bark(mint, context, authorization).await?;
        if context.protocol() != CustodyProtocol::VirtualMusig2 {
            return Err(frost_invalid("Bark virtual partial uses the wrong protocol").into());
        }
        self.manager(context.root_epoch().get())?
            .lock()
            .await
            .virtual_musig_partial(nonce_id, authorization, commitments, transcript)
            .map_err(frost_boundary_error)
    }

    #[cfg(feature = "federation-bark")]
    pub async fn bark_threshold_dh(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
        request: &ThresholdDhRequest,
    ) -> Result<ThresholdDhShare, cdk::Error> {
        let validated = self.validate_bark(mint, context, authorization).await?;
        if context.protocol() != CustodyProtocol::ThresholdDh || &validated.derivation != derivation
        {
            return Err(
                frost_invalid("Bark threshold-DH derivation differs from accepted state").into(),
            );
        }
        self.manager(context.root_epoch().get())?
            .lock()
            .await
            .threshold_dh_share(
                context,
                derivation.clone(),
                authorization,
                request,
                &mut cdk_frost::rand_core::OsRng,
            )
            .map_err(frost_boundary_error)
    }

    #[cfg(feature = "federation-bark")]
    pub async fn abort_bark_custody(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
        reason: cdk_frost::NonceBurnReason,
    ) -> Result<(), cdk::Error> {
        self.validate_bark(mint, context, authorization).await?;
        let manager = self.manager(context.root_epoch().get())?;
        match context.protocol() {
            CustodyProtocol::VirtualMusig2 => manager
                .lock()
                .await
                .burn_virtual_musig_session(context.session_id(), reason),
            CustodyProtocol::ThresholdDh => manager
                .lock()
                .await
                .burn_threshold_dh_session(context.session_id(), reason),
            CustodyProtocol::OrdinaryFrost => {
                return Err(frost_invalid("Bark abort cannot burn ordinary FROST state").into());
            }
        }
        .map_err(frost_boundary_error)
    }

    fn manager(
        &self,
        root_epoch: u64,
    ) -> Result<&Arc<tokio::sync::Mutex<ParticipantKeyManager>>, cdk::Error> {
        self.managers
            .get(&root_epoch)
            .ok_or_else(|| frost_invalid("FROST root epoch is not active").into())
    }

    async fn validate_onchain(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        self.authorizer
            .validate_onchain(mint, context, authorization)
            .await
    }

    async fn validate_signing_authorization(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        #[cfg(feature = "federation-bark")]
        {
            let bark_application = mint.federation_config().and_then(|runtime| {
                cdk::federation::federation_bark_application(runtime.public_config())
                    .ok()
                    .flatten()
            });
            let recovery_application = mint.federation_config().and_then(|runtime| {
                cdk::federation::federation_bark_recovery_application(runtime.public_config())
                    .ok()
                    .flatten()
            });
            match (bark_application, recovery_application) {
                (_, Some(application))
                    if authorization.application_id().to_bytes()
                        == application.application_id().to_bytes() =>
                {
                    self.validate_bark_recovery(mint, context, authorization)
                        .await
                }
                (Some(application), _)
                    if authorization.application_id().to_bytes()
                        == application.application_id.to_bytes() =>
                {
                    self.validate_bark(mint, context, authorization).await
                }
                _ => self.validate_onchain(mint, context, authorization).await,
            }
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            self.validate_onchain(mint, context, authorization).await
        }
    }

    #[cfg(feature = "federation-bark")]
    async fn validate_bark_recovery(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        let typed = FederationBarkRecoveryFrostAuthorization::from_bytes(authorization.payload())?;
        let entry = mint
            .localstore()
            .get_federation_journal_entry(
                FederationJournalEntryKey::Operation(typed.registered_operation_id),
                None,
            )
            .await?
            .ok_or_else(|| frost_invalid("accepted Bark recovery registration is unavailable"))?;
        let projection = mint.federation_bark_projection_snapshot().await?;
        let input_index = usize::try_from(typed.recovery_input_index)
            .map_err(|_| frost_invalid("Bark recovery input index does not fit usize"))?;
        let chain_state = mint.federation_chain_observation_state().await?;
        let accepted = typed.validate_accepted_state(&entry, &projection, &chain_state)?;
        let selected = &accepted.input;
        let package = &accepted.package;
        let action = &accepted.action;
        let transition = &accepted.registered;
        let authorization_head = &accepted.authorization_head;

        let runtime = mint
            .federation_config()
            .ok_or_else(|| frost_invalid("federation configuration is unavailable"))?;
        let public_config = runtime.public_config();
        let wallet = public_config
            .frost_wallet
            .as_ref()
            .ok_or_else(|| frost_invalid("FROST wallet configuration is unavailable"))?;
        let application = cdk::federation::federation_bark_recovery_application(public_config)?
            .ok_or_else(|| frost_invalid("Bark recovery application is unavailable"))?;
        if selected.allocation.application_id != application.application_id()
            || selected.allocation.application_key_epoch != application.application_key_epoch()
        {
            return Err(
                frost_invalid("Bark recovery input uses a different recovery application").into(),
            );
        }
        let public = application.derived_public_package(public_config, &wallet.root)?;
        let adapter = FrostBip32::new(public.clone(), application.network())
            .map_err(|_| frost_invalid("FROST BIP32 adapter rejected the public package"))?;
        let branch = match selected.allocation.keychain {
            cdk::federation::FederationAddressKeychain::External => WalletBranch::External,
            cdk::federation::FederationAddressKeychain::Internal => WalletBranch::Internal,
        };
        let index = WalletIndex::new(selected.allocation.derivation_index)
            .map_err(|_| frost_invalid("wallet derivation index is out of range"))?;
        let derived = adapter
            .derive(branch, index)
            .map_err(|_| frost_invalid("wallet address derivation failed"))?;
        let expected = FederationBarkRecoveryFrostAuthorization::new(
            entry.operation_id,
            action.latest_operation_id,
            transition.action_id,
            package,
            input_index,
            derived.signing_derivation(),
        )?;
        let expected_signers = authorization_head
            .signer_set
            .iter()
            .map(|member| cdk_frost::MemberId::new(member.get()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(frost_boundary_error)?;
        if typed != expected
            || authorization.application_id() != public.registration().id()
            || authorization.application_key_epoch() != public.registration().key_epoch()
            || authorization.derivation_version() != public.registration().derivation_version()
            || authorization.session_id() != context.session_id()
            || context.protocol() != CustodyProtocol::OrdinaryFrost
            || context.federation_id() != public_config.federation_id.to_bytes()
            || context.root_epoch().get() != application.root_key_epoch()
            || context.registration() != public.registration()
            || context.derivation_binding() != derived.signing_derivation().binding()
            || context.authorization_reference() != expected.authorization_reference
            || context.message_commitment() != expected.message
            || context.attempt() != action.attempt
            || context.signer_set() != expected_signers
        {
            return Err(frost_invalid(
                "Bark recovery context differs from the accepted CPFP package",
            )
            .into());
        }
        Ok(ValidatedCdkAuthorization {
            derivation: derived.signing_derivation().clone(),
        })
    }

    #[cfg(feature = "federation-bark")]
    async fn validate_bark(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        self.authorizer
            .validate_bark(mint, context, authorization)
            .await
    }
}

/// The mint's own check that a Bark custody signing request is bound to an
/// accepted action plan and attempt.
#[cfg(feature = "federation-bark")]
async fn validate_bark_authorization(
    mint: &Mint,
    context: &SigningSessionContext,
    authorization: &AuthorizationRequest,
) -> Result<ValidatedCdkAuthorization, cdk::Error> {
    let typed = FederationBarkFrostAuthorization::from_bytes(authorization.payload())?;
    let entry = mint
        .localstore()
        .get_federation_journal_entry(
            FederationJournalEntryKey::Operation(typed.reference.accepted_operation_id),
            None,
        )
        .await?
        .ok_or_else(|| frost_invalid("accepted Bark operation is unavailable"))?;
    let runtime = mint
        .federation_config()
        .ok_or_else(|| frost_invalid("federation configuration is unavailable"))?;
    let public_config = runtime.public_config();
    typed
        .reference
        .validate_accepted_operation(public_config, &entry)?;
    let operation = super::bark_operation(&entry.envelope.operation)?;
    let (plan_entry, authorization_attempt) = match &operation {
        cdk::federation::FederationBarkOperation::ActionPlan(plan) => (entry.clone(), plan.attempt),
        cdk::federation::FederationBarkOperation::ActionTransition(transition)
            if transition.phase == FederationBarkSemanticPhase::SigningAttemptAdvanced =>
        {
            (
                mint.localstore()
                    .get_federation_journal_entry(
                        FederationJournalEntryKey::Operation(transition.plan_operation_id),
                        None,
                    )
                    .await?
                    .ok_or_else(|| frost_invalid("accepted Bark action plan is unavailable"))?,
                transition.attempt,
            )
        }
        _ => return Err(frost_invalid("Bark custody reference has no action plan").into()),
    };
    let cdk::federation::FederationBarkOperation::ActionPlan(plan) =
        super::bark_operation(&plan_entry.envelope.operation)?
    else {
        return Err(frost_invalid("Bark custody plan reference is invalid").into());
    };
    let plan_commitment =
        cdk::federation::FederationBarkOperation::ActionPlan(plan.clone()).commitment()?;
    if let cdk::federation::FederationBarkOperation::ActionTransition(transition) = &operation {
        let signer_set = cdk::federation::federation_bark_signer_set_for_attempt(
            public_config,
            transition.attempt,
        )?;
        let expected = cdk::federation::FederationBarkActionTransition::signing_attempt_advanced(
            plan_entry.operation_id,
            transition.predecessor_operation_id,
            &plan,
            transition.actor,
            transition.attempt,
            signer_set,
        )?;
        if transition != &expected {
            return Err(frost_invalid(
                "Bark custody retry differs from the deterministic accepted attempt",
            )
            .into());
        }
    }
    if !matches!(
        plan.kind,
        cdk::federation::FederationBarkActionKind::IncomingClaim
            | cdk::federation::FederationBarkActionKind::LightningSend
            | cdk::federation::FederationBarkActionKind::FailedSendRevocation
            | cdk::federation::FederationBarkActionKind::Refresh
            | cdk::federation::FederationBarkActionKind::EmergencyExit
    ) || !typed
        .request_transcript
        .starts_with(&plan_commitment.to_bytes())
    {
        return Err(frost_invalid(
            "Bark custody transcript is not bound to the accepted action plan",
        )
        .into());
    }
    let locator_is_input = plan
        .inputs
        .iter()
        .any(|input| input.locator == typed.reference.locator);
    let locator_is_authorized_output = bark_action_authorizes_output_locator(plan.kind)
        && plan
            .outputs
            .iter()
            .any(|output| output.locator == typed.reference.locator);
    if !locator_is_input && !locator_is_authorized_output {
        return Err(
            frost_invalid("Bark custody locator is not authorized by the accepted action").into(),
        );
    }
    let action_authorization = match plan.kind {
        cdk::federation::FederationBarkActionKind::LightningSend => {
            let payment_hash = plan
                .payment_hash
                .ok_or_else(|| frost_invalid("accepted Bark send payment hash is missing"))?;
            let mut authorization = plan_commitment.to_bytes().to_vec();
            authorization.extend_from_slice(b"bark/lightning-send-authorization/v1");
            authorization.extend_from_slice(&payment_hash.to_bytes());
            authorization.extend_from_slice(&u64::from(plan.external_amount).to_be_bytes());
            authorization.extend_from_slice(&u64::from(plan.fee_amount).to_be_bytes());
            for input in &plan.inputs {
                authorization.extend_from_slice(&input.vtxo_id.to_bytes());
            }
            Some(authorization)
        }
        cdk::federation::FederationBarkActionKind::FailedSendRevocation => {
            let payment_hash = plan
                .payment_hash
                .ok_or_else(|| frost_invalid("accepted Bark revocation payment hash is missing"))?;
            let mut authorization = plan_commitment.to_bytes().to_vec();
            authorization.extend_from_slice(b"bark/lightning-revocation-authorization/v1");
            authorization.extend_from_slice(&payment_hash.to_bytes());
            for input in &plan.inputs {
                authorization.extend_from_slice(&input.vtxo_id.to_bytes());
            }
            Some(authorization)
        }
        cdk::federation::FederationBarkActionKind::IncomingClaim
        | cdk::federation::FederationBarkActionKind::Refresh
        | cdk::federation::FederationBarkActionKind::EmergencyExit => None,
    };
    if action_authorization
        .as_ref()
        .is_some_and(|authorization| !typed.request_transcript.starts_with(authorization))
    {
        return Err(frost_invalid(
            "Bark custody request differs from the accepted action authorization",
        )
        .into());
    }
    wait_for_bark_custody_projection(mint, &plan, authorization_attempt).await?;
    let wallet = public_config
        .frost_wallet
        .as_ref()
        .ok_or_else(|| frost_invalid("FROST wallet configuration is unavailable"))?;
    let application = cdk::federation::federation_bark_application(public_config)?
        .ok_or_else(|| frost_invalid("Bark application configuration is unavailable"))?;
    let adapter = cdk_frost_bark::FrostBark::new(
        application.derived_public_package(public_config, &wallet.root)?,
    )
    .map_err(|_| {
        cdk::Error::from(frost_invalid(
            "Bark FROST adapter rejected the derived public package",
        ))
    })?;
    let locator = cdk_frost_bark::BarkKeyLocator::vtxo(typed.reference.locator.index);
    let derived = adapter
        .derive(locator)
        .map_err(|_| cdk::Error::from(frost_invalid("Bark VTXO key derivation failed")))?;
    let derivation = typed.signing_derivation()?;
    let expected_protocol = match typed.reference.protocol {
        FederationBarkCustodyProtocol::Frost => CustodyProtocol::OrdinaryFrost,
        FederationBarkCustodyProtocol::Musig2 => CustodyProtocol::VirtualMusig2,
        FederationBarkCustodyProtocol::ThresholdDh => CustodyProtocol::ThresholdDh,
    };
    let expected_signers = typed
        .reference
        .signer_set
        .iter()
        .map(|member| cdk_frost::MemberId::new(member.get()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(frost_boundary_error)?;
    let reference_commitment = typed.reference.commitment()?.to_bytes();
    if authorization.application_id() != adapter.application().registration().id()
        || authorization.application_key_epoch() != adapter.application().registration().key_epoch()
        || authorization.derivation_version()
            != adapter.application().registration().derivation_version()
        || authorization.session_id() != context.session_id()
        || context.protocol() != expected_protocol
        || context.federation_id() != public_config.federation_id.to_bytes()
        || context.root_epoch().get() != typed.reference.root_key_epoch
        || context.registration() != adapter.application().registration()
        || context.derivation_binding() != derivation.binding()
        || context.authorization_reference() != reference_commitment
        || context.message_commitment() != typed.message
        || context.attempt() != typed.reference.attempt
        || context.signer_set() != expected_signers
        || derivation != *derived.signing_derivation()
    {
        return Err(
            frost_invalid("Bark custody context differs from the accepted authorization").into(),
        );
    }
    Ok(ValidatedCdkAuthorization { derivation })
}

#[cfg(feature = "federation-bark")]
async fn wait_for_bark_custody_projection(
    mint: &Mint,
    plan: &cdk::federation::FederationBarkActionPlan,
    authorization_attempt: u32,
) -> Result<(), cdk::Error> {
    let deadline = tokio::time::Instant::now() + BARK_CUSTODY_PROJECTION_WAIT_TIMEOUT;
    let input_ids = plan
        .inputs
        .iter()
        .map(|input| input.vtxo_id)
        .collect::<Vec<_>>();
    loop {
        let authorization_is_current = match mint.federation_bark_action(plan.action_id).await? {
            Some(action)
                if cdk::federation::federation_bark_custody_attempt_is_current(
                    plan.kind,
                    action.phase,
                    action.signing_retry_phase,
                    action.attempt,
                    authorization_attempt,
                ) =>
            {
                // The inputs are read apart from the action head; an unchanged
                // head afterwards means no Bark operation on this action moved
                // them in between, as one snapshot read guaranteed.
                let inputs = mint.federation_bark_store()?.bark_vtxos(&input_ids).await?;
                let head_unchanged = mint
                    .federation_bark_action(plan.action_id)
                    .await?
                    .is_some_and(|after| after.latest_operation_id == action.latest_operation_id);
                head_unchanged
                    && bark_custody_inputs_are_authorized(plan, action.effective_phase(), &inputs)
            }
            _ => false,
        };
        if authorization_is_current {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(
                frost_invalid("Bark custody request differs from a reserved action input").into(),
            );
        }
        tokio::time::sleep(BARK_CUSTODY_PROJECTION_POLL_INTERVAL).await;
    }
}

#[cfg(feature = "federation-bark")]
fn bark_custody_inputs_are_authorized(
    plan: &cdk::federation::FederationBarkActionPlan,
    phase: FederationBarkSemanticPhase,
    vtxos: &[cdk::federation::FederationBarkVtxoRecord],
) -> bool {
    plan.inputs.iter().all(|input| {
        vtxos.iter().any(|record| {
            record.vtxo_id == input.vtxo_id
                && record.locator == input.locator
                && record.amount == input.amount
                && record.reservation_action_id == Some(plan.action_id)
                && match record.state {
                    cdk::federation::FederationBarkVtxoState::Reserved => true,
                    // Disclosure retains the exact claim reservation while
                    // recovery finishes spending the already authorized input.
                    cdk::federation::FederationBarkVtxoState::PendingSpend => {
                        plan.kind == FederationBarkActionKind::IncomingClaim
                            && phase == FederationBarkSemanticPhase::ReceivePreimageDisclosed
                    }
                    cdk::federation::FederationBarkVtxoState::Exiting => {
                        plan.kind == FederationBarkActionKind::EmergencyExit
                    }
                    _ => false,
                }
        })
    })
}

struct ValidatedCdkAuthorization {
    derivation: SigningDerivation,
}

/// Decides whether a signing request is authorized by accepted federation
/// state. Production uses [`MintSigningAuthorizer`]; tests supply fixed
/// authorizations instead of a test-only field on the runtime.
#[async_trait::async_trait]
trait SigningAuthorizer: Send + Sync {
    async fn validate_onchain(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        validate_onchain_authorization(mint, context, authorization).await
    }

    #[cfg(feature = "federation-bark")]
    async fn validate_bark(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        validate_bark_authorization(mint, context, authorization).await
    }
}

/// The mint's own authorization checks.
struct MintSigningAuthorizer;

impl SigningAuthorizer for MintSigningAuthorizer {}

/// The mint's own check that an on-chain signing request is bound to an
/// accepted transaction.
async fn validate_onchain_authorization(
    mint: &Mint,
    context: &SigningSessionContext,
    authorization: &AuthorizationRequest,
) -> Result<ValidatedCdkAuthorization, cdk::Error> {
    #[cfg(feature = "federation-bdk")]
    {
        validate_cdk_authorization(mint, context, authorization).await
    }
    #[cfg(not(feature = "federation-bdk"))]
    {
        let _ = (mint, context, authorization);
        Err(frost_invalid("CDK on-chain FROST application is disabled").into())
    }
}

/// Fixed authorizations keyed by signing session, accepted without consulting
/// the mint; any other session falls through to the mint's own checks.
#[cfg(any(test, feature = "test-utils"))]
struct FixedSigningAuthorizations(
    BTreeMap<
        [u8; 32],
        (
            SigningSessionContext,
            AuthorizationRequest,
            SigningDerivation,
        ),
    >,
);

#[cfg(any(test, feature = "test-utils"))]
impl FixedSigningAuthorizations {
    fn fixed(
        &self,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<Option<ValidatedCdkAuthorization>, cdk::Error> {
        match self.0.get(&context.session_id().to_bytes()) {
            Some((accepted_context, accepted_authorization, derivation)) => {
                if accepted_context != context || accepted_authorization != authorization {
                    return Err(frost_invalid("test authorization binding mismatch").into());
                }
                Ok(Some(ValidatedCdkAuthorization {
                    derivation: derivation.clone(),
                }))
            }
            None => Ok(None),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
#[async_trait::async_trait]
impl SigningAuthorizer for FixedSigningAuthorizations {
    async fn validate_onchain(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        match self.fixed(context, authorization)? {
            Some(validated) => Ok(validated),
            None => validate_onchain_authorization(mint, context, authorization).await,
        }
    }

    #[cfg(feature = "federation-bark")]
    async fn validate_bark(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        match self.fixed(context, authorization)? {
            Some(validated) => Ok(validated),
            None => validate_bark_authorization(mint, context, authorization).await,
        }
    }
}

#[cfg(feature = "federation-bdk")]
async fn validate_cdk_authorization(
    mint: &Mint,
    context: &SigningSessionContext,
    request: &AuthorizationRequest,
) -> Result<ValidatedCdkAuthorization, cdk::Error> {
    let typed = FederationCdkOnchainFrostAuthorization::from_bytes(request.payload())?;
    let accepted = mint
        .federation_transaction_signing_authorization(typed.proposal_id)
        .await?;
    let input_index = usize::try_from(typed.input_index)
        .map_err(|_| frost_invalid("FROST input index does not fit usize"))?;
    let selected = accepted
        .proposal
        .selected_inputs
        .get(input_index)
        .ok_or_else(|| frost_invalid("FROST input index is out of bounds"))?;
    let runtime = mint
        .federation_config()
        .ok_or_else(|| frost_invalid("federation configuration is unavailable"))?;
    let wallet = runtime
        .public_config()
        .frost_wallet
        .as_ref()
        .ok_or_else(|| frost_invalid("FROST wallet configuration is unavailable"))?;
    let application = wallet
        .cdk_onchain_application
        .as_ref()
        .ok_or_else(|| frost_invalid("CDK on-chain application is unavailable"))?;
    if application.application_id != accepted.proposal.scope.application_id
        || application.application_key_epoch != accepted.proposal.scope.application_key_epoch
        || application.root_key_epoch != accepted.proposal.scope.root_key_epoch
    {
        return Err(frost_invalid(
            "accepted transaction application does not match the configured wallet",
        )
        .into());
    }
    let root = wallet
        .root(application.root_key_epoch)
        .ok_or_else(|| frost_invalid("accepted transaction root epoch is unavailable"))?;
    let public = application.derived_public_package(runtime.public_config(), root)?;
    let now_epoch_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    if !cdk::federation::federation_wallet_signing_attempt_is_current(
        context.attempt(),
        now_epoch_millis,
    ) {
        return Err(
            frost_invalid("FROST signing attempt is outside the active lease window").into(),
        );
    }
    let plan = cdk::federation::FederationWalletLivenessSchedule::new(
        typed.proposal_id.to_bytes(),
        runtime.public_config().members.keys().copied(),
        application.threshold,
    )?
    .signing_plan(context.attempt())?;
    let expected_signers = plan
        .signer_set
        .iter()
        .map(|member| member.get())
        .collect::<Vec<_>>();
    let context_signers = context
        .signer_set()
        .iter()
        .map(|member| member.get())
        .collect::<Vec<_>>();
    if context_signers != expected_signers {
        return Err(frost_invalid(
            "FROST signer set differs from the deterministic active attempt",
        )
        .into());
    }
    let branch = match selected.allocation.keychain {
        cdk::federation::FederationAddressKeychain::External => WalletBranch::External,
        cdk::federation::FederationAddressKeychain::Internal => WalletBranch::Internal,
    };
    let adapter = FrostBip32::new(public.clone(), application.network)
        .map_err(|_| frost_invalid("FROST BIP32 adapter rejected the public package"))?;
    let index = WalletIndex::new(selected.allocation.derivation_index)
        .map_err(|_| frost_invalid("wallet derivation index is out of range"))?;
    let derived = adapter
        .derive(branch, index)
        .map_err(|_| frost_invalid("wallet address derivation failed"))?;
    let expected = FederationCdkOnchainFrostAuthorization::new(
        &accepted,
        input_index,
        derived.signing_derivation(),
    )?;
    if typed != expected
        || context.federation_id() != runtime.federation_id().to_bytes()
        || context.root_epoch().get() != application.root_key_epoch
        || context.registration() != public.registration()
        || context.authorization_reference() != expected.authorization_reference
        || context.message_commitment() != expected.message
        || context.derivation_binding() != derived.signing_derivation().binding()
        || request.session_id() != context.session_id()
    {
        return Err(frost_invalid(
            "FROST request differs from locally revalidated accepted transaction",
        )
        .into());
    }
    Ok(ValidatedCdkAuthorization {
        derivation: derived.signing_derivation().clone(),
    })
}

fn nonce_encryption_key(
    runtime: &FederationRuntimeConfig,
    root_epoch: u64,
) -> Result<SecretEncryptionKey, cdk::Error> {
    let mut hasher = Sha256::new();
    hasher.update(b"cdk/federation/frost-nonce-storage/v1");
    hasher.update(
        runtime
            .private_config()
            .identity_secret_key
            .to_secret_bytes(),
    );
    hasher.update(runtime.federation_id().to_bytes());
    hasher.update(root_epoch.to_be_bytes());
    hasher.update(runtime.member_id().get().to_be_bytes());
    SecretEncryptionKey::new(hasher.finalize().into()).map_err(frost_boundary_error)
}

fn frost_boundary_error(error: cdk_frost::Error) -> cdk::Error {
    cdk::federation::FederationError::FrostSigning(error).into()
}

fn frost_invalid(reason: &'static str) -> cdk::federation::FederationError {
    cdk::federation::FederationError::FrostSigningInvalid { reason }
}

pub fn signing_runtime(state: &FederationNode) -> Option<&FederationFrostSigningRuntime> {
    state.federation.frost_signing()
}
