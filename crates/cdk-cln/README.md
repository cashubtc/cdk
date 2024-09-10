
## Minimum Supported Rust Version (MSRV)

The `cdk-cln` library should always compile with any combination of features on Rust **1.63.0**.

To build and test with the MSRV you will need to pin the below dependency versions:

```shell
cargo update -p half --precise 2.2.1
cargo update -p tokio --precise 1.38.1
cargo update -p tokio-util --precise 0.7.11
cargo update -p reqwest --precise 0.12.4
cargo update -p serde_with --precise 3.1.0
cargo update -p regex --precise 1.9.6
cargo update -p backtrace --precise 0.3.58
```
