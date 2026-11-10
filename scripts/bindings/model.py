"""Candidate identities, exact inventories, and deterministic archives."""

import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


class ReleaseError(Exception):
    """An actionable release precondition or verification failure."""


GROUPS = (
    "generated",
    "linux-x86_64",
    "linux-aarch64",
    "android",
    "darwin",
    "windows",
    "packages",
)
ANDROID = {
    "aarch64-linux-android": "arm64-v8a",
    "armv7-linux-androideabi": "armeabi-v7a",
    "i686-linux-android": "x86",
    "x86_64-linux-android": "x86_64",
}
APPLE = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-apple-ios",
    "aarch64-apple-ios-sim",
    "x86_64-apple-ios",
)
LINUX = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu")
WINDOWS = "x86_64-pc-windows-msvc"
DART_TARGETS = (
    *LINUX,
    *APPLE,
    WINDOWS,
    *(t for t in ANDROID if t != "i686-linux-android"),
)
GO_TARGETS = (*LINUX, *APPLE[:2], WINDOWS)


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def run(args, *, cwd=None, env=None, capture=False):
    result = subprocess.run(
        [str(a) for a in args],
        cwd=cwd,
        env=env,
        check=True,
        stdout=subprocess.PIPE if capture else None,
        text=True,
    )
    return result.stdout.strip() if capture else None


def sha256(path):
    with Path(path).open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def digest(value):
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def read_json(path):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"Duplicate JSON key: {key}")
            result[key] = value
        return result

    return json.loads(Path(path).read_text(), object_pairs_hook=unique)


def inputs(root):
    return read_json(root / "bindings/release/inputs.json")


def version(root):
    with (root / "Cargo.toml").open("rb") as handle:
        value = tomllib.load(handle)["workspace"]["package"]["version"]
    require(
        re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", value),
        "Invalid release version",
    )
    return value


def extension(target):
    return "dll" if "windows" in target else "dylib" if "apple" in target else "so"


def library(language, target):
    prefix = "" if "windows" in target else "lib"
    return f"{prefix}cdk_ffi_{language}.{extension(target)}"


def dart_archive(target):
    return f"dart-{target}." + ("zip" if "windows" in target else "tar.gz")


def go_library(target):
    return f"go-{target}.{extension(target)}"


def expected(group, ver):
    if group == "generated":
        return {"generated.tar"}
    if group.startswith("linux-"):
        target = f"{group[6:]}-unknown-linux-gnu"
        return {dart_archive(target), go_library(target)}
    if group == "windows":
        return {dart_archive(WINDOWS), go_library(WINDOWS)}
    if group == "android":
        return {dart_archive(t) for t in ANDROID if t != "i686-linux-android"} | {
            "kotlin.tar.gz",
            f"cdk-android-{ver}.aar",
            f"cdk-android-{ver}.pom",
            f"cdk-android-{ver}.module",
            f"cdk-android-{ver}-sources.jar",
            f"cdk-android-{ver}-javadoc.jar",
        }
    if group == "darwin":
        return (
            {dart_archive(t) for t in APPLE}
            | {go_library(t) for t in APPLE[:2]}
            | {
                "CashuDevKitFFI.xcframework.zip",
                "swift.tar.gz",
            }
        )
    if group == "packages":
        return {"dart.tar.gz", "go.tar.gz"}
    raise ReleaseError(f"Unknown group: {group}")


def dependencies(group):
    if group == "generated":
        return set()
    if group == "packages":
        return (
            {dart_archive(t) for t in DART_TARGETS}
            | {go_library(t) for t in GO_TARGETS}
            | {"generated.tar"}
        )
    return {"generated.tar"}


def inventory(directory, names):
    result = {}
    for name in sorted(names):
        require(
            re.fullmatch(r"[A-Za-z0-9_.+-]+", name), f"Invalid artifact name: {name}"
        )
        path = directory / name
        require(
            path.is_file() and not path.is_symlink(),
            f"Missing regular artifact: {path}",
        )
        result[name] = {"sha256": sha256(path), "size": path.stat().st_size}
    return result


