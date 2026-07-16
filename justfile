set positional-arguments

alias b := build
alias c := check
alias t := test

default:
  @just --list

# Enter a Nix development shell without copying ignored files from JJ workspaces.
develop shell="stable":
  #!/usr/bin/env bash
  set -euo pipefail

  shell_name={{quote(shell)}}
  case "$shell_name" in
    ""|*[!a-zA-Z0-9._-]*)
      echo "Error: invalid development shell name: $shell_name" >&2
      exit 2
      ;;
  esac

  flake_ref=".#${shell_name}"
  if command -v jj >/dev/null 2>&1 && jj root >/dev/null 2>&1; then
    revision=$(jj log -r @ --no-graph -T commit_id)

    if git_dir=$(jj git root 2>/dev/null); then
      case "$git_dir" in
        */.git)
          git_source=${git_dir%/.git}
          ;;
        *)
          git_source=$git_dir
          ;;
      esac

      flake_ref="git+file://${git_source}?rev=${revision}#${shell_name}"
    else
      echo "Warning: JJ repository is not Git-backed; using the workspace path." >&2
    fi
  fi

  exec nix develop "$flake_ref" -c "${SHELL:-bash}"

# Create a new SQL migration file
new-migration target name:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ "{{target}}" != "mint" ] && [ "{{target}}" != "wallet" ]; then
    echo "Error: target must be either 'mint' or 'wallet'"
    exit 1
  fi

  timestamp=$(date +%Y%m%d%H%M%S)
  migration_path="./crates/cdk-sql-common/src/{{target}}/migrations/${timestamp}_{{name}}.sql"

  # Create the file
  mkdir -p "$(dirname "$migration_path")"
  touch "$migration_path"
  echo "Created new migration: $migration_path"


final-check: lint clippy test

quick-check: lint clippy test-units

# run `cargo build` on everything
build *ARGS="--workspace --all-targets":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo build {{ARGS}}

# Build the default Nix package.
nix-build:
  nix build --accept-flake-config

# Build and push the default package and reusable Crane dependency artifacts to Attic.
attic-push:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  targets=(
    '.#default'
    '.#deps'
    '.#deps-msrv'
    '.#itest-archive'
    '.#start-fake-mint'
    '.#start-regtest-mints'
    '.#start-fake-auth-mint'
    '.#start-regtest'
    '.#signatory'
    '.#cdk-payment-processor'
    '.#cdk-mintd-grpc'
  )
  system="$(nix eval --impure --raw --expr builtins.currentSystem)"
  if [[ "$system" == *-linux ]]; then
    targets+=('.#deps-static')
  fi
  nix build --accept-flake-config --json --no-link "${targets[@]}" \
    | jq -r '.[].outputs | to_entries[].value' \
    | attic push --stdin cashudevkit-cache:cashudevkit

