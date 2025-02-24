fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/cdk-mint-rpc.proto");
    tonic_build::compile_protos("src/proto/cdk-mint-rpc.proto")?;
    Ok(())
}
