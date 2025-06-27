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
    "-p cdk-rexie"
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
    "-p cdk-rexie"
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
    "-p cdk-rexie"
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