# Build a statically-linked binary by profile name (requires nix)
# Profiles: cdk-mintd-static, cdk-mintd-ldk-static, cdk-cli-static
build-static profile:
  #!/usr/bin/env bash
  set -euo pipefail
  nix build .#{{profile}}
  mkdir -p ./static-bin
  cp -f ./result/bin/* ./static-bin/
  echo "Static binaries in ./static-bin/:"
  ls -la ./static-bin/

# Build all statically-linked binaries and generate checksums (requires nix)
build-static-all: (build-static "cdk-mintd-static") (build-static "cdk-mintd-ldk-static") (build-static "cdk-cli-static")
  #!/usr/bin/env bash
  set -euo pipefail
  cd ./static-bin
  sha256sum -- *-x86_64 > SHA256SUMS
  echo "Checksums:"
  cat SHA256SUMS

# Build the CI cache warmup targets locally.
ci-cache-build:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  nix build -L --no-link \
    '.#deps' \
    '.#deps-msrv' \
    '.#checks.x86_64-linux.workspace-clippy-all-targets' \
    '.#dart-bindings' \
    '.#kotlin-bindings' \
    '.#go-bindings'

# Push the locally built CI cache warmup targets to Cachix.
ci-cache-push:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  nix build --json --no-link \
    '.#deps' \
    '.#deps-msrv' \
    '.#checks.x86_64-linux.workspace-clippy-all-targets' \
    '.#dart-bindings' \
    '.#kotlin-bindings' \
    '.#go-bindings' \
    | jq -r '.[].outputs | to_entries[].value' \
    | cachix push cashudevkit

# Build and push CI cache warmup targets as they complete.
ci-cache-watch:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  cachix watch-exec cashudevkit -- just ci-cache-build

# run `cargo check` on everything
check *ARGS="--workspace --all-targets":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo check {{ARGS}}

# run code formatters
format:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo fmt --all
  nixpkgs-fmt $(echo **.nix)

# run doc tests
test:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi

  # Unit/lib tests always run from source (not included in the nextest archive)
  cargo test --lib --workspace --exclude cdk-postgres

  # Run the mint integration test (from the pre-built nextest archive when
  # CDK_ITEST_ARCHIVE is set, otherwise via cargo test)
  source ./misc/itest_helpers.sh
  run_test mint

test-units:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo test --lib --workspace --exclude cdk-postgres --exclude cdk-integration-tests

  # run doc tests
  cargo test --doc

# Initialize and start the local PostgreSQL instance used by cdk-postgres tests
start-postgres:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  data_dir="${CDK_POSTGRES_DATA_DIR:-.pg_data}"
  host="${CDK_POSTGRES_HOST:-127.0.0.1}"
  port="${CDK_POSTGRES_PORT:-5432}"
  db="${CDK_POSTGRES_DB:-cdk_mint}"
  user="${CDK_POSTGRES_USER:-cdk}"
  password="${CDK_POSTGRES_PASSWORD:-cdk}"
  log_file="${CDK_POSTGRES_LOG:-${data_dir}/postgres.log}"
  for cmd in initdb pg_ctl pg_isready psql createuser createdb; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
      echo "$cmd is required to start the local PostgreSQL helper" >&2
      echo "enter the regtest shell with: nix develop .#regtest" >&2
      exit 1
    fi
  done
  if [[ ! "$user" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
    echo "CDK_POSTGRES_USER must be a simple PostgreSQL identifier" >&2
    exit 1
  fi
  if [[ ! "$db" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
    echo "CDK_POSTGRES_DB must be a simple PostgreSQL identifier" >&2
    exit 1
  fi
  escaped_password="${password//\'/\'\'}"
  if [ ! -s "${data_dir}/PG_VERSION" ]; then
    initdb -D "$data_dir" --auth=trust --no-locale
  fi
  mkdir -p "$(dirname "$log_file")"
  if ! pg_ctl -D "$data_dir" status >/dev/null 2>&1; then
    pg_ctl -D "$data_dir" -l "$log_file" -o "-h ${host} -p ${port}" start
  fi
  for _ in {1..30}; do
    if pg_isready -h "$host" -p "$port" -d postgres >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  if ! pg_isready -h "$host" -p "$port" -d postgres >/dev/null 2>&1; then
    echo "PostgreSQL did not become ready on ${host}:${port}; see ${log_file}" >&2
    exit 1
  fi
  if [ "$(psql -h "$host" -p "$port" -d postgres -tAc "SELECT 1 FROM pg_roles WHERE rolname = '${user}';")" != "1" ]; then
    createuser -h "$host" -p "$port" "$user"
  fi
  psql -h "$host" -p "$port" -d postgres -v ON_ERROR_STOP=1 -c "ALTER ROLE \"${user}\" WITH LOGIN PASSWORD '${escaped_password}';" >/dev/null
  if [ "$(psql -h "$host" -p "$port" -d postgres -tAc "SELECT 1 FROM pg_database WHERE datname = '${db}';")" != "1" ]; then
    createdb -h "$host" -p "$port" -O "$user" "$db"
  fi
  echo "PostgreSQL is ready at postgresql://${user}:***@${host}:${port}/${db}"

# Stop the local PostgreSQL helper instance
stop-postgres:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  data_dir="${CDK_POSTGRES_DATA_DIR:-.pg_data}"
  if [ ! -s "${data_dir}/PG_VERSION" ]; then
    echo "No local PostgreSQL data directory found at ${data_dir}"
    exit 0
  fi
  if ! pg_ctl -D "$data_dir" status >/dev/null 2>&1; then
    echo "Local PostgreSQL helper is not running"
    exit 0
  fi
  pg_ctl -D "$data_dir" stop -m fast

# Show local PostgreSQL helper status
pg-status:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  data_dir="${CDK_POSTGRES_DATA_DIR:-.pg_data}"
  host="${CDK_POSTGRES_HOST:-127.0.0.1}"
  port="${CDK_POSTGRES_PORT:-5432}"
  db="${CDK_POSTGRES_DB:-cdk_mint}"
  pg_ctl -D "$data_dir" status || true
  pg_isready -h "$host" -p "$port" -d "$db" || true

# Open psql against the local PostgreSQL helper database
pg-connect:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  host="${CDK_POSTGRES_HOST:-127.0.0.1}"
  port="${CDK_POSTGRES_PORT:-5432}"
  db="${CDK_POSTGRES_DB:-cdk_mint}"
  user="${CDK_POSTGRES_USER:-cdk}"
  psql -h "$host" -p "$port" -U "$user" -d "$db"

coverage:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  # Clean up previous coverage data
  cargo llvm-cov clean --workspace

  # Start PostgreSQL for cdk-postgres tests
  echo "Starting PostgreSQL for coverage..."
  just start-postgres

  # Run unit tests with coverage (all workspace crates including cdk-postgres)
  echo "Running unit tests coverage..."
  cargo llvm-cov --no-report --lib --workspace --exclude cdk-integration-tests

  # Run cdk-nostr tests with all features; its npubcash/nwc modules are
  # feature-gated, so the default-feature workspace run does not execute them.
  echo "Running cdk-nostr feature-gated tests coverage..."
  cargo llvm-cov --no-report --lib -p cdk-nostr --all-features

  # Run pure integration tests with coverage (memory backend)
  echo "Running pure integration tests coverage (memory)..."
  CDK_TEST_DB_TYPE=memory cargo llvm-cov --no-report -p cdk-integration-tests --test integration_tests_pure

  # Run pure integration tests with coverage (sqlite backend)
  echo "Running pure integration tests coverage (sqlite)..."
  CDK_TEST_DB_TYPE=sqlite cargo llvm-cov --no-report -p cdk-integration-tests --test integration_tests_pure

  # Run NWC e2e tests with coverage (requires nostr-rs-relay on PATH)
  echo "Running NWC e2e tests coverage (memory)..."
  CDK_TEST_DB_TYPE=memory cargo llvm-cov --no-report -p cdk-integration-tests --test nwc_e2e

  # Run NIP-17 inbox e2e tests with coverage (requires nostr-rs-relay on PATH)
  echo "Running NIP-17 inbox e2e tests coverage..."
  cargo llvm-cov --no-report -p cdk-integration-tests --test nip17_inbox_e2e

  # Generate report
  echo "Generating coverage report..."
  cargo llvm-cov report --lcov --output-path lcov.info

test-pure db="memory":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi

  if [ -n "${CDK_ITEST_ARCHIVE:-}" ] && [ -f "$CDK_ITEST_ARCHIVE" ]; then
    # Run pure integration tests from nextest archive (extracted once, reused)
    source ./misc/itest_helpers.sh
    export CDK_TEST_DB_TYPE={{db}}
    run_test integration_tests_pure -j 1
    run_test test_swap_flow -j 1
    run_test wallet_saga -j 1
    run_test nwc_e2e -j 1
  else
    # Run pure integration tests (cargo test will only build what's needed for the test)
    CDK_TEST_DB_TYPE={{db}} cargo test -p cdk-integration-tests --test integration_tests_pure -- --test-threads 1

    # Run swap flow tests (detailed testing of swap operation)
    CDK_TEST_DB_TYPE={{db}} cargo test -p cdk-integration-tests --test test_swap_flow -- --test-threads 1

    # Run wallet saga tests
    CDK_TEST_DB_TYPE={{db}} cargo test -p cdk-integration-tests --test wallet_saga -- --test-threads 1

    # Run NWC (NIP-47) e2e tests (requires nostr-rs-relay on PATH; skipped locally if absent)
    CDK_TEST_DB_TYPE={{db}} cargo test -p cdk-integration-tests --test nwc_e2e -- --test-threads 1
  fi

# Run Redis cache clippy and unit tests against both single-node and cluster Redis.
# Starts a single-node Redis (port 6379) and a 3-node cluster (ports 7001-7003)
# using Nix-managed scripts, mirrors both code paths in cdk-axum cache.
test-redis:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi

  # Start both Redis topologies and stop them on exit
  trap 'stop-redis-single; stop-redis-cluster' EXIT
  start-redis-single
  start-redis-cluster

  # Lint once (compile-time only, topology is irrelevant for clippy)
  CDK_MINTD_CACHE_BACKEND=redis \
  CDK_MINTD_CACHE_REDIS_USE_CLUSTER=false \
  CDK_MINTD_CACHE_REDIS_URL=redis://127.0.0.1:6379 \
    cargo clippy -p cdk-axum --features redis -- -D warnings

  # Run the full lib test suite once with both endpoint vars present.
  # Each test constructs its own topology explicitly so a single invocation
  # exercises both the single-node and cluster paths.
  CDK_MINTD_CACHE_BACKEND=redis \
  CDK_MINTD_CACHE_REDIS_URL=redis://127.0.0.1:6379 \
  CDK_MINTD_CACHE_REDIS_CLUSTER_NODES=redis://127.0.0.1:7001,redis://127.0.0.1:7002,redis://127.0.0.1:7003 \
    cargo test -p cdk-axum --features redis,integration-tests --lib


# Mutation Testing Commands

# Run mutation tests on a specific crate
# Usage: just mutants <crate-name>
# Example: just mutants cashu
mutants CRATE:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutation tests on crate: {{CRATE}}"
  cargo mutants --package {{CRATE}} -vV

# Run mutation tests on the cashu crate
mutants-cashu:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutation tests on cashu crate..."
  cargo mutants --package cashu -vV

# Run mutation tests on NUT-27 with wallet+nostr features enabled
mutants-cashu-nut27-nostr:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutation tests on cashu NUT-27 with Nostr features..."
  cargo mutants --package cashu --no-config --file crates/cashu/src/nuts/nut27.rs --features wallet,nostr -vV

# Run mutation tests on the cdk crate
mutants-cdk:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutation tests on cdk crate..."
  cargo mutants --package cdk -vV

# Run mutation tests on entire workspace (WARNING: very slow)
mutants-all:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutation tests on entire workspace..."
  echo "WARNING: This may take a very long time!"
  cargo mutants -vV

# Quick mutation test for current work (alias for mutants-diff)
mutants-quick:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutations on changed files since HEAD..."
  cargo mutants --in-diff HEAD -vV

# Fuzzing Commands

# Run fuzzing on a specific target
# Usage: just fuzz <target> [duration] [jobs]
# Example: just fuzz fuzz_token
# Example: just fuzz fuzz_token 60
# Example: just fuzz fuzz_token 60 4  (run for 60s on 4 cores)
fuzz TARGET DURATION="0" JOBS="1":
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running fuzzer on target: {{TARGET}} (jobs: {{JOBS}})"
  cd fuzz
  # Create corpus directory if it doesn't exist
  mkdir -p "corpus/{{TARGET}}"
  # Use seeds directory if it exists
  SEEDS_DIR=""
  if [ -d "seeds/{{TARGET}}" ]; then
    SEEDS_DIR="seeds/{{TARGET}}"
  fi
  FORK_FLAG=""
  if [ "{{JOBS}}" != "1" ]; then
    FORK_FLAG="-fork={{JOBS}}"
  fi
  CARGO_FUZZ=(cargo fuzz)
  if command -v rustup >/dev/null 2>&1 && NIGHTLY_CARGO="$(rustup which --toolchain nightly cargo 2>/dev/null)"; then
    export PATH="${NIGHTLY_CARGO%/cargo}:$PATH"
  fi
  if [ "{{DURATION}}" = "0" ]; then
    "${CARGO_FUZZ[@]}" run {{TARGET}} corpus/{{TARGET}} $SEEDS_DIR -- $FORK_FLAG
  else
    "${CARGO_FUZZ[@]}" run {{TARGET}} corpus/{{TARGET}} $SEEDS_DIR -- -max_total_time={{DURATION}} $FORK_FLAG
  fi

# List available fuzz targets
fuzz-list:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Available fuzz targets:"
  cd fuzz
  cargo fuzz list

# Run all fuzz targets for a short duration (useful for CI)
# Usage: just fuzz-ci [duration] [jobs]
# Example: just fuzz-ci 30 4  (run each target for 30s on 4 cores)
fuzz-ci DURATION="30" JOBS="1":
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running all fuzz targets for {{DURATION}} seconds each (jobs: {{JOBS}})..."
  cd fuzz
  FORK_FLAG=""
  if [ "{{JOBS}}" != "1" ]; then
    FORK_FLAG="-fork={{JOBS}}"
  fi
  CARGO_FUZZ=(cargo fuzz)
  if command -v rustup >/dev/null 2>&1 && NIGHTLY_CARGO="$(rustup which --toolchain nightly cargo 2>/dev/null)"; then
    export PATH="${NIGHTLY_CARGO%/cargo}:$PATH"
  fi
  for target in $(cargo fuzz list); do
    echo "Fuzzing $target..."
    mkdir -p "corpus/$target"
    SEEDS_DIR=""
    if [ -d "seeds/$target" ]; then
      SEEDS_DIR="seeds/$target"
    fi
    "${CARGO_FUZZ[@]}" run "$target" "corpus/$target" $SEEDS_DIR -- -max_total_time={{DURATION}} $FORK_FLAG
  done
  echo "All fuzz targets completed!"

# Run the security-sensitive federation and FROST decoder corpus.
# Set DURATION=0 for a continuous local campaign.
fuzz-federation-security DURATION="60" JOBS="1":
  #!/usr/bin/env bash
  set -euo pipefail
  for target in \
    fuzz_frost_wire \
    fuzz_federation_security_wire \
    fuzz_federation_operation \
    fuzz_federation_transaction_proposal
  do
    just fuzz "$target" "{{DURATION}}" "{{JOBS}}"
  done

# Run mutation tests only on changed code since HEAD
mutants-diff:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "Running mutation tests on changed code..."
  cargo mutants --in-diff HEAD -vV

# Run mutation tests and save output to log file
# Usage: just mutants-log <crate-name> <log-suffix>
# Example: just mutants-log cashu baseline
mutants-log CRATE SUFFIX:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  LOG_FILE="mutants-{{CRATE}}-{{SUFFIX}}.log"
  echo "Running mutation tests on {{CRATE}}, saving to $LOG_FILE..."
  cargo mutants --package {{CRATE}} -vV 2>&1 | tee "$LOG_FILE"
  echo "Results saved to $LOG_FILE"

# Mutation test with baseline comparison
# Usage: just mutants-check <crate-name>
# Example: just mutants-check cashu
mutants-check CRATE:
  #!/usr/bin/env bash
  set -euo pipefail
  BASELINE="mutants-{{CRATE}}-baseline.log"
  if [ ! -f "$BASELINE" ]; then
    echo "ERROR: No baseline found at $BASELINE"
    echo "Run: just mutants-log {{CRATE}} baseline"
    exit 1
  fi
  cargo mutants --package {{CRATE}} -vV | tee mutants-{{CRATE}}-current.log
  # Compare results
  echo "=== Baseline vs Current ==="
  diff <(grep "^CAUGHT\|^MISSED" "$BASELINE" | wc -l) \
       <(grep "^CAUGHT\|^MISSED" mutants-{{CRATE}}-current.log | wc -l) || true

test-nutshell:
  #!/usr/bin/env bash
  set -euo pipefail

  # Function to cleanup docker containers
  cleanup() {
    echo "Cleaning up docker containers..."
    docker stop nutshell 2>/dev/null || true
    docker rm nutshell 2>/dev/null || true
    unset CDK_ITESTS_DIR
  }

  # Trap to ensure cleanup happens on exit (success or failure)
  trap cleanup EXIT

  # Clean up any leftover containers from previous runs
  docker stop nutshell 2>/dev/null || true
  docker rm nutshell 2>/dev/null || true

  docker run -d --network=host --name nutshell -e MINT_LIGHTNING_BACKEND=FakeWallet -e MINT_LISTEN_HOST=0.0.0.0 -e MINT_LISTEN_PORT=3338 -e MINT_PRIVATE_KEY=TEST_PRIVATE_KEY -e MINT_INPUT_FEE_PPK=100  cashubtc/nutshell:latest poetry run mint

  export CDK_ITESTS_DIR=$(mktemp -d)

  # Wait for the Nutshell service to be ready
  echo "Waiting for Nutshell to start..."
  max_attempts=30
  attempt=0
  while ! curl -s http://127.0.0.1:3338/v1/info > /dev/null; do
    attempt=$((attempt+1))
    if [ $attempt -ge $max_attempts ]; then
      echo "Nutshell failed to start after $max_attempts attempts"
      echo "=== Docker container status ==="
      docker ps -a --filter name=nutshell
      echo "=== Docker logs ==="
      docker logs nutshell 2>&1 || true
      exit 1
    fi
    echo "Waiting for Nutshell to start (attempt $attempt/$max_attempts)..."
    # Show container status every 10 attempts
    if [ $((attempt % 10)) -eq 0 ]; then
      echo "=== Container status check ==="
      docker ps -a --filter name=nutshell
    fi
    sleep 1
  done
  echo "Nutshell is ready!"

  # Set environment variables and run tests
  export CDK_TEST_MINT_URL=http://127.0.0.1:3338
  export PAYMENT_BACKEND=FAKEWALLET

  # Track test results
  test_exit_code=0

  # Run first test and capture exit code
  # Serialize tests (--test-threads=1) to avoid concurrent writes against
  # Nutshell's SQLite backend, which can cause transient lock-acquisition
  # failures under parallel load.
  echo "Running happy_path_mint_wallet test..."
  if ! cargo test -p cdk-integration-tests --test happy_path_mint_wallet -- --test-threads=1; then
    echo "ERROR: happy_path_mint_wallet test failed"
    test_exit_code=1
  fi

  # Run second test and capture exit code
  echo "Running test_fees test..."
  if ! cargo test -p cdk-integration-tests --test test_fees -- --test-threads=1; then
    echo "ERROR: test_fees test failed"
    test_exit_code=1
  fi

  unset CDK_TEST_MINT_URL
  unset PAYMENT_BACKEND

  # Exit with error code if any test failed
  if [ $test_exit_code -ne 0 ]; then
    echo "One or more tests failed"
    exit $test_exit_code
  fi

  echo "All tests passed successfully"


# run `cargo clippy` on everything
clippy *ARGS="--workspace --all-targets":
  cargo clippy {{ARGS}} -- -D warnings

# run `cargo clippy --fix` on everything
clippy-fix *ARGS="--workspace --all-targets":
  cargo clippy {{ARGS}} --fix

typos:
  typos

# fix all typos
[no-exit-message]
typos-fix:
  just typos -w

# run all linting checks (format check, typos, nix format)
lint:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  echo "Checking Rust formatting..."
  cargo fmt --all -- --check
  echo "Checking Nix formatting..."
  nixpkgs-fmt --check $(echo **.nix)
  echo "Checking for typos..."
  typos
  echo "All checks passed!"

# Refresh stable and MSRV lockfiles, excluding registry releases newer than seven days.
update-msrv-lock:
    #!/usr/bin/env bash
    set -euo pipefail

    nix develop --ignore-environment .#nightly --command bash -c '
        export CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE="7 days"
        cargo_update() {
            cargo -Z min-publish-age update "$@"
        }

        cargo_update
        echo "Updated Cargo.lock with a seven-day minimum publish age"

        lock_dir=$(mktemp -d /data/rust/tmp/cdk-msrv-lock.XXXXXX)
        cleanup() {
            rm -f "$lock_dir/Cargo.lock"
            rmdir "$lock_dir" 2>/dev/null || true
        }
        trap cleanup EXIT

        cp Cargo.lock "$lock_dir/Cargo.lock"
        export CARGO_RESOLVER_LOCKFILE_PATH="$lock_dir/Cargo.lock"
        export CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS="fallback"

        cargo_update
        cargo_update home --precise 0.5.11
        cargo_update typed-index-collections --precise 3.3.0
        cargo_update simple_asn1 --precise 0.6.3
        cargo_update cookie_store --precise 0.22.0
        cargo_update serde_with --precise 3.17.0
        cargo_update time --precise 0.3.44
        cargo_update unicode-segmentation --precise 1.12.0
        cargo_update idna_adapter --precise 1.2.1
        cargo_update icu_normalizer --precise 2.0.1
        cargo_update icu_provider --precise 2.1.1
        cargo_update icu_locale_core --precise 2.1.1
        cp "$lock_dir/Cargo.lock" Cargo.lock.msrv
        echo "Updated Cargo.lock.msrv for MSRV 1.85.0"
    '

itest db suite="all":
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/itests.sh "{{db}}" "{{suite}}"

itest-ln db:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/itests.sh "{{db}}" ln

itest-onchain db:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/itests.sh "{{db}}" onchain

fake-mint-itest db:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/fake_itests.sh "{{db}}"
  bash ./misc/fake_itests.sh "{{db}}" external_signatory

itest-payment-processor ln:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/mintd_payment_processor.sh "{{ln}}"

fake-auth-mint-itest db openid_discovery:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/fake_auth_itests.sh "{{db}}" "{{openid_discovery}}"

nutshell-wallet-itest:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/nutshell_wallet_itest.sh

# Start interactive regtest environment (Bitcoin + 4 LN nodes + 2 CDK mints)
regtest db="sqlite" host="127.0.0.1":
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/interactive_regtest_mprocs.sh {{db}} "{{host}}"

# Generate local fake-wallet federation configs for process smoke testing
fed-regtest-generate dir="target/cdk-fed-regtest" members="5" threshold="3" port_start="21885" dkg_setup_route_probes="0" checkpoint_interval_journal_operations="20" checkpoint_interval_finalized_items="100":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  dkg_probe_arg=""
  if [[ "{{dkg_setup_route_probes}}" == "1" ]]; then
    dkg_probe_arg="--dkg-setup-route-probes"
  fi
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  target/debug/cdk-mintd federation test-trusted-dealer \
    --output-dir "{{dir}}" \
    --force \
    --local-fakewallet-configs \
    --members "{{members}}" \
    --threshold "{{threshold}}" \
    --port-start "{{port_start}}" \
    --checkpoint-interval-journal-operations "{{checkpoint_interval_journal_operations}}" \
    --checkpoint-interval-finalized-items "{{checkpoint_interval_finalized_items}}" \
    ${dkg_probe_arg:+$dkg_probe_arg}

# Run fast federation regtest and smoke unit checks without starting member processes
fed-regtest-check:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo test -p cdk-mintd --features federation-test-utils,sqlite federation_smoke -- --test-threads 1
  cargo test -p cdk-mintd --features federation-test-utils,sqlite smoke_operations -- --test-threads 1
  cargo test -p cdk-mintd --features federation-test-utils,sqlite federation_setup -- --test-threads 1
  cargo test -p cdk-mintd --features federation-test-utils,sqlite federation_dkg_status -- --test-threads 1

# Run the generated local fake-wallet federation smoke
fed-regtest dir="target/cdk-fed-regtest" members="5" threshold="3" port_start="21885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run a real cdk-cli wallet smoke against generated fake-wallet federation members
fed-regtest-cli-smoke dir="target/cdk-fed-regtest-cli" wallet_dir="target/cdk-fed-regtest-cli-wallet" members="5" threshold="3" port_start="62885" offline_member="0":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-60}"
  wait_for_cli_smoke_status() {
    local attempt
    for attempt in $(seq 1 60); do
      if just fed-regtest-status "{{dir}}" "1"; then
        return 0
      fi
      sleep 1
    done
    printf 'federation did not become ready for CLI smoke\n' >&2
    return 1
  }
  cargo build -p cdk-cli --features federation
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-start "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  trap 'just fed-regtest-stop "{{dir}}"' EXIT
  wait_for_cli_smoke_status
  if [[ "{{offline_member}}" != "0" ]]; then
    just fed-regtest-stop-member "{{offline_member}}" "{{dir}}"
  fi
  rm -rf -- "{{wallet_dir}}" "{{wallet_dir}}-restore" "{{wallet_dir}}-no-change"
  mint_url="http://127.0.0.1:{{port_start}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --member-request-timeout-secs 60 \
    --member-fallback-timeout-secs 60 \
    --quote-status-member-timeout-secs 60
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive mint-info "$mint_url" >/dev/null
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    mint "$mint_url" 20 --wait-duration 90
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-local.out"
  grep -Eq 'Local proof states: total=[0-9]+ .*unspent=[1-9][0-9]*' "{{wallet_dir}}/check-local.out"
  token="$(target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat send --mint-url "$mint_url" --amount 7 | tail -n 1)"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat receive "$token"
  set +e
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    receive "$token" > "{{wallet_dir}}/receive-spent.out" 2>&1
  receive_spent_status="$?"
  set -e
  cat "{{wallet_dir}}/receive-spent.out"
  if [[ "$receive_spent_status" == "0" ]]; then
    printf 'receiving an already-spent federation token unexpectedly succeeded\n' >&2
    exit 1
  fi
  grep -Fqi 'already spent' "{{wallet_dir}}/receive-spent.out"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --token "$token" | tee "{{wallet_dir}}/check-token.out"
  grep -Eq 'Token proof states: total=[0-9]+ .*spent=[1-9][0-9]*' "{{wallet_dir}}/check-token.out"
  invoice="$(target/debug/cdk-mintd federation fake-invoice --amount-msat 10000)"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    melt --mint-url "$mint_url" --method bolt11 --invoice "$invoice"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-no-change" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --member-request-timeout-secs 60 \
    --member-fallback-timeout-secs 60 \
    --quote-status-member-timeout-secs 60
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-no-change" --non-interactive --unit sat \
    mint "$mint_url" 11 --wait-duration 90
  no_change_invoice="$(target/debug/cdk-mintd federation fake-invoice --amount-msat 10000)"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-no-change" --non-interactive --unit sat \
    melt --mint-url "$mint_url" --method bolt11 --invoice "$no_change_invoice"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-no-change" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-no-change-melt.out"
  grep -Fq 'No local proofs found' "{{wallet_dir}}/check-no-change-melt.out"
  mkdir -p "{{wallet_dir}}-restore"
  cp "{{wallet_dir}}/seed" "{{wallet_dir}}-restore/seed"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-restore" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --member-request-timeout-secs 60 \
    --member-fallback-timeout-secs 60 \
    --quote-status-member-timeout-secs 60
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-restore" --non-interactive --unit sat \
    restore "$mint_url" | tee "{{wallet_dir}}/restore.out"
  grep -Eq 'Restored: [1-9][0-9]*' "{{wallet_dir}}/restore.out"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat balance
  if [[ "{{offline_member}}" != "0" ]]; then
    just fed-regtest-start-member "{{offline_member}}" "{{dir}}" "0"
  fi
  wait_for_cli_smoke_status
  printf 'cdk-cli federation smoke passed\n'

# Run the CLI federation smoke while one member is offline, then restart it and require catch-up
fed-regtest-cli-smoke-offline dir="target/cdk-fed-regtest-cli-offline" wallet_dir="target/cdk-fed-regtest-cli-offline-wallet" members="5" threshold="3" port_start="64885" offline_member="5":
  just fed-regtest-cli-smoke "{{dir}}" "{{wallet_dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{offline_member}}"

# Run CLI mint/swap/melt while one reachable member has a slow fakewallet payment backend
fed-regtest-cli-smoke-slow-member dir="target/cdk-fed-regtest-cli-slow" wallet_dir="target/cdk-fed-regtest-cli-slow-wallet" members="5" threshold="4" port_start="58885" slow_member="5" slow_delay_secs="30":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-60}"
  wait_for_fast_status() {
    local attempt
    local status_args=()
    local member
    for member in $(seq 1 "{{members}}"); do
      if [[ "$member" == "{{slow_member}}" ]]; then
        continue
      fi
      status_args+=(--member "$member")
    done
    for attempt in $(seq 1 60); do
      if target/debug/cdk-mintd federation status \
        --public-config "{{dir}}/federation-public.json" \
        --private-config "{{dir}}/member-1/federation-private.json" \
        "${status_args[@]}" \
        --require-ready \
        --require-converged \
        --require-checkpoint-quorum \
        --require-no-digest-alerts \
        --require-no-invalid-operation-alerts >/dev/null 2>&1; then
        return 0
      fi
      sleep 1
    done
    printf 'fast federation members did not become ready for slow-member CLI smoke\n' >&2
    target/debug/cdk-mintd federation status \
      --public-config "{{dir}}/federation-public.json" \
      --private-config "{{dir}}/member-1/federation-private.json" \
      "${status_args[@]}" || true
    return 1
  }
  wait_for_all_value_operations() {
    local attempt
    local status_output
    local expected_lines
    for attempt in $(seq 1 120); do
      if status_output="$(target/debug/cdk-mintd federation status \
        --public-config "{{dir}}/federation-public.json" \
        --private-config "{{dir}}/member-1/federation-private.json" \
        --require-ready \
        --require-converged \
        --require-checkpoint-quorum \
        --require-no-digest-alerts \
        --require-no-invalid-operation-alerts 2>/dev/null)"; then
        expected_lines="$(grep -Ec 'operations=10 latest_index=9 ' <<<"$status_output" || true)"
        if [[ "$expected_lines" == "{{members}}" ]]; then
          printf '%s\n' "$status_output" | tee "{{dir}}/slow-member-final-status.out"
          return 0
        fi
      fi
      sleep 1
    done
    printf 'all federation members did not converge on the ten expected value operations\n' >&2
    target/debug/cdk-mintd federation status \
      --public-config "{{dir}}/federation-public.json" \
      --private-config "{{dir}}/member-1/federation-private.json" || true
    return 1
  }
  wait_for_member_info() {
    local member="$1"
    local mint_url="http://127.0.0.1:$(({{port_start}} + member - 1))"
    local attempt
    for attempt in $(seq 1 30); do
      if curl -fsS "$mint_url/v1/info" >/dev/null 2>&1; then
        return 0
      fi
      sleep 1
    done
    printf 'member %s did not expose mint info at %s\n' "$member" "$mint_url" >&2
    return 1
  }
  start_slow_member() {
    local member="{{slow_member}}"
    local work_dir="{{dir}}/member-${member}/work"
    local config="{{dir}}/member-${member}/config.toml"
    local pid_file="${work_dir}/mintd.pid"
    mkdir -p "$work_dir"
    CDK_MINTD_FAKE_WALLET_MIN_DELAY="{{slow_delay_secs}}" \
    CDK_MINTD_FAKE_WALLET_MAX_DELAY="{{slow_delay_secs}}" \
      nohup target/debug/cdk-mintd --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
    echo "$!" > "$pid_file"
    echo "started slow member ${member} with pid $(cat "$pid_file")"
  }
  cargo build -p cdk-cli --features federation
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  trap 'just fed-regtest-stop "{{dir}}"' EXIT
  for member in $(seq 1 "{{members}}"); do
    if [[ "$member" == "{{slow_member}}" ]]; then
      start_slow_member
    else
      just fed-regtest-start-member "$member" "{{dir}}" "0"
    fi
  done
  for member in $(seq 1 "{{members}}"); do
    wait_for_member_info "$member"
  done
  wait_for_fast_status
  rm -rf -- "{{wallet_dir}}" "{{wallet_dir}}-change"
  mint_url="http://127.0.0.1:{{port_start}}"
  slow_mint_url="http://127.0.0.1:$(({{port_start}} + {{slow_member}} - 1))"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --quote-coordinator-member-id 1 \
    --member-request-timeout-secs 45 \
    --member-fallback-timeout-secs 45 \
    --quote-status-member-timeout-secs 45 \
    --mint-quote-visibility-attempts 20 \
    --mint-quote-visibility-retry-delay-ms 250 \
    --fanout-concurrency "{{members}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive mint-info "$mint_url" >/dev/null
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    mint "$mint_url" 12 --wait-duration 90
  receiver_pubkey="$(target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    generate-public-key --mint-url "$mint_url" | awk '/^public key: / {print $3}' | tail -n 1)"
  if [[ -z "$receiver_pubkey" ]]; then
    printf 'could not generate the receiver public key for the forced swap\n' >&2
    exit 1
  fi
  swap_token="$(/usr/bin/time -p target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    send --mint-url "$mint_url" --amount 3 --pubkey "$receiver_pubkey" \
    2>"{{wallet_dir}}/forced-swap.time" | tail -n 1)"
  if [[ "$swap_token" != cashu* ]]; then
    printf 'forced P2PK send did not return a Cashu token\n' >&2
    exit 1
  fi
  printf 'forced P2PK swap timing:\n'
  cat "{{wallet_dir}}/forced-swap.time"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-slow-member.out"
  grep -Eq 'Local proof states: total=[0-9]+ .*unspent=[1-9][0-9]*' "{{wallet_dir}}/check-slow-member.out"

  target/debug/cdk-cli --work-dir "{{wallet_dir}}-change" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --quote-coordinator-member-id "{{slow_member}}" \
    --member-request-timeout-secs 90 \
    --member-fallback-timeout-secs 90 \
    --quote-status-member-timeout-secs 90 \
    --mint-quote-visibility-attempts 20 \
    --mint-quote-visibility-retry-delay-ms 250 \
    --fanout-concurrency "{{members}}"
  slow_mint_started="$(date +%s)"
  /usr/bin/time -p target/debug/cdk-cli --work-dir "{{wallet_dir}}-change" --non-interactive --unit sat \
    mint "$slow_mint_url" 20 --wait-duration 120 \
    2>"{{wallet_dir}}/slow-coordinator-mint.time"
  slow_mint_elapsed="$(( $(date +%s) - slow_mint_started ))"
  if (( slow_mint_elapsed < {{slow_delay_secs}} - 2 )); then
    printf 'slow-member mint completed in %s seconds, below the configured %s second backend delay\n' \
      "$slow_mint_elapsed" "{{slow_delay_secs}}" >&2
    exit 1
  fi
  printf 'slow-member coordinator mint elapsed_seconds=%s\n' "$slow_mint_elapsed"
  cat "{{wallet_dir}}/slow-coordinator-mint.time"
  for _ in $(seq 1 4); do
    offline_token="$(target/debug/cdk-cli --work-dir "{{wallet_dir}}-change" --non-interactive --unit sat \
      send --mint-url "$slow_mint_url" --amount 1 --offline | tail -n 1)"
    if [[ "$offline_token" != cashu* ]]; then
      printf 'offline 1 sat send did not return a Cashu token\n' >&2
      exit 1
    fi
  done
  invoice="$(target/debug/cdk-mintd federation fake-invoice --amount-msat 10000)"
  /usr/bin/time -p target/debug/cdk-cli --work-dir "{{wallet_dir}}-change" --non-interactive --unit sat \
    melt --mint-url "$slow_mint_url" --method bolt11 --invoice "$invoice" \
    2>"{{wallet_dir}}/forced-change-melt.time"
  printf 'forced change melt timing:\n'
  cat "{{wallet_dir}}/forced-change-melt.time"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-change" --non-interactive --unit sat \
    balance | tee "{{wallet_dir}}/change-balance.out"
  grep -Eq '(^|[[:space:]])5 sat$' "{{wallet_dir}}/change-balance.out"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}-change" --non-interactive --unit sat \
    check-state --mint-url "$slow_mint_url" | tee "{{wallet_dir}}/check-change-melt.out"
  grep -Eq 'Local proof states: total=[0-9]+ .*unspent=[1-9][0-9]*' "{{wallet_dir}}/check-change-melt.out"

  wait_for_fast_status
  wait_for_all_value_operations
  printf 'cdk-cli slow-member mint/swap/change-melt smoke passed\n'

# Run the CLI federation smoke until threshold is unavailable and require mint failure without proofs
fed-regtest-cli-smoke-insufficient-threshold dir="target/cdk-fed-regtest-cli-insufficient" wallet_dir="target/cdk-fed-regtest-cli-insufficient-wallet" members="5" threshold="3" port_start="63885" offline_members="4 5":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-8}"
  wait_for_cli_smoke_status() {
    local attempt
    for attempt in $(seq 1 60); do
      if just fed-regtest-status "{{dir}}" "1"; then
        return 0
      fi
      sleep 1
    done
    printf 'federation did not become ready for insufficient-threshold CLI smoke\n' >&2
    return 1
  }
  cargo build -p cdk-cli --features federation
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-start "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  trap 'just fed-regtest-stop "{{dir}}"' EXIT
  wait_for_cli_smoke_status
  for member in {{offline_members}}; do
    just fed-regtest-stop-member "$member" "{{dir}}"
  done
  rm -rf -- "{{wallet_dir}}"
  mint_url="http://127.0.0.1:{{port_start}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --member-request-timeout-secs 3 \
    --member-fallback-timeout-secs 3 \
    --quote-status-member-timeout-secs 3 \
    --mint-quote-visibility-attempts 5 \
    --mint-quote-visibility-retry-delay-ms 100
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive mint-info "$mint_url" >/dev/null
  set +e
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    mint "$mint_url" 20 --wait-duration 8 > "{{wallet_dir}}/mint-insufficient.out" 2>&1
  mint_status="$?"
  set -e
  cat "{{wallet_dir}}/mint-insufficient.out"
  if [[ "$mint_status" == "0" ]]; then
    printf 'mint unexpectedly succeeded while threshold was unavailable\n' >&2
    exit 1
  fi
  if grep -Eq 'Minted [1-9][0-9]*|Received [1-9][0-9]*' "{{wallet_dir}}/mint-insufficient.out"; then
    printf 'mint output indicates spendable proofs were produced despite unavailable threshold\n' >&2
    exit 1
  fi
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-insufficient.out"
  grep -Fq 'No local proofs found' "{{wallet_dir}}/check-insufficient.out"
  printf 'cdk-cli insufficient-threshold smoke passed\n'

# Run the CLI federation smoke with one reachable member advertising mismatched federation metadata.
fed-regtest-cli-smoke-mismatched-member dir="target/cdk-fed-regtest-cli-mismatch" bad_dir="target/cdk-fed-regtest-cli-mismatch-bad" wallet_dir="target/cdk-fed-regtest-cli-mismatch-wallet" members="5" threshold="4" port_start="54885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-60}"
  bad_port_start=$(({{port_start}} + 100))
  wait_for_honest_status() {
    local attempt
    for attempt in $(seq 1 60); do
      if target/debug/cdk-mintd federation status \
        --public-config "{{dir}}/federation-public.json" \
        --private-config "{{dir}}/member-1/federation-private.json" \
        --member 1 \
        --member 3 \
        --member 4 \
        --member 5 \
        --require-ready \
        --require-converged \
        --require-checkpoint-quorum \
        --require-no-digest-alerts \
        --require-no-invalid-operation-alerts >/dev/null 2>&1; then
        return 0
      fi
      sleep 1
    done
    printf 'honest federation members did not become ready for mismatched-member CLI smoke\n' >&2
    target/debug/cdk-mintd federation status \
      --public-config "{{dir}}/federation-public.json" \
      --private-config "{{dir}}/member-1/federation-private.json" \
      --member 1 \
      --member 3 \
      --member 4 \
      --member 5 || true
    return 1
  }
  wait_for_mismatched_member_info() {
    local mint_url="http://127.0.0.1:$(({{port_start}} + 1))"
    local attempt
    for attempt in $(seq 1 30); do
      if curl -fsS "$mint_url/v1/info" >/dev/null 2>&1; then
        return 0
      fi
      sleep 1
    done
    printf 'mismatched member did not expose mint info at %s\n' "$mint_url" >&2
    return 1
  }
  cargo build -p cdk-cli --features federation
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-clean "{{bad_dir}}" >/dev/null 2>&1 || true
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  just fed-regtest-generate "{{bad_dir}}" "{{members}}" "{{threshold}}" "$bad_port_start"
  trap 'just fed-regtest-stop "{{dir}}"; just fed-regtest-stop "{{bad_dir}}"' EXIT
  for member in 1 3 4 5; do
    just fed-regtest-start-member "$member" "{{dir}}" "0"
  done
  work_dir="{{dir}}/member-2/work"
  config="{{dir}}/member-2/config.toml"
  pid_file="${work_dir}/mintd.pid"
  mkdir -p "$work_dir"
  CDK_MINTD_FEDERATION_PUBLIC_CONFIG_PATH="{{bad_dir}}/federation-public.json" \
  CDK_MINTD_FEDERATION_PRIVATE_CONFIG_PATH="{{bad_dir}}/member-1/federation-private.json" \
    nohup target/debug/cdk-mintd --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
  echo "$!" > "$pid_file"
  echo "started mismatched member 2 with pid $(cat "$pid_file")"
  wait_for_honest_status
  wait_for_mismatched_member_info
  rm -rf -- "{{wallet_dir}}"
  mint_url="http://127.0.0.1:{{port_start}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --quote-coordinator-member-id 1 \
    --member-request-timeout-secs 60 \
    --member-fallback-timeout-secs 60 \
    --quote-status-member-timeout-secs 60 \
    --fanout-concurrency 1
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive mint-info "$mint_url" >/dev/null
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    mint "$mint_url" 12 --wait-duration 90
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-mismatched-member.out"
  grep -Eq 'Local proof states: total=[0-9]+ .*unspent=[1-9][0-9]*' "{{wallet_dir}}/check-mismatched-member.out"
  wait_for_honest_status
  printf 'cdk-cli mismatched-member smoke passed\n'

# Run the CLI federation smoke until too many reachable members advertise mismatched metadata.
fed-regtest-cli-smoke-insufficient-valid-advertisements dir="target/cdk-fed-regtest-cli-invalid-advertisements" bad_dir="target/cdk-fed-regtest-cli-invalid-advertisements-bad" wallet_dir="target/cdk-fed-regtest-cli-invalid-advertisements-wallet" members="5" threshold="4" port_start="55885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-60}"
  bad_port_start=$(({{port_start}} + 100))
  wait_for_member_info() {
    local member="$1"
    local mint_url="http://127.0.0.1:$(({{port_start}} + member - 1))"
    local attempt
    for attempt in $(seq 1 30); do
      if curl -fsS "$mint_url/v1/info" >/dev/null 2>&1; then
        return 0
      fi
      sleep 1
    done
    printf 'member %s did not expose mint info at %s\n' "$member" "$mint_url" >&2
    return 1
  }
  start_mismatched_member() {
    local member="$1"
    local bad_private_member="$2"
    local work_dir="{{dir}}/member-${member}/work"
    local config="{{dir}}/member-${member}/config.toml"
    local pid_file="${work_dir}/mintd.pid"
    mkdir -p "$work_dir"
    CDK_MINTD_FEDERATION_PUBLIC_CONFIG_PATH="{{bad_dir}}/federation-public.json" \
    CDK_MINTD_FEDERATION_PRIVATE_CONFIG_PATH="{{bad_dir}}/member-${bad_private_member}/federation-private.json" \
      nohup target/debug/cdk-mintd --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
    echo "$!" > "$pid_file"
    echo "started mismatched member ${member} with pid $(cat "$pid_file")"
  }
  cargo build -p cdk-cli --features federation
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-clean "{{bad_dir}}" >/dev/null 2>&1 || true
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  just fed-regtest-generate "{{bad_dir}}" "{{members}}" "{{threshold}}" "$bad_port_start"
  trap 'just fed-regtest-stop "{{dir}}"; just fed-regtest-stop "{{bad_dir}}"' EXIT
  for member in 1 3 4; do
    just fed-regtest-start-member "$member" "{{dir}}" "0"
  done
  start_mismatched_member 2 1
  start_mismatched_member 5 2
  for member in 1 2 3 4 5; do
    wait_for_member_info "$member"
  done
  rm -rf -- "{{wallet_dir}}"
  mkdir -p "{{wallet_dir}}"
  mint_url="http://127.0.0.1:{{port_start}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --quote-coordinator-member-id 1 \
    --member-request-timeout-secs 60 \
    --member-fallback-timeout-secs 60 \
    --quote-status-member-timeout-secs 60 \
    --fanout-concurrency 1
  set +e
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    mint-info "$mint_url" > "{{wallet_dir}}/mint-info-invalid-advertisements.out" 2>&1
  mint_info_status="$?"
  set -e
  cat "{{wallet_dir}}/mint-info-invalid-advertisements.out"
  if [[ "$mint_info_status" == "0" ]]; then
    printf 'mint-info unexpectedly succeeded with insufficient valid member advertisements\n' >&2
    exit 1
  fi
  grep -Fq 'insufficient federation keyset advertisements' "{{wallet_dir}}/mint-info-invalid-advertisements.out"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-invalid-advertisements.out"
  grep -Fq 'No local proofs found' "{{wallet_dir}}/check-invalid-advertisements.out"
  printf 'cdk-cli insufficient-valid-advertisements smoke passed\n'

# Run the CLI federation smoke until too many reachable members omit federation metadata.
fed-regtest-cli-smoke-missing-metadata dir="target/cdk-fed-regtest-cli-missing-metadata" wallet_dir="target/cdk-fed-regtest-cli-missing-metadata-wallet" members="5" threshold="4" port_start="56885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  export CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS="${CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS:-60}"
  wait_for_member_info() {
    local member="$1"
    local mint_url="http://127.0.0.1:$(({{port_start}} + member - 1))"
    local attempt
    for attempt in $(seq 1 30); do
      if curl -fsS "$mint_url/v1/info" >/dev/null 2>&1; then
        return 0
      fi
      sleep 1
    done
    printf 'member %s did not expose mint info at %s\n' "$member" "$mint_url" >&2
    return 1
  }
  start_standalone_member() {
    local member="$1"
    local work_dir="{{dir}}/member-${member}/work"
    local config="{{dir}}/member-${member}/config.toml"
    local pid_file="${work_dir}/mintd.pid"
    mkdir -p "$work_dir"
    CDK_MINTD_FEDERATION_ENABLED=false \
      nohup target/debug/cdk-mintd --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
    echo "$!" > "$pid_file"
    echo "started standalone member ${member} with pid $(cat "$pid_file")"
  }
  cargo build -p cdk-cli --features federation
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  trap 'just fed-regtest-stop "{{dir}}"' EXIT
  for member in 1 3 4; do
    just fed-regtest-start-member "$member" "{{dir}}" "0"
  done
  start_standalone_member 2
  start_standalone_member 5
  for member in 1 2 3 4 5; do
    wait_for_member_info "$member"
  done
  rm -rf -- "{{wallet_dir}}"
  mkdir -p "{{wallet_dir}}"
  mint_url="http://127.0.0.1:{{port_start}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --quote-coordinator-member-id 1 \
    --member-request-timeout-secs 60 \
    --member-fallback-timeout-secs 60 \
    --quote-status-member-timeout-secs 60 \
    --fanout-concurrency 1
  set +e
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    mint-info "$mint_url" > "{{wallet_dir}}/mint-info-missing-metadata.out" 2>&1
  mint_info_status="$?"
  set -e
  cat "{{wallet_dir}}/mint-info-missing-metadata.out"
  if [[ "$mint_info_status" == "0" ]]; then
    printf 'mint-info unexpectedly succeeded with insufficient federation metadata\n' >&2
    exit 1
  fi
  grep -Eiq 'federation mint[- ]info' "{{wallet_dir}}/mint-info-missing-metadata.out"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-missing-metadata.out"
  grep -Fq 'No local proofs found' "{{wallet_dir}}/check-missing-metadata.out"
  printf 'cdk-cli missing-metadata smoke passed\n'

# Run the CLI federation smoke with an imported federation whose members are all unreachable.
fed-regtest-cli-smoke-all-members-unreachable dir="target/cdk-fed-regtest-cli-all-unreachable" wallet_dir="target/cdk-fed-regtest-cli-all-unreachable-wallet" members="5" threshold="3" port_start="57885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo build -p cdk-cli --features federation
  just fed-regtest-clean "{{dir}}" >/dev/null 2>&1 || true
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  rm -rf -- "{{wallet_dir}}"
  mkdir -p "{{wallet_dir}}"
  mint_url="http://127.0.0.1:{{port_start}}"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    federation add --public-config "{{dir}}/federation-public.json" \
    --member-request-timeout-secs 1 \
    --member-fallback-timeout-secs 1 \
    --quote-status-member-timeout-secs 1 \
    --mint-quote-visibility-attempts 2 \
    --mint-quote-visibility-retry-delay-ms 50
  set +e
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive \
    mint-info "$mint_url" > "{{wallet_dir}}/mint-info-all-unreachable.out" 2>&1
  mint_info_status="$?"
  set -e
  cat "{{wallet_dir}}/mint-info-all-unreachable.out"
  if [[ "$mint_info_status" == "0" ]]; then
    printf 'mint-info unexpectedly succeeded while all federation members were unreachable\n' >&2
    exit 1
  fi
  grep -Fq 'insufficient federation keyset advertisements' "{{wallet_dir}}/mint-info-all-unreachable.out"
  target/debug/cdk-cli --work-dir "{{wallet_dir}}" --non-interactive --unit sat \
    check-state --mint-url "$mint_url" | tee "{{wallet_dir}}/check-all-unreachable.out"
  grep -Fq 'No local proofs found' "{{wallet_dir}}/check-all-unreachable.out"
  printf 'cdk-cli all-members-unreachable smoke passed\n'

# Run generated local federation smoke with a longer offline operation window
fed-regtest-long-partition dir="target/cdk-fed-regtest-long-partition" members="5" threshold="3" port_start="25885" offline_batches="8":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  LONG_PARTITION_OPERATION_BATCHES="{{offline_batches}}" CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run generated local federation smoke with AlephBFT auto-rotation enabled
fed-regtest-aleph-rotation dir="target/cdk-fed-regtest-aleph-rotation" members="5" threshold="3" port_start="27885" offline_batches="8" max_round="6" target_session="1":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
    ALEPH_BFT_AUTO_ROTATION_MAX_ROUND="{{max_round}}" \
    ALEPH_BFT_AUTO_ROTATION_TARGET_SESSION="{{target_session}}" \
    LONG_PARTITION_OPERATION_BATCHES="{{offline_batches}}" \
    CDK_MINTD_BIN=target/debug/cdk-mintd \
    bash "{{dir}}/smoke-local-federation.sh"

# Run the generated setup-only DKG route-probe and restart activation smoke
fed-regtest-dkg-setup dir="target/cdk-fed-regtest-dkg" members="5" threshold="3" port_start="23885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "1"
  DKG_ROUTE_PROBE_ONLY=1 CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run generated DKG route probes and then continue through the full local federation smoke
fed-regtest-dkg-full dir="target/cdk-fed-regtest-dkg-full" members="5" threshold="3" port_start="23985":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "1"
  PROBE_DKG_SETUP_ROUTES=1 CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run a dealer-free live DKG ceremony, interruption/resume, activation restart, and wallet smoke
fed-regtest-live-dkg dir="target/cdk-fed-regtest-live-dkg" members="5" threshold="3" port_start="25985":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
    CDK_MINTD_BIN=target/debug/cdk-mintd \
    bash "{{dir}}/smoke-live-dkg-federation.sh"

# Run the real dealer-free dual DKG and federated Taproot BDK wallet against Bitcoin Core regtest
fed-regtest-frost-bdk dir="target/cdk-fed-regtest-frost-bdk" members="5" threshold="3" port_start="26985" fault_profile="happy" bitcoin_image="bitcoin/bitcoin:28.1":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo_profile="${FROST_BDK_CARGO_PROFILE:-dev}"
  cargo_profile_args=()
  case "$cargo_profile" in
    dev)
      cargo_profile_args+=(--profile dev)
      cargo_target_dir="target/debug"
      ;;
    release)
      cargo_profile_args+=(--release)
      cargo_target_dir="target/release"
      ;;
    *)
      printf 'FROST_BDK_CARGO_PROFILE must be dev or release\n' >&2
      exit 1
      ;;
  esac
  container="cdk-frost-bdk-regtest"
  started_container=0
  cleanup_core() {
    if [[ "$started_container" == "1" ]]; then
      docker stop "$container" >/dev/null 2>&1 || true
    fi
  }
  trap cleanup_core EXIT INT TERM
  if ! docker inspect "$container" >/dev/null 2>&1; then
    docker run --detach --rm --name "$container" \
      --publish 127.0.0.1:18443:18443 \
      "{{bitcoin_image}}" \
      -regtest=1 -server=1 -txindex=1 -fallbackfee=0.0002 \
      -rpcbind=0.0.0.0 -rpcallowip=0.0.0.0/0 -rpcuser=cdk -rpcpassword=cdk \
      >/dev/null
    started_container=1
  fi
  cargo_features=federation
  if [[ "${FROST_BDK_CAPTURE_RUNTIME_METRICS:-0}" == "1" ]]; then
    cargo_features="${cargo_features},prometheus"
  fi
  cargo build -p cdk-mintd -p cdk-cli --features "$cargo_features" "${cargo_profile_args[@]}"
  just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  FROST_BDK_REGTEST=1 \
    FROST_BDK_FAULT_PROFILE="{{fault_profile}}" \
    FROST_BDK_RUNTIME_BUILD_PROFILE="$cargo_profile" \
    BITCOIN_DOCKER_CONTAINER="$container" \
    CDK_MINTD_BIN="$cargo_target_dir/cdk-mintd" \
    CDK_CLI_BIN="$cargo_target_dir/cdk-cli" \
    bash "{{dir}}/smoke-live-dkg-federation.sh"

# Slow production-size FROST BDK smoke profiles; run explicitly, never in routine unit tests
fed-regtest-frost-bdk-8-of-11 dir="target/cdk-fed-regtest-frost-bdk-8-of-11" port_start="28985":
  FROST_BDK_CARGO_PROFILE=release STATUS_RETRIES=600 SMOKE_OPERATION_TIMEOUT_SECONDS=600 FROST_BDK_WAIT_RETRIES=900 FROST_BDK_MEMBER_REQUEST_TIMEOUT_SECONDS=300 just fed-regtest-frost-bdk "{{dir}}" 11 8 "{{port_start}}" happy

fed-regtest-frost-bdk-14-of-20 dir="target/cdk-fed-regtest-frost-bdk-14-of-20" port_start="30985":
  FROST_BDK_CARGO_PROFILE=release STATUS_RETRIES=1200 SMOKE_OPERATION_TIMEOUT_SECONDS=1200 FROST_BDK_WAIT_RETRIES=1800 FROST_BDK_MEMBER_REQUEST_TIMEOUT_SECONDS=600 just fed-regtest-frost-bdk "{{dir}}" 20 14 "{{port_start}}" happy

# Leave a fresh 3-of-5 federation idle across two signing leases, then deposit and melt
fed-regtest-frost-bdk-long-idle dir="target/cdk-fed-regtest-frost-bdk-long-idle" port_start="33985" idle_seconds="65":
  FROST_BDK_LONG_IDLE_SECONDS="{{idle_seconds}}" FROST_BDK_CAPTURE_RUNTIME_METRICS=1 just fed-regtest-frost-bdk "{{dir}}" 5 3 "{{port_start}}" happy

# Bounded process-level fault profiles using fresh state for every run
fed-regtest-frost-bdk-fault-matrix dir="target/cdk-fed-regtest-frost-bdk-faults" port_start="32985":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  profiles=(happy offline-one process-kill below-threshold-heal)
  offset=0
  for profile in "${profiles[@]}"; do
    just fed-regtest-frost-bdk "{{dir}}-$profile" 5 3 "$(({{port_start}} + offset))" "$profile"
    offset=$((offset + 100))
  done

# Run the bounded, in-memory dealer-free DKG phase benchmark
dkg-perf profile="5-of-3" denominations="one" iterations="3" build="release" phases="all" timeout_secs="300" full_ceremony="0":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo_args=(run -p cdk-common --features federation,bench --example dkg_performance)
  case "{{build}}" in
    release) cargo_args+=(--release) ;;
    debug) ;;
    *) printf 'build must be release or debug\n' >&2; exit 2 ;;
  esac
  benchmark_args=(
    --profile "{{profile}}"
    --denominations "{{denominations}}"
    --iterations "{{iterations}}"
    --phases "{{phases}}"
    --timeout-secs "{{timeout_secs}}"
  )
  case "{{full_ceremony}}" in
    1|true) benchmark_args+=(--full-ceremony) ;;
    0|false) ;;
    *) printf 'full_ceremony must be 0 or 1\n' >&2; exit 2 ;;
  esac
  cargo "${cargo_args[@]}" -- "${benchmark_args[@]}"

# Generate local fake-wallet federation configs backed by PostgreSQL member schemas
fed-regtest-postgres-generate dir="target/cdk-fed-regtest-postgres" members="5" threshold="3" port_start="22885" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint" dkg_setup_route_probes="0":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  dkg_probe_arg=""
  if [[ "{{dkg_setup_route_probes}}" == "1" ]]; then
    dkg_probe_arg="--dkg-setup-route-probes"
  fi
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,postgres,prometheus
  target/debug/cdk-mintd federation test-trusted-dealer \
    --output-dir "{{dir}}" \
    --force \
    --local-fakewallet-configs \
    --members "{{members}}" \
    --threshold "{{threshold}}" \
    --port-start "{{port_start}}" \
    --database-engine postgres \
    --postgres-base-url "{{postgres_base_url}}" \
    ${dkg_probe_arg:+$dkg_probe_arg}

# Check that the PostgreSQL service for Postgres-backed federation smoke is reachable
fed-regtest-postgres-preflight postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  redacted_url="$(printf '%s' "{{postgres_base_url}}" | sed -E 's#(postgres(ql)?://[^:/@]+):[^@]*@#\1:***@#')"
  if ! command -v pg_isready >/dev/null 2>&1; then
    echo "pg_isready is required for PostgreSQL federation smoke preflight" >&2
    echo "enter the regtest shell with: nix develop .#regtest" >&2
    exit 1
  fi
  if ! pg_isready -d "{{postgres_base_url}}" -t 2 >/dev/null 2>&1; then
    echo "PostgreSQL is not ready for federation smoke at ${redacted_url}" >&2
    echo "inside the regtest shell, start it with: just start-postgres" >&2
    exit 1
  fi
  echo "PostgreSQL is ready for federation smoke at ${redacted_url}"

# Run the generated PostgreSQL-backed fake-wallet federation smoke
fed-regtest-postgres dir="target/cdk-fed-regtest-postgres" members="5" threshold="3" port_start="22885" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  just fed-regtest-postgres-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{postgres_base_url}}"
  RESET_POSTGRES_SCHEMAS=1 CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run generated PostgreSQL-backed smoke with a longer offline operation window
fed-regtest-postgres-long-partition dir="target/cdk-fed-regtest-postgres-long-partition" members="5" threshold="3" port_start="26885" offline_batches="8" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  just fed-regtest-postgres-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{postgres_base_url}}"
  RESET_POSTGRES_SCHEMAS=1 LONG_PARTITION_OPERATION_BATCHES="{{offline_batches}}" CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run the generated setup-only DKG route-probe smoke with PostgreSQL member schemas
fed-regtest-postgres-dkg-setup dir="target/cdk-fed-regtest-postgres-dkg" members="5" threshold="3" port_start="24885" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  just fed-regtest-postgres-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{postgres_base_url}}" "1"
  RESET_POSTGRES_SCHEMAS=1 DKG_ROUTE_PROBE_ONLY=1 CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run PostgreSQL-backed DKG route probes and then continue through the full federation smoke
fed-regtest-postgres-dkg-full dir="target/cdk-fed-regtest-postgres-dkg-full" members="5" threshold="3" port_start="24985" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  just fed-regtest-postgres-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{postgres_base_url}}" "1"
  RESET_POSTGRES_SCHEMAS=1 PROBE_DKG_SETUP_ROUTES=1 CDK_MINTD_BIN=target/debug/cdk-mintd bash "{{dir}}/smoke-local-federation.sh"

# Run the dealer-free live DKG ceremony and wallet smoke with PostgreSQL member schemas
fed-regtest-postgres-live-dkg dir="target/cdk-fed-regtest-postgres-live-dkg" members="5" threshold="3" port_start="26985" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  just fed-regtest-postgres-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{postgres_base_url}}"
  RESET_POSTGRES_SCHEMAS=1 \
    CDK_MINTD_BIN=target/debug/cdk-mintd \
    bash "{{dir}}/smoke-live-dkg-federation.sh"

# Start generated PostgreSQL-backed fake-wallet federation members and leave them running
fed-regtest-postgres-start dir="target/cdk-fed-regtest-postgres" members="5" threshold="3" port_start="22885" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  if [ ! -f "{{dir}}/federation-public.json" ]; then
    just fed-regtest-postgres-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}" "{{postgres_base_url}}"
  else
    cargo build -p cdk-mintd --features federation-test-utils,sqlite,postgres,prometheus
  fi
  for member in $(seq 1 "{{members}}"); do
    work_dir="{{dir}}/member-${member}/work"
    config="{{dir}}/member-${member}/config.toml"
    pid_file="${work_dir}/mintd.pid"
    if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      echo "member ${member} already running with pid $(cat "$pid_file")"
      continue
    fi
    mkdir -p "$work_dir"
    nohup target/debug/cdk-mintd --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
    echo "$!" > "$pid_file"
    echo "started member ${member} with pid $(cat "$pid_file")"
  done

# Show signed status for generated PostgreSQL-backed fake-wallet federation members
fed-regtest-postgres-status dir="target/cdk-fed-regtest-postgres" signer="1":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,postgres,prometheus
  if [ ! -f "{{dir}}/federation-public.json" ]; then
    echo "missing federation public config: {{dir}}/federation-public.json" >&2
    echo "run: just fed-regtest-postgres-generate" >&2
    exit 1
  fi
  target/debug/cdk-mintd federation status \
    --public-config "{{dir}}/federation-public.json" \
    --private-config "{{dir}}/member-{{signer}}/federation-private.json" \
    --require-ready \
    --require-converged \
    --require-checkpoint-quorum \
    --require-no-digest-alerts \
    --require-no-invalid-operation-alerts

# Drop generated PostgreSQL member schemas for fake-wallet federation smoke
fed-regtest-postgres-reset-schemas members="5" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  just fed-regtest-postgres-preflight "{{postgres_base_url}}"
  redacted_url="$(printf '%s' "{{postgres_base_url}}" | sed -E 's#(postgres(ql)?://[^:/@]+):[^@]*@#\1:***@#')"
  for member in $(seq 1 "{{members}}"); do
    schema="cdk_federation_member_${member}"
    psql "{{postgres_base_url}}" -v ON_ERROR_STOP=1 -c "DROP SCHEMA IF EXISTS \"${schema}\" CASCADE;" >/dev/null
    echo "removed PostgreSQL schema ${schema} at ${redacted_url}"
  done

# Stop generated PostgreSQL-backed fake-wallet federation members
fed-regtest-postgres-stop dir="target/cdk-fed-regtest-postgres":
  #!/usr/bin/env bash
  set -euo pipefail
  just fed-regtest-stop "{{dir}}"

# Remove generated PostgreSQL-backed fake-wallet federation state
fed-regtest-postgres-clean dir="target/cdk-fed-regtest-postgres" members="5" postgres_base_url="postgresql://cdk:cdk@127.0.0.1:5432/cdk_mint":
  #!/usr/bin/env bash
  set -euo pipefail
  just fed-regtest-postgres-reset-schemas "{{members}}" "{{postgres_base_url}}"
  just fed-regtest-clean "{{dir}}"

# Start generated fake-wallet federation members and leave them running
fed-regtest-start dir="target/cdk-fed-regtest" members="5" threshold="3" port_start="21885":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  mintd_bin="${CDK_MINTD_BIN:-target/debug/cdk-mintd}"
  if [ ! -f "{{dir}}/federation-public.json" ]; then
    just fed-regtest-generate "{{dir}}" "{{members}}" "{{threshold}}" "{{port_start}}"
  elif [[ "$mintd_bin" == "target/debug/cdk-mintd" ]]; then
    cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  fi
  for member in $(seq 1 "{{members}}"); do
    work_dir="{{dir}}/member-${member}/work"
    config="{{dir}}/member-${member}/config.toml"
    pid_file="${work_dir}/mintd.pid"
    if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      echo "member ${member} already running with pid $(cat "$pid_file")"
      continue
    fi
    mkdir -p "$work_dir"
    nohup "$mintd_bin" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
    echo "$!" > "$pid_file"
    echo "started member ${member} with pid $(cat "$pid_file")"
  done

# Start one generated fake-wallet federation member
fed-regtest-start-member member dir="target/cdk-fed-regtest" build="1":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  mintd_bin="${CDK_MINTD_BIN:-target/debug/cdk-mintd}"
  work_dir="{{dir}}/member-{{member}}/work"
  config="{{dir}}/member-{{member}}/config.toml"
  pid_file="${work_dir}/mintd.pid"
  if [ ! -f "$config" ]; then
    echo "missing member config: $config" >&2
    echo "run: just fed-regtest-generate" >&2
    exit 1
  fi
  if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    echo "member {{member}} already running with pid $(cat "$pid_file")"
    exit 0
  fi
  mkdir -p "$work_dir"
  if [[ "{{build}}" == "1" && "$mintd_bin" == "target/debug/cdk-mintd" ]]; then
    cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  fi
  nohup "$mintd_bin" --work-dir "$work_dir" --config "$config" > "$work_dir/mintd.log" 2>&1 < /dev/null &
  echo "$!" > "$pid_file"
  echo "started member {{member}} with pid $(cat "$pid_file")"
  mint_url="$(awk -F' = ' '/^url = / { gsub("\"", "", $2); print $2; exit }' "$config")"
  for _ in $(seq 1 30); do
    if ! kill -0 "$(cat "$pid_file")" 2>/dev/null; then
      echo "member {{member}} exited before becoming reachable" >&2
      tail -n 80 "$work_dir/mintd.log" >&2 || true
      tail -n 80 "$work_dir"/logs/cdk-mintd.log.* >&2 || true
      exit 1
    fi
    if curl -fsS "$mint_url/v1/info" >/dev/null 2>&1; then
      echo "member {{member}} is reachable at $mint_url"
      exit 0
    fi
    sleep 1
  done
  echo "member {{member}} did not become reachable at $mint_url" >&2
  tail -n 80 "$work_dir/mintd.log" >&2 || true
  tail -n 80 "$work_dir"/logs/cdk-mintd.log.* >&2 || true
  exit 1

# Stop one generated fake-wallet federation member
fed-regtest-stop-member member dir="target/cdk-fed-regtest":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  pid_file="{{dir}}/member-{{member}}/work/mintd.pid"
  if [ ! -f "$pid_file" ]; then
    echo "member {{member}} has no pid file"
    exit 0
  fi
  pid="$(cat "$pid_file")"
  kill "$pid" 2>/dev/null || true
  rm -f "$pid_file"
  echo "stopped member {{member}}"

# Stop all generated fake-wallet federation members
fed-regtest-stop dir="target/cdk-fed-regtest":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  for pid_file in "{{dir}}"/member-*/work/mintd.pid; do
    [ -e "$pid_file" ] || continue
    pid="$(cat "$pid_file")"
    kill "$pid" 2>/dev/null || true
    rm -f "$pid_file"
    echo "stopped $(basename "$(dirname "$(dirname "$pid_file")")")"
  done

