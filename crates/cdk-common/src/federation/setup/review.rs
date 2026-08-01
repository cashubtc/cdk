//! Stable secret-free operator review of a federation setup proposal.

use core::fmt;

use bitcoin::hashes::{sha256, Hash};
use serde::{Deserialize, Serialize};

use super::{
    ensure_size, invalid, FederationDkgKeysetSpec, FederationPaymentVerificationPolicy,
    FederationSetupBitcoinNetwork, FederationSetupFrostApplication, FederationSetupFrostPolicy,
    FederationSetupId, FederationSetupProposal, FederationSetupProposalHash,
    FederationSetupProtocolVersions, FederationSetupUrlPolicy, MemberId, Result,
    FEDERATION_MAX_SETUP_STATUS_BYTES,
};
use crate::federation::{
    FederationCheckpointPolicy, FederationConfig, FederationId, FederationPaymentObservationPolicy,
    ThresholdParams,
};

/// One proposal member rendered without signatures, nonces, or private material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupMemberReview {
    /// Stable member slot.
    pub member_id: MemberId,
    /// Public wallet-facing mint URL.
    pub public_mint_url: String,
    /// Configured-member federation API URL.
    pub federation_api_url: String,
    /// First 16 lowercase hex characters of the identity-key SHA-256 fingerprint.
    pub identity_fingerprint: String,
}

/// Stable machine-readable and human-renderable proposal review document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupProposalReview {
    /// Setup protocol version.
    pub version: u16,
    /// Setup attempt being reviewed.
    pub setup_id: FederationSetupId,
    /// Federation identifier derived from the complete proposed roster.
    pub proposed_federation_id: FederationId,
    /// Exact proposal hash that approval signs.
    pub proposal_hash: FederationSetupProposalHash,
    /// Bitcoin network committed by the proposal.
    pub network: FederationSetupBitcoinNetwork,
    /// Exact required protocol tuple.
    pub required_versions: FederationSetupProtocolVersions,
    /// Canonically ordered member summary.
    pub members: Vec<FederationSetupMemberReview>,
    /// Signature and consensus thresholds.
    pub threshold_params: ThresholdParams,
    /// Mint and melt observation thresholds.
    pub payment_observation_policy: FederationPaymentObservationPolicy,
    /// Shared checkpoint cadence.
    pub checkpoint_policy: FederationCheckpointPolicy,
    /// Initial BLS keyset policies.
    pub keysets: Vec<FederationDkgKeysetSpec>,
    /// Initial FROST root/application policy.
    pub frost: FederationSetupFrostPolicy,
}

impl FederationSetupProposalReview {
    /// Validate a proposal and construct its complete secret-free review document.
    pub fn from_proposal(
        proposal: &FederationSetupProposal,
        url_policy: FederationSetupUrlPolicy,
    ) -> Result<Self> {
        proposal.validate(url_policy)?;
        let authorization = proposal.body.setup_authorization_hash()?;
        let config = FederationConfig::new(
            authorization,
            proposal.body.threshold_params,
            proposal.body.member_configurations(),
            Vec::new(),
        )?;
        let members = proposal
            .body
            .enrollments
            .values()
            .map(|enrollment| FederationSetupMemberReview {
                member_id: enrollment.member_id,
                public_mint_url: enrollment.public_mint_url.to_string(),
                federation_api_url: enrollment.federation_api_url.to_string(),
                identity_fingerprint: identity_fingerprint(&enrollment.identity_public_key),
            })
            .collect();
        let review = Self {
            version: proposal.body.version,
            setup_id: proposal.body.setup_id,
            proposed_federation_id: config.federation_id,
            proposal_hash: proposal.proposal_hash,
            network: proposal.body.network,
            required_versions: proposal.body.required_versions,
            members,
            threshold_params: proposal.body.threshold_params,
            payment_observation_policy: proposal.body.payment_observation_policy.clone(),
            checkpoint_policy: proposal.body.checkpoint_policy,
            keysets: proposal.body.keysets.clone(),
            frost: proposal.body.frost.clone(),
        };
        review.validate_size()?;
        Ok(review)
    }

    /// Serialize the bounded machine-readable review as stable pretty JSON.
    pub fn to_pretty_json(&self) -> Result<String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| invalid(format!("proposal review JSON failed: {error}")))?;
        ensure_size(
            "proposal review",
            json.len(),
            FEDERATION_MAX_SETUP_STATUS_BYTES,
        )?;
        Ok(json)
    }

    fn validate_size(&self) -> Result<()> {
        let human = self.to_string();
        ensure_size(
            "proposal review",
            human.len(),
            FEDERATION_MAX_SETUP_STATUS_BYTES,
        )
    }
}

