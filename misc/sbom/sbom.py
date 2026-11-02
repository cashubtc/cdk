#!/usr/bin/env python3
"""Collect SBOMs using ecosystem tools; never treat a source scan as a binary scan."""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import uuid

from native_evidence import NativeCollector, NativeError, build_records
from assurance import DIMENSIONS, assessment, require
from package_inventory import archive_natives, file_bom, go_bom, maven_bom, native_files, native_record
from rust_inventory import resolve_consumer, resolved_bom
from link_evidence import real_linker


ROOT = Path(__file__).resolve().parents[2]
LANGUAGES = ("dart", "swift", "go", "kotlin")
WRAPPERS = {language: f"cdk-ffi-{language}" for language in LANGUAGES}
IGNORE = shutil.ignore_patterns(
    "target", "build", ".build", ".git", ".gradle", ".dart_tool", "node_modules",
    ".env", "local.properties", "*.cdx.json",
)


class SbomError(Exception):
    """An incomplete or invalid collection, with an actionable diagnostic."""


def run(args, cwd, env=None):
    """Do not use a shell or include command output in public evidence."""
    print(f"SBOM: {args[0]} {args[1] if len(args) > 1 else ''}", file=sys.stderr)
    result = subprocess.run(
        [str(arg) for arg in args], cwd=cwd, env=env, text=True,
        stdout=subprocess.PIPE, check=False,
    )
    if result.returncode:
        if result.stdout:
            print(result.stdout[-4000:], file=sys.stderr)
        raise SbomError(f"{args[0]} failed (exit {result.returncode}); see its diagnostic above")
    return result.stdout


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def read_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError) as error:
        raise SbomError(f"Cannot read JSON from {path}: {error}") from error


def json_stream(text):
    """Go emits consecutive JSON objects, not one JSON array."""
    decoder = json.JSONDecoder()
    remaining = text.lstrip()
    while remaining:
        item, end = decoder.raw_decode(remaining)
        yield item
        remaining = remaining[end:].lstrip()


def go_environment(target):
    environment = dict(os.environ, CGO_ENABLED="1")
    if target:
        arch = {"x86_64": "amd64", "aarch64": "arm64"}.get(target.split("-", 1)[0])
        system = next((goos for marker, goos in (("apple-darwin", "darwin"), ("windows", "windows"),
                                               ("unknown-linux", "linux")) if marker in target), None)
        if not arch or not system:
            raise SbomError(f"No Go binding platform mapping for Rust target {target}")
        environment.update(GOOS=system, GOARCH=arch)
    return environment


def components(bom):
    def walk(items):
        for item in items:
            yield item
            yield from walk(item.get("components", []))

    root = bom.get("metadata", {}).get("component")
    yield from walk(([root] if root else []) + bom.get("components", []))


def package_components(bom):
    """Cargo target subcomponents are not additional dependency packages."""
    root = bom.get("metadata", {}).get("component")
    return ([root] if root else []) + bom.get("components", [])


def normalize_subject(bom):
    """cdxgen can repeat its subject in components; retain one consistent definition."""
    subject = bom.get("metadata", {}).get("component", {})
    reference = subject.get("bom-ref")
    if not reference:
        return
    retained = []
    for item in bom.get("components", []):
        if item.get("bom-ref") != reference:
            retained.append(item)
            continue
        for key, value in item.items():
            if key == "group" and not value and not subject.get(key):
                continue
            if key in subject and subject[key] != value:
                raise SbomError(f"Conflicting subject definitions for {reference}")
            subject[key] = value
    bom["components"] = retained


def normalize_vcs_urls(bom):
    """Correct the Gradle plugin's scp-style Git URL conversion without dropping it."""
    for component in components(bom):
        for reference in component.get("externalReferences", []):
            if reference.get("type") != "vcs":
                continue
            url = reference.get("url", "")
            match = re.fullmatch(r"ssh://([^/:]+):([A-Za-z_][^\s]*)", url)
            if match:
                reference["url"] = f"ssh://{match[1]}/{match[2]}"
                component.setdefault("properties", []).append({"name": "cdk:original-vcs-url", "value": url})


def dart_dependency_graph(bom, graph):
    """Attach Pub's resolved edges; a lockfile alone only supplies a package list."""
    normalize_subject(bom)
    packages = {item["name"]: item for item in graph["packages"]}
    indexed = {(item.get("name"), item.get("version")): item for item in components(bom)}
    edges = []
    for name, package in packages.items():
        if package.get("source", "hosted") not in {"hosted", "root"}:
            raise SbomError(f"Dart {name}: non-hosted dependency identity needs explicit collector support")
        component = indexed.get((name, package["version"]))
        if not component or not component.get("bom-ref"):
            raise SbomError(f"Dart collector omitted resolved package: {name}@{package['version']}")
        dependencies = []
        for dependency in package.get("dependencies", []):
            resolved = packages.get(dependency)
            if not resolved:
                raise SbomError(f"Pub dependency graph omitted {dependency}")
            child = indexed.get((dependency, resolved["version"]))
            if not child or not child.get("bom-ref"):
                raise SbomError(f"Dart collector omitted dependency {dependency}")
            dependencies.append(child["bom-ref"])
        component.setdefault("properties", []).append(
            {"name": "cdk:dart:dependency-kind", "value": package.get("kind", "unknown")}
        )
        edges.append({"ref": component["bom-ref"], "dependsOn": sorted(dependencies)})
    bom["dependencies"] = edges


def check_graph(bom):
    """Schema validation alone does not reject dangling dependency references."""
    if bom.get("bomFormat") != "CycloneDX" or not bom.get("metadata", {}).get("component"):
        raise SbomError("Generator did not provide a CycloneDX document with a subject")
    refs = [item["bom-ref"] for item in components(bom) if "bom-ref" in item]
    if len(refs) != len(set(refs)):
        raise SbomError("Duplicate component bom-ref values")
    known = set(refs)
    for edge in bom.get("dependencies", []):
        for ref in [edge["ref"], *edge.get("dependsOn", []), *edge.get("provides", [])]:
            if ref not in known:
                raise SbomError(f"Dangling dependency reference: {ref}")


def require_components(bom, expected, ecosystem):
    actual = {(item.get("name"), item.get("version")) for item in components(bom)}
    actual.update((f"{item['group']}/{item['name']}", item.get("version"))
                  for item in components(bom) if item.get("group"))
    missing = set(expected) - actual
    if missing:
        names = ", ".join(f"{name}@{version}" for name, version in sorted(missing))
        raise SbomError(f"{ecosystem} collector omitted resolved packages: {names}")


