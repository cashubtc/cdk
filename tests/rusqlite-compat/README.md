# Downstream rusqlite compatibility

This independent workspace lets an application choose a rusqlite version while
using cdk-sqlite. The root workspace includes LDK Node 0.7, which constrains
rusqlite to 0.31 even when only cdk-sqlite is selected for testing.

The fixture runs the shared mint and wallet database test suites and checks that
a wallet database can be reopened and read with the application's rusqlite.
No SQLite dependencies are patched.

CI builds both endpoints on Rust 1.85 using Crane, with vendored dependencies and
committed lockfiles: Cargo.lock.min selects 0.31.0; Cargo.lock selects 0.39.0.
The Nix checks assert the locked rusqlite versions and run Cargo with --locked.
Run the same checks locally from the repository root:

```sh
nix build -L .#checks.x86_64-linux.rusqlite-compat-min
nix build -L .#checks.x86_64-linux.rusqlite-compat-max
```

In a JJ workspace without a Git checkout, use path:.#checks instead of .#checks.
For a quick Cargo run of the upper endpoint:

```sh
nix develop .#msrv --command cargo test --locked --manifest-path tests/rusqlite-compat/Cargo.toml
```

When changing the supported range, update the endpoint versions in flake.nix,
the requirements in both Cargo.toml files, and both fixture lockfiles. From
tests/rusqlite-compat, generate lockfiles with Rust 1.85 and
CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback,
then select each endpoint with cargo update -p rusqlite --precise VERSION.
Keep the upper endpoint as Cargo.lock and the lower endpoint as Cargo.lock.min.
Test newly admitted releases before widening the range.

The fixture pins simple_asn1 to the version in Cargo.lock.msrv because its newer
release requires a time version incompatible with Rust 1.85. The rusqlite upper
bound matches LDK Node: 0.40.0 and 0.40.1 also fail on Rust 1.85.
