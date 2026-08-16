//! Consensus-bound Bark recovery CPFP packages and signing authorization.

use std::collections::{BTreeMap, BTreeSet};

use bitcoin::hashes::{sha256::Hash as Sha256Hash, Hash};
use bitcoin::secp256k1::{schnorr, Message, Secp256k1, XOnlyPublicKey};
use bitcoin::sighash::{Prevouts, SighashCache};
use bitcoin::{
    absolute, transaction, OutPoint, ScriptBuf, Sequence, TapSighashType, Transaction, TxOut,
};
use cdk_common::federation::{
    FederationAllocatedAddressRef, FederationChainObservationState, FederationJournalEntry,
    FederationMaterializedStateRow, FederationOperationId, FederationOperationStatus,
    FederationWalletUtxoState, MemberId, Result,
};
use cdk_frost::{
    ApplicationAuthorization, ApplicationRegistration, AuthorizationRequest,
    AuthorizedSigningRequest, SigningDerivation,
};
use serde::{Deserialize, Serialize};

use crate::{
    canonical_json_bytes, invalid_bark, validate_federation_bark_operation, FederationBarkActionId,
    FederationBarkActionRecord, FederationBarkActionTransition, FederationBarkArtifact,
    FederationBarkArtifactKind, FederationBarkDigest, FederationBarkOperation,
    FederationBarkProjectionSnapshot, FederationBarkRecoveryApplicationConfig,
    FederationBarkSemanticPhase,
};

const BARK_CPFP_PACKAGE_VERSION: u16 = 1;
const BARK_CPFP_PACKAGE_SET_VERSION: u16 = 1;
const BARK_CPFP_PUBLICATION_VERSION: u16 = 1;
const BARK_RECOVERY_FROST_AUTHORIZATION_VERSION: u16 = 2;
const BARK_RECOVERY_FROST_AUTHORIZATION_DOMAIN: &[u8] =
    b"cdk/federation/bark-recovery-frost-authorization/v1";
const MAX_RECOVERY_AUTHORIZATION_BYTES: usize = 16 * 1024;
const BARK_RECOVERY_RESERVATION_STATE_VERSION: u16 = 1;

/// Durable owner of one Bark recovery-wallet fee input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRecoveryReservation {
    /// Reserved recovery-wallet outpoint.
    pub outpoint: OutPoint,
    /// Exit action owning the reservation.
    pub action_id: FederationBarkActionId,
    /// Accepted `ExitRegistered` operation which created the reservation.
    pub registered_operation_id: FederationOperationId,
    /// Accepted `ExitPublished` operation which consumed it, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_operation_id: Option<FederationOperationId>,
}

/// Bark-owned reservation projection layered over the neutral chain state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRecoveryState {
    version: u16,
    reservations: BTreeMap<String, FederationBarkRecoveryReservation>,
}

impl Default for FederationBarkRecoveryState {
    fn default() -> Self {
        Self::new()
    }
}

impl FederationBarkRecoveryState {
    /// Construct an empty current-version projection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: BARK_RECOVERY_RESERVATION_STATE_VERSION,
            reservations: BTreeMap::new(),
        }
    }

    /// Decode and validate durable projection bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let value: Self = serde_json::from_slice(bytes)?;
        value.validate()?;
        Ok(value)
    }

    /// Encode validated durable projection bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }

    /// Build stable materialized-state rows for checkpoint convergence.
    pub fn materialized_rows(&self) -> Result<Vec<FederationMaterializedStateRow>> {
        self.validate()?;
        Ok(self
            .reservations
            .values()
            .map(|reservation| {
                FederationMaterializedStateRow::new(
                    "federation_bark_recovery_reservations",
                    reservation.outpoint.to_string(),
                )
                .with_field("action_id", reservation.action_id.to_string())
                .with_field(
                    "registered_operation_id",
                    reservation.registered_operation_id.to_string(),
                )
                .with_field(
                    "published_operation_id",
                    reservation
                        .published_operation_id
                        .map(|operation_id| operation_id.to_string())
                        .unwrap_or_default(),
                )
            })
            .collect())
    }

    /// Reserve every exact package input. Returns whether state changed.
    pub fn reserve(
        &mut self,
        package_set: &FederationBarkCpfpPackageSet,
        registered_operation_id: FederationOperationId,
    ) -> Result<bool> {
        package_set.validate()?;
        let expected = package_set
            .packages
            .iter()
            .flat_map(|package| &package.recovery_inputs)
            .map(|input| input.outpoint)
            .collect::<BTreeSet<_>>();
        let existing = expected.iter().all(|outpoint| {
            self.reservations
                .get(&outpoint.to_string())
                .is_some_and(|reservation| {
                    reservation.action_id == package_set.action_id
                        && reservation.registered_operation_id == registered_operation_id
                        && reservation.published_operation_id.is_none()
                })
        });
        if existing {
            return Ok(false);
        }
        for outpoint in &expected {
            if self.reservations.contains_key(&outpoint.to_string()) {
                return Err(invalid_bark(
                    "Bark recovery input is reserved by a different exit",
                ));
            }
        }
        for outpoint in expected {
            self.reservations.insert(
                outpoint.to_string(),
                FederationBarkRecoveryReservation {
                    outpoint,
                    action_id: package_set.action_id,
                    registered_operation_id,
                    published_operation_id: None,
                },
            );
        }
        self.validate()?;
        Ok(true)
    }

    /// Mark every exact reserved package input consumed by publication.
    pub fn publish(
        &mut self,
        package_set: &FederationBarkCpfpPackageSet,
        registered_operation_id: FederationOperationId,
        published_operation_id: FederationOperationId,
    ) -> Result<bool> {
        package_set.validate()?;
        let outpoints = package_set
            .packages
            .iter()
            .flat_map(|package| &package.recovery_inputs)
            .map(|input| input.outpoint)
            .collect::<Vec<_>>();
        let replay = outpoints.iter().all(|outpoint| {
            self.reservations
                .get(&outpoint.to_string())
                .is_some_and(|reservation| {
                    reservation.action_id == package_set.action_id
                        && reservation.registered_operation_id == registered_operation_id
                        && reservation.published_operation_id == Some(published_operation_id)
                })
        });
        if replay {
            return Ok(false);
        }
        for outpoint in &outpoints {
            let reservation = self
                .reservations
                .get(&outpoint.to_string())
                .ok_or_else(|| invalid_bark("Bark recovery publication has no reservation"))?;
            if reservation.action_id != package_set.action_id
                || reservation.registered_operation_id != registered_operation_id
                || reservation.published_operation_id.is_some()
            {
                return Err(invalid_bark(
                    "Bark recovery publication differs from its reservation",
                ));
            }
        }
        for outpoint in outpoints {
            self.reservations
                .get_mut(&outpoint.to_string())
                .ok_or_else(|| invalid_bark("Bark recovery reservation disappeared"))?
                .published_operation_id = Some(published_operation_id);
        }
        self.validate()?;
        Ok(true)
    }

    fn validate(&self) -> Result<()> {
        if self.version != BARK_RECOVERY_RESERVATION_STATE_VERSION
            || self.reservations.iter().any(|(key, reservation)| {
                key != &reservation.outpoint.to_string()
                    || reservation.action_id.to_bytes() == [0; 32]
            })
        {
            return Err(invalid_bark("Bark recovery reservation state is invalid"));
        }
        Ok(())
    }
}

