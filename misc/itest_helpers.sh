#!/usr/bin/env bash
# Shared helper functions for integration test scripts.
# Source this file from each test script:
#   source "$(dirname "$0")/itest_helpers.sh"

# ========================================
# Helper: run a binary from $PATH (Nix pre-built) or fall back to cargo run
# ========================================
run_bin() {
    local bin_name="$1"
    shift
    if command -v "$bin_name" &>/dev/null; then
        echo "Using pre-built binary: $bin_name"
        "$bin_name" "$@"
    else
        echo "Pre-built binary not found, falling back to: cargo run --bin $bin_name"
        cargo run --bin "$bin_name" -- "$@"
    fi
}

run_bin_bg() {
    local bin_name="$1"
    shift
    if command -v "$bin_name" &>/dev/null; then
        echo "Using pre-built binary: $bin_name"
        "$bin_name" "$@" &
    else
        echo "Pre-built binary not found, falling back to: cargo run --bin $bin_name"
        cargo run --bin "$bin_name" -- "$@" &
    fi
}

# Helper: run cdk-mintd from $PATH (Nix pre-built) or fall back to cargo run
# with grpc-processor feature
run_mintd_bg() {
    if command -v cdk-mintd &>/dev/null; then
        echo "Using pre-built binary: cdk-mintd"
        cdk-mintd &
    else
        echo "Pre-built cdk-mintd not found, falling back to cargo run"
        cargo run --bin cdk-mintd --no-default-features --features grpc-processor &
    fi
}

# Helper: run cargo nextest with archive if available, or fall back to cargo test.
# For nextest: translates '-- --nocapture' to '--no-capture' and strips '--' separators.
#
# Usage: run_test <test_name> [extra cargo-test args...]
run_test() {
    local test_name="$1"
    shift
    if [ -n "${CDK_ITEST_ARCHIVE:-}" ] && [ -f "${CDK_ITEST_ARCHIVE:-}" ]; then
        # Build nextest args, translating cargo test conventions
        local nextest_args=()
        for arg in "$@"; do
            if [ "$arg" = "--" ]; then
                continue
            fi
            if [ "$arg" = "--nocapture" ]; then
                nextest_args+=("--no-capture")
            else
                nextest_args+=("$arg")
            fi
        done
        echo "Running test '$test_name' from nextest archive"
        cargo nextest run --archive-file "$CDK_ITEST_ARCHIVE" --workspace-remap . -E "binary(~$test_name)" "${nextest_args[@]}"
    else
        echo "Running test '$test_name' via cargo test"
        cargo test -p cdk-integration-tests --test "$test_name" "$@"
    fi
}
