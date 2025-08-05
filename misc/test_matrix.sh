#!/bin/bash

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
NC='\033[0m' # No Color

# Configuration
TEST_SUBSET=${TEST_SUBSET:-""}  # Can be set to "small" or "full"
RUN_INTEGRATION_TESTS=${RUN_INTEGRATION_TESTS:-"false"}  # Set to "true" to run integration tests that require external services
RUN_WASM_TESTS=${RUN_WASM_TESTS:-"false"}  # Set to "true" to run WASM tests
RUN_MSRV_TESTS=${RUN_MSRV_TESTS:-"false"}  # Set to "true" to run MSRV build tests
DEBUG=${DEBUG:-"false"}  # Set to "true" to show test output while running

# Parallel execution configuration
MAX_PARALLEL_JOBS=${MAX_PARALLEL_JOBS:-""}  # Auto-detect if not set
SEQUENTIAL=${SEQUENTIAL:-"false"}  # Set to "true" to force sequential execution
PROGRESS_UPDATE_INTERVAL=${PROGRESS_UPDATE_INTERVAL:-1}  # Progress update interval in seconds

# Auto-detect optimal parallel job count
if [ -z "$MAX_PARALLEL_JOBS" ]; then
    if command -v nproc >/dev/null 2>&1; then
        # Linux
        MAX_PARALLEL_JOBS=$(nproc)
    elif command -v sysctl >/dev/null 2>&1; then
        # macOS
        MAX_PARALLEL_JOBS=$(sysctl -n hw.ncpu)
    else
        # Fallback
        MAX_PARALLEL_JOBS=4
    fi
    
    # For CPU-intensive tasks like compilation, use slightly fewer cores to avoid thrashing
    if [ "$MAX_PARALLEL_JOBS" -gt 4 ]; then
        MAX_PARALLEL_JOBS=$((MAX_PARALLEL_JOBS - 1))
    fi
fi

# Ensure minimum of 1 job
if [ "$MAX_PARALLEL_JOBS" -lt 1 ]; then
    MAX_PARALLEL_JOBS=1
fi

# Array to store test configurations
declare -a TEST_CONFIGS

# Parallel execution globals
declare -a ACTIVE_JOBS=()
declare -a FAILED_TESTS=()
declare -a PASSED_TESTS=()
declare -a JOB_NAMES=()
declare -a JOB_START_TIMES=()
TOTAL_TESTS=0
COMPLETED_TESTS=0
START_TIME=$(date +%s)

echo -e "${YELLOW}Starting CDK test matrix...${NC}"
echo -e "${BLUE}Configuration:${NC}"
echo -e "  TEST_SUBSET=${TEST_SUBSET}"
echo -e "  RUN_WASM_TESTS=${RUN_WASM_TESTS}"
echo -e "  RUN_INTEGRATION_TESTS=${RUN_INTEGRATION_TESTS}"
echo -e "  RUN_MSRV_TESTS=${RUN_MSRV_TESTS}"
echo -e "  DEBUG=${DEBUG}"
echo -e "  MAX_PARALLEL_JOBS=${MAX_PARALLEL_JOBS}"
echo -e "  SEQUENTIAL=${SEQUENTIAL}"
echo ""
echo -e "${BLUE}Usage examples:${NC}"
echo -e "  TEST_SUBSET=small ./misc/test_matrix.sh            # Run only basic tests"
echo -e "  TEST_SUBSET=full ./misc/test_matrix.sh             # Run all standard tests"
echo -e "  RUN_WASM_TESTS=true ./misc/test_matrix.sh          # Run only WASM tests"
echo -e "  RUN_INTEGRATION_TESTS=true ./misc/test_matrix.sh   # Run only integration tests"
echo -e "  RUN_MSRV_TESTS=true ./misc/test_matrix.sh          # Run only MSRV build tests"
echo -e "  DEBUG=true TEST_SUBSET=small ./misc/test_matrix.sh # Run with debug output"
echo -e "  SEQUENTIAL=true TEST_SUBSET=small ./misc/test_matrix.sh # Force sequential execution"
echo -e "  MAX_PARALLEL_JOBS=8 TEST_SUBSET=full ./misc/test_matrix.sh # Use 8 parallel jobs"
echo ""

