use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::Arc;

use crate::database::mint::Database;
use crate::database::Error;
use crate::federation::{
    FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationError,
    FederationPaymentObservationPolicy, FederationSetupActivation, FederationSetupApproval,
    FederationSetupBitcoinNetwork, FederationSetupCoordinatorPhase,
    FederationSetupCoordinatorPublicKey, FederationSetupEnrollment,
    FederationSetupFrostApplication, FederationSetupFrostPolicy, FederationSetupId,
    FederationSetupInvitation, FederationSetupInvitationCapability, FederationSetupInvitationId,
    FederationSetupInvitationRecord, FederationSetupLocalRole, FederationSetupManifest,
    FederationSetupParticipantPhase, FederationSetupProposal, FederationSetupProposalBody,
    FederationSetupProtocolVersions, FederationSetupSession, FederationSetupUrlPolicy, MemberId,
    ThresholdParams, FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
    FEDERATION_SETUP_PROTOCOL_VERSION,
};
use crate::mint_url::MintUrl;
use crate::nuts::nut01::SecretKey;
use crate::{Amount, CurrencyUnit};

const NOW_UNIX: u64 = 1_900_000_000;
const URL_POLICY: FederationSetupUrlPolicy = FederationSetupUrlPolicy::RequireHttps;

struct SetupFixture {
    session: FederationSetupSession,
    invitation: FederationSetupInvitationRecord,
    enrollment: FederationSetupEnrollment,
    proposal: FederationSetupProposal,
    approvals: BTreeMap<MemberId, FederationSetupApproval>,
    manifest: FederationSetupManifest,
    activation: FederationSetupActivation,
}

