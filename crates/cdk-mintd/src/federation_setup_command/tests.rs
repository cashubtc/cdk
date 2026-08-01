use std::collections::BTreeSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use cdk_common::database::mint::Database;
use cdk_common::database::MintFederationSetupDatabase;
use cdk_common::federation::{
    FederationCheckpointPolicy, FederationDkgKeysetSpec, FederationPaymentObservationPolicy,
    FederationSetupApproval, FederationSetupBitcoinNetwork, FederationSetupEnrollment,
    FederationSetupFrostApplication, FederationSetupFrostPolicy, FederationSetupInvitation,
    FederationSetupInvitationState, FederationSetupProposal, FederationSetupUrlPolicy, MemberId,
    ThresholdParams, FEDERATION_SETUP_MIN_INVITATION_LIFETIME_SECS,
};
use cdk_common::mint_url::MintUrl;
use cdk_common::{Amount, CurrencyUnit};
use cdk_sqlite::MintSqliteDatabase;
use tempfile::TempDir;

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
