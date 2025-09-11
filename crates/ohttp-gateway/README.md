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
