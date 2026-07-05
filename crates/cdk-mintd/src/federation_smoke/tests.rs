use std::path::PathBuf;

use super::*;

fn smoke_args() -> FederationSmokeOperationsArgs {
    FederationSmokeOperationsArgs {
        public_config: PathBuf::from("public.json"),
        private_config: None,
        unit: "sat".to_string(),
        mint_amount: 4,
        scenario: FederationSmokeScenario::Standard,
        wallet_db: None,
        seed_offset: 0,
        quote_retries: 100,
        quote_sleep_ms: 50,
        operation_timeout_seconds: Some(300),
    }
}

#[test]
fn smoke_operation_args_accept_default_polling_delay() {
    let args = smoke_args();

    validate_smoke_operation_args(&args).expect("default smoke polling delay validates");
}

#[test]
fn smoke_operation_args_reject_zero_quote_retries() {
    let mut args = smoke_args();
    args.quote_retries = 0;

    let err = validate_smoke_operation_args(&args)
        .expect_err("zero smoke quote retries should be rejected");

    assert!(err.to_string().contains("--quote-retries"));
}

#[test]
fn smoke_operation_args_reject_zero_mint_amount() {
    let mut args = smoke_args();
    args.mint_amount = 0;

    let err = validate_smoke_operation_args(&args)
        .expect_err("zero smoke mint amount should be rejected");

    assert!(err.to_string().contains("--mint-amount"));
}

#[test]
fn smoke_operation_args_reject_zero_quote_sleep() {
    let mut args = smoke_args();
    args.quote_sleep_ms = 0;

    let err = validate_smoke_operation_args(&args)
        .expect_err("zero smoke quote sleep should be rejected");

    assert!(err.to_string().contains("--quote-sleep-ms"));
}

#[test]
fn smoke_operation_args_reject_zero_timeout() {
    let mut args = smoke_args();
    args.operation_timeout_seconds = Some(0);

    let err = validate_smoke_operation_args(&args)
        .expect_err("zero smoke operation timeout should be rejected");

    assert!(err.to_string().contains("--operation-timeout-seconds"));
}

#[test]
fn smoke_operation_args_require_wallet_db_for_resumable_scenarios() {
    for scenario in [
        FederationSmokeScenario::PendingMeltStart,
        FederationSmokeScenario::PendingMeltFinish,
        FederationSmokeScenario::PendingMeltFailStart,
        FederationSmokeScenario::PendingMeltFailFinish,
    ] {
        let mut args = smoke_args();
        args.scenario = scenario;

        let err = validate_smoke_operation_args(&args)
            .expect_err("resumable smoke scenario should require a wallet database");

        assert!(err.to_string().contains("--wallet-db"));
        assert!(err.to_string().contains(smoke_scenario_name(scenario)));

        args.wallet_db = Some(PathBuf::from("wallet.sqlite"));
        validate_smoke_operation_args(&args)
            .expect("resumable smoke scenario validates with a wallet database");
    }
}

#[test]
fn smoke_operation_args_require_private_config_for_signed_operation_scenarios() {
    for scenario in [
        FederationSmokeScenario::QuarantineMintConflict,
        FederationSmokeScenario::KeysetRotation,
        FederationSmokeScenario::OfflineMemberKeysetRotation,
    ] {
        let mut args = smoke_args();
        args.scenario = scenario;

        let err = validate_smoke_operation_args(&args)
            .expect_err("signed-operation smoke scenario should require private config");

        assert!(err.to_string().contains("--private-config"));
        assert!(err.to_string().contains(smoke_scenario_name(scenario)));

        args.private_config = Some(PathBuf::from("member-1/federation-private.json"));
        validate_smoke_operation_args(&args)
            .expect("signed-operation smoke scenario validates with private config");
    }
}

#[test]
fn smoke_operation_args_allow_standard_and_offline_without_optional_paths() {
    for scenario in [
        FederationSmokeScenario::Standard,
        FederationSmokeScenario::OfflineMemberOperations,
    ] {
        let mut args = smoke_args();
        args.scenario = scenario;

        validate_smoke_operation_args(&args)
            .expect("plain smoke scenario should not require optional paths");
    }
}