# Show signed status for generated fake-wallet federation members
fed-regtest-status dir="target/cdk-fed-regtest" signer="1":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo build -p cdk-mintd --features federation-test-utils,sqlite,prometheus
  if [ ! -f "{{dir}}/federation-public.json" ]; then
    echo "missing federation public config: {{dir}}/federation-public.json" >&2
    echo "run: just fed-regtest-generate" >&2
    exit 1
  fi
  target/debug/cdk-mintd federation status \
    --public-config "{{dir}}/federation-public.json" \
    --private-config "{{dir}}/member-{{signer}}/federation-private.json" \
    --require-ready \
    --require-converged \
    --require-checkpoint-quorum \
    --require-no-digest-alerts \
    --require-no-invalid-operation-alerts

# Remove generated local fake-wallet federation state
fed-regtest-clean dir="target/cdk-fed-regtest":
  #!/usr/bin/env bash
  set -euo pipefail
  case "{{dir}}" in
    ""|"/"|"/tmp"|"/private/tmp"|".")
      echo "refusing to remove unsafe federation regtest dir: {{dir}}" >&2
      exit 1
      ;;
  esac
  just fed-regtest-stop "{{dir}}" || true
  rm -rf -- "{{dir}}"
  echo "removed {{dir}}"

# Lightning Network Commands (require regtest environment to be running)

