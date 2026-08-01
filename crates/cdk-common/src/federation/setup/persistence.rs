//! Pure durable-record and lifecycle types for federation setup.

use serde::{Deserialize, Serialize};

use super::{
    invalid, validate_nonzero_identifier, validate_setup_version, FederationSetupId,
    FederationSetupInvitation, FederationSetupInvitationCapabilityHash,
    FederationSetupInvitationId, FederationSetupManifestHash, MemberId, Result,
    FEDERATION_SETUP_PROTOCOL_VERSION,
};
use crate::federation::{FederationConfigDigest, FederationId, FederationSetupAuthorizationHash};

fn validate_sql_timestamp(label: &'static str, timestamp: u64) -> Result<()> {
    if timestamp > i64::MAX as u64 {
        return Err(invalid(format!(
            "{label} exceeds the portable setup persistence range"
        )));
    }
    Ok(())
}

/// Local role played by this database in one setup attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum FederationSetupLocalRole {
    /// This member also hosts the bounded setup coordinator.
    Coordinator {
        /// Local member slot in the proposed federation.
        member_id: MemberId,
    },
    /// This member joins a coordinator hosted elsewhere.
    Member {
        /// Local member slot assigned by its invitation.
        member_id: MemberId,
    },
}

impl FederationSetupLocalRole {
    /// Return this operator's member slot.
    #[must_use]
    pub const fn member_id(self) -> MemberId {
        match self {
            Self::Coordinator { member_id } | Self::Member { member_id } => member_id,
        }
    }

    const fn is_coordinator(self) -> bool {
        matches!(self, Self::Coordinator { .. })
    }
}

/// Coordinator-side aggregate setup lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupCoordinatorPhase {
    /// Invitations and signed enrollments may be collected.
    CollectingEnrollments,
    /// The immutable coordinator-signed proposal has been published.
    ProposalPublished,
    /// Every configured member approved and the manifest was certified.
    Certified,
    /// Manifest distribution is complete and the coordinator is retired.
    Closed,
    /// The setup was explicitly aborted.
    Aborted,
}

impl FederationSetupCoordinatorPhase {
    fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (
                    Self::CollectingEnrollments,
                    Self::ProposalPublished | Self::Aborted
                ) | (Self::ProposalPublished, Self::Certified | Self::Aborted)
                    | (Self::Certified, Self::Closed | Self::Aborted)
            )
    }
}

/// Participant-local setup and activation lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupParticipantPhase {
    /// A protected local identity has been generated and persisted.
    IdentityCreated,
    /// The signed enrollment has been accepted.
    Enrolled,
    /// The exact immutable proposal has been durably reviewed.
    ProposalReviewed,
    /// The one-proposal approval fence has been persisted.
    Approved,
    /// The complete certified manifest has been validated and persisted.
    Certified,
    /// Manifest-bound BLS/FROST DKG is running.
    DkgRunning,
    /// DKG and final-config confirmation are complete.
    ReadyToActivate,
    /// The final config reference is active locally.
    Activated,
    /// The local setup was explicitly aborted.
    Aborted,
}

impl FederationSetupParticipantPhase {
    fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::IdentityCreated, Self::Enrolled | Self::Aborted)
                    | (Self::Enrolled, Self::ProposalReviewed | Self::Aborted)
                    | (Self::ProposalReviewed, Self::Approved | Self::Aborted)
                    | (Self::Approved, Self::Certified | Self::Aborted)
                    | (Self::Certified, Self::DkgRunning | Self::Aborted)
                    | (Self::DkgRunning, Self::ReadyToActivate | Self::Aborted)
                    | (Self::ReadyToActivate, Self::Activated | Self::Aborted)
            )
    }
}

/// Local durable state for one setup attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupSession {
    /// Strict setup record version.
    pub version: u16,
    /// Setup attempt being resumed.
    pub setup_id: FederationSetupId,
    /// Local role and member slot.
    pub local_role: FederationSetupLocalRole,
    /// Coordinator lifecycle, present only for the creator/coordinator member.
    pub coordinator_phase: Option<FederationSetupCoordinatorPhase>,
    /// Participant-local lifecycle for this member.
    pub participant_phase: FederationSetupParticipantPhase,
    /// Creation time in Unix seconds.
    pub created_at_unix: u64,
    /// Last durable transition time in Unix seconds.
    pub updated_at_unix: u64,
}

