# CDK CLI

[![crates.io](https://img.shields.io/crates/v/cdk-cli.svg)](https://crates.io/crates/cdk-cli)
[![Documentation](https://docs.rs/cdk-cli/badge.svg)](https://docs.rs/cdk-cli)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

> **Warning**
> This project is in early development, it does however work with real sats! Always use amounts you don't mind losing.

A command-line Cashu wallet implementation built with the Cashu Development Kit (CDK). This tool allows you to interact with Cashu mints from the terminal, performing operations like minting, melting, and transferring ecash tokens.

## Features

- **Multiple Mint Support**: Connect to and manage multiple Cashu mints
- **Token Operations**: Mint, melt, send, and receive Cashu tokens
- **Wallet Management**: Create and manage multiple wallets
- **Lightning Integration**: Pay Lightning invoices and receive payments
- **Token Storage**: Secure local storage of tokens and mint configurations

## Installation

### Option 1: Download Pre-built Binary
Download the latest release from the [GitHub releases page](https://github.com/cashubtc/cdk/releases).

### Option 2: Build from Source
```bash
git clone https://github.com/cashubtc/cdk.git
cd cdk
cargo build --bin cdk-cli --release
# Binary will be at ./target/release/cdk-cli
```

## Quick Start

### 1. Add a Mint
```bash
# Add a mint (use a real mint URL or start your own with cdk-mintd)
cdk-cli wallet add-mint http://127.0.0.1:8085
```

### 2. Mint Tokens
```bash
# Create a mint quote for 100 sats
cdk-cli wallet mint-quote 100

# Pay the Lightning invoice shown, then mint the tokens
cdk-cli wallet mint <quote_id>
```

### 3. Send Tokens
```bash
# Send 50 sats as a token
cdk-cli wallet send 50
```

### 4. Receive Tokens
```bash
# Receive a token from someone else
cdk-cli wallet receive <cashu_token>
```

### 5. Check Balance
```bash
# View your current balance
cdk-cli wallet balance
```

## Basic Usage

### Wallet Operations
```bash
# List all wallets
cdk-cli wallet list

# Create a new wallet
cdk-cli wallet new --name my-wallet

# Set default wallet
cdk-cli wallet set-default my-wallet

# Show wallet info
cdk-cli wallet info
```

### Mint Management
```bash
# List connected mints
cdk-cli wallet list-mints

# Remove a mint
cdk-cli wallet remove-mint <mint_url>

# Get mint information
cdk-cli wallet mint-info <mint_url>
```

### Payment Operations
```bash
# Pay a Lightning invoice
cdk-cli wallet pay-invoice <lightning_invoice>

# Create melt quote for an invoice
cdk-cli wallet melt-quote <lightning_invoice>

# Execute the melt
cdk-cli wallet melt <quote_id>
```

### Token Management
```bash
# List all tokens
cdk-cli wallet list-tokens

# Check token states
cdk-cli wallet check-tokens

# Restore wallet from seed
cdk-cli wallet restore --seed <seed_words>
```

## Configuration

The CLI stores its configuration and wallet data in:
- **Linux/macOS**: `~/.config/cdk-cli/`
- **Windows**: `%APPDATA%\cdk-cli\`

## Examples

### Complete Workflow Example
```bash
# 1. Start a test mint (in another terminal)
cdk-mintd

# 2. Add the mint
cdk-cli wallet add-mint http://127.0.0.1:8085

# 3. Create a mint quote
cdk-cli wallet mint-quote 1000

# 4. Pay the Lightning invoice (if using real Lightning backend)
# or wait a few seconds if using fake wallet

# 5. Mint the tokens
cdk-cli wallet mint <quote_id>

# 6. Check balance
cdk-cli wallet balance

# 7. Send some tokens
cdk-cli wallet send 100

# 8. The recipient can receive with:
cdk-cli wallet receive <cashu_token_string>
```

### Working with Multiple Wallets
```bash
# Create wallets for different purposes
cdk-cli wallet new --name savings
cdk-cli wallet new --name daily

# Switch between wallets
cdk-cli wallet set-default savings
cdk-cli wallet balance

cdk-cli wallet set-default daily
cdk-cli wallet balance
```

## Help and Documentation

```bash
# General help
cdk-cli --help

# Help for specific commands
cdk-cli wallet --help
cdk-cli wallet mint-quote --help
```

## License

Code is under the [MIT License](../../LICENSE)

## Contribution

All contributions are welcome.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, shall be licensed as above, without any additional terms or conditions.