def copy_project(source, destination):
    if not source.is_dir():
        raise SbomError(f"Project directory does not exist: {source}")
    shutil.copytree(source, destination, ignore=IGNORE, symlinks=False)
    return destination


def cargo_options(args):
    result = ["--target", args.target]
    if args.no_default_features:
        result.append("--no-default-features")
    for features in args.features:
        result.extend(["--features", features])
    return result


def cargo_build_targets(package):
    """Prefer shipped FFI libraries, then binaries, never an incidental rlib."""
    targets = package["targets"]
    native_types = {"cdylib", "dylib", "staticlib"}
    if any(native_types.intersection(target.get("crate_types", target["kind"]))
           for target in targets):
        return ["--lib"]
    if any("bin" in target["kind"] for target in targets):
        return ["--bins"]
    if any("lib" in target["kind"] or "rlib" in target["kind"] for target in targets):
        return ["--lib"]
    raise SbomError(f"Package {package['name']} has no supported library or executable target")


def rust_dependency_roles(metadata, package):
    """Follow runtime edges without crossing build/dev or proc-macro boundaries."""
    packages = {item["id"]: item for item in metadata["packages"]}
    members = set(metadata["workspace_members"])
    roots = [item["id"] for item in packages.values()
             if item["id"] in members and item["name"] == package["name"]]
    if len(roots) != 1 or not metadata.get("resolve"):
        raise SbomError(f"Cannot classify dependencies for {package['name']}: resolved Cargo graph is missing")
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}

    def walk(runtime):
        visited = set()
        pending = [roots[0]]
        while pending:
            identifier = pending.pop()
            if identifier in visited:
                continue
            visited.add(identifier)
            for dependency in nodes[identifier]["deps"]:
                kinds = {kind["kind"] for kind in dependency["dep_kinds"]}
                allowed = {None, "normal"} if runtime else {None, "normal", "build"}
                if not kinds.intersection(allowed):
                    continue
                child = packages[dependency["pkg"]]
                if runtime and any("proc-macro" in target["kind"] for target in child["targets"]):
                    continue
                pending.append(child["id"])
        return visited

    runtime = walk(True)
    non_dev = walk(False)
    return {identifier: "runtime" if identifier in runtime else "build" for identifier in non_dev}


def classify_rust_bom(bom, metadata, package):
    """Retain build-time tools in the inventory without asserting they are linked."""
    roles = rust_dependency_roles(metadata, package)
    for component in package_components(bom):
        role = roles.get(component.get("bom-ref"))
        if role is None:
            raise SbomError(f"Rust component has no resolved dependency role: {component.get('name')}")
        component["scope"] = "required" if role == "runtime" else "excluded"
        component.setdefault("properties", []).append({"name": "cdk:rust:dependency-role", "value": role})
    expected = {(item["name"], item["version"]) for item in metadata["packages"] if item["id"] in roles}
    require_components(bom, expected, "Rust resolved inventory")


def rust_build_bom(bom, metadata, package):
    """An independently scannable build environment; never mark its tools excluded."""
    roles = rust_dependency_roles(metadata, package)
    packages = {item["id"]: item for item in metadata["packages"]}
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}
    starts = set()
    for identifier in roles:
        for dependency in nodes[identifier]["deps"]:
            if dependency["pkg"] not in roles:
                continue
            if (any(kind["kind"] == "build" for kind in dependency["dep_kinds"]) or
                    any("proc-macro" in target["kind"] for target in packages[dependency["pkg"]]["targets"])):
                starts.add(dependency["pkg"])
    included = set()
    pending = list(starts)
    while pending:
        identifier = pending.pop()
        if identifier in included:
            continue
        included.add(identifier)
        pending.extend(dependency["pkg"] for dependency in nodes[identifier]["deps"]
                       if dependency["pkg"] in roles and
                       any(kind["kind"] in {None, "normal", "build"} for kind in dependency["dep_kinds"]))
    document = copy.deepcopy(bom)
    subject = document["metadata"]["component"]
    subject["bom-ref"] = "urn:cdk:build-environment:" + package["name"]
    subject["name"] = package["name"] + " build dependencies"
    subject["type"] = "application"
    for field in ("purl", "components", "hashes"):
        subject.pop(field, None)
    document["components"] = [component for component in document["components"] if component["bom-ref"] in included]
    for component in document["components"]:
        component["scope"] = "required"
    document["dependencies"] = [{"ref": subject["bom-ref"], "dependsOn": sorted(starts)}] + [
        {"ref": identifier, "dependsOn": sorted({dependency["pkg"] for dependency in nodes[identifier]["deps"]
                                                if dependency["pkg"] in included and
                                                any(kind["kind"] in {None, "normal", "build"}
                                                    for kind in dependency["dep_kinds"])})}
        for identifier in sorted(included)
    ]
    return document


def selection(scope, metadata):
    members = set(metadata["workspace_members"])
    packages = {item["name"]: item for item in metadata["packages"] if item["id"] in members}
    if scope == "all":
        return list(packages.values()), list(LANGUAGES)
    if scope == "rust":
        return list(packages.values()), []
    if scope == "bindings":
        return [packages[name] for name in WRAPPERS.values()], list(LANGUAGES)
    if scope in LANGUAGES:
        return [packages[WRAPPERS[scope]]], [scope]
    if scope in packages:
        languages = [language for language, name in WRAPPERS.items() if name == scope]
        return [packages[scope]], languages
    raise SbomError(f"Unknown scope '{scope}'; use all, rust, bindings, a language or a workspace crate")


def parse_projects(values):
    projects = {}
    for value in values:
        language, separator, path = value.partition("=")
        if not separator or language not in LANGUAGES or not path or language in projects:
            raise SbomError("Use --project LANGUAGE=PATH once per language")
        projects[language] = Path(path).resolve()
    return projects


