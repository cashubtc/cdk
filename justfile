alias b := build
alias c := check
alias t := test

default:
  @just --list

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

final-check: typos format clippy test

# run `cargo build` on everything
build *ARGS="--workspace --all-targets":
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi
  cargo build {{ARGS}}

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
  cargo test --lib

  # Run pure integration tests
  cargo test -p cdk-integration-tests --test mint 

  
# run doc tests
test-pure db="memory": build
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -f Cargo.toml ]; then
    cd {{invocation_directory()}}
  fi

  # Run pure integration tests
  CDK_TEST_DB_TYPE={{db}} cargo test -p cdk-integration-tests --test integration_tests_pure -- --test-threads 1

test-all db="memory":
    #!/usr/bin/env bash
    set -euo pipefail
    just test {{db}}
    ./misc/itests.sh "{{db}}"
    ./misc/fake_itests.sh "{{db}}" external_signatory
    ./misc/fake_itests.sh "{{db}}"
    
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
  
  docker run -d -p 3338:3338 --name nutshell -e MINT_LIGHTNING_BACKEND=FakeWallet -e MINT_LISTEN_HOST=0.0.0.0 -e MINT_LISTEN_PORT=3338 -e MINT_PRIVATE_KEY=TEST_PRIVATE_KEY -e MINT_INPUT_FEE_PPK=100  cashubtc/nutshell:latest poetry run mint
  
  export CDK_ITESTS_DIR=$(mktemp -d)

  # Wait for the Nutshell service to be ready
  echo "Waiting for Nutshell to start..."
  max_attempts=30
  attempt=0
  while ! curl -s http://127.0.0.1:3338/v1/info > /dev/null; do
    attempt=$((attempt+1))
    if [ $attempt -ge $max_attempts ]; then
      echo "Nutshell failed to start after $max_attempts attempts"
      exit 1
    fi
    echo "Waiting for Nutshell to start (attempt $attempt/$max_attempts)..."
    sleep 1
  done
  echo "Nutshell is ready!"
  
  # Set environment variables and run tests
  export CDK_TEST_MINT_URL=http://127.0.0.1:3338
  export LN_BACKEND=FAKEWALLET
  
  # Track test results
  test_exit_code=0
  
  # Run first test and capture exit code
  echo "Running happy_path_mint_wallet test..."
  if ! cargo test -p cdk-integration-tests --test happy_path_mint_wallet; then
    echo "ERROR: happy_path_mint_wallet test failed"
    test_exit_code=1
  fi
  
  # Run second test and capture exit code
  echo "Running test_fees test..."
  if ! cargo test -p cdk-integration-tests --test test_fees; then
    echo "ERROR: test_fees test failed"
    test_exit_code=1
  fi
  
  unset CDK_TEST_MINT_URL
  unset LN_BACKEND
  
  # Exit with error code if any test failed
  if [ $test_exit_code -ne 0 ]; then
    echo "One or more tests failed"
    exit $test_exit_code
  fi
  
  echo "All tests passed successfully"
    

# run `cargo clippy` on everything
clippy *ARGS="--locked --offline --workspace --all-targets":
  cargo clippy {{ARGS}}

# run `cargo clippy --fix` on everything
clippy-fix *ARGS="--locked --offline --workspace --all-targets":
  cargo clippy {{ARGS}} --fix

typos: 
  typos

# fix all typos
[no-exit-message]
typos-fix:
  just typos -w

# Goose AI Recipe Commands

# Update changelog from staged changes using Goose AI  
goose-git-msg:
  #!/usr/bin/env bash
  set -euo pipefail
  goose run --recipe ./misc/recipes/git-commit-message.yaml --interactive

# Create git message from staged changes using Goose AI
goose-changelog-staged:
  #!/usr/bin/env bash
  set -euo pipefail
  goose run --recipe ./misc/recipes/changelog-update.yaml --interactive

# Update changelog from recent commits using Goose AI
# Usage: just goose-changelog-commits [number_of_commits]
goose-changelog-commits *COMMITS="5":
  #!/usr/bin/env bash
  set -euo pipefail
  COMMITS={{COMMITS}} goose run --recipe ./misc/recipes/changelog-from-commits.yaml --interactive

itest db:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/itests.sh "{{db}}"

fake-mint-itest db:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/fake_itests.sh "{{db}}" external_signatory
  ./misc/fake_itests.sh "{{db}}"

itest-payment-processor ln:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/mintd_payment_processor.sh "{{ln}}"

fake-auth-mint-itest db openid_discovery:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/fake_auth_itests.sh "{{db}}" "{{openid_discovery}}"

nutshell-wallet-itest:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/nutshell_wallet_itest.sh

# Start interactive regtest environment (Bitcoin + 4 LN nodes + 2 CDK mints)
regtest db="sqlite":
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/interactive_regtest_mprocs.sh {{db}}

