# CDK FFI Bindings

UniFFI bindings for the CDK (Cashu Development Kit), providing foreign function interface access to wallet functionality for multiple programming languages.

## Supported Languages

- **🐍 Python** - With REPL integration for development
- **🍎 Swift** - iOS and macOS development
- **🎯 Kotlin** - Android and JVM development

## Development Tasks

### Build & Check
```bash
just ffi-build        # Build FFI library (release)
just ffi-build --debug # Build debug version
just ffi-check         # Check compilation
just ffi-clean         # Clean build artifacts
```

### Generate Bindings
```bash
# Generate for specific languages
just ffi-generate python
just ffi-generate swift
just ffi-generate kotlin

# Generate all languages
just ffi-generate-all

# Use --debug for faster development builds
just ffi-generate python --debug
```

### Development & Testing
```bash
# Python development with REPL
just ffi-dev-python    # Generates bindings and opens Python REPL with cdk_ffi loaded

# Test bindings
just ffi-test-python   # Test Python bindings import
just ffi-test-live-python # Run live Python test against testnut.cashudevkit.org
```

## Quick Start

```bash
# Start development
just ffi-dev-python

# In the Python REPL:
>>> dir(cdk_ffi)  # Explore available functions
>>> help(cdk_ffi.generate_mnemonic)  # Get help
```

## Live Tests

The live Python test in `tests/test_live_async_onchain_melt.py` covers
`PreparedMelt.confirm_prefer_async()`, immediate and pending melt outcomes,
`PendingMelt.wait()`, and `Wallet.finalize_pending_melts()` against
`https://testnut.cashudevkit.org`.

## Language Packages

For production use, see language-specific repositories:

- [cdk-swift](https://github.com/cashubtc/cdk-swift) - iOS/macOS packages
- [cdk-kotlin](https://github.com/cashubtc/cdk-kotlin) - Android/JVM packages  
- [cdk-go](https://github.com/cashubtc/cdk-go) - Golang packages
- [cdk-python](https://github.com/cashubtc/cdk-python) - PyPI packages
