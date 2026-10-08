//! Export immutable releases and build version-specific test executables.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use super::process::{output, run};
use super::{check, Result};

fn git(workspace: &Path) -> Result<Command> {
    let mut command = Command::new("git");
    match workspace.join(".jj").exists() {
        true => {
            let mut repo = workspace.join(".jj/repo");
            if repo.is_file() {
                repo = repo
                    .parent()
                    .expect("JJ directory")
                    .join(fs::read_to_string(&repo)?.trim());
            }
            let store = repo.join("store");
            let backend = store
                .join(fs::read_to_string(store.join("git_target"))?.trim())
                .canonicalize()?;
            command.arg(format!("--git-dir={}", backend.display()));
        }
        false => {
            command.arg("-C").arg(workspace);
        }
    }
    Ok(command)
}

pub(crate) fn export_release(workspace: &Path, revision: &str, destination: &Path) -> Result<()> {
    let commit = match workspace.join(".jj").exists() {
        true => output(Command::new("jj").current_dir(workspace).args([
            "--ignore-working-copy",
            "log",
            "-r",
            revision,
            "--no-graph",
            "-T",
            "commit_id",
        ]))?,
        false => output(git(workspace)?.args([
            "rev-parse",
            "--end-of-options",
            &format!("{revision}^{{commit}}"),
        ]))?,
    };
    let commit = commit.trim();
    check(
        matches!(commit.len(), 40 | 64),
        "baseline must resolve to one commit",
    )?;
    println!("Baseline {revision}: {commit}");
    let archive = destination.with_extension("tar");
    run(git(workspace)?
        .args(["archive", "--format=tar", commit])
        .stdout(Stdio::from(fs::File::create(&archive)?)))?;
    fs::create_dir_all(destination)?;
    // The archive is generated from this repository's trusted Git object.
    run(Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(destination))?;
    fs::remove_file(archive)?;
    Ok(())
}

pub(crate) fn export_current(workspace: &Path, destination: &Path) -> Result<()> {
    let files = match workspace.join(".jj").exists() {
        true => output(Command::new("jj").current_dir(workspace).args([
            "--ignore-working-copy",
            "file",
            "list",
        ]))?,
        false => output(git(workspace)?.arg("ls-files"))?,
    };
    for name in files.lines() {
        let source = workspace.join(name);
        if source.is_file() {
            let target = destination.join(name);
            fs::create_dir_all(target.parent().expect("file parent"))?;
            fs::copy(source, target)?;
        }
    }
    Ok(())
}