# Lightning Network Commands (require regtest environment to be running)

# Get CLN node 1 info
ln-cln1 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh ln-cln1 {{ARGS}}

# Get CLN node 2 info  
ln-cln2 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh ln-cln2 {{ARGS}}

# Get LND node 1 info
ln-lnd1 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh ln-lnd1 {{ARGS}}

# Get LND node 2 info
ln-lnd2 *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh ln-lnd2 {{ARGS}}

# Bitcoin regtest commands
btc *ARGS:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh btc {{ARGS}}

# Mine blocks in regtest
btc-mine blocks="10":
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh btc-mine {{blocks}}

# Show mint information
mint-info:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh mint-info

# Run integration tests against regtest environment
mint-test:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh mint-test

# Restart mints after recompiling (useful for development)
restart-mints:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh restart-mints

# Show regtest environment status
regtest-status:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh show-status

# Show regtest environment logs
regtest-logs:
  #!/usr/bin/env bash
  set -euo pipefail
  ./misc/regtest_helper.sh show-logs

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

  args=(
    "-p cashu"
    "-p cdk-prometheus"
    "-p cdk-common"
    "-p cdk-sql-common"
    "-p cdk-sqlite"
    "-p cdk-postgres"
    "-p cdk-redb"
    "-p cdk-signatory"
    "-p cdk"
    "-p cdk-ffi"
    "-p cdk-axum"
    "-p cdk-mint-rpc"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-lnbits"
    "-p cdk-ldk-node"
    "-p cdk-fake-wallet"
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

  # Extract version from the cdk-ffi crate
  VERSION=$(cargo metadata --format-version 1 --no-deps | jq -r '.packages[] | select(.name == "cdk-ffi") | .version')
  
  # Trigger Swift package release after Rust crates are published
  echo "📦 Triggering Swift package release for version $VERSION..."
  just ffi-release-swift $VERSION

check-docs:
  #!/usr/bin/env bash
  set -euo pipefail
  args=(
    "-p cashu"
    "-p cdk-common"
    "-p cdk-sql-common"
    "-p cdk"
    "-p cdk-redb"
    "-p cdk-sqlite"
    "-p cdk-axum"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-lnbits"
    "-p cdk-fake-wallet"
    "-p cdk-mint-rpc"
    "-p cdk-payment-processor"
    "-p cdk-signatory"
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
    "-p cdk-sql-common"
    "-p cdk"
    "-p cdk-redb"
    "-p cdk-sqlite"
    "-p cdk-axum"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-lnbits"
    "-p cdk-fake-wallet"
    "-p cdk-mint-rpc"
    "-p cdk-payment-processor"
    "-p cdk-signatory"
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
  cargo build {{ARGS}} --package cdk-ffi --features postgres

# Generate bindings for a specific language
ffi-generate LANGUAGE *ARGS="--release": ffi-build
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
    cargo build --package cdk-ffi --features postgres
  fi
  
  LIB_EXT=$(just _ffi-lib-ext)
  
  echo "$EMOJI Generating $LANG bindings..."
  mkdir -p target/bindings/$LANG
  
  cargo run --bin uniffi-bindgen generate \
    --library target/$BUILD_TYPE/libcdk_ffi.$LIB_EXT \
    --language $LANG \
    --out-dir target/bindings/$LANG
  
  echo "✅ $LANG bindings generated in target/bindings/$LANG/"

# Generate Python bindings (shorthand)
ffi-generate-python *ARGS="--release": 
  just ffi-generate python {{ARGS}}

# Generate Swift bindings (shorthand)
ffi-generate-swift *ARGS="--release":
  just ffi-generate swift {{ARGS}}

# Generate Kotlin bindings (shorthand)
ffi-generate-kotlin *ARGS="--release":
  just ffi-generate kotlin {{ARGS}}

# Generate bindings for all supported languages
ffi-generate-all *ARGS="--release": ffi-build
  @echo "🔧 Generating UniFFI bindings for all languages..."
  just ffi-generate python {{ARGS}}
  just ffi-generate swift {{ARGS}}
  just ffi-generate kotlin {{ARGS}}
  @echo "✅ All bindings generated successfully!"

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

# Trigger Swift Package release workflow
ffi-release-swift VERSION:
  #!/usr/bin/env bash
  set -euo pipefail
  
  echo "🚀 Triggering Publish Swift Package workflow..."
  echo "   Version: {{VERSION}}"
  echo "   CDK Ref: v{{VERSION}}"
  
  # Trigger the workflow using GitHub CLI
  gh workflow run "Publish Swift Package" \
    --repo cashubtc/cdk-swift \
    --field version="{{VERSION}}" \
    --field cdk_repo="cashubtc/cdk" \
    --field cdk_ref="v{{VERSION}}"
  
  echo "✅ Workflow triggered successfully!"
