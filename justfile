alias b := build
alias c := check
alias t := test

default:
  @just --list

# Create a new SQL migration file
new-migration target name:
    #!/usr/bin/env bash
    if [ "{{target}}" != "mint" ] && [ "{{target}}" != "wallet" ]; then
        echo "Error: target must be either 'mint' or 'wallet'"
        exit 1
    fi
    
    timestamp=$(date +%Y%m%d%H%M%S)
    migration_path="./crates/cdk-sqlite/src/{{target}}/migrations/${timestamp}_{{name}}.sql"
    
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
test: build
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
    just test {{db}}
    ./misc/itests.sh "{{db}}"
    ./misc/fake_itests.sh "{{db}}" external_signatory
    ./misc/fake_itests.sh "{{db}}"
    
test-nutshell:
  #!/usr/bin/env bash
  docker run -d -p 3338:3338 --name nutshell -e MINT_LIGHTNING_BACKEND=FakeWallet -e MINT_LISTEN_HOST=0.0.0.0 -e MINT_LISTEN_PORT=3338 -e MINT_PRIVATE_KEY=TEST_PRIVATE_KEY -e MINT_INPUT_FEE_PPK=100  cashubtc/nutshell:latest poetry run mint
  
  # Wait for the Nutshell service to be ready
  echo "Waiting for Nutshell to start..."
  max_attempts=30
  attempt=0
  while ! curl -s http://127.0.0.1:3338/v1/info > /dev/null; do
    attempt=$((attempt+1))
    if [ $attempt -ge $max_attempts ]; then
      echo "Nutshell failed to start after $max_attempts attempts"
      docker stop nutshell
      docker rm nutshell
      exit 1
    fi
    echo "Waiting for Nutshell to start (attempt $attempt/$max_attempts)..."
    sleep 1
  done
  echo "Nutshell is ready!"
  
  export CDK_TEST_MINT_URL=http://127.0.0.1:3338
  export LN_BACKEND=FAKEWALLET
  cargo test -p cdk-integration-tests --test happy_path_mint_wallet
  cargo test -p cdk-integration-tests --test test_fees
  unset CDK_TEST_MINT_URL
  unset LN_BACKEND
  docker stop nutshell
  docker rm nutshell
    

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
  goose run --recipe ./misc/recipes/git-commit-message.yaml --interactive

# Create git message from staged changes using Goose AI
goose-changelog-staged:
  #!/usr/bin/env bash
  goose run --recipe ./misc/recipes/changelog-update.yaml --interactive

# Update changelog from recent commits using Goose AI
# Usage: just goose-changelog-commits [number_of_commits]
goose-changelog-commits *COMMITS="5":
  #!/usr/bin/env bash
  COMMITS={{COMMITS}} goose run --recipe ./misc/recipes/changelog-from-commits.yaml --interactive

itest db:
  #!/usr/bin/env bash
  ./misc/itests.sh "{{db}}"

  
fake-mint-itest db:
  #!/usr/bin/env bash
  ./misc/fake_itests.sh "{{db}}" external_signatory
  ./misc/fake_itests.sh "{{db}}"

  
itest-payment-processor ln:
  #!/usr/bin/env bash
  ./misc/mintd_payment_processor.sh "{{ln}}"

  
fake-auth-mint-itest db openid_discovery:
  #!/usr/bin/env bash
  ./misc/fake_auth_itests.sh "{{db}}" "{{openid_discovery}}"

nutshell-wallet-itest:
  #!/usr/bin/env bash
  ./misc/nutshell_wallet_itest.sh

# Start interactive regtest environment (Bitcoin + 4 LN nodes + 2 CDK mints)
regtest db="sqlite":
  #!/usr/bin/env bash
  ./misc/interactive_regtest_mprocs.sh {{db}}

# Lightning Network Commands (require regtest environment to be running)

# Get CLN node 1 info
ln-cln1 *ARGS:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh ln-cln1 {{ARGS}}

# Get CLN node 2 info  
ln-cln2 *ARGS:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh ln-cln2 {{ARGS}}

# Get LND node 1 info
ln-lnd1 *ARGS:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh ln-lnd1 {{ARGS}}

# Get LND node 2 info
ln-lnd2 *ARGS:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh ln-lnd2 {{ARGS}}

# Bitcoin regtest commands
btc *ARGS:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh btc {{ARGS}}

# Mine blocks in regtest
btc-mine blocks="10":
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh btc-mine {{blocks}}

# Show mint information
mint-info:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh mint-info

