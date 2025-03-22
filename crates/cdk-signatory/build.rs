fn main() {
    #[cfg(feature = "grpc")]
    tonic_build::compile_protos("proto/signatory.proto").unwrap();
}