def validate_files(directory, records):
    require(
        inventory(directory, records) == records,
        f"Artifact hashes or sizes differ in {directory}",
    )


def tree_inventory(directory, exclude=()):
    result = {}
    for path in directory.rglob("*"):
        relative = path.relative_to(directory)
        if any(part in exclude for part in relative.parts):
            continue
        if path.is_symlink():
            result[relative.as_posix()] = {"link": os.readlink(path)}
        elif path.is_file():
            result[relative.as_posix()] = {
                "sha256": sha256(path),
                "executable": bool(path.stat().st_mode & 0o111),
            }
    return result


def host():
    arch = {"arm64": "aarch64", "AMD64": "x86_64"}.get(
        platform.machine(), platform.machine()
    )
    return {"os": platform.system(), "arch": arch}


def supported_groups():
    current = host()
    if current == {"os": "Linux", "arch": "x86_64"}:
        return ("generated", "linux-x86_64", "android", "windows", "packages")
    if current == {"os": "Linux", "arch": "aarch64"}:
        return ("linux-aarch64", "packages")
    if current == {"os": "Darwin", "arch": "aarch64"}:
        return ("darwin",)
    return ()


def scratch():
    # Honor this workstation's scratch volume, then the caller's TMPDIR.
    preferred = Path("/data/rust/tmp")
    parent = preferred if preferred.is_dir() and os.access(preferred, os.W_OK) else None
    return Path(tempfile.mkdtemp(prefix="cdk-bindings-", dir=parent))


def empty_directory(path):
    path.mkdir(parents=True, exist_ok=True)
    require(not any(path.iterdir()), f"Output must be empty: {path}")


def source_identity(root):
    if (root / ".jj").exists():
        revision = run(
            ["jj", "log", "-r", "@", "--no-graph", "-T", "commit_id"],
            cwd=root,
            capture=True,
        )
        gitdir = run(["jj", "git", "root"], cwd=root, capture=True)
        git = ["git", f"--git-dir={gitdir}"]
    else:
        require(
            not run(
                ["git", "status", "--porcelain", "--untracked-files=normal"],
                cwd=root,
                capture=True,
            ),
            "Commit the recipe before building a candidate",
        )
        git = ["git", "-C", root]
        revision = run([*git, "rev-parse", "HEAD"], capture=True)
    require(re.fullmatch(r"[0-9a-f]{40}", revision), "Expected a full source commit")
    epoch = int(run([*git, "show", "-s", "--format=%ct", revision], capture=True))
    return {"commit": revision, "epoch": epoch}, git


def recipe_identity(root, source):
    return {
        **source,
        "version": version(root),
        "inputs": {
            name: sha256(root / name)
            for name in (
                "Cargo.lock",
                "flake.lock",
                "rust-toolchain.toml",
                "bindings/release/inputs.json",
            )
        },
    }


def export_source(root, destination):
    source, git = source_identity(root)
    archive_path = destination.parent / "source.tar"
    run([*git, "archive", "--format=tar", f"--output={archive_path}", source["commit"]])
    extract(archive_path, destination)
    archive_path.unlink()
    return recipe_identity(destination, source)


