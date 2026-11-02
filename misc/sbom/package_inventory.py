"""Hash packaged native files, including libraries nested inside Maven archives."""

import gzip
import hashlib
from pathlib import Path, PurePosixPath
import tempfile
import zipfile
import zlib
from urllib.parse import quote


NATIVE_SUFFIXES = {".so", ".dylib", ".jnilib", ".dll", ".a", ".lib", ".rlib", ".node", ".wasm"}


def native_name(name):
    if name.endswith(".gz"):
        return native_name(name[:-3])
    return Path(name).suffix.lower() in NATIVE_SUFFIXES or ".so." in Path(name).name


def native_magic(prefix):
    return prefix.startswith((b"\x7fELF", b"MZ", b"\x00asm", b"!<arch>\n", b"<bigaf>\n",
                              b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce"))


def binary_identity(prefix):
    if prefix.startswith(b"\x7fELF"):
        if len(prefix) < 20 or prefix[4] not in {1, 2} or prefix[5] not in {1, 2}:
            raise ValueError("Invalid ELF header in packaged library")
        machine = int.from_bytes(prefix[18:20], "little" if prefix[5] == 1 else "big")
        return {"format": "ELF", "architecture": {3: "x86", 40: "arm", 62: "x86_64", 183: "aarch64"}.get(machine, str(machine))}
    if prefix.startswith(b"!<arch>\n"):
        return {"format": "ar", "architecture": "requires-member-inspection"}
    if prefix.startswith(b"<bigaf>\n"):
        return {"format": "AIX-big-ar", "architecture": "requires-member-inspection"}
    if prefix[:2] in {b"\x01\xdf", b"\x01\xf7"}:
        return {"format": "XCOFF", "architecture": "powerpc" if prefix[:2] == b"\x01\xdf" else "powerpc64"}
    if prefix.startswith(b"\x00asm\x01\x00\x00\x00"):
        return {"format": "WebAssembly", "architecture": "wasm"}
    if prefix.startswith(b"MZ"):
        if len(prefix) < 64:
            raise ValueError("Invalid PE header in packaged library")
        offset = int.from_bytes(prefix[60:64], "little")
        if prefix[offset:offset + 4] != b"PE\0\0":
            raise ValueError("Cannot identify PE architecture from packaged library header")
        machine = int.from_bytes(prefix[offset + 4:offset + 6], "little")
        return {"format": "PE", "architecture": {0x14c: "x86", 0x8664: "x86_64", 0xaa64: "aarch64"}.get(machine, str(machine))}
    if prefix[:4] in {b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce"}:
        endian = "little" if prefix[0] in {0xcf, 0xce} else "big"
        cpu = int.from_bytes(prefix[4:8], endian)
        return {"format": "Mach-O", "architecture": {7: "x86", 0x1000007: "x86_64", 0x100000c: "aarch64"}.get(cpu, str(cpu))}
    if prefix[:4] in {b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca"}:
        return {"format": "Mach-O-universal", "architecture": "multiple"}
    raise ValueError("Unrecognized packaged native library format")


def native_record(stream, name, owner, container_hash=None):
    prefix = stream.read(65536)
    compressed = prefix.startswith(b"\x1f\x8b")
    identity = {} if compressed else binary_identity(prefix)
    checksum = hashlib.sha256(prefix)
    size = len(prefix)
    while block := stream.read(1024 * 1024):
        checksum.update(block)
        size += len(block)
        if size > 512 * 1024 * 1024:
            raise ValueError("Native file exceeds inventory size limit")
    if compressed:
        stream.seek(0)
        try:
            with gzip.GzipFile(fileobj=stream) as decoded:
                unpacked = native_record(decoded, name.removesuffix(".gz"), owner, container_hash)
        except (gzip.BadGzipFile, EOFError, zlib.error) as error:
            raise ValueError(f"Invalid compressed native file {name}: {error}") from error
        identity = {key: unpacked[key] for key in ("format", "architecture")}
        identity.update(encoding="gzip", uncompressed_sha256=unpacked["sha256"], uncompressed_size=unpacked["size"])
    return {"path": name, "owner": owner, "sha256": checksum.hexdigest(), "size": size,
            "container_sha256": container_hash, **identity}


def archive_natives(path, owner, checksum, depth=0, prefix="", budget=None):
    try:
        return _archive_natives(path, owner, checksum, depth, prefix, budget)
    except (zipfile.BadZipFile, EOFError) as error:
        raise ValueError(f"Invalid packaged archive for {owner}: {error}") from error


def _archive_natives(path, owner, checksum, depth, prefix, budget):
    if depth > 4:
        raise ValueError("Packaged archive nesting exceeds inventory limit")
    if budget is None:
        budget = [2 * 1024 * 1024 * 1024]
    records = []
    with zipfile.ZipFile(path) as archive:
        names = set()
        for entry in archive.infolist():
            name = PurePosixPath(entry.filename)
            if entry.filename in names or name.is_absolute() or ".." in name.parts:
                raise ValueError(f"Unsafe or duplicate packaged archive entry: {entry.filename}")
            names.add(entry.filename)
            nested = Path(entry.filename).suffix.lower() in {".jar", ".aar", ".zip"}
            if entry.is_dir():
                continue
            if entry.flag_bits & 1:
                raise ValueError("Encrypted archive entries cannot be inventoried")
            with archive.open(entry) as stream:
                selected = native_name(entry.filename)
                if not nested and not selected:
                    selected = native_magic(stream.read(8))
                    stream.seek(0)
                if not (nested or selected):
                    continue
                budget[0] -= entry.file_size
                if entry.file_size > 512 * 1024 * 1024 or budget[0] < 0:
                    raise ValueError("Oversized native archive entry")
                if nested:
                    with tempfile.SpooledTemporaryFile(max_size=8 * 1024 * 1024) as copied:
                        while block := stream.read(1024 * 1024):
                            copied.write(block)
                        copied.seek(0)
                        records.extend(archive_natives(copied, owner, checksum, depth + 1,
                                                       prefix + entry.filename + "!", budget))
                else:
                    try:
                        records.append(native_record(stream, prefix + entry.filename, owner, checksum))
                    except ValueError as error:
                        raise ValueError(f"{owner}: {prefix}{entry.filename}: {error}") from error
    return records


def native_files(project, include_archives=False):
    for path in sorted(project.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Packaged symlink requires explicit inventory support: {path}")
        if not path.is_file():
            continue
        selected = native_name(path.name) or include_archives and path.suffix.lower() in {".jar", ".aar", ".zip"}
        if not selected:
            with path.open("rb") as source:
                magic = source.read(8)
            selected = native_magic(magic)
        if selected:
            if not path.resolve().is_relative_to(project.resolve()):
                raise ValueError(f"Packaged native file escapes project: {path}")
            yield path


def file_bom(name, records):
    root = "urn:cdk:packaged-native:" + name
    components = []
    for number, record in enumerate(records):
        components.append({
            "type": "file", "bom-ref": f"{root}:{number}", "name": record["path"],
            "hashes": [{"alg": "SHA-256", "content": record["sha256"]}],
            "properties": [{"name": "cdk:packaged-native:" + key, "value": str(value)}
                           for key, value in record.items() if key != "sha256" and value is not None],
        })
    return {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
            "metadata": {"component": {"type": "application", "name": name, "bom-ref": root}},
            "components": components,
            "dependencies": [{"ref": root, "dependsOn": [component["bom-ref"] for component in components]}]}


def maven_bom(name, graphs):
    root = "urn:cdk:gradle:" + name
    modules = {}
    edges = {}
    for graph in graphs:
        for module in graph["modules"]:
            ref = f"{module['group']}:{module['name']}:{module['version']}"
            modules[ref] = {"type": "library", "bom-ref": ref, "group": module["group"],
                            "name": module["name"], "version": module["version"], "scope": "required",
                            "purl": f"pkg:maven/{module['group']}/{module['name']}@{module['version']}"}
            edges.setdefault(ref, set()).update(module["dependencies"])
    return {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
            "metadata": {"component": {"type": "application", "name": name, "bom-ref": root}},
            "components": list(modules.values()),
            "dependencies": [{"ref": root, "dependsOn": sorted(modules)}] +
                            [{"ref": ref, "dependsOn": sorted(deps)} for ref, deps in sorted(edges.items())]}


def go_bom(modules, packages, version):
    """Keep the full selected module set and explicit Go standard-library identity."""
    main = [module for module in modules if module.get("Main")]
    if len(main) != 1 or not version.startswith("go"):
        raise ValueError("Go inventory requires one main module and an identified toolchain")
    imported = {package.get("Module", {}).get("Path") for package in packages}
    indexed = {}
    for module in modules:
        path = module["Path"]
        ref = "pkg:golang/" + quote(path, safe="/")
        if module.get("Version"):
            ref += "@" + quote(module["Version"], safe="")
        component = {"type": "library", "bom-ref": ref, "name": path, "purl": ref, "scope": "required",
                     "properties": [{"name": "cdk:go:module-role", "value": "imported" if path in imported else "selected-only"}]}
        if module.get("Version"):
            component["version"] = module["Version"]
        for key in ("Sum", "GoModSum", "GoVersion"):
            if module.get(key):
                component["properties"].append({"name": "cdk:go:" + key, "value": module[key]})
        indexed[path] = component
    stdlib = {"type": "library", "bom-ref": "pkg:golang/stdlib@" + version,
              "name": "stdlib", "version": version, "purl": "pkg:golang/stdlib@" + version, "scope": "required"}
    subject = indexed.pop(main[0]["Path"])
    components = list(indexed.values()) + [stdlib]
    # All selected modules remain required here so scanners also check modules
    # used only by tests/build tooling. This is not a binary reachability claim.
    return {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
            "metadata": {"component": subject}, "components": components,
            "dependencies": [{"ref": subject["bom-ref"], "dependsOn": [item["bom-ref"] for item in components]}]}