/// One exact Bark recovery-wallet input reserved for a CPFP child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRecoveryInput {
    /// Consensus-observed outpoint.
    pub outpoint: OutPoint,
    /// Exact previous output used for BIP341 sighash construction.
    pub previous_output: TxOut,
    /// Accepted recovery-wallet allocation owning the output.
    pub allocation: FederationAllocatedAddressRef,
}

/// One exact parent/child package prepared before exit registration is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkCpfpPackage {
    /// Package schema version.
    pub version: u16,
    /// Parent exit transaction containing the P2A anchor.
    #[serde(with = "transaction_bytes")]
    pub parent: Transaction,
    /// Sorted recovery-wallet fee inputs; the P2A input is implicit and first.
    pub recovery_inputs: Vec<FederationBarkRecoveryInput>,
    /// Accepted internal allocation receiving deterministic change.
    pub change: FederationAllocatedAddressRef,
    /// Exact change script derived from `change`.
    #[serde(with = "hex_bytes")]
    pub change_script: Vec<u8>,
    /// Exact non-witness child transaction accepted before signing.
    #[serde(with = "transaction_bytes")]
    pub unsigned_child: Transaction,
    /// Exact child fee in satoshis.
    pub fee_sat: u64,
    /// Maximum aggregate fee authorized by the exit plan.
    pub fee_ceiling_sat: u64,
}

