#!/bin/bash

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
MAX_PARALLEL_JOBS=${MAX_PARALLEL_JOBS:-$(( $(nproc) * 10 ))}
TEST_SUBSET=${TEST_SUBSET:-"all"}  # Can be set to "small" for testing
TEMP_DIR=$(mktemp -d)
RESULTS_FILE="$TEMP_DIR/results"
LOCK_FILE="$TEMP_DIR/lock"

# Counters
TOTAL_TESTS=0
PASSED_TESTS=0
FAILED_TESTS=0

# Arrays to store test configurations and results
declare -a TEST_CONFIGS
declare -a FAILED_TEST_DETAILS

# Cleanup function
cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

echo -e "${YELLOW}Starting complete CDK test matrix (parallel execution)...${NC}"
echo -e "${BLUE}Running with $MAX_PARALLEL_JOBS parallel jobs${NC}"
echo ""

# Function to update results
update_counters() {
    local status="$1"
    local test_name="$2"
    local command="$3"
    local pid=$$

    if [ "$status" = "PASSED" ]; then
        echo "PASSED" >> "$RESULTS_FILE.$pid"
    else
        echo "FAILED" >> "$RESULTS_FILE.$pid"
        echo "$test_name: $command" >> "$RESULTS_FILE.$pid.failed" 2>/dev/null || true
    fi
}

# Function to run a single test
run_test() {
    local test_name="$1"
    local cargo_args="$2"
    local test_type="${3:-clippy}"  # clippy, test, or build
    
    local cmd=""
    case "$test_type" in
        "clippy")
            cmd="cargo clippy $cargo_args -- -D warnings"
            ;;
        "test")
            cmd="cargo test $cargo_args"
            ;;
        "build")
            cmd="cargo build $cargo_args"
            ;;
        "check")
            cmd="cargo check $cargo_args"
            ;;
    esac

    if $cmd 2>/dev/null; then
        echo -e "✅ ${GREEN}PASSED${NC}: $test_name"
        update_counters "PASSED" "$test_name" "$cmd"
    else
        echo -e "❌ ${RED}FAILED${NC}: $test_name"
        echo -e "${RED}Command: $cmd${NC}"
        update_counters "FAILED" "$test_name" "$cmd"
        # Show actual error for debugging
        echo -e "${RED}Error details:${NC}"
        $cmd || true
        echo ""
    fi
}

# Add test configuration
add_test() {
    local name="$1"
    local args="$2"
    local type="${3:-clippy}"
    TEST_CONFIGS+=("$name|$args|$type")
    TOTAL_TESTS=$((TOTAL_TESTS + 1))
}


# All build configurations from CI workflow
echo -e "${BLUE}Adding all CI build configurations...${NC}"

if [ "$TEST_SUBSET" = "small" ]; then
    echo -e "${YELLOW}Running small test subset for validation...${NC}"
    # Just a few key tests for validation
    add_test "cashu (default)" "-p cashu" "clippy"
    add_test "cdk (default)" "-p cdk" "clippy"
    add_test "cdk-mintd (default)" "--bin cdk-mintd" "clippy"
    add_test "example: wallet" "--example wallet" "check"
    add_test "doc tests" "--doc" "test"
