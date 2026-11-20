#!/usr/bin/env bash
# Exercise the Supabase reservation migration against an isolated PostgreSQL DB.
set -euo pipefail
export PGHOST="${CDK_TEST_PG_HOST:-127.0.0.1}"
export PGPORT="${CDK_TEST_PG_PORT:-5432}"
export PGUSER="${CDK_TEST_PG_USER:-cdk_user}"
export PGPASSWORD="${CDK_TEST_PG_PASSWORD:-cdk_password}"
rpc_database="cdk_rpc_test_$$"
createdb "$rpc_database"
trap 'dropdb --if-exists "$rpc_database"' EXIT
psql --no-psqlrc --dbname "$rpc_database" --file crates/cdk-supabase/migrations/tests/reserve_derivation_index.sql