impl FederationBarkCpfpPackage {
    /// Construct and validate one exact CPFP package.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent: Transaction,
        recovery_inputs: Vec<FederationBarkRecoveryInput>,
        change: FederationAllocatedAddressRef,
        change_script: Vec<u8>,
        unsigned_child: Transaction,
        fee_sat: u64,
        fee_ceiling_sat: u64,
    ) -> Result<Self> {
        let value = Self {
            version: BARK_CPFP_PACKAGE_VERSION,
            parent,
            recovery_inputs,
            change,
            change_script,
            unsigned_child,
            fee_sat,
            fee_ceiling_sat,
        };
        value.validate()?;
        Ok(value)
    }

    /// Validate the Bitcoin package shape and return its P2A parent outpoint.
    pub fn validate(&self) -> Result<OutPoint> {
        if self.version != BARK_CPFP_PACKAGE_VERSION
            || self.recovery_inputs.is_empty()
            || self.change_script.is_empty()
            || self.fee_sat == 0
            || self.fee_sat > self.fee_ceiling_sat
            || self.unsigned_child.version != transaction::Version(3)
            || self.unsigned_child.lock_time != absolute::LockTime::ZERO
            || self.unsigned_child.output.len() != 1
            || self.unsigned_child.output[0].script_pubkey.as_bytes() != self.change_script
        {
            return Err(invalid_bark("recovery CPFP package shape is invalid"));
        }
        let anchors = self
            .parent
            .output
            .iter()
            .enumerate()
            .filter(|(_, output)| output.script_pubkey == ScriptBuf::new_p2a())
            .collect::<Vec<_>>();
        let [(anchor_index, anchor_output)] = anchors.as_slice() else {
            return Err(invalid_bark(
                "recovery CPFP parent must contain exactly one P2A anchor",
            ));
        };
        let anchor_index = u32::try_from(*anchor_index)
            .map_err(|_| invalid_bark("recovery CPFP anchor index overflow"))?;
        let anchor = OutPoint::new(self.parent.compute_txid(), anchor_index);
        if self.unsigned_child.input.len() != self.recovery_inputs.len().saturating_add(1)
            || self.unsigned_child.input[0].previous_output != anchor
            || self.unsigned_child.input.iter().any(|input| {
                input.sequence != Sequence::ENABLE_RBF_NO_LOCKTIME
                    || !input.script_sig.is_empty()
                    || !input.witness.is_empty()
            })
        {
            return Err(invalid_bark("recovery CPFP inputs differ from the package"));
        }
        let mut previous = None;
        let mut unique = BTreeSet::new();
        for (offset, input) in self.recovery_inputs.iter().enumerate() {
            if input.previous_output.value.to_sat() == 0
                || input.previous_output.script_pubkey.is_empty()
                || input.allocation.keychain
                    != cdk_common::federation::FederationAddressKeychain::External
                || !unique.insert(input.outpoint)
                || previous.is_some_and(|value| value >= input.outpoint)
                || self.unsigned_child.input[offset + 1].previous_output != input.outpoint
            {
                return Err(invalid_bark(
                    "recovery CPFP fee inputs are invalid or non-canonical",
                ));
            }
            previous = Some(input.outpoint);
        }
        if self.change.keychain != cdk_common::federation::FederationAddressKeychain::Internal
            || self
                .recovery_inputs
                .iter()
                .any(|input| input.allocation.scope() != self.change.scope())
        {
            return Err(invalid_bark(
                "recovery CPFP inputs and change do not share the recovery scope",
            ));
        }
        let input_sat =
            self.recovery_inputs
                .iter()
                .try_fold(anchor_output.value.to_sat(), |sum, input| {
                    sum.checked_add(input.previous_output.value.to_sat())
                        .ok_or_else(|| invalid_bark("recovery CPFP input amount overflow"))
                })?;
        let output_sat = self.unsigned_child.output[0].value.to_sat();
        if input_sat.checked_sub(output_sat) != Some(self.fee_sat) {
            return Err(invalid_bark(
                "recovery CPFP fee differs from transaction value",
            ));
        }
        Ok(anchor)
    }

    /// Stable commitment to the complete accepted package.
    pub fn commitment(&self) -> Result<FederationBarkDigest> {
        self.validate()?;
        FederationBarkDigest::from_bytes(
            Sha256Hash::hash(&canonical_json_bytes(self)?).to_byte_array(),
        )
    }

    /// Recompute the exact BIP341 key-path message for one recovery input.
    pub fn signing_message(&self, recovery_input_index: usize) -> Result<[u8; 32]> {
        self.validate()?;
        let child_input_index = recovery_input_index
            .checked_add(1)
            .ok_or_else(|| invalid_bark("recovery CPFP input index overflow"))?;
        if recovery_input_index >= self.recovery_inputs.len() {
            return Err(invalid_bark("recovery CPFP input index is out of bounds"));
        }
        let anchor_output = self
            .parent
            .output
            .iter()
            .find(|output| output.script_pubkey == ScriptBuf::new_p2a())
            .cloned()
            .ok_or_else(|| invalid_bark("recovery CPFP parent has no anchor"))?;
        let mut prevouts = Vec::with_capacity(self.recovery_inputs.len().saturating_add(1));
        prevouts.push(anchor_output);
        prevouts.extend(
            self.recovery_inputs
                .iter()
                .map(|input| input.previous_output.clone()),
        );
        SighashCache::new(&self.unsigned_child)
            .taproot_key_spend_signature_hash(
                child_input_index,
                &Prevouts::All(&prevouts),
                TapSighashType::Default,
            )
            .map(|sighash| sighash.to_byte_array())
            .map_err(|error| invalid_bark(error.to_string()))
    }
}

/// All CPFP packages bound into one accepted `ExitRegistered` transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkCpfpPackageSet {
    /// Package-set schema version.
    pub version: u16,
    /// Exit action owning every package.
    pub action_id: FederationBarkActionId,
    /// Accepted exit plan operation.
    pub plan_operation_id: FederationOperationId,
    /// Actor permitted to coordinate signing and publication.
    pub actor: MemberId,
    /// Accepted signing attempt.
    pub attempt: u32,
    /// Deterministic sorted signer set.
    pub signer_set: Vec<MemberId>,
    /// Packages sorted by parent transaction ID.
    pub packages: Vec<FederationBarkCpfpPackage>,
}

impl FederationBarkCpfpPackageSet {
    /// Construct and validate a package set.
    pub fn new(
        action_id: FederationBarkActionId,
        plan_operation_id: FederationOperationId,
        actor: MemberId,
        attempt: u32,
        signer_set: Vec<MemberId>,
        packages: Vec<FederationBarkCpfpPackage>,
    ) -> Result<Self> {
        let value = Self {
            version: BARK_CPFP_PACKAGE_SET_VERSION,
            action_id,
            plan_operation_id,
            actor,
            attempt,
            signer_set,
            packages,
        };
        value.validate()?;
        Ok(value)
    }

