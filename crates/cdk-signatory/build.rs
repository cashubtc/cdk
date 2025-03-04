fn main() {
    #[cfg(feature = "grpc")]
    tonic_build::compile_protos("src/proto/signatory.proto").unwrap();
}
