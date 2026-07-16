use super::*;
use cdk_common::federation::{FederationConfig, MemberConfigPrivate};

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
        dkg_setup_route_probes: true,
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
                continue;
            }

            let relative_path = path
                .strip_prefix(root)
                .expect("generated path is under root")
                .to_string_lossy()
                .replace('\\', "/");
            let contents = std::fs::read_to_string(&path).expect("generated text file");
            out.insert(relative_path, contents);
        }
    }

    let mut contents = BTreeMap::new();
    visit(root, root, &mut contents);
    contents
}

fn assert_relative_file_contents_equal(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) {
    if left.keys().collect::<Vec<_>>() != right.keys().collect::<Vec<_>>() {
        panic!("deterministic generation produced different relative file sets");
    }

    for (path, left_contents) in left {
        let right_contents = right
            .get(path)
            .expect("right file set contains the same relative path");
        if left_contents != right_contents {
            panic!("deterministic generation produced different contents for {path}");
        }
    }
}

#[test]
fn trusted_dealer_core_artifacts_are_backend_agnostic() {
    let output_dir = temp_output_dir("artifacts");
    let _ = std::fs::remove_dir_all(&output_dir);
    let args = generator_args(output_dir.clone());
    let artifacts =
        generate_trusted_dealer_core_artifacts(&args).expect("core trusted dealer artifacts");

    let public_json =
        std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
    assert!(public_json.contains("development_single_observation"));
    assert!(!public_json.contains("\"fake_wallet\""));
    let public_config: FederationConfig =
        serde_json::from_str(&public_json).expect("public config json");
    public_config.validate().expect("public config validates");
    assert_eq!(public_config.threshold_params.member_count, 3);
    assert_eq!(public_config.threshold_params.signature_threshold, 2);
    assert_eq!(public_config.keysets.len(), 2);
    let setup_transcript = public_config
        .setup_transcript
        .as_ref()
        .expect("setup transcript");

    let setup_public_json = std::fs::read_to_string(&artifacts.dkg_setup_public_config_path)
        .expect("setup public config file");
    let setup_public_config: FederationConfig =
        serde_json::from_str(&setup_public_json).expect("setup public config json");
    setup_public_config
        .validate()
        .expect("setup public config validates");
    assert_eq!(
        setup_public_config.federation_id,
        public_config.federation_id
    );
    assert!(setup_public_config.keysets.is_empty());
    assert!(setup_public_config.setup_transcript.is_none());

    assert!(!output_dir.join("smoke-local-federation.sh").exists());
    for member_id in public_config.members.keys() {
        let member_artifacts = artifacts
            .member_artifacts
            .get(member_id)
            .expect("member core artifacts");
        assert!(!output_dir
            .join(format!("member-{}", member_id.get()))
            .join("config.toml")
            .exists());
        assert!(!output_dir
            .join(format!("member-{}", member_id.get()))
            .join("dkg-setup-config.toml")
            .exists());

        let private_json = std::fs::read_to_string(&member_artifacts.private_config_path)
            .expect("private config file");
        let private_config: MemberConfigPrivate =
            serde_json::from_str(&private_json).expect("private config json");
        assert_eq!(private_config.member_id, *member_id);
        assert_eq!(
            private_config.federation_id,
            Some(public_config.federation_id)
        );
        assert_eq!(
            private_config.setup_transcript_hash,
            Some(setup_transcript.transcript_hash)
        );
        private_config
            .validate_against_public(&public_config)
            .expect("private config validates");

        let setup_private_json =
            std::fs::read_to_string(&member_artifacts.dkg_setup_private_config_path)
                .expect("setup private config file");
        let setup_private_config: MemberConfigPrivate =
            serde_json::from_str(&setup_private_json).expect("setup private config json");
        assert_eq!(setup_private_config.member_id, *member_id);
        assert_eq!(
            setup_private_config.federation_id,
            Some(public_config.federation_id)
        );
        assert!(setup_private_config.keyset_secret_shares.is_empty());
        assert!(setup_private_config.setup_transcript_hash.is_none());
        setup_private_config
            .validate_against_public(&setup_public_config)
            .expect("setup private config validates");
    }

    let first_member = MemberId::new(1).expect("first member id");
    let first_probe_path = artifacts
        .member_artifacts
        .get(&first_member)
        .expect("first member artifacts")
        .dkg_setup_request_dir
        .join("from-1-commitment.json");
    let commitment_json =
        std::fs::read_to_string(first_probe_path).expect("DKG commitment route-probe request");
    let commitment_request: FederationSignedRequest<FederationDkgCommitmentRequest> =
        serde_json::from_str(&commitment_json).expect("signed DKG commitment request");
    commitment_request
        .verify(&public_config, first_member)
        .expect("commitment request verifies");
    commitment_request
        .payload
        .validate_sender(&public_config, first_member)
        .expect("commitment sender validates");

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn deterministic_test_seed_reproduces_core_artifacts_byte_for_byte() {
    let first_output_dir = temp_output_dir("deterministic_first");
    let second_output_dir = temp_output_dir("deterministic_second");
    let third_output_dir = temp_output_dir("deterministic_third");
    let _ = std::fs::remove_dir_all(&first_output_dir);
    let _ = std::fs::remove_dir_all(&second_output_dir);
    let _ = std::fs::remove_dir_all(&third_output_dir);

    let mut first_args = generator_args(first_output_dir.clone());
    first_args.deterministic_test_seed = Some("repeatable federation setup".to_string());
    let mut second_args = generator_args(second_output_dir.clone());
    second_args.deterministic_test_seed = first_args.deterministic_test_seed.clone();
    let mut third_args = generator_args(third_output_dir.clone());
    third_args.deterministic_test_seed = Some("different federation setup".to_string());

    generate_trusted_dealer_core_artifacts(&first_args)
        .expect("first deterministic trusted dealer artifacts");
    generate_trusted_dealer_core_artifacts(&second_args)
        .expect("second deterministic trusted dealer artifacts");
    generate_trusted_dealer_core_artifacts(&third_args)
        .expect("third deterministic trusted dealer artifacts");

    assert_relative_file_contents_equal(
        &relative_file_contents(&first_output_dir),
        &relative_file_contents(&second_output_dir),
    );
    let first_public_config = std::fs::read_to_string(first_output_dir.join(PUBLIC_CONFIG_FILE))
        .expect("first public config");
    let third_public_config = std::fs::read_to_string(third_output_dir.join(PUBLIC_CONFIG_FILE))
        .expect("third public config");
    assert_ne!(first_public_config, third_public_config);

    let _ = std::fs::remove_dir_all(&first_output_dir);
    let _ = std::fs::remove_dir_all(&second_output_dir);
    let _ = std::fs::remove_dir_all(&third_output_dir);
}
