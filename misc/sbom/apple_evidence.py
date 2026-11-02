"""Mach-O build-input evidence; SDK stubs are not deployed library identities."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import struct


APPLE_TARGETS = {
    "aarch64-apple-darwin": ("macosx", "arm64", 1),
    "x86_64-apple-darwin": ("macosx", "x86_64", 1),
    "aarch64-apple-ios": ("iphoneos", "arm64", 2),
    "aarch64-apple-ios-sim": ("iphonesimulator", "arm64", 7),
    "x86_64-apple-ios": ("iphonesimulator", "x86_64", 7),
}


def packed_version(value):
    return f"{value >> 16}.{(value >> 8) & 255}.{value & 255}"


def macho_loads(path, target):
    """Read bounded thin 64-bit load commands without executing the image."""
    _, architecture, platform = APPLE_TARGETS[target]
    with path.open("rb") as source:
        header = source.read(32)
        if len(header) != 32 or header[:4] != b"\xcf\xfa\xed\xfe":
            raise ValueError("Apple inspection requires a thin little-endian 64-bit Mach-O image")
        _, cpu, _, filetype, count, size, _, _ = struct.unpack("<8I", header)
        if cpu != {"arm64": 0x100000c, "x86_64": 0x1000007}[architecture]:
            raise ValueError("Mach-O architecture does not match the requested target")
        if filetype not in {2, 6, 8} or count > size // 8 or size > 16 * 1024 * 1024:
            raise ValueError("Unsupported/malformed Mach-O executable load commands")
        commands = source.read(size)
    if len(commands) != size:
        raise ValueError("Truncated Mach-O load commands")
    result = {"imports": [], "rpaths": [], "platform": None, "minimum_os": None, "sdk": None}
    offset = 0
    for _ in range(count):
        if offset + 8 > size:
            raise ValueError("Truncated Mach-O load command")
        kind, length = struct.unpack_from("<2I", commands, offset)
        if length < 8 or length % 8 or offset + length > size:
            raise ValueError("Invalid Mach-O load command size")
        command = commands[offset:offset + length]
        if kind in {0xc, 0xd, 0x80000018, 0x8000001f, 0x20, 0x80000023, 0x8000001c}:
            minimum = 12 if kind == 0x8000001c else 24
            if length < minimum:
                raise ValueError("Truncated Mach-O path command")
            start = struct.unpack_from("<I", command, 8)[0]
            if start < minimum or start >= length or b"\0" not in command[start:]:
                raise ValueError("Invalid Mach-O path string")
            name = command[start:].split(b"\0", 1)[0].decode("utf-8")
            if not name or any(ord(char) < 32 for char in name):
                raise ValueError("Unsafe Mach-O path")
            if kind == 0x8000001c:
                result["rpaths"].append(name)
            elif kind != 0xd:  # LC_ID_DYLIB names this image, not a dependency.
                result["imports"].append({"install_name": name, "load_command": hex(kind)})
        elif kind in {0x32, 0x24}:
            if result["platform"] is not None:
                raise ValueError("Ambiguous Mach-O platform commands")
            if kind == 0x32:
                if length < 24:
                    raise ValueError("Truncated Mach-O build version")
                actual, minimum_os, sdk, tools = struct.unpack_from("<4I", command, 8)
                if length != 24 + tools * 8:
                    raise ValueError("Invalid Mach-O build tools")
            else:  # Legacy LC_VERSION_MIN_MACOSX is unambiguous; legacy iOS is not.
                if length != 16:
                    raise ValueError("Invalid legacy Mach-O version")
                actual = 1
                minimum_os, sdk = struct.unpack_from("<2I", command, 8)
            result.update(platform=actual, minimum_os=packed_version(minimum_os), sdk=packed_version(sdk))
        offset += length
    if offset != size or result["platform"] != platform:
        raise ValueError("Missing/mismatched Mach-O platform for the requested target")
    return result


def platform_path(name):
    """Only canonical public Apple platform install names select SDK requirements."""
    path = PurePosixPath(name)
    if str(path) != name or ".." in path.parts:
        return False
    return (name.startswith("/usr/lib/") or name.startswith("/System/Library/Frameworks/"))


def stub_identity(path, target):
    """Support explicit TAPI v4/v5 identities, not heuristic version inference."""
    text = path.read_text()
    sdk, arch, _ = APPLE_TARGETS[target]
    suffix = {"macosx": "macos", "iphoneos": "ios", "iphonesimulator": "ios-simulator"}[sdk]
    expected = f"{arch}-{suffix}"
    if text.lstrip().startswith("{"):
        document = json.loads(text)
        library = document.get("main_library", {})
        targets = {entry["target"] for entry in library.get("target_info", [])}
        names = [entry["name"] for entry in library.get("install_names", [])
                 if expected in entry.get("targets", [expected])]
        if document.get("tapi_tbd_version") != 5 or expected not in targets or len(names) != 1:
            raise ValueError(f"Unsupported/mismatched Apple SDK JSON stub: {path}")
        return names[0]
    # A file can contain multiple YAML documents for re-exported sublibraries.
    # Only the first library's header identifies this stub.
    header = re.split(r"(?m)^(?:exports|reexports|reexported-libraries|allowable-clients):", text)[0]
    header = re.split(r"(?m)^---", header, maxsplit=2)
    header = header[1] if len(header) > 1 else header[0]
    version = re.findall(r"(?m)^tbd-version:\s*(\d+)\s*$", header)
    names = re.findall(r"(?m)^install-name:\s*['\"]?([^'\"\s]+)['\"]?\s*$", header)
    targets = re.findall(r"(?ms)^targets:\s*\[([^\]]+)\]", header)
    if version != ["4"] or len(names) != 1 or len(targets) != 1:
        raise ValueError(f"Unsupported/ambiguous Apple SDK stub header: {path}")
    if expected not in {item.strip().strip("'\"") for item in targets[0].split(",")}:
        raise ValueError(f"Apple SDK stub does not support the requested target: {path}")
    return names[0]


class AppleNative:
    """Adapter around NativeCollector's hashed inputs and component graph."""

    def __init__(self, collector):
        self.collector = collector
        self.sdk = None
        self.sysroot = None
        self.requirements = {}
        self.images = {}

    def sdk_context(self):
        if self.sdk is None:
            c = self.collector
            name = APPLE_TARGETS[c.target][0]
            developer = Path(os.environ.get("DEVELOPER_DIR", ""))
            if not developer.is_absolute() or not developer.is_dir():
                raise ValueError("Apple native evidence requires an explicit Xcode DEVELOPER_DIR")
            command = ["/usr/bin/xcrun", "--sdk", name]
            path = Path(c.run([*command, "--show-sdk-path"], c.root).strip()).resolve(strict=True)
            if not path.is_relative_to(developer.resolve()) or not path.is_dir():
                raise ValueError("Selected Apple SDK does not belong to the recorded Xcode")
            settings = c.file_evidence(path / "SDKSettings.json")
            self.sdk = {"name": name, "path": str(path), "settings": settings,
                        "version": c.run([*command, "--show-sdk-version"], c.root).strip(),
                        "build": c.run([*command, "--show-sdk-build-version"], c.root).strip()}
        return self.sdk

    def requirement(self, name, framework=False):
        if not platform_path(name):
            raise ValueError(f"Unknown/non-platform Apple library {name}; provider evidence is required")
        if name in self.requirements:
            return self.requirements[name]
        sdk = self.sdk_context()
        root = Path(sdk["path"])
        relative = PurePosixPath(name).relative_to("/")
        candidates = [root / (str(relative).removesuffix(".dylib") + ".tbd")]
        # macOS versioned frameworks also expose an unversioned SDK symlink.
        flat = re.sub(r"/Versions/[^/]+/", "/", str(relative))
        candidates.append(root / (flat.removesuffix(".dylib") + ".tbd"))
        stub = next((path for path in candidates if path.is_file()), None)
        if stub is None or not stub.resolve().is_relative_to(root):
            raise ValueError(f"Missing/unsafe Apple SDK stub for {name}")
        identity = stub_identity(stub, self.collector.target)
        if identity != name and not (framework and re.sub(r"/Versions/[^/]+/", "/", identity) == name):
            raise ValueError(f"Apple SDK stub install name does not match {name}")
        if identity not in self.requirements:
            evidence = self.collector.file_evidence(stub)
            reference = "urn:cdk:apple-platform:" + hashlib.sha256((self.collector.target + identity).encode()).hexdigest()
            self.collector.components.append({"type": "library", "bom-ref": reference, "name": identity, "scope": "required",
                "properties": [{"name": "cdk:apple:provision", "value": "deployment-platform-not-bundled"},
                               {"name": "cdk:apple:sdk-version", "value": sdk["version"]},
                               {"name": "cdk:apple:sdk-stub-sha256", "value": evidence["sha256"]}]})
            self.requirements[identity] = {"install_name": identity, "component": reference, "stub": evidence,
                "sdk": sdk, "runtime_version": "provided by deployment OS; not inferred from SDK or dylib compatibility versions"}
        return self.requirements[identity]

    def link_requirements(self, libraries):
        requirements = []
        for flag in libraries:
            if flag.startswith("framework=") and re.fullmatch(r"[A-Za-z0-9_]+", flag[10:]):
                name = flag[10:]
                requirements.append(self.requirement(f"/System/Library/Frameworks/{name}.framework/{name}", framework=True))
            else:
                raise ValueError(f"Unclassified Apple native link flag: {flag}")
        return requirements

    def rust_sysroot(self):
        if self.sysroot is None:
            c = self.collector
            self.sysroot = Path(c.run(["rustc", "--print", "sysroot"], c.root).strip()).resolve(strict=True)
        return self.sysroot

    def resolve_host_library(self, name, image, rpaths):
        c = self.collector
        if not c.host:
            raise ValueError(f"Unknown/non-platform Apple library {name}; provider evidence is required")
        sysroot = self.rust_sysroot()

        def expand(value):
            if value.startswith("@loader_path/"):
                return image.parent / value[len("@loader_path/"):]
            if Path(value).is_absolute():
                return Path(value)
            raise ValueError(f"Unsupported Mach-O host search path: {value}")

        if name.startswith("@rpath/"):
            paths = [expand(path) / name[len("@rpath/"):] for path in rpaths]
            if os.environ.get("DYLD_FALLBACK_LIBRARY_PATH") == str(sysroot / "lib"):
                paths.append(sysroot / "lib" / name[len("@rpath/"):])
        else:
            paths = [expand(name)]
        found = {path.resolve() for path in paths if path.is_file()}
        if len(found) != 1:
            raise ValueError(f"Missing/ambiguous Mach-O host dependency: {name}")
        path = found.pop()
        if not path.is_relative_to(sysroot / "lib"):
            raise ValueError(f"Unknown Mach-O host provider outside the Rust sysroot: {path}")
        # A toolchain version is a distribution identity, not an upstream LLVM identity.
        manifests = list((sysroot / "lib/rustlib").glob("manifest-*"))
        entry = "file:" + str(path.relative_to(sysroot))
        owners = [manifest for manifest in manifests if entry in manifest.read_text().splitlines()]
        if len(owners) != 1:
            raise ValueError(f"Rust host library has no unique installed-component manifest: {path}")
        evidence = c.file_evidence(path)
        manifest = c.file_evidence(owners[0])
        reference = "urn:cdk:rust-distribution-file:" + evidence["sha256"]
        if not any(component["bom-ref"] == reference for component in c.components):
            c.components.append({"type": "library", "bom-ref": reference, "name": path.name,
                                 "hashes": [{"alg": "SHA-256", "content": evidence["sha256"]}],
                                 "properties": [{"name": "cdk:native:identity", "value": "Rust distribution file; upstream composition unresolved"}]})
        return path, {"install_name": name, "component": reference, **evidence, "manifest": manifest}

    def shared_libraries(self, artifact):
        pending = [artifact.resolve()]
        visited = set()
        dynamic = []
        while pending:
            current = pending.pop(0)
            if current in visited:
                continue
            visited.add(current)
            loads = macho_loads(current, self.collector.target)
            if current.is_relative_to(self.collector.profile_dir):
                sdk = self.sdk_context()
                if tuple(map(int, loads["sdk"].split("."))) != tuple(map(int, (sdk["version"] + ".0.0").split(".")[:3])):
                    raise ValueError("Mach-O image SDK does not match the selected Xcode SDK")
            self.images[str(current)] = {key: value for key, value in loads.items() if key != "imports"}
            for requirement in loads["imports"]:
                name = requirement["install_name"]
                if platform_path(name):
                    evidence = self.requirement(name)
                else:
                    path, evidence = self.resolve_host_library(name, current, loads["rpaths"])
                    pending.append(path)
                dynamic.append({**evidence, "required_by": str(current), "load_command": requirement["load_command"],
                                "minimum_os": loads["minimum_os"], "image_sdk": loads["sdk"]})
        return dynamic
