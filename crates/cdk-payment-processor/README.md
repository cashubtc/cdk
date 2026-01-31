# CDK Payment Processor

The cdk-payment-processor is a Rust crate that provides both a binary and a library for handling payments to and from a cdk mint. 

## Overview

### Library Components
- **Payment Processor Server**: Handles interaction with payment processor backend implementations
- **Client**: Used by mintd to query the server for payment information
- **Backend Implementations**: Supports CLN, LND, and a fake wallet (for testing)

### Features
- Modular backend system supporting multiple Lightning implementations
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

The server requires different environment variables depending on your chosen Lightning Network backend.

#### Core Settings
```sh
# Choose backend: CLN, LND, or FAKEWALLET
export CDK_PAYMENT_PROCESSOR_LN_BACKEND="CLN"

# Server configuration
export CDK_PAYMENT_PROCESSOR_LISTEN_HOST="127.0.0.1"
export CDK_PAYMENT_PROCESSOR_LISTEN_PORT="8090"
```

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

### Building and Running

Build and run the binary with your chosen backend:

```sh
# For CLN backend
cargo run --bin cdk-payment-processor --no-default-features --features cln

# For LND backend
cargo run --bin cdk-payment-processor --no-default-features --features lnd

# For fake wallet (testing only)
cargo run --bin cdk-payment-processor --no-default-features --features fake
```

## Development

To implement a new backend:
1. Create a new module implementing the payment processor traits
2. Add appropriate feature flags
3. Update the binary to support the new backend

For library usage examples and API documentation, refer to the crate documentation.
