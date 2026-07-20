//! Application-aware FROST signing runtime and private REST handlers.

use std::collections::BTreeMap;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Json, State};
use axum::response::{IntoResponse, Response};
#[cfg(feature = "federation-bark")]
use cdk::federation::{
    FederationBarkCustodyProtocol, FederationBarkFrostAuthorization, FederationBarkProtocolStatus,
};
use cdk::federation::{
    FederationBarkFrostRegistrationAdapter, FederationCdkOnchainFrostAuthorization,
    FederationCdkOnchainFrostAuthorizationAdapter, FederationFrostAggregateResultNotification,
    FederationFrostCommitmentRequest, FederationFrostCommitmentResponse,
    FederationFrostInputSignatureAudit, FederationFrostSignatureShareResponse,
    FederationFrostSigningAbortRequest, FederationFrostSigningAck,
    FederationFrostSigningPackageRequest, FederationRestMessageKind, FederationSignedRequest,
    FederationSignedTransaction, FederationTransactionProposalId,
};
use cdk::mint::{FederationRuntimeConfig, Mint};
use cdk_frost::{
    AuthorizationRequest, DerivedPublicPackage, NonceCommitment, ParticipantKeyManager,
    RedbNonceStore, SecretEncryptionKey, SignatureShare, SigningDerivation, SigningSessionContext,
};
#[cfg(feature = "federation-bark")]
use cdk_frost::{
    CustodyProtocol, ThresholdDhRequest, ThresholdDhShare, VirtualMusigNonceCommitment,
    VirtualMusigNonceCommitmentSet, VirtualMusigNonceId, VirtualMusigPartialShare,
    VirtualMusigTranscript,
};
use cdk_frost_bip32::{FrostBip32, WalletBranch, WalletIndex};
use sha2::{Digest, Sha256};
use tracing::instrument;

use crate::MintState;

use super::{admit_private_request, federation_error_response, federation_runtime_config};

const RESULT_CACHE_LIMIT: usize = 1_024;
#[cfg(feature = "federation-bark")]
const BARK_CUSTODY_PROJECTION_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
#[cfg(feature = "federation-bark")]
const BARK_CUSTODY_PROJECTION_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(20);

#[cfg(feature = "federation-bark")]
fn usize_to_u64_saturating(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

type AggregateResultCache = BTreeMap<[u8; 32], ([u8; 32], [u8; 64])>;

#[allow(dead_code)]
struct CoordinatedFrostSignature {
    signature: cdk_frost::AggregateSignature,
    package_hash: [u8; 32],
}

/// Application-neutral coordinator for one exact selected signer set.
#[derive(Debug)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct FederationFrostSigningCoordinator<'a, T> {
    client: &'a super::peer_client::FederationRestPeerClient<T>,
    public: DerivedPublicPackage,
}

