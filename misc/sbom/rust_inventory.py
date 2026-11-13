"""Resolve one consumer configuration without unifying unrelated workspace features."""

import json
from pathlib import Path
import shutil
import tomllib
from urllib.parse import quote


def resolve_consumer(workspace, package, args, runner):
    consumer = workspace / ".sbom-consumer"
    (consumer / "src").mkdir(parents=True)
    manifest = Path(package["manifest_path"])
    relative = manifest.relative_to(args.source_root)
    dependency = {
        "package": package["name"], "path": str(workspace / relative.parent),
        "default-features": not args.no_default_features,
        "features": [feature for group in args.features for feature in group.split(",") if feature],
    }
    # JSON strings/arrays are valid TOML values for these fields.
    text = '[package]\nname = "cdk-sbom-consumer"\nversion = "0.0.0"\nedition = "2021"\n'
    text += '\n[workspace]\nresolver = "2"\n\n[dependencies.product]\n'
    text += "\n".join(f"{key} = {json.dumps(value)}" for key, value in dependency.items()) + "\n"
    (consumer / "Cargo.toml").write_text(text)
    (consumer / "src/lib.rs").write_text("// Resolution only; never compiled.\n")
    shutil.copyfile(workspace / "Cargo.lock", consumer / "Cargo.lock")
    # Only the synthetic root may be added. Validate every resolved dependency
    # against the original lock below rather than silently updating versions.
    raw = runner([
        "cargo", "metadata", "--format-version", "1", "--filter-platform", args.target,
        "--manifest-path", consumer / "Cargo.toml",
    ], workspace)
    (consumer / "cargo-metadata.raw.json").write_text(raw)
    metadata = json.loads(raw)
    original = tomllib.loads((workspace / "Cargo.lock").read_text())["package"]
    locked = {(item["name"], item["version"], item.get("source")): item.get("checksum") for item in original}
    for item in tomllib.loads((consumer / "Cargo.lock").read_text())["package"]:
        if item["name"] == "cdk-sbom-consumer" and not item.get("source"):
            continue
        key = (item["name"], item["version"], item.get("source"))
        if key not in locked or item.get("checksum") != locked[key]:
            raise ValueError(f"Consumer resolution changed locked dependency {item['name']}@{item['version']}")
    selected = [item for item in metadata["packages"] if item["name"] == package["name"] and
                Path(item["manifest_path"]).resolve() == (workspace / relative).resolve()]
    if len(selected) != 1:
        raise ValueError("Cannot identify the selected package in the consumer graph")
    # The synthetic root is not shipped and is not part of the product SBOM.
    root = metadata["resolve"]["root"]
    metadata["packages"] = [item for item in metadata["packages"] if item["id"] != root]
    metadata["resolve"]["nodes"] = [item for item in metadata["resolve"]["nodes"] if item["id"] != root]
    metadata["workspace_members"] = [selected[0]["id"]]
    metadata["resolve"]["root"] = selected[0]["id"]
    return metadata, consumer / "Cargo.lock"


def resolved_bom(metadata, package, roles, lock):
    checksums = {(item["name"], item["version"], item.get("source")): item.get("checksum")
                 for item in tomllib.loads(lock.read_text())["package"]}
    items = {}
    for item in metadata["packages"]:
        if item["id"] not in roles:
            continue
        component = {
            "type": "library", "bom-ref": item["id"], "name": item["name"], "version": item["version"],
            "purl": f"pkg:cargo/{quote(item['name'], safe='')}@{quote(item['version'], safe='')}",
            "properties": [{"name": "cdk:rust:resolved-features", "value": json.dumps(next(
                node.get("features", []) for node in metadata["resolve"]["nodes"] if node["id"] == item["id"]))}],
        }
        if item.get("license"):
            component["licenses"] = [{"license": {"name": item["license"]}}]
        checksum = checksums.get((item["name"], item["version"], item.get("source")))
        if checksum:
            component["properties"].append({"name": "cdk:source:archive-sha256", "value": checksum})
        items[item["id"]] = component
    subject = next(item for item in metadata["packages"] if item["id"] in metadata["workspace_members"] and
                   item["name"] == package["name"])
    return {
        "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
        "metadata": {"component": items.pop(subject["id"])}, "components": list(items.values()),
        "dependencies": [{"ref": node["id"], "dependsOn": sorted({dep["pkg"] for dep in node["deps"]
                           if dep["pkg"] in roles and any(kind["kind"] in {None, "normal", "build"}
                                                         for kind in dep["dep_kinds"])})}
                         for node in metadata["resolve"]["nodes"] if node["id"] in roles],
    }
