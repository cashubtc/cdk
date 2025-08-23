use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn cairo_cli_flow_script_succeeds() {
    // Paths
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // crates/cdk-integration-tests
    let script_path = crate_dir.join("tests/scripts/cairo-cli-test-flow.sh");
    assert!(
        script_path.exists(),
        "Script not found at path: {}",
        script_path.display()
    );

    // Workspace root: .../cdk
    let workspace_root = crate_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("Failed to resolve workspace root (expected two parents)");

    // Run the script via bash; success if exit status is ok
    let output = Command::new("bash")
        .arg(script_path.as_os_str())
        .env("INTEGRATION_TEST", "true")
        .current_dir(&workspace_root)
        .output()
        .expect("Failed to spawn bash for cairo-cli test flow");

    if !output.status.success() {
        eprintln!(
            "cairo-cli-test-flow.sh failed with status: {}",
            output.status
        );
        eprintln!(
            "--- STDOUT ---\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
        eprintln!(
            "--- STDERR ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert!(
        output.status.success(),
        "cairo-cli-test-flow.sh did not complete successfully"
    );
}