# Get CLN node 1 info
ln-cln1 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh ln-cln1 "$@"

# Get CLN node 2 info
ln-cln2 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh ln-cln2 "$@"

# Get LND node 1 info
ln-lnd1 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh ln-lnd1 "$@"

# Get LND node 2 info
ln-lnd2 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh ln-lnd2 "$@"

# Bitcoin regtest commands
btc *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh btc "$@"

# Mine blocks in regtest
btc-mine blocks="10":
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh btc-mine {{blocks}}

# Show mint information
mint-info:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh mint-info

# Run integration tests against regtest environment
mint-test:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh mint-test

# Restart mints after recompiling (useful for development)
restart-mints:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh restart-mints

# Show regtest environment status
regtest-status:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh show-status

# Show regtest environment logs
regtest-logs:
  #!/usr/bin/env bash
  set -euo pipefail
  bash ./misc/regtest_helper.sh show-logs

run-examples:
  cargo r --example p2pk
  cargo r --example mint-token
  cargo r --example melt-token
  cargo r --example proof_selection
  cargo r --example wallet

check-wasm *ARGS="--target wasm32-unknown-unknown":
  #!/usr/bin/env bash
  set -euo pipefail

  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi

  buildargs=(
    "-p cdk"
    "-p cdk --no-default-features"
    "-p cdk --no-default-features --features wallet"
    "-p cdk --no-default-features --features mint"
  )

  for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg'"
    cargo check $arg {{ARGS}}
    echo
  done

