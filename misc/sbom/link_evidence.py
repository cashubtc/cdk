"""Observe the real target linker and retain maps without recording secret argv/env values."""

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tomllib
import uuid


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def real_linker(root, target):
    variable = "CARGO_TARGET_" + target.upper().replace("-", "_") + "_LINKER"
    explicit = os.environ.get(variable)
    if not explicit:
        directories = [parent / ".cargo" for parent in (root, *root.parents)]
        directories.append(Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")))
        for directory in directories:
            for name in ("config", "config.toml"):
                path = directory / name
                if path.is_file():
                    tables = tomllib.loads(path.read_text()).get("target", {})
                    if any(value.get("linker") for key, value in tables.items() if key == target or key.startswith("cfg(")):
                        raise ValueError(f"Set {variable} explicitly to capture a configured target linker")
    tool = shutil.which(explicit or "cc")
    if not tool:
        raise ValueError("Cannot identify the target linker executable")
    # Preserve argv[0] aliases such as clang++ and NDK API-qualified drivers.
    return Path(tool).absolute()


def archive_members(text):
    """Retain named contributions; LTO-generated contributions remain in the raw map."""
    members = {}
    for line in text.splitlines():
        match = re.search(r"(/[^\r\n()]*\.(?:a|rlib))\(([^\r\n()]+)\)", line)
        if match:
            path = Path(match[1].strip())
            if path.is_file():
                members.setdefault(path, set()).add(match[2])
    return [{"path": str(path), "sha256": digest(path), "members": sorted(names)}
            for path, names in sorted(members.items())]


def invoke_linker():
    directory = Path(os.environ["CDK_SBOM_LINK_CAPTURE"])
    tool = Path(os.environ["CDK_SBOM_REAL_LINKER"])
    arguments = sys.argv[1:]
    expanded = []
    for argument in arguments:
        if argument.startswith("@"):
            # Keep response files private; the original arguments are passed on unchanged.
            raise ValueError("Linker response-file capture needs explicit parser support")
        expanded.append(argument)
    positions = [number for number, argument in enumerate(expanded) if argument == "-o"]
    if len(positions) != 1 or positions[0] + 1 >= len(expanded):
        raise ValueError("Cannot identify a unique target linker output")
    output = Path(expanded[positions[0] + 1]).resolve()
    identity = uuid.uuid4().hex
    map_path = directory / (identity + ".map")
    result = subprocess.run([str(tool), *arguments, "-Wl,-Map=" + str(map_path)], check=False)
    if result.returncode:
        return result.returncode
    if not output.is_file() or not map_path.is_file():
        raise ValueError("Target linker did not produce its output and requested map")
    text = map_path.read_text()
    record = {"output": str(output), "output_sha256_before_rustc_postprocessing": digest(output),
              "tool": str(tool), "tool_sha256": digest(tool), "map": map_path.name,
              "map_sha256": digest(map_path), "arguments_sha256": hashlib.sha256(json.dumps(arguments).encode()).hexdigest(),
              "named_archive_members": archive_members(text),
              "limitations": ["Named archive contributions only; LTO-generated objects require the raw map and compiler evidence."]}
    (directory / (identity + ".json")).write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(invoke_linker())
    except (ValueError, OSError, KeyError) as error:
        print(f"Linker evidence incomplete: {error}", file=sys.stderr)
        raise SystemExit(1)
