fn main() {
    println!("cargo:rerun-if-changed=src/proto/signatory.proto");

    #[cfg(feature = "grpc")]
    tonic_build::compile_protos("proto/signatory.proto").unwrap();
}