release m="":
  #!/usr/bin/env bash
  set -euo pipefail

  # Extract version from the cdk-ffi crate
  VERSION=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[] | select(.name == "cdk-ffi") | .version')

  if ! git ls-remote --exit-code --tags origin "refs/tags/v$VERSION" > /dev/null; then
    echo "Tag v$VERSION does not exist on origin. Push the release tag before publishing crates."
    exit 1
  fi

  args=(
    "-p cashu"
    "-p cdk-prometheus"
    "-p cdk-http-client"
    "-p cdk-common"
    "-p cdk-nostr"
    "-p cdk-sql-common"
    "-p cdk-sqlite"
    "-p cdk-postgres"
    "-p cdk-redb"
    "-p cdk-signatory"
    "-p cdk-fake-wallet"
    "-p cdk"
    "-p cdk-supabase"
    "-p cdk-ffi"
    "-p cdk-axum"
    "-p cdk-mint-rpc"
    "-p cdk-bdk"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-ldk-node"
    "-p cdk-payment-processor"
    "-p cdk-cli"
    "-p cdk-mintd"
  )

  for arg in "${args[@]}";
  do
    echo "Publishing '$arg'"
    cargo publish $arg {{m}}
    echo
  done

  # Trigger all FFI binding releases (Dart, Kotlin, Swift, Go)
  echo "📦 Triggering all FFI binding releases for version $VERSION..."
  just ffi-release-all $VERSION

