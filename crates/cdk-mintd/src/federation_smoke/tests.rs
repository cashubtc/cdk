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
