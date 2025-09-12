# OHTTP Client

## Overview

The OHTTP Client implements the [Oblivious HTTP specification](https://ietf-wg-ohai.github.io/oblivious-http/draft-ietf-ohai-ohttp.html) to enable private HTTP communications. It can work with:

- **OHTTP Gateways**: Direct connection to gateways that decrypt and forward requests to backend services
- **OHTTP Relays**: Connection through relays that forward encrypted requests to gateways without seeing content

