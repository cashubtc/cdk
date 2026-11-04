#!/usr/bin/env python3
"""Assemble a local, real Go package from generated CDK bindings and native libraries."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil


PLATFORMS = {
    "x86_64-unknown-linux-gnu": ("linux", "amd64", "libcdk_ffi_go.so"),
    "aarch64-unknown-linux-gnu": ("linux", "arm64", "libcdk_ffi_go.so"),
    "x86_64-apple-darwin": ("darwin", "amd64", "libcdk_ffi_go.dylib"),
    "aarch64-apple-darwin": ("darwin", "arm64", "libcdk_ffi_go.dylib"),
    "x86_64-pc-windows-msvc": ("windows", "amd64", "cdk_ffi_go.dll"),
}


def assemble(bindings, libraries, output, module):
    output.mkdir(parents=True, exist_ok=False)
    package = output / "bindings/cdkffi"
    package.mkdir(parents=True)
    for name in ("cdk_ffi.go", "cdk_ffi.h"):
        shutil.copyfile(bindings / name, package / name)
    records = []
    for target, source in libraries.items():
        system, architecture, name = PLATFORMS[target]
        directory = f"native/{system}_{architecture}"
        destination = package / directory / name
        destination.parent.mkdir(parents=True)
        shutil.copyfile(source, destination)
        flags = f"-L${{SRCDIR}}/{directory} -lcdk_ffi_go"
        if system != "windows":
            flags += f" -Wl,-rpath,${{SRCDIR}}/{directory} -lm"
        if system == "linux":
            flags += " -ldl"
        (package / f"link_{system}_{architecture}.go").write_text(
            f"//go:build {system} && {architecture}\n\npackage cdk_ffi\n\n// #cgo LDFLAGS: {flags}\nimport \"C\"\n")
        with destination.open("rb") as stream:
            checksum = hashlib.file_digest(stream, "sha256").hexdigest()
        records.append({"target": target, "source": str(source.resolve()), "sha256": checksum})
    (output / "go.mod").write_text(f"module {module}\n\ngo 1.22\n")
    (output / "assembly.json").write_text(json.dumps({"libraries": records}, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bindings", type=Path, required=True, help="directory containing generated cdk_ffi.go and cdk_ffi.h")
    parser.add_argument("--library", action="append", required=True, metavar="RUST_TARGET=PATH")
    parser.add_argument("--output", type=Path, required=True, help="new package directory; never overwritten")
    parser.add_argument("--module", default="github.com/cashubtc/cdk-go")
    args = parser.parse_args()
    libraries = {}
    for item in args.library:
        target, separator, path = item.partition("=")
        if not separator or target not in PLATFORMS or target in libraries:
            parser.error("Each --library needs a unique supported RUST_TARGET=PATH")
        libraries[target] = Path(path)
    assemble(args.bindings, libraries, args.output, args.module)
    print(args.output)


if __name__ == "__main__":
    main()