/// Exercise exact replay, conflicts, lifecycle CAS, and every setup artifact.
pub async fn federation_setup_persistence<DB>(db: DB)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let fixture = setup_fixture(31, 0);
    let setup_id = fixture.session.setup_id;

    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("setup transaction");
    tx.add_federation_setup_session(&fixture.session)
        .await
        .expect("insert setup session");
    tx.add_federation_setup_session(&fixture.session)
        .await
        .expect("exact session replay");
    tx.add_federation_setup_invitation(&fixture.invitation)
        .await
        .expect("insert invitation");
    tx.add_federation_setup_invitation(&fixture.invitation)
        .await
        .expect("exact invitation replay");
    tx.commit().await.expect("commit initial setup state");

    let mut conflicting_session = fixture.session.clone();
    conflicting_session.local_role = FederationSetupLocalRole::Coordinator {
        member_id: member_id(2),
    };
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move { tx.add_federation_setup_session(&conflicting_session).await })
    })
    .await;
    let mut conflicting_invitation = fixture.invitation.clone();
    conflicting_invitation.expires_at_unix += 1;
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move {
            tx.add_federation_setup_invitation(&conflicting_invitation)
                .await
        })
    })
    .await;

    let serialized_invitation = serde_json::to_string(&fixture.invitation)
        .expect("secret-free invitation record serializes");
    assert!(!serialized_invitation.contains(&"ab".repeat(32)));
    assert!(!format!("{:?}", fixture.invitation).contains(&"ab".repeat(32)));

    let consumed = fixture.invitation.consumed().expect("consumed state");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("consume transaction");
    tx.consume_federation_setup_invitation(&fixture.invitation, &consumed, &fixture.enrollment)
        .await
        .expect("consume invitation");
    tx.consume_federation_setup_invitation(&fixture.invitation, &consumed, &fixture.enrollment)
        .await
        .expect("exact consumption replay");
    tx.commit().await.expect("commit invitation consumption");

    assert_eq!(
        db.get_federation_setup_invitation(setup_id, fixture.invitation.invitation_id)
            .await
            .expect("stored invitation"),
        Some(consumed)
    );
    assert_eq!(
        db.get_federation_setup_enrollments(setup_id)
            .await
            .expect("stored enrollment"),
        vec![fixture.enrollment.clone()]
    );
    let conflicting_enrollment = signed_enrollment(31, 1, 1);
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move {
            tx.add_federation_setup_enrollment(&conflicting_enrollment)
                .await
        })
    })
    .await;

    let enrolled = fixture
        .session
        .with_participant_phase(FederationSetupParticipantPhase::Enrolled, NOW_UNIX + 1)
        .expect("enrolled transition");
    transition_session(&db, &fixture.session, &enrolled).await;
    let proposal_published = enrolled
        .with_coordinator_phase(
            FederationSetupCoordinatorPhase::ProposalPublished,
            NOW_UNIX + 2,
        )
        .expect("proposal transition");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("proposal transaction");
    tx.publish_federation_setup_proposal(&enrolled, &proposal_published, &fixture.proposal)
        .await
        .expect("publish proposal");
    tx.publish_federation_setup_proposal(&enrolled, &proposal_published, &fixture.proposal)
        .await
        .expect("exact proposal replay");
    tx.commit().await.expect("commit proposal");

    let reviewed = proposal_published
        .with_participant_phase(
            FederationSetupParticipantPhase::ProposalReviewed,
            NOW_UNIX + 3,
        )
        .expect("review transition");
    transition_session(&db, &proposal_published, &reviewed).await;
    let approved = reviewed
        .with_participant_phase(FederationSetupParticipantPhase::Approved, NOW_UNIX + 4)
        .expect("approval transition");
    let local_approval = fixture
        .approvals
        .get(&member_id(1))
        .expect("local approval");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("approval transaction");
    tx.fence_federation_setup_approval(&reviewed, &approved, &fixture.proposal, local_approval)
        .await
        .expect("persist approval fence");
    tx.fence_federation_setup_approval(&reviewed, &approved, &fixture.proposal, local_approval)
        .await
        .expect("exact approval fence replay");
    for approval in fixture.approvals.values() {
        tx.add_federation_setup_approval(approval)
            .await
            .expect("persist received approval");
        tx.add_federation_setup_approval(approval)
            .await
            .expect("exact received approval replay");
    }
    tx.commit().await.expect("commit approvals");

    let coordinator_certified = approved
        .with_coordinator_phase(FederationSetupCoordinatorPhase::Certified, NOW_UNIX + 5)
        .expect("coordinator certification transition");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("manifest transaction");
    tx.store_federation_setup_manifest(&approved, &coordinator_certified, &fixture.manifest)
        .await
        .expect("store certified manifest");
    tx.store_federation_setup_manifest(&approved, &coordinator_certified, &fixture.manifest)
        .await
        .expect("exact manifest replay");
    tx.commit().await.expect("commit manifest");

    let participant_certified = coordinator_certified
        .with_participant_phase(FederationSetupParticipantPhase::Certified, NOW_UNIX + 6)
        .expect("participant certification transition");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("manifest receipt transaction");
    tx.store_federation_setup_manifest(
        &coordinator_certified,
        &participant_certified,
        &fixture.manifest,
    )
    .await
    .expect("record manifest receipt");
    tx.commit().await.expect("commit manifest receipt");

    let dkg_running = participant_certified
        .with_participant_phase(FederationSetupParticipantPhase::DkgRunning, NOW_UNIX + 7)
        .expect("DKG transition");
    transition_session(&db, &participant_certified, &dkg_running).await;
    let ready = dkg_running
        .with_participant_phase(
            FederationSetupParticipantPhase::ReadyToActivate,
            NOW_UNIX + 8,
        )
        .expect("ready transition");
    transition_session(&db, &dkg_running, &ready).await;
    let activated = ready
        .with_participant_phase(FederationSetupParticipantPhase::Activated, NOW_UNIX + 9)
        .expect("activation transition");
    let mut tx = Database::begin_transaction(&db)
        .await
        .expect("activation transaction");
    tx.activate_federation_setup(&ready, &activated, &fixture.activation)
        .await
        .expect("activate setup");
    tx.activate_federation_setup(&ready, &activated, &fixture.activation)
        .await
        .expect("exact activation replay");
    tx.commit().await.expect("commit activation");

    assert_eq!(
        db.get_federation_setup_session(setup_id)
            .await
            .expect("final session"),
        Some(activated)
    );
    assert_eq!(
        db.get_federation_setup_invitations(setup_id)
            .await
            .expect("invitation list"),
        vec![fixture.invitation.consumed().expect("consumed invitation")]
    );
    assert_eq!(
        db.get_federation_setup_proposal(setup_id)
            .await
            .expect("proposal"),
        Some(fixture.proposal.clone())
    );
    assert_eq!(
        db.get_federation_setup_approval_fence(setup_id, member_id(1))
            .await
            .expect("approval fence"),
        Some(local_approval.clone())
    );
    assert_eq!(
        db.get_federation_setup_approvals(setup_id)
            .await
            .expect("approval list"),
        fixture.approvals.values().cloned().collect::<Vec<_>>()
    );
    assert_eq!(
        db.get_federation_setup_manifest(setup_id)
            .await
            .expect("manifest"),
        Some(fixture.manifest.clone())
    );
    assert_eq!(
        db.get_federation_setup_activations(setup_id)
            .await
            .expect("activation list"),
        vec![fixture.activation.clone()]
    );

    let alternate = setup_fixture(31, 1);
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move { tx.add_federation_setup_proposal(&alternate.proposal).await })
    })
    .await;
    let alternate_approval = alternate
        .approvals
        .get(&member_id(1))
        .expect("alternate approval")
        .clone();
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move {
            tx.add_federation_setup_approval_fence(&alternate_approval)
                .await
        })
    })
    .await;
    let alternate_received_approval = alternate
        .approvals
        .get(&member_id(2))
        .expect("alternate received approval")
        .clone();
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move {
            tx.add_federation_setup_approval(&alternate_received_approval)
                .await
        })
    })
    .await;
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move { tx.add_federation_setup_manifest(&alternate.manifest).await })
    })
    .await;
    let mut alternate_activation = fixture.activation;
    alternate_activation.final_config_digest =
        crate::federation::FederationConfigDigest::from_bytes([99; 32]);
    assert_conflicting_write(&db, |tx| {
        Box::pin(async move {
            tx.add_federation_setup_activation(&alternate_activation)
                .await
        })
    })
    .await;
}