    /// Validate canonical ordering, fee budget, and every package.
    pub fn validate(&self) -> Result<()> {
        if self.version != BARK_CPFP_PACKAGE_SET_VERSION
            || self.attempt == 0
            || self.signer_set.is_empty()
            || self.packages.is_empty()
            || self.signer_set.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invalid_bark("recovery CPFP package set is invalid"));
        }
        let mut previous_parent = None;
        let mut all_inputs = BTreeSet::new();
        let mut total_fee = 0_u64;
        let mut fee_ceiling = None;
        for package in &self.packages {
            let anchor = package.validate()?;
            if previous_parent.is_some_and(|txid| txid >= anchor.txid)
                || package
                    .recovery_inputs
                    .iter()
                    .any(|input| !all_inputs.insert(input.outpoint))
            {
                return Err(invalid_bark(
                    "recovery CPFP packages reuse inputs or are not canonical",
                ));
            }
            previous_parent = Some(anchor.txid);
            total_fee = total_fee
                .checked_add(package.fee_sat)
                .ok_or_else(|| invalid_bark("recovery CPFP fee overflow"))?;
            match fee_ceiling {
                Some(expected) if expected != package.fee_ceiling_sat => {
                    return Err(invalid_bark("recovery CPFP fee ceilings differ"));
                }
                Some(_) => {}
                None => fee_ceiling = Some(package.fee_ceiling_sat),
            }
        }
        if total_fee > fee_ceiling.unwrap_or_default() {
            return Err(invalid_bark(
                "recovery CPFP package set exceeds its aggregate fee ceiling",
            ));
        }
        Ok(())
    }

    /// Canonical proof artifact carried by `ExitRegistered`.
    pub fn artifact(&self) -> Result<FederationBarkArtifact> {
        self.validate()?;
        let bytes = canonical_json_bytes(self)?;
        Ok(FederationBarkArtifact {
            kind: FederationBarkArtifactKind::Proof,
            hash: FederationBarkDigest::from_bytes(Sha256Hash::hash(&bytes).to_byte_array())?,
            bytes,
        })
    }

    /// Decode one canonical package-set proof artifact.
    pub fn from_artifact(artifact: &FederationBarkArtifact) -> Result<Self> {
        if artifact.kind != FederationBarkArtifactKind::Proof {
            return Err(invalid_bark("recovery CPFP package set is not a proof"));
        }
        let value: Self = serde_json::from_slice(&artifact.bytes)?;
        value.validate()?;
        if value.artifact()? != *artifact {
            return Err(invalid_bark("recovery CPFP package set is not canonical"));
        }
        Ok(value)
    }
}

/// Signed children bound into the existing `ExitPublished` phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkCpfpPublication {
    /// Publication schema version.
    pub version: u16,
    /// Commitment to the accepted package set.
    pub package_set: FederationBarkDigest,
    /// Signed children in package order.
    #[serde(with = "transaction_vec_bytes")]
    pub signed_children: Vec<Transaction>,
}

impl FederationBarkCpfpPublication {
    /// Construct signed publication evidence after exact transaction checks.
    pub fn new(
        package_set: &FederationBarkCpfpPackageSet,
        signed_children: Vec<Transaction>,
    ) -> Result<Self> {
        if signed_children.len() != package_set.packages.len()
            || signed_children
                .iter()
                .zip(&package_set.packages)
                .any(|(signed, package)| {
                    let mut unsigned = signed.clone();
                    for input in &mut unsigned.input {
                        input.script_sig = ScriptBuf::new();
                        input.witness.clear();
                    }
                    unsigned != package.unsigned_child
                })
        {
            return Err(invalid_bark(
                "signed recovery children differ from accepted packages",
            ));
        }
        let secp = Secp256k1::verification_only();
        for (package, child) in package_set.packages.iter().zip(&signed_children) {
            if !child.input[0].witness.is_empty() {
                return Err(invalid_bark("recovery P2A input must remain unsigned"));
            }
            for (input_index, recovery_input) in package.recovery_inputs.iter().enumerate() {
                let witness = &child.input[input_index + 1].witness;
                let signature_bytes = witness
                    .iter()
                    .next()
                    .filter(|_| witness.len() == 1)
                    .ok_or_else(|| invalid_bark("recovery input witness is not key-path only"))?;
                let signature = schnorr::Signature::from_slice(signature_bytes)
                    .map_err(|error| invalid_bark(error.to_string()))?;
                let script = recovery_input.previous_output.script_pubkey.as_bytes();
                if script.len() != 34 || script[0] != 0x51 || script[1] != 0x20 {
                    return Err(invalid_bark("recovery input is not P2TR"));
                }
                let output_key = XOnlyPublicKey::from_slice(&script[2..])
                    .map_err(|error| invalid_bark(error.to_string()))?;
                secp.verify_schnorr(
                    &signature,
                    &Message::from_digest(package.signing_message(input_index)?),
                    &output_key,
                )
                .map_err(|error| invalid_bark(error.to_string()))?;
            }
        }
        let value = Self {
            version: BARK_CPFP_PUBLICATION_VERSION,
            package_set: FederationBarkDigest::from_bytes(
                Sha256Hash::hash(&canonical_json_bytes(package_set)?).to_byte_array(),
            )?,
            signed_children,
        };
        value.validate()?;
        Ok(value)
    }

