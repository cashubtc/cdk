"""Exercise diagnostic orchestration on Linux without claiming to test Apple tools."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import apple_diagnostics as apple
from sbom import SbomError, cargo_build_command, digest, parser as collector_parser


class AppleDiagnosticTests(unittest.TestCase):
    def test_workflow_cancels_same_branch_or_pr_runs_regardless_of_inputs(self):
        workflow = (apple.ROOT / ".github/workflows/apple-sbom.yml").read_text()
        group = re.search(r"^  group: (.+)$", workflow, re.MULTILINE)
        self.assertIsNotNone(group)
        self.assertEqual(group.group(1), "apple-sbom-${{ github.workflow }}-${{ github.ref }}")
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' || startsWith(github.ref, 'refs/heads/') }}", workflow)

    def test_workflow_runs_on_pull_requests_without_branch_or_path_filters(self):
        workflow = (apple.ROOT / ".github/workflows/apple-sbom.yml").read_text()
        self.assertRegex(workflow, r"(?m)^  pull_request: \{\}$")
        self.assertNotIn("pull_request_target:", workflow)

    def test_default_matrix_costs_one_runner_and_full_matrix_has_five_targets(self):
        default = apple.matrix("macos-arm64", "cdk-ffi-swift")["include"]
        self.assertEqual(default, [{"target": "aarch64-apple-darwin", "runner": "macos-15"}])
        full = apple.matrix("swift-all", "cdk-ffi-swift")["include"]
        self.assertEqual(len(full), 5)
        self.assertEqual({entry["target"] for entry in full}, set(apple.TARGETS))
        self.assertEqual(next(entry["runner"] for entry in full if entry["target"] == "x86_64-apple-darwin"), "macos-15-intel")
        with self.assertRaisesRegex(ValueError, "iOS matrix"):
            apple.matrix("swift-all", "cdk-cli")
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            apple.matrix("macos-arm64", "arbitrary-command")

    def test_launcher_removes_credentials_nix_sdk_and_loader_overrides(self):
        original = {"HOME": "/Users/runner", "PATH": "/unexpected/bin", "TMPDIR": "/private/tmp/runner",
                    "ImageVersion": "example-image", "GITHUB_RUN_ID": "123", "GITHUB_TOKEN": "secret",
                    "AWS_SECRET_ACCESS_KEY": "secret", "NIX_CFLAGS_COMPILE": "wrong-sdk-flags",
                    "SDKROOT": "/nix/store/not-xcode", "CC": "unexpected-compiler", "RUSTFLAGS": "-L/extra",
                    "DYLD_LIBRARY_PATH": "/unexpected/lib", "CARGO_TARGET_DIR": "/old-cache"}
        env = apple.clean_environment(original, "/nix/store/tools/bin", Path("/Applications/Xcode.app/Contents/Developer"),
                                      "aarch64-apple-darwin", "1.99.0")
        for key in ("GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "NIX_CFLAGS_COMPILE", "SDKROOT", "CC", "RUSTFLAGS",
                    "DYLD_LIBRARY_PATH", "CARGO_TARGET_DIR"):
            self.assertNotIn(key, env)
        self.assertEqual(env["RUSTUP_TOOLCHAIN"], "1.99.0")
        self.assertEqual(env["ImageVersion"], "example-image")
        self.assertEqual(env["MACOSX_DEPLOYMENT_TARGET"], "13.0")
        self.assertIn("/Users/runner/.cargo/bin", env["PATH"])
        ios = apple.clean_environment(original, "/tools/bin", Path("/Xcode"), "aarch64-apple-ios", "1.99.0")
        self.assertNotIn("MACOSX_DEPLOYMENT_TARGET", ios)
        self.assertNotIn("SDKROOT", ios)
        self.assertEqual(ios["IPHONEOS_DEPLOYMENT_TARGET"], "14.0")

    def test_empty_or_relative_tool_paths_are_not_accepted(self):
        for tools in ("", "relative/bin", "/tools/bin:"):
            with self.subTest(tools=tools), self.assertRaises(ValueError):
                apple.clean_environment({"HOME": "/Users/runner"}, tools, Path("/Xcode"), "aarch64-apple-darwin", "1.99.0")

    def test_isolated_environment_finds_protoc_in_exported_tool_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            tools = Path(directory) / "pinned-tools/bin"
            tools.mkdir(parents=True)
            protoc = tools / "protoc"
            protoc.write_text(f"#!{sys.executable}\nprint('pinned protoc fixture')\n")
            protoc.chmod(0o700)
            environment = apple.clean_environment(
                {"HOME": directory, "PROTOC": "/unexpected/protoc", "PATH": "/unexpected/bin"},
                str(tools), Path("/Xcode"), "aarch64-apple-darwin", "1.99.0")
            self.assertNotIn("PROTOC", environment)
            self.assertEqual(shutil.which("protoc", path=environment["PATH"]), str(protoc))
            self.assertEqual(subprocess.check_output(["protoc", "--version"], env=environment, text=True).strip(),
                             "pinned protoc fixture")

    def test_probe_uses_shared_production_build_command(self):
        package = {"name": "cdk-ffi-swift", "targets": [{"kind": ["lib"], "crate_types": ["staticlib", "cdylib"]},
                                                        {"kind": ["bin"], "crate_types": ["bin"]}]}
        args = collector_parser().parse_args(["cdk-ffi-swift", "--target", "aarch64-apple-darwin",
                                              "--profile", "release-ffi", "--features", "example"])
        command = cargo_build_command(package, args)
        self.assertEqual(command[:3], ["cargo", "auditable", "build"])
        self.assertIn("--locked", command)
        self.assertEqual(command[-1], "--lib")
        self.assertIn("--message-format", command)
        self.assertIn("example", command)
        self.assertNotIn("--bins", command)

    def test_failed_command_retains_log_and_does_not_become_success(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "run"
            session = apple.Session(root, "aarch64-apple-darwin", "cdk-ffi-swift", "dev")
            self.assertEqual(json.loads((root / "report.json").read_text())["status"], "incomplete")
            code = session.command("failed-verifier", [sys.executable, "-c", "print('native adapter unavailable'); raise SystemExit(7)"])
            self.assertEqual(code, 7)
            session.command("successful-followup", [sys.executable, "-c", "print('additional diagnostic evidence')"])
            self.assertEqual(session.finish(), 1)
            report = json.loads((root / "report.json").read_text())
            self.assertEqual(report["status"], "failed")
            self.assertFalse(report["release_ready"])
            first = report["commands"][0]
            self.assertEqual(first["log_sha256"], digest(root / first["log"]))
            self.assertIn("native adapter", (root / first["log"]).read_text())

    def test_timeout_is_a_failure_and_retains_diagnostics(self):
        with tempfile.TemporaryDirectory() as directory:
            session = apple.Session(Path(directory) / "run", "aarch64-apple-darwin", "cdk-ffi-swift", "dev")
            self.assertEqual(session.command("timeout", [sys.executable, "-c", "import time; time.sleep(10)"], timeout=0.02), 124)
            self.assertEqual(session.finish(), 1)

    def test_no_release_claim_and_no_compiler_cache_in_feedback_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "run"
            session = apple.Session(root, "aarch64-apple-darwin", "cdk-ffi-swift", "dev")
            (root / "cargo-target").mkdir()
            (root / "cargo-target/large-binary").write_bytes(b"not feedback evidence")
            self.assertEqual(session.finish(), 0)
            report = json.loads((root / "report.json").read_text())
            self.assertEqual(report["status"], "diagnostics_collected")
            self.assertFalse(report["release_ready"])
            manifest = json.loads((root / "diagnostic-files.json").read_text())
            self.assertFalse(any(entry["path"].startswith("cargo-target/") for entry in manifest))
            self.assertTrue(any(entry["path"] == "report.json" for entry in manifest))

    def test_linux_execution_is_refused_with_a_failed_report(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "run"
            args = apple.parser().parse_args(["run", "--target", "aarch64-apple-darwin", "--output", str(root)])
            with patch.object(apple.sys, "platform", "linux"), patch.object(apple.Session, "command") as command:
                self.assertEqual(apple.diagnose(args), 1)
            command.assert_not_called()
            self.assertEqual(json.loads((root / "report.json").read_text())["status"], "failed")

    def test_direct_run_requires_the_isolated_launcher_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            args = apple.parser().parse_args(["run", "--target", "aarch64-apple-darwin", "--output", str(Path(directory) / "run")])
            with patch.object(apple.sys, "platform", "darwin"), patch.dict(os.environ, {}, clear=True):
                self.assertEqual(apple.diagnose(args), 1)

    def test_product_artifacts_must_stay_in_the_reported_target_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            session = apple.Session(Path(directory) / "run", "aarch64-apple-darwin", "cdk-ffi-swift", "dev")
            package = {"manifest_path": "/source/Cargo.toml", "targets": [{"name": "cdk_ffi_swift"}]}
            records = [{"reason": "compiler-artifact", "manifest_path": "/source/Cargo.toml",
                        "target": {"name": "cdk_ffi_swift"}, "filenames": [str(Path(directory) / "outside.dylib")]}]
            with self.assertRaisesRegex(ValueError, "escaped"):
                apple.inspect_artifacts(session, records, package, {}, "aarch64-apple-darwin")

    def test_existing_diagnostic_output_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sentinel = root / "report.json"
            sentinel.write_text("user evidence")
            with self.assertRaises(FileExistsError):
                apple.Session(root, "aarch64-apple-darwin", "cdk-ffi-swift", "dev")
            self.assertEqual(sentinel.read_text(), "user evidence")

    def exercise_failed_verifier(self, root, verifier_text):
        """Fake only Apple commands, not their failure propagation or file handling."""
        manifest = root / "source/Cargo.toml"
        package = {"name": "cdk-ffi-swift", "version": "1", "manifest_path": str(manifest),
                   "targets": [{"name": "cdk_ffi_swift", "kind": ["lib"], "crate_types": ["staticlib", "cdylib"]}]}
        args = apple.parser().parse_args(["run", "--target", "aarch64-apple-darwin", "--profile", "dev",
                                          "--output", str(root / "run")])

        def fake_command(session, name, arguments, environment=None, timeout=900):
            log = session.output / "logs" / (name + ".log")
            log.parent.mkdir(exist_ok=True)
            text = "fixture\n"
            code = 0
            if name == "shared-build-verifier":
                code, text = 1, verifier_text
            elif name == "product-metadata":
                text = json.dumps({"packages": [package]})
            elif name == "cargo-build-probe":
                artifact = session.output / "cargo-target/aarch64-apple-darwin/debug/libcdk_ffi_swift.a"
                artifact.parent.mkdir(parents=True)
                artifact.write_bytes(b"!<arch>\n")
                text = json.dumps({"reason": "compiler-artifact", "package_id": "product", "manifest_path": str(manifest),
                                   "target": package["targets"][0], "filenames": [str(artifact)]}) + "\n"
                text += json.dumps({"reason": "build-finished", "success": True})
            log.write_text(text)
            session.report["commands"].append({"name": name, "status": "passed" if code == 0 else "failed",
                                               "returncode": code, "log": str(log.relative_to(session.output))})
            return code

        with patch.object(apple.sys, "platform", "darwin"), patch.dict(os.environ, {"CDK_SBOM_NATIVE_ENV": "1"}, clear=True), \
                patch.object(apple.Session, "command", fake_command):
            self.assertEqual(apple.diagnose(args), 1)
        return json.loads((root / "run/report.json").read_text())

    def test_fallback_collects_real_shaped_records_without_waiving_verifier_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            report = self.exercise_failed_verifier(Path(directory), "Missing tools: readelf\n")
            self.assertEqual(report["status"], "failed")
            self.assertFalse(report["release_ready"])
            self.assertEqual(len(report["artifacts"]), 1)
            self.assertEqual(report["artifacts"][0]["format"], "ar")
            self.assertIn("cargo-build-probe", [entry["name"] for entry in report["commands"]])
            self.assertIn("artifact-0-members", [entry["name"] for entry in report["commands"]])

    def test_failed_compilation_is_not_retried_as_a_fallback_probe(self):
        with tempfile.TemporaryDirectory() as directory:
            report = self.exercise_failed_verifier(Path(directory), "SBOM: cargo auditable\ncompilation failed\n")
            self.assertEqual(report["status"], "failed")
            self.assertNotIn("cargo-build-probe", [entry["name"] for entry in report["commands"]])
            self.assertIn("refusing to repeat", report["error"])

    def test_missing_embedded_dependency_is_retained_as_a_failed_report(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(apple, "inspect_artifacts", side_effect=SbomError("embedded dependency missing")):
            report = self.exercise_failed_verifier(Path(directory), "Missing tools: readelf\n")
            self.assertEqual(report["status"], "failed")
            self.assertIn("embedded dependency missing", report["error"])


if __name__ == "__main__":
    unittest.main()