/// Prove rollback at invitation, proposal, approval-fence, manifest, and activation boundaries.
pub async fn federation_setup_transaction_rollbacks<DB>(db: DB)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    rollback_invitation_consumption(&db, setup_fixture(41, 0)).await;
    rollback_proposal_publication(&db, setup_fixture(42, 0)).await;
    rollback_approval_fence(&db, setup_fixture(43, 0)).await;
    rollback_manifest_storage(&db, setup_fixture(44, 0)).await;
    rollback_activation(&db, setup_fixture(45, 0)).await;
}

/// Prove only one conflicting invitation consumption and approval fence can win.
pub async fn federation_setup_concurrent_races<DB>(db: DB)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let db = Arc::new(db);
    let fixture = setup_fixture(51, 0);
    persist_session_and_invitation(db.as_ref(), &fixture).await;
    let consumed = fixture.invitation.consumed().expect("consumed invitation");
    let conflicting_enrollment = signed_enrollment(51, 1, 1);

    let first = consume_and_commit(
        Arc::clone(&db),
        fixture.invitation.clone(),
        consumed.clone(),
        fixture.enrollment.clone(),
    );
    let second = consume_and_commit(
        Arc::clone(&db),
        fixture.invitation.clone(),
        consumed,
        conflicting_enrollment,
    );
    let (first, second) = tokio::join!(first, second);
    assert_ne!(
        first, second,
        "exactly one conflicting enrollment must commit"
    );
    assert_eq!(
        db.as_ref()
            .get_federation_setup_enrollments(fixture.session.setup_id)
            .await
            .expect("winning enrollment")
            .len(),
        1
    );

    let fence_fixture = setup_fixture(52, 0);
    let reviewed = advance_to_reviewed(db.as_ref(), &fence_fixture.session).await;
    let approved = reviewed
        .with_participant_phase(FederationSetupParticipantPhase::Approved, NOW_UNIX + 4)
        .expect("approved phase");
    let alternate = setup_fixture(52, 1);
    let first_approval = fence_fixture
        .approvals
        .get(&member_id(1))
        .expect("first approval")
        .clone();
    let second_approval = alternate
        .approvals
        .get(&member_id(1))
        .expect("second approval")
        .clone();
    let first = fence_and_commit(
        Arc::clone(&db),
        reviewed.clone(),
        approved.clone(),
        fence_fixture.proposal,
        first_approval,
    );
    let second = fence_and_commit(
        Arc::clone(&db),
        reviewed,
        approved,
        alternate.proposal,
        second_approval,
    );
    let (first, second) = tokio::join!(first, second);
    assert_ne!(
        first, second,
        "exactly one conflicting approval fence must commit"
    );
    assert!(db
        .as_ref()
        .get_federation_setup_approval_fence(fence_fixture.session.setup_id, member_id(1))
        .await
        .expect("winning approval fence")
        .is_some());
}