impl FederationSetupSession {
    /// Construct initial local state for a coordinator or joining member.
    pub fn new(
        setup_id: FederationSetupId,
        local_role: FederationSetupLocalRole,
        created_at_unix: u64,
    ) -> Result<Self> {
        let session = Self {
            version: FEDERATION_SETUP_PROTOCOL_VERSION,
            setup_id,
            local_role,
            coordinator_phase: local_role
                .is_coordinator()
                .then_some(FederationSetupCoordinatorPhase::CollectingEnrollments),
            participant_phase: FederationSetupParticipantPhase::IdentityCreated,
            created_at_unix,
            updated_at_unix: created_at_unix,
        };
        session.validate()?;
        Ok(session)
    }

    /// Validate the persisted session shape.
    pub fn validate(&self) -> Result<()> {
        validate_setup_version("setup session", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        validate_sql_timestamp("setup creation time", self.created_at_unix)?;
        validate_sql_timestamp("setup update time", self.updated_at_unix)?;
        if self.created_at_unix > self.updated_at_unix {
            return Err(invalid("setup session update predates creation"));
        }
        if self.local_role.is_coordinator() != self.coordinator_phase.is_some() {
            return Err(invalid(
                "setup coordinator phase does not match the local role",
            ));
        }
        Ok(())
    }

    /// Return an updated session after one legal coordinator phase transition.
    pub fn with_coordinator_phase(
        &self,
        next: FederationSetupCoordinatorPhase,
        updated_at_unix: u64,
    ) -> Result<Self> {
        let current = self
            .coordinator_phase
            .ok_or_else(|| invalid("member-only setup has no coordinator lifecycle"))?;
        if !current.can_transition_to(next) {
            return Err(invalid("illegal setup coordinator phase transition"));
        }
        let mut updated = self.clone();
        updated.coordinator_phase = Some(next);
        updated.updated_at_unix = updated_at_unix;
        self.validate_transition_to(&updated)?;
        Ok(updated)
    }

    /// Return an updated session after one legal participant phase transition.
    pub fn with_participant_phase(
        &self,
        next: FederationSetupParticipantPhase,
        updated_at_unix: u64,
    ) -> Result<Self> {
        if !self.participant_phase.can_transition_to(next) {
            return Err(invalid("illegal setup participant phase transition"));
        }
        let mut updated = self.clone();
        updated.participant_phase = next;
        updated.updated_at_unix = updated_at_unix;
        self.validate_transition_to(&updated)?;
        Ok(updated)
    }

    /// Validate an exact compare-and-set transition from this record.
    pub fn validate_transition_to(&self, updated: &Self) -> Result<()> {
        self.validate()?;
        updated.validate()?;
        if self.version != updated.version
            || self.setup_id != updated.setup_id
            || self.local_role != updated.local_role
            || self.created_at_unix != updated.created_at_unix
            || updated.updated_at_unix < self.updated_at_unix
        {
            return Err(invalid("setup session transition changes immutable fields"));
        }
        let coordinator_changed = self.coordinator_phase != updated.coordinator_phase;
        let participant_changed = self.participant_phase != updated.participant_phase;
        if coordinator_changed && participant_changed {
            return Err(invalid(
                "one setup session transition cannot advance both lifecycles",
            ));
        }
        if !coordinator_changed && !participant_changed {
            if self == updated {
                return Ok(());
            }
            return Err(invalid(
                "setup session timestamp changed without a phase transition",
            ));
        }
        if coordinator_changed
            && !self
                .coordinator_phase
                .zip(updated.coordinator_phase)
                .is_some_and(|(current, next)| current.can_transition_to(next))
        {
            return Err(invalid("illegal setup coordinator phase transition"));
        }
        if participant_changed
            && !self
                .participant_phase
                .can_transition_to(updated.participant_phase)
        {
            return Err(invalid("illegal setup participant phase transition"));
        }
        Ok(())
    }
}

/// Durable public invitation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FederationSetupInvitationState {
    /// The capability may enroll its assigned slot once.
    Available,
    /// A signed enrollment consumed this invitation.
    Consumed,
    /// The invitation expired before consumption.
    Expired,
}

/// Secret-free durable record for one invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupInvitationRecord {
    /// Strict setup record version.
    pub version: u16,
    /// Setup attempt containing the slot.
    pub setup_id: FederationSetupId,
    /// Public invitation identifier.
    pub invitation_id: FederationSetupInvitationId,
    /// Assigned member slot.
    pub member_id: MemberId,
    /// Absolute invitation expiry.
    pub expires_at_unix: u64,
    /// Context-bound hash of the high-entropy capability.
    pub capability_hash: FederationSetupInvitationCapabilityHash,
    /// Current one-way invitation state.
    pub state: FederationSetupInvitationState,
}