    /// Canonical proof artifact carried by `ExitPublished`.
    pub fn artifact(&self) -> Result<FederationBarkArtifact> {
        self.validate()?;
        let bytes = canonical_json_bytes(self)?;
        Ok(FederationBarkArtifact {
            kind: FederationBarkArtifactKind::Proof,
            hash: FederationBarkDigest::from_bytes(Sha256Hash::hash(&bytes).to_byte_array())?,
            bytes,
        })
    }

    /// Decode one canonical publication proof artifact.
    pub fn from_artifact(artifact: &FederationBarkArtifact) -> Result<Self> {
        if artifact.kind != FederationBarkArtifactKind::Proof {
            return Err(invalid_bark("recovery CPFP publication is not a proof"));
        }
        let value: Self = serde_json::from_slice(&artifact.bytes)?;
        value.validate()?;
        if value.artifact()? != *artifact {
            return Err(invalid_bark("recovery CPFP publication is not canonical"));
        }
        Ok(value)
    }

    fn validate(&self) -> Result<()> {
        if self.version != BARK_CPFP_PUBLICATION_VERSION
            || self.package_set.to_bytes() == [0; 32]
            || self.signed_children.is_empty()
        {
            return Err(invalid_bark("recovery CPFP publication is invalid"));
        }
        Ok(())
    }
}

/// Typed ordinary-FROST authorization for one accepted recovery-wallet input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationBarkRecoveryFrostAuthorization {
    /// Authorization schema version.
    pub version: u16,
    /// Accepted `ExitRegistered` operation authorizing this exact package.
    pub registered_operation_id: FederationOperationId,
    /// Accepted action head authorizing the current signer attempt.
    pub authorization_operation_id: FederationOperationId,
    /// Exit action owning the package.
    pub action_id: FederationBarkActionId,
    /// Package commitment.
    pub package: FederationBarkDigest,
    /// Recovery input index excluding the implicit P2A input.
    pub recovery_input_index: u32,
    /// Canonical accepted-state authorization reference.
    pub authorization_reference: [u8; 32],
    /// Exact BIP341 key-path sighash.
    pub message: [u8; 32],
    /// Encoded application-derived BIP32/Taproot signing plan.
    pub derivation: Vec<u8>,
}

/// Exact accepted recovery state revalidated before one FROST share is released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederationBarkValidatedRecoveryAuthorization {
    /// Accepted exit registration.
    pub registered: FederationBarkActionTransition,
    /// Current accepted action head, including any actor/signer replacement.
    pub action: FederationBarkActionRecord,
    /// Transition authorizing the current custody attempt.
    pub authorization_head: FederationBarkActionTransition,
    /// Exact accepted CPFP package.
    pub package: FederationBarkCpfpPackage,
    /// Exact recovery-wallet input selected within the package.
    pub input: FederationBarkRecoveryInput,
}

impl FederationBarkRecoveryFrostAuthorization {
    /// Build an authorization from a revalidated accepted package.
    pub fn new(
        registered_operation_id: FederationOperationId,
        authorization_operation_id: FederationOperationId,
        action_id: FederationBarkActionId,
        package: &FederationBarkCpfpPackage,
        recovery_input_index: usize,
        derivation: &SigningDerivation,
    ) -> Result<Self> {
        let recovery_input_index = u32::try_from(recovery_input_index)
            .map_err(|_| invalid_bark("recovery FROST input index overflow"))?;
        let package_hash = package.commitment()?;
        let message = package.signing_message(recovery_input_index as usize)?;
        let authorization_reference = recovery_authorization_reference(
            registered_operation_id,
            authorization_operation_id,
            action_id,
            package_hash,
            recovery_input_index,
            message,
        );
        Ok(Self {
            version: BARK_RECOVERY_FROST_AUTHORIZATION_VERSION,
            registered_operation_id,
            authorization_operation_id,
            action_id,
            package: package_hash,
            recovery_input_index,
            authorization_reference,
            message,
            derivation: derivation.encode(),
        })
    }

