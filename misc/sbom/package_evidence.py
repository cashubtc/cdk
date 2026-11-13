#!/usr/bin/env python3
"""Package unchanged SBOM evidence without promoting its assurance claims."""

import argparse
from pathlib import Path
import re
import tarfile

from assurance import require
from provenance import digest, read_run


def package(source, output, name, kind="sbom", exclude=()):
    suffix = "-sbom-diagnostics" if kind == "diagnostics" else "-sbom"
    if kind not in {"sbom", "diagnostics"} or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,179}", name) or not name.endswith(suffix):
        raise ValueError("Use a safe product/version/platform name ending in " + suffix)
    if source.is_symlink() or not source.is_dir():
        raise ValueError("Evidence source must be a real directory")
    source = source.resolve(strict=True)
    if output.resolve().is_relative_to(source):
        raise ValueError("Package output must be outside the evidence source")
    exclusions = [Path(value) for value in exclude]
    if any(path.is_absolute() or ".." in path.parts or path == Path(".") for path in exclusions):
        raise ValueError("Evidence exclusions must be safe relative paths")
    if kind == "sbom":
        if exclusions:
            raise ValueError("Release SBOM packages cannot omit evidence")
        if not (source / "index.json").is_file():
            raise ValueError("Release SBOM packages require a completed collection index")
        _, index = read_run(source)
        require(index.get("assurance", {}), ["final_bytes"])

    def evidence_files():
        files = []
        for path in sorted(source.rglob("*")):
            relative = path.relative_to(source)
            if any(relative.is_relative_to(excluded) for excluded in exclusions):
                continue
            if path.is_symlink() or not (path.is_file() or path.is_dir()):
                raise ValueError(f"Unsafe evidence package entry: {relative}")
            if path.is_file():
                files.append(path)
        return files

    files = evidence_files()
    if not files:
        raise ValueError("No evidence files to package")
    checksums = {path: digest(path) for path in files}
    output.mkdir(parents=True, exist_ok=False)
    archive = output / (name + ".tar.gz")
    with tarfile.open(archive, "x:gz", dereference=True) as bundle:
        root = tarfile.TarInfo(name)
        root.type = tarfile.DIRTYPE
        root.mode = 0o755
        bundle.addfile(root)
        for path in files:
            bundle.add(path, arcname=str(Path(name) / path.relative_to(source)), recursive=False)
    if evidence_files() != files or any(digest(path) != checksum for path, checksum in checksums.items()):
        raise ValueError("Evidence changed during packaging; package must not be uploaded")
    if kind == "sbom":
        _, after = read_run(source)
        if after != index:
            raise ValueError("SBOM index changed during packaging")
    checksum = output / (archive.name + ".sha256")
    checksum.write_text(f"{digest(archive)}  {archive.name}\n")
    return archive, checksum


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--kind", choices=("sbom", "diagnostics"), default="sbom")
    parser.add_argument("--exclude", action="append", default=[])
    args = parser.parse_args()
    try:
        for path in package(args.source, args.output, args.name, args.kind, args.exclude):
            print(path)
    except (OSError, ValueError, KeyError, tarfile.TarError) as error:
        parser.exit(1, f"Evidence packaging failed: {error}\n")


if __name__ == "__main__":
    main()
