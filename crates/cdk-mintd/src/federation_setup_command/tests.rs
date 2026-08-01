use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use cdk_common::database::mint::{Database, FederationDkgDatabase, FederationFrostWalletDatabase};
use cdk_common::database::MintFederationSetupDatabase;
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
    FederationSetupApproval, FederationSetupBitcoinNetwork, FederationSetupEnrollment,
    FederationSetupFrostApplication, FederationSetupFrostPolicy, FederationSetupInvitation,
    FederationSetupInvitationState, FederationSetupParticipantPhase, FederationSetupProposal,
    FederationSetupUrlPolicy, MemberId, ThresholdParams,
    FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::{Amount, CurrencyUnit};
use cdk_sqlite::MintSqliteDatabase;
use tempfile::TempDir;
use tokio::sync::oneshot;

use super::*;

const NOW_UNIX: u64 = 1_900_000_000;
const URL_POLICY: FederationSetupUrlPolicy = FederationSetupUrlPolicy::AllowInsecureLocalhost;

struct FakeCoordinator {
    db: Arc<MintSqliteDatabase>,
    fail_enrollment_after_commit: AtomicBool,
    fail_approval_after_commit: AtomicBool,
}

impl FakeCoordinator {
    fn new(db: Arc<MintSqliteDatabase>) -> Self {
        Self {
            db,
            fail_enrollment_after_commit: AtomicBool::new(true),
            fail_approval_after_commit: AtomicBool::new(true),
        }
    }
}

#[async_trait]
impl FederationSetupCoordinator for FakeCoordinator {
    async fn enroll(
        &self,
        invitation: &FederationSetupInvitation,
        enrollment: &FederationSetupEnrollment,
    ) -> Result<FederationSetupEnrollment> {
        invitation
            .validate(NOW_UNIX, URL_POLICY)
            .context("fake coordinator rejected invitation")?;
        enrollment
            .validate(URL_POLICY)
            .context("fake coordinator rejected enrollment")?;
        if invitation.setup_id != enrollment.setup_id
            || invitation.invitation_id != enrollment.invitation_id
            || invitation.member_id != enrollment.member_id
        {
            bail!("fake coordinator enrollment binding mismatch");
        }
        if let Some(existing) = self
            .db
            .get_federation_setup_enrollments(invitation.setup_id)
            .await?
            .into_iter()
            .find(|existing| existing.member_id == enrollment.member_id)
        {
            if existing == *enrollment {
                return Ok(existing);
            }
            bail!("fake coordinator enrollment conflict");
        }

        let available = self
            .db
            .get_federation_setup_invitation(invitation.setup_id, invitation.invitation_id)
            .await?
            .ok_or_else(|| anyhow!("fake coordinator invitation is missing"))?;
        if available.state != FederationSetupInvitationState::Available
            || available.capability_hash != invitation.capability_hash()
        {
            bail!("fake coordinator invitation is unavailable");
        }
        let consumed = available.consumed()?;
        let mut tx = Database::begin_transaction(self.db.as_ref()).await?;
        tx.consume_federation_setup_invitation(&available, &consumed, enrollment)
            .await?;
        tx.commit().await?;
        if self
            .fail_enrollment_after_commit
            .swap(false, Ordering::SeqCst)
        {
            bail!("simulated lost enrollment acknowledgement");
        }
        Ok(enrollment.clone())
    }

    async fn proposal(
        &self,
        invitation: &FederationSetupInvitation,
    ) -> Result<FederationSetupProposal> {
        if invitation.capability_hash()
            != self
                .db
                .get_federation_setup_invitation(invitation.setup_id, invitation.invitation_id)
                .await?
                .ok_or_else(|| anyhow!("fake coordinator invitation is missing"))?
                .capability_hash
        {
            bail!("fake coordinator invitation capability mismatch");
        }
        self.db
            .get_federation_setup_proposal(invitation.setup_id)
            .await?
            .ok_or_else(|| anyhow!("fake coordinator proposal is not published"))
    }

