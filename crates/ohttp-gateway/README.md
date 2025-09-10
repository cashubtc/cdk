# OHTTP Gateway

A high-performance OHTTP (Oblivious HTTP) gateway that provides privacy-preserving HTTP proxying by decrypting OHTTP-encapsulated requests and forwarding them to backend services. Built with Axum for async performance and designed for production deployments.

## Overview

The OHTTP Gateway implements the [Oblivious HTTP specification](https://ietf-wg-ohai.github.io/oblivious-http/draft-ietf-ohai-ohttp.html) as a transparent proxy that:

- **Decapsulates OHTTP requests**: Decrypts incoming encrypted HTTP requests
- **Forwards to backends**: Proxies decrypted requests to configured backend services  
- **Encapsulates responses**: Encrypts backend responses for return to clients
- **Zero data storage**: Operates as a stateless proxy with no request logging or storage
- **High performance**: Built on Axum with async/await for concurrent request handling

## Architecture

```
OHTTP Client → [OHTTP Gateway] → Backend Service
                     ↑
           (encrypt/decrypt with HPKE)
```

**Request Flow:**
1. Client sends encrypted request (`message/ohttp-req`) to `/.well-known/ohttp-gateway`
2. Gateway decrypts using private OHTTP keys
3. Gateway forwards plain HTTP request to configured backend
4. Backend processes request and returns response
5. Gateway encrypts response and returns as `message/ohttp-res`

## Features

- ✅ **RFC Compliant**: Full OHTTP specification implementation
- ✅ **Zero Storage**: Stateless operation with no request persistence
- ✅ **High Performance**: Async request handling with Axum
- ✅ **Automatic Key Management**: Generates and manages OHTTP keys
- ✅ **Flexible Backend**: Forward to any HTTP/HTTPS service
- ✅ **Health Monitoring**: Built-in health check endpoints
- ✅ **CORS Support**: Cross-origin resource sharing for web clients
- ✅ **Production Ready**: Comprehensive error handling and logging

## Installation

### From Source

```bash
# Clone the repository
git clone <repository-url>
cd ohttp-mono/ohttp-gateway

# Build the gateway
cargo build --release

# Install globally (optional)
cargo install --path .
```

### Docker (if available)

```bash
# Build Docker image
docker build -t ohttp-gateway .

# Run with Docker
docker run -p 8080:8080 \
  -e OHTTP_GATEWAY_BACKEND_URL=http://your-backend:8080 \
  ohttp-gateway
```

## Quick Start

### Basic Usage

```bash
# Start gateway forwarding to local backend
ohttp-gateway --port 8080 --backend-url http://localhost:3000

# Start with environment variables
export OHTTP_GATEWAY_PORT=8080
export OHTTP_GATEWAY_BACKEND_URL=http://my-backend:8080
ohttp-gateway
```

### With External Backend

```bash
# Forward to remote API
ohttp-gateway --port 8080 --backend-url https://api.example.com

# Forward to local development server
ohttp-gateway --port 8080 --backend-url http://localhost:3000
```

## Configuration

### Command Line Options

| Option | Environment Variable | Description | Default |
|--------|---------------------|-------------|---------|
| `--port, -p` | `OHTTP_GATEWAY_PORT` | Port to bind gateway | `8080` |
| `--backend-url` | `OHTTP_GATEWAY_BACKEND_URL` | Backend service URL | `http://localhost:8080` |
| `--work-dir` | `OHTTP_GATEWAY_WORK_DIR` | Working directory for keys | `~/.ohttp-gateway` |

### Environment Variables

```bash
# Gateway configuration
export OHTTP_GATEWAY_PORT=8080
export OHTTP_GATEWAY_BACKEND_URL=http://your-backend:8080
export OHTTP_GATEWAY_WORK_DIR=/path/to/work/dir

# Logging configuration
export RUST_LOG=debug

# Start gateway
ohttp-gateway
```

### OHTTP Keys

The gateway automatically generates OHTTP keys in the work directory if they don't exist:

```bash
# Keys are auto-generated on first run to work_dir/ohttp_keys.json
ohttp-gateway --work-dir ./my-work-dir

# View generated keys
cat ./my-work-dir/ohttp_keys.json
```

**Manual Key Generation:**
```bash
# Generate keys separately if needed
# (Keys are automatically generated on startup)
ohttp-gateway --port 8080 --backend-url http://localhost:3000
```

## Usage

### Starting the Gateway

```bash
# Basic startup
ohttp-gateway --port 8080 --backend-url http://localhost:3000

# With custom work directory
ohttp-gateway \
  --port 8080 \
  --backend-url http://localhost:3000 \
  --work-dir /etc/ohttp

# With debug logging
RUST_LOG=debug ohttp-gateway --port 8080 --backend-url http://localhost:3000
```

### Endpoints

The gateway exposes these endpoints:

#### `POST /.well-known/ohttp-gateway`
**Main OHTTP endpoint** - Accepts encrypted requests and forwards them to the backend.

- **Content-Type**: `message/ohttp-req`
- **Response**: `message/ohttp-res`
- **Function**: Decrypts OHTTP request, forwards to backend, encrypts response

#### `GET /ohttp-keys`
**Key configuration endpoint** - Returns the public OHTTP keys for client encryption.

- **Response**: JSON with OHTTP public key configuration
- **Function**: Allows clients to fetch encryption keys

#### `POST /test-gateway`
**Test endpoint** - Accepts JSON requests for testing gateway functionality.

- **Content-Type**: `application/json`
- **Function**: Non-OHTTP endpoint for testing and debugging

#### `POST /*` (Fallback)
**Catch-all OHTTP handler** - Any POST request is treated as potential OHTTP.

- **Function**: Attempts OHTTP decapsulation on all POST requests

### Testing the Gateway

#### Health Check

```bash
# Simple health check via client
ohttp-client --gateway-url http://localhost:8080 health

# Direct HTTP check
curl -X POST http://localhost:8080/.well-known/ohttp-gateway \
  -H "Content-Type: message/ohttp-req" \
  --data-binary "@test-request.bin"
```

#### Key Retrieval

```bash
# Get OHTTP keys
curl http://localhost:8080/ohttp-keys

# Expected response format:
{
  "keys": [{
    "config": "base64-encoded-key-config...",
    "key_id": 1
  }]
}
```

#### End-to-End Test

```bash
# Terminal 1: Start backend service
python3 -m http.server 3000

# Terminal 2: Start gateway
ohttp-gateway --port 8080 --backend-url http://localhost:3000

# Terminal 3: Test with client
ohttp-client --gateway-url http://localhost:8080 send \
  --data "Hello Backend!" \
  --path "/test"
```

## Backend Integration

### Backend Requirements

Your backend service must:
- Accept standard HTTP requests
- Return standard HTTP responses  
- Be accessible from the gateway server
- Handle the request paths your clients will use

### Example Backends

#### Simple HTTP Server

```python
# simple_backend.py
from http.server import HTTPServer, BaseHTTPRequestHandler
import json

class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        content_length = int(self.headers['Content-Length'])
        body = self.rfile.read(content_length)
        
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        
        response = {
            "message": "Received via OHTTP",
            "path": self.path,
            "body": body.decode('utf-8')
        }
        self.wfile.write(json.dumps(response).encode())

# Run: python3 simple_backend.py
HTTPServer(('localhost', 3000), Handler).serve_forever()
```

#### Express.js Backend

```javascript
// backend.js
const express = require('express');
const app = express();

app.use(express.json());
app.use(express.text());

app.all('*', (req, res) => {
  res.json({
    message: 'Received via OHTTP Gateway',
    method: req.method,
    path: req.path,
    headers: req.headers,
    body: req.body
  });
});

// Run: node backend.js
app.listen(3000, () => console.log('Backend on port 3000'));
```

#### FastAPI Backend

```python
# backend.py
from fastapi import FastAPI, Request
import uvicorn

app = FastAPI()

@app.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE"])
async def handle_all(request: Request, path: str):
    body = await request.body()
    return {
        "message": "Received via OHTTP Gateway",
        "method": request.method,
        "path": path,
        "headers": dict(request.headers),
        "body": body.decode() if body else None
    }

# Run: uvicorn backend:app --port 3000
```

### Backend Configuration

```bash
# Local backend
ohttp-gateway --backend-url http://localhost:3000

# Remote backend
ohttp-gateway --backend-url https://api.myservice.com

# Backend with custom port
ohttp-gateway --backend-url http://backend.local:8080

# HTTPS backend with custom headers
ohttp-gateway --backend-url https://secure-api.example.com
```

## Production Deployment

### Systemd Service

```ini
# /etc/systemd/system/ohttp-gateway.service
[Unit]
Description=OHTTP Gateway
After=network.target

[Service]
Type=simple
User=ohttp
Group=ohttp
WorkingDirectory=/opt/ohttp-gateway
Environment=OHTTP_GATEWAY_PORT=8080
Environment=OHTTP_GATEWAY_BACKEND_URL=http://localhost:3000
Environment=OHTTP_GATEWAY_WORK_DIR=/var/lib/ohttp
Environment=RUST_LOG=info
ExecStart=/usr/local/bin/ohttp-gateway
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
# Enable and start service
sudo systemctl enable ohttp-gateway
sudo systemctl start ohttp-gateway
sudo systemctl status ohttp-gateway
```

### Docker Deployment

```dockerfile
# Dockerfile
FROM rust:1.85 as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ohttp-gateway /usr/local/bin/
EXPOSE 8080
CMD ["ohttp-gateway"]
```

```bash
# Build and run
docker build -t ohttp-gateway .
docker run -d \
  --name ohttp-gateway \
  -p 8080:8080 \
  -e OHTTP_GATEWAY_BACKEND_URL=http://backend:3000 \
  -e OHTTP_GATEWAY_WORK_DIR=/var/lib/ohttp \
  -v ./work-dir:/var/lib/ohttp \
  ohttp-gateway
```

### Docker Compose

```yaml
# docker-compose.yml
version: '3.8'
services:
  ohttp-gateway:
    build: .
    ports:
      - "8080:8080"
    environment:
      - OHTTP_GATEWAY_BACKEND_URL=http://backend:3000
      - OHTTP_GATEWAY_WORK_DIR=/data
      - RUST_LOG=info
    volumes:
      - ./data:/data
    depends_on:
      - backend

  backend:
    image: nginx:alpine
    ports:
      - "3000:80"
    volumes:
      - ./html:/usr/share/nginx/html
```

### Nginx Reverse Proxy

```nginx
# /etc/nginx/sites-available/ohttp-gateway
server {
    listen 443 ssl http2;
    server_name gateway.example.com;
    
    ssl_certificate /etc/ssl/certs/gateway.crt;
    ssl_certificate_key /etc/ssl/private/gateway.key;
    
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        
        # OHTTP requires binary content handling
        proxy_request_buffering off;
        proxy_buffering off;
        
        # CORS headers for web clients
        add_header Access-Control-Allow-Origin "*" always;
        add_header Access-Control-Allow-Methods "GET, POST, OPTIONS" always;
        add_header Access-Control-Allow-Headers "Content-Type, Content-Length" always;
    }
}
```

## Monitoring and Logging

### Logging Configuration

```bash
# Basic logging
RUST_LOG=info ohttp-gateway

# Debug logging
RUST_LOG=debug ohttp-gateway

# Module-specific logging
RUST_LOG=ohttp_gateway=debug,axum=info ohttp-gateway

# Structured JSON logging (for production)
RUST_LOG=info RUST_LOG_FORMAT=json ohttp-gateway
```

### Health Monitoring

```bash
# Check gateway health
curl -f http://localhost:8080/ohttp-keys || echo "Gateway unhealthy"

# Monitor with script
#!/bin/bash
while true; do
  if curl -s -f http://localhost:8080/ohttp-keys > /dev/null; then
    echo "$(date): Gateway healthy"
  else
    echo "$(date): Gateway unhealthy"
  fi
  sleep 30
done
```

### Metrics Collection

The gateway provides basic metrics through logs. For production monitoring:

```bash
# Enable detailed logging
RUST_LOG=ohttp_gateway=info ohttp-gateway 2>&1 | tee gateway.log

# Extract metrics from logs
grep "Request processed" gateway.log | wc -l  # Request count
grep "Error" gateway.log | tail -10          # Recent errors
```

## Security Considerations

### Key Management

- **Rotation**: Regularly rotate OHTTP keys for forward secrecy
- **Backup**: Securely backup key files
- **Permissions**: Restrict key file access (`chmod 600`)

```bash
# Secure key file permissions (keys are in work directory)
chmod 600 /var/lib/ohttp/ohttp_keys.json
chown ohttp:ohttp /var/lib/ohttp/ohttp_keys.json
```

### Network Security

- **TLS**: Always use HTTPS in production
- **Firewall**: Restrict access to gateway ports
- **Backend Access**: Ensure backend is not directly accessible

### Privacy Protection

- **No Logging**: Gateway doesn't log request content
- **Memory Safety**: Rust prevents memory-based attacks  
- **Stateless**: No session storage reduces attack surface

## Performance Tuning

### Configuration

```bash
# Increase worker threads for high load
TOKIO_WORKER_THREADS=8 ohttp-gateway

# Adjust stack size if needed
RUST_MIN_STACK=4194304 ohttp-gateway
```

### Optimization Tips

1. **Backend Latency**: Keep backend services fast and nearby
2. **Key Caching**: OHTTP keys are cached in memory
3. **Connection Pooling**: Gateway maintains connection pools to backends
4. **Async I/O**: All operations are non-blocking

### Benchmarking

```bash
# Install benchmark tools
cargo install drill

# Create benchmark file
echo 'base: "http://localhost:8080"
concurrency: 10
iterations: 1000

plan:
  - name: OHTTP Gateway Load Test
    request:
      url: /ohttp-keys
      method: GET' > benchmark.yml

# Run benchmark
drill --benchmark benchmark.yml
```

## Troubleshooting

### Common Issues

#### Gateway Won't Start

```
Error: Address already in use
```
**Solution**: Change port or stop the conflicting service.

```bash
# Find process using port
lsof -i :8080

# Use different port
ohttp-gateway --port 8081
```

#### Backend Connection Errors

```
Error: Connection refused to backend
```
**Solution**: Verify backend URL and ensure service is running.

```bash
# Test backend directly
curl http://localhost:3000/health

# Check gateway configuration
ohttp-gateway --backend-url http://correct-backend:3000
```

#### OHTTP Decryption Failures

```
Error: OHTTP decapsulation failed
```
**Solution**: Check client is using correct keys and protocol version.

```bash
# Verify keys are accessible
curl http://localhost:8080/ohttp-keys

# Enable debug logging
RUST_LOG=debug ohttp-gateway
```

### Debug Mode

```bash
# Full debug output
RUST_LOG=trace ohttp-gateway

# Focus on specific modules
RUST_LOG=ohttp_gateway::gateway=debug ohttp-gateway

# Network-level debugging
RUST_LOG=hyper=debug,reqwest=debug ohttp-gateway
```

### Log Analysis

```bash
# Monitor real-time logs
tail -f gateway.log | grep -E "(ERROR|WARN)"

# Count requests by type
grep "POST /.well-known/ohttp-gateway" gateway.log | wc -l
grep "GET /ohttp-keys" gateway.log | wc -l

# Check for errors
grep -C 3 "Error" gateway.log | tail -20
```

## Development

### Building from Source

```bash
# Development build
cargo build

# Release build with optimizations
cargo build --release

# Development with file watching
cargo install cargo-watch
cargo watch -x run
```

### Testing

```bash
# Run unit tests
cargo test

# Run integration tests
cargo test --test integration

# Test with coverage
cargo install cargo-tarpaulin
cargo tarpaulin --out html
```

### Contributing

We welcome contributions! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass
5. Submit a pull request

## API Reference

### Configuration

```rust
use ohttp_gateway::{Cli, key_config};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    
    // Get work directory and load OHTTP keys
    let work_dir = cli.get_work_dir()?;
    let ohttp_keys_path = work_dir.join("ohttp_keys.json");
    let ohttp = key_config::load_or_generate_keys(&ohttp_keys_path)?;
    
    // Build your application with the gateway
    let app = build_app(ohttp, cli.backend_url);
    
    // Serve the application
    serve(app, cli.port).await
}
```

### Integration

```rust
use axum::{routing::post, Router, Extension};
use ohttp_gateway::gateway;

let app = Router::new()
    .route("/.well-known/ohttp-gateway", post(gateway::handle_ohttp_request))
    .route("/ohttp-keys", get(gateway::handle_ohttp_keys))
    .layer(Extension(ohttp_config))
    .layer(Extension(backend_url));
```

## License

This project is licensed under the MIT License - see the LICENSE file for details.
