alias b := build
alias c := check
alias t := test

default:
  @just --list

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
  CDK_TEST_DB_TYPE={{db}} cargo test -p cdk-integration-tests --test integration_tests_pure

test-all db="memory":
    #!/usr/bin/env bash
    just test {{db}}
    ./misc/itests.sh "{{db}}"
    ./misc/fake_itests.sh "{{db}}"
    

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
  ./misc/fake_itests.sh "{{db}}"

  
itest-payment-processor ln:
  #!/usr/bin/env bash
  ./misc/mintd_payment_processor.sh "{{ln}}"

  
fake-auth-mint-itest db openid_discovery:
  #!/usr/bin/env bash
  ./misc/fake_auth_itests.sh "{{db}}" "{{openid_discovery}}"

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
