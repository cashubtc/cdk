#![allow(clippy::unwrap_used)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/lnrpc.proto");
    println!("cargo:rerun-if-changed=src/proto/routerrpc.proto");

    // Tell cargo to tell rustc to allow missing docs in generated code
    println!("cargo:rustc-env=RUSTDOC_ARGS=--allow-missing-docs");

    // Configure tonic build to generate code with documentation
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .type_attribute(".", "#[allow(missing_docs)]")
        .field_attribute(".", "#[allow(missing_docs)]")
        .compile_protos(
            &["src/proto/lnrpc.proto", "src/proto/routerrpc.proto"],
            &["src/proto"],
        )?;

    Ok(())
}
