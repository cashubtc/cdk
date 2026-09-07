#!/usr/bin/env python3
"""Check ordinary CDK from a consumer without workspace dev-feature unification."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib


def main():
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory(prefix="cdk-standalone-api-") as directory:
        consumer = Path(directory)
        dependencies = {
            "cashu": {"default-features": False},
            "cdk": {"default-features": False, "features": ["mint", "wallet"]},
            "cdk-sqlite": {"default-features": False, "features": ["mint", "wallet"]},
        }
        manifest = '[package]\nname = "standalone-api-check"\nversion = "0.0.0"\nedition = "2021"\n\n[workspace]\n\n[dependencies]\nserde_json = "1"\n'
        for name, options in dependencies.items():
            fields = [f'path = {json.dumps(str(root / "crates" / name))}']
            fields.extend(
                f"{key} = {json.dumps(value)}" for key, value in options.items()
            )
            manifest += f'{name} = {{ {", ".join(fields)} }}\n'
        (consumer / "Cargo.toml").write_text(manifest)
        # Keep dependency versions pinned while Cargo removes unrelated workspace
        # packages and adds this consumer. Features are still resolved independently.
        (consumer / "Cargo.lock").write_text((root / "Cargo.lock").read_text())
        (consumer / "src").mkdir()
        (consumer / "src/lib.rs").write_text(
            (root / "scripts/fixtures/standalone_api.rs").read_text()
        )
        env = os.environ.copy()
        env.setdefault("CARGO_TARGET_DIR", str(root / "target"))
        cargo = env.get("CARGO", "cargo")
        common = ["--offline", "--manifest-path", str(consumer / "Cargo.toml")]
        graph = subprocess.check_output(
            [cargo, "tree", *common, "--edges", "normal", "--prefix", "none"],
            env=env,
            text=True,
        )
        forbidden = ("bls12_381 ", "cdk-frost ", "cdk-federation-bark ")
        assert not any(line.startswith(forbidden) for line in graph.splitlines()), (
            "Ordinary downstream dependencies include federation cryptography"
        )
        ffi = tomllib.loads((root / "crates/cdk-ffi/Cargo.toml").read_text())
        assert "federation" not in ffi["features"]["default"]
        subprocess.run([cargo, "test", *common], env=env, check=True)
        print("Standalone API, serialization, dependencies, and FFI defaults verified.")


if __name__ == "__main__":
    main()
