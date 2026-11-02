#!/usr/bin/env python3
"""Sign/verify a complete run with an explicit Ed25519 builder trust policy."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess

from assurance import DIMENSIONS, require


PREDICATE = "https://cashudevkit.org/sbom/provenance/v1"


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def read_run(path):
    path = path / "index.json" if path.is_dir() else path
    root = path.parent.resolve()
    index = json.loads(path.read_text())
    if index.get("status") != "complete":
        raise ValueError("Only a complete collection can supply release evidence")
    seen = set()
    for item in index.get("documents", []) + index.get("evidence", []):
        relative = Path(item["path"])
        source = root / relative
        if relative.is_absolute() or ".." in relative.parts or str(relative) in seen or source.is_symlink() or not source.resolve().is_relative_to(root):
            raise ValueError("Unsafe/duplicate evidence path in build index")
        seen.add(str(relative))
        if not re.fullmatch(r"[0-9a-f]{64}", item["sha256"]) or digest(source) != item["sha256"]:
            raise ValueError(f"Build evidence hash does not match: {relative}")
    documents = {item["path"] for item in index.get("documents", [])}
    for artifact in index.get("artifacts", []):
        if not re.fullmatch(r"[0-9a-f]{64}", artifact["sha256"]):
            raise ValueError("Invalid build artifact digest")
        for reference in [artifact.get("scan_document"), *artifact.get("native_documents", [])]:
            if reference and reference not in documents:
                raise ValueError("Build artifact refers to an unverified document")
    return root, index


def index_digest(root, index):
    raw = (root / "index.json").read_bytes()
    if json.loads(raw) != index:
        raise ValueError("Build index changed after verification")
    return hashlib.sha256(raw).hexdigest()


def openssl(args):
    result = subprocess.run(["openssl", *map(str, args)], stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if result.returncode:
        raise ValueError("OpenSSL signing/key verification failed")
    return result.stdout


def key_fingerprint(path, private=False):
    args = ["pkey", "-in", path, "-pubout", "-outform", "DER"]
    args += ["-passin", "pass:"] if private else ["-pubin"]
    der = openssl(args)
    if len(der) != 44 or not der.startswith(bytes.fromhex("302a300506032b6570032100")):
        raise ValueError("Provenance keys must be Ed25519 keys")
    return hashlib.sha256(der).hexdigest()


def sign_run(path, key, builder):
    root, index = read_run(path)
    if key.stat().st_mode & 0o077:
        raise ValueError("The signing key must not be accessible to group/other users")
    fingerprint = key_fingerprint(key, private=True)
    checksum = index_digest(root, index)
    statement = {
        "_type": "https://in-toto.io/Statement/v1", "predicateType": PREDICATE,
        "subject": [{"name": "index.json", "digest": {"sha256": checksum}}],
        "predicate": {"builder": builder, "key_sha256": fingerprint,
                      "issued_at": datetime.now(timezone.utc).isoformat(),
                      "configuration": index.get("configuration", {}), "source": index.get("source", {})},
    }
    destination = root / "provenance.json"
    signature = root / "provenance.ed25519"
    if destination.exists() or signature.exists():
        raise ValueError("Provenance files already exist; refusing to overwrite them")
    with destination.open("x") as stream:
        json.dump(statement, stream, sort_keys=True, indent=2)
        stream.write("\n")
    openssl(["pkeyutl", "-sign", "-rawin", "-inkey", key, "-passin", "pass:", "-in", destination, "-out", signature])
    if index_digest(root, index) != checksum:
        raise ValueError("Index changed during signing; resulting signature must not be used")
    return {"status": "signed-not-yet-trusted", "key_sha256": fingerprint, "builder": builder}


def verify_run(path, policy_path):
    root, index = read_run(path)
    policy = json.loads(policy_path.read_text())
    if policy.get("version") != 1 or not policy.get("keys"):
        raise ValueError("Trust policy must explicitly authorize builder keys")
    statement_path = root / "provenance.json"
    statement = json.loads(statement_path.read_text())
    if statement.get("_type") != "https://in-toto.io/Statement/v1" or statement.get("predicateType") != PREDICATE:
        raise ValueError("Unexpected provenance statement/predicate type")
    checksum = index_digest(root, index)
    if statement.get("subject") != [{"name": "index.json", "digest": {"sha256": checksum}}]:
        raise ValueError("Provenance does not bind this exact index")
    predicate = statement["predicate"]
    matches = [item for item in policy["keys"] if item.get("sha256") == predicate.get("key_sha256") and
               item.get("builder") == predicate.get("builder")]
    if len(matches) != 1:
        raise ValueError("Provenance signer/builder is not uniquely authorized by policy")
    authorized = matches[0]
    public_key = policy_path.parent / authorized["public_key"]
    if key_fingerprint(public_key) != authorized["sha256"]:
        raise ValueError("Trusted public-key fingerprint does not match policy")
    openssl(["pkeyutl", "-verify", "-rawin", "-pubin", "-inkey", public_key,
             "-sigfile", root / "provenance.ed25519", "-in", statement_path])
    configuration = index.get("configuration", {})
    if predicate.get("configuration") != configuration or predicate.get("source") != index.get("source", {}):
        raise ValueError("Provenance configuration/source does not match the signed index")
    targets = {artifact["target"] for artifact in index.get("artifacts", [])} or {configuration.get("target")}
    if not targets.issubset(set(authorized.get("targets", []))):
        raise ValueError("Signer is not authorized for this target")
    profiles = {configuration.get("profile"), *[item.get("configuration", {}).get("profile") for item in index.get("input_builds", [])]}
    if policy.get("profiles") and not profiles.issubset(set(policy["profiles"])):
        raise ValueError("Build profile is not authorized for release")
    inputs = index.get("input_builds", []) + index.get("input_inventories", [])
    if policy.get("require_authenticated_inputs", True) and any(item.get("authentication", {}).get("status") != "authenticated" for item in inputs):
        raise ValueError("Policy requires authenticated input builds/inventories")
    if policy.get("require_clean_source", True) and index.get("source", {}).get("dirty", True):
        raise ValueError("Policy requires a clean source revision")
    issued = datetime.fromisoformat(predicate["issued_at"])
    if issued.tzinfo is None:
        raise ValueError("Provenance timestamp must include its time zone")
    age = (datetime.now(timezone.utc) - issued).total_seconds()
    if age < -300 or policy.get("max_age_seconds") is not None and age > policy["max_age_seconds"]:
        raise ValueError("Provenance timestamp is outside the policy window")
    read_run(root)
    if index_digest(root, index) != checksum:
        raise ValueError("Index changed during policy verification")
    return {"status": "authenticated", "builder": authorized["builder"], "key_sha256": authorized["sha256"],
            "index_sha256": checksum}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run", type=Path)
    parser.add_argument("--signing-key", type=Path)
    parser.add_argument("--builder")
    parser.add_argument("--trust-policy", type=Path)
    parser.add_argument("--require-assurance", action="append", choices=DIMENSIONS, default=[])
    args = parser.parse_args()
    try:
        _, index = read_run(args.run)
        if args.signing_key:
            if not args.builder or args.trust_policy or args.require_assurance:
                raise ValueError("Signing requires --builder and must be separate from verification")
            report = sign_run(args.run, args.signing_key, args.builder)
        else:
            if not args.trust_policy:
                raise ValueError("Verification requires an explicit --trust-policy")
            report = verify_run(args.run, args.trust_policy)
            assurance = index.get("assurance")
            if assurance:
                assurance["dimensions"]["authenticated_provenance"] = dict(report)
                require(assurance, args.require_assurance)
                report["assurance"] = assurance
            elif args.require_assurance:
                raise ValueError("Legacy run has no machine-readable assurances")
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    except (ValueError, OSError, KeyError, TypeError) as error:
        print(f"SBOM provenance rejected: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
