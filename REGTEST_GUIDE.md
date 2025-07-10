# CDK Regtest Environment Guide

A comprehensive guide for setting up and using the CDK regtest environment for development and testing.

## Quick Start

### Start the Environment
```bash
# Start regtest with SQLite database (default)
just regtest

# Or with REDB database
just regtest redb
```

The script will:
1. Check for `mprocs` and offer to install it if missing
2. Build necessary binaries
3. Set up Bitcoin regtest + 4 Lightning nodes + 2 CDK mints
4. Launch `mprocs` TUI showing all component logs
5. Both mints start automatically

### Stop the Environment
Press `q` in mprocs or `Ctrl+C` in the terminal. Everything cleans up automatically.

## Network Components

When running, you get a complete Lightning Network environment:

### Bitcoin Network
- **Bitcoin RPC**: `127.0.0.1:18443` (user: `testuser`, pass: `testpass`)

### Lightning Nodes
- **CLN Node 1**: `$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc`
- **CLN Node 2**: `$CDK_ITESTS_DIR/cln/two/regtest/lightning-rpc`
- **LND Node 1**: `https://localhost:10009`
- **LND Node 2**: `https://localhost:10010`

### CDK Mints
- **CLN Mint**: `http://127.0.0.1:8085` (connected to CLN node 1)
- **LND Mint**: `http://127.0.0.1:8087` (connected to LND node 2)

### Environment Variables
Available in all terminals automatically:
- `CDK_TEST_MINT_URL`: CLN mint URL
- `CDK_TEST_MINT_URL_2`: LND mint URL
- `CDK_ITESTS_DIR`: Temporary directory with all data

## Using the Environment

All commands work from any terminal - they automatically find the running environment.

### Lightning Node Operations
```bash
# Get node information
just ln-cln1 getinfo
just ln-cln2 getinfo
just ln-lnd1 getinfo
just ln-lnd2 getinfo

# Create and pay invoices
just ln-cln1 invoice 1000 label "Test payment"
just ln-lnd1 payinvoice <bolt11>

# Check balances and channels
just ln-cln1 listfunds
just ln-lnd1 listchannels
```

### Bitcoin Operations
```bash
just btc getblockchaininfo    # Blockchain status
just btc getbalance          # Wallet balance
just btc-mine 5              # Mine 5 blocks
```

### CDK Mint Operations
```bash
just mint-info        # Show both mints' info
just mint-test        # Run integration tests
just restart-mints    # Recompile and restart mints
just regtest-status   # Check all components
just regtest-logs     # Show recent logs
```

## mprocs TUI Interface

The `mprocs` interface shows all component logs in real-time:

### Controls
- **Arrow keys**: Navigate between processes
- **Enter**: Focus on a process to see its output
- **Tab**: Switch between process list and output view
- **s**: Start a process (if stopped)
- **k**: Kill a process
- **r**: Restart a process
- **PageUp/PageDown**: Scroll through logs
- **?**: Show help
- **q**: Quit and stop environment

### Process List
- `cln-mint`: CDK mint connected to CLN (auto-started)
- `lnd-mint`: CDK mint connected to LND (auto-started)
- `bitcoind`: Bitcoin regtest node logs
- `cln-one`: CLN node 1 logs
- `cln-two`: CLN node 2 logs
- `lnd-one`: LND node 1 logs
- `lnd-two`: LND node 2 logs

## Development Workflows

### Testing Lightning Payment Flow
```bash
# Terminal 1: Start environment
just regtest

# Terminal 2: Create invoice and pay
just ln-cln1 invoice 1000 test "Test payment"
just ln-lnd1 payinvoice <bolt11_from_above>
just ln-cln1 listinvoices
just ln-lnd1 listpayments
```

### Developing Mint Code
```bash
# Terminal 1: Keep regtest running
just regtest

# Terminal 2: After making code changes
just restart-mints    # Recompiles and restarts both mints
just mint-info       # Test the changes
just mint-test       # Run integration tests
```

### Using CDK CLI Tools
```bash
# Terminal 1: Start environment
just regtest

# Terminal 2: Use environment variables
cargo run --bin cdk-cli -- --mint-url $CDK_TEST_MINT_URL mint-info
cargo run --bin cdk-cli -- --mint-url $CDK_TEST_MINT_URL_2 mint-info
```

### Direct API Testing
```bash
# Query mint info directly
curl $CDK_TEST_MINT_URL/v1/info | jq
curl $CDK_TEST_MINT_URL/v1/keysets | jq

# Test both mints
curl http://127.0.0.1:8085/v1/info | jq
curl http://127.0.0.1:8087/v1/info | jq
```

## File Structure

All components run in a temporary directory:

```
$CDK_ITESTS_DIR/
├── bitcoin/              # Bitcoin regtest data
├── cln/
│   ├── one/             # CLN node 1 data
│   └── two/             # CLN node 2 data
├── lnd/
│   ├── one/             # LND node 1 data
│   │   ├── tls.cert
│   │   └── data/chain/bitcoin/regtest/admin.macaroon
│   └── two/             # LND node 2 data
├── cln_mint/            # CLN mint working directory
├── lnd_mint/            # LND mint working directory
├── start_cln_mint.sh    # Mint startup scripts
├── start_lnd_mint.sh
└── mprocs.yaml         # mprocs configuration
```

## Installation Requirements

### mprocs (TUI Interface)
If not installed, the script will offer to install it:
```bash
# Automatic installation during regtest setup
just regtest

# Manual installation
cargo install mprocs

# Or via package manager
# Ubuntu/Debian: apt install mprocs
# macOS: brew install mprocs
```

### System Dependencies
Managed automatically via Nix development shell:
- Bitcoin Core
- Core Lightning (CLN)
- LND (Lightning Network Daemon)
- Rust toolchain

## Advanced Usage

### Manual mprocs Launch
```bash
# If you need to restart just the mprocs interface
source /tmp/cdk_regtest_env
just regtest-logs
```

### Environment State
The environment creates a state file at `/tmp/cdk_regtest_env` that:
- Shares environment variables between terminals
- Allows `just` commands to work from anywhere
- Automatically cleaned up when environment stops

### Process Management
From within mprocs:
- Restart individual mints after code changes
- Monitor specific component logs
- Start/stop services for testing scenarios

## Troubleshooting

### Environment Not Starting
- Check that ports are available: 8085, 8087, 18443, 19846, 19847, 10009, 10010
- Ensure the Nix development shell is active: `nix develop`
- Check individual component logs in mprocs

### Helper Commands Not Working
- Ensure the regtest environment is running
- Check that `/tmp/cdk_regtest_env` file exists
- Verify environment variables are set: `echo $CDK_TEST_MINT_URL`

### Connection Issues
- Use `just regtest-status` to check component health
- Check mint logs with `just regtest-logs`
- Verify Lightning node status with `just ln-cln1 getinfo`

### mprocs Issues
- If mprocs crashes, processes continue running
- Use `Ctrl+C` in the original terminal to clean up
- Restart with `just regtest-logs`

## Common Error Solutions

### "Port already in use"
```bash
# Find and kill processes using ports
sudo lsof -ti:8085 | xargs kill -9
sudo lsof -ti:8087 | xargs kill -9
```

### "Environment not found"
```bash
# Clean up and restart
rm -f /tmp/cdk_regtest_env
just regtest
```

### "Binary not found"
```bash
# Rebuild binaries
just build
just regtest
```

This environment provides everything needed for CDK development and testing in a single, easy-to-use interface! 🎉