check-docs:
  #!/usr/bin/env bash
  set -euo pipefail
  args=(
    "-p cashu"
    "-p cdk-common"
    "-p cdk-http-client"
    "-p cdk-nostr"
    "-p cdk-sql-common"
    "-p cdk-sqlite"
    "-p cdk-postgres"
    "-p cdk-redb"
    "-p cdk-signatory"
    "-p cdk-fake-wallet"
    "-p cdk"
    "-p cdk-ffi"
    "-p cdk-axum"
    "-p cdk-mint-rpc"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-ldk-node"
    "-p cdk-prometheus"
    "-p cdk-payment-processor"
    "-p cdk-cli"
    "-p cdk-mintd"
  )

  for arg in "${args[@]}"; do
    echo  "Checking '$arg' docs"
    cargo doc $arg --all-features
    echo
  done

# Build docs for all crates and error on warnings
docs-strict:
  #!/usr/bin/env bash
  set -euo pipefail
  args=(
    "-p cashu"
    "-p cdk-common"
    "-p cdk-http-client"
    "-p cdk-nostr"
    "-p cdk-sql-common"
    "-p cdk-sqlite"
    "-p cdk-postgres"
    "-p cdk-redb"
    "-p cdk-signatory"
    "-p cdk-fake-wallet"
    "-p cdk"
    "-p cdk-ffi"
    "-p cdk-axum"
    "-p cdk-mint-rpc"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-ldk-node"
    "-p cdk-prometheus"
    "-p cdk-payment-processor"
    "-p cdk-cli"
    "-p cdk-mintd"
  )

  for arg in "${args[@]}"; do
    echo "Building docs for $arg with strict warnings"
    RUSTDOCFLAGS="-D warnings" cargo doc $arg --all-features --no-deps
    echo
  done