async fn rollback_invitation_consumption<DB>(db: &DB, fixture: SetupFixture)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    persist_session_and_invitation(db, &fixture).await;
    let expired = fixture.invitation.expired().expect("expired invitation");
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("expire transaction");
    tx.transition_federation_setup_invitation(&fixture.invitation, &expired)
        .await
        .expect("expire invitation");
    tx.commit().await.expect("commit expiry");

    let mut tx = Database::begin_transaction(db)
        .await
        .expect("failing consume transaction");
    let result = tx
        .consume_federation_setup_invitation(
            &fixture.invitation,
            &fixture.invitation.consumed().expect("consumed state"),
            &fixture.enrollment,
        )
        .await;
    assert_setup_conflict(result.expect_err("stale invitation CAS must fail"));
    tx.rollback().await.expect("rollback enrollment insert");
    assert!(db
        .get_federation_setup_enrollments(fixture.session.setup_id)
        .await
        .expect("enrollments after rollback")
        .is_empty());
}

async fn rollback_proposal_publication<DB>(db: &DB, fixture: SetupFixture)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    persist_session(db, &fixture.session).await;
    let enrolled = fixture
        .session
        .with_participant_phase(FederationSetupParticipantPhase::Enrolled, NOW_UNIX + 1)
        .expect("enrolled phase");
    transition_session(db, &fixture.session, &enrolled).await;
    let aborted = enrolled
        .with_coordinator_phase(FederationSetupCoordinatorPhase::Aborted, NOW_UNIX + 2)
        .expect("aborted phase");
    transition_session(db, &enrolled, &aborted).await;
    let published = enrolled
        .with_coordinator_phase(
            FederationSetupCoordinatorPhase::ProposalPublished,
            NOW_UNIX + 2,
        )
        .expect("published phase");
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("failing proposal transaction");
    let result = tx
        .publish_federation_setup_proposal(&enrolled, &published, &fixture.proposal)
        .await;
    assert_setup_conflict(result.expect_err("stale session CAS must fail"));
    tx.rollback().await.expect("rollback proposal insert");
    assert!(db
        .get_federation_setup_proposal(fixture.session.setup_id)
        .await
        .expect("proposal after rollback")
        .is_none());
}

async fn rollback_approval_fence<DB>(db: &DB, fixture: SetupFixture)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let reviewed = advance_to_reviewed(db, &fixture.session).await;
    let aborted = reviewed
        .with_participant_phase(FederationSetupParticipantPhase::Aborted, NOW_UNIX + 4)
        .expect("aborted phase");
    transition_session(db, &reviewed, &aborted).await;
    let approved = reviewed
        .with_participant_phase(FederationSetupParticipantPhase::Approved, NOW_UNIX + 4)
        .expect("approved phase");
    let approval = fixture
        .approvals
        .get(&member_id(1))
        .expect("local approval");
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("failing fence transaction");
    let result = tx
        .fence_federation_setup_approval(&reviewed, &approved, &fixture.proposal, approval)
        .await;
    assert_setup_conflict(result.expect_err("stale approval CAS must fail"));
    tx.rollback().await.expect("rollback approval fence");
    assert!(db
        .get_federation_setup_approval_fence(fixture.session.setup_id, member_id(1))
        .await
        .expect("fence after rollback")
        .is_none());
}