else
    echo -e "${BLUE}Running full test matrix...${NC}"

    # Examples (these run but don't need clippy/test)
    add_test "example: mint-token" "--example mint-token" "check"
    add_test "example: melt-token" "--example melt-token" "check"
    add_test "example: p2pk" "--example p2pk" "check"
    add_test "example: proof-selection" "--example proof-selection" "check"
    add_test "example: wallet" "--example wallet" "check"

    # Main clippy and test matrix from CI
    add_test "cashu (default)" "-p cashu" "clippy"
    add_test "cashu (no default)" "-p cashu --no-default-features" "clippy"
    add_test "cashu (wallet only)" "-p cashu --no-default-features --features wallet" "clippy"
    add_test "cashu (mint only)" "-p cashu --no-default-features --features mint" "clippy"
    add_test "cashu (mint + swagger)" '-p cashu --no-default-features --features mint --features swagger' "clippy"
    add_test "cashu (auth only)" "-p cashu --no-default-features --features auth" "clippy"
    add_test "cashu (mint + auth)" '-p cashu --no-default-features --features mint --features auth' "clippy"
    add_test "cashu (wallet + auth)" '-p cashu --no-default-features --features wallet --features auth' "clippy"

    add_test "cdk-common (default)" "-p cdk-common" "clippy"
    add_test "cdk-common (no default)" "-p cdk-common --no-default-features" "clippy"
    add_test "cdk-common (wallet only)" "-p cdk-common --no-default-features --features wallet" "clippy"
    add_test "cdk-common (mint only)" "-p cdk-common --no-default-features --features mint" "clippy"
    add_test "cdk-common (auth only)" "-p cdk-common --no-default-features --features auth" "clippy"
    add_test 'cdk-common (mint + swagger)' '-p cdk-common --no-default-features --features mint --features swagger' "clippy"
    add_test 'cdk-common (mint + auth)' '-p cdk-common --no-default-features --features mint --features auth' "clippy"
    add_test 'cdk-common (wallet + auth)' '-p cdk-common --no-default-features --features wallet --features auth' "clippy"

    add_test "cdk (default)" "-p cdk" "clippy"
    add_test "cdk (no default)" "-p cdk --no-default-features" "clippy"
    add_test "cdk (wallet only)" "-p cdk --no-default-features --features wallet" "clippy"
    add_test "cdk (mint only)" "-p cdk --no-default-features --features mint" "clippy"
    add_test "cdk (auth only)" "-p cdk --no-default-features --features auth" "clippy"
    add_test "cdk (auth default)" "-p cdk --features auth" "clippy"
    add_test "cdk (mint + swagger)" '-p cdk --no-default-features --features mint --features swagger' "clippy"
    add_test "cdk (auth + mint)" '-p cdk --no-default-features --features auth --features mint' "clippy"
    add_test "cdk (auth + wallet)" '-p cdk --no-default-features --features auth --features wallet' "clippy"

    add_test "cdk-redb" "-p cdk-redb" "clippy"
    add_test "cdk-sqlite" "-p cdk-sqlite" "clippy"
    add_test "cdk-sqlite (sqlcipher)" "-p cdk-sqlite --features sqlcipher" "clippy"

    add_test "cdk-axum (no default)" "-p cdk-axum --no-default-features" "clippy"
    add_test "cdk-axum (swagger only)" "-p cdk-axum --no-default-features --features swagger" "clippy"
    add_test "cdk-axum (redis only)" "-p cdk-axum --no-default-features --features redis" "clippy"
    add_test "cdk-axum (redis + swagger)" '-p cdk-axum --no-default-features --features redis --features swagger' "clippy"
    add_test "cdk-axum (auth + redis)" '-p cdk-axum --no-default-features --features auth --features redis' "clippy"
    add_test "cdk-axum (default)" "-p cdk-axum" "clippy"

    add_test "cdk-cln" "-p cdk-cln" "clippy"
    add_test "cdk-lnd" "-p cdk-lnd" "clippy"
    add_test "cdk-lnbits" "-p cdk-lnbits" "clippy"
    add_test "cdk-fake-wallet" "-p cdk-fake-wallet" "clippy"
    add_test "cdk-payment-processor" "-p cdk-payment-processor" "clippy"

    # CLI binaries
    add_test "cdk-cli (default)" "--bin cdk-cli" "clippy"
    add_test "cdk-cli (sqlcipher)" "--bin cdk-cli --features sqlcipher" "clippy"
    add_test "cdk-cli (redb)" "--bin cdk-cli --features redb" "clippy"
    add_test "cdk-cli (sqlcipher + redb)" '--bin cdk-cli --features sqlcipher --features redb' "clippy"

    # cdk-mintd binary tests (missing from current script)
    add_test "cdk-mintd (default)" "--bin cdk-mintd" "clippy"
    add_test "cdk-mintd (redis)" "--bin cdk-mintd --features redis" "clippy"
    add_test "cdk-mintd (redb)" "--bin cdk-mintd --features redb" "clippy"
    add_test "cdk-mintd (redis + swagger + redb)" '--bin cdk-mintd --features redis --features swagger --features redb' "clippy"
    add_test "cdk-mintd (sqlcipher)" "--bin cdk-mintd --features sqlcipher" "clippy"
    add_test "cdk-mintd (lnd only)" "--bin cdk-mintd --no-default-features --features lnd" "clippy"
    add_test "cdk-mintd (cln only)" "--bin cdk-mintd --no-default-features --features cln" "clippy"
    add_test "cdk-mintd (lnbits only)" "--bin cdk-mintd --no-default-features --features lnbits" "clippy"
    add_test "cdk-mintd (fakewallet only)" "--bin cdk-mintd --no-default-features --features fakewallet" "clippy"
    add_test "cdk-mintd (grpc-processor only)" "--bin cdk-mintd --no-default-features --features grpc-processor" "clippy"
    add_test "cdk-mintd (management-rpc + lnd)" '--bin cdk-mintd --no-default-features --features management-rpc --features lnd' "clippy"
    add_test "cdk-mintd (management-rpc + cln)" '--bin cdk-mintd --no-default-features --features management-rpc --features cln' "clippy"
    add_test "cdk-mintd (management-rpc + lnbits)" '--bin cdk-mintd --no-default-features --features management-rpc --features lnbits' "clippy"
    add_test "cdk-mintd (management-rpc + grpc-processor)" '--bin cdk-mintd --no-default-features --features management-rpc --features grpc-processor' "clippy"
    add_test "cdk-mintd (swagger + lnd)" '--bin cdk-mintd --no-default-features --features swagger --features lnd' "clippy"
    add_test "cdk-mintd (swagger + cln)" '--bin cdk-mintd --no-default-features --features swagger --features cln' "clippy"
    add_test "cdk-mintd (swagger + lnbits)" '--bin cdk-mintd --no-default-features --features swagger --features lnbits' "clippy"
    add_test "cdk-mintd (auth + lnd)" '--bin cdk-mintd --no-default-features --features auth --features lnd' "clippy"
    add_test "cdk-mintd (auth + cln)" '--bin cdk-mintd --no-default-features --features auth --features cln' "clippy"
    add_test "cdk-mint-cli" "--bin cdk-mint-cli" "clippy"
    add_test "cdk-mint-rpc" "-p cdk-mint-rpc" "clippy"
    add_test "doc tests" "--doc" "test"

