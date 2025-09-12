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
- ✅ **Cashu Gateway Prober**: Support for gateway probing and purpose discovery

## Endpoints

- `POST /.well-known/ohttp-gateway` - Main OHTTP endpoint for encapsulated requests
- `GET /.well-known/ohttp-gateway` - Returns OHTTP public keys for clients
- `GET /.well-known/ohttp-gateway?allowed_purposes` - Gateway prober endpoint for Cashu opt-in detection
- `GET /ohttp-keys` - Alternative endpoint for OHTTP public keys

## Gateway Prober Support

The gateway implements Cashu gateway prober support. When a GET request is made to `/.well-known/ohttp-gateway?allowed_purposes`, the gateway responds with:

- **Status**: 200 OK
- **Content-Type**: `application/x-ohttp-allowed-purposes` 
- **Body**: TLS ALPN protocol list encoded containing the magic Cashu purpose string

The encoding follows the same format as BIP77 - a U16BE count of strings followed by U8 length encoded strings:
- 2 bytes: Big-endian count of strings in the list (1 for Cashu)
- 1 byte: Length of the purpose string (42 bytes)
- 42 bytes: The magic Cashu purpose string `CASHU 2253f530-151f-4800-a58e-c852a8dc8cff`

Example request:
```bash
curl "http://localhost:8080/.well-known/ohttp-gateway?allowed_purposes"
```
