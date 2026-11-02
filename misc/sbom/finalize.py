#!/usr/bin/env python3
"""Verify final ELF/package bytes against observed builds, including normal stripping."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import uuid
import zipfile
import subprocess

from assurance import DIMENSIONS, assessment, inherited_dimensions, require
from elf_evidence import verify_postprocessing
from provenance import digest, index_digest, read_run, verify_run
from consumer import go_consumer
from sbom import ROOT, Collector, SbomError, native_files, parser as collector_parser, run, write_json


def cargo_packages(bom):
    items = [bom.get("metadata", {}).get("component", {}), *bom.get("components", [])]
    return {(item["name"], item["version"]) for item in items
            if item.get("purl", "").startswith("pkg:cargo/") and item.get("version")}


def check_platform(record, artifact):
    parts = record["path"].replace("!", "/").split("/")
    abis = {"arm64-v8a": "aarch64-linux-android", "armeabi-v7a": "armv7-linux-androideabi",
            "x86": "i686-linux-android", "x86_64": "x86_64-linux-android"}
    for number, part in enumerate(parts[:-1]):
        if part == "jni":
            if artifact["target"] != abis.get(parts[number + 1]) or parts[-1] != artifact["name"]:
                raise ValueError("Packaged JNI filename/ABI does not match its verified build")
        if part == "native" and parts[number + 1].startswith("linux_"):
            expected = {"linux_amd64": "x86_64-unknown-linux-gnu", "linux_arm64": "aarch64-unknown-linux-gnu"}.get(parts[number + 1])
            if artifact["target"] != expected or parts[-1] != artifact["name"]:
                raise ValueError("Packaged Go filename/platform does not match its verified build")


def materialize(container, member, destination):
    """Copy only an inventoried member; never extract archive paths to the filesystem."""
    if not member:
        shutil.copyfile(container, destination)
        return
    head, *tail = member.split("!", 1)
    with zipfile.ZipFile(container) as archive, archive.open(head) as stream:
        if tail:
            with tempfile.SpooledTemporaryFile(max_size=8 * 1024 * 1024) as nested:
                shutil.copyfileobj(stream, nested)
                nested.seek(0)
                materialize(nested, tail[0], destination)
        else:
            with destination.open("wb") as output:
                shutil.copyfileobj(stream, output)


def finalize(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    collector = Collector(collector_parser().parse_args(["rust"]))
    collector.output = output
    index = collector.index
    index.update(mode="final-artifact", scope="final-distributions", configuration={"target": "multi-target", "profile": "final-artifact"})
    index["source"] = {"dirty": True, "note": "Per-build source identities are retained in input build indexes."}
    try:
        index["source"] = {"revision": run(["git", "rev-parse", "HEAD"], ROOT).strip(),
                           "dirty": bool(run(["git", "status", "--porcelain"], ROOT).strip())}
        builds = []
        bound = {}

        def bound_json(path):
            raw = path.read_bytes()
            if hashlib.sha256(raw).hexdigest() != bound[str(path)]:
                raise ValueError("Input document changed after build verification")
            return json.loads(raw)
        for number, path in enumerate(args.build_run):
            root, build = read_run(path)
            authentication = verify_run(path, args.trust_policy) if args.trust_policy else {"status": "unauthenticated-local-build"}
            collector.evidence(root / "index.json", f"builds/{number}/index.json")
            if index["evidence"][-1]["sha256"] != index_digest(root, build):
                raise ValueError("Input build index changed while copying")
            if args.trust_policy and authentication["index_sha256"] != index["evidence"][-1]["sha256"]:
                raise ValueError("Copied build index differs from the authenticated index")
            if args.trust_policy:
                collector.evidence(root / "provenance.json", f"builds/{number}/provenance.json")
                collector.evidence(root / "provenance.ed25519", f"builds/{number}/provenance.ed25519")
            for item in build["documents"] + build["evidence"]:
                collector.evidence(root / item["path"], f"builds/{number}/{item['path']}")
                if index["evidence"][-1]["sha256"] != item["sha256"]:
                    raise ValueError("Input build evidence changed while copying")
                bound[str(root / item["path"])] = item["sha256"]
            builds.append((root, build))
            index.setdefault("unresolved_native_identities", []).extend(build.get("unresolved_native_identities", []))
            index.setdefault("input_builds", []).append({"index_sha256": index_digest(root, build),
                                                        "configuration": build.get("configuration", {}), "authentication": authentication})
        for number, path in enumerate(args.inventory_run):
            root, inventory = read_run(path)
            authentication = verify_run(path, args.trust_policy) if args.trust_policy else {"status": "unauthenticated-local-inventory"}
            collector.evidence(root / "index.json", f"inventories/{number}/index.json")
            if index["evidence"][-1]["sha256"] != index_digest(root, inventory):
                raise ValueError("Input inventory index changed while copying")
            if args.trust_policy:
                if authentication["index_sha256"] != index["evidence"][-1]["sha256"]:
                    raise ValueError("Copied inventory differs from authenticated bytes")
                collector.evidence(root / "provenance.json", f"inventories/{number}/provenance.json")
                collector.evidence(root / "provenance.ed25519", f"inventories/{number}/provenance.ed25519")
            for item in inventory["documents"] + inventory["evidence"]:
                collector.evidence(root / item["path"], f"inventories/{number}/{item['path']}")
                if index["evidence"][-1]["sha256"] != item["sha256"]:
                    raise ValueError("Input inventory evidence changed while copying")
            index.setdefault("input_inventories", []).append({"index_sha256": index_digest(root, inventory), "authentication": authentication})
            index.setdefault("unresolved_native_identities", []).extend(inventory.get("unresolved_native_identities", []))
        originals = []
        for path in args.original:
            checksum = digest(path)
            matches = [(root, build, artifact) for root, build in builds for artifact in build["artifacts"]
                       if artifact["sha256"] == checksum]
            if len(matches) != 1:
                raise ValueError(f"Original artifact is not uniquely bound to an input build: {path}")
            root, build, artifact = matches[0]
            if not artifact.get("native_documents"):
                raise ValueError("Original artifact has no native build evidence")
            for name in artifact["native_documents"]:
                properties = {item["name"]: item["value"] for item in bound_json(root / name)["metadata"]["properties"]}
                if properties.get("cdk:artifact:sha256") != checksum or properties.get("cdk:target") != artifact["target"]:
                    raise ValueError("Original native document is not bound to this artifact/target")
            originals.append((path.resolve(), root, artifact))
        distributions = list(args.distribution)
        if args.strip_tool:
            tool = shutil.which(str(args.strip_tool))
            if not tool:
                raise ValueError("The requested strip tool is not executable")
            commands = []
            for number, (path, _, artifact) in enumerate(originals):
                destination = output / "stripped" / str(number) / artifact["name"]
                if Path(artifact["name"]).name != artifact["name"]:
                    raise ValueError("Unsafe artifact name")
                destination.parent.mkdir(parents=True)
                shutil.copyfile(path, destination)
                run([tool, "--strip-all", destination], output)
                commands.append({"tool": str(Path(tool).resolve()), "tool_sha256": digest(Path(tool)),
                                 "arguments": ["--strip-all"], "input_sha256": artifact["sha256"],
                                 "output_sha256": digest(destination)})
                distributions.append(destination)
            collector.generated_evidence("strip-commands.json", commands)
        if not distributions:
            raise ValueError("Supply --distribution or --strip-tool")
        used = set()
        checks = []
        consumer_results = []
        for number, path in enumerate(distributions):
            path = path.resolve(strict=True)
            files = list(native_files(path, include_archives=True)) if path.is_dir() else [path]
            manifest = [{"path": str(item.relative_to(path)), "sha256": digest(item)} for item in sorted(path.rglob("*")) if item.is_file()] if path.is_dir() else [{"path": path.name, "sha256": digest(path)}]
            collector.generated_evidence(f"distribution-{number}-manifest.json", manifest)
            records = collector.packaged_files(files, f"distribution-{number}", "final-release", path if path.is_dir() else None)
            index.setdefault("distributions", []).append({"path": str(path), "manifest": f"evidence/distribution-{number}-manifest.json",
                                                          "native_document": f"packages/distribution-{number}.native-files.cdx.json"})
            for record in records:
                if record.get("encoding"):
                    raise ValueError("Compressed native-asset finalization requires additional transformation evidence")
                with tempfile.TemporaryDirectory(prefix=".final-", dir=output) as temporary:
                    candidate = Path(temporary) / "artifact"
                    if path.is_dir():
                        source = path / record["path"].split("!", 1)[0]
                    else:
                        source = path
                    member = record["path"].split("!", 1)[1] if "!" in record["path"] else None
                    materialize(source, member, candidate)
                    if digest(candidate) != record["sha256"]:
                        raise ValueError("Packaged native bytes changed after inventory")
                    matches = []
                    for original, root, artifact in originals:
                        if digest(original) != artifact["sha256"]:
                            raise ValueError("Original artifact changed after verification")
                        if record["sha256"] == artifact["sha256"]:
                            matches.append((original, root, artifact, {"method": "exact-build-bytes"}))
                        elif record["format"] == "ELF":
                            try:
                                equivalent = verify_postprocessing(original, candidate)
                            except ValueError:
                                continue
                            matches.append((original, root, artifact, equivalent))
                    if len(matches) != 1:
                        raise ValueError(f"Final native file is not uniquely equivalent to a verified build: {record['path']}")
                    original, root, artifact, equivalence = matches[0]
                    check_platform({**record, "path": str(path / record["path"]) if path.is_dir() else record["path"]}, artifact)
                    if args.go_project:
                        named = Path(temporary) / artifact["name"]
                        shutil.copyfile(candidate, named)
                        consumer_results.append(go_consumer(args.go_project, named, artifact["target"], output,
                                                            args.go_runner, args.go_runner_arg))
                    used.add(artifact["sha256"])
                    scan_name = None
                    if artifact.get("scan_document"):
                        scanned = output / "evidence" / f"final-scan-{len(checks)}.json"
                        run(["syft", "scan", f"file:{candidate}", "--output", f"cyclonedx-json={scanned}"], output)
                        scan_name = f"artifacts/final-{len(checks)}"
                        bom = collector.document(scanned, scan_name, "final-artifact-scan")
                        if cargo_packages(bom) != cargo_packages(bound_json(root / artifact["scan_document"])):
                            raise ValueError("Final embedded Rust inventory differs from the verified build")
                    entry = {"name": record["path"], "sha256": record["sha256"], "target": artifact["target"],
                             "original_sha256": artifact["sha256"], "verification": equivalence["method"], "native_documents": []}
                    if scan_name:
                        entry["scan_document"] = scan_name + ".cdx.json"
                    for native in artifact["native_documents"]:
                        bom = copy.deepcopy(bound_json(root / native))
                        old_ref = bom["metadata"]["component"]["bom-ref"]
                        new_ref = "urn:cdk:final-artifact:" + record["sha256"]
                        bom["metadata"]["component"].update({"bom-ref": new_ref, "name": record["path"],
                            "hashes": [{"alg": "SHA-256", "content": record["sha256"]}]})
                        for prop in bom["metadata"]["properties"]:
                            if prop["name"] == "cdk:artifact:sha256":
                                prop["value"] = record["sha256"]
                        bom["metadata"]["properties"].append({"name": "cdk:original-artifact:sha256", "value": artifact["sha256"]})
                        for edge in bom.get("dependencies", []):
                            if edge["ref"] == old_ref:
                                edge["ref"] = new_ref
                        source = collector.generated_evidence(f"final-native-{len(checks)}-{len(entry['native_documents'])}.json", bom)
                        name = f"artifacts/native-final-{len(checks)}-{len(entry['native_documents'])}"
                        collector.document(source, name, "final-native-evidence")
                        entry["native_documents"].append(name + ".cdx.json")
                    index["artifacts"].append(entry)
                    checks.append({"path": record["path"], "original_sha256": artifact["sha256"],
                                   "final_sha256": record["sha256"], **equivalence})
            if path.is_dir():
                after = [{"path": str(item.relative_to(path)), "sha256": digest(item)}
                         for item in sorted(path.rglob("*")) if item.is_file()]
                if after != manifest:
                    raise ValueError("Final distribution directory changed during verification")
            elif digest(path) != manifest[0]["sha256"]:
                raise ValueError("Final distribution changed during verification")
        if used != {artifact["sha256"] for _, _, artifact in originals}:
            raise ValueError("The final distributions omit one or more explicitly supplied original artifacts")
        collector.generated_evidence("postprocessing-verification.json", checks)
        if consumer_results:
            collector.generated_evidence("go-final-consumer-tests.json", consumer_results)
            index["coverage"]["go_final_consumer_test"] = True
        index["assurance"] = assessment(index)
        index["assurance"]["dimensions"]["final_bytes"] = {"status": "verified", "artifacts": len(checks), "scope": "exact bytes or ELF runtime/audit equivalence"}
        index["assurance"]["dimensions"].update(inherited_dimensions([build for _, build in builds]))
        if args.trust_policy:
            index["assurance"]["dimensions"]["authenticated_provenance"]["reason"] = "Input builds authenticated; this finalization still needs its own signature."
        require(index["assurance"], args.require_assurance)
        for item in index["documents"] + index["evidence"]:
            if digest(output / item["path"]) != item["sha256"]:
                raise ValueError("Finalization evidence changed during collection")
        index["status"] = "complete"
    except (ValueError, OSError, KeyError, SbomError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        index["error"] = str(error)
        raise
    finally:
        write_json(output / "index.json", index)
    print(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-run", action="append", required=True, type=Path)
    parser.add_argument("--original", action="append", required=True, type=Path)
    parser.add_argument("--inventory-run", action="append", default=[], type=Path,
                        help="retain the final binding project's resolved language/build-tool inventories")
    parser.add_argument("--distribution", action="append", default=[], type=Path)
    parser.add_argument("--strip-tool", type=Path, help="run the real strip --strip-all on private copies, then verify those bytes")
    parser.add_argument("--trust-policy", type=Path, help="require authenticated input build indexes")
    parser.add_argument("--go-project", type=Path, help="compile and execute a downstream Go program against each final native library")
    parser.add_argument("--go-runner", type=Path, help="explicit runner for cross-target Go execution, for example qemu-aarch64")
    parser.add_argument("--go-runner-arg", action="append", default=[])
    parser.add_argument("--require-assurance", action="append", choices=DIMENSIONS, default=["final_bytes"])
    parser.add_argument("--output", type=Path, default=Path("target/sbom") / uuid.uuid4().hex)
    args = parser.parse_args()
    try:
        finalize(args)
        return 0
    except (ValueError, OSError, KeyError, SbomError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        print(f"Final SBOM rejected: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
