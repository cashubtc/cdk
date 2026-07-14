use std::collections::BTreeSet;

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
        host: "127.0.0.1".to_string(),
        port_start: 8085,
        database_engine: FederationTrustedDealerDatabaseEngine::Sqlite,
        postgres_url_template: None,
        postgres_base_url: None,
        dkg_setup_route_probes: false,
        local_fakewallet_configs: false,
        deterministic_test_seed: None,
        force: false,
    }
}

#[test]
fn trusted_dealer_command_defaults_to_core_only_with_fakewallet_feature() {
    let output_dir = temp_output_dir("command_core_only");
    let _ = std::fs::remove_dir_all(&output_dir);
    let args = generator_args(output_dir.clone());

    run_trusted_dealer_command(&args).expect("trusted dealer command");

    assert!(output_dir.join(PUBLIC_CONFIG_FILE).exists());
    assert!(output_dir.join(DKG_SETUP_PUBLIC_CONFIG_FILE).exists());
    assert!(!output_dir.join(SMOKE_SCRIPT_FILE).exists());
    assert!(!output_dir.join(LIVE_DKG_SMOKE_SCRIPT_FILE).exists());
    for member_id in 1..=args.members {
        let member_dir = output_dir.join(format!("member-{member_id}"));
        assert!(member_dir.join(PRIVATE_CONFIG_FILE).exists());
        assert!(member_dir.join(DKG_SETUP_PRIVATE_CONFIG_FILE).exists());
        assert!(!member_dir.join(MEMBER_CONFIG_FILE).exists());
        assert!(!member_dir.join(DKG_SETUP_MEMBER_CONFIG_FILE).exists());
        assert!(!member_dir.join(LIVE_DKG_MEMBER_CONFIG_FILE).exists());
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn trusted_dealer_command_generates_local_fakewallet_files_only_when_requested() {
    let output_dir = temp_output_dir("command_local_fakewallet");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.local_fakewallet_configs = true;

    run_trusted_dealer_command(&args).expect("trusted dealer command");

    assert!(output_dir.join(PUBLIC_CONFIG_FILE).exists());
    assert!(output_dir.join(DKG_SETUP_PUBLIC_CONFIG_FILE).exists());
    assert!(output_dir.join(SMOKE_SCRIPT_FILE).exists());
    assert!(output_dir.join(LIVE_DKG_SMOKE_SCRIPT_FILE).exists());
    for member_id in 1..=args.members {
        let member_dir = output_dir.join(format!("member-{member_id}"));
        assert!(member_dir.join(PRIVATE_CONFIG_FILE).exists());
        assert!(member_dir.join(DKG_SETUP_PRIVATE_CONFIG_FILE).exists());
        assert!(member_dir.join(MEMBER_CONFIG_FILE).exists());
        assert!(member_dir.join(DKG_SETUP_MEMBER_CONFIG_FILE).exists());
        assert!(member_dir.join(LIVE_DKG_MEMBER_CONFIG_FILE).exists());
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn fakewallet_is_documented_as_development_backend() {
    let docs = [
        include_str!("../../../../docs/federated-cashu-consensus-current.md"),
        include_str!("../../../../docs/federated-cashu-consensus-current.html"),
        include_str!("../../../../docs/federated-cashu-progress.html"),
    ];
    let forbidden_phrases = [
        "core fake-wallet eCash flows",
        "Core fake-wallet flows",
        "fake-wallet phase",
        "fake-wallet federation path",
        "current fake-wallet path",
        "fake wallet semantics",
        "fake-wallet member",
        "federated fake-wallet",
        "fake-wallet melt",
        "fake-wallet payment",
        "fake-wallet flows",
        "fake-wallet path",
    ];

    for doc in docs {
        for phrase in forbidden_phrases {
            assert!(
                !doc.contains(phrase),
                "federation docs must not describe fakewallet as federation semantics: {phrase}"
            );
        }
    }
}

#[test]
fn trusted_dealer_artifacts_are_written_and_validated() {
    let output_dir = temp_output_dir("artifacts with spaces");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.dkg_setup_route_probes = true;
    let artifacts = generate_trusted_dealer_artifacts(&args).expect("trusted dealer artifacts");

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
            .keysets
            .iter()
            .filter(|keyset| keyset.active)
            .count(),
        1
    );
    assert_eq!(
        public_config
            .keysets
            .iter()
            .filter(|keyset| !keyset.active)
            .count(),
        1
    );
    let setup_transcript = public_config
        .setup_transcript
        .as_ref()
        .expect("setup transcript");
    let dkg_setup_public_json = std::fs::read_to_string(&artifacts.dkg_setup_public_config_path)
        .expect("setup-only public config file");
    let dkg_setup_public_config: FederationConfig =
        serde_json::from_str(&dkg_setup_public_json).expect("setup-only public config json");
    dkg_setup_public_config
        .validate()
        .expect("setup-only public config validates");
    assert_eq!(
        dkg_setup_public_config.federation_id,
        public_config.federation_id
    );
    assert!(dkg_setup_public_config.keysets.is_empty());
    assert!(dkg_setup_public_config.setup_transcript.is_none());
    let first_member = MemberId::new(1).expect("first member id");
    let second_member = MemberId::new(2).expect("second member id");
    let first_receiver_probe_dir = output_dir
        .join(DKG_SETUP_ROUTE_PROBE_DIR)
        .join("receiver-1");
    let commitment_json =
        std::fs::read_to_string(first_receiver_probe_dir.join("from-1-commitment.json"))
            .expect("DKG commitment route-probe request");
    let commitment_request: FederationSignedRequest<FederationDkgCommitmentRequest> =
        serde_json::from_str(&commitment_json).expect("signed DKG commitment request");
    commitment_request
        .verify(&public_config, first_member)
        .expect("commitment request verifies");
    commitment_request
        .payload
        .validate_sender(&public_config, first_member)
        .expect("commitment sender validates");
    assert_eq!(commitment_request.sequence, 1);
    assert_eq!(
        commitment_request.kind,
        FederationRestMessageKind::DkgCommitment
    );
    let reveal_json = std::fs::read_to_string(first_receiver_probe_dir.join("from-1-reveal.json"))
        .expect("DKG reveal route-probe request");
    let reveal_request: FederationSignedRequest<FederationDkgRevealRequest> =
        serde_json::from_str(&reveal_json).expect("signed DKG reveal request");
    reveal_request
        .verify(&public_config, first_member)
        .expect("reveal request verifies");
    reveal_request
        .payload
        .validate_sender(&public_config, first_member)
        .expect("reveal sender validates");
    assert_eq!(reveal_request.sequence, 2);
    assert_eq!(reveal_request.kind, FederationRestMessageKind::DkgReveal);
    let secret_share_json =
        std::fs::read_to_string(first_receiver_probe_dir.join("from-2-secret-share.json"))
            .expect("DKG secret-share route-probe request");
    let secret_share_request: FederationSignedRequest<FederationDkgSecretShareRequest> =
        serde_json::from_str(&secret_share_json).expect("signed DKG secret-share request");
    secret_share_request
        .verify(&public_config, first_member)
        .expect("secret-share request verifies");
    secret_share_request
        .payload
        .validate_sender_receiver(&public_config, second_member, first_member)
        .expect("secret-share sender and receiver validate");
    assert_eq!(secret_share_request.sequence, 3);
    assert_eq!(
        secret_share_request.kind,
        FederationRestMessageKind::DkgSecretShare
    );
    let transcript_signature_json =
        std::fs::read_to_string(first_receiver_probe_dir.join("from-2-transcript-signature.json"))
            .expect("DKG transcript-signature route-probe request");
    let transcript_signature_request: FederationSignedRequest<
        FederationDkgTranscriptSignatureRequest,
    > = serde_json::from_str(&transcript_signature_json)
        .expect("signed DKG transcript-signature request");
    transcript_signature_request
        .verify(&public_config, first_member)
        .expect("transcript-signature request verifies");
    transcript_signature_request
        .payload
        .validate_sender(&public_config, second_member)
        .expect("transcript-signature sender validates");
    assert_eq!(transcript_signature_request.sequence, 4);
    assert_eq!(
        transcript_signature_request.kind,
        FederationRestMessageKind::DkgTranscriptSignature
    );

    let smoke_script =
        std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
    let live_dkg_smoke_script = std::fs::read_to_string(&artifacts.live_dkg_smoke_script_path)
        .expect("live DKG smoke script file");
    assert!(smoke_script.starts_with("#!/usr/bin/env bash"));
    assert!(live_dkg_smoke_script.starts_with("#!/usr/bin/env bash"));
    for script_path in [
        &artifacts.smoke_script_path,
        &artifacts.live_dkg_smoke_script_path,
    ] {
        let syntax = std::process::Command::new("bash")
            .arg("-n")
            .arg(script_path)
            .status()
            .expect("run bash syntax validation");
        assert!(
            syntax.success(),
            "generated smoke script must parse as bash"
        );
    }
    assert!(smoke_script.contains("federation status"));
    assert!(smoke_script.contains("federation dkg-status"));
    assert!(smoke_script.contains("federation payment-observations"));
    assert!(smoke_script.contains("federation smoke-operations"));
    assert!(smoke_script.contains("--require-ready"));
    assert!(smoke_script.contains("--from-database"));
    assert!(smoke_script.contains("member_status_line()"));
    assert!(smoke_script.contains("member_dkg_status()"));
    assert!(smoke_script.contains("compare_digests()"));
    assert!(smoke_script.contains("local signer=\"${MEMBERS[0]}\""));
    assert!(smoke_script.contains("private_config=\"$(member_private_config \"$signer\")\""));
    assert!(smoke_script.contains("federation compare-digests"));
    assert!(smoke_script.contains("compare_digests >/dev/null 2>&1"));
    assert!(smoke_script.contains("compare_digests >&2 || true"));
    assert!(smoke_script
        .contains("--work-dir \"$work_dir\" --config \"$config\" federation dkg-status"));
    assert!(smoke_script.contains("PROBE_DKG_SETUP_ROUTES"));
    assert!(smoke_script.contains("DKG_ROUTE_PROBE_ONLY"));
    assert!(smoke_script.contains("FINAL_PUBLIC_CONFIG=\"$PUBLIC_CONFIG\""));
    assert!(smoke_script.contains("USE_DKG_SETUP_CONFIGS=0"));
    assert!(smoke_script.contains("DKG_SETUP_PUBLIC_CONFIG"));
    assert!(smoke_script.contains("PUBLIC_CONFIG=\"$DKG_SETUP_PUBLIC_CONFIG\""));
    assert!(smoke_script.contains("PUBLIC_CONFIG=\"$FINAL_PUBLIC_CONFIG\""));
    assert!(smoke_script.contains("USE_DKG_SETUP_CONFIGS=1"));
    assert!(smoke_script.contains("PROBE_DKG_SETUP_ROUTES=1"));
    assert!(smoke_script.contains("dkg setup route probe passed"));
    assert!(smoke_script.contains("CDK_FEDERATION_DISABLE_PEER_SYNC=1"));
    assert!(!smoke_script.contains("FEDERATION_CONSENSUS_BACKEND"));
    assert!(smoke_script.contains("member_dkg_setup_request_dir()"));
    assert!(smoke_script.contains("member_mint_url()"));
    assert!(smoke_script.contains("member_federation_url()"));
    assert!(smoke_script.contains("probe_dkg_setup_routes()"));
    assert!(smoke_script.contains("post_dkg_setup_request()"));
    assert!(smoke_script.contains("wait_for_dkg_setup_records()"));
    assert!(smoke_script.contains("wait_for_runtime_keysets()"));
    assert!(smoke_script.contains("$mint_url/v1/keysets"));
    assert!(smoke_script.contains("did not expose activated DKG keysets after restart"));
    assert!(smoke_script.contains("member_dkg_completion_records_line()"));
    assert!(smoke_script.contains("could not post DKG setup route-probe request"));
    assert!(smoke_script.contains("/federation/v1/dkg/commitments"));
    assert!(smoke_script.contains("/federation/v1/dkg/reveals"));
    assert!(smoke_script.contains("/federation/v1/dkg/secret-shares"));
    assert!(smoke_script.contains("/federation/v1/dkg/transcript-signatures"));
    assert!(smoke_script.contains("keyset_share_records"));
    assert!(smoke_script.contains("transcript_signature_records"));
    assert!(smoke_script.contains(
        "probe_dkg_setup_routes\nwait_for_dkg_setup_records\nif [[ \"$PROBE_DKG_SETUP_ROUTES\" == \"1\" ]]; then"
        ));
    assert!(smoke_script.contains("wait_for_dkg_status \"$member\""));
    assert!(smoke_script.contains(
            "stop_member \"$member\"\n  done\n  for member in \"${MEMBERS[@]}\"; do\n    start_member \"$member\""
        ));
    assert!(smoke_script.contains("wait_for_runtime_keysets \"$member\""));
    assert!(smoke_script.contains("if [[ \"$DKG_ROUTE_PROBE_ONLY\" == \"1\" ]]; then"));
    assert!(
        smoke_script.contains("USE_DKG_SETUP_CONFIGS=0\n  PUBLIC_CONFIG=\"$FINAL_PUBLIC_CONFIG\"")
    );
    assert!(smoke_script.contains("reset_member_work_dirs\n  for member in"));
    assert!(smoke_script.contains("wait_for_dkg_status()"));
    assert!(smoke_script.contains("print_dkg_record_runbook()"));
    assert!(smoke_script.contains("DKG record recovery for member"));
    assert!(smoke_script.contains("--keyset-share <keyset-share.json>"));
    assert!(smoke_script.contains("--transcript-signature <transcript-signature.json>"));
    assert!(smoke_script.contains("setup_records with collected commitment"));
    assert!(smoke_script.contains("Combine --from-database"));
    assert!(smoke_script.contains("member_quorum_status_line()"));
    assert!(smoke_script.contains("--require-catch-up-certificate-quorum"));
    assert!(smoke_script.contains("--require-no-digest-alerts"));
    assert!(smoke_script.contains("--require-no-invalid-operation-alerts"));
    assert!(smoke_script.contains("private_config=\"$(member_private_config \"$member\")\""));
    assert!(
        !smoke_script.contains("private_config=\"$(member_private_config \"$PRIMARY_MEMBER\")\"")
    );
    assert!(smoke_script.contains("reset_member_work_dirs"));
    assert!(smoke_script.contains("rm -rf \"$work_dir\""));
    assert!(smoke_script.contains("PRIMARY_MEMBER"));
    assert!(smoke_script.contains("RESTART_MEMBER"));
    assert!(smoke_script.contains("QUARANTINE_OFFLINE_MEMBER"));
    assert!(smoke_script.contains("CRASH_WINDOW_MEMBER"));
    assert!(smoke_script.contains("LONG_PARTITION_MEMBER"));
    assert!(smoke_script.contains("EXERCISE_CRASH_WINDOW"));
    assert!(smoke_script.contains("unknown RESTART_MEMBER"));
    assert!(smoke_script
        .contains("RESTART_MEMBER must differ from PRIMARY_MEMBER for offline operation smoke"));
    assert!(smoke_script.contains("unknown CRASH_WINDOW_MEMBER"));
    assert!(smoke_script.contains("unknown LONG_PARTITION_MEMBER"));
    assert!(smoke_script.contains(
        "LONG_PARTITION_MEMBER must differ from PRIMARY_MEMBER for long-partition operation smoke"
    ));
    assert!(smoke_script.contains("PENDING_WALLET_DB"));
    assert!(smoke_script.contains("FAILED_PENDING_WALLET_DB"));
    assert!(smoke_script.contains("SMOKE_QUOTE_RETRIES"));
    assert!(smoke_script.contains("SMOKE_QUOTE_SLEEP_MS"));
    assert!(smoke_script.contains("SMOKE_OPERATION_TIMEOUT_SECONDS"));
    assert!(smoke_script.contains("STANDARD_OPERATION_BATCHES"));
    assert!(smoke_script.contains("POST_CRASH_SEED_OFFSET"));
    assert!(smoke_script.contains("POST_CRASH_OPERATION_BATCHES"));
    assert!(smoke_script.contains("LONG_PARTITION_SEED_OFFSET"));
    assert!(smoke_script.contains("LONG_PARTITION_OPERATION_BATCHES"));
    assert!(smoke_script.contains("CHECK_PROMETHEUS_METRICS"));
    assert!(smoke_script.contains("CDK_MINTD_PROMETHEUS_ENABLED=false"));
    assert!(smoke_script.contains("local mintd_env_count=0"));
    assert!(smoke_script.contains("if (( mintd_env_count == 0 )); then"));
    assert!(
        smoke_script.contains("\"$CDK_MINTD_BIN\" --work-dir \"$work_dir\" --config \"$config\"")
    );
    assert!(smoke_script.contains("env \"${mintd_env[@]}\" \"$CDK_MINTD_BIN\""));
    assert!(live_dkg_smoke_script.contains("DKG_SETUP_PHASE=1"));
    assert!(live_dkg_smoke_script.contains("DKG_CAPTURE_METRICS"));
    assert!(live_dkg_smoke_script.contains("DKG_METRICS_INTERVAL_SECONDS"));
    assert!(live_dkg_smoke_script.contains("sample_dkg_metrics"));
    assert!(live_dkg_smoke_script
        .contains("sample_dkg_metrics() {\n  if [[ \"$DKG_CAPTURE_METRICS\" != \"1\" ]]; then"));
    assert!(live_dkg_smoke_script.contains("record_metrics_member_pid"));
    assert!(live_dkg_smoke_script.contains("member-$member.pid"));
    assert!(live_dkg_smoke_script.contains("average_cpu_percent"));
    assert!(live_dkg_smoke_script.contains("total_user_seconds"));
    assert!(live_dkg_smoke_script.contains("total_system_seconds"));
    assert!(live_dkg_smoke_script.contains("dkg_rest samples="));
    assert!(live_dkg_smoke_script.contains("dkg_metrics_result="));
    assert!(live_dkg_smoke_script.contains("report_dkg_metrics setup_failure"));
    assert!(live_dkg_smoke_script.contains("DKG_METRICS_REPORTED"));
    assert!(live_dkg_smoke_script.contains("trap cleanup EXIT"));
    assert!(live_dkg_smoke_script.contains("trap handle_signal INT TERM"));
    assert!(live_dkg_smoke_script.contains("exit 130"));
    assert!(live_dkg_smoke_script.contains("startup_failures="));
    assert!(live_dkg_smoke_script.contains("runtime_failures="));
    assert!(live_dkg_smoke_script.contains("Federation DKG driver tick metrics"));
    assert!(live_dkg_smoke_script.contains("duplicate_successes="));
    assert!(live_dkg_smoke_script.contains(">> \"$work_dir/mintd.log\""));
    assert!(live_dkg_smoke_script.contains("HELD_MEMBER"));
    assert!(live_dkg_smoke_script.contains("INTERRUPTED_MEMBER"));
    assert!(live_dkg_smoke_script.contains("wait_for_interrupt_checkpoint"));
    assert!(live_dkg_smoke_script.contains("wait_for_dkg_driver_metric_completion"));
    assert!(live_dkg_smoke_script.contains("INTERRUPT_CHECKPOINT_RETRIES:-2400"));
    assert!(live_dkg_smoke_script.contains("INTERRUPT_CHECKPOINT_SLEEP_SECONDS=0.05"));
    assert!(live_dkg_smoke_script.contains("DKG_EXPORT_JOBS:-4"));
    assert!(live_dkg_smoke_script.contains("DKG_START_JOBS:-2"));
    assert!(live_dkg_smoke_script.contains("RUNTIME_START_JOBS:-2"));
    assert!(live_dkg_smoke_script.contains("start_dkg_members_bounded"));
    assert!(live_dkg_smoke_script.contains("wait_for_dkg_listener"));
    assert!(live_dkg_smoke_script.contains("if (( ${#batch[@]} > 0 )); then"));
    assert!(
        live_dkg_smoke_script.contains("exited before persisting a complete live DKG record set")
    );
    assert!(live_dkg_smoke_script.contains("start_runtime_members_bounded"));
    assert!(live_dkg_smoke_script.contains("recovered_members=\" $* \""));
    assert!(live_dkg_smoke_script.contains("wait_for_converged \"$INTERRUPTED_MEMBER\""));
    assert!(live_dkg_smoke_script.contains("export_final_configs"));
    assert!(live_dkg_smoke_script.contains("member %s failed to export its final DKG config"));
    assert!(live_dkg_smoke_script.contains("keyset_shares || true)\" == \"0"));
    assert!(live_dkg_smoke_script
        .contains("live DKG completed while a required participant was offline"));
    assert!(live_dkg_smoke_script.contains("participant_state_ceremonies"));
    assert!(live_dkg_smoke_script.contains("--final-config-dir"));
    assert!(live_dkg_smoke_script.contains("--frost-cdk-network regtest"));
    assert!(live_dkg_smoke_script.contains("--frost-cdk-minimum-confirmations 1"));
    assert!(live_dkg_smoke_script.contains("member_frost_bdk_config"));
    assert!(live_dkg_smoke_script.contains("run_frost_bdk_regtest"));
    assert!(live_dkg_smoke_script.contains("run_onchain_mint"));
    assert!(live_dkg_smoke_script.contains("run_onchain_melt"));
    assert!(live_dkg_smoke_script.contains("capture_frost_bdk_checkpoint"));
    assert!(live_dkg_smoke_script.contains("FROST_BDK_MEMBER_REQUEST_TIMEOUT_SECONDS:-60"));
    assert!(live_dkg_smoke_script.contains("FROST_BDK_MEMBER_REQUEST_MAX_RETRIES:-200"));
    assert!(live_dkg_smoke_script.contains("FROST_BDK_LONG_IDLE_SECONDS:-0"));
    assert!(live_dkg_smoke_script.contains("FROST_BDK_LONG_IDLE_SECONDS < 60"));
    assert!(live_dkg_smoke_script.contains("capture_frost_bdk_checkpoint 01-after-long-idle"));
    assert!(live_dkg_smoke_script.contains("FROST_BDK_CAPTURE_RUNTIME_METRICS:-0"));
    assert!(live_dkg_smoke_script.contains("member_prometheus_url()"));
    assert!(live_dkg_smoke_script.contains("capture_runtime_metrics_snapshot idle-start"));
    assert!(live_dkg_smoke_script.contains("report_runtime_metrics_window active"));
    assert!(live_dkg_smoke_script.contains("ps -p \"$pid\" -o time= -o rss="));
    assert!(live_dkg_smoke_script.contains("schema_version=1 profile=%s window=%s"));
    assert!(live_dkg_smoke_script.contains("max_end_rss_kb=%d"));
    assert!(live_dkg_smoke_script
        .contains("runtime metrics capture requires a dev or release build profile"));
    assert!(live_dkg_smoke_script
        .contains("--member-request-timeout-secs \"$FROST_BDK_MEMBER_REQUEST_TIMEOUT_SECONDS\""));
    assert!(live_dkg_smoke_script
        .contains("--member-request-max-retries \"$FROST_BDK_MEMBER_REQUEST_MAX_RETRIES\""));
    assert!(live_dkg_smoke_script.contains("FROST_BDK_FAULT_PROFILE"));
    assert!(live_dkg_smoke_script.contains("below-threshold-heal"));
    assert!(live_dkg_smoke_script.contains("process-kill"));
    assert!(live_dkg_smoke_script.contains("--onchain-quote-index 0"));
    assert!(live_dkg_smoke_script
        .contains("expected exactly two unique deposit and two unique melt transactions"));
    assert!(live_dkg_smoke_script.contains("compare_final_public_configs"));
    assert!(live_dkg_smoke_script.contains("DKG_SETUP_PHASE=0"));
    assert!(!live_dkg_smoke_script.contains("FEDERATION_CONSENSUS_BACKEND"));
    assert!(live_dkg_smoke_script.contains("wait_for_runtime_keysets"));
    assert!(live_dkg_smoke_script.contains("federation smoke-operations"));
    assert!(live_dkg_smoke_script.contains("--scenario standard"));
    assert!(
        live_dkg_smoke_script.contains("live dealer-free DKG mint/swap/melt/restore smoke passed")
    );
    assert!(smoke_script.contains("validate_smoke_settings()"));
    assert!(smoke_script.contains("require_positive_integer()"));
    assert!(smoke_script.contains("require_nonnegative_integer()"));
    assert!(smoke_script.contains("printf '%s must be a positive integer"));
    assert!(smoke_script.contains("printf '%s must be a non-negative integer"));
    assert!(smoke_script.contains("require_positive_integer STATUS_RETRIES"));
    assert!(smoke_script.contains("require_positive_integer STATUS_SLEEP_SECONDS"));
    assert!(smoke_script.contains("require_positive_integer SMOKE_QUOTE_RETRIES"));
    assert!(smoke_script.contains("require_positive_integer SMOKE_QUOTE_SLEEP_MS"));
    assert!(smoke_script.contains("require_positive_integer SMOKE_OPERATION_TIMEOUT_SECONDS"));
    assert!(smoke_script.contains("require_positive_integer STANDARD_OPERATION_BATCHES"));
    assert!(smoke_script.contains("STANDARD_SEED_OFFSET must be 255 or lower"));
    assert!(smoke_script
        .contains("STANDARD_SEED_OFFSET plus STANDARD_OPERATION_BATCHES must be 256 or lower"));
    assert!(smoke_script.contains("require_nonnegative_integer POST_CRASH_SEED_OFFSET"));
    assert!(
        smoke_script.contains("POST_CRASH_SEED_OFFSET must be after the standard smoke seed range")
    );
    assert!(smoke_script.contains("require_positive_integer POST_CRASH_OPERATION_BATCHES"));
    assert!(smoke_script
        .contains("POST_CRASH_SEED_OFFSET plus POST_CRASH_OPERATION_BATCHES must be 256 or lower"));
    assert!(smoke_script.contains("if [[ -z \"$LONG_PARTITION_SEED_OFFSET\" ]]; then"));
    assert!(smoke_script.contains(
        "LONG_PARTITION_SEED_OFFSET=$((POST_CRASH_SEED_OFFSET + POST_CRASH_OPERATION_BATCHES))"
    ));
    assert!(smoke_script.contains("require_nonnegative_integer LONG_PARTITION_SEED_OFFSET"));
    assert!(smoke_script
        .contains("LONG_PARTITION_SEED_OFFSET must be after the standard smoke seed range"));
    assert!(smoke_script.contains(
        "LONG_PARTITION_SEED_OFFSET plus LONG_PARTITION_OPERATION_BATCHES must not overlap the post-crash seed range"
    ));
    assert!(smoke_script.contains("require_positive_integer LONG_PARTITION_OPERATION_BATCHES"));
    assert!(smoke_script.contains(
        "LONG_PARTITION_SEED_OFFSET plus LONG_PARTITION_OPERATION_BATCHES must be 256 or lower"
    ));
    assert!(smoke_script.contains("member_payment_report_lines()"));
    assert!(smoke_script.contains("--context mint-quote"));
    assert!(smoke_script.contains("member_prometheus_url()"));
    assert!(smoke_script.contains("run_smoke_operations()"));
    assert!(smoke_script.contains("local has_private_config=0"));
    assert!(smoke_script.contains(
        "local timeout_args=(--operation-timeout-seconds \"$SMOKE_OPERATION_TIMEOUT_SECONDS\")"
    ));
    assert!(smoke_script.contains("if [[ \"$arg\" == \"--private-config\" ]]; then"));
    assert!(smoke_script.contains("--private-config \"$(member_private_config \"$signer\")\""));
    assert!(smoke_script.contains("return\n  fi"));
    assert!(smoke_script.contains("wait_for_prometheus_metrics()"));
    assert!(smoke_script.contains("prometheus_metrics_are_ready()"));
    assert!(smoke_script.contains("cdk_federation_consensus_operation_count"));
    assert!(smoke_script.contains("cdk_federation_pending_mempool_items"));
    assert!(smoke_script.contains("cdk_federation_sync_ready"));
    assert!(smoke_script.contains("cdk_federation_sync_state"));
    assert!(smoke_script.contains("cdk_federation_payment_observation_reports"));
    assert!(smoke_script.contains("federation Prometheus metrics include a sensitive label"));
    assert!(smoke_script.contains("wait_for_catch_up_certificate_quorum()"));
    assert!(smoke_script.contains("member %s did not report quorum-backed catch-up certificates"));
    assert!(smoke_script.contains("--quote-retries \"$SMOKE_QUOTE_RETRIES\""));
    assert!(smoke_script.contains("--quote-sleep-ms \"$SMOKE_QUOTE_SLEEP_MS\""));
    assert!(smoke_script.contains("\"${timeout_args[@]}\""));
    assert!(smoke_script.contains("wait_for_quarantine_report()"));
    assert!(smoke_script.contains("validate_smoke_members()"));
    assert!(smoke_script.contains("--scenario standard"));
    assert!(smoke_script.contains("--operation-batches \"$STANDARD_OPERATION_BATCHES\""));
    assert!(smoke_script.contains("if [[ \"$EXERCISE_CRASH_WINDOW\" == \"1\" ]]; then"));
    assert!(smoke_script.contains("stop_member \"$CRASH_WINDOW_MEMBER\""));
    assert!(smoke_script.contains("start_member \"$CRASH_WINDOW_MEMBER\""));
    assert!(smoke_script.contains("wait_for_status \"$CRASH_WINDOW_MEMBER\""));
    assert!(smoke_script.contains("wait_for_catch_up_certificate_quorum \"$CRASH_WINDOW_MEMBER\""));
    assert!(smoke_script.contains("--seed-offset \"$POST_CRASH_SEED_OFFSET\""));
    assert!(smoke_script.contains("--operation-batches \"$POST_CRASH_OPERATION_BATCHES\""));
    assert!(smoke_script.contains("--scenario offline-member-operations"));
    assert!(smoke_script.contains("stop_member \"$LONG_PARTITION_MEMBER\""));
    assert!(smoke_script.contains("--seed-offset \"$LONG_PARTITION_SEED_OFFSET\""));
    assert!(!smoke_script.contains("--seed-offset 1"));
    assert!(smoke_script.contains("--operation-batches \"$LONG_PARTITION_OPERATION_BATCHES\""));
    assert!(smoke_script.contains("start_member \"$LONG_PARTITION_MEMBER\""));
    assert!(smoke_script.contains("wait_for_status \"$LONG_PARTITION_MEMBER\""));
    assert!(
        smoke_script.contains("wait_for_catch_up_certificate_quorum \"$LONG_PARTITION_MEMBER\"")
    );
    assert!(smoke_script.contains("--scenario pending-melt-start"));
    assert!(smoke_script.contains("--scenario pending-melt-finish"));
    assert!(smoke_script.contains("--scenario pending-melt-fail-start"));
    assert!(smoke_script.contains("--scenario pending-melt-fail-finish"));
    assert!(smoke_script.contains("--scenario quarantine-mint-conflict"));
    assert!(smoke_script.contains("--scenario offline-member-keyset-rotation"));
    assert!(smoke_script.contains("--wallet-db \"$PENDING_WALLET_DB\""));
    assert!(smoke_script.contains("--wallet-db \"$FAILED_PENDING_WALLET_DB\""));
    assert!(smoke_script.contains(
            "--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-finish"
        ));
    assert!(smoke_script.contains(
            "--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-fail-finish"
        ));
    assert!(smoke_script.contains(
            "stop_member \"$RESTART_MEMBER\"\nrun_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-finish"
        ));
    assert!(smoke_script.contains(
            "stop_member \"$RESTART_MEMBER\"\nrun_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario pending-melt-fail-finish"
        ));
    assert!(
        smoke_script.contains("--private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\"")
    );
    assert!(smoke_script.contains("wait_for_quarantine_report \"$PRIMARY_MEMBER\""));
    assert!(smoke_script.contains(
            "federation payment-observations --public-config \"$PUBLIC_CONFIG\" --private-config \"$private_config\" --member \"$member\" --context mint-quote"
        ));
    assert!(smoke_script.contains("stop_member \"$QUARANTINE_OFFLINE_MEMBER\""));
    assert!(smoke_script.contains("start_member \"$QUARANTINE_OFFLINE_MEMBER\""));
    assert!(smoke_script.contains("wait_for_status \"$QUARANTINE_OFFLINE_MEMBER\""));
    assert!(smoke_script.contains("wait_for_catch_up_certificate_quorum \"$RESTART_MEMBER\""));
    assert!(smoke_script
        .contains("wait_for_catch_up_certificate_quorum \"$QUARANTINE_OFFLINE_MEMBER\""));
    assert!(smoke_script.contains(
            "run_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario offline-member-keyset-rotation"
        ));
    assert!(smoke_script.contains(
            "stop_member \"$RESTART_MEMBER\"\nrun_smoke_operations --private-config \"$(member_private_config \"$PRIMARY_MEMBER\")\" --scenario offline-member-keyset-rotation"
        ));
    assert!(smoke_script.contains(
            "wait_for_status \"$RESTART_MEMBER\"\nwait_for_converged\nwait_for_catch_up_certificate_quorum \"$RESTART_MEMBER\""
        ));
    assert!(smoke_script.contains(
            "wait_for_status \"$QUARANTINE_OFFLINE_MEMBER\"\n\nwait_for_converged\nwait_for_catch_up_certificate_quorum \"$QUARANTINE_OFFLINE_MEMBER\""
        ));
    assert!(smoke_script.contains("wait_for_quarantine_report \"$QUARANTINE_OFFLINE_MEMBER\""));
    assert!(smoke_script.contains("local federation smoke test passed"));
    assert!(smoke_script.contains(&artifacts.public_config_path.to_string_lossy().to_string()));
    assert!(smoke_script.contains(
        &artifacts
            .dkg_setup_public_config_path
            .to_string_lossy()
            .to_string()
    ));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = std::fs::metadata(&artifacts.smoke_script_path)
            .expect("smoke script metadata")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0);
        let live_dkg_mode = std::fs::metadata(&artifacts.live_dkg_smoke_script_path)
            .expect("live DKG smoke script metadata")
            .permissions()
            .mode();
        assert_ne!(live_dkg_mode & 0o111, 0);

        for member in artifacts.member_artifacts.values() {
            for private_path in [
                &member.private_config_path,
                &member.dkg_setup_private_config_path,
                &member.config_path,
                &member.dkg_setup_config_path,
                &member.live_dkg_config_path,
                &member.frost_bdk_config_path,
            ] {
                let mode = std::fs::metadata(private_path)
                    .expect("private generated file metadata")
                    .permissions()
                    .mode();
                assert_eq!(
                    mode & 0o077,
                    0,
                    "private generated file must not grant group/other access: {}",
                    private_path.display()
                );
            }
            for entry in
                std::fs::read_dir(&member.dkg_setup_request_dir).expect("DKG route probe directory")
            {
                let path = entry.expect("DKG route probe entry").path();
                let mode = std::fs::metadata(&path)
                    .expect("DKG route probe metadata")
                    .permissions()
                    .mode();
                assert_eq!(
                    mode & 0o077,
                    0,
                    "signed DKG route probe must be private: {}",
                    path.display()
                );
            }
        }

        let syntax = std::process::Command::new("bash")
            .arg("-n")
            .arg(&artifacts.live_dkg_smoke_script_path)
            .status()
            .expect("run bash syntax check");
        assert!(syntax.success());
    }

    let mut live_dkg_ceremony_ids = BTreeSet::new();
    for (member_id, member_artifacts) in &artifacts.member_artifacts {
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
        let dkg_setup_private_json =
            std::fs::read_to_string(&member_artifacts.dkg_setup_private_config_path)
                .expect("setup-only private config file");
        let dkg_setup_private_config: MemberConfigPrivate =
            serde_json::from_str(&dkg_setup_private_json).expect("setup-only private config json");
        assert_eq!(dkg_setup_private_config.member_id, *member_id);
        assert_eq!(
            dkg_setup_private_config.federation_id,
            Some(public_config.federation_id)
        );
        assert!(dkg_setup_private_config.keyset_secret_shares.is_empty());
        assert!(dkg_setup_private_config.setup_transcript_hash.is_none());
        dkg_setup_private_config
            .validate_against_public(&dkg_setup_public_config)
            .expect("setup-only private config validates");

        let config_text =
            std::fs::read_to_string(&member_artifacts.config_path).expect("member config text");
        let dkg_setup_config_text =
            std::fs::read_to_string(&member_artifacts.dkg_setup_config_path)
                .expect("setup-only member config text");
        let live_dkg_config_text = std::fs::read_to_string(&member_artifacts.live_dkg_config_path)
            .expect("live DKG member config text");
        let frost_bdk_config_text =
            std::fs::read_to_string(&member_artifacts.frost_bdk_config_path)
                .expect("FROST BDK member config text");
        let expected_prometheus_port = 8085 + 3 + member_id.get() - 1;
        assert!(config_text.contains("[prometheus]"));
        assert!(config_text.contains("enabled = true"));
        assert!(config_text.contains("address = \"127.0.0.1\""));
        assert!(config_text.contains(&format!("port = {expected_prometheus_port}")));
        assert!(config_text.contains(&member_artifacts.federation_api_url));
        assert!(config_text.contains(&member_artifacts.public_mint_url));
        assert!(dkg_setup_config_text.contains(
            &artifacts
                .dkg_setup_public_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(dkg_setup_config_text.contains(
            &member_artifacts
                .dkg_setup_private_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(live_dkg_config_text.contains(
            &artifacts
                .dkg_setup_public_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(frost_bdk_config_text.contains("onchain_backend = \"federated-bdk\""));
        assert!(frost_bdk_config_text.contains("network = \"regtest\""));
        assert!(frost_bdk_config_text.contains("minimum_confirmations = 1"));
        assert!(frost_bdk_config_text.contains("fee_policy_version = 1"));
        assert!(frost_bdk_config_text.contains("chain_source_scope = \"shared\""));
        assert!(frost_bdk_config_text.contains("chain_source_id = \"regtest-shared-core\""));
        assert!(frost_bdk_config_text.contains(
            &member_artifacts
                .live_dkg_final_config_dir
                .join("federation-public.json")
                .to_string_lossy()
                .to_string()
        ));
        assert!(frost_bdk_config_text.contains(
            &member_artifacts
                .live_dkg_final_config_dir
                .join("federation-private.json")
                .to_string_lossy()
                .to_string()
        ));
        assert!(live_dkg_config_text.contains(
            &member_artifacts
                .dkg_setup_private_config_path
                .to_string_lossy()
                .to_string()
        ));

        let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
            .expect("member config toml");
        let dkg_setup_settings =
            config::Settings::try_new(Some(&member_artifacts.dkg_setup_config_path))
                .expect("setup-only member config toml");
        let live_dkg_settings =
            config::Settings::try_new(Some(&member_artifacts.live_dkg_config_path))
                .expect("live DKG member config toml");
        let frost_bdk_settings =
            config::Settings::try_new(Some(&member_artifacts.frost_bdk_config_path))
                .expect("FROST BDK member config toml");
        assert_eq!(settings.database.engine, config::DatabaseEngine::Sqlite);
        assert_eq!(
            dkg_setup_settings.database.engine,
            config::DatabaseEngine::Sqlite
        );
        assert_eq!(
            live_dkg_settings.database.engine,
            config::DatabaseEngine::Sqlite
        );
        assert_eq!(
            frost_bdk_settings.database.engine,
            config::DatabaseEngine::Sqlite
        );
        assert_eq!(
            frost_bdk_settings
                .onchain
                .as_ref()
                .expect("on-chain settings")
                .onchain_backend,
            config::OnchainBackend::FederatedBdk
        );
        frost_bdk_settings
            .validate_backend_pairing()
            .expect("on-chain-only FROST BDK pairing");
        assert!(
            settings
                .federation
                .as_ref()
                .expect("federation settings")
                .enabled
        );
        assert!(
            settings
                .federation
                .as_ref()
                .expect("federation settings")
                .allow_development_payment_observation_policy
        );
        assert!(
            dkg_setup_settings
                .federation
                .as_ref()
                .expect("setup-only federation settings")
                .enabled
        );
        assert!(
            dkg_setup_settings
                .federation
                .as_ref()
                .expect("setup-only federation settings")
                .allow_development_payment_observation_policy
        );
        let dkg_setup_federation = dkg_setup_settings
            .federation
            .as_ref()
            .expect("setup-only federation settings");
        assert!(!dkg_setup_federation.dkg.enabled);
        let live_dkg_federation = live_dkg_settings
            .federation
            .as_ref()
            .expect("live DKG federation settings");
        assert!(live_dkg_federation.enabled);
        assert!(live_dkg_federation.dkg.enabled);
        assert!(live_dkg_federation.dkg.allow_insecure_localhost);
        assert_eq!(live_dkg_federation.dkg.unit, "sat");
        assert_eq!(live_dkg_federation.dkg.amounts, vec![1, 2, 4]);
        assert_eq!(live_dkg_federation.dkg.max_ticks, 1200);
        assert_eq!(live_dkg_federation.dkg.retry_delay_ms, 250);
        assert_eq!(live_dkg_federation.dkg.member_request_burst, 16);
        assert_eq!(live_dkg_federation.dkg.member_request_refill_ms, 250);
        let ceremony_id = live_dkg_federation
            .dkg
            .ceremony_id
            .as_ref()
            .expect("live DKG ceremony ID");
        ceremony_id
            .parse::<FederationDkgCeremonyId>()
            .expect("valid live DKG ceremony ID");
        live_dkg_ceremony_ids.insert(ceremony_id.clone());
        assert_eq!(settings.info.listen_port, 8084 + member_id.get());
        assert_eq!(dkg_setup_settings.info.listen_port, 8084 + member_id.get());
        assert_eq!(live_dkg_settings.info.listen_port, 8084 + member_id.get());
        #[cfg(feature = "prometheus")]
        {
            let prometheus = settings.prometheus.expect("prometheus settings");
            assert!(prometheus.enabled);
            assert_eq!(prometheus.address.as_deref(), Some("127.0.0.1"));
            assert_eq!(prometheus.port, Some(expected_prometheus_port));
        }
        assert!(smoke_script.contains(&member_artifacts.config_path.to_string_lossy().to_string()));
        assert!(smoke_script.contains(
            &member_artifacts
                .dkg_setup_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(smoke_script.contains(
            &member_artifacts
                .private_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(smoke_script.contains(
            &member_artifacts
                .dkg_setup_private_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(smoke_script.contains(&member_artifacts.work_dir.to_string_lossy().to_string()));
        assert!(smoke_script.contains(
            &member_artifacts
                .dkg_setup_work_dir
                .to_string_lossy()
                .to_string()
        ));
        assert!(smoke_script.contains(&member_artifacts.public_mint_url));
        assert!(smoke_script.contains(&member_artifacts.prometheus_url));
        assert!(live_dkg_smoke_script.contains(
            &member_artifacts
                .live_dkg_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(live_dkg_smoke_script.contains(
            &member_artifacts
                .frost_bdk_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(live_dkg_smoke_script.contains(
            &member_artifacts
                .live_dkg_work_dir
                .to_string_lossy()
                .to_string()
        ));
        assert!(live_dkg_smoke_script.contains(
            &member_artifacts
                .live_dkg_final_config_dir
                .to_string_lossy()
                .to_string()
        ));
        assert!(live_dkg_smoke_script.contains(&member_artifacts.prometheus_url));
    }
    assert_eq!(live_dkg_ceremony_ids.len(), 1);

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn generated_template_renderer_rejects_incomplete_or_ambiguous_markers() {
    assert_eq!(
        render_generated_template("value=@@VALUE@@\n", &[("@@VALUE@@", "'safe value'")])
            .expect("complete template"),
        "value='safe value'\n"
    );

    let missing = render_generated_template("value=plain\n", &[("@@VALUE@@", "value")])
        .expect_err("missing marker rejected");
    assert!(missing.to_string().contains("occurs 0 times"));

    let repeated = render_generated_template(
        "first=@@VALUE@@ second=@@VALUE@@\n",
        &[("@@VALUE@@", "value")],
    )
    .expect_err("repeated marker rejected");
    assert!(repeated.to_string().contains("occurs 2 times"));

    let duplicate = render_generated_template(
        "value=@@VALUE@@\n",
        &[("@@VALUE@@", "first"), ("@@VALUE@@", "second")],
    )
    .expect_err("duplicate substitution rejected");
    assert!(duplicate.to_string().contains("duplicate"));

    let unknown =
        render_generated_template("value=@@UNKNOWN@@\n", &[]).expect_err("unknown marker rejected");
    assert!(unknown.to_string().contains("unknown marker"));
}

#[test]
fn deterministic_test_seed_reproduces_member_config_mnemonic() {
    let member_id = MemberId::new(1).expect("member id");
    let database_config = "[database]\nengine = \"sqlite\"\n";
    let config = || {
        member_config_toml(
            &member_id,
            "127.0.0.1",
            "http://127.0.0.1:8085",
            "http://127.0.0.1:8085",
            8085,
            8088,
            Path::new("federation-public.json"),
            Path::new("member-1/federation-private.json"),
            Path::new("member-1/config.toml"),
            Path::new("member-1/work"),
            &CurrencyUnit::Sat,
            database_config,
            Some("repeatable local smoke setup"),
        )
        .expect("deterministic member config")
    };
    let first = config();
    let second = config();
    assert_eq!(first, second);

    let different = member_config_toml(
        &member_id,
        "127.0.0.1",
        "http://127.0.0.1:8085",
        "http://127.0.0.1:8085",
        8085,
        8088,
        Path::new("federation-public.json"),
        Path::new("member-1/federation-private.json"),
        Path::new("member-1/config.toml"),
        Path::new("member-1/work"),
        &CurrencyUnit::Sat,
        database_config,
        Some("different local smoke setup"),
    )
    .expect("different deterministic member config");
    assert_ne!(first, different);
}

#[test]
fn trusted_dealer_artifacts_skip_dkg_route_probes_by_default() {
    let output_dir = temp_output_dir("artifacts_without_dkg_probes");
    let _ = std::fs::remove_dir_all(&output_dir);
    let artifacts = generate_trusted_dealer_artifacts(&generator_args(output_dir.clone()))
        .expect("trusted dealer artifacts");

    let first_member = MemberId::new(1).expect("first member id");
    let first_probe_path = artifacts
        .member_artifacts
        .get(&first_member)
        .expect("first member artifacts")
        .dkg_setup_request_dir
        .join("from-1-commitment.json");
    assert!(!first_probe_path.exists());
    let smoke_script = std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script");
    assert!(smoke_script.contains("PROBE_DKG_SETUP_ROUTES"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn trusted_dealer_artifacts_support_postgres_database_template() {
    let output_dir = temp_output_dir("postgres_database_template");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;
    args.postgres_url_template =
        Some("postgresql://cdk:cdk@localhost:5432/cdk_member_{member}".to_string());

    let artifacts =
        generate_trusted_dealer_artifacts(&args).expect("postgres trusted dealer artifacts");

    for (member_id, member_artifacts) in &artifacts.member_artifacts {
        let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
            .expect("member config toml");
        let dkg_setup_settings =
            config::Settings::try_new(Some(&member_artifacts.dkg_setup_config_path))
                .expect("setup-only member config toml");
        let live_dkg_settings =
            config::Settings::try_new(Some(&member_artifacts.live_dkg_config_path))
                .expect("live DKG member config toml");
        assert_eq!(settings.database.engine, config::DatabaseEngine::Postgres);
        assert_eq!(
            dkg_setup_settings.database.engine,
            config::DatabaseEngine::Postgres
        );
        assert_eq!(
            live_dkg_settings.database.engine,
            config::DatabaseEngine::Postgres
        );
        let postgres = settings
            .database
            .postgres
            .as_ref()
            .expect("postgres config");
        let dkg_setup_postgres = dkg_setup_settings
            .database
            .postgres
            .as_ref()
            .expect("setup-only postgres config");
        let live_dkg_postgres = live_dkg_settings
            .database
            .postgres
            .as_ref()
            .expect("live DKG postgres config");
        assert_eq!(
            postgres.url,
            format!(
                "postgresql://cdk:cdk@localhost:5432/cdk_member_{}",
                member_id.get()
            )
        );
        assert_eq!(dkg_setup_postgres.url, postgres.url);
        assert_eq!(live_dkg_postgres.url, postgres.url);
        assert!(!postgres.url.contains("{member}"));
        assert!(!dkg_setup_postgres.url.contains("{member}"));
    }
    let smoke_script =
        std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
    assert!(smoke_script.contains("POSTGRES_SCHEMA_RESET_URL=''"));
    assert!(smoke_script.contains("reset_postgres_schemas()"));

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn trusted_dealer_artifacts_support_postgres_schema_base_url() {
    let output_dir = temp_output_dir("postgres_schema_base_url");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;
    args.postgres_base_url = Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string());

    let artifacts =
        generate_trusted_dealer_artifacts(&args).expect("postgres trusted dealer artifacts");

    for (member_id, member_artifacts) in &artifacts.member_artifacts {
        let settings = config::Settings::try_new(Some(&member_artifacts.config_path))
            .expect("member config toml");
        let dkg_setup_settings =
            config::Settings::try_new(Some(&member_artifacts.dkg_setup_config_path))
                .expect("setup-only member config toml");
        let live_dkg_settings =
            config::Settings::try_new(Some(&member_artifacts.live_dkg_config_path))
                .expect("live DKG member config toml");
        assert_eq!(settings.database.engine, config::DatabaseEngine::Postgres);
        assert_eq!(
            dkg_setup_settings.database.engine,
            config::DatabaseEngine::Postgres
        );
        assert_eq!(
            live_dkg_settings.database.engine,
            config::DatabaseEngine::Postgres
        );
        let postgres = settings
            .database
            .postgres
            .as_ref()
            .expect("postgres config");
        let dkg_setup_postgres = dkg_setup_settings
            .database
            .postgres
            .as_ref()
            .expect("setup-only postgres config");
        let live_dkg_postgres = live_dkg_settings
            .database
            .postgres
            .as_ref()
            .expect("live DKG postgres config");
        assert_eq!(
            postgres.url,
            format!(
                "postgresql://cdk:cdk@localhost:5432/cdk_mint schema=cdk_federation_member_{}",
                member_id.get()
            )
        );
        assert_eq!(dkg_setup_postgres.url, postgres.url);
        assert_eq!(live_dkg_postgres.url, postgres.url);
    }
    let smoke_script =
        std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
    assert!(smoke_script
        .contains("POSTGRES_SCHEMA_RESET_URL='postgresql://cdk:cdk@localhost:5432/cdk_mint'"));
    assert!(smoke_script.contains("RESET_POSTGRES_SCHEMAS=\"${RESET_POSTGRES_SCHEMAS:-0}\""));
    assert!(smoke_script.contains("generated PostgreSQL member schemas are configured"));
    assert!(smoke_script.contains("psql is required to reset PostgreSQL member schemas"));
    assert!(smoke_script.contains("DROP SCHEMA IF EXISTS"));
    assert!(smoke_script.contains("CREATE SCHEMA"));
    for member_id in artifacts.member_artifacts.keys() {
        assert!(smoke_script.contains(&format!("cdk_federation_member_{}", member_id.get())));
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn trusted_dealer_postgres_storage_options_are_validated() {
    let output_dir = temp_output_dir("postgres_validation");
    let mut args = generator_args(output_dir);
    args.database_engine = FederationTrustedDealerDatabaseEngine::Postgres;

    let err = validate_trusted_dealer_args(&args).expect_err("postgres storage is required");
    assert!(err
        .to_string()
        .contains("--postgres-url-template or --postgres-base-url is required"));

    args.postgres_url_template = Some("postgresql://cdk:cdk@localhost:5432/shared".to_string());
    let err = validate_trusted_dealer_args(&args).expect_err("member placeholder is required");
    assert!(err.to_string().contains("must contain {member}"));

    args.postgres_url_template =
        Some("postgresql://cdk:cdk@localhost:5432/cdk_member_{member}".to_string());
    args.postgres_base_url = Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string());
    let err = validate_trusted_dealer_args(&args).expect_err("only one postgres option");
    assert!(err.to_string().contains("not both"));

    args.postgres_url_template = None;
    args.postgres_base_url = Some("   ".to_string());
    let err = validate_trusted_dealer_args(&args).expect_err("base url cannot be empty");
    assert!(err
        .to_string()
        .contains("--postgres-base-url must not be empty"));

    args.database_engine = FederationTrustedDealerDatabaseEngine::Sqlite;
    args.postgres_base_url = Some("postgresql://cdk:cdk@localhost:5432/cdk_mint".to_string());
    let err = validate_trusted_dealer_args(&args).expect_err("sqlite rejects postgres options");
    assert!(err
        .to_string()
        .contains("require --database-engine postgres"));
}

#[test]
fn trusted_dealer_smoke_script_supports_four_member_bft_matrix() {
    let output_dir = temp_output_dir("four_member_matrix");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.members = 4;
    args.threshold = 3;
    args.port_start = 19_545;

    let artifacts =
        generate_trusted_dealer_artifacts(&args).expect("four member trusted dealer artifacts");
    let public_json =
        std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
    let public_config: FederationConfig =
        serde_json::from_str(&public_json).expect("public config json");
    public_config.validate().expect("public config validates");
    assert_eq!(public_config.threshold_params.member_count, 4);
    assert_eq!(public_config.threshold_params.signature_threshold, 3);
    assert_eq!(public_config.threshold_params.consensus_threshold, 3);
    assert_eq!(artifacts.member_artifacts.len(), 4);

    let smoke_script =
        std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
    assert!(smoke_script.contains("MEMBERS=(1 2 3 4)"));
    assert!(smoke_script.contains("for member in \"${MEMBERS[@]}\"; do"));
    assert!(smoke_script.contains("wait_for_converged"));
    assert!(smoke_script.contains("wait_for_catch_up_certificate_quorum"));
    assert!(smoke_script.contains("--scenario offline-member-operations"));
    assert!(smoke_script.contains("--scenario pending-melt-start"));
    assert!(smoke_script.contains("--scenario pending-melt-finish"));
    assert!(smoke_script.contains("--scenario pending-melt-fail-start"));
    assert!(smoke_script.contains("--scenario pending-melt-fail-finish"));
    assert!(smoke_script.contains("--scenario quarantine-mint-conflict"));
    assert!(smoke_script.contains("--scenario offline-member-keyset-rotation"));
    assert!(smoke_script.contains("federation payment-observations"));
    assert!(smoke_script.contains("wait_for_prometheus_metrics"));
    assert!(smoke_script
        .contains("ALEPH_BFT_AUTO_ROTATION_MAX_ROUND=\"${ALEPH_BFT_AUTO_ROTATION_MAX_ROUND:-}\""));
    assert!(!smoke_script.contains("FEDERATION_CONSENSUS_BACKEND"));
    assert!(smoke_script.contains(
        "FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=\"${FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-}\""
    ));
    assert!(smoke_script.contains(
        "CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=\"$FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS\""
    ));
    assert!(smoke_script.contains(
        "CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_MAX_ROUND=\"$ALEPH_BFT_AUTO_ROTATION_MAX_ROUND\""
    ));
    assert!(smoke_script.contains(
        "CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT=\"$ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION\""
    ));
    assert!(smoke_script.contains("wait_for_configured_aleph_bft_auto_rotation"));
    assert!(smoke_script.contains("run_configured_aleph_bft_auto_rotation_warmup"));
    assert!(smoke_script
        .contains("--operation-batches \"$ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES\""));

    for (member_id, member_artifacts) in &artifacts.member_artifacts {
        assert!(smoke_script.contains(&format!("    {}) printf", member_id.get())));
        assert!(smoke_script.contains(&member_artifacts.config_path.to_string_lossy().to_string()));
        assert!(smoke_script.contains(
            &member_artifacts
                .private_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(smoke_script.contains(&member_artifacts.work_dir.to_string_lossy().to_string()));
        assert!(smoke_script.contains(&member_artifacts.prometheus_url));
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn trusted_dealer_smoke_script_supports_five_member_matrix() {
    let output_dir = temp_output_dir("five_member_matrix");
    let _ = std::fs::remove_dir_all(&output_dir);
    let mut args = generator_args(output_dir.clone());
    args.members = 5;
    args.threshold = 3;
    args.port_start = 19_565;

    let artifacts =
        generate_trusted_dealer_artifacts(&args).expect("five member trusted dealer artifacts");
    let public_json =
        std::fs::read_to_string(&artifacts.public_config_path).expect("public config file");
    let public_config: FederationConfig =
        serde_json::from_str(&public_json).expect("public config json");
    public_config.validate().expect("public config validates");
    assert_eq!(public_config.threshold_params.member_count, 5);
    assert_eq!(public_config.threshold_params.signature_threshold, 3);
    assert_eq!(artifacts.member_artifacts.len(), 5);

    let smoke_script =
        std::fs::read_to_string(&artifacts.smoke_script_path).expect("smoke script file");
    assert!(smoke_script.contains("MEMBERS=(1 2 3 4 5)"));
    assert!(smoke_script.contains("PRIMARY_MEMBER=\"${PRIMARY_MEMBER:-1}\""));
    assert!(smoke_script.contains("RESTART_MEMBER=\"${RESTART_MEMBER:-2}\""));
    assert!(smoke_script
        .contains("QUARANTINE_OFFLINE_MEMBER=\"${QUARANTINE_OFFLINE_MEMBER:-$RESTART_MEMBER}\""));
    assert!(
        smoke_script.contains("CRASH_WINDOW_MEMBER=\"${CRASH_WINDOW_MEMBER:-$RESTART_MEMBER}\"")
    );
    assert!(smoke_script
        .contains("LONG_PARTITION_MEMBER=\"${LONG_PARTITION_MEMBER:-$RESTART_MEMBER}\""));
    assert!(smoke_script.contains("EXERCISE_CRASH_WINDOW=\"${EXERCISE_CRASH_WINDOW:-1}\""));
    assert!(smoke_script.contains("STANDARD_SEED_OFFSET=\"${STANDARD_SEED_OFFSET:-}\""));
    assert!(
        smoke_script.contains("STANDARD_OPERATION_BATCHES=\"${STANDARD_OPERATION_BATCHES:-2}\"")
    );
    assert!(smoke_script.contains("POST_CRASH_SEED_OFFSET=\"${POST_CRASH_SEED_OFFSET:-}\""));
    assert!(smoke_script
        .contains("POST_CRASH_OPERATION_BATCHES=\"${POST_CRASH_OPERATION_BATCHES:-1}\""));
    assert!(smoke_script.contains("LONG_PARTITION_SEED_OFFSET=\"${LONG_PARTITION_SEED_OFFSET:-}\""));
    assert!(smoke_script
        .contains("LONG_PARTITION_OPERATION_BATCHES=\"${LONG_PARTITION_OPERATION_BATCHES:-4}\""));
    assert!(smoke_script.contains(
        "ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION=\"${ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION:-1}\""
    ));
    assert!(smoke_script.contains(
        "CDK_MINTD_FEDERATION_ALEPH_BFT_LIVE_SESSION_ROTATION_LIMIT=\"$ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION\""
    ));
    assert!(smoke_script.contains(
        "ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES=\"${ALEPH_BFT_AUTO_ROTATION_WARMUP_OPERATION_BATCHES:-1}\""
    ));
    assert!(smoke_script.contains(
        "FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=\"$SMOKE_OPERATION_TIMEOUT_SECONDS\""
    ));
    assert!(smoke_script
        .contains("require_positive_integer FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS"));
    assert!(smoke_script.contains(
        "STANDARD_SEED_OFFSET must be after the AlephBFT auto-rotation warm-up seed range"
    ));
    assert!(smoke_script.contains("ALEPH_BFT_AUTO_ROTATION_MAX_ROUND must be 65535 or lower"));
    assert!(smoke_script.contains("CHECK_PROMETHEUS_METRICS=\"${CHECK_PROMETHEUS_METRICS:-1}\""));
    assert!(smoke_script.contains("CDK_MINTD_PROMETHEUS_ENABLED=false"));
    assert!(smoke_script
        .contains("SMOKE_OPERATION_TIMEOUT_SECONDS=\"${SMOKE_OPERATION_TIMEOUT_SECONDS:-300}\""));
    assert!(smoke_script.contains("for member in \"${MEMBERS[@]}\"; do"));
    assert!(smoke_script.contains("wait_for_converged"));
    assert!(smoke_script.contains("wait_for_prometheus_metrics"));
    assert!(smoke_script.contains("wait_for_aleph_bft_session_at_least"));
    assert!(smoke_script.contains("wait_for_configured_aleph_bft_auto_rotation"));
    assert!(smoke_script.contains("run_configured_aleph_bft_auto_rotation_warmup"));
    assert!(smoke_script.contains("wait_for_catch_up_certificate_quorum"));
    assert!(smoke_script.contains("--seed-offset \"$STANDARD_SEED_OFFSET\""));
    assert!(smoke_script.contains("--scenario offline-member-operations"));
    assert!(smoke_script.contains("--scenario pending-melt-fail-finish"));
    assert!(smoke_script.contains("--scenario quarantine-mint-conflict"));
    assert!(smoke_script.contains("federation payment-observations"));

    for (member_id, member_artifacts) in &artifacts.member_artifacts {
        assert!(smoke_script.contains(&format!("    {}) printf", member_id.get())));
        assert!(smoke_script.contains(&member_artifacts.config_path.to_string_lossy().to_string()));
        assert!(smoke_script.contains(
            &member_artifacts
                .private_config_path
                .to_string_lossy()
                .to_string()
        ));
        assert!(smoke_script.contains(&member_artifacts.work_dir.to_string_lossy().to_string()));
        assert!(smoke_script.contains(&member_artifacts.prometheus_url));
    }

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[test]
fn trusted_dealer_artifacts_refuse_to_overwrite_without_force() {
    let output_dir = temp_output_dir("overwrite");
    let _ = std::fs::remove_dir_all(&output_dir);
    let args = generator_args(output_dir.clone());
    generate_trusted_dealer_artifacts(&args).expect("first trusted dealer artifacts");

    let err = generate_trusted_dealer_artifacts(&args)
        .expect_err("second generation should refuse overwrite");
    assert!(err.to_string().contains("already exists"));

    let mut forced_args = args;
    forced_args.force = true;
    generate_trusted_dealer_artifacts(&forced_args).expect("forced generation");

    let _ = std::fs::remove_dir_all(&output_dir);
}

#[cfg(unix)]
#[test]
fn private_writer_creates_and_repairs_restrictive_permissions() {
    use std::os::unix::fs::symlink;
    use std::os::unix::fs::PermissionsExt;

    let output_dir = temp_output_dir("private_writer_permissions");
    let _ = std::fs::remove_dir_all(&output_dir);
    let path = output_dir.join("member-private.json");

    write_private_text_file(&path, "first", false).expect("create private file");
    let created_mode = std::fs::metadata(&path)
        .expect("created private file metadata")
        .permissions()
        .mode();
    assert_eq!(created_mode & 0o077, 0);

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("make fixture permissive");
    write_private_text_file(&path, "second", true).expect("repair private file");
    let repaired_mode = std::fs::metadata(&path)
        .expect("repaired private file metadata")
        .permissions()
        .mode();
    assert_eq!(repaired_mode & 0o077, 0);
    assert_eq!(
        std::fs::read_to_string(&path).expect("repaired private contents"),
        "second"
    );

    let symlink_target = output_dir.join("symlink-target");
    let symlink_path = output_dir.join("symlink-private.json");
    std::fs::write(&symlink_target, "untouched").expect("symlink target");
    symlink(&symlink_target, &symlink_path).expect("private file symlink fixture");
    write_private_text_file(&symlink_path, "replacement", true)
        .expect("atomically replace private file symlink");
    assert_eq!(
        std::fs::read_to_string(&symlink_target).expect("symlink target contents"),
        "untouched"
    );
    assert_eq!(
        std::fs::read_to_string(&symlink_path).expect("replaced symlink contents"),
        "replacement"
    );
    assert!(!std::fs::symlink_metadata(&symlink_path)
        .expect("replaced symlink metadata")
        .file_type()
        .is_symlink());

    let _ = std::fs::remove_dir_all(&output_dir);
}
