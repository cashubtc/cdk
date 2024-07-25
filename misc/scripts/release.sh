#!/usr/bin/env bash

set -euo pipefail

args=(
    "-p cdk"
    "-p cdk-redb"
    "-p cdk-sqlite"
    "-p cdk-rexie"
    "-p cdk-cln"
    "-p cdk-fake-wallet"
    "-p cdk-strike"
    "-p cdk-cli"
    "-p cdk-axum"
    "-p cdk-mintd"
)

for arg in "${args[@]}";
do
    echo "Publishing '$arg'"
    cargo publish "$arg"
    echo
done
