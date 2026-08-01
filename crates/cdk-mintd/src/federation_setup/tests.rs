use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use super::*;
use crate::config;
use cdk_common::federation::{FederationConfig, MemberConfigPrivate};

fn temp_output_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cdk_mintd_federation_{name}_{}",
        std::process::id()
    ))
}

fn generator_args(output_dir: PathBuf) -> FederationTrustedDealerArgs {
    FederationTrustedDealerArgs {
        output_dir,
        members: 3,
        threshold: 2,
        unit: "sat".to_string(),
        amounts: vec![1, 2, 4],
        input_fee_ppk: 0,
        final_expiry: None,
        checkpoint_interval_journal_operations:
            cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS,
        checkpoint_interval_finalized_items:
            cdk_common::federation::DEFAULT_CHECKPOINT_INTERVAL_FINALIZED_ITEMS,
        host: "127.0.0.1".to_string(),
        port_start: 8085,
        database_engine: FederationTrustedDealerDatabaseEngine::Sqlite,
        postgres_url_template: None,
        postgres_base_url: None,
        local_fakewallet_configs: false,
        deterministic_test_seed: None,
        force: false,
    }
}

#[test]
fn trusted_dealer_command_defaults_to_final_core_configs_only() {
    let output_dir = temp_output_dir("command_core_only");
    let _ = std::fs::remove_dir_all(&output_dir);
    let args = generator_args(output_dir.clone());

    run_trusted_dealer_command(&args).expect("trusted dealer command");

    assert!(output_dir.join(PUBLIC_CONFIG_FILE).exists());
    assert!(!output_dir.join(SMOKE_SCRIPT_FILE).exists());
    assert!(!output_dir.join("dkg-setup-public.json").exists());
    for member_id in 1..=args.members {
        let member_dir = output_dir.join(format!("member-{member_id}"));
        assert!(member_dir.join(PRIVATE_CONFIG_FILE).exists());
        assert!(!member_dir.join(MEMBER_CONFIG_FILE).exists());
        assert!(!member_dir.join("dkg-setup-private.json").exists());
        assert!(!member_dir.join("dkg-live-config.toml").exists());
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn local_fakewallet_generator_emits_parseable_final_configs_and_smoke() {
    let output_dir = temp_output_dir("local_fakewallet");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.local_fakewallet_configs = true;
    args.deterministic_test_seed = Some("local-fixture".to_string());

    let artifacts = generate_trusted_dealer_artifacts(&args).expect("trusted dealer artifacts");
    let public_json =
        std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
    let public_config: FederationConfig =
        serde_json::from_str(&public_json).expect("public config json");
    public_config.validate().expect("public config validates");

    let smoke = std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script");
    assert!(smoke.starts_with("#!/usr/bin/env bash"));
    assert!(smoke.contains("federation smoke-operations"));
    assert!(!smoke.contains("dkg-member"));
    assert!(!smoke.contains("dkg-roster"));
    assert!(!smoke.contains("dkg-bind"));
    assert_eq!(
        std::fs::metadata(&artifacts.smoke_script_path)
            .expect("smoke script metadata")
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    let shell_check = Command::new("bash")
        .arg("-n")
        .arg(&artifacts.smoke_script_path)
        .status()
        .expect("run bash syntax check");
    assert!(shell_check.success());

    for (member_id, member) in &artifacts.member_artifacts {
        let private_json =
            std::fs::read_to_string(&member.private_config_path).expect("private config file");
        let private_config: MemberConfigPrivate =
            serde_json::from_str(&private_json).expect("private config json");
        private_config
            .validate_against_public(&public_config)
            .expect("private config validates");
        assert_eq!(private_config.member_id, *member_id);

        let settings = config::Settings::try_new(Some(&member.config_path))
            .expect("generated member config parses");
        let federation = settings.federation.expect("federation settings");
        assert!(federation.enabled);
        assert!(!federation.dkg.enabled);
        assert_eq!(
            federation.public_config_path,
            Some(artifacts.public_config_path.clone())
        );
        assert_eq!(
            federation.private_config_path,
            Some(member.private_config_path.clone())
        );
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn local_postgres_configs_keep_member_storage_isolated() {
    let output_dir = temp_output_dir("postgres");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.local_fakewallet_configs = true;
    args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;
    args.postgres_url_template = Some("postgresql://example/member-{member}".to_string());

    let artifacts = generate_trusted_dealer_artifacts(&args).expect("PostgreSQL artifacts");
    for (member_id, member) in &artifacts.member_artifacts {
        let settings = config::Settings::try_new(Some(&member.config_path))
            .expect("generated PostgreSQL member config parses");
        let postgres = settings
            .database
            .postgres
            .expect("generated PostgreSQL database config");
        assert_eq!(
            postgres.url,
            format!("postgresql://example/member-{}", member_id.get())
        );
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn generated_templates_reject_missing_and_duplicate_markers() {
    let missing = render_generated_template("plain", &[("@@MISSING@@", "value")])
        .expect_err("missing marker must fail");
    assert!(missing.to_string().contains("occurs 0 times"));

    let duplicate = render_generated_template("@@VALUE@@ @@VALUE@@", &[("@@VALUE@@", "value")])
        .expect_err("duplicate marker occurrence must fail");
    assert!(duplicate.to_string().contains("occurs 2 times"));
}