# Function to run a single test (parallel version)
run_test_parallel() {
    local test_name="$1"
    local cargo_args="$2"
    local test_type="${3:-clippy}"  # clippy, test, build, check, wasm-build, or integration
    local target="${4:-""}"  # Optional target for cross-compilation
    local job_id="$5"
    
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
        "wasm-build")
            if [ -n "$target" ]; then
                cmd="cargo build $cargo_args --target $target"
            else
                cmd="cargo build $cargo_args --target wasm32-unknown-unknown"
            fi
            ;;
        "docs-strict")
            cmd="just docs-strict"
            ;;
        "integration")
            # Integration tests are handled separately
            cmd="$cargo_args"  # cargo_args contains the full command for integration tests
            ;;
    esac

    local start_time=$(date +%s)
    local log_file="/tmp/test_matrix_${job_id}.log"
    
    # Run the command and capture output
    if [ "$DEBUG" = "true" ]; then
        echo -e "${CYAN}[${job_id}]${NC} ${BLUE}Starting: $test_name${NC}"
        echo -e "${CYAN}[${job_id}]${NC} ${YELLOW}Command: $cmd${NC}"
        if $cmd 2>&1 | tee "$log_file"; then
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            echo -e "${CYAN}[${job_id}]${NC} ✅ ${GREEN}PASSED${NC}: $test_name (${duration}s)"
            echo "PASSED|$test_name|$duration" > "/tmp/test_result_${job_id}"
        else
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            echo -e "${CYAN}[${job_id}]${NC} ❌ ${RED}FAILED${NC}: $test_name (${duration}s)"
            echo -e "${CYAN}[${job_id}]${NC} ${RED}Command: $cmd${NC}"
            echo "FAILED|$test_name|$duration|$cmd" > "/tmp/test_result_${job_id}"
        fi
    else
        if $cmd > "$log_file" 2>&1; then
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            echo -e "${CYAN}[${job_id}]${NC} ✅ ${GREEN}PASSED${NC}: $test_name (${duration}s)"
            echo "PASSED|$test_name|$duration" > "/tmp/test_result_${job_id}"
        else
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            echo -e "${CYAN}[${job_id}]${NC} ❌ ${RED}FAILED${NC}: $test_name (${duration}s)"
            echo "FAILED|$test_name|$duration|$cmd" > "/tmp/test_result_${job_id}"
        fi
    fi
}

# Function to run a single test (sequential version - kept for compatibility)
run_test() {
    local test_name="$1"
    local cargo_args="$2"
    local test_type="${3:-clippy}"  # clippy, test, build, check, wasm-build, or integration
    local target="${4:-""}"  # Optional target for cross-compilation
    
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
        "wasm-build")
            if [ -n "$target" ]; then
                cmd="cargo build $cargo_args --target $target"
            else
                cmd="cargo build $cargo_args --target wasm32-unknown-unknown"
            fi
            ;;
        "docs-strict")
            cmd="just docs-strict"
            ;;
        "integration")
            # Integration tests are handled separately
            cmd="$cargo_args"  # cargo_args contains the full command for integration tests
            ;;
    esac

    echo -e "${BLUE}Starting: $test_name${NC}"
    local start_time=$(date +%s)
    
    if [ "$DEBUG" = "true" ]; then
        echo -e "${YELLOW}Command: $cmd${NC}"
        if $cmd; then
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            echo -e "✅ ${GREEN}PASSED${NC}: $test_name (${duration}s)"
        else
            echo -e "❌ ${RED}FAILED${NC}: $test_name"
            echo -e "${RED}Command: $cmd${NC}"
            echo ""
            echo -e "${RED}Test '$test_name' failed. Exiting.${NC}"
            exit 1
        fi
    else
        if $cmd 2>/dev/null; then
            local end_time=$(date +%s)
            local duration=$((end_time - start_time))
            echo -e "✅ ${GREEN}PASSED${NC}: $test_name (${duration}s)"
        else
            echo -e "❌ ${RED}FAILED${NC}: $test_name"
            echo -e "${RED}Command: $cmd${NC}"
            # Show actual error for debugging
            echo -e "${RED}Error details:${NC}"
            $cmd || true
            echo ""
            echo -e "${RED}Test '$test_name' failed. Exiting.${NC}"
            exit 1
        fi
    fi
}

