fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/cdk-mint-rpc.proto");
    // Tell cargo to tell rustc to allow missing docs in generated code
    println!("cargo:rustc-env=RUSTDOC_ARGS=--allow-missing-docs");
    
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        // Configure the build to generate code with documentation attributes
        .compile_protos(&["src/proto/cdk-mint-rpc.proto"], &["src/proto"])?;
    Ok(())
}
