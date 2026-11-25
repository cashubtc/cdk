# CDK Payment Processor

The cdk-payment-processor is a Rust crate that provides both a binary and a library for handling payments to and from a cdk mint. 

## Overview

### Library Components
- **Payment Processor Server**: Handles interaction with payment processor backend implementations
- **Client**: Used by mintd to query the server for payment information
- **Backend Implementations**: Supports CLN, LND, BDK with Bitcoin Core, and a fake wallet (for testing)

### Features
- Modular backend system supporting multiple payment backend implementations
- Extensible design allowing for custom backend implementations

## Building from Source

### Prerequisites
1. Install Nix package manager
2. Enter development environment (use `.#regtest` for full stack including CLN/LND):
```sh
nix develop
# or for full stack:
nix develop .#regtest
```

### Configuration

The server requires different environment variables depending on your chosen payment backend.

#### Core Settings
```sh
# Choose backend: CLN, LND, BDK, or FAKEWALLET
export CDK_PAYMENT_PROCESSOR_BACKEND="CLN"

# Server configuration
export CDK_PAYMENT_PROCESSOR_LISTEN_HOST="127.0.0.1"
export CDK_PAYMENT_PROCESSOR_LISTEN_PORT="8090"
```

When mintd connects with a TLS directory, the directory must contain
`ca.pem`, `client.pem`, and `client.key`. The CA certificate verifies the
payment processor, and the client certificate and private key authenticate
mintd to an mTLS-enabled processor. Configured TLS does not fall back to an
unauthenticated client connection when either client identity file is missing.

Mintd can explicitly opt into a plaintext connection with
`[grpc_processor].allow_insecure = true`, including for a non-loopback address.
This is unsafe on an untrusted network because the connection has no transport
authentication or encryption and can be intercepted or modified. Use plaintext
only on a trusted, isolated network or through a separately authenticated and
encrypted tunnel; prefer mTLS even for internal deployments.

#### Backend-Specific Configuration

##### Core Lightning (CLN)
```sh
# Path to CLN RPC socket
export CDK_PAYMENT_PROCESSOR_CLN_RPC_PATH="/path/to/lightning-rpc"
```

##### Lightning Network Daemon (LND)
```sh
# LND connection details
export CDK_PAYMENT_PROCESSOR_LND_ADDRESS="localhost:10009"
export CDK_PAYMENT_PROCESSOR_LND_CERT_FILE="/path/to/tls.cert"
export CDK_PAYMENT_PROCESSOR_LND_MACAROON_FILE="/path/to/macaroon"
```

##### On-chain Bitcoin (BDK)

To serve on-chain payments from multiple mintd replicas, run one BDK payment
processor with a persistent volume and point every mintd replica at it using
`[grpc_processor]` and a `grpcprocessor` payment backend for `sat`. The BDK
process owns address allocation, chain synchronization, and withdrawal batching
for that wallet. Use a deployment strategy that stops the old BDK process
before starting its replacement.

| Environment variable | Value |
| --- | --- |
| `CDK_PAYMENT_PROCESSOR_BACKEND` | `BDK` |
| `CDK_PAYMENT_PROCESSOR_BDK_WORK_DIR` | Persistent directory for the BDK wallet and withdrawal intent database |
| `CDK_PAYMENT_PROCESSOR_BDK_MNEMONIC` | BIP39 wallet mnemonic, supplied as a secret |
| `CDK_PAYMENT_PROCESSOR_BDK_NETWORK` | Bitcoin network, such as `bitcoin` or `regtest` |
| `CDK_PAYMENT_PROCESSOR_BDK_BITCOIN_RPC_HOST` | Bitcoin Core RPC host |
| `CDK_PAYMENT_PROCESSOR_BDK_BITCOIN_RPC_PORT` | Bitcoin Core RPC port |
| `CDK_PAYMENT_PROCESSOR_BDK_BITCOIN_RPC_USER` | Bitcoin Core RPC username |
| `CDK_PAYMENT_PROCESSOR_BDK_BITCOIN_RPC_PASSWORD` | Bitcoin Core RPC password, supplied as a secret |
| `CDK_PAYMENT_PROCESSOR_BDK_INIT_MODE` | Set `new` for initial bootstrap. The default `existing` checks the persisted wallet identity and requires the intent database. |
| `CDK_PAYMENT_PROCESSOR_BDK_NUM_CONFS` | Required on-chain confirmations, default `6` |
| `CDK_PAYMENT_PROCESSOR_BDK_SYNC_INTERVAL_SECS` | Chain sync interval, default `30` |
| `CDK_PAYMENT_PROCESSOR_BDK_POLL_INTERVAL_SECS` | Withdrawal batch poll interval, default `30` |

### Building and Running

Build and run the binary with your chosen backend:

```sh
# For CLN backend
cargo run --bin cdk-payment-processor --no-default-features --features cln

# For LND backend
cargo run --bin cdk-payment-processor --no-default-features --features lnd

# For BDK with Bitcoin Core
cargo run --bin cdk-payment-processor --no-default-features --features bdk

# For fake wallet (testing only)
cargo run --bin cdk-payment-processor --no-default-features --features fake
```

## Development

To implement a new backend:
1. Create a new module implementing the payment processor traits
2. Add appropriate feature flags
3. Update the binary to support the new backend

For library usage examples and API documentation, refer to the crate documentation.
