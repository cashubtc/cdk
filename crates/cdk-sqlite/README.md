
# Cashu Development Kit SQLite Storage Backend

**ALPHA** This library is in early development, the api will change and should be used with caution.

cdk-sqlite is the sqlite storage backend for cdk.

## Crate Feature Flags

The following crate feature flags are available:

| Feature     | Default | Description                        |
|-------------|:-------:|------------------------------------|
| `wallet`    |   Yes   | Enable cashu wallet features       |
| `mint`      |   Yes   | Enable cashu mint wallet features  |

## Implemented [NUTs](https://github.com/cashubtc/nuts/):

See <https://github.com/cashubtc/cdk/blob/main/README.md>


## Minimum Supported Rust Version (MSRV)

The `cdk` library should always compile with any combination of features on Rust **1.63.0**.

To build and test with the MSRV you will need to pin the below dependency versions:

```shell
cargo update -p half --precise 2.2.1
cargo update -p home --precise 0.5.5
cargo update -p tokio --precise 1.38.1
cargo update -p serde_with --precise 3.1.0
cargo update -p reqwest --precise 0.12.4
```

## License

This project is distributed under the MIT software license - see the [LICENSE](../../LICENSE) file for details.