# Cleanup function
cleanup_parallel() {
    # Kill any remaining background jobs
    for job_id in "${ACTIVE_JOBS[@]}"; do
        if kill -0 "$job_id" 2>/dev/null; then
            kill "$job_id" 2>/dev/null || true
        fi
    done
    
    # Clean up temporary files
    rm -f /tmp/test_matrix_*.log 2>/dev/null || true
    rm -f /tmp/test_result_* 2>/dev/null || true
}

# Set up signal handlers for cleanup
trap cleanup_parallel EXIT INT TERM

# Function to wait for a job slot to become available
wait_for_slot() {
    while [ ${#ACTIVE_JOBS[@]} -ge "$MAX_PARALLEL_JOBS" ]; do
        check_completed_jobs
        sleep 0.1
    done
}

# Function to check for completed jobs and update status
check_completed_jobs() {
    local new_active_jobs=()
    
    for job_id in "${ACTIVE_JOBS[@]}"; do
        if kill -0 "$job_id" 2>/dev/null; then
            # Job is still running
            new_active_jobs+=("$job_id")
        else
            # Job completed, process results
            if [ -f "/tmp/test_result_${job_id}" ]; then
                local result=$(cat "/tmp/test_result_${job_id}")
                IFS='|' read -r status test_name duration cmd <<< "$result"
                
                if [ "$status" = "PASSED" ]; then
                    PASSED_TESTS+=("$test_name")
                else
                    FAILED_TESTS+=("$test_name|$cmd")
                    # In non-fail-fast mode, we continue; in fail-fast mode we would exit here
                fi
                
                COMPLETED_TESTS=$((COMPLETED_TESTS + 1))
                rm -f "/tmp/test_result_${job_id}" 2>/dev/null || true
            fi
        fi
    done
    
    ACTIVE_JOBS=("${new_active_jobs[@]}")
}

# Function to show progress
show_progress() {
    local elapsed_time=$(($(date +%s) - START_TIME))
    local elapsed_formatted=$(printf "%02d:%02d" $((elapsed_time / 60)) $((elapsed_time % 60)))
    
    if [ "$TOTAL_TESTS" -gt 0 ]; then
        local percent=$((COMPLETED_TESTS * 100 / TOTAL_TESTS))
        local active_count=${#ACTIVE_JOBS[@]}
        local passed_count=${#PASSED_TESTS[@]}
        local failed_count=${#FAILED_TESTS[@]}
        
        echo -e "${MAGENTA}Progress: ${percent}% (${COMPLETED_TESTS}/${TOTAL_TESTS}) | Active: ${active_count} | Passed: ${passed_count} | Failed: ${failed_count} | Time: ${elapsed_formatted}${NC}"
    fi
}

# Function to run tests in parallel
run_tests_parallel() {
    echo -e "${YELLOW}Running tests in parallel with ${MAX_PARALLEL_JOBS} workers...${NC}"
    echo ""
    
    TOTAL_TESTS=${#TEST_CONFIGS[@]}
    local test_index=0
    
    # Start initial batch of jobs
    for config in "${TEST_CONFIGS[@]}"; do
        wait_for_slot
        
        IFS='|' read -r name args type target <<< "$config"
        
        # Generate unique job ID
        local job_id=$(($(date +%s%N) + test_index))
        
        # Start job in background
        (
            run_test_parallel "$name" "$args" "$type" "$target" "$job_id"
        ) &
        
        local pid=$!
        ACTIVE_JOBS+=("$pid")
        
        test_index=$((test_index + 1))
        
        # Show progress periodically
        if [ $((test_index % 5)) -eq 0 ] || [ "$test_index" -eq "$TOTAL_TESTS" ]; then
            check_completed_jobs
            show_progress
        fi
        
        # Small delay to prevent overwhelming the system
        sleep 0.05
    done
    
    echo -e "${BLUE}All tests queued. Waiting for completion...${NC}"
    
    # Wait for all remaining jobs to complete
    while [ ${#ACTIVE_JOBS[@]} -gt 0 ]; do
        check_completed_jobs
        show_progress
        sleep "$PROGRESS_UPDATE_INTERVAL"
    done
    
    # Final progress update
    check_completed_jobs
    show_progress
    echo ""
}

# Function to display final results
show_final_results() {
    local total_time=$(($(date +%s) - START_TIME))
    local total_time_formatted=$(printf "%02d:%02d" $((total_time / 60)) $((total_time % 60)))
    
    echo -e "${BLUE}============== FINAL RESULTS ==============${NC}"
    echo -e "${GREEN}Passed tests: ${#PASSED_TESTS[@]}${NC}"
    echo -e "${RED}Failed tests: ${#FAILED_TESTS[@]}${NC}"
    echo -e "${BLUE}Total execution time: ${total_time_formatted}${NC}"
    echo ""
    
    if [ ${#FAILED_TESTS[@]} -gt 0 ]; then
        echo -e "${RED}❌ FAILED TESTS:${NC}"
        for failed_test in "${FAILED_TESTS[@]}"; do
            IFS='|' read -r test_name cmd <<< "$failed_test"
            echo -e "${RED}  • $test_name${NC}"
            if [ -n "$cmd" ] && [ "$DEBUG" = "true" ]; then
                echo -e "${RED}    Command: $cmd${NC}"
            fi
        done
        echo ""
        return 1
    else
        echo -e "${GREEN}🎉 All tests passed!${NC}"
        return 0
    fi
}

# Add test configuration
add_test() {
    local name="$1"
    local args="$2"
    local type="${3:-clippy}"
    local target="${4:-""}"
    if [ -n "$target" ]; then
        TEST_CONFIGS+=("$name|$args|$type|$target")
    else
        TEST_CONFIGS+=("$name|$args|$type")
    fi
}

echo -e "${BLUE}Adding all CI build configurations...${NC}"

if [ "$TEST_SUBSET" = "small" ]; then
    echo -e "${YELLOW}Running small test subset for validation...${NC}"
    # Just a few key tests for validation
    add_test "cashu (default)" "-p cashu" "clippy"
    add_test "cdk (default)" "-p cdk" "clippy"
    add_test "cdk-mintd (default)" "--bin cdk-mintd" "clippy"
    add_test "example: wallet" "--example wallet" "check"
    add_test "doc tests" "--doc" "test"
elif [ "$TEST_SUBSET" = "full" ]; then
    echo -e "${BLUE}Running full test matrix...${NC}"

    # Examples (these run but don't need clippy/test)
    add_test "example: mint-token" "--example mint-token" "check"
    add_test "example: melt-token" "--example melt-token" "check"
    add_test "example: p2pk" "--example p2pk" "check"
    add_test "example: proof-selection" "--example proof-selection" "check"
    add_test "example: wallet" "--example wallet" "check"
    add_test "example: auth_wallet" "--example auth_wallet" "check"

    # Main clippy and test matrix from CI
    add_test "cashu (default)" "-p cashu" "clippy"
    add_test "cashu (no default)" "-p cashu --no-default-features" "clippy"
    add_test "cashu (wallet only)" "-p cashu --no-default-features --features wallet" "clippy"
    add_test "cashu (mint only)" "-p cashu --no-default-features --features mint" "clippy"
    add_test "cashu (wallet + mint)" '-p cashu --no-default-features --features wallet --features mint' "clippy"
    add_test "cashu (mint + swagger)" '-p cashu --no-default-features --features mint --features swagger' "clippy"
    add_test "cashu (auth only)" "-p cashu --no-default-features --features auth" "clippy"
    add_test "cashu (mint + auth)" '-p cashu --no-default-features --features mint --features auth' "clippy"
    add_test "cashu (wallet + auth)" '-p cashu --no-default-features --features wallet --features auth' "clippy"

    add_test "cdk-common (default)" "-p cdk-common" "clippy"
    add_test "cdk-common (no default)" "-p cdk-common --no-default-features" "clippy"
    add_test "cdk-common (wallet only)" "-p cdk-common --no-default-features --features wallet" "clippy"
    add_test "cdk-common (mint only)" "-p cdk-common --no-default-features --features mint" "clippy"
    add_test "cdk-common (wallet + mint)" '-p cdk-common --no-default-features --features wallet --features mint' "clippy"
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
    add_test "cdk (http_subscription)" '-p cdk --no-default-features --features http_subscription' "clippy"
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
    add_test "cdk-payment-processor (no default)" "-p cdk-payment-processor --no-default-features" "clippy"

    # CLI binaries
    add_test "cdk-cli (default)" "--bin cdk-cli" "clippy"
    add_test "cdk-cli (sqlcipher)" "--bin cdk-cli --features sqlcipher" "clippy"
    add_test "cdk-cli (redb)" "--bin cdk-cli --features redb" "clippy"
    add_test "cdk-cli (sqlcipher + redb)" '--bin cdk-cli --features sqlcipher --features redb' "clippy"

    # cdk-mintd binary tests
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
    
    # Documentation tests
    add_test "strict documentation" "" "docs-strict"

# WASM Tests (from check-wasm and check-wasm-msrv jobs)
elif [ "$RUN_WASM_TESTS" = "true" ]; then
    echo -e "${BLUE}Adding WASM target tests...${NC}"
    echo -e "${YELLOW}Note: WASM tests require 'rustup target add wasm32-unknown-unknown'${NC}"
    add_test "cdk WASM (default)" "-p cdk" "wasm-build" "wasm32-unknown-unknown"
    add_test "cdk WASM (no default)" "-p cdk --no-default-features" "wasm-build" "wasm32-unknown-unknown"
    add_test "cdk WASM (wallet only)" "-p cdk --no-default-features --features wallet" "wasm-build" "wasm32-unknown-unknown"

# Integration tests (require external services)
elif [ "$RUN_INTEGRATION_TESTS" = "true" ]; then
    echo -e "${BLUE}Adding integration tests (require external services)...${NC}"
    
    # Pure integration tests (fake wallet tests)
    add_test "pure itest (memory)" "just test-pure memory" "integration"
    add_test "pure itest (sqlite)" "just test-pure sqlite" "integration"
    add_test "pure itest (redb)" "just test-pure redb" "integration"
    add_test "pure test (mint)" "just test" "integration"
    
    # Fake mint integration tests
    add_test "fake mint itest (REDB)" "just fake-mint-itest REDB" "integration"
    add_test "fake mint itest (SQLITE)" "just fake-mint-itest SQLITE" "integration"
    
    # Regtest integration tests
    add_test "regtest itest (REDB)" "just itest REDB" "integration"
    add_test "regtest itest (SQLITE)" "just itest SQLITE" "integration"
    
    # Payment processor tests
    add_test "payment processor (FAKEWALLET)" "just itest-payment-processor FAKEWALLET" "integration"
    add_test "payment processor (CLN)" "just itest-payment-processor CLN" "integration"
    add_test "payment processor (LND)" "just itest-payment-processor LND" "integration"
    
    echo -e "${YELLOW}Note: fake-mint-auth-itest requires Keycloak and Docker - skipping for now${NC}"
    echo -e "${YELLOW}To run: docker compose -f misc/keycloak/docker-compose-recover.yml up -d${NC}"
    echo -e "${YELLOW}Then: just fake-auth-mint-itest REDB http://127.0.0.1:8080/realms/cdk-test-realm/.well-known/openid-configuration${NC}"

# MSRV Build Tests (from msrv-build job)
elif [ "$RUN_MSRV_TESTS" = "true" ]; then
    echo -e "${BLUE}Adding MSRV build tests...${NC}"
    echo -e "${YELLOW}Note: MSRV tests use build instead of clippy to match CI behavior${NC}"
    
    # MSRV build configurations from CI
    add_test "MSRV: cashu (wallet + mint)" "-p cashu --no-default-features --features wallet --features mint" "build"
    add_test "MSRV: cdk-common (wallet + mint)" "-p cdk-common --no-default-features --features wallet --features mint" "build"
    add_test "MSRV: cdk (default)" "-p cdk" "build"
    add_test "MSRV: cdk (mint + auth)" "-p cdk --no-default-features --features mint --features auth" "build"
    add_test "MSRV: cdk (wallet + auth)" "-p cdk --no-default-features --features wallet --features auth" "build"
    add_test "MSRV: cdk (http_subscription)" "-p cdk --no-default-features --features http_subscription" "build"
    add_test "MSRV: cdk-axum (default)" "-p cdk-axum" "build"
    add_test "MSRV: cdk-axum (redis only)" "-p cdk-axum --no-default-features --features redis" "build"
    add_test "MSRV: cdk-lnbits" "-p cdk-lnbits" "build"
    add_test "MSRV: cdk-fake-wallet" "-p cdk-fake-wallet" "build"
    add_test "MSRV: cdk-cln" "-p cdk-cln" "build"
    add_test "MSRV: cdk-lnd" "-p cdk-lnd" "build"
    add_test "MSRV: cdk-mint-rpc" "-p cdk-mint-rpc" "build"
    add_test "MSRV: cdk-sqlite" "-p cdk-sqlite" "build"
    add_test "MSRV: cdk-mintd" "-p cdk-mintd" "build"
    add_test "MSRV: cdk-payment-processor (no default)" "-p cdk-payment-processor --no-default-features" "build"
else
    echo -e "${YELLOW}No test type specified!${NC}"
    echo -e "${BLUE}Please specify one of the following:${NC}"
    echo -e "  TEST_SUBSET=small ./misc/test_matrix.sh            # Run basic validation tests"
    echo -e "  TEST_SUBSET=full ./misc/test_matrix.sh             # Run comprehensive clippy tests"
    echo -e "  RUN_WASM_TESTS=true ./misc/test_matrix.sh          # Run WASM target tests"
    echo -e "  RUN_INTEGRATION_TESTS=true ./misc/test_matrix.sh   # Run integration tests"
    echo -e "  RUN_MSRV_TESTS=true ./misc/test_matrix.sh          # Run MSRV build tests"
    echo ""
    echo -e "${YELLOW}Note: These test types are mutually exclusive.${NC}"
    exit 0
fi

echo -e "${BLUE}Total test configurations: ${#TEST_CONFIGS[@]}${NC}"
echo ""

# Choose execution mode
if [ "$SEQUENTIAL" = "true" ]; then
    echo -e "${YELLOW}Running tests sequentially (forced)...${NC}"
    
    # Execute all tests one by one
    for config in "${TEST_CONFIGS[@]}"; do
        IFS='|' read -r name args type target <<< "$config"
        run_test "$name" "$args" "$type" "$target"
    done
    
    echo ""
    echo -e "${GREEN}🎉 All tests passed!${NC}"
else
    # Run tests in parallel (default)
    run_tests_parallel
    
    # Show final results and exit with appropriate code
    if show_final_results; then
        exit 0
    else
        exit 1
    fi
fi