use std::path::PathBuf;
use std::time::Duration;

use cdk::mint_url::MintUrl;
use cdk_common::wallet::{MeltOperationData, OperationData};

use super::*;

fn smoke_args() -> FederationSmokeOperationsArgs {
    FederationSmokeOperationsArgs {
        public_config: PathBuf::from("public.json"),
        private_config: None,
        conflicting_private_config: None,
        unit: "sat".to_string(),
        mint_amount: 4,
        scenario: FederationSmokeScenario::Standard,
        wallet_db: None,
        seed_offset: 0,
        operation_batches: 1,
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
fn smoke_connector_config_uses_bounded_member_timeouts_independent_from_operation_timeout() {
    let mut args = smoke_args();
    args.operation_timeout_seconds = Some(42);

    let config = smoke_connector_config(&args);

    assert_eq!(
        config.member_fallback_timeout,
        FEDERATION_SMOKE_MEMBER_TIMEOUT
    );
    assert_eq!(
        config.member_request_timeout,
        FEDERATION_SMOKE_MEMBER_TIMEOUT
    );
    assert_eq!(
        config.quote_status_member_timeout,
        FEDERATION_SMOKE_MEMBER_TIMEOUT
    );
    assert_eq!(config.mint_quote_visibility_attempts, args.quote_retries);
    assert_eq!(
        config.mint_quote_visibility_retry_delay,
        Duration::from_millis(args.quote_sleep_ms)
    );
}

#[test]
fn smoke_operation_args_reject_zero_operation_batches() {
    let mut args = smoke_args();
    args.operation_batches = 0;

    let err = validate_smoke_operation_args(&args)
        .expect_err("zero smoke operation batches should be rejected");

    assert!(err.to_string().contains("--operation-batches"));
}

#[test]
fn smoke_operation_args_reject_seed_offset_batch_overflow() {
    let mut args = smoke_args();
    args.seed_offset = 254;
    args.operation_batches = 2;

    validate_smoke_operation_args(&args).expect("last seed offset 255 validates");

    args.operation_batches = 3;
    let err = validate_smoke_operation_args(&args)
        .expect_err("smoke operation batches should not wrap seed offsets");

    assert!(err.to_string().contains("--seed-offset"));
    assert!(err.to_string().contains("--operation-batches"));
}

#[test]
fn smoke_operation_args_reject_batches_for_stateful_signed_scenarios() {
    for scenario in [
        FederationSmokeScenario::PendingMeltStart,
        FederationSmokeScenario::PendingMeltFinish,
        FederationSmokeScenario::PendingMeltFailStart,
        FederationSmokeScenario::PendingMeltFailFinish,
        FederationSmokeScenario::QuarantineMintConflict,
        FederationSmokeScenario::KeysetRotation,
        FederationSmokeScenario::OfflineMemberKeysetRotation,
    ] {
        let mut args = smoke_args();
        args.scenario = scenario;
        args.operation_batches = 2;
        args.wallet_db = Some(PathBuf::from("wallet.sqlite"));
        args.private_config = Some(PathBuf::from("member-1/federation-private.json"));

        let err = validate_smoke_operation_args(&args)
            .expect_err("stateful smoke scenario should reject operation batches");

        assert!(err.to_string().contains("--operation-batches"));
    }
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
        if scenario == FederationSmokeScenario::QuarantineMintConflict {
            args.conflicting_private_config =
                Some(PathBuf::from("member-2/federation-private.json"));
        }
        validate_smoke_operation_args(&args)
            .expect("signed-operation smoke scenario validates with private config");
    }
}

#[test]
fn quarantine_smoke_requires_a_second_member_private_config() {
    let mut args = smoke_args();
    args.scenario = FederationSmokeScenario::QuarantineMintConflict;
    args.private_config = Some(PathBuf::from("member-1/federation-private.json"));

    let err = validate_smoke_operation_args(&args)
        .expect_err("quarantine conflict needs an independent observer signature");

    assert!(err.to_string().contains("--conflicting-private-config"));
}

#[test]
fn smoke_operation_args_allow_standard_and_offline_without_optional_paths() {
    for scenario in [
        FederationSmokeScenario::Standard,
        FederationSmokeScenario::OfflineMemberOperations,
    ] {
        let mut args = smoke_args();
        args.scenario = scenario;
        args.operation_batches = 2;

        validate_smoke_operation_args(&args)
            .expect("plain smoke scenario should not require optional paths");
    }
}

#[test]
fn pending_melt_finish_selects_recoverable_melt_saga() {
    let mint_url = "http://127.0.0.1:1234"
        .parse::<MintUrl>()
        .expect("valid mint URL");
    let unit = CurrencyUnit::Sat;
    let saga = pending_melt_saga(
        "00000000-0000-0000-0000-000000000001",
        "expired-but-recoverable",
        MeltSagaState::MeltRequested,
        mint_url.clone(),
        unit.clone(),
        10,
    );

    let selected = select_recoverable_pending_melt_quote_id(&[saga], &mint_url, &unit);

    assert_eq!(selected.as_deref(), Some("expired-but-recoverable"));
}

#[test]
fn pending_melt_finish_ignores_unsubmitted_melt_saga() {
    let mint_url = "http://127.0.0.1:1234"
        .parse::<MintUrl>()
        .expect("valid mint URL");
    let unit = CurrencyUnit::Sat;
    let saga = pending_melt_saga(
        "00000000-0000-0000-0000-000000000002",
        "proofs-reserved-only",
        MeltSagaState::ProofsReserved,
        mint_url.clone(),
        unit.clone(),
        10,
    );

    let selected = select_recoverable_pending_melt_quote_id(&[saga], &mint_url, &unit);

    assert!(selected.is_none());
}

#[test]
fn pending_melt_finish_selects_oldest_recoverable_melt_saga() {
    let mint_url = "http://127.0.0.1:1234"
        .parse::<MintUrl>()
        .expect("valid mint URL");
    let unit = CurrencyUnit::Sat;
    let newer = pending_melt_saga(
        "00000000-0000-0000-0000-000000000003",
        "newer",
        MeltSagaState::PaymentPending,
        mint_url.clone(),
        unit.clone(),
        20,
    );
    let older = pending_melt_saga(
        "00000000-0000-0000-0000-000000000004",
        "older",
        MeltSagaState::MeltRequested,
        mint_url.clone(),
        unit.clone(),
        10,
    );

    let selected = select_recoverable_pending_melt_quote_id(&[newer, older], &mint_url, &unit);

    assert_eq!(selected.as_deref(), Some("older"));
}

fn pending_melt_saga(
    operation_id: &str,
    quote_id: &str,
    state: MeltSagaState,
    mint_url: MintUrl,
    unit: CurrencyUnit,
    created_at: u64,
) -> WalletSaga {
    let operation_id = operation_id.parse().expect("valid operation id");
    let mut saga = WalletSaga::new(
        operation_id,
        WalletSagaState::Melt(state),
        Amount::from(1),
        mint_url,
        unit,
        OperationData::Melt(MeltOperationData {
            quote_id: quote_id.to_string(),
            amount: Amount::from(1),
            fee_reserve: Amount::from(1),
            counter_start: None,
            counter_end: None,
            change_amount: None,
            metadata: Default::default(),
            final_proof_ys: None,
            change_blinded_messages: None,
        }),
    );
    saga.created_at = created_at;
    saga.updated_at = created_at;
    saga
}