async fn rollback_manifest_storage<DB>(db: &DB, fixture: SetupFixture)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    persist_session(db, &fixture.session).await;
    let published = fixture
        .session
        .with_coordinator_phase(
            FederationSetupCoordinatorPhase::ProposalPublished,
            NOW_UNIX + 1,
        )
        .expect("published phase");
    transition_session(db, &fixture.session, &published).await;
    let aborted = published
        .with_coordinator_phase(FederationSetupCoordinatorPhase::Aborted, NOW_UNIX + 2)
        .expect("aborted phase");
    transition_session(db, &published, &aborted).await;
    let certified = published
        .with_coordinator_phase(FederationSetupCoordinatorPhase::Certified, NOW_UNIX + 2)
        .expect("certified phase");
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("failing manifest transaction");
    let result = tx
        .store_federation_setup_manifest(&published, &certified, &fixture.manifest)
        .await;
    assert_setup_conflict(result.expect_err("stale manifest CAS must fail"));
    tx.rollback().await.expect("rollback manifest insert");
    assert!(db
        .get_federation_setup_manifest(fixture.session.setup_id)
        .await
        .expect("manifest after rollback")
        .is_none());
}

async fn rollback_activation<DB>(db: &DB, fixture: SetupFixture)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let member_session = FederationSetupSession::new(
        fixture.session.setup_id,
        FederationSetupLocalRole::Member {
            member_id: member_id(1),
        },
        NOW_UNIX,
    )
    .expect("member session");
    persist_session(db, &member_session).await;
    let mut current = member_session;
    for (phase, time) in [
        (FederationSetupParticipantPhase::Enrolled, 1),
        (FederationSetupParticipantPhase::ProposalReviewed, 2),
        (FederationSetupParticipantPhase::Approved, 3),
        (FederationSetupParticipantPhase::Certified, 4),
        (FederationSetupParticipantPhase::DkgRunning, 5),
        (FederationSetupParticipantPhase::ReadyToActivate, 6),
    ] {
        let next = current
            .with_participant_phase(phase, NOW_UNIX + time)
            .expect("participant phase");
        transition_session(db, &current, &next).await;
        current = next;
    }
    let ready = current;
    let aborted = ready
        .with_participant_phase(FederationSetupParticipantPhase::Aborted, NOW_UNIX + 7)
        .expect("aborted phase");
    transition_session(db, &ready, &aborted).await;
    let activated = ready
        .with_participant_phase(FederationSetupParticipantPhase::Activated, NOW_UNIX + 7)
        .expect("activated phase");
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("failing activation transaction");
    let result = tx
        .activate_federation_setup(&ready, &activated, &fixture.activation)
        .await;
    assert_setup_conflict(result.expect_err("stale activation CAS must fail"));
    tx.rollback().await.expect("rollback activation insert");
    assert!(db
        .get_federation_setup_activations(fixture.session.setup_id)
        .await
        .expect("activations after rollback")
        .is_empty());
}

async fn persist_session_and_invitation<DB>(db: &DB, fixture: &SetupFixture)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("initial transaction");
    tx.add_federation_setup_session(&fixture.session)
        .await
        .expect("initial session");
    tx.add_federation_setup_invitation(&fixture.invitation)
        .await
        .expect("initial invitation");
    tx.commit().await.expect("commit initial setup");
}

async fn persist_session<DB>(db: &DB, session: &FederationSetupSession)
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("session transaction");
    tx.add_federation_setup_session(session)
        .await
        .expect("persist session");
    tx.commit().await.expect("commit session");
}

async fn transition_session<DB>(
    db: &DB,
    expected: &FederationSetupSession,
    updated: &FederationSetupSession,
) where
    DB: Database<Error> + Send + Sync + 'static,
{
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("transition transaction");
    tx.transition_federation_setup_session(expected, updated)
        .await
        .expect("transition session");
    tx.commit().await.expect("commit transition");
}

