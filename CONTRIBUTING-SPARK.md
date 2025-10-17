# Contributing to Spark CDK Integration

Thank you for your interest in improving the Spark Lightning backend for CDK!

## Development Setup

### Prerequisites

1. **Rust** (1.85.0 or later)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Spark SDK** (sibling directory to CDK)
   ```bash
   git clone https://github.com/breez/spark-sdk ../spark-sdk
   ```

3. **Development Tools**
   ```bash
   cargo install cargo-watch
   cargo install cargo-edit
   ```

### Building

```bash
cd cdk
cargo build --package cdk-spark --all-features
```

### Running Tests

```bash
# Unit tests
cargo test --package cdk-spark

# Integration tests (requires network)
cargo test --package cdk-integration-tests --features spark -- --ignored

# All tests
cargo test --workspace --features spark
```

### Code Quality

```bash
# Format code
cargo fmt --all

# Run clippy
cargo clippy --package cdk-spark --all-features -- -D warnings

# Check documentation
cargo doc --package cdk-spark --no-deps --open
```

## Architecture

### Key Components

1. **CdkSpark** (`src/lib.rs`)
   - Main struct implementing `MintPayment` trait
   - Wraps `SparkWallet` from spark-sdk
   - Handles payment operations and events

2. **SparkConfig** (`src/config.rs`)
   - Configuration management
   - Validation logic
   - Default values

3. **Error** (`src/error.rs`)
   - Error types and conversions
   - Mapping to CDK payment errors

### Payment Flow

```
CDK Mint
  └─> MintPayment trait
      └─> CdkSpark
          └─> SparkWallet (spark-sdk)
              └─> Spark Network
```

## Making Changes

### Adding New Features

1. **Fork and Clone**
   ```bash
   git clone https://github.com/YOUR_USERNAME/cdk
   cd cdk
   git checkout -b feature/spark-your-feature
   ```

2. **Make Changes**
   - Update code in `crates/cdk-spark/src/`
   - Add tests in `crates/cdk-spark/src/tests.rs`
   - Update documentation

3. **Test Thoroughly**
   ```bash
   cargo test --package cdk-spark
   cargo clippy --package cdk-spark
   cargo fmt --all --check
   ```

4. **Update Documentation**
   - Update `README.md` if user-facing
   - Add doc comments to new functions
   - Update `CHANGELOG.md`

5. **Submit PR**
   ```bash
   git add .
   git commit -m "feat(spark): Add your feature"
   git push origin feature/spark-your-feature
   ```

### Fixing Bugs

1. **Create Test Case**
   - Add failing test demonstrating the bug
   - Place in `src/tests.rs` or integration test

2. **Fix the Bug**
   - Implement fix
   - Verify test passes
   - Check for regressions

3. **Document Fix**
   - Update `CHANGELOG.md`
   - Add doc comments if behavior changed

### Code Style

Follow Rust conventions and CDK patterns:

```rust
// Good: Clear, documented, error handling
/// Creates a Lightning invoice for the specified amount
///
/// # Arguments
/// * `amount` - Amount in satoshis
/// * `description` - Optional invoice description
///
/// # Errors
/// Returns error if wallet is not initialized or amount is invalid
pub async fn create_invoice(
    &self,
    amount: u64,
    description: Option<String>,
) -> Result<String, Error> {
    // Implementation
}

// Bad: Unclear, undocumented, unwrap
pub async fn make_inv(&self, amt: u64, desc: Option<String>) -> String {
    // Implementation with .unwrap()
}
```

## Testing Guidelines

### Unit Tests

Test individual components in isolation:

```rust
#[test]
fn test_amount_conversion() {
    let sats = Amount::from(100);
    let msats = convert_to_msats(sats);
    assert_eq!(msats, 100_000);
}
```

### Integration Tests

Test full workflows (may require network):

```rust
#[tokio::test]
#[ignore] // Requires network connection
async fn test_invoice_payment_flow() {
    let spark = setup_test_spark().await;
    let invoice = spark.create_invoice(1000, None).await.unwrap();
    // ... test full flow
}
```

### Manual Testing

Use the test configuration:

```bash
cargo build --package cdk-mintd --features spark --release
./target/release/cdk-mintd --config test-spark-mint.toml
```

## Common Tasks

### Adding Configuration Option

1. Add field to `SparkConfig` in `src/config.rs`
2. Add default value
3. Update validation if needed
4. Update `cdk-mintd` config parsing
5. Update documentation and examples

### Adding Payment Method

1. Add new variant to `IncomingPaymentOptions` or `OutgoingPaymentOptions`
2. Handle in `create_incoming_payment_request` or `make_payment`
3. Add tests
4. Update documentation

### Handling New Event Type

1. Add event variant to `WalletEvent` handling
2. Convert to CDK `Event` format
3. Test event propagation
4. Update documentation

## Documentation

### Code Documentation

Use Rust doc comments:

```rust
/// Brief description
///
/// Longer description with more details
///
/// # Arguments
/// * `param` - Description
///
/// # Returns
/// Description of return value
///
/// # Errors
/// When this function errors
///
/// # Examples
/// ```
/// let result = function(param);
/// ```
pub fn function(param: Type) -> Result<ReturnType, Error> {
    // Implementation
}
```

### User Documentation

Update these files:
- `crates/cdk-spark/README.md` - User-facing guide
- `docs/spark-backend-guide.md` - Detailed operations
- `crates/cdk-mintd/example.config.toml` - Config examples

## Debugging

### Enable Debug Logging

```bash
RUST_LOG=debug ./target/debug/cdk-mintd --config test-spark-mint.toml
```

### Common Issues

**Spark Connection Fails**
- Check network connectivity
- Verify correct network (signet/testnet/mainnet)
- Check operator pool configuration

**Payment Not Detected**
- Check event stream is active
- Verify payment hash matches
- Check logs for payment events

**Build Failures**
- Update dependencies: `cargo update`
- Clean build: `cargo clean && cargo build`
- Check Rust version: `rustc --version`

## Release Process

1. Update version in `Cargo.toml`
2. Update `CHANGELOG.md`
3. Create release PR
4. Tag release: `git tag -a v0.13.0 -m "Release v0.13.0"`
5. Push tag: `git push origin v0.13.0`

## Getting Help

- **Matrix Chat**: #dev:matrix.cashu.space
- **GitHub Issues**: Report bugs or request features
- **Discussions**: Ask questions in GitHub Discussions
- **Spark SDK**: https://sdk-doc-spark.breez.technology/

## Code of Conduct

Be respectful, inclusive, and constructive. We're all here to build great software together!

## License

By contributing, you agree that your contributions will be licensed under the MIT License.

---

**Thank you for contributing to CDK!** 🎉

