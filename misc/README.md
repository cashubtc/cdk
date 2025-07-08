# CDK Interactive Regtest Environment

This directory contains scripts for setting up and interacting with a CDK regtest environment for development and testing.

## Scripts

### 1. `interactive_regtest_mprocs.sh`
Sets up a complete regtest environment with:
- Bitcoin regtest node
- 2 CLN (Core Lightning) nodes with channels
- 2 LND nodes with channels 
- 2 CDK mint instances (one connected to CLN, one to LND)

Unlike `itests.sh`, this script keeps the environment running for interactive use and creates a state file (`/tmp/cdk_regtest_env`) that allows other terminal sessions to find and interact with the environment.

### 2. `regtest_helper.sh`
Helper script providing convenient commands to interact with the running regtest environment. Automatically detects the environment using the state file.

## Quick Start

### Using `just` (Recommended)

1. **Start the regtest environment:**
   ```bash
   just regtest [database_type]
   ```
   - `database_type`: Optional, defaults to "sqlite". Can be "sqlite" or "redb"
   - The script will check for `mprocs` and offer to install it if missing
   - After setup, it will launch `mprocs` showing logs from all nodes and mints
   - Press 'q' in mprocs to quit and stop the environment

2. **In another terminal, interact with Lightning nodes:**
   ```bash
   # Get node information
   just ln-cln1 getinfo
   just ln-lnd1 getinfo
   
   # Mine some blocks
   just btc-mine 5
   
   # Check mint status
   just mint-info
   
   # Start mprocs log viewer in another terminal
   just regtest-mprocs
   
   # See all available commands
   just --list
   ```

### Using Scripts Directly

1. **Start the regtest environment:**
   ```bash
   ./misc/interactive_regtest_mprocs.sh [database_type]
   ```
   - `database_type`: Optional, defaults to "sqlite". Can be "sqlite" or "redb"
   - The script will build necessary binaries and set up the full environment
   - Keep this terminal open - the environment runs until you press Ctrl+C

2. **In another terminal, use the helper script:**
   ```bash
   ./misc/regtest_helper.sh help
   ```

## How It Works

The interactive regtest environment uses a state file (`/tmp/cdk_regtest_env`) to share environment information between terminal sessions:

1. When you run `just regtest` or `./misc/interactive_regtest_mprocs.sh`, it creates the state file with all necessary environment variables
2. When you run Lightning node commands in other terminals (e.g., `just ln-cln1 getinfo`), the helper script automatically sources the state file
3. When the environment shuts down (Ctrl+C), it cleans up the state file automatically

This allows you to use `just` commands from any terminal without needing to export environment variables manually.

## Environment Details

When running, the environment provides:

### Network Endpoints
- **Bitcoin RPC**: `127.0.0.1:18443` (user: `testuser`, pass: `testpass`)
- **CLN Node 1**: Unix socket at `$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc`
- **CLN Node 2**: Unix socket at `$CDK_ITESTS_DIR/cln/two/regtest/lightning-rpc`
- **LND Node 1**: HTTPS on `localhost:10009`
- **LND Node 2**: HTTPS on `localhost:10010`

### CDK Mints
- **CLN Mint**: `http://127.0.0.1:8085` (connected to CLN node 1)
- **LND Mint**: `http://127.0.0.1:8087` (connected to LND node 2)

### Environment Variables
The following variables are exported for easy access:
- `CDK_TEST_MINT_URL`: CLN mint URL
- `CDK_TEST_MINT_URL_2`: LND mint URL  
- `CDK_ITESTS_DIR`: Temporary directory with all data

## Usage Examples

### Using `just` Commands (Recommended)

```bash
# Start the environment
just regtest

# In another terminal:
# Get Lightning node info
just ln-cln1 getinfo
just ln-cln2 getinfo
just ln-lnd1 getinfo
just ln-lnd2 getinfo

# Create and pay invoices
just ln-cln1 invoice 1000 label description
just ln-lnd1 payinvoice <bolt11>

# Bitcoin operations  
just btc getblockchaininfo
just btc-mine 10
just btc getbalance

# CDK operations
just mint-info
just mint-test
just restart-mints    # Stop, recompile, and restart mints
just regtest-status
just regtest-logs     # Show recent logs
just regtest-mprocs   # Start mprocs TUI log viewer
```

### Using Helper Script Directly

```bash
# Lightning Node Operations
./misc/regtest_helper.sh ln-cln1 getinfo
./misc/regtest_helper.sh ln-lnd1 getinfo
./misc/regtest_helper.sh ln-cln1 invoice 1000 label description
./misc/regtest_helper.sh ln-lnd1 payinvoice <bolt11>

# Bitcoin Operations
./misc/regtest_helper.sh btc getblockchaininfo
./misc/regtest_helper.sh btc-mine 10
./misc/regtest_helper.sh btc getbalance

# CDK Mint Operations
./misc/regtest_helper.sh mint-info
./misc/regtest_helper.sh mint-test
./misc/regtest_helper.sh restart-mints
./misc/regtest_helper.sh show-status
```

### Legacy Examples (for reference)

### Direct API Access
```bash
# Query mint info directly
curl http://127.0.0.1:8085/v1/info | jq

# Get mint keysets
curl http://127.0.0.1:8085/v1/keysets | jq
```

### Development Workflow
```bash
# Terminal 1: Start environment
just regtest

# Terminal 2: Development and testing
just ln-cln1 getinfo  # Check CLN status
just mint-info        # Check mint status  
just mint-test        # Run integration tests

# Or use CDK CLI tools directly with the mint URLs
# The environment sets CDK_TEST_MINT_URL and CDK_TEST_MINT_URL_2
cargo run --bin cdk-cli -- --mint-url $CDK_TEST_MINT_URL mint-info
```

## File Locations

All files are stored in a temporary directory (`$CDK_ITESTS_DIR`):
```
$CDK_ITESTS_DIR/
├── bitcoin/           # Bitcoin regtest data
├── cln/
│   ├── one/          # CLN node 1 data
│   └── two/          # CLN node 2 data
├── lnd/
│   ├── one/          # LND node 1 data  
│   │   ├── tls.cert
│   │   └── data/chain/bitcoin/regtest/admin.macaroon
│   └── two/          # LND node 2 data
│       ├── tls.cert
│       └── data/chain/bitcoin/regtest/admin.macaroon
├── cln_mint/         # CLN mint working directory
│   └── mintd.log
└── lnd_mint/         # LND mint working directory
    └── mintd.log
```

## Cleanup

- Press `Ctrl+C` in the terminal running `interactive_regtest_mprocs.sh`
- All processes will be terminated and the temporary directory will be cleaned up automatically
- No manual cleanup is required

## Troubleshooting

### Environment not starting
- Check that ports 8085, 8087, 18443, 19846, 19847, 10009, 10010 are available
- Ensure you have the necessary dependencies (Bitcoin Core, CLN, LND) available
- Check the logs in `$CDK_ITESTS_DIR/cln_mint/mintd.log` and `$CDK_ITESTS_DIR/lnd_mint/mintd.log`

### Helper script not working
- Ensure the regtest environment is running first
- The `CDK_ITESTS_DIR` environment variable must be set (done automatically by `interactive_regtest_mprocs.sh`)

### Connection issues
- Use `./misc/regtest_helper.sh show-status` to check component health
- Check mint logs with `./misc/regtest_helper.sh show-logs`
