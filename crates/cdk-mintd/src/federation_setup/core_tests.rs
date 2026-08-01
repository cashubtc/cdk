use super::*;
use cdk_common::federation::{FederationConfig, FederationDkgSetupMethod, MemberConfigPrivate};

fn temp_output_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "cdk_mintd_federation_core_{name}_{}",
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
        port_start: 18_085,
        database_engine: FederationTrustedDealerDatabaseEngine::Sqlite,
        postgres_url_template: None,
        postgres_base_url: None,
        #[cfg(feature = "fakewallet")]
        local_fakewallet_configs: false,
        deterministic_test_seed: None,
        force: false,
    }
}

fn relative_file_contents(root: &Path) -> BTreeMap<String, String> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("read generated directory") {
            let entry = entry.expect("generated directory entry");
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("generated path under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                let contents = std::fs::read_to_string(path).expect("generated text file");
                out.insert(relative, contents);
            }
        }
    }

    let mut contents = BTreeMap::new();
    visit(root, root, &mut contents);
    contents
}

#[test]
fn trusted_dealer_core_emits_only_final_strict_configs() {
    let output_dir = temp_output_dir("artifacts");
    let _ = std::fs::remove_dir_all(&output_dir);
    let artifacts = generate_trusted_dealer_core_artifacts(&generator_args(output_dir.clone()))
        .expect("core trusted dealer artifacts");

    let public_json =
        std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
    let public_config: FederationConfig =
        serde_json::from_str(&public_json).expect("public config json");
    public_config.validate().expect("public config validates");
    assert_eq!(public_config.threshold_params.member_count, 3);
    assert_eq!(public_config.threshold_params.signature_threshold, 2);
    assert_eq!(public_config.keysets.len(), 2);
    assert_eq!(
        public_config
            .setup_transcript
            .as_ref()
            .expect("final setup transcript")
            .payload
            .setup_method,
        FederationDkgSetupMethod::TrustedDealer
    );

    assert!(!output_dir.join("dkg-setup-public.json").exists());
    assert!(!output_dir.join("dkg-setup-route-probe").exists());
    for (member_id, member) in &artifacts.member_artifacts {
        let private_json =
            std::fs::read_to_string(&member.private_config_path).expect("private config file");
        let private_config: MemberConfigPrivate =
            serde_json::from_str(&private_json).expect("private config json");
        assert_eq!(private_config.member_id, *member_id);
        private_config
            .validate_against_public(&public_config)
            .expect("private config validates");
        let member_dir = output_dir.join(format!("member-{}", member_id.get()));
        assert!(!member_dir.join("dkg-setup-private.json").exists());
        assert!(!member_dir.join("dkg-live-config.toml").exists());
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn deterministic_test_seed_reproduces_final_configs_byte_for_byte() {
    let first = temp_output_dir("deterministic_first");
    let second = temp_output_dir("deterministic_second");
    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);

    let mut first_args = generator_args(first.clone());
    first_args.deterministic_test_seed = Some("repeatable-test-fixture".to_string());
    let mut second_args = generator_args(second.clone());
    second_args.deterministic_test_seed = first_args.deterministic_test_seed.clone();
    generate_trusted_dealer_core_artifacts(&first_args).expect("first generation");
    generate_trusted_dealer_core_artifacts(&second_args).expect("second generation");

    assert_eq!(
        relative_file_contents(&first),
        relative_file_contents(&second)
    );
    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);
}

#[test]
fn trusted_dealer_rejects_ambiguous_postgres_configuration() {
    let output_dir = temp_output_dir("postgres_validation");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;
    args.postgres_url_template = Some("postgresql://example/{member}".to_string());
    args.postgres_base_url = Some("postgresql://example/base".to_string());

    let error = generate_trusted_dealer_core_artifacts(&args)
        .expect_err("ambiguous PostgreSQL configuration must fail");
    assert!(error.to_string().contains("use either"));
    let _ = std::fs::remove_dir_all(&output_dir);
}