    /// Decode and canonically validate an adapter payload.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_RECOVERY_AUTHORIZATION_BYTES {
            return Err(invalid_bark("recovery FROST authorization is oversized"));
        }
        let value: Self = serde_json::from_slice(bytes)?;
        value.validate()?;
        if value.to_bytes()? != bytes {
            return Err(invalid_bark(
                "recovery FROST authorization is not canonical",
            ));
        }
        Ok(value)
    }

    /// Revalidate this request against the current accepted Bark and chain state.
    pub fn validate_accepted_state(
        &self,
        registered_entry: &FederationJournalEntry,
        projection: &FederationBarkProjectionSnapshot,
        chain_state: &FederationChainObservationState,
    ) -> Result<FederationBarkValidatedRecoveryAuthorization> {
        if registered_entry.operation_id != self.registered_operation_id
            || registered_entry.status() != FederationOperationStatus::Applied
        {
            return Err(invalid_bark(
                "Bark recovery registration is not the referenced applied operation",
            ));
        }
        let FederationBarkOperation::ActionTransition(registered) =
            validate_federation_bark_operation(&registered_entry.envelope.operation)?
        else {
            return Err(invalid_bark("Bark recovery reference is not a transition"));
        };
        if registered.phase != FederationBarkSemanticPhase::ExitRegistered
            || registered.action_id != self.action_id
        {
            return Err(invalid_bark(
                "Bark recovery reference is not an exit registration",
            ));
        }
        let package_sets = registered
            .artifacts
            .iter()
            .filter_map(|artifact| FederationBarkCpfpPackageSet::from_artifact(artifact).ok())
            .collect::<Vec<_>>();
        let [package_set] = package_sets.as_slice() else {
            return Err(invalid_bark(
                "accepted Bark exit must contain exactly one recovery package set",
            ));
        };
        if package_set.action_id != registered.action_id
            || package_set.plan_operation_id != registered.plan_operation_id
            || package_set.actor != registered.actor
            || package_set.attempt != registered.attempt
            || package_set.signer_set != registered.signer_set
        {
            return Err(invalid_bark(
                "Bark recovery package set differs from its accepted transition",
            ));
        }
        let action = projection
            .actions
            .iter()
            .find(|action| action.action_id == registered.action_id)
            .cloned()
            .ok_or_else(|| invalid_bark("Bark recovery action head is unavailable"))?;
        let FederationBarkOperation::ActionTransition(authorization_head) = &action.operation
        else {
            return Err(invalid_bark(
                "Bark recovery action head is not a transition",
            ));
        };
        if action.signing_retry_phase.unwrap_or(action.phase)
            != FederationBarkSemanticPhase::ExitRegistered
            || action.plan_operation_id != Some(registered.plan_operation_id)
            || action.latest_operation_id != self.authorization_operation_id
            || authorization_head.action_id != registered.action_id
            || authorization_head.phase != action.phase
            || authorization_head.actor != action.actor
            || authorization_head.plan_operation_id != registered.plan_operation_id
            || authorization_head.attempt != action.attempt
        {
            return Err(invalid_bark(
                "Bark recovery signer attempt differs from the accepted action head",
            ));
        }
        let authorization_head = authorization_head.clone();
        let package = package_set
            .packages
            .iter()
            .find(|package| package.commitment().ok() == Some(self.package))
            .cloned()
            .ok_or_else(|| invalid_bark("authorized Bark recovery package is unavailable"))?;
        let input_index = usize::try_from(self.recovery_input_index)
            .map_err(|_| invalid_bark("Bark recovery input index does not fit usize"))?;
        let input = package
            .recovery_inputs
            .get(input_index)
            .cloned()
            .ok_or_else(|| invalid_bark("Bark recovery input index is out of bounds"))?;
        let accepted_utxo = chain_state.utxo(input.outpoint).ok_or_else(|| {
            invalid_bark("Bark recovery input is absent from accepted chain state")
        })?;
        if accepted_utxo.txout != input.previous_output
            || accepted_utxo.allocation != input.allocation
            || !matches!(
                accepted_utxo.state,
                FederationWalletUtxoState::Spendable | FederationWalletUtxoState::Reserved
            )
        {
            return Err(invalid_bark(
                "Bark recovery input differs from accepted chain state",
            ));
        }
        Ok(FederationBarkValidatedRecoveryAuthorization {
            registered,
            action,
            authorization_head,
            package,
            input,
        })
    }

    /// Canonically encode the adapter payload.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = canonical_json_bytes(self)?;
        if bytes.len() > MAX_RECOVERY_AUTHORIZATION_BYTES {
            return Err(invalid_bark("recovery FROST authorization is oversized"));
        }
        Ok(bytes)
    }

    /// Decode the exact reviewed signing derivation.
    pub fn signing_derivation(&self) -> Result<SigningDerivation> {
        SigningDerivation::decode(&self.derivation).map_err(|error| invalid_bark(error.to_string()))
    }

    fn validate(&self) -> Result<()> {
        if self.version != BARK_RECOVERY_FROST_AUTHORIZATION_VERSION
            || self.authorization_reference == [0; 32]
            || self.message == [0; 32]
            || self.authorization_reference
                != recovery_authorization_reference(
                    self.registered_operation_id,
                    self.authorization_operation_id,
                    self.action_id,
                    self.package,
                    self.recovery_input_index,
                    self.message,
                )
        {
            return Err(invalid_bark("recovery FROST authorization is invalid"));
        }
        self.signing_derivation()?;
        Ok(())
    }
}

/// Reviewed parser/derivation adapter for the Bark recovery application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FederationBarkRecoveryFrostAuthorizationAdapter {
    registration: ApplicationRegistration,
}

impl FederationBarkRecoveryFrostAuthorizationAdapter {
    /// Construct an adapter from a validated Bark recovery registration.
    pub fn new(application: &FederationBarkRecoveryApplicationConfig) -> Result<Self> {
        Ok(Self {
            registration: application.registration()?,
        })
    }
}