def archive(source, destination, epoch):
    """Normalize metadata without altering any file's content; preserve safe symlinks."""
    paths = sorted(source.rglob("*"), key=lambda p: p.relative_to(source).as_posix())
    if str(destination).endswith(".zip"):
        # Python and zlib come from flake.lock; timestamps are a fixed ZIP epoch.
        with zipfile.ZipFile(
            destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
        ) as out:
            for path in paths:
                if path.is_dir() and not path.is_symlink():
                    continue
                info = zipfile.ZipInfo(
                    path.relative_to(source).as_posix(), (1980, 1, 1, 0, 0, 0)
                )
                info.create_system = 3
                info.compress_type = zipfile.ZIP_DEFLATED
                mode = (
                    stat.S_IFLNK | 0o777
                    if path.is_symlink()
                    else stat.S_IFREG
                    | (0o755 if path.stat().st_mode & 0o111 else 0o644)
                )
                info.external_attr = mode << 16
                out.writestr(
                    info,
                    os.readlink(path).encode()
                    if path.is_symlink()
                    else path.read_bytes(),
                    compress_type=zipfile.ZIP_DEFLATED,
                    compresslevel=9,
                )
        return
    with destination.open("wb") as raw:
        compressor = (
            gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0)
            if str(destination).endswith(".gz")
            else None
        )
        with tarfile.open(
            fileobj=compressor or raw, mode="w|", format=tarfile.PAX_FORMAT
        ) as out:
            for path in paths:
                info = out.gettarinfo(
                    str(path), arcname=path.relative_to(source).as_posix()
                )
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = epoch
                info.pax_headers = {}
                info.mode = (
                    0o755
                    if info.isdir() or info.issym() or path.stat().st_mode & 0o111
                    else 0o644
                )
                if info.isfile():
                    with path.open("rb") as handle:
                        out.addfile(info, handle)
                else:
                    out.addfile(info)
        if compressor:
            compressor.close()


def extract(source, destination):
    empty_directory(destination)
    if str(source).endswith(".zip"):
        with zipfile.ZipFile(source) as zipped:
            for item in zipped.infolist():
                name = PurePosixPath(item.filename)
                require(
                    not name.is_absolute()
                    and ".." not in name.parts
                    and ".git" not in name.parts,
                    "Unsafe archive member",
                )
                target = destination.joinpath(*name.parts)
                require(
                    target.resolve().is_relative_to(destination.resolve()),
                    "Archive escapes destination",
                )
                target.parent.mkdir(parents=True, exist_ok=True)
                if item.is_dir():
                    target.mkdir(exist_ok=True)
                elif stat.S_ISLNK(item.external_attr >> 16):
                    link = zipped.read(item).decode()
                    require(
                        (target.parent / link)
                        .resolve()
                        .is_relative_to(destination.resolve()),
                        "Unsafe archive link",
                    )
                    target.symlink_to(link)
                else:
                    target.write_bytes(zipped.read(item))
                    target.chmod(0o755 if item.external_attr >> 16 & 0o111 else 0o644)
    else:
        with tarfile.open(source) as packed:
            require(
                all(
                    ".git" not in PurePosixPath(m.name).parts
                    for m in packed.getmembers()
                ),
                "Archive contains Git metadata",
            )
            packed.extractall(destination, filter="data")


def check_group(directory, group, identity):
    record = read_json(directory / f"group-{group}.json")
    require(
        record.get("schema") == 1 and record.get("group") == group,
        "Invalid group inventory",
    )
    require(record.get("source") == identity, f"Source identity mismatch: {group}")
    require(
        set(record["files"]) == expected(group, identity["version"]),
        f"Incomplete output inventory: {group}",
    )
    require(
        set(record["imports"]) == dependencies(group),
        f"Incomplete dependency inventory: {group}",
    )
    validate_files(directory, record["files"])
    validate_files(directory, record["imports"])
    return record


def candidate_manifest(root, directory):
    source, _ = source_identity(root)
    identity = recipe_identity(root, source)
    groups = {g: check_group(directory, g, identity) for g in GROUPS}
    allowed = (
        {name for group in groups.values() for name in group["files"]}
        | {f"group-{g}.json" for g in GROUPS}
        | {"candidate.json", "checksums.txt"}
    )
    require(
        {path.name for path in directory.iterdir()} <= allowed,
        "Unexpected files in candidate directory",
    )
    manifest = {"schema": 1, "source": identity, "groups": groups}
    write_json(directory / "candidate.json", manifest)
    files = {
        name: info for group in groups.values() for name, info in group["files"].items()
    }
    files["candidate.json"] = {"sha256": sha256(directory / "candidate.json")}
    (directory / "checksums.txt").write_text(
        "".join(f"{info['sha256']}  {name}\n" for name, info in sorted(files.items()))
    )
    return manifest


