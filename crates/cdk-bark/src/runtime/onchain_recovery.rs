//! Onchain recovery operations.

use super::{accepted_maintenance_plan, accepted_send_transition, FederatedBarkRuntime};
use crate::Error;
use bitcoin::consensus::deserialize as deserialize_transaction;
use bitcoin::{FeeRate, ScriptBuf, Transaction, Weight, Witness};
use cdk_common::federation::{
    FederationAllocatedAddressRef, FederationChainObservationState, FederationJournalEntry,
    FederationOperationStatus, FederationWalletUtxoState,
};
use cdk_federation_bark::{
    federation_bark_cpfp_allocation_quote, federation_bark_recovery_application,
    federation_bark_signer_set_for_attempt, FederationBarkActionKind, FederationBarkActionRecord,
    FederationBarkArtifactKind, FederationBarkCpfpPackage, FederationBarkCpfpPackageSet,
    FederationBarkCpfpPublication, FederationBarkOperation,
    FederationBarkRecoveryFrostAuthorization, FederationBarkRecoveryInput,
    FederationBarkSemanticPhase, FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU,
};
use cdk_frost::{
    AuthorizationRequest, CustodyProtocol, RootKeyEpoch, SigningAuthorizationScope,
    SigningSessionContext,
};
use cdk_frost_bip32::{FrostBip32, WalletBranch, WalletIndex};
use std::collections::BTreeMap;

impl FederatedBarkRuntime {
    /// Independent consensus address-allocation scope for Bark recovery funds.
    pub fn recovery_address_scope(
        &self,
    ) -> Result<cdk_common::federation::FederationAddressAllocationScope, Error> {
        let recovery = federation_bark_recovery_application(&self.federation)
            .map_err(|_| Error::InvalidPublicConfig)?
            .ok_or(Error::MissingApplication)?;
        Ok(cdk_common::federation::FederationAddressAllocationScope {
            application_id: recovery.application_id(),
            application_key_epoch: recovery.application_key_epoch(),
        })
    }

    /// Derive one accepted Bark-recovery script without advancing local state.
    pub fn derive_recovery_script(
        &self,
        internal: bool,
        derivation_index: u32,
    ) -> Result<ScriptBuf, Error> {
        let frost_wallet = self
            .federation
            .frost_wallet
            .as_ref()
            .ok_or(Error::InvalidPublicConfig)?;
        let recovery = federation_bark_recovery_application(&self.federation)
            .map_err(|_| Error::InvalidPublicConfig)?
            .ok_or(Error::MissingApplication)?;
        recovery
            .derive_script(
                &self.federation,
                &frost_wallet.root,
                internal,
                derivation_index,
            )
            .map_err(|_| Error::InvalidPublicConfig)
    }

    /// Accepted confirmation depth for Bark recovery outputs.
    pub fn recovery_minimum_confirmations(&self) -> Result<u32, Error> {
        federation_bark_recovery_application(&self.federation)
            .map_err(|_| Error::InvalidPublicConfig)?
            .ok_or(Error::MissingApplication)
            .map(|application| application.minimum_confirmations())
    }