impl ApplicationAuthorization for FederationBarkRecoveryFrostAuthorizationAdapter {
    fn registration(&self) -> ApplicationRegistration {
        self.registration
    }

    fn authorize(
        &self,
        request: &AuthorizationRequest,
    ) -> std::result::Result<AuthorizedSigningRequest, cdk_frost::Error> {
        if request.application_id() != self.registration.id()
            || request.application_key_epoch() != self.registration.key_epoch()
            || request.derivation_version() != self.registration.derivation_version()
        {
            return Err(cdk_frost::Error::Application(
                "Bark recovery authorization uses an unregistered application epoch".to_string(),
            ));
        }
        let authorization = FederationBarkRecoveryFrostAuthorization::from_bytes(request.payload())
            .map_err(|error| cdk_frost::Error::Application(error.to_string()))?;
        let derivation = authorization
            .signing_derivation()
            .map_err(|error| cdk_frost::Error::Application(error.to_string()))?;
        Ok(AuthorizedSigningRequest::new(
            authorization.message,
            derivation,
        ))
    }
}

fn recovery_authorization_reference(
    registered_operation_id: FederationOperationId,
    authorization_operation_id: FederationOperationId,
    action_id: FederationBarkActionId,
    package: FederationBarkDigest,
    recovery_input_index: u32,
    message: [u8; 32],
) -> [u8; 32] {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(BARK_RECOVERY_FROST_AUTHORIZATION_DOMAIN);
    bytes.extend_from_slice(&registered_operation_id.to_bytes());
    bytes.extend_from_slice(&authorization_operation_id.to_bytes());
    bytes.extend_from_slice(&action_id.to_bytes());
    bytes.extend_from_slice(&package.to_bytes());
    bytes.extend_from_slice(&recovery_input_index.to_be_bytes());
    bytes.extend_from_slice(&message);
    Sha256Hash::hash(&bytes).to_byte_array()
}

mod transaction_bytes {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(
        transaction: &Transaction,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&cdk_common::util::hex::encode(
            bitcoin::consensus::serialize(transaction),
        ))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Transaction, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let bytes = cdk_common::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
        bitcoin::consensus::deserialize(&bytes).map_err(serde::de::Error::custom)
    }
}

mod transaction_vec_bytes {
    use super::*;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(
        transactions: &[Transaction],
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        transactions
            .iter()
            .map(|transaction| {
                cdk_common::util::hex::encode(bitcoin::consensus::serialize(transaction))
            })
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Vec<Transaction>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<String>::deserialize(deserializer)?
            .into_iter()
            .map(|value| {
                let bytes =
                    cdk_common::util::hex::decode(&value).map_err(serde::de::Error::custom)?;
                bitcoin::consensus::deserialize(&bytes).map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&cdk_common::util::hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        cdk_common::util::hex::decode(&value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Keypair, SecretKey};
    use bitcoin::{Amount, TxIn, Txid, Witness};
    use cdk_common::federation::{FederationAddressKeychain, FederationFrostApplicationId};

    use super::*;

    fn allocation(
        keychain: FederationAddressKeychain,
        derivation_index: u32,
    ) -> FederationAllocatedAddressRef {
        FederationAllocatedAddressRef {
            application_id: FederationFrostApplicationId::from_bytes([11; 32])
                .expect("application id"),
            application_key_epoch: 1,
            quote: format!("recovery-{derivation_index}"),
            keychain,
            derivation_index,
        }
    }

    fn package_fixture() -> (FederationBarkCpfpPackageSet, Keypair) {
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[42; 32]).expect("secret key");
        let keypair = Keypair::from_secret_key(&secp, &secret);
        let (output_key, _) = keypair.x_only_public_key();
        let mut p2tr_bytes = vec![0x51, 0x20];
        p2tr_bytes.extend_from_slice(&output_key.serialize());
        let recovery_script = ScriptBuf::from_bytes(p2tr_bytes);
        let parent = Transaction {
            version: transaction::Version(3),
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([7; 32]), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(240),
                script_pubkey: ScriptBuf::new_p2a(),
            }],
        };
        let recovery_outpoint = OutPoint::new(Txid::from_byte_array([8; 32]), 1);
        let change_script = ScriptBuf::from_bytes(vec![0x51]);
        let unsigned_child = Transaction {
            version: transaction::Version(3),
            lock_time: absolute::LockTime::ZERO,
            input: vec![
                TxIn {
                    previous_output: OutPoint::new(parent.compute_txid(), 0),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                },
                TxIn {
                    previous_output: recovery_outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                },
            ],
            output: vec![TxOut {
                value: Amount::from_sat(9_240),
                script_pubkey: change_script.clone(),
            }],
        };
        let package = FederationBarkCpfpPackage::new(
            parent,
            vec![FederationBarkRecoveryInput {
                outpoint: recovery_outpoint,
                previous_output: TxOut {
                    value: Amount::from_sat(10_000),
                    script_pubkey: recovery_script,
                },
                allocation: allocation(FederationAddressKeychain::External, 3),
            }],
            allocation(FederationAddressKeychain::Internal, 4),
            change_script.into_bytes(),
            unsigned_child,
            1_000,
            2_000,
        )
        .expect("CPFP package");
        let package_set = FederationBarkCpfpPackageSet::new(
            FederationBarkActionId::from_bytes([2; 32]).expect("action id"),
            FederationOperationId::from_bytes([3; 32]),
            MemberId::new(1).expect("actor"),
            1,
            vec![MemberId::new(1).expect("signer")],
            vec![package],
        )
        .expect("CPFP package set");
        (package_set, keypair)
    }