    async fn approve(
        &self,
        invitation: &FederationSetupInvitation,
        approval: &FederationSetupApproval,
    ) -> Result<FederationSetupApproval> {
        let proposal = self.proposal(invitation).await?;
        approval
            .validate(&proposal)
            .context("fake coordinator rejected approval")?;
        if approval.member_id != invitation.member_id {
            bail!("fake coordinator approval member mismatch");
        }
        if let Some(existing) = self
            .db
            .get_federation_setup_approvals(invitation.setup_id)
            .await?
            .into_iter()
            .find(|existing| existing.member_id == approval.member_id)
        {
            if existing == *approval {
                return Ok(existing);
            }
            bail!("fake coordinator approval conflict");
        }
        let mut tx = Database::begin_transaction(self.db.as_ref()).await?;
        tx.add_federation_setup_approval(approval).await?;
        tx.commit().await?;
        if self
            .fail_approval_after_commit
            .swap(false, Ordering::SeqCst)
        {
            bail!("simulated lost approval acknowledgement");
        }
        Ok(approval.clone())
    }
}

#[tokio::test]
async fn independent_operators_review_and_approve_without_copying_private_material() {
    let root = TempDir::new().expect("test directory");
    let creator_dir = root.path().join("creator");
    let member_two_dir = root.path().join("member-two");
    let member_three_dir = root.path().join("member-three");
    for directory in [&creator_dir, &member_two_dir, &member_three_dir] {
        std::fs::create_dir(directory).expect("independent operator directory");
    }

    let creator_config = creator_dir.join("setup.json");
    let creator_db_path = creator_dir.join("mint.sqlite");
    let creator_db = Arc::new(
        MintSqliteDatabase::new(creator_db_path.clone())
            .await
            .expect("creator database"),
    );
    let created = create_setup(
        creator_db.as_ref(),
        &creator_config,
        &creator_dir,
        &creator_db_path,
        &create_spec(),
        NOW_UNIX,
    )
    .await
    .expect("creator setup");
    let creator_secrets_before =
        std::fs::read(creator_dir.join(LOCAL_SETUP_SECRETS_FILE)).expect("creator secrets bytes");

    let member_two_id = MemberId::new(2).expect("member two ID");
    let member_three_id = MemberId::new(3).expect("member three ID");
    let member_two_invitation = read_invitation(
        created
            .invitation_paths
            .get(&member_two_id)
            .expect("member two invitation path"),
    );
    let replacement_path = creator_dir.join("member-three-replacement.invite");
    let member_three_invitation = issue_invitation(
        creator_db.as_ref(),
        &creator_config,
        member_three_id,
        true,
        &replacement_path,
        NOW_UNIX + 1,
    )
    .await
    .expect("replace unconsumed invitation");

    let coordinator = FakeCoordinator::new(Arc::clone(&creator_db));
    let member_two_config = member_two_dir.join("setup.json");
    let member_two_db_path = member_two_dir.join("mint.sqlite");
    let member_two_db = MintSqliteDatabase::new(member_two_db_path.clone())
        .await
        .expect("member two database");
    let first_join = join_setup(
        &member_two_db,
        &coordinator,
        &member_two_config,
        &member_two_dir,
        &member_two_db_path,
        member_two_invitation.clone(),
        mint_url("http://127.0.0.1:8412"),
        mint_url("http://127.0.0.1:8512"),
        URL_POLICY,
        NOW_UNIX,
    )
    .await;
    assert!(
        first_join.is_err(),
        "first acknowledgement is deliberately lost"
    );
    let member_two_secrets_before = std::fs::read(member_two_dir.join(LOCAL_SETUP_SECRETS_FILE))
        .expect("member two secrets before retry");
    let member_two_enrollment = join_setup(
        &member_two_db,
        &coordinator,
        &member_two_config,
        &member_two_dir,
        &member_two_db_path,
        member_two_invitation,
        mint_url("http://127.0.0.1:8412"),
        mint_url("http://127.0.0.1:8512"),
        URL_POLICY,
        NOW_UNIX + 1,
    )
    .await
    .expect("member two retry");
    assert_eq!(
        member_two_secrets_before,
        std::fs::read(member_two_dir.join(LOCAL_SETUP_SECRETS_FILE))
            .expect("member two secrets after retry")
    );

    let member_three_config = member_three_dir.join("setup.json");
    let member_three_db_path = member_three_dir.join("mint.sqlite");
    let member_three_db = MintSqliteDatabase::new(member_three_db_path.clone())
        .await
        .expect("member three database");
    let member_three_enrollment = join_setup(
        &member_three_db,
        &coordinator,
        &member_three_config,
        &member_three_dir,
        &member_three_db_path,
        member_three_invitation,
        mint_url("http://127.0.0.1:8413"),
        mint_url("http://127.0.0.1:8513"),
        URL_POLICY,
        NOW_UNIX + 1,
    )
    .await
    .expect("member three join");
    let refused = issue_invitation(
        creator_db.as_ref(),
        &creator_config,
        member_three_id,
        true,
        &creator_dir.join("forbidden-replacement.invite"),
        NOW_UNIX + 2,
    )
    .await;
    assert!(refused.is_err(), "consumed invitation cannot be replaced");

    let proposal = propose_setup(creator_db.as_ref(), &creator_config, NOW_UNIX + 2)
        .await
        .expect("publish proposal");
    let creator_review = review_local_proposal(
        creator_db.as_ref(),
        &creator_config,
        proposal.clone(),
        NOW_UNIX + 3,
    )
    .await
    .expect("creator review");
    let member_two_review = fetch_and_review_proposal(
        &member_two_db,
        &coordinator,
        &member_two_config,
        NOW_UNIX + 3,
    )
    .await
    .expect("member two review");
    let member_three_review = fetch_and_review_proposal(
        &member_three_db,
        &coordinator,
        &member_three_config,
        NOW_UNIX + 3,
    )
    .await
    .expect("member three review");
    assert_eq!(creator_review, member_two_review);
    assert_eq!(creator_review, member_three_review);

    let wrong_hash_approval = approve_setup(
        &member_three_db,
        &coordinator,
        &member_three_config,
        FederationSetupProposalHash::from_bytes([42; 32]),
        NOW_UNIX + 4,
    )
    .await;
    assert!(wrong_hash_approval.is_err());
    assert!(member_three_db
        .get_federation_setup_approval_fence(created.setup_id, member_three_id)
        .await
        .expect("member three fence read")
        .is_none());

    let creator_approval = approve_setup(
        creator_db.as_ref(),
        &coordinator,
        &creator_config,
        proposal.proposal_hash,
        NOW_UNIX + 4,
    )
    .await
    .expect("creator approval");
    let first_approval = approve_setup(
        &member_two_db,
        &coordinator,
        &member_two_config,
        proposal.proposal_hash,
        NOW_UNIX + 4,
    )
    .await;
    assert!(
        first_approval.is_err(),
        "first approval acknowledgement is deliberately lost"
    );
    let fenced_before = member_two_db
        .get_federation_setup_approval_fence(created.setup_id, member_two_id)
        .await
        .expect("member two fence read")
        .expect("member two approval was fenced before transmission");
    let member_two_approval = approve_setup(
        &member_two_db,
        &coordinator,
        &member_two_config,
        proposal.proposal_hash,
        NOW_UNIX + 5,
    )
    .await
    .expect("member two approval retry");
    assert_eq!(fenced_before, member_two_approval);
    let member_three_approval = approve_setup(
        &member_three_db,
        &coordinator,
        &member_three_config,
        proposal.proposal_hash,
        NOW_UNIX + 4,
    )
    .await
    .expect("member three approval");

    let approvals = creator_db
        .get_federation_setup_approvals(created.setup_id)
        .await
        .expect("coordinator approvals");
    assert_eq!(approvals.len(), 3);
    assert!(approvals.contains(&creator_approval));
    assert!(approvals.contains(&member_two_approval));
    assert!(approvals.contains(&member_three_approval));

    let creator_secrets: FederationSetupLocalSecrets = read_private_json_file(
        &creator_dir.join(LOCAL_SETUP_SECRETS_FILE),
        "creator test secrets",
    )
    .expect("creator secrets");
    let member_two_secrets: FederationSetupLocalSecrets = read_private_json_file(
        &member_two_dir.join(LOCAL_SETUP_SECRETS_FILE),
        "member two test secrets",
    )
    .expect("member two secrets");
    let member_three_secrets: FederationSetupLocalSecrets = read_private_json_file(
        &member_three_dir.join(LOCAL_SETUP_SECRETS_FILE),
        "member three test secrets",
    )
    .expect("member three secrets");
    assert_eq!(
        creator_secrets_before,
        std::fs::read(creator_dir.join(LOCAL_SETUP_SECRETS_FILE))
            .expect("creator secrets after workflow")
    );
    assert_ne!(
        creator_secrets.identity_secret_key.public_key(),
        member_two_secrets.identity_secret_key.public_key()
    );
    assert_ne!(
        creator_secrets.identity_secret_key.public_key(),
        member_three_secrets.identity_secret_key.public_key()
    );
    assert_ne!(
        member_two_secrets.identity_secret_key.public_key(),
        member_three_secrets.identity_secret_key.public_key()
    );
    assert_eq!(
        member_two_secrets.enrollment, member_two_enrollment,
        "retry retains the exact enrollment bytes"
    );
    assert_eq!(member_three_secrets.enrollment, member_three_enrollment);
    assert!(format!("{member_two_secrets:?}").contains("[REDACTED]"));

    for path in [
        creator_config,
        creator_dir.join(LOCAL_SETUP_SECRETS_FILE),
        created
            .invitation_paths
            .get(&member_two_id)
            .expect("member two invitation path")
            .clone(),
        replacement_path,
        member_two_config,
        member_two_dir.join(LOCAL_SETUP_SECRETS_FILE),
        member_three_config,
        member_three_dir.join(LOCAL_SETUP_SECRETS_FILE),
    ] {
        assert_protected(&path);
    }
}

