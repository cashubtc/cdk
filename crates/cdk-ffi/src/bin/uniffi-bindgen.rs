fn main() {
    uniffi_bindgen()
}

fn uniffi_bindgen() {
    // Manually parse command line arguments for language and library path
    let args: Vec<String> = std::env::args().collect();
    let language = args
        .iter()
        .position(|arg| arg == "--language")
        .and_then(|idx| args.get(idx + 1));
    let library_path = args
        .iter()
        .position(|arg| arg == "--library")
        .and_then(|idx| args.get(idx + 1))
        .expect("specify the library path with --library");
    let output_dir = args
        .iter()
        .position(|arg| arg == "--out-dir")
        .and_then(|idx| args.get(idx + 1))
        .expect("--out-dir is required when using --library");

    match language {
        Some(lang) if lang == "dart" => {
            uniffi_dart::gen::generate_dart_bindings(
                "src/cdk_ffi.udl".into(),
                None,
                Some(output_dir.as_str().into()),
                library_path.as_str().into(),
                true,
            )
            .expect("Failed to generate dart bindings");
        }
        _ => uniffi::uniffi_bindgen_main(),
    }
}
