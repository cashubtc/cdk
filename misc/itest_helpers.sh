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
        if [ -n "${CDK_BIN_FEATURES:-}" ]; then
            cargo run --features "$CDK_BIN_FEATURES" --bin "$bin_name" -- "$@"
        else
            cargo run --bin "$bin_name" -- "$@"
        fi
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
        if [ -n "${CDK_BIN_FEATURES:-}" ]; then
            cargo run --features "$CDK_BIN_FEATURES" --bin "$bin_name" -- "$@" &
        else
            cargo run --bin "$bin_name" -- "$@" &
        fi
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
# For nextest: translates cargo test conventions and strips '--' separators.
#
# Usage: run_test <test_name> [extra cargo-test args...]
run_test() {
    local test_name="$1"
    shift
    if [ -n "${CDK_ITEST_ARCHIVE:-}" ] && [ -f "${CDK_ITEST_ARCHIVE:-}" ]; then
        # Build nextest args, translating cargo test conventions
        local nextest_args=()
        local args=("$@")
        local i=0
        while [ "$i" -lt "${#args[@]}" ]; do
            local arg="${args[$i]}"
            if [ "$arg" = "--" ]; then
                i=$((i + 1))
                continue
            fi
            if [ "$arg" = "--nocapture" ]; then
                nextest_args+=("--no-capture")
            elif [ "$arg" = "--test-threads" ]; then
                i=$((i + 1))
                if [ "$i" -lt "${#args[@]}" ]; then
                    nextest_args+=("-j" "${args[$i]}")
                fi
            elif [[ "$arg" == --test-threads=* ]]; then
                nextest_args+=("-j" "${arg#--test-threads=}")
            else
                nextest_args+=("$arg")
            fi
            i=$((i + 1))
        done
        echo "Running test '$test_name' from nextest archive"
        cargo nextest run --archive-file "$CDK_ITEST_ARCHIVE" --workspace-remap . -E "binary(/^${test_name}$/)" "${nextest_args[@]}"
    else
        echo "Running test '$test_name' via cargo test"
        if [ -n "${CDK_ITEST_FEATURES:-}" ]; then
            cargo test -p cdk-integration-tests --features "$CDK_ITEST_FEATURES" --test "$test_name" "$@"
        else
            cargo test -p cdk-integration-tests --test "$test_name" "$@"
        fi
    fi
}
