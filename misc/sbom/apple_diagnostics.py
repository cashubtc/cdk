#!/usr/bin/env python3
"""Non-publishing Apple probes around the shared collector; never waive its failures."""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tomllib

from native_evidence import NativeError, build_records
from package_inventory import binary_identity
from sbom import LANGUAGES, ROOT, SbomError, cargo_build_command, digest, parser as collector_parser, require_components, write_json


PRODUCTS = ("cdk-ffi-swift", "cdk-ffi-go", "cdk-ffi-dart", "cdk", "cdk-common", "cdk-cli", "cdk-mintd")
TARGETS = {
    "aarch64-apple-darwin": ("macos-15", "macosx"),
    "x86_64-apple-darwin": ("macos-15-intel", "macosx"),
    "aarch64-apple-ios": ("macos-15", "iphoneos"),
    "aarch64-apple-ios-sim": ("macos-15", "iphonesimulator"),
    "x86_64-apple-ios": ("macos-15-intel", "iphonesimulator"),
}
TARGET_SETS = {
    "macos-arm64": ("aarch64-apple-darwin",),
    "macos-both": ("aarch64-apple-darwin", "x86_64-apple-darwin"),
    "swift-all": tuple(TARGETS),
}
CONTEXT = ("ImageOS", "ImageVersion", "RUNNER_ARCH", "GITHUB_SHA", "GITHUB_RUN_ID",
           "GITHUB_RUN_ATTEMPT", "GITHUB_REPOSITORY", "GITHUB_SERVER_URL")


def matrix(target_set, product):
    if product not in PRODUCTS:
        raise ValueError("Unsupported diagnostic product")
    if target_set == "swift-all" and product not in {"cdk-ffi-swift", "cdk-ffi-dart"}:
        raise ValueError("The iOS matrix is only for the existing Swift/Dart Apple release targets")
    return {"include": [{"target": target, "runner": TARGETS[target][0]} for target in TARGET_SETS[target_set]]}


def clean_environment(original, tools, developer, target, toolchain):
    """Do not leak credentials, Nix SDK overrides or loader/compiler overrides."""
    home = Path(original["HOME"])
    directories = tools.split(os.pathsep)
    if not home.is_absolute() or not directories or any(not Path(path).is_absolute() for path in directories):
        raise ValueError("HOME and pinned SBOM tool paths must be absolute")
    environment = {key: original[key] for key in CONTEXT if key in original}
    environment.update(HOME=str(home), PATH=os.pathsep.join([
        *directories, str(home / ".cargo/bin"), "/usr/bin", "/bin", "/usr/sbin", "/sbin"]),
        DEVELOPER_DIR=str(developer), RUSTUP_TOOLCHAIN=toolchain, CARGO_BUILD_JOBS="2",
        LANG="en_US.UTF-8", LC_ALL="en_US.UTF-8", TZ="UTC", CDK_SBOM_NATIVE_ENV="1")
    if original.get("TMPDIR"):
        environment["TMPDIR"] = original["TMPDIR"]
    if target.endswith("apple-darwin"):
        environment["MACOSX_DEPLOYMENT_TARGET"] = "13.0"
    else:
        environment["IPHONEOS_DEPLOYMENT_TARGET"] = "14.0"
    return environment


