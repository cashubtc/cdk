//! Build script

/// Build script for dart
fn main() {
    use camino::Utf8Path;

    let args: Vec<String> = std::env::args().collect();

    let library_path = args
        .iter()
        .find_map(|arg| {
            if !arg.starts_with("--")
                && (arg.ends_with(".dylib") || arg.ends_with(".so") || arg.ends_with(".dll"))
            {
                Some(arg.clone())
            } else {
                None
            }
        })
        .expect("Library path not found - specify a .dylib, .so, or .dll file");

    let output_dir = args
        .iter()
        .position(|arg| arg == "--out-dir")
        .and_then(|idx| args.get(idx + 1))
        .expect("--out-dir is required");

    // Get absolute path to uniffi.toml
    let current_dir = std::env::current_dir().expect("Failed to get current directory");
    let config_path_abs = current_dir.join("uniffi.toml");
    let config_path = Utf8Path::from_path(&config_path_abs).expect("Invalid UTF-8 in path");

    let library_path = Utf8Path::new(library_path.as_str());
    let out_dir = Utf8Path::new(output_dir.as_str());

    // Generate bindings using library mode
    // The library has embedded UniFFI metadata and UDL will be auto-located from cdk-ffi's source
    uniffi_dart::gen::generate_dart_bindings(
        library_path,      // Not used in library mode, but required by API
        Some(config_path), // Config file with Dart-specific settings
        Some(out_dir),     // Output directory
        library_path,      // Library file with embedded metadata
        true,              // library_mode = true (auto-locate UDL from dependencies)
    )
    .expect("Failed to generate dart bindings");

    // Post-generation patching to fix uniffi-dart codegen issues
    let generated_path = out_dir.join("cdk_ffi.dart");
    patch_generated(&generated_path);
}

/// Patches the generated cdk_ffi.dart to fix uniffi-dart codegen issues.
fn patch_generated(path: &camino::Utf8Path) {
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path, e));

    let mut content = content;

    // 1. Fix state variable collision
    content = content.replace(
        "final state = _UniffiForeignFutureState()",
        "final _futureState = _UniffiForeignFutureState()",
    );
    content = content.replace(
        "_uniffiForeignFutureHandleMap.insert(state)",
        "_uniffiForeignFutureHandleMap.insert(_futureState)",
    );
    content = content.replace(
        "removedState ?? state",
        "removedState ?? _futureState",
    );

    // 2. Insert _RustOwnedWalletDatabase proxy class before FfiConverterCallbackInterfaceWalletDatabase
    let proxy_class = r#"
/// Proxy for Rust-created WalletDatabase objects.
/// Holds a raw Rust Arc pointer; all trait methods throw because
/// Dart never calls them — the object is only lowered back to Rust.
class _RustOwnedWalletDatabase implements WalletDatabase {
  final Pointer<Void> _ptr;
  _RustOwnedWalletDatabase(this._ptr);

  Pointer<Void> clonePointer() {
    return rustCall((status) => uniffi_cdk_ffi_fn_clone_walletdatabase(_ptr, status));
  }

  void dispose() {
    rustCall((status) => uniffi_cdk_ffi_fn_free_walletdatabase(_ptr, status));
  }

  @override
  dynamic noSuchMethod(Invocation invocation) =>
    throw UnimplementedError(
      'Cannot call WalletDatabase methods on a Rust-owned object from Dart');
}

"#;
    let anchor = "class FfiConverterCallbackInterfaceWalletDatabase {";
    content = content.replace(anchor, &format!("{}{}", proxy_class, anchor));

    // 3. Patch lift() to handle Rust-created pointers
    content = content.replace(
        "  static WalletDatabase lift(Pointer<Void> handle) {\n    return _handleMap.get(handle.address);\n  }",
        "  static WalletDatabase lift(Pointer<Void> handle) {\n    try {\n      return _handleMap.get(handle.address);\n    } catch (_) {\n      // Rust-created object — wrap the pointer in a proxy\n      return _RustOwnedWalletDatabase(handle);\n    }\n  }",
    );

    // 4. Patch lower() to handle _RustOwnedWalletDatabase
    content = content.replace(
        "  static Pointer<Void> lower(WalletDatabase value) {\n    _ensureVTableInitialized();\n    final handle = _handleMap.insert(value);\n    return Pointer<Void>.fromAddress(handle);\n  }",
        "  static Pointer<Void> lower(WalletDatabase value) {\n    if (value is _RustOwnedWalletDatabase) {\n      return value.clonePointer();\n    }\n    _ensureVTableInitialized();\n    final handle = _handleMap.insert(value);\n    return Pointer<Void>.fromAddress(handle);\n  }",
    );

    std::fs::write(path, content)
        .unwrap_or_else(|e| panic!("Failed to write {}: {}", path, e));

    eprintln!("Patched {}", path);
}
