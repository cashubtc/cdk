//! Build script

#![allow(clippy::unwrap_used)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto");

    // Tell cargo to tell rustc to allow missing docs in generated code
    println!("cargo:rustc-env=RUSTDOC_ARGS=--allow-missing-docs");

    // Compiles the legacy monolithic proto alongside the per-domain protos
    tonic_prost_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .type_attribute(".", "#[allow(missing_docs)]")
        .field_attribute(".", "#[allow(missing_docs)]")
        .compile_protos(
            &["src/proto/cdk-mint-rpc.proto", "src/proto/keyset.proto"],
            &["src/proto"],
        )?;

    Ok(())
}
