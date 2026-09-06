#!/usr/bin/env bash
set -euo pipefail
trap 'stop-pgbouncer; stop-postgres' EXIT
start-postgres
start-pgbouncer

# Include the integration-test binaries that assert the shared gauges balance.
cargo test -p cdk-sql-common -p cdk-sqlite --features cdk-sqlite/prometheus
cargo test -p cdk-sqlite --features sqlcipher,prometheus

# Each invocation targets one endpoint. Proxy-specific tests opt in explicitly.
for mode in direct session transaction; do
    case "$mode" in
        direct) port="${CDK_TEST_PG_PORT:-5432}" ;;
        session) port="${CDK_TEST_PGBOUNCER_SESSION_PORT:-6433}" ;;
        transaction) port="${CDK_TEST_PGBOUNCER_TRANSACTION_PORT:-6432}" ;;
    esac
    export CDK_MINTD_DATABASE_URL="host=127.0.0.1 port=$port user=${CDK_TEST_PG_USER:-cdk_user} password=${CDK_TEST_PG_PASSWORD:-cdk_password} dbname=${CDK_TEST_PG_DATABASE:-cdk_mint}"
    export CDK_TEST_PGBOUNCER_MODE="$mode"
    # Two-server fault tests hold a control transaction; avoid unrelated tests
    # consuming its required second server at the same time.
    cargo test -p cdk-postgres --features prometheus -- --test-threads 1
    if [[ "$mode" == transaction ]]; then
        cargo test -p cdk-postgres --features prometheus connection::tests::pgbouncer_ -- --ignored --test-threads 1
    fi
    cargo test -p cdk-integration-tests --test signatory_rotation
done