class Collector:
    def __init__(self, args, root=ROOT):
        self.args = args
        self.root = root
        self.projects = parse_projects(args.project)
        self.output = None
        self.cargo_target_directory = self.root / "target"
        self.packaged_records = []
        self.index = {
            "status": "incomplete", "mode": "build-associated" if args.build else "inventory",
            "scope": args.scope, "documents": [], "evidence": [], "artifacts": [],
            "tools": {}, "configuration": {}, "coverage": {},
            "limitations": [
                "Cargo metadata is a workspace-resolved inventory, not a compilation-unit graph.",
                "Consumer applications may resolve different compatible package versions.",
                "Native provenance checks cover Cargo links declarations; other native dependencies require review.",
                "Packaged-native file inventories establish hashes, formats and owning packages; "
                "they do not independently identify every upstream library bundled inside those files.",
            ],
        }

    def preflight(self, languages):
        tools = {"git", "cargo", "rustc", "cargo-cyclonedx", "cyclonedx"}
        if any(lang != "kotlin" for lang in languages):
            tools.add("cdxgen")
        tools.update({"dart": "dart", "swift": "swift", "go": "go", "kotlin": "java"}[lang]
                     for lang in languages)
        if self.args.build:
            tools.add("syft")
            if not self.args.existing:
                tools.update({"cargo-auditable", "readelf"})
        missing = sorted(tool for tool in tools if not shutil.which(tool))
        if missing:
            raise SbomError("Missing tools: " + ", ".join(missing) +
                            ". Enter nix develop .#sbom; Swift also requires Xcode on macOS.")
        for language in languages:
            project = self.projects.get(language, self.root / "bindings" / language)
            manifest = {"dart": "pubspec.yaml", "swift": "Package.swift",
                        "go": "go.mod", "kotlin": "settings.gradle.kts"}[language]
            if not (project / manifest).is_file():
                raise SbomError(f"{language}: missing {manifest}. Supply --project {language}=PATH "
                                "to the assembled binding package; do not scan a synthetic manifest.")
        if "kotlin" in languages:
            sdk = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
            if not sdk or not Path(sdk).is_dir():
                raise SbomError("Kotlin requires ANDROID_HOME pointing to an installed Android SDK (API 34)")
        for tool in sorted(tools - {"git", "cargo-auditable"}):
            version_args = [tool, "version"] if tool in {"go", "syft"} else [tool, "--version"]
            if tool == "java":
                version_args = [tool, "--version"]
            if tool == "cargo-cyclonedx":
                version_args = ["cargo", "cyclonedx", "--version"]
            self.index["tools"][tool] = run(version_args, self.root).strip()
        if "cargo-auditable" in tools:
            # The wrapper's --version is forwarded to Cargo, not its own version.
            self.index["tools"]["cargo-auditable"] = {
                "executable_sha256": digest(Path(shutil.which("cargo-auditable")))
            }

    def evidence(self, source, name):
        destination = self.output / "evidence" / name
        if destination.exists():
            raise SbomError(f"Duplicate evidence path: {name}")
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
        self.index["evidence"].append({"path": str(destination.relative_to(self.output)),
                                       "sha256": digest(destination)})

    def generated_evidence(self, name, value):
        destination = self.output / "evidence" / name
        if destination.exists():
            raise SbomError(f"Duplicate evidence path: {name}")
        write_json(destination, value)
        self.index["evidence"].append({"path": str(destination.relative_to(self.output)),
                                       "sha256": digest(destination)})
        return destination

    def document(self, source, name, kind):
        destination = self.output / (name + ".cdx.json")
        destination.parent.mkdir(parents=True, exist_ok=True)
        # Normalize in a private copy, including for caller-supplied native BOMs.
        original = read_json(source)
        normalize_subject(original)
        normalize_vcs_urls(original)
        with tempfile.TemporaryDirectory(prefix=".document-", dir=self.output) as temporary:
            normalized = Path(temporary) / "input.json"
            write_json(normalized, original)
            run(["cyclonedx", "convert", "--input-file", normalized, "--input-format", "json",
                 "--output-file", destination, "--output-format", "json", "--output-version", "v1_6"],
                self.root)
        bom = read_json(destination)
        check_graph(bom)
        run(["cyclonedx", "validate", "--input-file", destination, "--input-format", "json",
             "--input-version", "v1_6", "--fail-on-errors"], self.root)
        self.index["documents"].append({"path": str(destination.relative_to(self.output)),
                                        "kind": kind, "sha256": digest(destination)})
        return bom

    def packaged_files(self, paths, name, owner, base=None):
        records = []
        for path in paths:
            checksum = digest(path)
            if path.suffix.lower() in {".jar", ".aar", ".zip"}:
                records.extend(archive_natives(path, owner, checksum, prefix=path.name + "!"))
            else:
                with path.open("rb") as stream:
                    records.append(native_record(stream, str(path.relative_to(base)) if base else path.name, owner))
            if digest(path) != checksum:
                raise SbomError(f"Packaged file changed during collection: {path}")
        source = self.generated_evidence(name + "-native-files.json", records)
        bom_source = source.with_name(source.stem + ".cdx.json")
        write_json(bom_source, file_bom(name, records))
        self.document(bom_source, f"packages/{name}.native-files", "packaged-native-files")
        self.index["coverage"].setdefault("packaged_native_files", {})[name] = len(records)
        self.packaged_records.extend(records)
        return records

    def kotlin_documents(self, project):
        for module in ("cdk-android", "cdk-jvm"):
            report = project / module / "build/reports/sbom"
            evidence = read_json(report / "resolved.json")
            product = [graph for graph in evidence["graphs"] if graph["role"] == "product"]
            build = [graph for graph in evidence["graphs"] if graph["role"] == "build"]
            expected = {(f"{item['group']}/{item['name']}", item["version"])
                        for graph in product for item in graph["modules"]}
            if not product or not expected:
                raise SbomError(f"Gradle produced no independently resolved dependencies for {module}")
            bom = self.document(report / "bom.json", f"bindings/kotlin/{module}", "gradle-product")
            require_components(bom, expected, "Gradle independent resolution")
            self.evidence(report / "resolved.json", f"kotlin/{module}/dependency-resolution.json")
            for role, graphs in (("resolved", product), ("build", build)):
                source = self.generated_evidence(f"kotlin/{module}/{role}.bom.json", maven_bom(module + "-" + role, graphs))
                self.document(source, f"bindings/kotlin/{module}.{role}", "gradle-" + role)
                artifacts = []
                seen = set()
                natives = []
                for graph in graphs:
                    for artifact in graph["artifacts"]:
                        path = Path(artifact["file"])
                        owner = f"pkg:maven/{artifact['group']}/{artifact['name']}@{artifact['version']}"
                        checksum = digest(path)
                        if (owner, checksum) in seen:
                            continue
                        seen.add((owner, checksum))
                        artifacts.append({"owner": owner, "filename": path.name, "sha256": checksum})
                        if path.suffix.lower() in {".jar", ".aar"}:
                            natives.extend(archive_natives(path, owner, checksum, prefix=path.name + "!"))
                        if digest(path) != checksum:
                            raise SbomError(f"Resolved Gradle artifact changed: {path}")
                self.generated_evidence(f"kotlin/{module}/{role}-artifacts.json", artifacts)
                source = self.generated_evidence(f"kotlin/{module}/{role}-native-files.json", file_bom(module + "-" + role, natives))
                self.document(source, f"bindings/kotlin/{module}.{role}.native-files", "packaged-native-files")
                self.index["coverage"].setdefault("packaged_native_files", {})[module + "-" + role] = len(natives)
                unresolved = self.index.setdefault("unresolved_native_identities", [])
                known = {(item["owner"], item["sha256"]) for item in unresolved}
                for item in natives:
                    if (item["owner"], item["sha256"]) not in known:
                        unresolved.append({"owner": item["owner"], "path": item["path"], "sha256": item["sha256"],
                                           "reason": "Upstream component versions inside this vendor-native blob are not independently established."})
                        known.add((item["owner"], item["sha256"]))

    def rust(self, workspace, packages):
        self.staged_workspace = workspace
        lock = workspace / "Cargo.lock"
        before = digest(lock)
        if len(packages) == 1:
            self.args.source_root = self.root
            package = packages[0]
            metadata, consumer_lock = resolve_consumer(workspace, package, self.args, run)
            self.generated_evidence("cargo-metadata.json", metadata)
            self.evidence(lock, "Cargo.lock")
            self.evidence(consumer_lock, "consumer/Cargo.lock")
            self.evidence(consumer_lock.parent / "Cargo.toml", "consumer/Cargo.toml")
            self.evidence(consumer_lock.parent / "cargo-metadata.raw.json", "consumer/cargo-metadata.raw.json")
            roles = rust_dependency_roles(metadata, package)
            bom = resolved_bom(metadata, package, roles, lock)
            classify_rust_bom(bom, metadata, package)
            source = workspace / "resolved.cdx.json"
            write_json(source, bom)
            self.document(source, f"rust/{package['name']}", "rust-consumer-resolved")
            write_json(source, rust_build_bom(bom, metadata, package))
            self.document(source, f"rust/{package['name']}.build", "rust-build-dependencies")
            if digest(lock) != before:
                raise SbomError("Consumer resolution modified the original lockfile")
            self.index["configuration"]["rust_resolution"] = "isolated-consumer"
            self.index["configuration"]["cargo_metadata_view"] = "synthetic consumer root removed; raw response retained separately"
            return metadata
        metadata_args = ["cargo", "metadata", "--locked", "--format-version", "1",
                         "--filter-platform", self.args.target]
        options = cargo_options(self.args)[2:]
        metadata = json.loads(run(metadata_args + options, workspace))
        metadata_path = workspace / "sbom-metadata.json"
        write_json(metadata_path, metadata)
        self.evidence(metadata_path, "cargo-metadata.json")
        self.evidence(lock, "Cargo.lock")
        run(["cargo", "cyclonedx", "--format", "json", "--spec-version", "1.5",
             "--target-in-filename", *cargo_options(self.args)], workspace)
        if digest(lock) != before:
            raise SbomError("The Rust generator changed Cargo.lock in staging; refusing unlocked output")
        for package in packages:
            manifest = Path(package["manifest_path"]).relative_to(self.root)
            # cargo-cyclonedx 0.5.9 names files after the package, not 'bom.json'.
            source = workspace / manifest.parent / f"{package['name']}_{self.args.target}.cdx.json"
            if not source.is_file():
                raise SbomError(f"Rust generator did not produce {source.name}")
            bom = read_json(source)
            classify_rust_bom(bom, metadata, package)
            write_json(source, bom)
            self.document(source, f"rust/{package['name']}", "rust-resolved")
            build_source = source.with_name("build-dependencies.json")
            write_json(build_source, rust_build_bom(bom, metadata, package))
            self.document(build_source, f"rust/{package['name']}.build", "rust-build-dependencies")
        return metadata

    def language(self, language, staging):
        source = self.projects.get(language, self.root / "bindings" / language)
        project = copy_project(source, staging / language)
        expected = set()
        if language == "dart":
            # No build hooks are invoked by pub get. Keep the resulting lock as evidence.
            run(["dart", "pub", "get"], project)
            dependencies = run(["dart", "pub", "deps", "--json"], project)
            graph = json.loads(dependencies)
            expected = {(item["name"], item["version"]) for item in graph["packages"]
                        if item.get("kind") != "root"}
            write_json(project / "pub-deps.json", graph)
            self.evidence(project / "pub-deps.json", "dart/pub-deps.json")
            self.evidence(project / "pubspec.lock", "dart/pubspec.lock")
            self.evidence(project / "pubspec.yaml", "dart/pubspec.yaml")
        elif language == "swift":
            run(["swift", "package", "resolve"], project)
            graph = json.loads(run(["swift", "package", "show-dependencies", "--format", "json"], project))
            def visit(node):
                for child in node.get("dependencies", []):
                    expected.add((child["name"], child["version"]))
                    visit(child)
            visit(graph)
            write_json(project / "swift-deps.json", graph)
            self.evidence(project / "swift-deps.json", "swift/dependencies.json")
            if (project / "Package.resolved").exists():
                self.evidence(project / "Package.resolved", "swift/Package.resolved")
            self.evidence(project / "Package.swift", "swift/Package.swift")
        elif language == "go":
            environment = go_environment(self.args.target)
            run(["go", "mod", "download", "all"], project, environment)
            # -m all inventories all resolved modules, not only direct requirements.
            modules = run(["go", "list", "-m", "-json", "all"], project, environment)
            selected_modules = list(json_stream(modules))
            for item in selected_modules:
                if not item.get("Main"):
                    if "Replace" in item:
                        raise SbomError("Go module replacements require explicit SBOM support; refusing partial output")
            loaded = run(["go", "list", "-deps", "-json", "./..."], project, environment)
            packages = list(json_stream(loaded))
            if not packages:
                raise SbomError("Go project has no loadable generated packages; supply an assembled binding")
            for package in packages:
                module = package.get("Module", {})
                if module and not module.get("Main"):
                    expected.add((module["Path"], module["Version"]))
            (project / "modules.json").write_text(modules)
            (project / "packages.json").write_text(loaded)
            self.evidence(project / "modules.json", "go/modules.json")
            self.evidence(project / "packages.json", "go/packages.json")
            self.evidence(project / "go.mod", "go/go.mod")
            if (project / "go.sum").exists():
                self.evidence(project / "go.sum", "go/go.sum")
            version = run(["go", "env", "GOVERSION"], project, environment).strip()
            self.index["tools"]["go-resolved-toolchain"] = version
            source = self.generated_evidence("go/selected-modules.json", go_bom(selected_modules, packages, version))
            self.document(source, "bindings/go/selected-modules", "go-selected-modules-and-stdlib")
            cgo = [{key: item[key] for key in ("ImportPath", "CgoFiles", "CFiles", "CXXFiles", "HFiles",
                                             "CgoCFLAGS", "CgoCPPFLAGS", "CgoCXXFLAGS", "CgoLDFLAGS") if key in item}
                   for item in packages if item.get("CgoFiles")]
            self.generated_evidence("go/cgo-inputs.json", cgo)
            if self.args.build:
                run(["go", "build", "./..."], project, environment)
                package = project / "bindings/cdkffi"
                if not (package / "cdk_ffi.go").is_file():
                    raise SbomError("Go build verification requires the assembled CDK bindings/cdkffi package")
                shutil.copyfile(ROOT / "misc/sbom/go_smoke_test.go", package / "sbom_native_test.go")
                run(["go", "test", "./bindings/cdkffi", "-run", "^TestSbomNativeLoad$", "-count=1"], project, environment)
                self.index["coverage"]["go_package_compiles"] = True
                self.index["coverage"]["go_native_load_test"] = True
        else:
            wrapper = project / "gradlew"
            if not wrapper.is_file():
                raise SbomError("Kotlin project has no Gradle wrapper")
            self.index["tools"]["cyclonedx-gradle-plugin"] = "2.4.1"
            for relative in ("build.gradle.kts", "settings.gradle.kts", "gradle/wrapper/gradle-wrapper.properties",
                             "cdk-android/build.gradle.kts", "cdk-jvm/build.gradle.kts"):
                if (project / relative).is_file():
                    self.evidence(project / relative, "kotlin/" + relative)
            settings = project / "settings.gradle.kts"
            settings.write_text(settings.read_text() +
                                '\n// Expose the settings-script classpath to the isolated SBOM init script.\n'
                                'extensions.extraProperties["cdkSbomSettingsClasspath"] = buildscript.configurations.getByName("classpath")\n')
            self.evidence(ROOT / "misc/sbom/kotlin.init.gradle", "kotlin/collector.init.gradle")
            run(["bash", wrapper, "--no-daemon", "--init-script", ROOT / "misc/sbom/kotlin.init.gradle",
                 ":cdk-android:cdkSbomEvidence", ":cdk-jvm:cdkSbomEvidence"], project)
            self.kotlin_documents(project)
            self.packaged_files(list(native_files(project)), "kotlin-project", "cdk-ffi-kotlin", project)
            return
        output = project / "ecosystem.cdx.json"
        environment = go_environment(self.args.target) if language == "go" else dict(os.environ)
        environment["FETCH_LICENSE"] = "true"
        # Allow the Go collector to resolve imports only inside our copy.
        dependency_option = "--install-deps" if language == "go" else "--no-install-deps"
        run(["cdxgen", "--type", language, "--spec-version", "1.6", "--no-recurse",
             dependency_option, "--fail-on-error", "--output", output, project], project, environment)
        if language == "dart":
            generated = read_json(output)
            dart_dependency_graph(generated, graph)
            write_json(output, generated)
        bom = self.document(output, f"bindings/{language}/ecosystem", f"{language}-resolved")
        require_components(bom, expected, language)
        if language != "swift":
            self.packaged_files(list(native_files(project)), language + "-project", WRAPPERS[language], project)

    def build(self, package, metadata):
        command = ["cargo", "auditable", "build", "--locked", "--package", package["name"],
                   "--profile", self.args.profile, "--message-format", "json", *cargo_options(self.args)]
        if self.args.existing:
            output = ""
            records = None
        else:
            command.extend(cargo_build_targets(package))
            environment = dict(os.environ)
            if self.args.clean_build:
                self.cargo_target_directory = self.output / "clean-target"
                environment["CARGO_TARGET_DIR"] = str(self.cargo_target_directory)
                self.index["configuration"]["fresh_target_directory"] = True
            if self.args.linker_maps:
                directory = self.output / "link-capture"
                directory.mkdir()
                tool = real_linker(self.root, self.args.target)
                wrapper = directory / "linker"
                wrapper.write_text(f"#!{sys.executable}\nimport runpy, sys\nsys.path.insert(0, {str(ROOT / 'misc/sbom')!r})\nrunpy.run_module('link_evidence', run_name='__main__')\n")
                wrapper.chmod(0o700)
                environment.update({"CDK_SBOM_REAL_LINKER": str(tool), "CDK_SBOM_LINK_CAPTURE": str(directory),
                                    "CARGO_TARGET_" + self.args.target.upper().replace("-", "_") + "_LINKER": str(wrapper)})
                self.evidence(ROOT / "misc/sbom/link_evidence.py", "linker/collector.py")
                self.evidence(wrapper, "linker/wrapper.py")
            output = run(command, self.root, environment)
            records = build_records(output)
            self.generated_evidence("cargo-build.json", records)
            if self.args.linker_maps:
                maps = []
                for source in sorted((self.output / "link-capture").glob("*.json")):
                    record = read_json(source)
                    map_path = source.parent / record["map"]
                    if map_path.parent != source.parent or digest(map_path) != record["map_sha256"]:
                        raise SbomError("Linker map changed or escaped its capture directory")
                    linked = Path(record["output"])
                    record["final_link_output_sha256"] = digest(linked)
                    self.evidence(map_path, "linker/maps/" + map_path.name)
                    self.evidence(source, "linker/records/" + source.name)
                    maps.append(record)
                if not maps and any("bin" in target["kind"] or set(target.get("crate_types", [])).intersection({"cdylib", "dylib"}) for target in package["targets"]):
                    raise SbomError("No target linker maps were observed; use a fresh target directory for this build")
                self.generated_evidence("linker/outputs.json", maps)
                self.index["coverage"]["linker_maps"] = len(maps)
        artifacts = []
        for line in output.splitlines():
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("reason") == "compiler-artifact" and message.get("package_id") == package["id"]:
                if message.get("executable"):
                    artifacts.append(Path(message["executable"]))
                for filename in message.get("filenames", []):
                    if Path(filename).suffix in {".so", ".dylib", ".dll", ".a"}:
                        artifacts.append(Path(filename))
                    if Path(filename).suffix == ".rlib":
                        artifacts.append(Path(filename))
        artifacts.extend(Path(path).resolve() for path in self.args.artifact)
        if not artifacts:
            raise SbomError(f"Cargo reported no executable/native artifact for {package['name']}; "
                            "verify the build output or supply its path with --artifact PATH")
        roles = rust_dependency_roles(metadata, package)
        runtime = [item for item in metadata["packages"] if roles.get(item["id"]) == "runtime"]
        profile_directory = "debug" if self.args.profile == "dev" else self.args.profile
        if records is not None:
            # Metadata remains a conservative source inventory, even in an
            # isolated consumer (Cargo metadata enables dev resolution). Actual
            # target/host compiler records describe the observed build instead.
            runtime, host_packages = self.compilation_inventory(package, metadata, records, profile_directory)
            verbose = run(["rustc", "-vV"], self.root)
            self.generated_evidence("rust-toolchain.json", {"rustc": verbose, "cargo": self.index["tools"].get("cargo")})
            self.toolchain_inventory(package, verbose)
            host = next(line.removeprefix("host: ") for line in verbose.splitlines() if line.startswith("host: "))
            native = NativeCollector(metadata, host_packages,
                                     records, host, self.cargo_target_directory / profile_directory, self.root, run, host=True)
            document, provenance = native.collect_build_environment(digest(self.output / "evidence/cargo-build.json"))
            source = self.generated_evidence("native-build.json", document)
            self.generated_evidence("native-build-provenance.json", provenance)
            self.document(source, f"rust/{package['name']}.build-native", "native-build-environment")
        expected = {(item["name"], item["version"]) for item in runtime}
        linked = {item["links"] for item in runtime if item.get("links")}
        for number, artifact in enumerate(sorted(set(artifacts))):
            if not artifact.is_file():
                raise SbomError(f"Missing artifact: {artifact}")
            checksum = digest(artifact)
            entry = {"name": artifact.name, "sha256": checksum, "target": self.args.target,
                      "rust_document": f"rust/{package['name']}.cdx.json"}
            if records is not None and self.args.linker_maps and artifact.suffix not in {".rlib", ".a"}:
                matched = [record for record in maps if record["final_link_output_sha256"] == checksum]
                if not matched:
                    raise SbomError("The artifact is not bound to an observed target linker map")
                entry["linker_maps"] = ["evidence/linker/maps/" + record["map"] for record in matched]
            if records is not None:
                entry["compiled_document"] = f"rust/{package['name']}.compiled.cdx.json"
                entry["build_documents"] = [f"rust/{package['name']}.{kind}.cdx.json"
                                            for kind in ("build", "build-native", "toolchain")]
            self.index["artifacts"].append(entry)
            scanned = self.output / "evidence" / f"artifact-{number}.json"
            scan_name = f"artifacts/{number}-{artifact.name}"
            if artifact.suffix in {".rlib", ".a"}:
                if records is None:
                    raise SbomError("Static/Rust library inventories require observed Cargo build records")
                self.index["limitations"].append(
                    f"{artifact.name}: compilation/link-input inventory, not embedded binary verification; "
                    "system requirements are finalized by the consuming executable.")
                entry["verification"] = "observed-compilation-and-native-inputs"
            else:
                run(["syft", "scan", f"file:{artifact}", "--output", f"cyclonedx-json={scanned}"], self.root)
                bom = self.document(scanned, scan_name, "artifact-scan")
                require_components(bom, expected, "embedded Rust artifact")
                entry["verification"] = "embedded-rust-and-native-inputs"
                entry["scan_document"] = scan_name + ".cdx.json"
            if records is not None:
                # Cargo reports target and host artifacts separately, including
                # cached build scripts. Never infer native linkage from `links` alone.
                target_directory = self.cargo_target_directory
                native = NativeCollector(metadata, runtime, records, self.args.target,
                                         target_directory / self.args.target / profile_directory,
                                         self.root, run)
                native_bom, provenance = native.collect(artifact, checksum)
                source = self.generated_evidence(f"native-{number}.json", native_bom)
                self.generated_evidence(f"native-provenance-{number}.json", provenance)
                name = f"artifacts/native-{number}-{checksum[:12]}"
                self.document(source, name, "native-build-evidence")
                entry["native_documents"] = [name + ".cdx.json"]
            elif linked:
                self.native_evidence(linked, checksum, entry)
            if digest(artifact) != checksum:
                raise SbomError("Artifact changed during collection")

    def compilation_inventory(self, package, metadata, records, profile_directory):
        indexed = {item["id"]: item for item in metadata["packages"]}
        manifests = {}
        for item in metadata["packages"]:
            path = Path(item["manifest_path"])
            if not item.get("source"):
                path = self.root / path.relative_to(self.staged_workspace)
            manifests[str(path)] = item
        target_dir = self.cargo_target_directory / self.args.target / profile_directory
        host_dir = self.cargo_target_directory / profile_directory
        units = []
        contexts = {"target": {}, "host": {}}
        for record in records:
            if record["reason"] != "compiler-artifact" or "custom-build" in record["target"]["kind"]:
                continue
            item = indexed.get(record["package_id"]) or manifests.get(record.get("manifest_path"))
            if item is None:
                raise SbomError(f"Compiler reported a package absent from the resolved inventory: {record['package_id']}")
            paths = [Path(path).resolve() for path in record["filenames"]]
            context = "target" if any(path.is_relative_to(target_dir.resolve()) for path in paths) else "host"
            if context == "host" and not all(path.is_relative_to(host_dir.resolve()) for path in paths):
                raise SbomError(f"Compiler output outside the recorded host/target directories: {record['package_id']}")
            actual = dict(item, id=record["package_id"])
            contexts[context][actual["id"]] = actual
            units.append({"package_id": actual["id"], "resolved_id": item["id"], "context": context,
                          "features": record.get("features", []), "target": record["target"]["name"]})
        if not any(item["name"] == package["name"] for item in contexts["target"].values()):
            raise SbomError("The selected product has no observed target compilation unit")
        self.generated_evidence("compilation-units.json", units)
        source = read_json(self.output / f"rust/{package['name']}.cdx.json")
        present = {unit["resolved_id"] for unit in units}
        self.generated_evidence("resolved-not-compiled.json", [
            {"package_id": item["id"], "name": item["name"], "version": item["version"]}
            for item in metadata["packages"] if item["id"] not in present
        ])
        source["metadata"].setdefault("properties", []).append({
            "name": "cdk:rust:inventory-kind", "value": "observed host/target package set; not a compilation-unit edge attestation",
        })
        source["components"] = [item for item in source["components"] if item["bom-ref"] in present]
        for item in package_components(source):
            item["scope"] = "required"
            item.setdefault("properties", []).append({"name": "cdk:rust:observed-units", "value": json.dumps([
                unit for unit in units if unit["resolved_id"] == item["bom-ref"]], sort_keys=True)})
        root = source["metadata"]["component"]["bom-ref"]
        source["dependencies"] = [{"ref": root, "dependsOn": sorted(present - {root})}]
        path = self.generated_evidence("compiled-inventory.json", source)
        self.document(path, f"rust/{package['name']}.compiled", "observed-rust-compilation")
        self.index["coverage"]["compiled_packages"] = {key: len(value) for key, value in contexts.items()}
        return list(contexts["target"].values()), list(contexts["host"].values())

    def toolchain_inventory(self, package, verbose):
        fields = dict(line.split(": ", 1) for line in verbose.splitlines() if ": " in line)
        version = fields["release"]
        directory = Path(run(["rustc", "--print", "target-libdir", "--target", self.args.target], self.root).strip())
        files = [{"name": path.name, "sha256": digest(path)} for path in sorted(directory.glob("*.rlib"))]
        if not files:
            raise SbomError("No installed target Rust standard-library distribution was found")
        self.generated_evidence("rust-sysroot.json", {"target": self.args.target, "directory": str(directory), "files": files})
        root = "urn:cdk:toolchain:" + package["name"]
        components = [
            {"type": "application", "bom-ref": "rustc", "name": "rustc", "version": version, "scope": "required",
             "purl": f"pkg:generic/rust@{version}",
             "properties": [{"name": "cdk:rust:compiler-commit", "value": fields.get("commit-hash", "unknown")}]},
            {"type": "library", "bom-ref": "rust-std", "name": "rust-std", "version": version, "scope": "required",
             "purl": f"pkg:generic/rust-std@{version}",
             "properties": [{"name": "cdk:rust:target", "value": self.args.target},
                            {"name": "cdk:rust:coverage", "value": "installed target distribution; not exact linked objects"}]},
        ]
        cargo = re.match(r"cargo (\S+)", self.index["tools"].get("cargo", ""))
        if cargo:
            components.append({"type": "application", "bom-ref": "cargo", "name": "cargo",
                               "version": cargo.group(1), "scope": "required"})
        if "android" in self.args.target:
            ndk = os.environ.get("ANDROID_NDK_HOME") or os.environ.get("ANDROID_NDK_ROOT")
            if not ndk:
                raise SbomError("Android toolchain inventory requires ANDROID_NDK_HOME")
            properties = Path(ndk) / "source.properties"
            match = re.search(r"Pkg.Revision\s*=\s*([^\r\n]+)", properties.read_text())
            if not match:
                raise SbomError("Cannot identify the Android NDK revision")
            self.evidence(properties, "android-ndk-source.properties")
            components.append({"type": "application", "bom-ref": "android-ndk", "name": "Android NDK",
                               "version": match.group(1).strip(), "scope": "required"})
        source = self.generated_evidence("toolchain-bom.json", {
            "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
            "metadata": {"component": {"type": "application", "name": package["name"] + " toolchain", "bom-ref": root}},
            "components": components, "dependencies": [{"ref": root, "dependsOn": [item["bom-ref"] for item in components]}],
        })
        self.document(source, f"rust/{package['name']}.toolchain", "build-toolchain-distribution")

    def native_evidence(self, linked, checksum, entry):
        covered = set()
        for number, source in enumerate(self.args.native_bom):
            bom = read_json(Path(source))
            properties = {item["name"]: item["value"]
                          for item in bom.get("metadata", {}).get("properties", [])}
            if properties.get("cdk:artifact:sha256") != checksum:
                continue
            if properties.get("cdk:target") != self.args.target:
                raise SbomError("Native evidence target does not match the build")
            for component in components(bom):
                for prop in component.get("properties", []):
                    if prop["name"] == "cdk:rust:links" and component.get("version"):
                        covered.add(prop["value"])
            name = f"artifacts/native-{number}-{checksum[:12]}"
            self.document(Path(source).resolve(), name, "native-build-evidence")
            entry.setdefault("native_documents", []).append(name + ".cdx.json")
        if linked - covered:
            raise SbomError("Native library evidence is missing for Cargo links values: " +
                            ", ".join(sorted(linked - covered)) +
                             ". Supply artifact-bound --native-bom evidence; see docs/sbom.md.")

    def source_snapshot(self):
        names = run(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--",
                     "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "flake.nix", "flake.lock", ".cargo", "crates", "bindings", "misc/sbom"], self.root)
        files = []
        for name in sorted(set(names.split("\0")) - {""}):
            path = self.root / name
            if Path(name).name.startswith("test_") and name.startswith("misc/sbom/"):
                continue
            if path.is_symlink() or not path.resolve().is_relative_to(self.root.resolve()):
                raise SbomError("Source snapshot requires explicit provenance for symlinked inputs")
            if path.is_file():
                files.append({"path": name, "sha256": digest(path)})
        return files

    def collect(self):
        # No-deps discovery does not download dependencies or update the lockfile.
        metadata = json.loads(run(["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"], self.root))
        self.cargo_target_directory = Path(metadata.get("target_directory", self.root / "target"))
        packages, languages = selection(self.args.scope, metadata)
        languages = [language for language in languages if language not in self.args.exclude_language]
        if self.args.build and len(packages) != 1:
            raise SbomError("--build requires one crate or language, not all/bindings")
        if self.args.existing and (not self.args.build or not self.args.artifact):
            raise SbomError("--existing requires --build and at least one --artifact")
        if (self.args.clean_build or self.args.linker_maps) and (not self.args.build or self.args.existing):
            raise SbomError("--clean-build and --linker-maps require a newly observed --build")
        if not self.args.build and (self.args.artifact or self.args.native_bom):
            raise SbomError("--artifact and --native-bom require --build")
        self.preflight(languages)
        if not self.args.target:
            verbose = run(["rustc", "-vV"], self.root)
            self.args.target = next(line.removeprefix("host: ") for line in verbose.splitlines()
                                    if line.startswith("host: "))
        if not re.fullmatch(r"[A-Za-z0-9_.-]+", self.args.target):
            raise SbomError("Invalid target triple")
        if not self.args.profile:
            self.args.profile = "release-ffi" if packages[0]["name"].startswith("cdk-ffi") else "release"
        self.index["configuration"] = {"target": self.args.target, "profile": self.args.profile,
                                       "features": self.args.features,
                                       "default_features": not self.args.no_default_features,
                                       "existing_artifacts": self.args.existing,
                                       "excluded_languages": self.args.exclude_language}
        self.index["source"] = {"revision": run(["git", "rev-parse", "HEAD"], self.root).strip(),
                                "dirty": bool(run(["git", "status", "--porcelain"], self.root).strip())}
        self.index["coverage"] = {"rust_packages": [package["name"] for package in packages],
                                  "bindings": languages, "cargo_links_evidence_checked": False}
        self.output = self.args.output.resolve() if self.args.output else self.root / "target/sbom" / uuid.uuid4().hex
        self.output.mkdir(parents=True, exist_ok=False)
        try:
            snapshot = self.source_snapshot()
            source_file = self.generated_evidence("source-files.json", snapshot)
            self.index["source"]["manifest_sha256"] = digest(source_file)
            with tempfile.TemporaryDirectory(prefix=".staging-", dir=self.output) as temporary:
                staging = Path(temporary)
                workspace = staging / "workspace"
                workspace.mkdir()
                for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "README.md", "LICENSE.md"):
                    if not (self.root / name).is_file():
                        continue
                    shutil.copyfile(self.root / name, workspace / name)
                for name in ("rust-toolchain.toml", "flake.lock"):
                    if (self.root / name).is_file():
                        self.evidence(self.root / name, name)
                for name in ("crates", "bindings", ".cargo"):
                    if (self.root / name).exists():
                        copy_project(self.root / name, workspace / name)
                resolved = self.rust(workspace, packages)
                for language in languages:
                    self.language(language, staging)
                if self.args.build:
                    self.build(packages[0], resolved)
                    self.index["coverage"]["cargo_links_evidence_checked"] = True
                for number, distribution in enumerate(self.args.distribution):
                    path = Path(distribution).resolve(strict=True)
                    paths = list(native_files(path, include_archives=True)) if path.is_dir() else [path]
                    manifest = [{"path": str(item.relative_to(path)), "sha256": digest(item)}
                                for item in sorted(path.rglob("*")) if item.is_file()] if path.is_dir() else [
                                    {"path": path.name, "sha256": digest(path)}]
                    self.generated_evidence(f"distribution-{number}-manifest.json", manifest)
                    self.packaged_files(paths, f"distribution-{number}", packages[0]["name"], path if path.is_dir() else None)
                    self.index.setdefault("distributions", []).append({
                        "path": str(path), "manifest": f"evidence/distribution-{number}-manifest.json",
                        "native_document": f"packages/distribution-{number}.native-files.cdx.json",
                    })
                if self.args.build:
                    self.verify_packaged_artifacts()
                self.index["products"] = {
                    language: {
                        "rust_document": f"rust/{WRAPPERS[language]}.cdx.json",
                        "ecosystem_documents": [document["path"] for document in self.index["documents"]
                                                 if document["path"].startswith(f"bindings/{language}/")],
                        "packaged_native_documents": [document["path"] for document in self.index["documents"]
                                                      if document["path"].startswith(f"packages/{language}-project.")],
                    } for language in languages
                }
            for entry in self.index["documents"] + self.index["evidence"]:
                if digest(self.output / entry["path"]) != entry["sha256"]:
                    raise SbomError(f"Output changed after verification: {entry['path']}")
            if self.source_snapshot() != snapshot:
                raise SbomError("Source inputs changed during collection; refusing mixed-source provenance")
            self.index["assurance"] = assessment(self.index)
            require(self.index["assurance"], self.args.require_assurance)
            self.index["status"] = "complete"
            if not self.args.build:
                self.index["limitations"].append(
                    "Inventory only: native library versions and released binary contents are not verified."
                )
        except (SbomError, NativeError, OSError, ValueError, KeyError) as error:
            self.index["error"] = str(error)
            raise
        finally:
            self.index.setdefault("assurance", assessment(self.index))
            write_json(self.output / "index.json", self.index)
        print(self.output)

    def verify_packaged_artifacts(self):
        built = {artifact["sha256"] for artifact in self.index["artifacts"]}
        supplied = {}
        for path in self.args.native_bom:
            bom = read_json(Path(path))
            properties = {item["name"]: item["value"] for item in bom.get("metadata", {}).get("properties", [])}
            if properties.get("cdk:artifact:sha256") and properties.get("cdk:target"):
                self.document(Path(path), "packages/provenance-" + str(len(supplied)), "external-native-provenance")
                supplied[properties["cdk:artifact:sha256"]] = properties["cdk:target"]
        for record in self.packaged_records:
            if record["sha256"] in built:
                continue
            if record["sha256"] not in supplied:
                raise SbomError(f"Packaged native file {record['path']} does not match a verified build artifact. "
                                "Assemble the package from this build's exact bytes, or supply reviewed artifact-bound --native-bom provenance.")
            target = supplied[record["sha256"]]
            self.index["coverage"]["external_native_assertions"] = True
            architecture = {"aarch64": "aarch64", "x86_64": "x86_64", "i686": "x86", "armv7": "arm"}.get(target.split("-", 1)[0])
            if record["architecture"] != architecture:
                raise SbomError(f"Packaged native provenance architecture does not match {record['path']}")
        self.index["coverage"]["packaged_native_build_hashes_checked"] = True


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("scope", nargs="?", default="all", help="all, rust, bindings, a language or a workspace crate")
    result.add_argument("--build", action="store_true", help="build and require artifact/native evidence")
    result.add_argument("--existing", action="store_true", help="verify --artifact files without rebuilding")
    result.add_argument("--clean-build", action="store_true", help="use a fresh target directory in this run; never erase existing caches")
    result.add_argument("--linker-maps", action="store_true", help="observe the real GNU/Android target linker and retain its maps")
    result.add_argument("--target", help="Rust target triple (default: rustc host)")
    result.add_argument("--profile", help="Cargo profile (default: release-ffi for FFI, release otherwise)")
    result.add_argument("--features", action="append", default=[])
    result.add_argument("--no-default-features", action="store_true")
    result.add_argument("--project", action="append", default=[], metavar="LANGUAGE=PATH",
                         help="assembled binding project (required for generated Go/Swift manifests)")
    result.add_argument("--exclude-language", action="append", default=[], choices=LANGUAGES,
                        help="explicitly omit an ecosystem, for example swift on Linux (recorded in the index)")
    result.add_argument("--output", type=Path, help="new output directory; never overwrites an existing run")
    result.add_argument("--artifact", action="append", default=[], help="additional final artifact to verify")
    result.add_argument("--native-bom", action="append", default=[], help="artifact-bound native CycloneDX evidence")
    result.add_argument("--distribution", action="append", default=[], metavar="PATH",
                         help="inventory native files in a final AAR/JAR/ZIP or assembled package directory")
    result.add_argument("--require-assurance", action="append", choices=DIMENSIONS, default=[],
                        help="fail unless this specific evidence assurance is satisfied; missing identities stay partial")
    return result


def main():
    try:
        Collector(parser().parse_args()).collect()
    except (SbomError, NativeError, OSError, ValueError, KeyError) as error:
        print(f"SBOM incomplete: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