#[tokio::test]
async fn ordinary_http_client_drives_three_independent_operator_stores_to_certification() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("setup listener");
    let coordinator_address = listener.local_addr().expect("setup address");
    let creator_dkg_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("creator DKG listener");
    let member_two_dkg_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("member two DKG listener");
    let member_three_dkg_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("member three DKG listener");
    let creator_dkg_url = format!(
        "http://{}",
        creator_dkg_listener
            .local_addr()
            .expect("creator DKG address")
    );
    let member_two_dkg_url = format!(
        "http://{}",
        member_two_dkg_listener
            .local_addr()
            .expect("member two DKG address")
    );
    let member_three_dkg_url = format!(
        "http://{}",
        member_three_dkg_listener
            .local_addr()
            .expect("member three DKG address")
    );
    let now = current_unix_time().expect("current time");
    let root = TempDir::new().expect("test directory");
    let creator_dir = root.path().join("creator");
    let member_two_dir = root.path().join("member-two");
    let member_three_dir = root.path().join("member-three");
    for directory in [&creator_dir, &member_two_dir, &member_three_dir] {
        std::fs::create_dir(directory).expect("independent operator directory");
    }

    let creator_config = creator_dir.join("setup.json");
    let creator_db_path = creator_dir.join("mint.sqlite");
    let creator_db = Arc::new(
        MintSqliteDatabase::new(creator_db_path.clone())
            .await
            .expect("creator database"),
    );
    let mut spec = create_spec();
    spec.coordinator_url = mint_url(&format!("http://{coordinator_address}"));
    spec.federation_api_url = mint_url(&creator_dkg_url);
    let created = create_setup(
        creator_db.as_ref(),
        &creator_config,
        &creator_dir,
        &creator_db_path,
        &spec,
        now,
    )
    .await
    .expect("creator setup");
    let (_, creator_secrets) = load_local_state(&creator_config, now).expect("creator state");
    let dynamic_database: cdk_common::database::DynMintDatabase = creator_db.clone();
    let service = Arc::new(
        cdk_axum::FederationSetupCoordinatorService::new(
            dynamic_database,
            created.setup_id,
            3,
            creator_secrets
                .coordinator_secret_key
                .clone()
                .expect("coordinator secret"),
            URL_POLICY,
            cdk_axum::FederationSetupCoordinatorServiceConfig::default(),
        )
        .expect("setup service"),
    );
    let router = cdk_axum::create_federation_setup_router(service.clone());
    let (shutdown, shutdown_receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown_receiver.await;
        })
        .await
        .expect("setup server");
    });

    let member_two_id = MemberId::new(2).expect("member two");
    let member_three_id = MemberId::new(3).expect("member three");
    let member_two_invitation = read_invitation(
        created
            .invitation_paths
            .get(&member_two_id)
            .expect("member two invitation"),
    );
    let member_three_invitation = read_invitation(
        created
            .invitation_paths
            .get(&member_three_id)
            .expect("member three invitation"),
    );
    let member_two_config = member_two_dir.join("setup.json");
    let member_three_config = member_three_dir.join("setup.json");
    let member_two_db_path = member_two_dir.join("mint.sqlite");
    let member_three_db_path = member_three_dir.join("mint.sqlite");
    let member_two_db = MintSqliteDatabase::new(member_two_db_path.clone())
        .await
        .expect("member two database");
    let member_three_db = MintSqliteDatabase::new(member_three_db_path.clone())
        .await
        .expect("member three database");
    let coordinator = HttpSetupCoordinator::default();
    let (member_two_join, member_three_join) = tokio::join!(
        join_setup(
            &member_two_db,
            &coordinator,
            &member_two_config,
            &member_two_dir,
            &member_two_db_path,
            member_two_invitation,
            mint_url("http://127.0.0.1:8412"),
            mint_url(&member_two_dkg_url),
            URL_POLICY,
            now,
        ),
        join_setup(
            &member_three_db,
            &coordinator,
            &member_three_config,
            &member_three_dir,
            &member_three_db_path,
            member_three_invitation,
            mint_url("http://127.0.0.1:8413"),
            mint_url(&member_three_dkg_url),
            URL_POLICY,
            now,
        )
    );
    member_two_join.expect("member two HTTP join");
    member_three_join.expect("member three HTTP join");

    for config_path in [&creator_config, &member_two_config, &member_three_config] {
        let error = load_certified_dkg_handoff(config_path)
            .expect_err("DKG must not start before a complete manifest is persisted");
        assert!(error.to_string().contains("after certification"));
    }

    let proposal = propose_setup(creator_db.as_ref(), &creator_config, now)
        .await
        .expect("publish proposal");
    review_local_proposal(creator_db.as_ref(), &creator_config, proposal.clone(), now)
        .await
        .expect("creator review");
    let (member_two_review, member_three_review) = tokio::join!(
        fetch_and_review_proposal(&member_two_db, &coordinator, &member_two_config, now),
        fetch_and_review_proposal(&member_three_db, &coordinator, &member_three_config, now,)
    );
    assert_eq!(
        member_two_review.expect("member two review"),
        member_three_review.expect("member three review")
    );
    approve_setup(
        creator_db.as_ref(),
        &coordinator,
        &creator_config,
        proposal.proposal_hash,
        now,
    )
    .await
    .expect("creator approval");
    let (member_two_approval, member_three_approval) = tokio::join!(
        approve_setup(
            &member_two_db,
            &coordinator,
            &member_two_config,
            proposal.proposal_hash,
            now,
        ),
        approve_setup(
            &member_three_db,
            &coordinator,
            &member_three_config,
            proposal.proposal_hash,
            now,
        )
    );
    member_two_approval.expect("member two approval");
    member_three_approval.expect("member three approval");

    let (member_two_status, member_three_status) = tokio::join!(
        setup_status(&member_two_config),
        setup_status(&member_three_config)
    );
    assert_eq!(
        member_two_status
            .expect("member two status")
            .coordinator_phase,
        None
    );
    assert_eq!(
        member_three_status
            .expect("member three status")
            .coordinator_phase,
        None
    );
    assert_eq!(
        setup_status(&creator_config)
            .await
            .expect("creator status")
            .coordinator_phase,
        Some("certified".to_string())
    );
    for (database, config_path) in [
        (&member_two_db, &member_two_config),
        (&member_three_db, &member_three_config),
    ] {
        let config = read_local_config(config_path).expect("member config");
        let session = database
            .get_federation_setup_session(created.setup_id)
            .await
            .expect("member session")
            .expect("member session exists");
        assert_eq!(
            session.participant_phase,
            FederationSetupParticipantPhase::Certified
        );
        let manifest = database
            .get_federation_setup_manifest(config.setup_id)
            .await
            .expect("member manifest")
            .expect("member manifest exists");
        assert_eq!(manifest.proposal.proposal_hash, proposal.proposal_hash);
    }
    let _ = shutdown.send(());
    server.await.expect("setup server task");
    service.shutdown().await;

    let databases: Vec<Arc<MintSqliteDatabase>> = vec![
        creator_db.clone(),
        Arc::new(member_two_db),
        Arc::new(member_three_db),
    ];
    let handoffs = [&creator_config, &member_two_config, &member_three_config]
        .into_iter()
        .map(|config_path| {
            load_certified_dkg_handoff(config_path).expect("certified manifest DKG handoff")
        })
        .collect::<Vec<_>>();

    for config_path in [&creator_config, &member_two_config, &member_three_config] {
        let error = activate_setup(config_path, now)
            .await
            .expect_err("activation must fail before DKG is complete");
        assert!(error.to_string().contains("cannot activate before DKG"));
        let config = read_local_config(config_path).expect("pre-DKG local config");
        assert!(!config.activation_dir().exists());
    }

    let mut routers = Vec::new();
    for (handoff, database) in handoffs.iter().zip(&databases) {
        let dynamic_database: cdk_common::database::DynMintDatabase = database.clone();
        assert_eq!(
            handoff.manifest.proposal.proposal_hash,
            proposal.proposal_hash
        );
        assert_eq!(
            handoff.public_config.setup_authorization,
            handoffs[0].public_config.setup_authorization
        );
        assert_eq!(handoff.public_config, handoffs[0].public_config);
        assert_eq!(
            handoff.private_config.member_id,
            handoff
                .public_config
                .member(handoff.private_config.member_id)
                .expect("local member in certified roster")
                .member_id
        );
        let auto_start = cdk_axum::FederationDkgAutoStartConfig::from_manifest(
            &handoff.manifest,
            handoff.url_policy,
            cdk_axum::FederationDkgDriverRunConfig::default(),
        )
        .expect("manifest-derived DKG auto-start");
        assert_eq!(
            auto_start.setup_authorization(),
            handoff.public_config.setup_authorization
        );
        assert_eq!(
            auto_start.ceremony_id(),
            handoff.manifest.ceremony_id().expect("manifest ceremony")
        );
        assert_eq!(auto_start.keyset_spec(), &create_spec().keysets[0]);
        assert_eq!(
            auto_start.root_key_epoch(),
            handoff.manifest.frost_policy().root_key_epoch
        );
        register_manifest_dkg_ceremony(&dynamic_database, handoff, &auto_start)
            .await
            .expect("manifest-derived lifecycle registration");
        let records = database
            .get_federation_dkg_ceremonies()
            .await
            .expect("DKG lifecycle records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].member_id, handoff.private_config.member_id);
        assert_eq!(records[0].ceremony_id, auto_start.ceremony_id());
        assert_eq!(
            records[0].setup_authorization,
            auto_start.setup_authorization()
        );
        let session = database
            .get_federation_setup_session(created.setup_id)
            .await
            .expect("running setup session")
            .expect("running setup session exists");
        assert_eq!(
            session.participant_phase,
            FederationSetupParticipantPhase::DkgRunning
        );
        let mint = build_certified_dkg_mint(database.clone(), handoff)
            .await
            .expect("setup-only DKG mint");
        routers.push(
            cdk_axum::create_federation_dkg_setup_router(
                Arc::new(mint),
                cdk_axum::FederationPeerSyncConfig::default(),
                cdk_axum::FederationDkgAutoStartConfig::from_manifest(
                    &handoff.manifest,
                    handoff.url_policy,
                    cdk_axum::FederationDkgDriverRunConfig {
                        max_ticks: 2_000,
                        retry_delay: std::time::Duration::from_millis(10),
                        ..cdk_axum::FederationDkgDriverRunConfig::default()
                    },
                )
                .expect("manifest DKG auto-start"),
            )
            .await
            .expect("setup-only DKG router"),
        );
    }

    let listeners = [
        creator_dkg_listener,
        member_two_dkg_listener,
        member_three_dkg_listener,
    ];
    let mut dkg_shutdown_senders = Vec::new();
    let mut dkg_servers = Vec::new();
    for (listener, router) in listeners.into_iter().zip(routers) {
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        dkg_shutdown_senders.push(shutdown_sender);
        dkg_servers.push(tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = shutdown_receiver.await;
                })
                .await
                .expect("DKG server");
        }));
    }

    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let mut all_ready = true;
            for database in &databases {
                let ceremonies = database
                    .get_federation_dkg_ceremonies()
                    .await
                    .expect("DKG ceremonies while polling");
                let activations = database
                    .get_federation_dkg_activations()
                    .await
                    .expect("DKG activations while polling");
                let frost_roots = database
                    .get_federation_frost_root_public_configs()
                    .await
                    .expect("FROST roots while polling");
                all_ready &= ceremonies
                    .iter()
                    .any(|record| record.state == FederationDkgCeremonyState::Complete)
                    && activations.len() == 3
                    && frost_roots.len() == 1;
            }
            if all_ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("concurrent manifest-derived DKG completion");

    let (creator_status, member_two_status, member_three_status) = tokio::join!(
        setup_status(&creator_config),
        setup_status(&member_two_config),
        setup_status(&member_three_config)
    );
    for status in [creator_status, member_two_status, member_three_status] {
        let status = status.expect("post-DKG setup status");
        assert_eq!(status.participant_phase, "ready_to_activate");
        assert!(status.dkg.expect("DKG progress").finalized_ready);
        assert!(status.activation.is_none());
    }
    let activation_now = current_unix_time().expect("activation time") + 1;

    activate_setup_with_faults(
        &member_two_config,
        activation_now,
        FinalConfigExportFault::None,
        SetupActivationFault::BeforeDatabaseCommit,
    )
    .await
    .expect_err("interrupted activation transaction");
    assert!(databases[1]
        .get_federation_setup_activations(created.setup_id)
        .await
        .expect("member two interrupted activations")
        .is_empty());
    assert_eq!(
        databases[1]
            .get_federation_setup_session(created.setup_id)
            .await
            .expect("member two interrupted session")
            .expect("member two interrupted session exists")
            .participant_phase,
        FederationSetupParticipantPhase::ReadyToActivate
    );

    activate_setup_with_faults(
        &member_three_config,
        activation_now,
        FinalConfigExportFault::FailPublicWrite,
        SetupActivationFault::None,
    )
    .await
    .expect_err("partial final-config export");
    assert!(!read_local_config(&member_three_config)
        .expect("member three config")
        .activation_dir()
        .exists());

    let creator_activation = activate_setup(&creator_config, activation_now)
        .await
        .expect("creator activation");
    let creator_replay = activate_setup(&creator_config, activation_now + 1)
        .await
        .expect("idempotent creator activation replay");
    assert_eq!(creator_activation, creator_replay);
    let service_output = activation_service_manager_output(&creator_activation);
    assert!(service_output.starts_with("activated=true\nsetup_id="));
    assert!(service_output.contains("\nfederation_setup_config_path=unset\n"));
    assert!(service_output.ends_with("service_restart_required=true\n"));
    let (member_two_activation, member_three_activation) = tokio::join!(
        activate_setup(&member_two_config, activation_now),
        activate_setup(&member_three_config, activation_now)
    );
    let member_two_activation = member_two_activation.expect("member two activation recovery");
    let member_three_activation =
        member_three_activation.expect("member three partial-export recovery");
    assert_eq!(
        creator_activation.final_config_digest,
        member_two_activation.final_config_digest
    );
    assert_eq!(
        creator_activation.final_config_digest,
        member_three_activation.final_config_digest
    );
    let creator_public: FederationConfig = crate::read_federation_json(
        &creator_activation.public_config_path,
        "restarted creator public config",
    )
    .expect("restarted creator public config");
    let member_two_public: FederationConfig = crate::read_federation_json(
        &member_two_activation.public_config_path,
        "restarted member two public config",
    )
    .expect("restarted member two public config");
    let member_three_public: FederationConfig = crate::read_federation_json(
        &member_three_activation.public_config_path,
        "restarted member three public config",
    )
    .expect("restarted member three public config");
    assert_eq!(creator_public, member_two_public);
    assert_eq!(creator_public, member_three_public);
    for (database, config_path) in
        databases
            .iter()
            .zip([&creator_config, &member_two_config, &member_three_config])
    {
        let status = setup_status(config_path)
            .await
            .expect("restarted activated setup status");
        assert_eq!(status.participant_phase, "activated");
        assert!(status.activation.is_some());
        assert_eq!(
            database
                .get_federation_setup_activations(created.setup_id)
                .await
                .expect("durable activation after restart")
                .len(),
            1
        );
    }

    for sender in dkg_shutdown_senders {
        let _ = sender.send(());
    }
    for server in dkg_servers {
        server.await.expect("DKG server task");
    }
}

