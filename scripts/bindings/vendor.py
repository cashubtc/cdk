"""Apply the reviewed Dart generator ordering fix to Nix's vendored sources."""

import hashlib
import json
from pathlib import Path
import shutil
import sys


def patch_vendor(source, destination):
    destination.mkdir()
    patched = 0
    for group in source.iterdir():
        output = destination / group.name
        if group.name == "config.toml":
            output.write_text(group.read_text().replace(str(source), str(destination)))
        elif not list(group.glob("uniffi-dart-*")):
            output.symlink_to(group.resolve(), target_is_directory=True)
        else:
            output.mkdir()
            for crate in group.iterdir():
                target = output / crate.name
                if not crate.name.startswith("uniffi-dart-"):
                    target.symlink_to(crate.resolve(), target_is_directory=True)
                    continue
                shutil.copytree(crate, target)
                relative = "src/gen/types.rs"
                path = target / relative
                original = path.read_text()
                # This map is iterated when emitting converter declarations.
                # Its String keys support a stable order without API changes.
                if original.count("HashMap") != 4:
                    raise RuntimeError("Review the Dart generator ordering patch")
                path.chmod(0o644)
                path.write_text(original.replace("HashMap", "BTreeMap"))
                checksum = target / ".cargo-checksum.json"
                metadata = json.loads(checksum.read_text())
                metadata["files"][relative] = hashlib.sha256(
                    path.read_bytes()
                ).hexdigest()
                checksum.chmod(0o644)
                checksum.write_text(json.dumps(metadata, sort_keys=True))
                patched += 1
    if patched != 1:
        raise RuntimeError(f"Expected one pinned Dart generator, found {patched}")


if __name__ == "__main__":
    patch_vendor(Path(sys.argv[1]), Path(sys.argv[2]))