pub(crate) fn build(workspace: &Path, source: &Path, destination: &Path) -> Result<()> {
    install_payment_control(workspace, source)?;
    // Unique target names prevent other builds in the shared Cargo directory
    // from replacing an executable between the build and the copy.
    let suffix = format!(
        "{}-{}",
        source
            .parent()
            .expect("scratch root")
            .file_name()
            .expect("root name")
            .to_string_lossy(),
        source.file_name().expect("source name").to_string_lossy()
    );
    let mint = format!("cdk-upgrade-mintd-{suffix}");
    let wallet = format!("cdk-upgrade-wallet-{suffix}");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(source.join("crates/cdk-mintd/Cargo.toml"))?,
        "\n[[bin]]\nname = \"{mint}\"\npath = \"src/main.rs\""
    )?;
    let driver = source.join("crates/cdk-upgrade-driver");
    fs::create_dir_all(driver.join("src"))?;
    fs::copy(
        workspace.join("crates/cdk-integration-tests/upgrade/driver.rs"),
        driver.join("src/main.rs"),
    )?;
    fs::copy(
        workspace.join("crates/cdk-integration-tests/upgrade/pending.rs"),
        driver.join("src/pending.rs"),
    )?;
    fs::write(
        driver.join("Cargo.toml"),
        format!(
            r#"[package]
name = "cdk-upgrade-driver"
version = "0.0.0"
edition = "2021"

[dependencies]
anyhow.workspace = true
cdk = {{ workspace = true, features = ["wallet"] }}
cdk-sqlite = {{ workspace = true, features = ["wallet"] }}
cdk-fake-wallet.workspace = true
cdk-common = {{ workspace = true, features = ["wallet"] }}
serde = {{ workspace = true, features = ["derive"] }}
serde_json.workspace = true
tokio = {{ workspace = true, features = ["macros", "rt-multi-thread"] }}
uuid.workspace = true

[lints]
workspace = true

[[bin]]
name = "{wallet}"
path = "src/main.rs"
"#
        ),
    )?;
    let manifest = source.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)?;
    if !text.contains("\"crates/*\"") {
        fs::write(
            manifest,
            text.replacen(
                "members = [",
                "members = [\n    \"crates/cdk-upgrade-driver\",",
                1,
            ),
        )?;
    }
    let metadata: Value =
        serde_json::from_str(&output(Command::new("cargo").current_dir(source).args([
            "metadata",
            "--no-deps",
            "--format-version=1",
        ]))?)?;
    let target = PathBuf::from(
        metadata["target_directory"]
            .as_str()
            .expect("Cargo target directory"),
    )
    .join("debug");
    fs::create_dir_all(destination)?;
    run(Command::new("cargo").current_dir(source).args([
        "build",
        "-p",
        "cdk-mintd",
        "--bin",
        &mint,
        "--no-default-features",
        "--features",
        "sqlite,fakewallet",
    ]))?;
    fs::copy(target.join(mint), destination.join("mintd"))?;
    run(Command::new("cargo").current_dir(source).args([
        "build",
        "-p",
        "cdk-upgrade-driver",
        "--bin",
        &wallet,
    ]))?;
    fs::copy(target.join(wallet), destination.join("wallet"))?;
    Ok(())
}

fn install_payment_control(workspace: &Path, source: &Path) -> Result<()> {
    let directory = source.join("crates/cdk-fake-wallet/src");
    fs::copy(
        workspace.join("crates/cdk-integration-tests/upgrade/payment_control.rs"),
        directory.join("upgrade_control.rs"),
    )?;
    let path = directory.join("lib.rs");
    let mut text = fs::read_to_string(&path)?;
    let hooks = [
        ("        let receiver = self.receiver.lock().await.take().ok_or(Error::NoReceiver)?;", "        let receiver = self.receiver.lock().await.take().ok_or(Error::NoReceiver)?;\n        upgrade_control::start_incoming(self.sender.clone());"),
        ("pub mod error;", "mod upgrade_control;\n\npub mod error;"),
        ("        // ALL invoices get immediate payment processing (original behavior)", "        if upgrade_control::hold_incoming(&request, &payment_hash, &amount)? {\n            return Ok(CreateIncomingPaymentResponse { request_lookup_id: payment_hash, request, expiry, extra_json: None });\n        }\n\n        // ALL invoices get immediate payment processing (original behavior)"),
        ("                let description = bolt11.description().to_string();", "                if let Some(response) = upgrade_control::make_outgoing(&bolt11, unit)? {\n                    return Ok(response);\n                }\n                let description = bolt11.description().to_string();"),
        ("    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {", "    ) -> Result<Vec<WaitPaymentResponse>, Self::Err> {\n        if let Some(response) = upgrade_control::incoming(request_lookup_id)? {\n            return Ok(response);\n        }"),
        ("        // For fake wallet if the state is not explicitly set default to paid", "        if let Some(response) = upgrade_control::outgoing(request_lookup_id)? {\n            return Ok(response);\n        }\n        // For fake wallet if the state is not explicitly set default to paid"),
    ];
    for (anchor, replacement) in hooks {
        check(
            text.matches(anchor).count() == 1,
            format!("fake backend hook changed: {anchor}"),
        )?;
        text = text.replacen(anchor, replacement, 1);
    }
    fs::write(path, text)?;
    Ok(())
}