async fn advance_to_reviewed<DB>(
    db: &DB,
    initial: &FederationSetupSession,
) -> FederationSetupSession
where
    DB: Database<Error> + Send + Sync + 'static,
{
    persist_session(db, initial).await;
    let enrolled = initial
        .with_participant_phase(FederationSetupParticipantPhase::Enrolled, NOW_UNIX + 1)
        .expect("enrolled phase");
    transition_session(db, initial, &enrolled).await;
    let reviewed = enrolled
        .with_participant_phase(
            FederationSetupParticipantPhase::ProposalReviewed,
            NOW_UNIX + 2,
        )
        .expect("reviewed phase");
    transition_session(db, &enrolled, &reviewed).await;
    reviewed
}

async fn consume_and_commit<DB>(
    db: Arc<DB>,
    available: FederationSetupInvitationRecord,
    consumed: FederationSetupInvitationRecord,
    enrollment: FederationSetupEnrollment,
) -> bool
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let mut tx = Database::begin_transaction(db.as_ref())
        .await
        .expect("race transaction");
    match tx
        .consume_federation_setup_invitation(&available, &consumed, &enrollment)
        .await
    {
        Ok(()) => tx.commit().await.is_ok(),
        Err(_) => {
            tx.rollback().await.expect("rollback losing enrollment");
            false
        }
    }
}

async fn fence_and_commit<DB>(
    db: Arc<DB>,
    reviewed: FederationSetupSession,
    approved: FederationSetupSession,
    proposal: FederationSetupProposal,
    approval: FederationSetupApproval,
) -> bool
where
    DB: Database<Error> + Send + Sync + 'static,
{
    let mut tx = Database::begin_transaction(db.as_ref())
        .await
        .expect("fence race transaction");
    match tx
        .fence_federation_setup_approval(&reviewed, &approved, &proposal, &approval)
        .await
    {
        Ok(()) => tx.commit().await.is_ok(),
        Err(_) => {
            tx.rollback().await.expect("rollback losing approval");
            false
        }
    }
}

async fn assert_conflicting_write<DB, F>(db: &DB, write: F)
where
    DB: Database<Error> + Send + Sync + 'static,
    F: for<'a> FnOnce(
        &'a mut (dyn crate::database::MintTransaction<Error> + Send + Sync),
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), Error>> + Send + 'a>,
    >,
{
    let mut tx = Database::begin_transaction(db)
        .await
        .expect("conflict transaction");
    let error = write(&mut *tx)
        .await
        .expect_err("conflicting setup write must fail");
    assert_setup_conflict(error);
    tx.rollback().await.expect("rollback conflict");
}

fn assert_setup_conflict(error: Error) {
    assert!(matches!(
        error,
        Error::Federation(FederationError::SetupPersistenceConflict { .. })
    ));
}

