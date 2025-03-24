fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/cdk-mint-rpc.proto");
    tonic_build::configure()
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile_protos(&["src/proto/cdk-mint-rpc.proto"], &["src/proto"])?;
    Ok(())
}
