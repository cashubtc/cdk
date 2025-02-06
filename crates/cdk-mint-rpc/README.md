
# Cashu Mint Management RPC

This crate is a grpc client and server to control and manage a cdk mint. This crate exposes a server complnate that can be imported as library compontant, see its usage in `cdk-mintd`. The client can be used as a cli by running `cargo r --bin cdk-mint-cli`.

The server can be run with or without certificate authentication. For running with authentication follow the below steps to create certificates.


# gRPC TLS Certificate Generation Guide

This guide explains how to generate the necessary TLS certificates for securing gRPC communication between client and server.

## Overview

The script generates the following certificates and keys:
- Certificate Authority (CA) certificate and key
- Server certificate and key
- Client certificate and key

All certificates are generated in PEM format, which is commonly used in Unix/Linux systems.

## Prerequisites

- OpenSSL installed on your system
- Bash shell environment

## Generated Files

The script will create the following files:
- `ca.key` - Certificate Authority private key
- `ca.pem` - Certificate Authority certificate
- `server.key` - Server private key
- `server.pem` - Server certificate
- `client.key` - Client private key
- `client.pem` - Client certificate

## Usage

1. Save the script as `generate_certs.sh`
2. Make it executable:
   ```bash
   chmod +x generate_certs.sh
   ```
3. Run the script:
   ```bash
   ./generate_certs.sh
   ```

## Certificate Details

### Certificate Authority (CA)
- 4096-bit RSA key
- Valid for 365 days
- Used to sign both server and client certificates

### Server Certificate
- 4096-bit RSA key
- Valid for 365 days
- Includes Subject Alternative Names (SAN):
  - DNS: localhost
  - DNS: my-server
  - IP: 127.0.0.1

### Client Certificate
- 4096-bit RSA key
- Valid for 365 days
- Used for client authentication


## Verification

The script includes verification steps to ensure the certificates are properly generated:
```bash
# Verify server certificate
openssl verify -CAfile ca.pem server.pem

# Verify client certificate
openssl verify -CAfile ca.pem client.pem
```

## Security Notes

1. Keep private keys (*.key files) secure and never share them
2. The CA certificate (ca.pem) needs to be distributed to both client and server
3. Server needs:
   - server.key
   - server.pem
   - ca.pem
4. Client needs:
   - client.key
   - client.pem
   - ca.pem