fi

echo -e "${BLUE}Total test configurations: $TOTAL_TESTS${NC}"
echo ""

# Run tests in parallel
echo -e "${YELLOW}Running tests in parallel...${NC}"

# Function to run tests in parallel batches
run_parallel_tests() {
    local pids=()
    local batch_count=0
    
    for config in "${TEST_CONFIGS[@]}"; do
        IFS='|' read -r name args type <<< "$config"
        
        # Start test in background
        {
            echo -e "${BLUE}Starting: $name${NC}"
            run_test "$name" "$args" "$type"
        } &
        
        pids+=($!)
        batch_count=$((batch_count + 1))
        
        # When we reach batch size, wait for completion
        if [ ${#pids[@]} -eq $MAX_PARALLEL_JOBS ]; then
            for pid in "${pids[@]}"; do
                wait $pid
            done
            pids=()
        fi
    done
    
    # Wait for any remaining background jobs
    for pid in "${pids[@]}"; do
        wait $pid
    done
}

# Execute all tests
run_parallel_tests

# Count results
if [ -f "$RESULTS_FILE" ]; then
    PASSED_TESTS=$(grep -c "PASSED" "$RESULTS_FILE" 2>/dev/null || echo 0)
    FAILED_TESTS=$(grep -c "FAILED" "$RESULTS_FILE" 2>/dev/null || echo 0)
fi

# Read failed test details
if [ -f "$RESULTS_FILE.failed" ]; then
    mapfile -t FAILED_TEST_DETAILS < "$RESULTS_FILE.failed" || true
fi

# Print summary
echo ""
echo -e "${YELLOW}========================================${NC}"
echo -e "${YELLOW}TEST SUMMARY${NC}"
echo -e "${YELLOW}========================================${NC}"
echo -e "Total tests: $TOTAL_TESTS"
echo -e "${GREEN}Passed: $PASSED_TESTS${NC}"
echo -e "${RED}Failed: $FAILED_TESTS${NC}"

if [ $FAILED_TESTS -gt 0 ]; then
    echo ""
    echo -e "${RED}FAILED TESTS:${NC}"
    for detail in "${FAILED_TEST_DETAILS[@]}"; do
        echo -e "${RED}  - $detail${NC}"
    done
    echo ""
    echo -e "${RED}Some tests failed. Please check the errors above.${NC}"
    exit 1
else
    echo ""
    echo -e "${GREEN}🎉 All tests passed!${NC}"
    exit 0
fi