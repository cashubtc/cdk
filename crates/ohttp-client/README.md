# OHTTP Client

A command-line tool for sending arbitrary data through OHTTP (Oblivious HTTP) gateways or relays, providing privacy-preserving HTTP communications by encrypting requests and responses.

## Overview

The OHTTP Client implements the [Oblivious HTTP specification](https://ietf-wg-ohai.github.io/oblivious-http/draft-ietf-ohai-ohttp.html) to enable private HTTP communications. It can work with:

- **OHTTP Gateways**: Direct connection to gateways that decrypt and forward requests to backend services
- **OHTTP Relays**: Connection through relays that forward encrypted requests to gateways without seeing content

## Features

- ✅ **Full OHTTP Protocol**: Implements HPKE encryption with proper `message/ohttp-req` and `message/ohttp-res` media types
- ✅ **Gateway & Relay Support**: Can connect directly to gateways or through privacy-preserving relays
- ✅ **Flexible Data Input**: Send data via command line, files, or JSON
- ✅ **Custom Headers**: Add arbitrary HTTP headers to requests
- ✅ **Multiple Methods**: Support for GET, POST, and other HTTP methods
- ✅ **Debug Logging**: Comprehensive logging for troubleshooting
- ✅ **Key Management**: Automatic OHTTP key fetching and validation

## Installation

### From Source

```bash
# Clone the repository
git clone <repository-url>
cd ohttp-mono/ohttp-client

# Build the client
cargo build --release

# Install globally (optional)
cargo install --path .
```

### Binary Usage

```bash
# Run from target directory
./target/release/ohttp-client --help

# Or if installed globally
ohttp-client --help
```

## Quick Start

### Basic Gateway Usage

```bash
# Send a simple request through a gateway
ohttp-client --gateway-url http://localhost:8080 send --data "Hello, OHTTP!"

# Send JSON data
ohttp-client --gateway-url http://localhost:8080 send \
  --json '{"message": "Hello", "timestamp": "2024-01-01"}'

# Send data from file
echo "Important data" > data.txt
ohttp-client --gateway-url http://localhost:8080 send --file data.txt
```

### Basic Relay Usage

```bash
# Send through a relay (relay forwards to its configured gateway)
ohttp-client --relay-url http://relay.example.com:3000 send --data "Hello via relay!"

# Send through relay to specific gateway
ohttp-client --relay-url http://relay.example.com:3000 \
  --relay-gateway-url http://gateway.example.com:8080 \
  send --data "Hello to specific gateway!"
```

## Usage

### Connection Options

| Option | Environment Variable | Description | Required |
|--------|---------------------|-------------|----------|
| `--gateway-url` | `OHTTP_GATEWAY_URL` | Direct gateway URL | Either gateway or relay |
| `--relay-url` | `OHTTP_RELAY_URL` | Relay URL (forwards to gateway) | Either gateway or relay |
| `--relay-gateway-url` | `OHTTP_RELAY_GATEWAY_URL` | Override relay's default gateway | No |
| `--ohttp-keys` | N/A | Path to OHTTP keys file | No (auto-fetched) |
| `--header` | N/A | Custom headers (format: `Header: Value`) | No |

### Commands

#### `send` - Send Data Through OHTTP

Send arbitrary data to the backend service through the OHTTP gateway/relay.

```bash
ohttp-client [CONNECTION_OPTIONS] send [SEND_OPTIONS]
```

**Send Options:**

| Option | Description | Example |
|--------|-------------|---------|
| `--method` | HTTP method (default: POST) | `--method GET` |
| `--data` | Data string to send | `--data "Hello World"` |
| `--file` | Read data from file | `--file request.json` |
| `--json` | Send JSON (sets Content-Type) | `--json '{"key": "value"}'` |
| `--path` | Request path (default: /) | `--path /api/v1/endpoint` |

**Examples:**

```bash
# POST request with text data
ohttp-client --gateway-url http://localhost:8080 send \
  --data "Plain text message"

# GET request with custom path
ohttp-client --gateway-url http://localhost:8080 send \
  --method GET \
  --path "/api/users/123"

# POST JSON with custom headers
ohttp-client --gateway-url http://localhost:8080 send \
  --json '{"user_id": 123, "action": "login"}' \
  --header "Authorization: Bearer token123" \
  --header "X-API-Version: v2"

# Send file content
ohttp-client --gateway-url http://localhost:8080 send \
  --file payload.bin \
  --header "Content-Type: application/octet-stream"
```

#### `get-keys` - Fetch OHTTP Keys

Retrieve and display the OHTTP key configuration from the gateway or relay.

```bash
ohttp-client --gateway-url http://localhost:8080 get-keys
```

#### `health` - Health Check

Send a health check request to verify the gateway/relay is operational.

```bash
ohttp-client --gateway-url http://localhost:8080 health
```

#### `info` - Show Configuration

Display current configuration and available endpoints.

```bash
ohttp-client --gateway-url http://localhost:8080 info
```

## Configuration

### Environment Variables

```bash
# Gateway connection
export OHTTP_GATEWAY_URL="http://localhost:8080"

# Or relay connection
export OHTTP_RELAY_URL="http://relay.example.com:3000"
export OHTTP_RELAY_GATEWAY_URL="http://gateway.example.com:8080"

# Run without specifying URLs
ohttp-client send --data "Hello!"
```

### OHTTP Keys

The client automatically fetches OHTTP keys from:
- **Gateway**: `{gateway-url}/ohttp-keys`
- **Relay**: `{relay-url}/ohttp-keys` (relay forwards to its configured gateway)

You can also provide a local keys file:

```bash
ohttp-client --ohttp-keys ./my-keys.json send --data "Hello"
```

## Advanced Usage

### Custom Headers

```bash
# Multiple headers
ohttp-client --gateway-url http://localhost:8080 send \
  --data "Authenticated request" \
  --header "Authorization: Bearer abc123" \
  --header "X-API-Key: xyz789" \
  --header "User-Agent: MyApp/1.0"
```

### Different HTTP Methods

```bash
# PUT request
ohttp-client --gateway-url http://localhost:8080 send \
  --method PUT \
  --path "/api/resource/123" \
  --json '{"status": "updated"}'

# DELETE request
ohttp-client --gateway-url http://localhost:8080 send \
  --method DELETE \
  --path "/api/resource/123"

# HEAD request
ohttp-client --gateway-url http://localhost:8080 send \
  --method HEAD \
  --path "/api/status"
```

### Working with Binary Data

```bash
# Send binary file
ohttp-client --gateway-url http://localhost:8080 send \
  --file image.jpg \
  --header "Content-Type: image/jpeg" \
  --path "/api/upload"
```

### Using Through Relay

```bash
# Basic relay usage (uses relay's default gateway)
ohttp-client --relay-url http://privacy-relay.com:3000 send \
  --data "Private message"

# Relay with specific target gateway
ohttp-client --relay-url http://privacy-relay.com:3000 \
  --relay-gateway-url http://my-gateway.com:8080 \
  send --data "Message to specific gateway"
```

## Output Format

The client provides detailed output including:

```
✅ Status: 200 OK
⏱️  Response Time: 245ms
📊 Content-Length: 156 bytes
📋 Content-Type: application/json

📥 Response Headers:
   server: nginx/1.18.0
   content-type: application/json
   content-length: 156

📨 Response Body:
{
  "message": "Hello, World!",
  "timestamp": "2024-01-01T12:00:00Z"
}
```

## Debug Mode

Enable detailed logging for troubleshooting:

```bash
# Debug level logging
RUST_LOG=debug ohttp-client --gateway-url http://localhost:8080 send --data "test"

# Trace level (very verbose)
RUST_LOG=trace ohttp-client --gateway-url http://localhost:8080 send --data "test"

# Module-specific logging
RUST_LOG=ohttp_client=debug ohttp-client send --data "test"
```

Debug output includes:
- OHTTP key fetching and validation
- Request/response encryption/decryption
- HTTP headers and status codes
- Network timing information
- Error details and stack traces

## Protocol Details

### OHTTP Flow

1. **Key Fetching**: Client fetches OHTTP keys from gateway/relay
2. **Request Encryption**: Client encrypts HTTP request using HPKE
3. **Encapsulation**: Request wrapped with `message/ohttp-req` content type
4. **Forwarding**: Encrypted request sent to `/.well-known/ohttp-gateway`
5. **Gateway Processing**: Gateway decrypts and forwards to backend
6. **Response Encryption**: Gateway encrypts backend response
7. **Decapsulation**: Client receives `message/ohttp-res` and decrypts

### Encryption

- **Algorithm**: HPKE (Hybrid Public Key Encryption)
- **Key Exchange**: secp256k1
- **Symmetric Encryption**: ChaCha20Poly1305
- **Message Format**: Binary HTTP (BHTTP) over OHTTP

## Error Handling

Common errors and solutions:

### Connection Errors

```
Error: Connection refused
```
**Solution**: Verify the gateway/relay is running and the URL is correct.

```
Error: Invalid gateway URL
```
**Solution**: Ensure URL includes protocol (http:// or https://).

### OHTTP Errors

```
Error: Failed to fetch OHTTP keys
```
**Solution**: Check that the gateway/relay supports OHTTP and is properly configured.

```
Error: OHTTP encapsulation failed
```
**Solution**: Verify the OHTTP keys are valid and the gateway supports the encryption algorithms.

### Request Errors

```
Error: Invalid header format
```
**Solution**: Headers must be in format `Header: Value`.

```
Error: File not found
```
**Solution**: Verify the file path is correct and the file is readable.

## Testing

### Unit Tests

```bash
cd ohttp-client
cargo test
```

### Integration Testing

```bash
# Start a test gateway
cd ../ohttp-gateway
cargo run -- --port 8080 --backend-url http://httpbin.org

# Test client in another terminal
cd ../ohttp-client
cargo run -- --gateway-url http://localhost:8080 send --data "test"
```

### Example Test Scenarios

```bash
# Test JSON API interaction
ohttp-client --gateway-url http://localhost:8080 send \
  --json '{"test": true}' \
  --path "/post"

# Test file upload
echo "test content" > test.txt
ohttp-client --gateway-url http://localhost:8080 send \
  --file test.txt \
  --path "/upload"

# Test headers and authentication
ohttp-client --gateway-url http://localhost:8080 send \
  --data "authenticated request" \
  --header "Authorization: Bearer test-token" \
  --path "/protected"
```

## Security Considerations

- **IP Privacy**: Use relays to hide your IP address from gateways
- **Content Privacy**: All request/response content is encrypted end-to-end
- **Metadata**: Some metadata (timing, size) may still be observable
- **Gateway Trust**: Choose trusted gateways as they see decrypted content
- **Relay Trust**: Relays cannot see content but can observe traffic patterns

## Performance

- **Encryption Overhead**: ~100-200ms for HPKE operations
- **Network Overhead**: ~100-500 bytes for OHTTP encapsulation
- **Relay Latency**: Additional round-trip time when using relays

## Troubleshooting

### Verbose Logging

```bash
# Enable all debug output
RUST_LOG=debug ohttp-client [options]

# Focus on specific components
RUST_LOG=ohttp_client::client=trace ohttp-client [options]
```

### Connection Testing

```bash
# Test basic connectivity
ohttp-client --gateway-url http://localhost:8080 health

# Test key fetching
ohttp-client --gateway-url http://localhost:8080 get-keys

# Test with simple data
ohttp-client --gateway-url http://localhost:8080 send --data "ping"
```

### Common Issues

1. **Gateway not responding**: Check if service is running and accessible
2. **OHTTP errors**: Verify gateway supports OHTTP protocol version
3. **Authentication failures**: Check headers and backend authentication requirements
4. **Large requests failing**: Some gateways may have size limits

## Contributing

We welcome contributions! Please see the main repository for:
- Issue reporting guidelines
- Pull request process
- Development setup instructions
- Coding standards

## License

This project is licensed under the MIT License - see the LICENSE file for details.