#[cfg(unix)]
#[test]
fn protected_setup_reader_rejects_weak_permissions_symlinks_and_oversized_files() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    const MAX_TEST_PROTECTED_FILE_BYTES: u64 = 1024 * 1024;

    let directory = TempDir::new().expect("test directory");
    let protected = directory.path().join("protected.txt");
    write_private_text_file(&protected, "protected", false).expect("protected file");
    std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o644))
        .expect("weaken test permissions");
    assert!(read_private_text_file(&protected, "weak test file").is_err());

    std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o600))
        .expect("restore test permissions");
    let symlink_path = directory.path().join("protected-link.txt");
    symlink(&protected, &symlink_path).expect("test symlink");
    assert!(read_private_text_file(&symlink_path, "symlink test file").is_err());

    let oversized = directory.path().join("oversized.txt");
    std::fs::write(
        &oversized,
        vec![
            0_u8;
            usize::try_from(MAX_TEST_PROTECTED_FILE_BYTES + 1).expect("test size fits usize")
        ],
    )
    .expect("oversized test file");
    std::fs::set_permissions(&oversized, std::fs::Permissions::from_mode(0o600))
        .expect("protect oversized test file");
    assert!(read_private_text_file(&oversized, "oversized test file").is_err());
    assert!(write_private_text_file(
        &directory.path().join("oversized-output.txt"),
        &"x".repeat(
            usize::try_from(MAX_TEST_PROTECTED_FILE_BYTES + 1).expect("test size fits usize")
        ),
        false,
    )
    .is_err());
}

