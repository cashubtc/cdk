# CDK CLI

[![crates.io](https://img.shields.io/crates/v/cdk-cli.svg)](https://crates.io/crates/cdk-cli)
[![Documentation](https://docs.rs/cdk-cli/badge.svg)](https://docs.rs/cdk-cli)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

> **Warning**
> This project is in early development, it does however work with real sats! Always use amounts you don't mind losing.

A command-line Cashu wallet implementation built with the Cashu Development Kit (CDK). This tool allows you to interact with Cashu mints from the terminal, performing operations like minting, melting, and transferring ecash tokens.

## Features

- **Multiple Mint Support**: Connect to and manage multiple Cashu mints simultaneously
- **Token Operations**: Mint, melt, send, and receive Cashu tokens
- **Lightning Integration**: Pay Lightning invoices (BOLT11, BOLT12, BIP353) and receive payments
- **Payment Requests**: Create and pay payment requests with various conditions (P2PK, HTLC)
- **Token Transfer**: Transfer tokens between different mints
- **Multi-Currency Support**: Support for different currency units (sat, usd, eur, etc.)
- **Database Options**: SQLite or Redb backend with optional encryption (SQLCipher)
- **Tor Support**: Built-in Tor transport support (when compiled with feature)
- **Secure Storage**: Local storage of tokens, mint configurations, and seed

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

### Build with Optional Features
```bash
# With Tor support
cargo build --bin cdk-cli --release --features tor

# With SQLCipher encryption
cargo build --bin cdk-cli --release --features sqlcipher

# With Redb database
cargo build --bin cdk-cli --release --features redb
```

## Quick Start

### 1. Check Your Balance
```bash
# View your current balance across all mints
cdk-cli balance
```

### 2. Mint Tokens
```bash
# Create and mint tokens from a mint (amount in sats)
cdk-cli mint http://127.0.0.1:8085 100

# Or with a description
cdk-cli mint http://127.0.0.1:8085 100 "My first mint"

# The command will display a Lightning invoice to pay
# After payment, tokens are automatically minted
```

### 3. Send Tokens
```bash
# Send tokens (you'll be prompted for amount and mint selection interactively)
cdk-cli send

# Or specify options directly
cdk-cli send --mint-url http://127.0.0.1:8085 --memo "Payment for coffee"
```

### 4. Receive Tokens
```bash
# Receive a token from someone else
cdk-cli receive <cashu_token>

# Receive from untrusted mint with transfer to trusted mint
cdk-cli receive <cashu_token> --allow-untrusted --transfer-to http://127.0.0.1:8085
```

## Global Options

The CLI supports several global options that apply to all commands:

```bash
# Use a specific database engine
cdk-cli --engine sqlite balance
cdk-cli --engine redb balance

# Set a custom work directory
cdk-cli --work-dir ~/my-wallet balance

# Set logging level
cdk-cli --log-level info balance

# Use a specific currency unit
cdk-cli --unit usd balance

# Use NIP-98 Wallet Signing Proxy
cdk-cli --proxy https://proxy.example.com balance

# Disable Tor (when built with Tor feature, it's on by default)
cdk-cli --tor off balance
```

## Commands Reference

### Balance Operations

```bash
# Check balance across all mints
cdk-cli balance
```

### Minting Tokens

```bash
# Mint tokens with a Lightning invoice
cdk-cli mint <MINT_URL> <AMOUNT>

# With options
cdk-cli mint http://127.0.0.1:8085 1000 \
  --method bolt11

# Using an existing quote
cdk-cli mint http://127.0.0.1:8085 --quote-id <quote_id>

# Claim pending mint quotes that have been paid
cdk-cli mint-pending
```

### Sending & Receiving Tokens

```bash
# Send tokens (interactive)
cdk-cli send

# Send with specific options
cdk-cli send \
  --memo "Coffee payment" \
  --mint-url http://127.0.0.1:8085 \
  --include-fee \
  --offline

# Send with P2PK lock
cdk-cli send --pubkey <public_key> --required-sigs 1

# Send with HTLC (Hash Time Locked Contract)
cdk-cli send --hash <hash> --locktime <unix_timestamp>

# Send as V3 token
cdk-cli send --v3

# Send with automatic transfer from other mints if needed
cdk-cli send --allow-transfer --max-transfer-amount 1000

# Receive tokens
cdk-cli receive <cashu_token>

# Receive with signing key (for P2PK)
cdk-cli receive <cashu_token> --signing-key <private_key>

# Receive with HTLC preimage
cdk-cli receive <cashu_token> --preimage <preimage>

# Receive via Nostr
cdk-cli receive --nostr-key <nostr_key> --relay wss://relay.example.com
```

### Lightning Payments

```bash
# Pay a Lightning invoice (interactive - will prompt for invoice)
cdk-cli melt

# Specify mint and payment method
cdk-cli melt --mint-url http://127.0.0.1:8085 --method bolt11

# Pay BOLT12 offer
cdk-cli melt --method bolt12

# Pay BIP353 address
cdk-cli melt --method bip353

# Multi-path payment
cdk-cli melt --mpp
```

### Payment Requests

```bash
# Create a payment request (interactive via Nostr)
cdk-cli create-request

# Create with specific amount
cdk-cli create-request --amount 1000 "Invoice for services"

# Create with P2PK condition
cdk-cli create-request --amount 500 \
  --pubkey <pubkey1> \
  --pubkey <pubkey2> \
  --num-sigs 2

# Create with HTLC
cdk-cli create-request --amount 1000 --hash <hash>
# Or use preimage instead
cdk-cli create-request --amount 1000 --preimage <preimage>

# Create with HTTP transport
cdk-cli create-request --amount 1000 \
  --transport http \
  --http-url https://myserver.com/payment

# Create without transport (just print the request)
cdk-cli create-request --amount 1000 --transport none

# Pay a payment request
cdk-cli pay-request <payment_request>

# Decode a payment request
cdk-cli decode-request <payment_request>
```

### Token Transfer Between Mints

```bash
# Transfer tokens between mints (interactive)
cdk-cli transfer

# Transfer specific amount
cdk-cli transfer \
  --source-mint http://mint1.example.com \
  --target-mint http://mint2.example.com \
  --amount 1000

# Transfer full balance from one mint to another
cdk-cli transfer \
  --source-mint http://mint1.example.com \
  --target-mint http://mint2.example.com \
  --full-balance
```

### Mint Information & Management

```bash
# Get mint information
cdk-cli mint-info <MINT_URL>

# Update mint URL (if mint has migrated)
cdk-cli update-mint-url <OLD_URL> <NEW_URL>

# List proofs from mint
cdk-cli list-mint-proofs
```

### Token & Proof Management

```bash
# Decode a Cashu token
cdk-cli decode-token <cashu_token>

# Check pending proofs and reclaim if no longer pending
cdk-cli check-pending

# Burn spent tokens (cleanup)
cdk-cli burn

# Restore proofs from seed for a specific mint
cdk-cli restore <MINT_URL>
```

### Advanced Features

#### Blind Authentication (NUT-14)

```bash
# Mint blind authentication proofs
cdk-cli mint-blind-auth <MINT_URL> --amount <AMOUNT>
```

#### CAT (Cashu Authentication Tokens)

```bash
# Login with username/password
cdk-cli cat-login --username <username> --password <password>

# Login with device code flow (OAuth-style)
cdk-cli cat-device-login
```

## Configuration

### Storage Location

The CLI stores its configuration and wallet data in:
- **Linux/macOS**: `~/.cdk-cli/`
- **Windows**: `%USERPROFILE%\.cdk-cli\`

You can override this with the `--work-dir` option.

### Database Options

The CLI supports multiple database backends:

#### SQLite (default)
```bash
cdk-cli --engine sqlite balance
```

#### SQLCipher (encrypted SQLite)
```bash
# Requires building with --features sqlcipher
cdk-cli --engine sqlite --password mypassword balance
```

#### Redb
```bash
# Requires building with --features redb
cdk-cli --engine redb balance
```

### Seed Management

The wallet seed is automatically generated and stored in `<work-dir>/seed` on first run. This seed is used to derive all keys and can be used to restore your wallet.

**Important**: Back up your seed file securely. Anyone with access to the seed can spend your tokens.

## Examples

### Complete Workflow Example

```bash
# 1. Start a test mint (in another terminal)
cdk-mintd

# 2. Mint some tokens
cdk-cli mint http://127.0.0.1:8085 1000 "Initial mint"
# Pay the displayed Lightning invoice

# 3. Check balance
cdk-cli balance

# 4. Send some tokens
cdk-cli send
# Follow interactive prompts

# 5. The recipient can receive with:
cdk-cli receive <cashu_token_string>

# 6. Pay a Lightning invoice
cdk-cli melt
# Follow prompts to enter invoice
```

### Multi-Mint Setup

```bash
# Mint from multiple mints
cdk-cli mint http://mint1.example.com 5000
cdk-cli mint http://mint2.example.com 3000

# Check balance (shows breakdown by mint)
cdk-cli balance

# Transfer between mints
cdk-cli transfer \
  --source-mint http://mint1.example.com \
  --target-mint http://mint2.example.com \
  --amount 2000
```

### Payment Request Workflow

```bash
# Recipient creates a payment request
cdk-cli create-request --amount 1000 "Payment for services"
# Copy the payment request string

# Sender pays the request
cdk-cli pay-request <payment_request_string>
```

### P2PK (Pay to Public Key) Usage

```bash
# Send tokens locked to a public key
cdk-cli send --pubkey <recipient_pubkey> --required-sigs 1

# Recipient receives with their private key
cdk-cli receive <cashu_token> --signing-key <private_key>
```

### HTLC (Hash Time Locked Contract) Usage

```bash
# Create a preimage and hash (externally)
# hash = SHA256(preimage)

# Send with HTLC
cdk-cli send --hash <hash> --locktime 1700000000

# Recipient receives with preimage
cdk-cli receive <cashu_token> --preimage <preimage>
```

## Help and Documentation

```bash
# General help
cdk-cli --help

# Help for specific commands
cdk-cli mint --help
cdk-cli send --help
cdk-cli receive --help
cdk-cli create-request --help
```

## Troubleshooting

### Pending Tokens
If you have pending tokens (sent but not received, or mint quotes paid but not claimed):

```bash
# Check and reclaim pending proofs
cdk-cli check-pending

# Claim paid mint quotes
cdk-cli mint-pending
```

### Cleaning Up
```bash
# Remove spent tokens from database
cdk-cli burn
```

### Restore from Seed
```bash
# Restore proofs from a specific mint
cdk-cli restore <MINT_URL>
```

## License

Code is under the [MIT License](../../LICENSE)

## Contribution

All contributions are welcome.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, shall be licensed as above, without any additional terms or conditions.
