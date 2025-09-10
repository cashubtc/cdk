# CDK-CLI

Cashu CLI wallet built on CDK.

## Installation

Build with OHTTP support:
```bash
cargo build --release --features ohttp
```

Build without OHTTP support:
```bash
cargo build --release
```

## Usage

### Basic Commands

Check wallet balance:
```bash
cdk-cli balance
```

Send tokens:
```bash
cdk-cli send --amount 100
```

Receive tokens:
```bash
cdk-cli receive <TOKEN>
```

### OHTTP Support

The CLI supports OHTTP (Oblivious HTTP) for enhanced privacy when communicating with mints. This requires building with the `ohttp` feature flag.

#### Simple OHTTP Usage

For most users, simply add the `--ohttp` flag to enable OHTTP mode. The CLI will use the mint URL as both the mint and gateway:

```bash
# Enable OHTTP mode (mint serves as both mint and gateway)
cdk-cli --ohttp balance
cdk-cli --ohttp send --amount 100
cdk-cli --ohttp receive <TOKEN>
```

#### Advanced OHTTP Configuration

For advanced usage, you can specify custom relay and gateway URLs:

1. **Both relay and gateway specified:**
```bash
cdk-cli --ohttp-relay https://relay.example.com --ohttp-gateway https://gateway.example.com balance
```

2. **Only relay specified (gateway auto-discovery):**
```bash
cdk-cli --ohttp-relay https://relay.example.com balance
```

3. **Only gateway specified (direct gateway connection):**
```bash
cdk-cli --ohttp-gateway https://gateway.example.com balance
```

#### OHTTP Arguments

- `--ohttp`: Enable OHTTP mode (mint serves as both mint and gateway) 
- `--ohttp-relay <URL>`: OHTTP relay URL for proxying requests through a relay server (advanced usage)
- `--ohttp-gateway <URL>`: OHTTP gateway URL (advanced usage, overrides mint URL as gateway)

#### Example OHTTP Usage

```bash
# Simple OHTTP mode - most common usage
cdk-cli --ohttp send --amount 100

# Advanced: Use OHTTP relay with auto-discovery
cdk-cli --ohttp-relay https://ohttp-relay.example.com send --amount 100

# Advanced: Use OHTTP with explicit relay and gateway
cdk-cli --ohttp-relay https://relay.example.com --ohttp-gateway https://gateway.example.com receive <TOKEN>

# Advanced: Use OHTTP gateway directly
cdk-cli --ohttp-gateway https://ohttp-gateway.example.com balance
```

#### OHTTP vs Regular Proxy

OHTTP provides better privacy compared to regular HTTP proxies:

- **Regular proxy:** `cdk-cli --proxy https://proxy.example.com balance`
- **OHTTP relay:** `cdk-cli --ohttp-relay https://relay.example.com balance`

OHTTP uses cryptographic techniques to ensure that the relay cannot see the content of requests while the gateway cannot see the client's identity.

## Building

### Features

- `ohttp`: Enables OHTTP support for enhanced privacy
- `sqlcipher`: Enables SQLCipher support for encrypted databases
- `redb`: Enables redb as an alternative database backend

### Examples

```bash
# Build with all features
cargo build --features "ohttp,sqlcipher,redb"

# Build with just OHTTP
cargo build --features ohttp
```