#[cfg_attr(not(test), allow(dead_code))]
impl<'a, T> FederationFrostSigningCoordinator<'a, T>
where
    T: cdk_http_client::Transport,
{
    /// Construct a coordinator without participant secret material.
    pub(crate) fn new(
        client: &'a super::peer_client::FederationRestPeerClient<T>,
        public: DerivedPublicPackage,
    ) -> Self {
        Self { client, public }
    }

    /// Run both rounds, verify every response, aggregate, and notify signers.
    pub(crate) async fn sign(
        &self,
        public_config: &cdk::federation::FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<cdk_frost::AggregateSignature> {
        Ok(self
            .sign_with_audit(public_config, context, derivation, authorization)
            .await?
            .signature)
    }

    async fn sign_with_audit(
        &self,
        public_config: &cdk::federation::FederationConfig,
        context: &SigningSessionContext,
        derivation: &SigningDerivation,
        authorization: &AuthorizationRequest,
    ) -> anyhow::Result<CoordinatedFrostSignature> {
        if context.signer_set().len() < usize::from(self.public.threshold()) {
            anyhow::bail!("FROST signer set is below threshold");
        }
        let commitment_request =
            FederationFrostCommitmentRequest::new(context, derivation, authorization);
        let mut commitments = Vec::with_capacity(context.signer_set().len());
        for signer in context.signer_set() {
            let member_id = cdk::federation::MemberId::new(signer.get())?;
            let member = public_config
                .member(member_id)
                .ok_or(cdk::federation::FederationError::UnknownMember { member_id })?;
            match self
                .client
                .request_frost_commitment(member, &commitment_request)
                .await
            {
                Ok(commitment) => commitments.push(commitment),
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
            }
        }
        let commitments = cdk_frost::NonceCommitmentSet::new(commitments)?;
        let mut shares = Vec::with_capacity(context.signer_set().len());
        for signer in context.signer_set() {
            let member_id = cdk::federation::MemberId::new(signer.get())?;
            let member = public_config
                .member(member_id)
                .ok_or(cdk::federation::FederationError::UnknownMember { member_id })?;
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
            match self
                .client
                .request_frost_signature_share(member, &package)
                .await
            {
                Ok(share) => shares.push(share),
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
            }
        }
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
        for signer in context.signer_set() {
            let member_id = cdk::federation::MemberId::new(signer.get())?;
            let member = public_config
                .member(member_id)
                .ok_or(cdk::federation::FederationError::UnknownMember { member_id })?;
            self.client
                .notify_frost_aggregate_result(member, &notification)
                .await?;
        }
        Ok(CoordinatedFrostSignature {
            signature,
            package_hash,
        })
    }

    /// Sign every input, finalize through BDK, then order the signed record in consensus.
    #[allow(dead_code)]
    pub(crate) async fn sign_accepted_transaction(
        &self,
        state: &MintState,
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
                    .map(|input| (input.outpoint, input.previous_output.clone()))
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
        for signer in context.signer_set() {
            let Ok(member_id) = cdk::federation::MemberId::new(signer.get()) else {
                continue;
            };
            let Some(member) = public_config.member(member_id) else {
                continue;
            };
            let _ = self.client.abort_frost_signing(member, &request).await;
        }
    }
}

/// One participant's long-lived, application-neutral FROST key boundary.
#[derive(Clone)]
pub(crate) struct FederationFrostSigningRuntime {
    managers: Arc<BTreeMap<u64, Arc<tokio::sync::Mutex<ParticipantKeyManager>>>>,
    aggregate_results: Arc<tokio::sync::Mutex<AggregateResultCache>>,
    #[cfg(test)]
    test_authorizations: Arc<
        BTreeMap<
            [u8; 32],
            (
                SigningSessionContext,
                AuthorizationRequest,
                SigningDerivation,
            ),
        >,
    >,
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
    pub(crate) async fn open(mint: &Mint, storage_root: &Path) -> anyhow::Result<Option<Self>> {
        let Some(runtime) = mint.federation_config() else {
            return Ok(None);
        };
        let Some(wallet) = runtime.public_config().frost_wallet.as_ref() else {
            return Ok(None);
        };
        let application = &wallet.cdk_onchain_application;

        let private_roots = mint
            .localstore()
            .get_federation_frost_root_private_configs()
            .await?;
        let nonce_root = storage_root.join("frost-nonces");
        std::fs::create_dir_all(&nonce_root)?;
        #[cfg(unix)]
        std::fs::set_permissions(&nonce_root, std::fs::Permissions::from_mode(0o700))?;
        let mut managers = BTreeMap::new();
        let public_root = wallet
            .root(application.root_key_epoch)
            .ok_or_else(|| anyhow::anyhow!("FROST application root is missing"))?;
        let private_root = private_roots
            .iter()
            .find(|root| root.root_key_epoch == application.root_key_epoch)
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
        let mut manager = ParticipantKeyManager::with_nonce_store(root, nonce_key, store)?;
        manager.register_application(FederationCdkOnchainFrostAuthorizationAdapter::new(
            application,
        )?)?;
        if let Some(bark) = &wallet.bark_application {
            let derived =
                manager.register_application(FederationBarkFrostRegistrationAdapter::new(bark)?)?;
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
        managers.insert(
            public_root.root_key_epoch,
            Arc::new(tokio::sync::Mutex::new(manager)),
        );
        Ok(Some(Self {
            managers: Arc::new(managers),
            aggregate_results: Arc::new(tokio::sync::Mutex::new(BTreeMap::new())),
            #[cfg(test)]
            test_authorizations: Arc::new(BTreeMap::new()),
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test(
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
            test_authorizations: Arc::new(
                authorizations
                    .into_iter()
                    .map(|value| (value.0.session_id().to_bytes(), value))
                    .collect(),
            ),
        }
    }

    /// Return bounded nonterminal protocol counts for one public root epoch.
    #[cfg(feature = "federation-bark")]
    pub(crate) async fn operator_protocol_status(
        &self,
        root_epoch: u64,
    ) -> anyhow::Result<FederationBarkProtocolStatus> {
        let manager = self.manager(root_epoch)?;
        let manager = manager.lock().await;
        let ordinary_frost_nonterminal =
            usize_to_u64_saturating(manager.nonce_store().nonterminal_records()?.len());
        let virtual_musig2_nonterminal = usize_to_u64_saturating(
            manager
                .protocol_store()
                .nonterminal_virtual_musig_records()?
                .len(),
        );
        let threshold_dh_nonterminal = usize_to_u64_saturating(
            manager
                .protocol_store()
                .nonterminal_threshold_dh_records()?
                .len(),
        );
        drop(manager);
        let aggregate_results_cached =
            usize_to_u64_saturating(self.aggregate_results.lock().await.len());
        Ok(FederationBarkProtocolStatus {
            ordinary_frost_nonterminal,
            virtual_musig2_nonterminal,
            threshold_dh_nonterminal,
            aggregate_results_cached,
        })
    }

    async fn commit(
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

    async fn sign(
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

    async fn record_aggregate(
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

    async fn abort(
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
    pub(super) async fn commit_bark_virtual_musig(
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
    pub(super) async fn sign_bark_virtual_musig(
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
    pub(super) async fn bark_threshold_dh(
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
    pub(super) async fn abort_bark_custody(
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
        #[cfg(test)]
        if let Some((accepted_context, accepted_authorization, derivation)) = self
            .test_authorizations
            .get(&context.session_id().to_bytes())
        {
            if accepted_context != context || accepted_authorization != authorization {
                return Err(frost_invalid("test authorization binding mismatch").into());
            }
            return Ok(ValidatedCdkAuthorization {
                derivation: derivation.clone(),
            });
        }
        validate_cdk_authorization(mint, context, authorization).await
    }

    async fn validate_signing_authorization(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        let bark_application = mint
            .federation_config()
            .and_then(|runtime| runtime.public_config().frost_wallet.as_ref())
            .and_then(|wallet| wallet.bark_application.as_ref());
        match bark_application {
            Some(application)
                if authorization.application_id().to_bytes()
                    == application.application_id.to_bytes() =>
            {
                self.validate_bark(mint, context, authorization).await
            }
            Some(_) | None => self.validate_onchain(mint, context, authorization).await,
        }
    }

    async fn validate_bark(
        &self,
        mint: &Mint,
        context: &SigningSessionContext,
        authorization: &AuthorizationRequest,
    ) -> Result<ValidatedCdkAuthorization, cdk::Error> {
        #[cfg(test)]
        if let Some((accepted_context, accepted_authorization, derivation)) = self
            .test_authorizations
            .get(&context.session_id().to_bytes())
        {
            if accepted_context != context || accepted_authorization != authorization {
                return Err(frost_invalid("test Bark authorization binding mismatch").into());
            }
            return Ok(ValidatedCdkAuthorization {
                derivation: derivation.clone(),
            });
        }
        #[cfg(not(feature = "federation-bark"))]
        {
            let _ = (mint, context, authorization);
            return Err(frost_invalid("Bark custody support is not enabled").into());
        }
        #[cfg(feature = "federation-bark")]
        {
            let typed = FederationBarkFrostAuthorization::from_bytes(authorization.payload())?;
            let entry = mint
                .localstore()
                .get_federation_journal_entry_by_operation_id(typed.reference.accepted_operation_id)
                .await?
                .ok_or_else(|| frost_invalid("accepted Bark operation is unavailable"))?;
            let runtime = mint
                .federation_config()
                .ok_or_else(|| frost_invalid("federation configuration is unavailable"))?;
            let public_config = runtime.public_config();
            typed
                .reference
                .validate_accepted_operation(public_config, &entry)?;
            let cdk::federation::FederationOperation::Bark(
                cdk::federation::FederationBarkOperation::ActionPlan(plan),
            ) = &entry.envelope.operation
            else {
                return Err(frost_invalid("Bark custody reference is not an action plan").into());
            };
            if !matches!(
                plan.kind,
                cdk::federation::FederationBarkActionKind::IncomingClaim
                    | cdk::federation::FederationBarkActionKind::LightningSend
                    | cdk::federation::FederationBarkActionKind::FailedSendRevocation
                    | cdk::federation::FederationBarkActionKind::Refresh
                    | cdk::federation::FederationBarkActionKind::EmergencyExit
            ) || !typed
                .request_transcript
                .starts_with(&typed.reference.plan_or_result_hash.to_bytes())
            {
                return Err(frost_invalid(
                    "Bark custody transcript is not bound to the accepted action plan",
                )
                .into());
            }
            if !plan
                .inputs
                .iter()
                .any(|input| input.locator == typed.reference.locator)
            {
                return Err(frost_invalid(
                    "Bark custody locator is not an input of the accepted action",
                )
                .into());
            }
            let action_authorization = match plan.kind {
                cdk::federation::FederationBarkActionKind::LightningSend => {
                    let payment_hash = plan.payment_hash.ok_or_else(|| {
                        frost_invalid("accepted Bark send payment hash is missing")
                    })?;
                    let mut authorization = typed.reference.plan_or_result_hash.to_bytes().to_vec();
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
                    let payment_hash = plan.payment_hash.ok_or_else(|| {
                        frost_invalid("accepted Bark revocation payment hash is missing")
                    })?;
                    let mut authorization = typed.reference.plan_or_result_hash.to_bytes().to_vec();
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
            wait_for_bark_custody_projection(mint, plan).await?;
            let wallet = public_config
                .frost_wallet
                .as_ref()
                .ok_or_else(|| frost_invalid("FROST wallet configuration is unavailable"))?;
            let application = wallet
                .bark_application
                .as_ref()
                .ok_or_else(|| frost_invalid("Bark application configuration is unavailable"))?;
            let adapter = cdk_frost_bark::FrostBark::new(
                application.derived_public_package(public_config, &wallet.root)?,
            )
            .map_err(|error| cdk::Error::from(frost_invalid(error)))?;
            let locator = cdk_frost_bark::BarkKeyLocator::vtxo(typed.reference.locator.index);
            let derived = adapter
                .derive(locator)
                .map_err(|error| cdk::Error::from(frost_invalid(error)))?;
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
                || authorization.application_key_epoch()
                    != adapter.application().registration().key_epoch()
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
                return Err(frost_invalid(
                    "Bark custody context differs from the accepted authorization",
                )
                .into());
            }
            Ok(ValidatedCdkAuthorization { derivation })
        }
    }
}

#[cfg(feature = "federation-bark")]
async fn wait_for_bark_custody_projection(
    mint: &Mint,
    plan: &cdk::federation::FederationBarkActionPlan,
) -> Result<(), cdk::Error> {
    let deadline = tokio::time::Instant::now() + BARK_CUSTODY_PROJECTION_WAIT_TIMEOUT;
    loop {
        let projection = mint
            .localstore()
            .get_federation_bark_projection_snapshot()
            .await?;
        if bark_custody_inputs_are_reserved(plan, &projection) {
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
fn bark_custody_inputs_are_reserved(
    plan: &cdk::federation::FederationBarkActionPlan,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
) -> bool {
    plan.inputs.iter().all(|input| {
        projection.vtxos.iter().any(|record| {
            record.vtxo_id == input.vtxo_id
                && record.locator == input.locator
                && record.amount == input.amount
                && matches!(
                    record.state,
                    cdk::federation::FederationBarkVtxoState::Reserved
                        | cdk::federation::FederationBarkVtxoState::Exiting
                )
                && (record.state != cdk::federation::FederationBarkVtxoState::Exiting
                    || plan.kind == cdk::federation::FederationBarkActionKind::EmergencyExit)
                && record.reservation_action_id == Some(plan.action_id)
        })
    })
}

struct ValidatedCdkAuthorization {
    derivation: SigningDerivation,
}

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
    let application = &wallet.cdk_onchain_application;
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
        .map_err(|error| frost_invalid(error.to_string()))?;
    let index = WalletIndex::new(selected.allocation.derivation_index)
        .map_err(|error| frost_invalid(error.to_string()))?;
    let derived = adapter
        .derive(branch, index)
        .map_err(|error| frost_invalid(error.to_string()))?;
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
    frost_invalid(error.to_string()).into()
}

fn frost_invalid(reason: impl ToString) -> cdk::federation::FederationError {
    cdk::federation::FederationError::FrostSigningInvalid {
        reason: reason.to_string(),
    }
}

pub(super) fn signing_runtime(state: &MintState) -> Option<&FederationFrostSigningRuntime> {
    state.federation.frost_signing()
}

pub(super) fn signing_unavailable_response() -> Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "federation FROST signing is unavailable",
    )
        .into_response()
}

pub(super) fn signing_error_response(error: cdk::Error) -> Response {
    match error {
        cdk::Error::Federation(error) => federation_error_response(error),
        error => {
            tracing::error!(
                "Federation FROST signing failed: {}",
                cdk::federation::redact_federation_operator_message(&error.to_string())
            );
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "federation FROST signing failed closed",
            )
                .into_response()
        }
    }
}

#[instrument(skip_all)]
pub(super) async fn post_frost_signing_commitment(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostCommitmentRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostCommitmentResponse>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningCommitment;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, derivation, authorization) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let commitment = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .commit(&state.mint, &context, &derivation, &authorization)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostCommitmentResponse::new(&commitment),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_frost_signing_package(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostSigningPackageRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSignatureShareResponse>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningPackage;
    let verified = admit_private_request(&state, &payload, kind).await?;
    state
        .ensure_federation_ready_for_authoritative_read()
        .await
        .map_err(signing_error_response)?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, nonce_id, authorization, commitments) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    let share = signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .sign(
            &state.mint,
            &context,
            nonce_id,
            &authorization,
            &commitments,
        )
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostSignatureShareResponse::new(payload.payload.package_hash, &share),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_frost_signing_abort(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostSigningAbortRequest>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSigningAck>>, Response> {
    let kind = FederationRestMessageKind::FrostSigningAbort;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, authorization, reason) = payload
        .payload
        .decode(runtime_config.federation_id(), runtime_config.member_id())
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .abort(&state.mint, &context, &authorization, reason)
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostSigningAck::new(context.session_id(), kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}

#[instrument(skip_all)]
pub(super) async fn post_frost_aggregate_result(
    State(state): State<MintState>,
    Json(payload): Json<FederationSignedRequest<FederationFrostAggregateResultNotification>>,
) -> Result<Json<FederationSignedRequest<FederationFrostSigningAck>>, Response> {
    let kind = FederationRestMessageKind::FrostAggregateResult;
    let verified = admit_private_request(&state, &payload, kind).await?;
    let runtime_config = federation_runtime_config(&state)?;
    let (context, authorization, package_hash, signature) = payload
        .payload
        .decode()
        .map_err(federation_error_response)?;
    signing_runtime(&state)
        .ok_or_else(signing_unavailable_response)?
        .record_aggregate(
            &state.mint,
            &context,
            &authorization,
            package_hash,
            signature,
        )
        .await
        .map_err(signing_error_response)?;
    state
        .record_federation_sequence(kind, verified.sender(), verified.sequence())
        .await;
    let response = FederationSignedRequest::new_signed(
        runtime_config.federation_id(),
        verified.sender(),
        verified.sequence(),
        kind,
        FederationFrostSigningAck::new(context.session_id(), kind),
        runtime_config.private_config(),
    )
    .map_err(federation_error_response)?;
    Ok(Json(response))
}
