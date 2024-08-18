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
    "-p cdk-js"
  )

  for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg'"
    cargo check $arg {{ARGS}}
    echo
  done

run-examples:
  cargo r --example p2pk
  cargo r --example mint-token
  cargo r --example proof_selection
  cargo r --example wallet

clippy-each:
  #!/usr/bin/env bash
  set -euo pipefail
  buildargs=(
  "-p cdk-integration-tests"
  "-p cdk"
  "-p cdk --no-default-features"
  "-p cdk --no-default-features --features wallet"
  "-p cdk --no-default-features --features mint"
  "-p cdk-redb"
  "-p cdk-redb --no-default-features --features wallet"
  "-p cdk-redb --no-default-features --features mint"
  "-p cdk-sqlite --no-default-features --features mint"
  "-p cdk-sqlite --no-default-features --features wallet"
  "-p cdk-cln"
  "-p cdk-axum"
  "-p cdk-fake-wallet"
  "-p cdk-strike"
  "--bin cdk-cli"
  "--bin cdk-mintd"
  )

  for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg'"
    cargo check $arg
    cargo clippy $arg -- -D warnings
    echo
  done
