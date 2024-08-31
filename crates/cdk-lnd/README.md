
## Minimum Supported Rust Version (MSRV)

The `cdk` library should always compile with any combination of features on Rust **1.66.0**.

To build and test with the MSRV you will need to pin the below dependency versions:

```shell
cargo update -p home --precise 0.5.5
cargo update -p prost --precise 0.12.3
cargo update -p prost-types --precise 0.12.3
cargo update -p prost-build --precise 0.12.3
cargo update -p prost-derive --precise 0.12.3
```