fn setup_fixture(setup_byte: u8, variant: u8) -> SetupFixture {
    let setup_id = FederationSetupId::from_bytes([setup_byte; 32]);
    let coordinator_secret = secret_key(60_000);
    let coordinator_identity =
        FederationSetupCoordinatorPublicKey::new(coordinator_secret.public_key())
            .expect("coordinator identity");
    let session = FederationSetupSession::new(
        setup_id,
        FederationSetupLocalRole::Coordinator {
            member_id: member_id(1),
        },
        NOW_UNIX,
    )
    .expect("setup session");
    let invitation_id = invitation_id(setup_byte, 1);
    let invitation = FederationSetupInvitation::new(
        setup_id,
        invitation_id,
        member_id(1),
        mint_url(&format!("https://coordinator-{setup_byte}.example")),
        coordinator_identity,
        NOW_UNIX + FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        FederationSetupInvitationCapability::from_bytes([0xab; 32]).expect("invitation capability"),
        NOW_UNIX,
        URL_POLICY,
    )
    .expect("setup invitation");
    let invitation = FederationSetupInvitationRecord::from_invitation(&invitation, NOW_UNIX)
        .expect("invitation record");

    let enrollments = (1..=3)
        .map(|member| (member_id(member), signed_enrollment(setup_byte, member, 0)))
        .collect::<BTreeMap<_, _>>();
    let enrollment = enrollments
        .get(&member_id(1))
        .expect("local enrollment")
        .clone();
    let mut checkpoint_policy = FederationCheckpointPolicy::default();
    checkpoint_policy.checkpoint_interval_journal_operations += u64::from(variant);
    let keyset = FederationDkgKeysetSpec::new(
        CurrencyUnit::Sat,
        true,
        0,
        None,
        [Amount::from(1), Amount::from(2)]
            .into_iter()
            .collect::<BTreeSet<_>>(),
    )
    .expect("setup keyset");
    let network = FederationSetupBitcoinNetwork::Signet;
    let frost = FederationSetupFrostPolicy::new(
        1,
        vec![FederationSetupFrostApplication::CdkOnchain {
            application_key_epoch: 1,
            confirmation_policy_version: 1,
            minimum_confirmations: 1,
            fee_policy_version: 1,
        }],
        network,
    )
    .expect("FROST policy");
    let body = FederationSetupProposalBody::new(
        setup_id,
        coordinator_identity,
        enrollments,
        ThresholdParams::new(3, 2, 3).expect("safe thresholds"),
        FederationSetupProtocolVersions::default(),
        network,
        FederationPaymentObservationPolicy::member_quorum(3),
        checkpoint_policy,
        vec![keyset],
        frost,
        URL_POLICY,
    )
    .expect("proposal body");
    let proposal = FederationSetupProposal::sign_no_aux_rand_for_test_vector(
        body,
        &coordinator_secret,
        URL_POLICY,
    )
    .expect("proposal");
    let approvals = (1..=3)
        .map(|member| {
            let id = member_id(member);
            let approval = FederationSetupApproval::sign_no_aux_rand_for_test_vector(
                &proposal,
                id,
                &secret_key(1_000 + member),
                URL_POLICY,
            )
            .expect("approval");
            (id, approval)
        })
        .collect::<BTreeMap<_, _>>();
    let manifest = FederationSetupManifest::certify_no_aux_rand_for_test_vector(
        proposal.clone(),
        approvals.clone(),
        &coordinator_secret,
        URL_POLICY,
    )
    .expect("manifest");
    let config = manifest.setup_public_config().expect("setup config");
    let activation = FederationSetupActivation {
        version: FEDERATION_SETUP_PROTOCOL_VERSION,
        setup_id,
        member_id: member_id(1),
        manifest_hash: manifest.manifest_hash,
        setup_authorization: manifest
            .setup_authorization_hash()
            .expect("setup authorization"),
        federation_id: config.federation_id,
        final_config_digest: config.config_digest().expect("config digest"),
        activated_at_unix: NOW_UNIX + 9,
    };
    SetupFixture {
        session,
        invitation,
        enrollment,
        proposal,
        approvals,
        manifest,
        activation,
    }
}

fn signed_enrollment(setup_byte: u8, member: u16, nonce_variant: u8) -> FederationSetupEnrollment {
    FederationSetupEnrollment::sign_no_aux_rand_for_test_vector(
        FederationSetupId::from_bytes([setup_byte; 32]),
        invitation_id(setup_byte, member),
        member_id(member),
        mint_url(&format!("https://mint-{member}-{setup_byte}.example")),
        mint_url(&format!("https://peer-{member}-{setup_byte}.example")),
        [setup_byte
            .wrapping_add(u8::try_from(member).expect("small member"))
            .wrapping_add(nonce_variant)
            .wrapping_add(1); 32],
        FederationSetupProtocolVersions::default(),
        &secret_key(1_000 + member),
        URL_POLICY,
    )
    .expect("signed enrollment")
}

fn invitation_id(setup_byte: u8, member: u16) -> FederationSetupInvitationId {
    FederationSetupInvitationId::from_bytes(
        [setup_byte.wrapping_add(u8::try_from(member).expect("small member")); 32],
    )
}

fn member_id(value: u16) -> MemberId {
    MemberId::new(value).expect("non-zero member ID")
}

fn mint_url(value: &str) -> MintUrl {
    MintUrl::from_str(value).expect("valid setup URL")
}

fn secret_key(value: u16) -> SecretKey {
    let mut bytes = [0_u8; 32];
    bytes[30..].copy_from_slice(&value.to_be_bytes());
    SecretKey::from_slice(&bytes).expect("valid fixture secret")
}
