# CDK Mintd

[![crates.io](https://img.shields.io/crates/v/cdk-mintd.svg)](https://crates.io/crates/cdk-mintd)
[![Documentation](https://docs.rs/cdk-mintd/badge.svg)](https://docs.rs/cdk-mintd)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

Cashu mint daemon implementation for the Cashu Development Kit (CDK). This binary provides a complete Cashu mint server implementation.

## Installation

From crates.io:
```bash
cargo install cdk-mintd
```

From source:
```bash
cargo install --path .
```

## Configuration

The mint can be configured through environment variables or a configuration file. See the documentation for available options.

## Usage

```bash
# Start the mint with default configuration
cdk-mintd

# Start with custom config file
cdk-mintd --config /path/to/config.toml

# Show help
cdk-mintd --help
```

## License

This project is licensed under the [MIT License](../../LICENSE).