    /// Prepare exact recovery-wallet CPFP packages for an accepted exit plan.
    ///
    /// Candidate inputs come only from the accepted recovery application and
    /// are consumed in canonical outpoint order. Change scripts must already
    /// have been allocated by consensus, so this method performs no address
    /// allocation and has no hidden key or coin-selection authority.
    pub async fn prepare_registered_cpfp_packages(
        &self,
        action: &FederationBarkActionRecord,
        accepted_plan: &FederationJournalEntry,
        chain_state: &FederationChainObservationState,
        change_allocations: &[FederationAllocatedAddressRef],
    ) -> Result<FederationBarkCpfpPackageSet, Error> {
        let plan =
            accepted_maintenance_plan(accepted_plan, FederationBarkActionKind::EmergencyExit)?;
        if action.action_id != plan.action_id
            || action.actor != self.member_id
            || action.plan_operation_id != Some(accepted_plan.operation_id)
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let frost_wallet = self
            .federation
            .frost_wallet
            .as_ref()
            .ok_or(Error::MaintenanceExecutionFailed)?;
        let recovery_application = federation_bark_recovery_application(&self.federation)
            .map_err(|_| Error::MaintenanceExecutionFailed)?
            .ok_or(Error::MaintenanceExecutionFailed)?;
        let recovery_scope = cdk_common::federation::FederationAddressAllocationScope {
            application_id: recovery_application.application_id(),
            application_key_epoch: recovery_application.application_key_epoch(),
        };
        let mut parents = BTreeMap::new();
        for artifact in plan
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind == FederationBarkArtifactKind::Transaction)
        {
            let transaction: Transaction = deserialize_transaction(&artifact.bytes)
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
            if transaction
                .output
                .iter()
                .filter(|output| output.script_pubkey == ScriptBuf::new_p2a())
                .count()
                == 1
            {
                parents.insert(transaction.compute_txid(), transaction);
            }
        }
        if parents.is_empty() {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let changes = change_allocations
            .iter()
            .map(|allocation| (allocation.quote.as_str(), allocation))
            .collect::<BTreeMap<_, _>>();
        if changes.len() != parents.len() {
            return Err(Error::MaintenanceExecutionFailed);
        }
        for parent_txid in parents.keys() {
            let quote = federation_bark_cpfp_allocation_quote(plan.action_id, *parent_txid);
            let allocation = changes
                .get(quote.as_str())
                .ok_or(Error::MaintenanceExecutionFailed)?;
            if allocation.scope() != recovery_scope
                || allocation.keychain
                    != cdk_common::federation::FederationAddressKeychain::Internal
            {
                return Err(Error::MaintenanceExecutionFailed);
            }
        }

        let local_outputs = self
            .recovery_wallet
            .observed_outputs()
            .await
            .into_iter()
            .map(|output| (output.outpoint, output.txout))
            .collect::<BTreeMap<_, _>>();
        let mut candidates = chain_state
            .utxos()
            .filter(|utxo| {
                utxo.state == FederationWalletUtxoState::Spendable
                    && utxo.allocation.scope() == recovery_scope
                    && utxo.allocation.keychain
                        == cdk_common::federation::FederationAddressKeychain::External
                    && local_outputs.get(&utxo.outpoint) == Some(&utxo.txout)
            })
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by_key(|utxo| utxo.outpoint);
        if candidates.is_empty() {
            return Err(Error::RecoveryFeeReserveInsufficient);
        }
        for candidate in &candidates {
            let expected = recovery_application
                .derive_script(
                    &self.federation,
                    &frost_wallet.root,
                    false,
                    candidate.allocation.derivation_index,
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
            if expected != candidate.txout.script_pubkey {
                return Err(Error::MaintenanceExecutionFailed);
            }
        }

        let fee_ceiling_sat = u64::from(plan.fee_bound);
        let fee_rate = FeeRate::from_sat_per_kwu(FEDERATION_BARK_EXIT_FEE_RATE_SAT_PER_KWU);
        let mut candidate_offset = 0_usize;
        let mut aggregate_fee_sat = 0_u64;
        let mut packages = Vec::with_capacity(parents.len());
        for (parent_txid, parent) in parents {
            let quote = federation_bark_cpfp_allocation_quote(plan.action_id, parent_txid);
            let change = (*changes
                .get(quote.as_str())
                .ok_or(Error::MaintenanceExecutionFailed)?)
            .clone();
            let change_script = recovery_application
                .derive_script(
                    &self.federation,
                    &frost_wallet.root,
                    true,
                    change.derivation_index,
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
            let mut prepared = None;
            for candidate_end in (candidate_offset + 1)..=candidates.len() {
                let selected = &candidates[candidate_offset..candidate_end];
                let outpoints = selected
                    .iter()
                    .map(|utxo| utxo.outpoint)
                    .collect::<Vec<_>>();
                let provisional = match self
                    .recovery_wallet
                    .engine()
                    .build_federated_p2a_cpfp(&parent, 1, change_script.clone(), &outpoints)
                    .await
                {
                    Ok(provisional) => provisional,
                    Err(_) => continue,
                };
                let mut signed_weight_template = provisional.unsigned_tx.clone();
                signed_weight_template.input[0].witness = Witness::new();
                for input in signed_weight_template.input.iter_mut().skip(1) {
                    input.witness = Witness::from_slice(&[vec![0_u8; 64]]);
                }
                let package_weight = Weight::from_wu(
                    parent
                        .weight()
                        .to_wu()
                        .checked_add(signed_weight_template.weight().to_wu())
                        .ok_or(Error::MaintenanceExecutionFailed)?,
                );
                let fee_sat = fee_rate
                    .fee_wu(package_weight)
                    .ok_or(Error::MaintenanceExecutionFailed)?
                    .to_sat()
                    .max(1);
                if aggregate_fee_sat
                    .checked_add(fee_sat)
                    .is_none_or(|total| total > fee_ceiling_sat)
                {
                    return Err(Error::RecoveryFeeReserveInsufficient);
                }
                let psbt = match self
                    .recovery_wallet
                    .engine()
                    .build_federated_p2a_cpfp(&parent, fee_sat, change_script.clone(), &outpoints)
                    .await
                {
                    Ok(psbt) => psbt,
                    Err(_) => continue,
                };
                let recovery_inputs = selected
                    .iter()
                    .map(|utxo| FederationBarkRecoveryInput {
                        outpoint: utxo.outpoint,
                        previous_output: utxo.txout.clone(),
                        allocation: utxo.allocation.clone(),
                    })
                    .collect::<Vec<_>>();
                let package = FederationBarkCpfpPackage::new(
                    parent.clone(),
                    recovery_inputs,
                    change,
                    change_script.clone().into_bytes(),
                    psbt.unsigned_tx,
                    fee_sat,
                    fee_ceiling_sat,
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
                prepared = Some((package, candidate_end, fee_sat));
                break;
            }
            let (package, next_offset, fee_sat) =
                prepared.ok_or(Error::RecoveryFeeReserveInsufficient)?;
            candidate_offset = next_offset;
            aggregate_fee_sat = aggregate_fee_sat
                .checked_add(fee_sat)
                .ok_or(Error::MaintenanceExecutionFailed)?;
            packages.push(package);
        }
        FederationBarkCpfpPackageSet::new(
            plan.action_id,
            accepted_plan.operation_id,
            action.actor,
            action.attempt,
            federation_bark_signer_set_for_attempt(&self.federation, action.attempt)
                .map_err(|_| Error::MaintenanceExecutionFailed)?,
            packages,
        )
        .map_err(|_| Error::MaintenanceExecutionFailed)
    }

    /// Sign every recovery input in an accepted CPFP package set and finalize
    /// the exact v3 children without granting Bark access to a private key.
    pub async fn sign_registered_cpfp_packages(
        &self,
        action: &FederationBarkActionRecord,
        registered_entry: &FederationJournalEntry,
    ) -> Result<FederationBarkCpfpPublication, Error> {
        if registered_entry.status() != FederationOperationStatus::Applied {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let registered = accepted_send_transition(
            registered_entry,
            FederationBarkSemanticPhase::ExitRegistered,
        )?;
        let package_sets = registered
            .artifacts
            .iter()
            .filter_map(|artifact| FederationBarkCpfpPackageSet::from_artifact(artifact).ok())
            .collect::<Vec<_>>();
        let [package_set] = package_sets.as_slice() else {
            return Err(Error::MaintenanceExecutionFailed);
        };
        if package_set.action_id != registered.action_id
            || package_set.plan_operation_id != registered.plan_operation_id
            || package_set.actor != registered.actor
            || package_set.attempt != registered.attempt
            || package_set.signer_set != registered.signer_set
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let FederationBarkOperation::ActionTransition(authorization_head) = &action.operation
        else {
            return Err(Error::MaintenanceExecutionFailed);
        };
        if action.action_id != registered.action_id
            || action.signing_retry_phase.unwrap_or(action.phase)
                != FederationBarkSemanticPhase::ExitRegistered
            || action.plan_operation_id != Some(registered.plan_operation_id)
            || (action.phase == FederationBarkSemanticPhase::ExitRegistered
                && action.latest_operation_id != registered_entry.operation_id)
            || action.actor != self.member_id
            || authorization_head.action_id != registered.action_id
            || authorization_head.phase != action.phase
            || authorization_head.actor != action.actor
            || authorization_head.plan_operation_id != registered.plan_operation_id
            || authorization_head.attempt != action.attempt
            || action.attempt < package_set.attempt
            || authorization_head.signer_set.is_empty()
        {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let frost_wallet = self
            .federation
            .frost_wallet
            .as_ref()
            .ok_or(Error::MaintenanceExecutionFailed)?;
        let recovery_application = federation_bark_recovery_application(&self.federation)
            .map_err(|_| Error::MaintenanceExecutionFailed)?
            .ok_or(Error::MaintenanceExecutionFailed)?;
        let public = recovery_application
            .derived_public_package(&self.federation, &frost_wallet.root)
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let adapter = FrostBip32::new(public.clone(), recovery_application.network())
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let signers = authorization_head
            .signer_set
            .iter()
            .map(|member| cdk_frost::MemberId::new(member.get()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| Error::MaintenanceExecutionFailed)?;
        let mut signed_children = Vec::with_capacity(package_set.packages.len());
        for package in &package_set.packages {
            package
                .validate()
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
            let mut signatures = Vec::with_capacity(package.recovery_inputs.len());
            for (input_index, selected) in package.recovery_inputs.iter().enumerate() {
                let branch = match selected.allocation.keychain {
                    cdk_common::federation::FederationAddressKeychain::External => {
                        WalletBranch::External
                    }
                    cdk_common::federation::FederationAddressKeychain::Internal => {
                        WalletBranch::Internal
                    }
                };
                let index = WalletIndex::new(selected.allocation.derivation_index)
                    .map_err(|_| Error::MaintenanceExecutionFailed)?;
                let derived = adapter
                    .derive(branch, index)
                    .map_err(|_| Error::MaintenanceExecutionFailed)?;
                let typed = FederationBarkRecoveryFrostAuthorization::new(
                    registered_entry.operation_id,
                    action.latest_operation_id,
                    registered.action_id,
                    package,
                    input_index,
                    derived.signing_derivation(),
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
                let scope = SigningAuthorizationScope::new(
                    self.federation.federation_id.to_bytes(),
                    RootKeyEpoch::new(recovery_application.root_key_epoch())
                        .map_err(|_| Error::MaintenanceExecutionFailed)?,
                    public.registration(),
                    derived.signing_derivation().binding(),
                    typed.authorization_reference,
                    typed.message,
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
                let context = SigningSessionContext::new_for_protocol(
                    scope,
                    action.attempt,
                    signers.clone(),
                    CustodyProtocol::OrdinaryFrost,
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
                let authorization = AuthorizationRequest::new(
                    public.registration().id(),
                    public.registration().key_epoch(),
                    public.registration().derivation_version(),
                    context.session_id(),
                    typed
                        .to_bytes()
                        .map_err(|_| Error::MaintenanceExecutionFailed)?,
                )
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
                let signature = self
                    .recovery_signer
                    .sign_frost(&context, derived.signing_derivation(), &authorization)
                    .await
                    .map_err(|_| Error::CustodySignerUnavailable)?;
                signatures.push(signature.to_bytes());
            }
            let selected_inputs = package
                .recovery_inputs
                .iter()
                .map(|input| (input.outpoint, input.previous_output.clone()))
                .collect::<Vec<_>>();
            let child = self
                .recovery_wallet
                .engine()
                .finalize_federated_p2a_cpfp(
                    &package.parent,
                    &package.unsigned_child,
                    &selected_inputs,
                    &signatures,
                )
                .await
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
            signed_children.push(child);
        }
        FederationBarkCpfpPublication::new(package_set, signed_children)
            .map_err(|_| Error::MaintenanceExecutionFailed)
    }

    /// Supply only accepted signed children to Bark's exit manager.
    ///
    /// Replaying this method is safe: parents which no longer request a CPFP
    /// are skipped, while any new unmatched request fails closed.
    pub async fn provide_accepted_cpfp_packages(
        &self,
        package_set: &FederationBarkCpfpPackageSet,
        publication: &FederationBarkCpfpPublication,
    ) -> Result<(), Error> {
        let expected =
            FederationBarkCpfpPublication::new(package_set, publication.signed_children.clone())
                .map_err(|_| Error::MaintenanceExecutionFailed)?;
        if publication != &expected {
            return Err(Error::MaintenanceExecutionFailed);
        }
        let requests = self.bark_wallet.exit_mgr().exits_needing_cpfp().await;
        if requests.iter().any(|request| {
            !package_set
                .packages
                .iter()
                .any(|package| package.parent == request.exit_tx)
        }) {
            return Err(Error::MaintenanceExecutionFailed);
        }
        for (package, child) in package_set
            .packages
            .iter()
            .zip(&publication.signed_children)
        {
            if requests
                .iter()
                .any(|request| request.exit_tx == package.parent)
            {
                self.bark_wallet
                    .exit_mgr()
                    .provide_cpfp_tx(
                        &self.bark_wallet,
                        package.parent.compute_txid(),
                        child.clone(),
                    )
                    .await
                    .map_err(|_| Error::MaintenanceExecutionFailed)?;
            }
        }
        Ok(())
    }
}