# Run integration tests against regtest environment
mint-test:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh mint-test

# Restart mints after recompiling (useful for development)
restart-mints:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh restart-mints

# Show regtest environment status
regtest-status:
  #!/usr/bin/env bash
  ./misc/regtest_helper.sh show-status

# Show regtest environment logs
regtest-logs:
  #!/usr/bin/env bash
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
    "-p cdk-common"
    "-p cdk-sqlite"
    "-p cdk-redb"
    "-p cdk-signatory"
    "-p cdk"
    "-p cdk-axum"
    "-p cdk-mint-rpc"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-lnbits"
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

check-docs:
  #!/usr/bin/env bash
  set -euo pipefail
  args=(
    "-p cashu"
    "-p cdk-common"
    "-p cdk"
    "-p cdk-redb"
    "-p cdk-sqlite"
    "-p cdk-axum"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-lnbits"
    "-p cdk-fake-wallet"
    "-p cdk-mint-rpc"
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

# Build the FFI library
ffi-build *ARGS="--release":
  cargo build {{ARGS}} --package cdk-ffi

# Check the FFI library compiles
ffi-check *ARGS="":
  cargo check {{ARGS}} --package cdk-ffi

# Run tests for the FFI library  
ffi-test *ARGS="":
  cargo test {{ARGS}} --package cdk-ffi

# Run clippy on the FFI library
ffi-clippy *ARGS="--all-targets":
  cargo clippy {{ARGS}} --package cdk-ffi

# Format the FFI code
ffi-format:
  cargo fmt --package cdk-ffi

# Clean FFI build artifacts
ffi-clean:
  cargo clean --package cdk-ffi
  rm -rf target/bindings

# Generate bindings for all supported languages
ffi-generate-all: ffi-build
  @echo "Generating UniFFI bindings for all languages..."
  just ffi-generate-python
  just ffi-generate-swift
  just ffi-generate-kotlin
  @echo "✅ All bindings generated successfully!"

# Generate Python bindings
ffi-generate-python: ffi-build
  #!/usr/bin/env bash
  set -euo pipefail
  echo "🐍 Generating Python bindings..."
  mkdir -p target/bindings/python
  
  # Determine the correct library extension
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
  else
    LIB_EXT="so"
  fi
  
  cargo run --bin uniffi-bindgen generate \
    --library target/release/libcdk_ffi.$LIB_EXT \
    --language python \
    --out-dir target/bindings/python
  
  echo "✅ Python bindings generated in target/bindings/python/"

# Generate Swift bindings
ffi-generate-swift: ffi-build
  #!/usr/bin/env bash
  set -euo pipefail
  echo "🍎 Generating Swift bindings..."
  mkdir -p target/bindings/swift
  
  # Determine the correct library extension
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
  else
    LIB_EXT="so"
  fi
  
  cargo run --bin uniffi-bindgen generate \
    --library target/release/libcdk_ffi.$LIB_EXT \
    --language swift \
    --out-dir target/bindings/swift
  
  echo "✅ Swift bindings generated in target/bindings/swift/"

# Generate Kotlin bindings
ffi-generate-kotlin: ffi-build
  #!/usr/bin/env bash
  set -euo pipefail
  echo "🎯 Generating Kotlin bindings..."
  mkdir -p target/bindings/kotlin
  
  # Determine the correct library extension
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
  else
    LIB_EXT="so"
  fi
  
  cargo run --bin uniffi-bindgen generate \
    --library target/release/libcdk_ffi.$LIB_EXT \
    --language kotlin \
    --out-dir target/bindings/kotlin
  
  echo "✅ Kotlin bindings generated in target/bindings/kotlin/"

# Generate Ruby bindings
ffi-generate-ruby: ffi-build
  #!/usr/bin/env bash
  set -euo pipefail
  echo "💎 Generating Ruby bindings..."
  mkdir -p target/bindings/ruby
  
  # Determine the correct library extension
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
  else
    LIB_EXT="so"
  fi
  
  cargo run --bin uniffi-bindgen generate \
    --library target/release/libcdk_ffi.$LIB_EXT \
    --language ruby \
    --out-dir target/bindings/ruby
  
  echo "✅ Ruby bindings generated in target/bindings/ruby/"

# Generate bindings for a specific language
ffi-generate LANGUAGE: ffi-build
  #!/usr/bin/env bash
  set -euo pipefail
  LANG="{{LANGUAGE}}"
  echo "Generating $LANG bindings..."
  mkdir -p target/bindings/$LANG
  
  # Determine the correct library extension
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
  else
    LIB_EXT="so"
  fi
  
  cargo run --bin uniffi-bindgen generate \
    --library target/release/libcdk_ffi.$LIB_EXT \
    --language $LANG \
    --out-dir target/bindings/$LANG
  
  echo "✅ $LANG bindings generated in target/bindings/$LANG/"

# Run the FFI example bindings generator
ffi-run-example: ffi-build
  cargo run --example generate_bindings

# Build debug version and generate Python bindings quickly (for development)
ffi-dev-python:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "🐍 Quick Python bindings generation for development..."
  cargo build --package cdk-ffi
  mkdir -p target/bindings/python
  
  # Determine the correct library extension
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
  else
    LIB_EXT="so"
  fi
  
  cargo run --bin uniffi-bindgen generate \
    --library target/debug/libcdk_ffi.$LIB_EXT \
    --language python \
    --out-dir target/bindings/python
  
  echo "✅ Development Python bindings generated!"

# Install Python dependencies for testing FFI bindings
ffi-install-python-deps:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "📦 Installing Python dependencies for testing..."
  if command -v pip &> /dev/null; then
    pip install cffi
  elif command -v pip3 &> /dev/null; then
    pip3 install cffi
  else
    echo "❌ No pip found. Please install Python and pip first."
    exit 1
  fi
  echo "✅ Python dependencies installed!"

# Test Python bindings with a simple script
ffi-test-python: ffi-dev-python
  #!/usr/bin/env bash
  set -euo pipefail
  cd target/bindings/python
  echo "🧪 Testing Python bindings..."
  python3 -c "import cdk_ffi; print('✅ Python bindings loaded successfully!'); seed = cdk_ffi.generate_seed(); print(f'✅ Generated seed with length: {len(seed)}')"
  echo "✅ Python bindings test completed!"

# Show information about the built FFI library
ffi-info:
  #!/usr/bin/env bash
  set -euo pipefail
  echo "📊 CDK FFI Library Information"
  echo "=============================="
  
  if [[ "$OSTYPE" == "darwin"* ]]; then
    LIB_EXT="dylib"
    LIB_PATH="target/release/libcdk_ffi.$LIB_EXT"
    if [ -f "$LIB_PATH" ]; then
      echo "Library: $LIB_PATH"
      echo "Size: $(du -h $LIB_PATH | cut -f1)"
      echo "Architecture: $(file $LIB_PATH)"
    else
      echo "❌ Library not found. Run 'just ffi-build' first."
    fi
  else
    LIB_EXT="so"
    LIB_PATH="target/release/libcdk_ffi.$LIB_EXT"
    if [ -f "$LIB_PATH" ]; then
      echo "Library: $LIB_PATH"
      echo "Size: $(du -h $LIB_PATH | cut -f1)"
      echo "Architecture: $(file $LIB_PATH)"
      echo "Dependencies: $(ldd $LIB_PATH | wc -l) shared libraries"
    else
      echo "❌ Library not found. Run 'just ffi-build' first."
    fi
  fi
  
  echo ""
  echo "Available bindings:"
  if [ -d "target/bindings" ]; then
    find target/bindings -name "*.py" -o -name "*.swift" -o -name "*.kt" -o -name "*.rb" | sort
  else
    echo "  None generated yet. Run 'just ffi-generate-all' to create bindings."
  fi

# Watch for changes and regenerate Python bindings automatically
ffi-watch-python:
  #!/usr/bin/env bash
  echo "👀 Watching for changes and regenerating Python bindings..."
  echo "Press Ctrl+C to stop."
  
  if ! command -v fswatch &> /dev/null; then
    echo "❌ fswatch not found. Please install it first:"
    echo "  macOS: brew install fswatch"  
    echo "  Linux: apt-get install fswatch or yum install fswatch"
    exit 1
  fi
  
  fswatch -o crates/cdk-ffi/src/ | while read event; do
    echo "🔄 Changes detected, regenerating Python bindings..."
    just ffi-dev-python
  done

# Full FFI development cycle: format, check, test, generate bindings
ffi-dev-cycle: ffi-format ffi-check ffi-test ffi-generate-python
  @echo "✅ FFI development cycle complete!"

# FFI CI check: build, test, and verify bindings can be generated
ffi-ci-check: ffi-format ffi-check ffi-test ffi-build
  @echo "🔍 Running FFI CI checks..."
  just ffi-generate-python > /dev/null
  @echo "✅ FFI CI checks passed!"
