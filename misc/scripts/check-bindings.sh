#!/usr/bin/env bash

set -euo pipefail

# Check bindings
buildargs=(
    "-p cdk-js --target wasm32-unknown-unknown"
)

for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg'"

    cargo build $arg

    if [[ $arg != *"--target wasm32-unknown-unknown"* ]];
    then
        cargo test $arg
    fi

    cargo clippy $arg -- -D warnings

    echo
done
