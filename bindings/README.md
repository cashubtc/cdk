# CDK Bindings

This part of the project is heavily inspired by [Bark Bindings][1],
particularly its model for exporting a Rust codebase through FFI and making it
accessible from other languages.

The purpose of this project is to expose the CDK Wallet and its associated
traits so they can be consumed in target languages as abstract classes or
interfaces. This enables developers to extend or implement language-specific
functionality while relying on the shared Rust core.

A separate crate is provided for each target language. These crates serve as
integration layers where Rust code can be adapted to the conventions and native
capabilities of each platform or language runtime.

This repository owns the Rust-facing binding surface, generation scripts, and
local smoke tests that validate the bindings alongside `cdk-ffi` changes.
Language-specific publish flows and consumer packaging can live in separate
repositories such as `cdk-swift`, which keeps release automation and
distribution concerns out of the main Rust workspace.

[1]: https://gitlab.com/ark-bitcoin/bark-ffi-bindings
