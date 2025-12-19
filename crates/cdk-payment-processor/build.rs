#![allow(clippy::unwrap_used)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/payment_processor.proto");
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .type_attribute(".", "#[allow(missing_docs)]")
        .field_attribute(".", "#[allow(missing_docs)]")
        .compile_protos(&["src/proto/payment_processor.proto"], &["src/proto"])?;
    Ok(())
}
