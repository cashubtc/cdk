#!/usr/bin/env bash

# Run before stopping regtest nodes. Only copy diagnostics, never node databases,
# wallets, credentials, or the generated environment file.
set -uo pipefail

work_dir="$1"
output_dir="$2"
mkdir -p "$output_dir" || exit 1

for node in one two; do
    node_dir="$output_dir/cln-$node"
    mkdir -p "$node_dir" || continue
    for method in getinfo listinvoices listsendpays listoffers listpeerchannels; do
        timeout --kill-after=1s 5s lightning-cli \
            --rpc-file="$work_dir/cln/$node/regtest/lightning-rpc" \
            "$method" > "$node_dir/$method.json" 2> "$node_dir/$method.stderr"
        status=$?
        if [ "$status" -ne 0 ]; then
            echo "$method exited with status $status" >> "$node_dir/$method.stderr"
        fi
    done
    if [ -f "$work_dir/cln/$node/debug.log" ]; then
        cp "$work_dir/cln/$node/debug.log" "$node_dir/debug.log"
    fi
done
