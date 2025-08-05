#!/bin/bash

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
TEST_SUBSET=${TEST_SUBSET:-""}  # Can be set to "small" or "full"
RUN_INTEGRATION_TESTS=${RUN_INTEGRATION_TESTS:-"false"}  # Set to "true" to run integration tests that require external services
RUN_WASM_TESTS=${RUN_WASM_TESTS:-"false"}  # Set to "true" to run WASM tests
RUN_MSRV_TESTS=${RUN_MSRV_TESTS:-"false"}  # Set to "true" to run MSRV build tests
DEBUG=${DEBUG:-"false"}  # Set to "true" to show test output while running

# Array to store test configurations
declare -a TEST_CONFIGS

echo -e "${YELLOW}Starting CDK test matrix...${NC}"
echo -e "${BLUE}Configuration:${NC}"
echo -e "  TEST_SUBSET=${TEST_SUBSET}"
echo -e "  RUN_WASM_TESTS=${RUN_WASM_TESTS}"
echo -e "  RUN_INTEGRATION_TESTS=${RUN_INTEGRATION_TESTS}"
echo -e "  RUN_MSRV_TESTS=${RUN_MSRV_TESTS}"
echo -e "  DEBUG=${DEBUG}"
echo ""
echo -e "${BLUE}Usage examples:${NC}"
echo -e "  TEST_SUBSET=small ./misc/test_matrix.sh            # Run only basic tests"
echo -e "  TEST_SUBSET=full ./misc/test_matrix.sh             # Run all standard tests"
echo -e "  RUN_WASM_TESTS=true ./misc/test_matrix.sh          # Run only WASM tests"
echo -e "  RUN_INTEGRATION_TESTS=true ./misc/test_matrix.sh   # Run only integration tests"
echo -e "  RUN_MSRV_TESTS=true ./misc/test_matrix.sh          # Run only MSRV build tests"
echo -e "  DEBUG=true TEST_SUBSET=small ./misc/test_matrix.sh # Run with debug output"
echo ""

# Function to run a single test
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
    
    if [ "$DEBUG" = "true" ]; then
        echo -e "${YELLOW}Command: $cmd${NC}"
        if $cmd; then
            echo -e "✅ ${GREEN}PASSED${NC}: $test_name"
        else
            echo -e "❌ ${RED}FAILED${NC}: $test_name"
            echo -e "${RED}Command: $cmd${NC}"
            echo ""
            echo -e "${RED}Test '$test_name' failed. Exiting.${NC}"
            exit 1
        fi
    else
        if $cmd 2>/dev/null; then
            echo -e "✅ ${GREEN}PASSED${NC}: $test_name"
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

# Run tests sequentially
echo -e "${YELLOW}Running tests sequentially...${NC}"

# Execute all tests one by one
for config in "${TEST_CONFIGS[@]}"; do
    IFS='|' read -r name args type target <<< "$config"
    run_test "$name" "$args" "$type" "$target"
done

echo ""
echo -e "${GREEN}🎉 All tests passed!${NC}"