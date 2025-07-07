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

run-examples:
  cargo r --example p2pk
  cargo r --example mint-token
  cargo r --example melt-token
  cargo r --example proof_selection
  cargo r --example wallet

clippy-each:
  #!/usr/bin/env bash
  set -euo pipefail
  buildargs=(
  "-p cashu --no-default-features"
  "-p cashu --no-default-features --features wallet"
  "-p cashu --no-default-features --features mint"
  "-p cdk-common --no-default-features"
  "-p cdk-common --no-default-features --features wallet"
  "-p cdk-common --no-default-features --features mint"
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
  "-p cdk-lnd"
  "-p cdk-mint-rpc"
  "--bin cdk-cli"
  "--bin cdk-mintd"
  )

  for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg'"
    cargo check $arg
    cargo clippy $arg -- -D warnings
    echo
  done

release m="":
  #!/usr/bin/env bash
  set -euo pipefail

  args=(
    "-p cashu"
    "-p cdk-common"
    "-p cdk"
    "-p cdk-redb"
    "-p cdk-sqlite"
    "-p cdk-axum"
    "-p cdk-mint-rpc"
    "-p cdk-cln"
    "-p cdk-lnd"
    "-p cdk-lnbits"
    "-p cdk-fake-wallet"
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
    "-p cdk-cli"
    "-p cdk-mintd"
  )

  for arg in "${args[@]}"; do
    echo  "Checking '$arg' docs"
    cargo doc $arg --all-features
    echo
  done
