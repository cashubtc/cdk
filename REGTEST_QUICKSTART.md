# CDK Interactive Regtest - Quick Start

A simple guide to get up and running with the interactive regtest environment.

## Start Environment

```bash
# Terminal 1: Start regtest with default sqlite database
just regtest

# Or with redb database  
just regtest redb
```

The script will:
1. Check for `mprocs` and offer to install it if missing  
2. Set up the regtest environment (Bitcoin + Lightning nodes + CDK mints)
3. Launch `mprocs` showing logs from all components
4. Press 'q' in mprocs to quit and stop the environment

## Use Lightning Nodes (in any other terminal)

The `just` commands work from any terminal - they automatically find the running environment.

### Get Node Information
```bash
just ln-cln1 getinfo    # CLN node 1
just ln-cln2 getinfo    # CLN node 2  
just ln-lnd1 getinfo    # LND node 1
just ln-lnd2 getinfo    # LND node 2
```

### Create and Pay Invoices
```bash
# Create 1000 sat invoice on CLN
just ln-cln1 invoice 1000 test_label "Test payment"

# Pay invoice with LND (use the bolt11 from above)
just ln-lnd1 payinvoice lnbcrt10u1...

# Check balances
just ln-cln1 listfunds
just ln-lnd1 listchannels
```

## Bitcoin Operations

```bash
just btc getblockchaininfo    # Blockchain status
just btc getbalance          # Wallet balance
just btc-mine 5              # Mine 5 blocks
```

## CDK Mint Operations

```bash
just mint-info        # Show both mints' info
just mint-test        # Run integration tests
just restart-mints    # Stop, recompile, and restart mints
just regtest-status   # Check all components
just regtest-logs     # Show recent logs
just regtest-mprocs   # Start mprocs TUI (if not already running)
```

## Stop Environment

Press `Ctrl+C` in the terminal running `just regtest`. Everything will be cleaned up automatically.

## Available Endpoints

- **CLN Mint**: http://127.0.0.1:8085
- **LND Mint**: http://127.0.0.1:8087  
- **Bitcoin RPC**: 127.0.0.1:18443 (testuser/testpass)

## Common Workflows

### Test Lightning Payment Flow
```bash
# Terminal 1
just regtest

# Terminal 2  
just ln-cln1 invoice 1000 test "Test payment"
# Copy the bolt11 invoice

just ln-lnd1 payinvoice <bolt11>
just ln-cln1 listinvoices
just ln-lnd1 listpayments
```

### Test CDK Integration
```bash
# Terminal 1
just regtest

# Terminal 2
just mint-test                           # Run all tests
cargo test -p cdk-integration-tests      # Or specific tests
```

### Development with CDK CLI
```bash  
# Terminal 1
just regtest

# Terminal 2 - use environment variables
echo $CDK_TEST_MINT_URL      # CLN mint URL
echo $CDK_TEST_MINT_URL_2    # LND mint URL

# Use with CDK CLI
cargo run --bin cdk-cli -- --mint-url $CDK_TEST_MINT_URL mint-info
```

### Development Workflow (Mint Code Changes)
```bash
# Terminal 1: Keep regtest running
just regtest

# Terminal 2: Make changes to mint code, then
just restart-mints           # Recompiles and restarts both mints
just mint-info              # Test the changes
just mint-test              # Run integration tests
```

That's it! The environment provides a full Lightning Network with CDK mints for testing and development.