def launch(args):
    if sys.platform != "darwin":
        raise ValueError("Apple diagnostics must execute on a real macOS runner, not this Linux host")
    if not re.fullmatch(r"\d+(?:\.\d+){0,2}", args.xcode_version):
        raise ValueError("Xcode version must identify an explicitly installed numeric version")
    developer = Path(f"/Applications/Xcode_{args.xcode_version}.app/Contents/Developer").resolve()
    if not developer.is_dir():
        raise ValueError(f"Requested Xcode is not installed: {developer}")
    toolchain = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    environment = clean_environment(os.environ, os.environ["CDK_SBOM_TOOLS_PATH"], developer, args.target, toolchain)
    for tool, variable in (("clang", "CC"), ("clang++", "CXX"), ("ar", "AR")):
        path = subprocess.check_output(["/usr/bin/xcrun", "--find", tool], env=environment, text=True).strip()
        if not Path(path).is_file() or not Path(path).resolve().is_relative_to(developer):
            raise ValueError(f"Selected {tool} does not belong to the requested Xcode")
        environment[variable + "_" + args.target.replace("-", "_")] = path
        if tool == "clang":
            environment["CARGO_TARGET_" + args.target.upper().replace("-", "_") + "_LINKER"] = path
    os.execve(sys.executable, [sys.executable, str(Path(__file__).resolve()), "run", "--target", args.target,
                             "--product", args.product, "--profile", args.profile, "--output", str(args.output)], environment)