    fn signed_publication_fixture(
        package_set: &FederationBarkCpfpPackageSet,
        keypair: &Keypair,
    ) -> FederationBarkCpfpPublication {
        let secp = Secp256k1::new();
        let package = &package_set.packages[0];
        let message = Message::from_digest(package.signing_message(0).expect("signing message"));
        let signature = secp.sign_schnorr_no_aux_rand(&message, keypair);
        let mut child = package.unsigned_child.clone();
        child.input[1].witness.push(signature.as_ref());
        FederationBarkCpfpPublication::new(package_set, vec![child]).expect("signed publication")
    }

    #[test]
    fn exact_cpfp_package_and_publication_roundtrip() {
        let (package_set, keypair) = package_fixture();
        let package_artifact = package_set.artifact().expect("package artifact");
        assert_eq!(
            FederationBarkCpfpPackageSet::from_artifact(&package_artifact)
                .expect("package roundtrip"),
            package_set
        );

        let publication = signed_publication_fixture(&package_set, &keypair);
        let publication_artifact = publication.artifact().expect("publication artifact");
        assert_eq!(
            FederationBarkCpfpPublication::from_artifact(&publication_artifact)
                .expect("publication roundtrip"),
            publication
        );
    }

    #[test]
    fn cpfp_publication_rejects_transaction_and_signature_mutations() {
        let (package_set, keypair) = package_fixture();
        let publication = signed_publication_fixture(&package_set, &keypair);

        let mut changed_transaction = publication.signed_children.clone();
        changed_transaction[0].output[0].value = Amount::from_sat(9_239);
        assert!(FederationBarkCpfpPublication::new(&package_set, changed_transaction).is_err());

        let mut changed_signature = publication.signed_children;
        let mut signature = changed_signature[0].input[1]
            .witness
            .iter()
            .next()
            .expect("signature")
            .to_vec();
        signature[0] ^= 1;
        changed_signature[0].input[1].witness = Witness::from_slice(&[signature]);
        assert!(FederationBarkCpfpPublication::new(&package_set, changed_signature).is_err());

        let mut artifact = package_set.artifact().expect("package artifact");
        artifact.bytes.push(b' ');
        assert!(FederationBarkCpfpPackageSet::from_artifact(&artifact).is_err());
    }

    #[test]
    fn recovery_reservations_are_conflict_safe_and_replay_safe() {
        let (package_set, _) = package_fixture();
        let registered = FederationOperationId::from_bytes([4; 32]);
        let published = FederationOperationId::from_bytes([5; 32]);
        let mut state = FederationBarkRecoveryState::new();

        assert!(state
            .reserve(&package_set, registered)
            .expect("initial reservation"));
        assert!(!state
            .reserve(&package_set, registered)
            .expect("reservation replay"));
        assert!(state
            .reserve(&package_set, FederationOperationId::from_bytes([6; 32]))
            .is_err());

        assert!(state
            .publish(&package_set, registered, published)
            .expect("initial publication"));
        assert!(!state
            .publish(&package_set, registered, published)
            .expect("publication replay"));
        assert!(state
            .publish(
                &package_set,
                registered,
                FederationOperationId::from_bytes([7; 32]),
            )
            .is_err());

        let bytes = state.to_bytes().expect("state bytes");
        assert_eq!(
            FederationBarkRecoveryState::from_bytes(&bytes).expect("state roundtrip"),
            state
        );
    }

    #[test]
    fn recovery_authorization_rotates_attempt_without_changing_package() {
        let (package_set, _) = package_fixture();
        let package = &package_set.packages[0];
        let registered = FederationOperationId::from_bytes([4; 32]);
        let first_head = FederationOperationId::from_bytes([5; 32]);
        let retry_head = FederationOperationId::from_bytes([6; 32]);
        let first = FederationBarkRecoveryFrostAuthorization::new(
            registered,
            first_head,
            package_set.action_id,
            package,
            0,
            &SigningDerivation::application_key(),
        )
        .expect("first authorization");
        let retry = FederationBarkRecoveryFrostAuthorization::new(
            registered,
            retry_head,
            package_set.action_id,
            package,
            0,
            &SigningDerivation::application_key(),
        )
        .expect("retry authorization");

        assert_eq!(first.registered_operation_id, retry.registered_operation_id);
        assert_eq!(first.package, retry.package);
        assert_eq!(first.message, retry.message);
        assert_ne!(
            first.authorization_operation_id,
            retry.authorization_operation_id
        );
        assert_ne!(first.authorization_reference, retry.authorization_reference);
        assert_eq!(
            FederationBarkRecoveryFrostAuthorization::from_bytes(
                &retry.to_bytes().expect("retry bytes")
            )
            .expect("retry roundtrip"),
            retry
        );

        let mut stale = first;
        stale.authorization_operation_id = retry_head;
        assert!(stale.to_bytes().is_err());
    }
}