fn create_spec() -> FederationSetupCreateSpec {
    let thresholds = ThresholdParams::with_bft_consensus(3, 2).expect("safe thresholds");
    FederationSetupCreateSpec {
        threshold_params: thresholds,
        public_mint_url: mint_url("http://127.0.0.1:8411"),
        federation_api_url: mint_url("http://127.0.0.1:8511"),
        coordinator_url: mint_url("http://127.0.0.1:8611"),
        network: FederationSetupBitcoinNetwork::Regtest,
        payment_observation_policy: FederationPaymentObservationPolicy::member_quorum(
            thresholds.consensus_threshold,
        ),
        checkpoint_policy: FederationCheckpointPolicy::default(),
        keysets: vec![FederationDkgKeysetSpec::new(
            CurrencyUnit::Sat,
            true,
            0,
            None,
            [Amount::from(1), Amount::from(2)]
                .into_iter()
                .collect::<BTreeSet<_>>(),
        )
        .expect("test keyset")],
        frost: FederationSetupFrostPolicy::new(
            1,
            vec![FederationSetupFrostApplication::CdkOnchain {
                application_key_epoch: 1,
                confirmation_policy_version: 1,
                minimum_confirmations: 1,
                fee_policy_version: 1,
            }],
            FederationSetupBitcoinNetwork::Regtest,
        )
        .expect("test FROST policy"),
        invitation_lifetime_secs: FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
        url_policy: URL_POLICY,
    }
}

fn read_invitation(path: &Path) -> FederationSetupInvitation {
    let value = read_private_text_file(path, "test invitation").expect("invitation file");
    FederationSetupInvitation::from_str(value.trim()).expect("invitation parses")
}

fn mint_url(value: &str) -> MintUrl {
    MintUrl::from_str(value).expect("valid test URL")
}

#[cfg(unix)]
fn assert_protected(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path)
        .expect("protected file metadata")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "{} must not be group/world readable",
        path.display()
    );
}

#[cfg(not(unix))]
fn assert_protected(path: &Path) {
    assert!(path.is_file());
}
