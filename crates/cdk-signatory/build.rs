#![allow(clippy::unwrap_used)]

fn main() {
    println!("cargo:rerun-if-changed=src/proto/signatory.proto");

    #[cfg(feature = "grpc")]
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .type_attribute(".", "#[allow(missing_docs)]")
        .field_attribute(".", "#[allow(missing_docs)]")
        .compile_protos(&["src/proto/signatory.proto"], &["src/proto"])
        .expect("valid proto");
}
