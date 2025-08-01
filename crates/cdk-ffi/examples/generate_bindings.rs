//! Example of how to generate language bindings from the FFI crate

use std::env;
use std::process::Command;

fn main() {
    println!("Generating UniFFI bindings for cdk-ffi...");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let target_dir = format!("{}/target/bindings", manifest_dir);

    // Create target directory
    std::fs::create_dir_all(&target_dir).unwrap();

    // Generate Python bindings
    let output = Command::new("cargo")
        .args(&[
            "run",
            "--bin",
            "uniffi-bindgen",
            "generate",
            "--library",
            "target/debug/libcdk_ffi.so",
            "--language",
            "python",
            "--out-dir",
            &target_dir,
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("Failed to generate Python bindings");

    if output.status.success() {
        println!("✅ Python bindings generated in {}/", target_dir);
    } else {
        println!("❌ Failed to generate Python bindings");
        println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    // Generate Swift bindings
    let output = Command::new("cargo")
        .args(&[
            "run",
            "--bin",
            "uniffi-bindgen",
            "generate",
            "--library",
            "target/debug/libcdk_ffi.so",
            "--language",
            "swift",
            "--out-dir",
            &target_dir,
        ])
        .current_dir(&manifest_dir)
        .output()
        .expect("Failed to generate Swift bindings");

    if output.status.success() {
        println!("✅ Swift bindings generated in {}/", target_dir);
    } else {
        println!("❌ Failed to generate Swift bindings");
        println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
        println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Binding generation complete!");
}