class Session:
    """Persist progress before commands, including failures and interrupted runs."""

    def __init__(self, output, target, product, profile):
        self.output = output.resolve()
        self.output.mkdir(parents=True, exist_ok=False)
        self.report = {
            "version": 1, "mode": "apple-diagnostic", "status": "incomplete", "release_ready": False,
            "target": target, "product": product, "profile": profile, "commands": [], "artifacts": [],
            "runner": {key: os.environ[key] for key in CONTEXT if key in os.environ},
            "limitations": [
                "Diagnostics are not a final-artifact SBOM, authenticated provenance or a reproducible build.",
                "The shared native collector currently supports Linux/Android, not Apple host/target provenance.",
                "Archive members are build inputs, not a claim about a downstream application's linked contents.",
                "Swift package/XCFramework finalization and downstream execution are not performed by this probe.",
            ],
        }
        self.save()

    def save(self):
        write_json(self.output / "report.json", self.report)

    def command(self, name, arguments, environment=None, timeout=900):
        if not re.fullmatch(r"[a-z0-9-]+", name) or (self.output / "logs" / (name + ".log")).exists():
            raise ValueError("Unsafe/duplicate diagnostic command name")
        log = self.output / "logs" / (name + ".log")
        log.parent.mkdir(exist_ok=True)
        entry = {"name": name, "arguments": list(map(str, arguments)), "status": "running",
                 "log": str(log.relative_to(self.output))}
        self.report["commands"].append(entry)
        self.save()
        print(f"Apple SBOM: {name} (full output: {entry['log']})", flush=True)
        with log.open("w") as stream:
            try:
                process = subprocess.Popen(entry["arguments"], cwd=ROOT, env=environment, stdout=stream,
                                           stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    code = process.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    # Stop only this command's process group, including rustc/cc
                    # children that would otherwise keep changing the logs.
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                    raise
            except (OSError, subprocess.TimeoutExpired) as error:
                stream.write(str(error) + "\n")
                code = 124 if isinstance(error, subprocess.TimeoutExpired) else 127
        entry.update(status="passed" if code == 0 else "failed", returncode=code, log_sha256=digest(log))
        self.save()
        with log.open("rb") as stream:
            stream.seek(max(0, log.stat().st_size - 6000))
            print(stream.read().decode("utf-8", errors="replace"), flush=True)
        return code

    def finish(self, error=None):
        if error:
            self.report["error"] = str(error)
        failed = error is not None or any(item["status"] != "passed" for item in self.report["commands"])
        self.report["status"] = "failed" if failed else "diagnostics_collected"
        self.save()
        files = []
        for path in sorted(self.output.rglob("*")):
            relative = path.relative_to(self.output)
            if relative.parts[0] == "cargo-target" or relative == Path("diagnostic-files.json") or not path.is_file():
                continue
            files.append({"path": str(relative), "sha256": digest(path)})
        write_json(self.output / "diagnostic-files.json", files)
        return 1 if failed else 0


def inspect_artifacts(session, records, package, environment, target):
    directory = session.output / "cargo-target" / target
    target_names = {item["name"] for item in package["targets"]}
    paths = set()
    for record in records:
        if record["reason"] != "compiler-artifact" or record["target"]["name"] not in target_names:
            continue
        if Path(record.get("manifest_path", "")).resolve() != Path(package["manifest_path"]).resolve():
            continue
        for filename in record["filenames"]:
            path = Path(filename)
            if path.suffix not in {".a", ".rlib", ".dylib"} and filename != record.get("executable"):
                continue
            if not path.resolve().is_relative_to(directory):
                raise ValueError("Product artifact escaped the diagnostic target directory")
            paths.add(path)
    if not paths:
        raise ValueError("Cargo reported no inspectable product artifacts")
    for number, path in enumerate(sorted(paths)):
        checksum = digest(path)
        with path.open("rb") as stream:
            identity = binary_identity(stream.read(65536))
        item = {"name": path.name, "sha256": checksum, **identity}
        session.report["artifacts"].append(item)
        session.command(f"artifact-{number}-file", ["/usr/bin/file", path], environment)
        if identity["format"] == "ar":
            session.command(f"artifact-{number}-members", ["/usr/bin/xcrun", "ar", "-t", path], environment)
        elif identity["format"] == "Mach-O":
            expected = "aarch64" if target.startswith("aarch64-") else "x86_64"
            if identity["architecture"] != expected:
                raise ValueError("Mach-O architecture does not match the diagnostic target")
            session.command(f"artifact-{number}-loads", ["/usr/bin/xcrun", "otool", "-l", path], environment)
            session.command(f"artifact-{number}-imports", ["/usr/bin/xcrun", "otool", "-L", path], environment)
            raw = session.output / f"artifact-{number}.syft.json"
            bom = session.output / f"artifact-{number}.cdx.json"
            if session.command(f"artifact-{number}-scan", ["syft", "scan", f"file:{path}", "--output", f"cyclonedx-json={raw}"], environment) == 0:
                if session.command(f"artifact-{number}-convert", ["cyclonedx", "convert", "--input-file", raw,
                        "--input-format", "json", "--output-file", bom, "--output-format", "json", "--output-version", "v1_6"], environment) == 0:
                    session.command(f"artifact-{number}-validate", ["cyclonedx", "validate", "--input-file", bom,
                                    "--input-format", "json", "--input-version", "v1_6", "--fail-on-errors"], environment)
                    document = json.loads(bom.read_text())
                    require_components(document, {(package["name"], package["version"])}, "Mach-O embedded Rust probe")
                    item["embedded_rust_components"] = sum(component.get("purl", "").startswith("pkg:cargo/")
                        for component in [document.get("metadata", {}).get("component", {}), *document.get("components", [])])
        else:
            raise ValueError("Product is not a supported Apple diagnostic artifact")
        if digest(path) != checksum:
            raise ValueError("Product bytes changed while inspecting them")
        session.save()


def diagnose(args):
    session = Session(args.output, args.target, args.product, args.profile)
    error = None
    completed = False
    try:
        if sys.platform != "darwin":
            raise ValueError("Apple builds must run on macOS; no local Apple validation is claimed")
        if os.environ.get("CDK_SBOM_NATIVE_ENV") != "1":
            raise ValueError("Use the launch command to select Xcode and isolate the native build environment")
        environment = dict(os.environ, CARGO_TARGET_DIR=str(session.output / "cargo-target"))
        sdk = TARGETS[args.target][1]
        for name, command in (
                ("revision", ["git", "rev-parse", "HEAD"]),
                ("source-status", ["git", "status", "--porcelain"]),
                ("xcode", ["/usr/bin/xcodebuild", "-version"]),
                ("sdk", ["/usr/bin/xcrun", "--sdk", sdk, "--show-sdk-version"]),
                ("sdk-build", ["/usr/bin/xcrun", "--sdk", sdk, "--show-sdk-build-version"]),
                ("clang", ["/usr/bin/xcrun", "clang", "--version"]),
                ("swift", ["/usr/bin/xcrun", "swift", "--version"]),
                ("rust", ["rustc", "-vV"]),
                ("cargo", ["cargo", "--version"])):
            session.command(name, command, environment)
        session.report["tool_selection"] = {key: environment[key] for key in (
            "DEVELOPER_DIR", "RUSTUP_TOOLCHAIN", "MACOSX_DEPLOYMENT_TARGET", "IPHONEOS_DEPLOYMENT_TARGET") if key in environment}
        session.report["flake_lock_sha256"] = digest(ROOT / "flake.lock")
        exclusions = [argument for language in LANGUAGES for argument in ("--exclude-language", language)]
        common = [sys.executable, ROOT / "misc/sbom/sbom.py", args.product, "--target", args.target,
                  "--profile", args.profile, *exclusions]
        session.command("resolved-inventory", [*common, "--output", session.output / "inventory"], environment, timeout=1800)
        session.command("shared-build-verifier", [*common, "--build", "--require-assurance", "compiled_packages",
                        "--output", session.output / "verified-build"], environment, timeout=5400)
        # Unsupported native provenance must remain a failed verification. Still
        # collect real build/layout evidence to make that failed CI run useful.
        record_file = session.output / "verified-build/evidence/cargo-build.json"
        if session.command("product-metadata", ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], environment):
            raise ValueError("Cannot identify the diagnostic product")
        metadata = json.loads((session.output / "logs/product-metadata.log").read_text())
        matches = [item for item in metadata["packages"] if item["name"] == args.product]
        if len(matches) != 1:
            raise ValueError("Diagnostic product is not uniquely identified in this workspace")
        package = matches[0]
        if record_file.is_file():
            records = json.loads(record_file.read_text())
            if not records or records[-1] != {"reason": "build-finished", "success": True}:
                raise ValueError("Shared collector retained an incomplete Cargo record")
        else:
            verifier_log = (session.output / "logs/shared-build-verifier.log").read_text()
            if "SBOM: cargo auditable" in verifier_log:
                raise ValueError("The shared Cargo build failed; refusing to repeat a failed or timed-out compilation")
            build_args = collector_parser().parse_args([args.product, "--target", args.target, "--profile", args.profile])
            if session.command("cargo-build-probe", cargo_build_command(package, build_args), environment, timeout=5400):
                raise ValueError("Diagnostic build failed; the verifier failure has not been waived")
            records = build_records((session.output / "logs/cargo-build-probe.log").read_text())
        write_json(session.output / "cargo-build-observed.json", records)
        inspect_artifacts(session, records, package, environment, args.target)
        completed = True
    except (OSError, ValueError, KeyError, SbomError, NativeError, subprocess.SubprocessError) as exception:
        error = exception
        print(f"Apple SBOM diagnostics incomplete: {exception}", file=sys.stderr)
    finally:
        if not completed and error is None:
            error = "Interrupted before diagnostic collection finished"
        status = session.finish(error)
    print(session.output)
    return status


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    selection = commands.add_parser("matrix", help="emit the bounded GitHub Actions matrix")
    selection.add_argument("--target-set", choices=TARGET_SETS, default="macos-arm64")
    selection.add_argument("--product", choices=PRODUCTS, default="cdk-ffi-swift")
    for name in ("launch", "run"):
        command = commands.add_parser(name)
        command.add_argument("--target", choices=TARGETS, required=True)
        command.add_argument("--product", choices=PRODUCTS, default="cdk-ffi-swift")
        command.add_argument("--profile", choices=("release-ffi", "release", "dev"), default="release-ffi")
        command.add_argument("--output", type=Path, required=True)
        if name == "launch":
            command.add_argument("--xcode-version", default="16.4")
    return result


def main():
    args = parser().parse_args()
    try:
        if args.command == "matrix":
            print(json.dumps(matrix(args.target_set, args.product)))
            return 0
        if args.command == "launch":
            launch(args)
            return 1  # execve never returns on success
        return diagnose(args)
    except (OSError, ValueError, KeyError, SbomError, NativeError, subprocess.SubprocessError) as error:
        print(f"Apple SBOM diagnostic setup failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
