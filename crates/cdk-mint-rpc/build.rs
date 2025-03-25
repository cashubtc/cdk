fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/cdk-mint-rpc.proto");
    
    // Tell cargo to tell rustc to allow missing docs in generated code
    println!("cargo:rustc-env=RUSTDOC_ARGS=--allow-missing-docs");
    
    // Configure tonic build to generate code with documentation
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        // Generate code with documentation for all elements
        .generate_comments(true)
        // Add #[allow(missing_docs)] attribute to all generated items
        .type_attribute(".", "#[allow(missing_docs)]")
        .field_attribute(".", "#[allow(missing_docs)]")
        .compile_protos(&["src/proto/cdk-mint-rpc.proto"], &["src/proto"])?;
    
    Ok(())
}