# =============================================================================
# FFI Commands - CDK Foreign Function Interface bindings
# =============================================================================

# Helper function to get library extension based on platform
_ffi-lib-ext:
  #!/usr/bin/env bash
  if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "dylib"
  else
    echo "so"
  fi

# Build the FFI library
ffi-build *ARGS="--release":
  #!/usr/bin/env bash
  set -euo pipefail

  if [[ "{{ARGS}}" == *"--debug"* ]]; then
    cargo build --package cdk-ffi
  else
    cargo build {{ARGS}} --package cdk-ffi
  fi

# Check the FFI crate
ffi-check:
  cargo check --package cdk-ffi --all-targets

# Generate bindings for a specific language
ffi-generate LANGUAGE *ARGS="--release":
  #!/usr/bin/env bash
  set -euo pipefail
  LANG="{{LANGUAGE}}"

  # Validate language
  case "$LANG" in
    python|swift|kotlin)
      ;;
    *)
      echo "❌ Unsupported language: $LANG"
      echo "Supported languages: python, swift, kotlin"
      exit 1
      ;;
  esac

  # Set emoji and build type
  case "$LANG" in
    python) EMOJI="🐍" ;;
    swift) EMOJI="🍎" ;;
    kotlin) EMOJI="🎯" ;;
  esac

  # Determine build type and library path
  if [[ "{{ARGS}}" == *"--release"* ]] || [[ "{{ARGS}}" == "" ]]; then
    BUILD_TYPE="release"
  else
    BUILD_TYPE="debug"
  fi

  just ffi-build {{ARGS}}

  LIB_EXT=$(just _ffi-lib-ext)

  echo "$EMOJI Generating $LANG bindings..."
  mkdir -p target/bindings/$LANG

  cargo run -p cdk-ffi --bin uniffi-bindgen generate \
    --library target/$BUILD_TYPE/libcdk_ffi.$LIB_EXT \
    --language $LANG \
    --out-dir target/bindings/$LANG \
    --no-format

  echo "✅ $LANG bindings generated in target/bindings/$LANG/"

# Generate Python bindings (shorthand)
ffi-generate-python *ARGS="--release":
  just ffi-generate python {{ARGS}}