def check_candidate(directory, checksum):
    require(
        re.fullmatch(r"[0-9a-f]{64}", checksum),
        "A trusted candidate manifest SHA-256 is required",
    )
    require(
        sha256(directory / "candidate.json") == checksum,
        "Candidate manifest checksum mismatch",
    )
    manifest = read_json(directory / "candidate.json")
    require(
        manifest.get("schema") == 1 and set(manifest["groups"]) == set(GROUPS),
        "Invalid candidate groups",
    )
    require(
        re.fullmatch(r"[0-9a-f]{40}", manifest["source"]["commit"]),
        "Invalid candidate source commit",
    )
    for group in GROUPS:
        record = check_group(directory, group, manifest["source"])
        require(
            record == manifest["groups"][group], f"Manifest/group mismatch: {group}"
        )
    allowed = (
        {name for group in manifest["groups"].values() for name in group["files"]}
        | {f"group-{g}.json" for g in GROUPS}
        | {"candidate.json", "checksums.txt"}
    )
    require(
        {path.name for path in directory.iterdir()} <= allowed,
        "Unexpected files in candidate directory",
    )
    return manifest


def collect(directory, checksum, receipts, output):
    manifest = check_candidate(directory, checksum)
    accepted = {}
    for path in sorted(receipts.glob("*.json")):
        receipt = read_json(path)
        require(
            receipt.get("schema") == 1 and receipt.get("candidate_sha256") == checksum,
            f"Receipt for different candidate: {path}",
        )
        group = receipt["group"]
        require(group in GROUPS, f"Unknown receipt group: {group}")
        require(
            receipt.get("source") == manifest["source"],
            f"Receipt source mismatch: {path}",
        )
        require(
            receipt.get("files") == manifest["groups"][group]["files"],
            f"Receipt output mismatch: {path}",
        )
        require(
            receipt.get("imports") == manifest["groups"][group]["imports"],
            f"Receipt import mismatch: {path}",
        )
        allowed = {
            "generated": [("Linux", "x86_64")],
            "linux-x86_64": [("Linux", "x86_64")],
            "linux-aarch64": [("Linux", "aarch64")],
            "android": [("Linux", "x86_64")],
            "windows": [("Linux", "x86_64")],
            "darwin": [("Darwin", "aarch64")],
            "packages": [("Linux", "x86_64"), ("Linux", "aarch64")],
        }
        require(
            (receipt["host"]["os"], receipt["host"]["arch"]) in allowed[group],
            f"Receipt host cannot verify {group}",
        )
        accepted.setdefault(group, []).append(receipt)
    missing = sorted(set(GROUPS) - accepted.keys())
    require(not missing, "Missing independent comparisons: " + ", ".join(missing))
    write_json(
        output, {"schema": 1, "candidate_sha256": checksum, "receipts": accepted}
    )


def check_collection(directory, checksum, ledger):
    # Re-run the same receipt validation rather than trusting a boolean in a ledger.
    value = read_json(ledger)
    require(
        value.get("schema") == 1 and value.get("candidate_sha256") == checksum,
        "Collection identity mismatch",
    )
    work = scratch()
    try:
        for group, receipts in value["receipts"].items():
            require(
                group in GROUPS and isinstance(receipts, list),
                "Invalid collection group",
            )
            for index, receipt in enumerate(receipts):
                write_json(work / f"{group}-{index}.json", receipt)
        collect(directory, checksum, work, work / "checked.json")
    finally:
        shutil.rmtree(work)
