# CDK-CLI

Cashu CLI wallet built on CDK.

## Installation

Build with OHTTP support (default):
```bash
cargo build --release
```

Build without OHTTP support:
```bash
cargo build --release --no-default-features
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

The CLI supports OHTTP (Oblivious HTTP) for enhanced privacy when communicating with mints. OHTTP is enabled by default and automatically used when:

1. The mint supports OHTTP (advertised in mint info)
2. You provide an OHTTP relay URL using `--ohttp-relay`

#### OHTTP Usage

To use OHTTP, simply provide a relay URL. The CLI will automatically detect if the mint supports OHTTP and configure the connection appropriately:

```bash
# Use OHTTP relay - CLI auto-detects OHTTP support and gateway URL from mint
cdk-cli --ohttp-relay https://relay.example.com balance
cdk-cli --ohttp-relay https://relay.example.com send --amount 100
cdk-cli --ohttp-relay https://relay.example.com receive <TOKEN>
```

#### How OHTTP Works in CDK-CLI

1. **Automatic Detection**: When `--ohttp-relay` is provided, the CLI checks if the mint supports OHTTP
2. **Gateway Discovery**: The gateway URL is automatically discovered from the mint's OHTTP configuration, or falls back to using the mint URL directly
3. **Transport Setup**: An OHTTP transport layer is created with the mint URL, relay, and gateway
4. **Privacy Protection**: Requests are routed through the relay, providing privacy from both the relay and the gateway

#### OHTTP Arguments

- `--ohttp-relay <URL>`: OHTTP relay URL for routing requests through a privacy relay

#### Example OHTTP Usage

```bash
# Standard OHTTP usage with relay
cdk-cli --ohttp-relay https://ohttp-relay.example.com balance

# All commands work with OHTTP
cdk-cli --ohttp-relay https://relay.example.com send --amount 100
cdk-cli --ohttp-relay https://relay.example.com receive <TOKEN>
cdk-cli --ohttp-relay https://relay.example.com mint --amount 1000
```

#### OHTTP vs Regular Proxy

OHTTP provides significantly better privacy compared to regular HTTP proxies:

- **Regular proxy:** `cdk-cli --proxy https://proxy.example.com balance`
  - Proxy can see all request content and your IP
- **OHTTP relay:** `cdk-cli --ohttp-relay https://relay.example.com balance`
  - Relay cannot see request content (cryptographically protected)
  - Gateway cannot see your real IP address
  - Provides true metadata protection

#### Important Notes

- OHTTP requires the mint to explicitly support it
- If you specify `--ohttp-relay` but the mint doesn't support OHTTP, you'll see a warning and fall back to regular HTTP
- Gateway URL is automatically determined from the mint's OHTTP configuration
- When OHTTP is used, WebSocket subscriptions are automatically disabled in favor of HTTP polling

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