# Generate Swift bindings and set up SPM package for testing
ffi-generate-swift *ARGS="--release":
  #!/usr/bin/env bash
  set -euo pipefail
  just ffi-generate swift {{ARGS}}

  ROOT="{{justfile_directory()}}"

  # Determine build type
  if [[ "{{ARGS}}" == *"--release"* ]] || [[ "{{ARGS}}" == "" ]]; then
    BUILD_TYPE="release"
  else
    BUILD_TYPE="debug"
  fi

  # Set up SPM-compatible directory structure for testing.
  # Derive the FFI module name from the generated header so the on-disk
  # directory matches the target path written into Package.swift below
  # (the header name follows uniffi.toml's module_name, e.g. CashuDevKitFFI).
  BINDINGS_DIR="$ROOT/target/bindings/swift"
  SWIFT_SRC="$ROOT/Sources/Cdk"
  HEADER_NAME=$(basename "$BINDINGS_DIR/"*FFI.h)
  MODULE_NAME="${HEADER_NAME%.h}"
  FFI_MODULE="$ROOT/Sources/${MODULE_NAME}"
  mkdir -p "$SWIFT_SRC" "$FFI_MODULE"

  # Copy generated Swift sources
  cp "$BINDINGS_DIR/"*.swift "$SWIFT_SRC/"

  # Copy generated FFI header and create modulemap.
  # The modulemap generated by uniffi uses 'link' directives; replace with one
  # that lets SPM link the dylib via Package.swift linkerSettings instead.
  cp "$BINDINGS_DIR/"*FFI.h "$FFI_MODULE/"
  printf 'module %s {\n    header "%s"\n    export *\n}\n' \
    "${MODULE_NAME}" "${HEADER_NAME}" > "$FFI_MODULE/module.modulemap"

  LIB_DIR="$ROOT/target/$BUILD_TYPE"

  # Generate Package.swift for local testing
  {
    echo '// swift-tools-version: 5.9'
    echo '// Auto-generated for local testing — do not commit'
    echo 'import PackageDescription'
    echo ''
    echo 'let package = Package('
    echo '    name: "cdk-swift",'
    echo '    platforms: [.macOS(.v13)],'
    echo '    products: ['
    echo '        .library(name: "Cdk", targets: ["Cdk"]),'
    echo '    ],'
    echo '    targets: ['
    echo '        .systemLibrary('
    echo "            name: \"${MODULE_NAME}\","
    echo "            path: \"Sources/${MODULE_NAME}\""
    echo '        ),'
    echo '        .target('
    echo '            name: "Cdk",'
    echo "            dependencies: [\"${MODULE_NAME}\"],"
    echo '            path: "Sources/Cdk",'
    echo '            linkerSettings: ['
    echo "                .unsafeFlags([\"-L${LIB_DIR}\", \"-lcdk_ffi\"]),"
    echo '            ]'
    echo '        ),'
    echo '        .testTarget('
    echo '            name: "CdkTests",'
    echo '            dependencies: ["Cdk"],'
    echo '            path: "bindings/swift/Tests"'
    echo '        ),'
    echo '    ]'
    echo ')'
  } > "$ROOT/Package.swift"
  echo "📦 SPM package ready for testing"

# Generate Kotlin bindings (shorthand)
ffi-generate-kotlin *ARGS="--release":
  just ffi-generate kotlin {{ARGS}}

# Generate bindings for all supported languages
ffi-generate-all *ARGS="--release":
  @echo "🔧 Generating UniFFI bindings for all languages..."
  just ffi-generate python {{ARGS}}
  just ffi-generate swift {{ARGS}}
  just ffi-generate kotlin {{ARGS}}
  @echo "✅ All bindings generated successfully!"

# Run Python FFI tests
ffi-test: ffi-generate-python
  #!/usr/bin/env bash
  set -euo pipefail
  echo "🧪 Running Python FFI tests..."
  python3 crates/cdk-ffi/tests/test_transactions.py
  python3 crates/cdk-ffi/tests/test_kvstore.py
  python3 crates/cdk-ffi/tests/test_rate_limit.py
  echo "✅ Tests completed!"

# Build debug version and generate Python bindings quickly (for development)
ffi-dev-python:
  #!/usr/bin/env bash
  set -euo pipefail

  # Generate Python bindings first
  just ffi-generate python --debug

  # Copy library to Python bindings directory
  LIB_EXT=$(just _ffi-lib-ext)
  echo "📦 Copying library to Python bindings directory..."
  cp target/debug/libcdk_ffi.$LIB_EXT target/bindings/python/

  # Launch Python REPL with CDK FFI loaded
  cd target/bindings/python
  echo "🐍 Launching Python REPL with CDK FFI library loaded..."
  echo "💡 The 'cdk_ffi' module is pre-imported and ready to use!"
  python3 -i -c "from cdk_ffi import *; print('✅ CDK FFI library loaded successfully!');"

# Test language bindings with a simple import
ffi-test-bindings LANGUAGE: (ffi-generate LANGUAGE "--debug")
  #!/usr/bin/env bash
  set -euo pipefail
  LANG="{{LANGUAGE}}"
  LIB_EXT=$(just _ffi-lib-ext)

  echo "📦 Copying library to $LANG bindings directory..."
  cp target/debug/libcdk_ffi.$LIB_EXT target/bindings/$LANG/

  cd target/bindings/$LANG
  echo "🧪 Testing $LANG bindings..."

  case "$LANG" in
    python)
      python3 -c "import cdk_ffi; print('✅ Python bindings work!')"
      ;;
    *)
      echo "✅ $LANG bindings generated (manual testing required)"
      ;;
  esac

# Test Python bindings (shorthand)
ffi-test-python:
  just ffi-test-bindings python

# Run live Python FFI tests against testnut.cashudevkit.org
ffi-test-live-python:
  #!/usr/bin/env bash
  set -euo pipefail

  just ffi-generate python --debug

  LIB_EXT=$(just _ffi-lib-ext)
  echo "📦 Copying debug library to Python bindings directory..."
  cp target/debug/libcdk_ffi.$LIB_EXT target/bindings/python/

  echo "🧪 Running live Python FFI tests..."
  python3 crates/cdk-ffi/tests/test_live_async_onchain_melt.py

# Trigger all FFI binding releases (Dart, Kotlin, Swift, Go)
ffi-release-all VERSION:
  #!/usr/bin/env bash
  set -euo pipefail

  echo "🚀 Triggering all FFI binding releases..."
  echo "   Version: {{VERSION}}"
  echo "   Tag: v{{VERSION}}"

  gh workflow run "FFI - Publish All Bindings" \
    --repo cashubtc/cdk \
    --field release_tag="v{{VERSION}}" \
    --field cdk_version="{{VERSION}}" \
    --field cdk_ref="v{{VERSION}}"

  echo "✅ All FFI binding workflows triggered successfully!"

# Trigger Dart Package release workflow
ffi-release-dart VERSION:
  #!/usr/bin/env bash
  set -euo pipefail

  echo "🚀 Triggering Dart bindings workflow..."
  echo "   Version: {{VERSION}}"
  echo "   Tag: v{{VERSION}}"

  gh workflow run "FFI - Dart Bindings" \
    --repo cashubtc/cdk \
    --field release_tag="v{{VERSION}}" \
    --field cdk_version="{{VERSION}}"

  echo "✅ Dart workflow triggered successfully!"

# Trigger Swift Package release workflow
ffi-release-swift VERSION:
  #!/usr/bin/env bash
  set -euo pipefail

  echo "🚀 Triggering FFI - Swift Bindings workflow..."
  echo "   Version: {{VERSION}}"

  # Trigger the workflow using GitHub CLI
  gh workflow run "FFI - Swift Bindings" \
    --repo cashubtc/cdk \
    --field release_tag="v{{VERSION}}" \
    --field cdk_version="{{VERSION}}" \
    --field cdk_ref="v{{VERSION}}"

  echo "✅ Swift workflow triggered successfully!"

# Trigger Kotlin Package release workflow
ffi-release-kotlin VERSION:
  #!/usr/bin/env bash
  set -euo pipefail

  echo "🚀 Triggering Kotlin bindings workflow..."
  echo "   Version: {{VERSION}}"
  echo "   CDK Ref: v{{VERSION}}"

  # Trigger the workflow using GitHub CLI
  gh workflow run "FFI - Kotlin Bindings" \
    --repo cashubtc/cdk \
    --field release_tag="v{{VERSION}}" \
    --field cdk_version="{{VERSION}}" \
    --field cdk_ref="v{{VERSION}}"

  echo "✅ Kotlin workflow triggered successfully!"

# Generate Go FFI bindings
binding-go:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  LIB_EXT=$(just _ffi-lib-ext)
  result=$(nix build .#go-bindings --print-out-paths --no-link)
  rm -rf target/bindings/go
  mkdir -p target/bindings/go target/release
  cp -r "$result"/bindings/* target/bindings/go/
  cp "$result/lib/libcdk_ffi_go.${LIB_EXT}" target/release/
  chmod -R u+w target/bindings/go target/release/libcdk_ffi_go.${LIB_EXT}

# Run Go binding tests
test-go:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  export CGO_ENABLED=1
  export GO111MODULE=off
  export CGO_LDFLAGS="-L{{justfile_directory()}}/target/release -lcdk_ffi_go"
  export LD_LIBRARY_PATH="{{justfile_directory()}}/target/release"
  BINDINGS_DIR="target/bindings/go/cdkffi"

  echo "🧪 Testing Go bindings compile..."
  cd "$BINDINGS_DIR"
  go build .
  echo "✅ Go bindings compile successfully!"

  echo "🧪 Running Go binding tests..."
  cp "{{justfile_directory()}}/bindings/go/cdk_test.go" .
  go test -v -count=1 -timeout 120s .
  echo "✅ Go binding tests passed!"

# Trigger Go Bindings release workflow
ffi-release-go VERSION:
  #!/usr/bin/env bash
  set -euo pipefail

  echo "🚀 Triggering Go bindings workflow..."
  echo "   Version: {{VERSION}}"
  echo "   Tag: v{{VERSION}}"

  gh workflow run "FFI - Go Bindings" \
    --repo cashubtc/cdk \
    --field release_tag="v{{VERSION}}" \
    --field cdk_version="{{VERSION}}" \
    --field cdk_ref="v{{VERSION}}"

  echo "✅ Go workflow triggered successfully!"

# Generate Dart FFI bindings via nix
binding-dart:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  result=$(nix build .#dart-bindings --print-out-paths --no-link)
  cp -r "$result"/* bindings/dart/
  chmod -R u+w bindings/dart/lib/src/generated/

# Run Dart binding tests
test-dart:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  cp rust-toolchain.toml bindings/dart/rust/rust-toolchain.toml
  trap 'rm -f bindings/dart/rust/rust-toolchain.toml' EXIT
  cd bindings/dart
  dart test

# Generate Kotlin JVM bindings via nix
binding-kotlin:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"
  result=$(nix build .#kotlin-bindings --print-out-paths --no-link)
  cp -r "$result"/* bindings/kotlin/
  chmod -R u+w bindings/kotlin

# Run Kotlin binding tests
test-kotlin:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}/bindings/kotlin"
  ./gradlew -PcdkJvmOnly=true :cdk-jvm:test


# Run Swift binding tests
test-swift:
  #!/usr/bin/env bash
  set -euo pipefail
  cd "{{justfile_directory()}}"

  # Determine which build dir contains the dylib
  if [[ -f target/release/libcdk_ffi.dylib ]] || [[ -f target/release/libcdk_ffi.so ]]; then
    LIB_DIR="$PWD/target/release"
  else
    LIB_DIR="$PWD/target/debug"
  fi

  if [[ "$(uname)" == "Darwin" ]]; then
    # Use env -i to fully escape the Nix environment. The Nix shell injects
    # SDK paths and compiler flags that conflict with the Xcode Swift compiler.
    env -i \
      HOME="$HOME" \
      PATH="/usr/bin:/bin:/usr/sbin:/sbin" \
      SDKROOT="$(/usr/bin/xcrun --sdk macosx --show-sdk-path)" \
      DEVELOPER_DIR="$(/usr/bin/xcode-select -p)" \
      DYLD_LIBRARY_PATH="$LIB_DIR" \
      /usr/bin/swift test
  else
    DYLD_LIBRARY_PATH="$LIB_DIR" swift test
  fi