impl FederationSetupInvitationRecord {
    /// Derive the secret-free durable record from a validated invitation.
    pub fn from_invitation(invitation: &FederationSetupInvitation, now_unix: u64) -> Result<Self> {
        invitation.validate(
            now_unix,
            super::FederationSetupUrlPolicy::AllowInsecureLocalhost,
        )?;
        let record = Self {
            version: invitation.version,
            setup_id: invitation.setup_id,
            invitation_id: invitation.invitation_id,
            member_id: invitation.member_id,
            expires_at_unix: invitation.expires_at_unix,
            capability_hash: invitation.capability_hash(),
            state: FederationSetupInvitationState::Available,
        };
        record.validate()?;
        Ok(record)
    }

    /// Validate a persisted invitation record.
    pub fn validate(&self) -> Result<()> {
        validate_setup_version("invitation record", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        validate_nonzero_identifier("invitation ID", self.invitation_id.to_bytes())?;
        validate_nonzero_identifier("capability hash", self.capability_hash.to_bytes())?;
        validate_sql_timestamp("invitation expiry", self.expires_at_unix)
    }

    /// Return the exact consumed state for this record.
    pub fn consumed(&self) -> Result<Self> {
        self.with_state(FederationSetupInvitationState::Consumed)
    }

    /// Return the exact expired state for this record.
    pub fn expired(&self) -> Result<Self> {
        self.with_state(FederationSetupInvitationState::Expired)
    }

    /// Validate a fresh capability replacing this still-unconsumed member slot.
    pub fn validate_replacement(&self, replacement: &Self) -> Result<()> {
        self.validate()?;
        replacement.validate()?;
        if self.state != FederationSetupInvitationState::Available
            || replacement.state != FederationSetupInvitationState::Available
            || self.version != replacement.version
            || self.setup_id != replacement.setup_id
            || self.member_id != replacement.member_id
            || self.invitation_id == replacement.invitation_id
            || self.capability_hash == replacement.capability_hash
            || replacement.expires_at_unix <= self.expires_at_unix
        {
            return Err(invalid(
                "replacement invitation must be a fresh capability for the same available slot",
            ));
        }
        Ok(())
    }

    fn with_state(&self, state: FederationSetupInvitationState) -> Result<Self> {
        let mut updated = self.clone();
        updated.state = state;
        self.validate_transition_to(&updated)?;
        Ok(updated)
    }

    /// Validate an exact compare-and-set invitation transition.
    pub fn validate_transition_to(&self, updated: &Self) -> Result<()> {
        self.validate()?;
        updated.validate()?;
        if self.version != updated.version
            || self.setup_id != updated.setup_id
            || self.invitation_id != updated.invitation_id
            || self.member_id != updated.member_id
            || self.expires_at_unix != updated.expires_at_unix
            || self.capability_hash != updated.capability_hash
        {
            return Err(invalid(
                "setup invitation transition changes immutable fields",
            ));
        }
        if self.state == updated.state {
            return Ok(());
        }
        if self.state != FederationSetupInvitationState::Available
            || !matches!(
                updated.state,
                FederationSetupInvitationState::Consumed | FederationSetupInvitationState::Expired
            )
        {
            return Err(invalid("illegal setup invitation state transition"));
        }
        Ok(())
    }
}

/// Durable local activation reference with no filesystem path or secret data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationSetupActivation {
    /// Strict setup record version.
    pub version: u16,
    /// Setup attempt activated by this member.
    pub setup_id: FederationSetupId,
    /// Locally activated member.
    pub member_id: MemberId,
    /// Certified manifest authorizing DKG.
    pub manifest_hash: FederationSetupManifestHash,
    /// Setup authorization preserved in the final config.
    pub setup_authorization: FederationSetupAuthorizationHash,
    /// Final federation identifier.
    pub federation_id: FederationId,
    /// Digest serving as the stable final-config reference.
    pub final_config_digest: FederationConfigDigest,
    /// Local atomic activation time in Unix seconds.
    pub activated_at_unix: u64,
}

impl FederationSetupActivation {
    /// Validate identifier and manifest bindings.
    pub fn validate(&self) -> Result<()> {
        validate_setup_version("setup activation", self.version)?;
        validate_nonzero_identifier("setup ID", self.setup_id.to_bytes())?;
        validate_nonzero_identifier("manifest hash", self.manifest_hash.to_bytes())?;
        validate_nonzero_identifier("setup authorization", self.setup_authorization.to_bytes())?;
        validate_nonzero_identifier("federation ID", self.federation_id.to_bytes())?;
        validate_nonzero_identifier("final config digest", self.final_config_digest.to_bytes())?;
        validate_sql_timestamp("activation time", self.activated_at_unix)
    }
}
