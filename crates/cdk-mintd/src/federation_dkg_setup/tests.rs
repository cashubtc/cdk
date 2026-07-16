use std::path::{Path, PathBuf};

use super::*;

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cdk_mintd_dkg_setup_{label}_{}",
        std::process::id()
    ))
}

fn member_args(root: &Path, member_id: u16) -> FederationDkgMemberArgs {
    FederationDkgMemberArgs {
        member_id,
        public_mint_url: format!("http://127.0.0.1:{}", 20_000 + member_id),
        federation_api_url: format!("http://127.0.0.1:{}", 21_000 + member_id),
        public_output: root.join(format!("member-{member_id}-public.json")),
        private_output: root.join(format!("member-{member_id}-private.json")),
        force: false,
    }
}

fn roster_args(root: &Path, members: Vec<PathBuf>) -> FederationDkgRosterArgs {
    FederationDkgRosterArgs {
        members,
        threshold: 2,
        payment_observation_threshold: None,
        unit: "sat".to_string(),
        amounts: vec![1, 2, 4],
        input_fee_ppk: 0,
        final_expiry: None,
        checkpoint_interval_journal_operations:
            cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS,
        checkpoint_interval_finalized_items:
            cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS,
        public_output: root.join("dkg-setup-public.json"),
        ceremony_config_output: root.join("dkg-ceremony.toml"),
        force: false,
    }
}

#[test]
fn independent_member_enrollments_assemble_and_bind() {
    let root = temp_dir("round_trip");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test directory");

    let member_args = (1..=3)
        .map(|member_id| member_args(&root, member_id))
        .collect::<Vec<_>>();
    for args in &member_args {
        run_dkg_member_command(args).expect("generate independent member enrollment");
    }
    let roster_args = roster_args(
        &root,
        member_args
            .iter()
            .map(|args| args.public_output.clone())
            .collect(),
    );
    run_dkg_roster_command(&roster_args).expect("assemble public roster");

    let public_config: FederationConfig =
        read_federation_json(&roster_args.public_output, "test public config")
            .expect("read public config");
    let public_config_json =
        std::fs::read_to_string(&roster_args.public_output).expect("read public config JSON");
    for removed_field in [
        "trust_model",
        "max_pending_quotes",
        "pending_quote_ttl_secs",
        "stale_observation_secs",
    ] {
        assert!(!public_config_json.contains(removed_field));
    }
    assert_eq!(public_config.members.len(), 3);
    assert!(public_config.keysets.is_empty());
    assert!(public_config.setup_transcript.is_none());
    assert_eq!(
        public_config
            .checkpoint_policy
            .checkpoint_interval_journal_operations,
        cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS
    );
    assert_eq!(
        public_config
            .checkpoint_policy
            .checkpoint_interval_finalized_items,
        cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS
    );
    assert!(matches!(
        public_config.payment_observation_policy.mint_quotes,
        cdk_common::federation::FederationPaymentVerificationPolicy::MemberQuorum {
            required: 3,
            ..
        }
    ));

    for args in &member_args {
        let output = root.join(format!("member-{}-bound.json", args.member_id));
        run_dkg_bind_command(&FederationDkgBindArgs {
            public_config: roster_args.public_output.clone(),
            private_identity: args.private_output.clone(),
            output: output.clone(),
            force: false,
        })
        .expect("bind private identity");
        let private: MemberConfigPrivate =
            read_federation_json(&output, "test bound private config")
                .expect("read bound private config");
        assert_eq!(private.federation_id, Some(public_config.federation_id));
        private
            .validate_against_public(&public_config)
            .expect("bound private config validates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(&output)
                    .expect("private output metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    let ceremony =
        std::fs::read_to_string(&roster_args.ceremony_config_output).expect("read ceremony config");
    assert!(ceremony.contains("[federation.dkg]"));
    assert!(ceremony.contains("allow_insecure_localhost = true"));
    assert!(ceremony.contains("amounts = [1, 2, 4]"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn roster_rejects_duplicate_member_enrollment() {
    let root = temp_dir("duplicate");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test directory");
    let member = member_args(&root, 1);
    run_dkg_member_command(&member).expect("generate member enrollment");
    let args = roster_args(
        &root,
        vec![member.public_output.clone(), member.public_output.clone()],
    );
    let err = run_dkg_roster_command(&args).expect_err("duplicate member must fail");
    assert!(err.to_string().contains("duplicate public DKG enrollment"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn roster_rejects_zero_checkpoint_interval() {
    let root = temp_dir("zero_checkpoint_interval");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test directory");
    let member_args = (1..=3)
        .map(|member_id| member_args(&root, member_id))
        .collect::<Vec<_>>();
    for args in &member_args {
        run_dkg_member_command(args).expect("generate independent member enrollment");
    }
    let mut args = roster_args(
        &root,
        member_args
            .iter()
            .map(|args| args.public_output.clone())
            .collect(),
    );
    args.checkpoint_interval_journal_operations = 0;

    let err = run_dkg_roster_command(&args).expect_err("zero checkpoint interval must fail");
    assert!(err
        .to_string()
        .contains("invalid federation checkpoint policy"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bind_rejects_identity_for_another_roster_member() {
    let root = temp_dir("wrong_identity");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test directory");
    let members = (1..=3)
        .map(|member_id| member_args(&root, member_id))
        .collect::<Vec<_>>();
    for member in &members {
        run_dkg_member_command(member).expect("generate member enrollment");
    }
    let roster = roster_args(
        &root,
        members
            .iter()
            .map(|member| member.public_output.clone())
            .collect(),
    );
    run_dkg_roster_command(&roster).expect("assemble roster");

    let mut wrong: MemberConfigPrivate =
        read_federation_json(&members[0].private_output, "private identity")
            .expect("read private identity");
    wrong.member_id = MemberId::new(2).expect("member id");
    let wrong_path = root.join("wrong-private.json");
    write_private_json_file(&wrong_path, &wrong, false).expect("write wrong identity");
    let err = run_dkg_bind_command(&FederationDkgBindArgs {
        public_config: roster.public_output,
        private_identity: wrong_path,
        output: root.join("must-not-exist.json"),
        force: false,
    })
    .expect_err("mismatched identity must fail");
    assert!(err
        .to_string()
        .contains("private DKG identity does not match the public roster"));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn member_rejects_insecure_remote_peer_endpoint() {
    let root = temp_dir("insecure_remote");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test directory");
    let mut args = member_args(&root, 1);
    args.federation_api_url = "http://federation.example.com".to_string();
    let err = run_dkg_member_command(&args).expect_err("remote peer HTTP must fail");
    assert!(err.to_string().contains("must use HTTPS"));
    assert!(!args.public_output.exists());
    assert!(!args.private_output.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn member_rejects_colliding_outputs_before_writing() {
    let root = temp_dir("colliding_outputs");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create test directory");
    let mut args = member_args(&root, 1);
    args.private_output = args.public_output.clone();
    let err = run_dkg_member_command(&args).expect_err("colliding outputs must fail");
    assert!(err.to_string().contains("must be distinct"));
    assert!(!args.public_output.exists());
    let _ = std::fs::remove_dir_all(root);
}
