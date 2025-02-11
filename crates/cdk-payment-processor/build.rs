fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=src/proto/payment_processor.proto");
    tonic_build::compile_protos("src/proto/payment_processor.proto")?;
    Ok(())
}
