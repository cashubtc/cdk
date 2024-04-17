#!/bin/bash

set -euo pipefail

buildargs=(
    "-p cdk"
)

for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg' docs"
    cargo doc $arg --all-features
    echo
done
