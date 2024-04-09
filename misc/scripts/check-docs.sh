#!/bin/bash

set -euo pipefail

buildargs=(
    "-p cashu"
    "-p cashu-sdk"
)

for arg in "${buildargs[@]}"; do
    echo  "Checking '$arg' docs"
    cargo doc $arg --all-features
    echo
done
