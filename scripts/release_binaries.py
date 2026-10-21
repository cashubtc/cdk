#!/usr/bin/env python3
"""Build, compare, and publish the release binaries using pinned Nix inputs."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
SYSTEMS = {
    "x86_64-linux": ("static", "x86_64"),
    "aarch64-linux": ("static", "aarch64"),
    "aarch64-darwin": ("darwin", "aarch64-apple-darwin"),
}
BINARIES = ("cdk-mintd", "cdk-mintd-ldk", "cdk-cli")
INPUTS = ("Cargo.toml", "Cargo.lock", "flake.nix", "flake.lock", "rust-toolchain.toml")


def git_command(*args):
    if (ROOT / ".jj").exists():
        directory = subprocess.check_output(
            ["jj", "--ignore-working-copy", "git", "root"], cwd=ROOT, text=True
        ).strip()
        return ["git", f"--git-dir={directory}", *args]
    return ["git", *args]


def run(*args, cwd=ROOT):
    if args[0] == "git":
        args = git_command(*args[1:])
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")


def source():
    # JJ working copies have their own commit; Git HEAD can point elsewhere.
    if (ROOT / ".jj").exists():
        commit = run("jj", "log", "--no-graph", "-r", "@", "-T", "commit_id")
    else:
        if run("git", "status", "--porcelain", "--untracked-files=normal"):
            raise ValueError("Release builds require a clean checkout")
        commit = run("git", "rev-parse", "HEAD")
    inputs = {
        name: subprocess.check_output(git_command("show", f"{commit}:{name}"), cwd=ROOT)
        for name in INPUTS
    }
    version = tomllib.loads(inputs["Cargo.toml"].decode())["workspace"]["package"][
        "version"
    ]
    return {
        "commit": commit,
        "version": version,
        "inputs": {
            name: hashlib.sha256(data).hexdigest() for name, data in inputs.items()
        },
    }


def check_tag(tag, info):
    if tag != "v" + info["version"] or not re.fullmatch(
        r"v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", tag
    ):
        raise ValueError("Tag must be v<workspace version>")
    if (
        run("git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
        != info["commit"]
    ):
        raise ValueError("Release tag does not identify the source commit")


def build(args):
    info = source()
    if args.tag:
        check_tag(args.tag, info)
    system = run("nix", "eval", "--impure", "--raw", "--expr", "builtins.currentSystem")
    kind, arch = SYSTEMS[system]
    output = args.directory.resolve()
    output.mkdir(parents=True, exist_ok=True)
    record = output / f"build-{system}.json"
    record.unlink(missing_ok=True)
    artifacts = []
    # Archive the commit so ignored files, checkout paths, and JJ/Git metadata
    # cannot influence the flake source. Dependencies may come from caches.
    with tempfile.TemporaryDirectory(prefix="cdk-release-") as scratch:
        snapshot = Path(scratch) / "source"
        snapshot.mkdir()
        archive = Path(scratch) / "source.tar"
        subprocess.run(
            git_command("archive", "--format=tar", "-o", str(archive), info["commit"]),
            cwd=ROOT,
            check=True,
        )
        subprocess.run(["tar", "-xf", str(archive), "-C", str(snapshot)], check=True)
        for binary in args.binaries or BINARIES:
            package = f"{binary}-{kind}"
            installable = f"path:{snapshot}#{package}"
            command = [
                "nix",
                "build",
                "--accept-flake-config",
                "--no-update-lock-file",
                "--no-link",
                "-L",
            ]
            result = json.loads(run(*command, "--json", installable))[0]
            # Unlike a second normal build, --rebuild runs the final derivation
            # again and fails if its output differs from the existing output.
            if args.check_reproducibility:
                subprocess.run(
                    [*command, "--rebuild", installable], cwd=ROOT, check=True
                )
            filename = f"{binary}-{info['version']}-{arch}"
            built = Path(result["outputs"]["out"]) / "bin" / filename
            subprocess.run([str(built), "--version"], check=True)
            subprocess.run(
                [str(built), "--help"], stdout=subprocess.DEVNULL, check=True
            )
            shutil.copyfile(built, output / filename)
            (output / filename).chmod(0o755)
            artifacts.append(
                {
                    "name": filename,
                    "sha256": digest(built),
                    "package": package,
                    "derivation": result["drvPath"],
                }
            )
    write_json(
        record,
        {
            "source": info,
            "system": system,
            "rebuild_checked": args.check_reproducibility,
            "artifacts": artifacts,
        },
    )


def inventory(directory):
    records = [
        json.loads((directory / f"build-{system}.json").read_text())
        for system in SYSTEMS
    ]
    info = records[0]["source"]
    files = []
    for system, record in zip(SYSTEMS, records):
        kind, arch = SYSTEMS[system]
        expected = {
            f"{binary}-{info['version']}-{arch}": f"{binary}-{kind}"
            for binary in BINARIES
        }
        if (
            record["source"] != info
            or record["system"] != system
            or type(record["rebuild_checked"]) is not bool
        ):
            raise ValueError("Inconsistent source or invalid build metadata")
        if len(record["artifacts"]) != len(expected):
            raise ValueError("Incomplete binary matrix")
        seen = set()
        for artifact in record["artifacts"]:
            name = artifact["name"]
            if (
                name not in expected
                or name in seen
                or artifact["package"] != expected[name]
            ):
                raise ValueError("Unexpected or duplicate binary")
            if digest(directory / name) != artifact["sha256"]:
                raise ValueError(f"Checksum mismatch: {name}")
            seen.add(name)
            files.append(name)
    return info, records, sorted(files)


def assemble(args):
    info, records, files = inventory(args.directory)
    check_tag(args.tag, info)
    write_json(
        args.directory / "BINARY-MANIFEST.json",
        {"schema": 1, "tag": args.tag, "builds": records},
    )
    files.append("BINARY-MANIFEST.json")
    (args.directory / "SHA256SUMS").write_text(
        "".join(f"{digest(args.directory / name)}  {name}\n" for name in sorted(files))
    )


def publish(args):
    info, records, files = inventory(args.directory)
    check_tag(args.tag, info)
    manifest = json.loads((args.directory / "BINARY-MANIFEST.json").read_text())
    if manifest != {"schema": 1, "tag": args.tag, "builds": records}:
        raise ValueError("Manifest differs from checked builds")
    files.append("BINARY-MANIFEST.json")
    expected = "".join(
        f"{digest(args.directory / name)}  {name}\n" for name in sorted(files)
    )
    if (args.directory / "SHA256SUMS").read_text() != expected:
        raise ValueError("Checksum inventory differs from checked builds")
    files.append("SHA256SUMS")
    # Resolve the remote tag immediately before upload, including annotated tags.
    remote = run(
        "git",
        "ls-remote",
        f"https://github.com/{args.repo}.git",
        f"refs/tags/{args.tag}",
        f"refs/tags/{args.tag}^{{}}",
    )
    refs = dict(line.split()[::-1] for line in remote.splitlines())
    if (
        refs.get(f"refs/tags/{args.tag}^{{}}", refs.get(f"refs/tags/{args.tag}"))
        != info["commit"]
    ):
        raise ValueError("Remote release tag differs from the built commit")
    release = json.loads(
        run("gh", "release", "view", args.tag, "--repo", args.repo, "--json", "assets")
    )
    existing = {asset["name"] for asset in release["assets"]}
    # Preflight every existing asset before uploading anything. A retry may fill
    # missing assets, but must never replace different bytes under the same tag.
    with tempfile.TemporaryDirectory(prefix="cdk-assets-") as scratch:
        for name in files:
            if name in existing:
                run(
                    "gh",
                    "release",
                    "download",
                    args.tag,
                    "--repo",
                    args.repo,
                    "--pattern",
                    name,
                    "--dir",
                    scratch,
                )
                if digest(Path(scratch) / name) != digest(args.directory / name):
                    raise ValueError(f"Existing release asset differs: {name}")
        missing = [
            str((args.directory / name).resolve())
            for name in files
            if name not in existing
        ]
        if missing:
            run("gh", "release", "upload", args.tag, "--repo", args.repo, *missing)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for command in ("build", "assemble", "publish"):
        child = commands.add_parser(command)
        child.add_argument("--directory", type=Path, default=ROOT / "static-bin")
        child.add_argument("--tag", required=command != "build")
        if command == "build":
            child.add_argument("--binaries", nargs="+", choices=BINARIES)
            child.add_argument(
                "--check-reproducibility",
                action="store_true",
                help="Force a second build and compare outputs (disabled by default)",
            )
        if command == "publish":
            child.add_argument("--repo", default="cashubtc/cdk")
        child.set_defaults(action=globals()[command])
    args = parser.parse_args()
    if hasattr(args, "repo") and not re.fullmatch(r"[\w.-]+/[\w.-]+", args.repo):
        parser.error("Invalid GitHub repository")
    try:
        args.action(args)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release failed: {error}\n")


if __name__ == "__main__":
    main()
