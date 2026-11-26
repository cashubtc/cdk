//! Immutable setup records and the local session steps that store them.
//!
//! A setup database stores each immutable record exactly once
//! ([`FederationSetupRecord`]). A local session step stores one record and
//! advances the local setup session in the same transaction; which record may
//! accompany which transition is decided here ([`FederationSetupStep`]), not
//! in the storage backend.

use super::{
    FederationSetupActivation, FederationSetupApproval, FederationSetupCoordinatorPhase,
    FederationSetupEnrollment, FederationSetupInvitationRecord, FederationSetupInvitationState,
    FederationSetupManifest, FederationSetupParticipantPhase, FederationSetupProposal,
    FederationSetupSession, FederationSetupUrlPolicy, Result,
};
use crate::federation::FederationError;

/// One immutable setup record, stored exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationSetupRecord<'a> {
    /// One signed enrollment, once per setup member and invitation.
    Enrollment(&'a FederationSetupEnrollment),
    /// The one immutable coordinator-signed proposal.
    Proposal(&'a FederationSetupProposal),
    /// The participant-local one-proposal approval fence.
    ApprovalFence(&'a FederationSetupApproval),
    /// One approval received by the coordinator.
    Approval(&'a FederationSetupApproval),
    /// The one complete certified manifest.
    Manifest(&'a FederationSetupManifest),
    /// One member's final config activation reference.
    Activation(&'a FederationSetupActivation),
}

/// One local setup-session step: a record stored together with the session
/// transition it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FederationSetupStep<'a> {
    /// The local signed enrollment, advancing from identity created to
    /// enrolled.
    Enroll(&'a FederationSetupEnrollment),
    /// The coordinator's proposal, advancing the coordinator from collecting
    /// enrollments to proposal published.
    PublishProposal(&'a FederationSetupProposal),
    /// The exact proposal this member reviewed, advancing from enrolled to
    /// proposal reviewed.
    ReviewProposal(&'a FederationSetupProposal),
    /// The local approval fence for `proposal`, advancing to approved.
    FenceApproval {
        /// The approved proposal.
        proposal: &'a FederationSetupProposal,
        /// This member's approval of it.
        approval: &'a FederationSetupApproval,
    },
    /// The certified manifest, advancing the coordinator or participant to
    /// certified.
    Certify(&'a FederationSetupManifest),
    /// The final config reference, advancing this member to activated.
    Activate(&'a FederationSetupActivation),
}

fn mismatch(reason: &'static str) -> FederationError {
    FederationError::InvalidSetup { reason }
}

impl<'a> FederationSetupStep<'a> {
    /// The record this step stores.
    #[must_use]
    pub const fn record(&self) -> FederationSetupRecord<'a> {
        match *self {
            Self::Enroll(enrollment) => FederationSetupRecord::Enrollment(enrollment),
            Self::PublishProposal(proposal) | Self::ReviewProposal(proposal) => {
                FederationSetupRecord::Proposal(proposal)
            }
            Self::FenceApproval { approval, .. } => FederationSetupRecord::ApprovalFence(approval),
            Self::Certify(manifest) => FederationSetupRecord::Manifest(manifest),
            Self::Activate(activation) => FederationSetupRecord::Activation(activation),
        }
    }

    /// Check that the session transition from `expected` to `updated` is
    /// legal and is the one this step's record belongs with.
    pub fn validate_transition(
        &self,
        expected: &FederationSetupSession,
        updated: &FederationSetupSession,
    ) -> Result<()> {
        expected.validate_transition_to(updated)?;
        match *self {
            Self::Enroll(enrollment) => {
                enrollment.validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
                if expected.participant_phase != FederationSetupParticipantPhase::IdentityCreated
                    || updated.participant_phase != FederationSetupParticipantPhase::Enrolled
                    || expected.setup_id != enrollment.setup_id
                    || expected.local_role.member_id() != enrollment.member_id
                {
                    return Err(mismatch(
                        "setup enrollment does not match the local enrollment transition",
                    ));
                }
            }
            Self::PublishProposal(proposal) => {
                proposal.validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
                if expected.setup_id != proposal.body.setup_id
                    || expected.coordinator_phase
                        != Some(FederationSetupCoordinatorPhase::CollectingEnrollments)
                    || updated.coordinator_phase
                        != Some(FederationSetupCoordinatorPhase::ProposalPublished)
                    || expected.participant_phase != FederationSetupParticipantPhase::Enrolled
                    || updated.participant_phase != FederationSetupParticipantPhase::Enrolled
                {
                    return Err(mismatch(
                        "setup proposal does not match the coordinator publication transition",
                    ));
                }
            }
            Self::ReviewProposal(proposal) => {
                proposal.validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
                if expected.participant_phase != FederationSetupParticipantPhase::Enrolled
                    || updated.participant_phase
                        != FederationSetupParticipantPhase::ProposalReviewed
                    || expected.setup_id != proposal.body.setup_id
                    || !proposal
                        .body
                        .enrollments
                        .contains_key(&expected.local_role.member_id())
                {
                    return Err(mismatch(
                        "setup proposal does not match the local review transition",
                    ));
                }
            }
            Self::FenceApproval { proposal, approval } => {
                proposal.validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
                approval.validate(proposal)?;
                if updated.participant_phase != FederationSetupParticipantPhase::Approved
                    || expected.setup_id != proposal.body.setup_id
                    || expected.local_role.member_id() != approval.member_id
                {
                    return Err(mismatch(
                        "setup approval fence does not match local session",
                    ));
                }
            }
            Self::Certify(manifest) => {
                manifest.validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
                let coordinator_certified = expected.coordinator_phase
                    == Some(FederationSetupCoordinatorPhase::ProposalPublished)
                    && updated.coordinator_phase
                        == Some(FederationSetupCoordinatorPhase::Certified);
                let participant_certified = expected.participant_phase
                    == FederationSetupParticipantPhase::Approved
                    && updated.participant_phase == FederationSetupParticipantPhase::Certified;
                if expected.setup_id != manifest.proposal.body.setup_id
                    || (!coordinator_certified && !participant_certified)
                {
                    return Err(mismatch(
                        "setup manifest does not match a certification transition",
                    ));
                }
            }
            Self::Activate(activation) => {
                activation.validate()?;
                if updated.participant_phase != FederationSetupParticipantPhase::Activated
                    || expected.setup_id != activation.setup_id
                    || expected.local_role.member_id() != activation.member_id
                {
                    return Err(mismatch("setup activation does not match local session"));
                }
            }
        }
        Ok(())
    }
}

/// Check that `enrollment` may be stored while `expected` becomes `consumed`.
pub fn validate_federation_setup_invitation_consumption(
    expected: &FederationSetupInvitationRecord,
    consumed: &FederationSetupInvitationRecord,
    enrollment: &FederationSetupEnrollment,
) -> Result<()> {
    expected.validate_transition_to(consumed)?;
    enrollment.validate(FederationSetupUrlPolicy::AllowInsecureLocalhost)?;
    if expected.setup_id != enrollment.setup_id
        || expected.invitation_id != enrollment.invitation_id
        || expected.member_id != enrollment.member_id
        || consumed.state != FederationSetupInvitationState::Consumed
    {
        return Err(mismatch(
            "setup enrollment does not match consumed invitation",
        ));
    }
    Ok(())
}