impl fmt::Display for FederationSetupProposalReview {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "federation setup proposal v{}", self.version)?;
        writeln!(formatter, "setup_id: {}", self.setup_id)?;
        writeln!(
            formatter,
            "proposed_federation_id: {}",
            self.proposed_federation_id
        )?;
        writeln!(formatter, "proposal_hash: {}", self.proposal_hash)?;
        writeln!(formatter, "network: {}", network_label(self.network))?;
        writeln!(
            formatter,
            "protocols: setup={} bls_dkg={} federation_rest={} wallet={} bls_keyset={} frost_dkg={} frost_ciphersuite={}",
            self.required_versions.setup,
            self.required_versions.bls_dkg,
            self.required_versions.federation_rest,
            self.required_versions.wallet,
            self.required_versions.bls_keyset,
            self.required_versions.frost_dkg,
            self.required_versions.frost_ciphersuite,
        )?;
        writeln!(
            formatter,
            "thresholds: members={} signatures={} consensus={}",
            self.threshold_params.member_count,
            self.threshold_params.signature_threshold,
            self.threshold_params.consensus_threshold,
        )?;
        writeln!(
            formatter,
            "payment_observations: mint={} melt={}",
            payment_policy_label(self.payment_observation_policy.mint_quotes),
            payment_policy_label(self.payment_observation_policy.melt_quotes),
        )?;
        writeln!(
            formatter,
            "checkpoints: journal_operations={} finalized_items={} canonical_bytes={}",
            self.checkpoint_policy
                .checkpoint_interval_journal_operations,
            self.checkpoint_policy.checkpoint_interval_finalized_items,
            self.checkpoint_policy.checkpoint_interval_canonical_bytes,
        )?;
        writeln!(formatter, "members:")?;
        for member in &self.members {
            writeln!(
                formatter,
                "  - member={} mint={} federation={} identity={}",
                member.member_id,
                member.public_mint_url,
                member.federation_api_url,
                member.identity_fingerprint,
            )?;
        }
        writeln!(formatter, "keysets:")?;
        for keyset in &self.keysets {
            let amounts = keyset
                .amounts
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let expiry = keyset
                .final_expiry
                .map_or_else(|| "none".to_string(), |expiry| expiry.to_string());
            writeln!(
                formatter,
                "  - unit={} active={} input_fee_ppk={} final_expiry={} amounts={}",
                keyset.unit, keyset.active, keyset.input_fee_ppk, expiry, amounts,
            )?;
        }
        writeln!(
            formatter,
            "frost_root_key_epoch: {}",
            self.frost.root_key_epoch
        )?;
        writeln!(formatter, "frost_applications:")?;
        for application in &self.frost.applications {
            match application {
                FederationSetupFrostApplication::CdkOnchain {
                    application_key_epoch,
                    confirmation_policy_version,
                    minimum_confirmations,
                    fee_policy_version,
                } => writeln!(
                    formatter,
                    "  - cdk_onchain application_key_epoch={} confirmation_policy_version={} minimum_confirmations={} fee_policy_version={}",
                    application_key_epoch,
                    confirmation_policy_version,
                    minimum_confirmations,
                    fee_policy_version,
                )?,
                FederationSetupFrostApplication::Bark {
                    application_key_epoch,
                    network,
                    ark_server_public_key,
                    policy,
                } => writeln!(
                    formatter,
                    "  - bark application_key_epoch={} network={} ark_identity={} policy_version={}",
                    application_key_epoch,
                    network_label(*network),
                    identity_fingerprint(ark_server_public_key),
                    policy.version,
                )?,
            }
        }
        Ok(())
    }
}

fn identity_fingerprint(key: &crate::nuts::nut01::PublicKey) -> String {
    let hash = sha256::Hash::hash(&key.to_bytes()).to_string();
    hash[..16].to_string()
}

const fn network_label(network: FederationSetupBitcoinNetwork) -> &'static str {
    match network {
        FederationSetupBitcoinNetwork::Bitcoin => "bitcoin",
        FederationSetupBitcoinNetwork::Testnet => "testnet",
        FederationSetupBitcoinNetwork::Signet => "signet",
        FederationSetupBitcoinNetwork::Regtest => "regtest",
    }
}

fn payment_policy_label(policy: FederationPaymentVerificationPolicy) -> String {
    match policy {
        FederationPaymentVerificationPolicy::DevelopmentSingleObservation => {
            "development_single_observation".to_string()
        }
        FederationPaymentVerificationPolicy::MemberQuorum { required } => {
            format!("member_quorum({required})")
        }
    }
}
