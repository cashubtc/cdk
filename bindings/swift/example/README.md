# CDK Swift Wallet Example

A command-line Cashu wallet demonstrating the CDK Swift bindings.

## Prerequisites

1. Build the Swift bindings from the `bindings/swift/` directory:

```bash
cd bindings/swift
./generate-bindings.sh
```

This produces the XCFramework and generated Swift sources.

## Run

```bash
cd bindings/swift/example
swift run
```

## Features

| Command | Description |
|---------|-------------|
| `balance` | Show wallet balance |
| `mint <amount>` | Create a Lightning invoice to receive sats |
| `send <amount>` | Create a Cashu token to send |
| `receive <token>` | Redeem a Cashu token |
| `pay <invoice>` | Pay a Lightning invoice (melt) |
| `transactions` | List transaction history |
