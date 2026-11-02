"""Native link-input provenance, without executing the artifact or guessing versions."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import tarfile
import tomllib

from package_inventory import binary_identity


ANDROID_TARGETS = {
    "aarch64-linux-android": ("aarch64-linux-android", "aarch64"),
    "armv7-linux-androideabi": ("arm-linux-androideabi", "arm"),
    "i686-linux-android": ("i686-linux-android", "x86"),
    "x86_64-linux-android": ("x86_64-linux-android", "x86_64"),
}


class NativeError(Exception):
    """Native coverage cannot be established from the available evidence."""


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def build_records(output):
    """Keep Cargo evidence, but not arbitrary compiler output or build-script env."""
    fields = {
        "compiler-artifact": ("package_id", "manifest_path", "target", "features", "filenames", "executable", "fresh"),
        "build-script-executed": ("package_id", "linked_libs", "linked_paths", "out_dir"),
        "build-finished": ("success",),
    }
    records = []
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if not isinstance(message, dict) or message.get("reason") not in fields:
            continue
        reason = message["reason"]
        records.append({"reason": reason, **{key: message[key] for key in fields[reason] if key in message}})
    if not records or records[-1] != {"reason": "build-finished", "success": True}:
        raise NativeError("Cargo did not supply a complete, successful JSON build record")
    return records


def verified_registry_source(package, lock):
    """Compare the cached .crate to Cargo.lock and its contents to unpacked sources."""
    matches = [entry for entry in lock["package"]
               if all(entry.get(key) == package.get(key) for key in ("name", "version", "source"))]
    if len(matches) != 1 or not matches[0].get("checksum"):
        raise NativeError(f"No locked registry checksum for native provider {package['name']}")
    source = Path(package["manifest_path"]).parent.resolve()
    archive = source.parents[2] / "cache" / source.parent.name / f"{package['name']}-{package['version']}.crate"
    if not archive.is_file() or sha256(archive) != matches[0]["checksum"]:
        raise NativeError(f"Missing or modified locked source archive for {package['name']}")
    files = {}
    prefix = f"{package['name']}-{package['version']}"
    with tarfile.open(archive, "r:gz") as cargo_archive:
        for member in cargo_archive:
            parts = PurePosixPath(member.name).parts
            if not parts or parts[0] != prefix or ".." in parts or PurePosixPath(member.name).is_absolute():
                raise NativeError("Invalid path in Cargo source archive")
            if member.isdir():
                continue
            if not member.isfile():
                raise NativeError("Unsupported non-regular file in native provider archive")
            relative = str(PurePosixPath(*parts[1:]))
            actual = source / relative
            if actual.is_symlink() or not actual.resolve().is_relative_to(source) or not actual.is_file():
                raise NativeError(f"Missing/unsafe native source: {package['name']}/{relative}")
            expected = hashlib.file_digest(cargo_archive.extractfile(member), "sha256").hexdigest()
            if sha256(actual) != expected:
                raise NativeError(f"Modified native source: {package['name']}/{relative}")
            files[relative] = expected
    extra = {str(path.relative_to(source)) for path in source.rglob("*")
             if path.is_file() or path.is_symlink()} - set(files) - {".cargo-ok"}
    if extra:
        raise NativeError(f"Untracked native source files in {package['name']}: {', '.join(sorted(extra)[:5])}")
    if sha256(archive) != matches[0]["checksum"]:
        raise NativeError("Source archive changed during verification")
    return source, {"archive": str(archive), "sha256": matches[0]["checksum"],
                    "verified_files": len(files),
                    "source_tree_sha256": hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()}


def version_match(source, relative, pattern):
    path = source / relative
    match = re.search(pattern, path.read_text())
    if not match:
        raise NativeError(f"Cannot establish upstream version from {path}")
    return match.group(1), {"path": str(path), "sha256": sha256(path)}


def nix_identity(data, store):
    """Handle legacy derivations and modern structured attributes, not path names."""
    entries = data.get("derivations", data)
    if len(entries) != 1:
        raise NativeError("Ambiguous Nix derivation identity")
    derivation = next(iter(entries.values()))
    attributes = derivation.get("structuredAttrs") or derivation["env"]
    outputs = {output["path"] for output in derivation["outputs"].values() if output.get("path")}
    if not ({str(store), store.name} & outputs) or not attributes.get("pname") or not attributes.get("version"):
        raise NativeError(f"Nix derivation does not identify shared-library output {store}")
    return attributes["pname"], attributes["version"]


def elf_search_paths(output, origin):
    tags = re.findall(r"\((RUNPATH|RPATH)\).*?\[([^\]]*)\]", output)
    # DT_RUNPATH supersedes DT_RPATH, even when its complete value is empty.
    selected = [value for tag, value in tags if tag == "RUNPATH"]
    if not selected:
        selected = [value for tag, value in tags if tag == "RPATH"]
    if len(selected) > 1:
        raise NativeError("Ambiguous duplicate ELF search-path tags")
    directories = []
    for paths in selected:
        # glibc decompose_rpath ignores an entirely empty value. Empty entries
        # inside a nonempty list mean CWD and must still be rejected.
        if paths == "":
            continue
        for directory in paths.split(":"):
            expanded = directory.replace("${ORIGIN}", str(origin)).replace("$ORIGIN", str(origin))
            if not expanded or "$" in expanded or not Path(expanded).is_absolute():
                raise NativeError(f"Unsupported/unsafe shared-library search path: {directory}")
            directories.append(Path(expanded))
    return directories


def check_link_overrides(root, target):
    """Unobserved linker overrides can select inputs outside Cargo's link record."""
    variables = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS",
                 "CARGO_TARGET_" + target.upper().replace("-", "_") + "_RUSTFLAGS")
    values = [(name, os.environ.get(name, "")) for name in variables]
    config_dirs = [directory / ".cargo" for directory in (root, *root.parents)]
    config_dirs.append(Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")))
    for directory in set(config_dirs):
        for name in ("config", "config.toml"):
            path = directory / name
            if not path.is_file():
                continue
            config = tomllib.loads(path.read_text())
            tables = [config.get("build", {})]
            tables.extend(value for key, value in config.get("target", {}).items()
                          if key == target or key.startswith("cfg("))
            for table in tables:
                flags = table.get("rustflags", [])
                values.append((str(path), " ".join(flags) if isinstance(flags, list) else flags))
    for origin, flags in values:
        if target in ANDROID_TARGETS:
            for alignment in ("max-page-size", "common-page-size"):
                flags = flags.replace(f"-C link-arg=-Wl,-z,{alignment}=16384", "")
        if re.search(r"(?:^|[\s\x1f])-(?:l|L)|link-args?\s*=|crt-static", flags):
            raise NativeError(f"Native linker override in {origin} requires additional linker provenance")


class NativeCollector:
    def __init__(self, metadata, runtime, records, target, profile_dir, root, runner, host=False):
        if not records or records[-1] != {"reason": "build-finished", "success": True}:
            raise NativeError("Native classification requires a complete successful Cargo build record")
        self.metadata = metadata
        self.runtime = runtime
        self.records = records
        self.target = target
        self.profile_dir = profile_dir.resolve()
        self.root = root
        self.run = runner
        self.host = host
        self.lock = tomllib.loads((root / "Cargo.lock").read_text())
        self.input_files = {}
        self.library_cache = {}
        self.dynamic_cache = {}
        self.components = []
        self.edges = []
        self.providers = []
        self.compiled = {}
        self.scripts = {}
        for record in records:
            if record["reason"] == "compiler-artifact":
                kinds = set(record["target"]["kind"])
                excluded = {"custom-build", "example", "test", "bench"}
                if not host:
                    excluded.add("proc-macro")
                if kinds.intersection(excluded):
                    continue
                if any(Path(path).resolve().is_relative_to(self.profile_dir) for path in record["filenames"]):
                    self.compiled[record["package_id"]] = record
            elif record["reason"] == "build-script-executed":
                if Path(record["out_dir"]).resolve().is_relative_to(self.profile_dir):
                    if record["package_id"] in self.scripts and self.scripts[record["package_id"]] != record:
                        raise NativeError("Ambiguous target build-script evidence")
                    self.scripts[record["package_id"]] = record

    def file_evidence(self, path):
        path = path.resolve(strict=True)
        checksum = sha256(path)
        self.input_files[str(path)] = checksum
        return {"path": str(path), "sha256": checksum}

    def source_component(self, package):
        component = {"type": "library", "bom-ref": package["id"], "name": package["name"],
                     "version": package["version"], "purl": f"pkg:cargo/{package['name']}@{package['version']}",
                     "properties": [{"name": "cdk:rust:links", "value": package.get("links") or ""}]}
        if package.get("license"):
            component["licenses"] = [{"expression": package["license"]}]
        self.components.append(component)
        return component

    def bundled(self, package, script, features):
        source, provenance = verified_registry_source(package, self.lock)
        self.file_evidence(Path(provenance["archive"]))
        provider = self.source_component(package)
        record = {"package_id": package["id"], "links": package.get("links"), "source": provenance,
                  "classification": "bundled-native-link-input", "archives": []}
        name = package["name"]
        if name == "libsqlite3-sys" and "bundled" in features and "bundled-sqlcipher" not in features:
            native_name = "sqlite"
            version, evidence = version_match(source, "sqlite3/sqlite3.h", r'#define\s+SQLITE_VERSION\s+"([^"]+)"')
            libraries = {"sqlite3"}
        elif name == "zstd-sys":
            native_name = "zstd"
            parts = [version_match(source, "zstd/lib/zstd.h", rf"#define\s+ZSTD_VERSION_{part}\s+(\d+)")[0]
                     for part in ("MAJOR", "MINOR", "RELEASE")]
            version = ".".join(parts)
            evidence = self.file_evidence(source / "zstd/lib/zstd.h")
            libraries = {"zstd"}
        elif name == "aws-lc-sys":
            native_name = "AWS-LC"
            version, evidence = version_match(source, "aws-lc/include/openssl/base.h",
                                              r'#define\s+AWSLC_VERSION_NUMBER_STRING\s+"([^"]+)"')
            libraries = {package["links"] + "_crypto"}
        elif name == "secp256k1-sys":
            native_name = "libsecp256k1"
            parts = [version_match(source, "depend/secp256k1/configure.ac",
                                   rf"define\(_PKG_VERSION_{part},\s*(\d+)\)")[0]
                     for part in ("MAJOR", "MINOR", "PATCH")]
            version = ".".join(parts)
            evidence = self.file_evidence(source / "depend/secp256k1/configure.ac")
            record["upstream_revision"] = (source / "depend/secp256k1-HEAD-revision.txt").read_text().strip()
            libraries = {"secp256k1"}
        elif name == "ring":
            # ring's C/assembly is part of its versioned distribution, not a
            # separately released 'ring_core' product with an invented version.
            native_name, version = None, None
            evidence = self.file_evidence(source / "Cargo.toml")
            libraries = {package["links"], package["links"] + "_test"}
            provider["properties"].append({"name": "cdk:native:composition", "value": "ring distribution includes C/assembly"})
        else:
            raise NativeError(f"No reviewed bundled-source adapter for {name}; native provenance is required")
        expected_flags = {"static=" + library for library in libraries}
        if set(script["linked_libs"]) != expected_flags:
            raise NativeError(f"Unexpected native link flags for {name}: {script['linked_libs']}")
        out = Path(script["out_dir"]).resolve()
        search = [Path(path.removeprefix("native=")).resolve() for path in script["linked_paths"]]
        if search != [out]:
            raise NativeError(f"Cannot prove bundled archive resolution for {name}")
        for library in sorted(libraries):
            archive = out / f"lib{library}.a"
            if not archive.is_file() or not archive.resolve().is_relative_to(out):
                raise NativeError(f"Missing/unsafe native link archive: {archive}")
            with archive.open("rb") as content:
                if content.read(8) != b"!<arch>\n":
                    raise NativeError(f"Unsupported native archive format: {archive}")
            record["archives"].append(self.file_evidence(archive))
        self.file_evidence(Path(evidence["path"]))
        record["version_evidence"] = evidence
        if native_name:
            reference = f"urn:cdk:native:{name}:{version}"
            native = {"type": "library", "bom-ref": reference, "name": native_name, "version": version,
                      "purl": f"pkg:generic/{native_name}@{version}",
                      "properties": [{"name": "cdk:source:bundled-in", "value": provider["purl"]},
                                     {"name": "cdk:source:archive-sha256", "value": provenance["sha256"]}]}
            if native_name == "sqlite":
                native["cpe"] = f"cpe:2.3:a:sqlite:sqlite:{version}:*:*:*:*:*:*:*"
            self.components.append(native)
            self.edges.append({"ref": provider["bom-ref"], "dependsOn": [reference]})
        self.providers.append(record)

    def providers_from_build(self):
        candidates = {package["id"]: package for package in self.runtime if package.get("links")}
        for package in self.runtime:
            if self.scripts.get(package["id"], {}).get("linked_libs"):
                candidates[package["id"]] = package
        for identifier, package in sorted(candidates.items()):
            if identifier not in self.compiled:
                self.providers.append({"package_id": identifier, "links": package.get("links"),
                                       "classification": "resolved-only-not-compiled"})
                continue
            script = self.scripts.get(identifier)
            if script is None:
                raise NativeError(f"Missing target build-script evidence for {package['name']}")
            if script["linked_libs"]:
                self.bundled(package, script, self.compiled[identifier].get("features", []))
                continue
            if package["name"] not in {"rayon-core", "aws-lc-rs", "defmt", "prettyplease"}:
                raise NativeError(f"Unclassified Cargo links declaration: {package['name']}")
            source, provenance = verified_registry_source(package, self.lock)
            self.source_component(package)
            record = {"package_id": identifier, "links": package["links"], "source": provenance,
                      "classification": "rust-link-coordination-no-native-library"}
            if package["name"] == "defmt":
                record["linker_script"] = self.file_evidence(Path(script["out_dir"]) / "defmt.x")
                record["classification"] = "rust-linker-script"
            self.providers.append(record)
        if any(package["name"] == "aws-lc-rs" and package["id"] in self.compiled for package in self.runtime):
            if not any("aws-lc-sys@" in record["package_id"] and record["classification"] == "bundled-native-link-input"
                       for record in self.providers):
                raise NativeError("AWS-LC wrapper has no verified underlying native provider")

    def nix_library(self, path):
        path = path.resolve(strict=True)
        if path in self.library_cache:
            return self.library_cache[path]
        if not str(path).startswith("/nix/store/"):
            raise NativeError(f"No installed-package provenance adapter for shared library {path}")
        store = Path(*path.parts[:4])
        deriver = self.run(["nix-store", "--query", "--deriver", store], self.root).strip()
        if not deriver.endswith(".drv"):
            raise NativeError(f"No Nix derivation for {path}")
        data = json.loads(self.run(["nix", "derivation", "show", deriver], self.root))
        name, version = nix_identity(data, store)
        reference = "urn:cdk:nix:" + store.name
        if not any(component["bom-ref"] == reference for component in self.components):
            self.components.append({"type": "library", "bom-ref": reference, "name": name,
                                    "version": version,
                                    "purl": f"pkg:generic/{name}@{version}",
                                    "properties": [{"name": "cdk:nix:store-path", "value": str(store)},
                                                   {"name": "cdk:nix:derivation", "value": deriver}]})
        result = reference, self.file_evidence(path)
        self.library_cache[path] = result
        return result

    def shared_libraries(self, artifact):
        with artifact.open("rb") as source:
            if source.read(4) != b"\x7fELF":
                raise NativeError("Automatic native inspection currently requires ELF artifacts")
        pending = [artifact.resolve()]
        visited = set()
        known = {}
        dynamic = []
        while pending:
            current = pending.pop(0)
            if current in visited:
                continue
            visited.add(current)
            if current not in self.dynamic_cache:
                self.dynamic_cache[current] = self.run(["readelf", "--dynamic", "--wide", current], self.root)
            output = self.dynamic_cache[current]
            needed = re.findall(r"\(NEEDED\).*?\[([^\]]+)\]", output)
            directories = elf_search_paths(output, current.parent)
            for name in needed:
                if not re.fullmatch(r"[A-Za-z0-9_.+-]+", name):
                    raise NativeError(f"Unsupported shared-library identity: {name}")
                found = next((directory / name for directory in directories if (directory / name).is_file()), None)
                if found is None:
                    found = known.get(name)
                if found is None:
                    raise NativeError(f"Cannot resolve shared-library requirement {name} from recorded ELF paths")
                found = found.resolve()
                if name in known and known[name] != found:
                    raise NativeError(f"Conflicting shared-library resolutions for {name}")
                known[name] = found
                reference, evidence = self.nix_library(found)
                dynamic.append({"required_by": str(current), "soname": name, "component": reference, **evidence})
                pending.append(found)
        return dynamic

    def android_libraries(self, artifact):
        """Record Android API requirements, never mislabel NDK stubs as shipped libc."""
        ndk = os.environ.get("ANDROID_NDK_HOME") or os.environ.get("ANDROID_NDK_ROOT")
        linker = os.environ.get("CARGO_TARGET_" + self.target.upper().replace("-", "_") + "_LINKER", "")
        match = re.search(r"(\d+)-clang(?:\+\+)?$", linker)
        api = match.group(1) if match else None
        if not ndk or not api:
            raise NativeError("Android native inventory requires ANDROID_NDK_HOME and an explicit API-qualified NDK linker")
        ndk = Path(ndk).resolve(strict=True)
        if not Path(linker).resolve(strict=True).is_relative_to(ndk):
            raise NativeError("Android linker does not belong to the recorded NDK")
        revision, evidence = version_match(ndk, "source.properties", r"Pkg.Revision\s*=\s*([^\r\n]+)")
        self.file_evidence(Path(evidence["path"]))
        system_libs = json.loads((ndk / "meta/system_libs.json").read_text())
        self.file_evidence(ndk / "meta/system_libs.json")
        triple, architecture = ANDROID_TARGETS[self.target]
        with artifact.open("rb") as source:
            if binary_identity(source.read(65536))["architecture"] != architecture:
                raise NativeError("Android artifact architecture does not match the requested target")
        needed = re.findall(r"\(NEEDED\).*?\[([^\]]+)\]",
                            self.run(["readelf", "--dynamic", "--wide", artifact], self.root))
        requirements = []
        for name in needed:
            if name not in system_libs or int(system_libs[name]) > int(api):
                raise NativeError(f"Unknown/non-platform Android shared library {name}; packaged provider evidence is required")
            candidates = list((ndk / "toolchains/llvm/prebuilt").glob(f"*/sysroot/usr/lib/{triple}/{api}/{name}"))
            if len(candidates) != 1:
                raise NativeError(f"Cannot identify the NDK API stub for {name} at API {api}")
            stub = self.file_evidence(candidates[0])
            reference = f"urn:cdk:android-api:{api}:{architecture}:{name}"
            self.components.append({"type": "library", "bom-ref": reference, "name": name, "scope": "required",
                                    "properties": [
                                        {"name": "cdk:android:provision", "value": "device-platform-not-bundled"},
                                        {"name": "cdk:android:min-api", "value": api},
                                        {"name": "cdk:android:ndk-revision", "value": revision.strip()},
                                        {"name": "cdk:android:stub-sha256", "value": stub["sha256"]},
                                    ]})
            requirements.append({"soname": name, "component": reference, "api": api, "ndk_revision": revision.strip(),
                                 "stub": stub, "runtime_version": "provided by deployment device; not inferred from NDK"})
        return requirements

    def collect(self, artifact, checksum):
        if self.target not in {"x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", *ANDROID_TARGETS}:
            raise NativeError("Automatic native provenance supports Linux GNU and Android targets only")
        check_link_overrides(self.root, self.target)
        # One package may emit both its library and multiple binaries. Do not
        # lose earlier filenames when its last compiler-artifact replaces a unit.
        reported = {Path(path).resolve() for record in self.records
                    if record["reason"] == "compiler-artifact" and
                    not set(record["target"]["kind"]).intersection({"proc-macro", "custom-build"})
                    for path in record["filenames"] if Path(path).resolve().is_relative_to(self.profile_dir)}
        if artifact.resolve() not in reported:
            raise NativeError("Artifact was not reported by this Cargo build; final post-processing needs separate provenance")
        self.providers_from_build()
        if artifact.suffix in {".a", ".rlib"}:
            with artifact.open("rb") as source:
                if source.read(8) != b"!<arch>\n":
                    raise NativeError("Unsupported static/Rust library archive format")
            dynamic = []
        else:
            dynamic = self.android_libraries(artifact) if self.target in ANDROID_TARGETS else self.shared_libraries(artifact)
        return self.documents(artifact.name, checksum, dynamic)

    def collect_build_environment(self, checksum):
        if not self.host:
            raise NativeError("Build-environment native inventory requires host compilation context")
        self.providers_from_build()
        dynamic = []
        tools = set()
        for record in self.records:
            if record["reason"] != "compiler-artifact":
                continue
            candidates = ([record["executable"]] if record.get("executable") else [])
            if "proc-macro" in record["target"]["kind"] or "custom-build" in record["target"]["kind"]:
                candidates.extend(record["filenames"])
            for value in candidates:
                path = Path(value).resolve()
                if path.is_relative_to(self.profile_dir) and path.is_file():
                    tools.add(path)
        for tool in sorted(tools):
            self.file_evidence(tool)
            dynamic.extend(self.shared_libraries(tool))
        bom, evidence = self.documents("Cargo host build environment", checksum, dynamic)
        evidence["host_tools"] = [{"path": str(path), "sha256": self.input_files[str(path)]} for path in sorted(tools)]
        bom["metadata"]["component"]["type"] = "application"
        bom["metadata"]["component"].pop("hashes", None)
        bom["metadata"]["properties"][0]["name"] = "cdk:cargo-build-record:sha256"
        evidence["build_record_sha256"] = evidence.pop("artifact_sha256")
        return bom, evidence

    def documents(self, name, checksum, dynamic):
        for path, checksum_at_start in self.input_files.items():
            if sha256(Path(path)) != checksum_at_start:
                raise NativeError(f"Native input changed during collection: {path}")
        subject = "urn:cdk:artifact:" + checksum
        refs = {component["bom-ref"] for component in self.components}
        edges = {edge["ref"]: edge["dependsOn"] for edge in self.edges}
        bom = {
            "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
            "metadata": {"component": {"type": "file", "bom-ref": subject, "name": name,
                                         "hashes": [{"alg": "SHA-256", "content": checksum}]},
                         "properties": [{"name": "cdk:artifact:sha256", "value": checksum},
                                        {"name": "cdk:target", "value": self.target}]},
            "components": self.components,
            "dependencies": [{"ref": subject, "dependsOn": sorted(refs)}] +
                            [{"ref": reference, "dependsOn": edges.get(reference, [])} for reference in sorted(refs)],
        }
        evidence = {"target": self.target, "artifact_sha256": checksum, "providers": self.providers,
                    "shared_libraries": dynamic,
                    "limitations": ["Trusted Cargo build/cache provenance, not a reproducible-build attestation.",
                                    "ELF default path resolution; runtime overrides and dlopen plugins require deployment inventory."]}
        return bom, evidence
