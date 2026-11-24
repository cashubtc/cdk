#!/usr/bin/env python3
"""Build, independently reproduce, collect, and publish CDK binding candidates."""

import argparse
from pathlib import Path
import re
import shutil
import subprocess
import sys
import urllib.request

from build import Builder, build
from model import (
    GROUPS,
    ReleaseError,
    candidate_manifest,
    check_candidate,
    collect,
    dependencies,
    empty_directory,
    host,
    inventory,
    read_json,
    recipe_identity,
    require,
    run,
    scratch,
    sha256,
    supported_groups,
    write_json,
)


def download(repo, tag, checksum, output):
    require(
        re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo),
        "Invalid GitHub repository",
    )
    require(re.fullmatch(r"candidate-[A-Za-z0-9_.-]+", tag), "Invalid candidate tag")
    require(re.fullmatch(r"[0-9a-f]{64}", checksum), "Expected a manifest SHA-256")
    empty_directory(output)
    base = f"https://github.com/{repo}/releases/download/{tag}/"

    def fetch(name):
        require(re.fullmatch(r"[A-Za-z0-9_.+-]+", name), "Unsafe candidate asset name")
        with (
            urllib.request.urlopen(base + name, timeout=120) as response,
            (output / name).open("wb") as destination,
        ):
            shutil.copyfileobj(response, destination)

    fetch("candidate.json")
    require(
        sha256(output / "candidate.json") == checksum,
        "Candidate manifest checksum mismatch",
    )
    manifest = read_json(output / "candidate.json")
    require(
        manifest.get("schema") == 1 and set(manifest["groups"]) == set(GROUPS),
        "Invalid candidate manifest",
    )
    for group, record in manifest["groups"].items():
        fetch(f"group-{group}.json")
        for name in record["files"]:
            fetch(name)
    check_candidate(output, checksum)


def verify(candidate, checksum, groups, output, source_repo):
    manifest = check_candidate(candidate, checksum)
    selected = groups or supported_groups()
    require(selected, f"No groups supported by {host()}")
    require(
        set(selected) <= set(supported_groups()),
        "Selected group is not supported on this host",
    )
    empty_directory(output)
    work = scratch()
    print(f"Verification workspace: {work}", flush=True)
    root = work / "recipe"
    # A trusted checksum identifies executable recipes. No candidate-provided URL
    # or shell command is executed by the downloader.
    run(
        ["git", "clone", "--no-checkout", "--filter=blob:none", "--", source_repo, root]
    )
    revision = manifest["source"]["commit"]
    run(["git", "checkout", "--detach", revision], cwd=root)
    branches = run(
        ["git", "branch", "-r", "--contains", revision], cwd=root, capture=True
    )
    require(
        branches,
        "Candidate recipe is not reachable from a branch in the selected source repository",
    )
    epoch = int(
        run(["git", "show", "-s", "--format=%ct", revision], cwd=root, capture=True)
    )
    require(
        recipe_identity(root, {"commit": revision, "epoch": epoch})
        == manifest["source"],
        "Candidate source/input identity mismatch",
    )
    failed = []
    for group in selected:
        rebuilt = output / f"rebuilt-{group}"
        try:
            # Invoke the candidate's scripts, not scripts from the verifier's
            # current checkout. Each build has a fresh source and target tree.
            command = [
                root / "scripts/bindings/release",
                "build",
                "--group",
                group,
                "--output",
                rebuilt,
            ]
            if dependencies(group):
                command += ["--candidate", candidate]
            run(command)
            record = read_json(rebuilt / f"group-{group}.json")
            require(
                record == manifest["groups"][group],
                f"Independent rebuild differs: {group}",
            )
            require(
                inventory(rebuilt, record["files"]) == record["files"],
                "Rebuilt files changed",
            )
            write_json(
                output / f"receipt-{group}.json",
                {
                    **record,
                    "candidate_sha256": checksum,
                    "host": host(),
                },
            )
            print(f"VERIFIED {group}: complete artifact bytes match", flush=True)
        except (
            ReleaseError,
            subprocess.CalledProcessError,
            OSError,
            ValueError,
            KeyError,
        ) as error:
            failed.append(group)
            print(f"FAILED {group}: {error}", file=sys.stderr, flush=True)
    outstanding = sorted(set(GROUPS) - set(selected))
    print("Groups requiring other verification: " + (", ".join(outstanding) or "none"))
    if failed:
        raise ReleaseError(
            f"Failed groups: {', '.join(failed)}; source retained at {root}"
        )
    shutil.rmtree(work)


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parents[2]
    )
    commands = result.add_subparsers(dest="command", required=True)
    commands.add_parser(
        "groups", help="Show groups this host can independently rebuild"
    )
    for name in ("build", "_build"):
        command = commands.add_parser(
            name, help="Build one group using pinned Nix tools"
        )
        command.add_argument("--group", choices=GROUPS, required=True)
        command.add_argument("--output", type=Path, required=True)
        command.add_argument("--candidate", type=Path)
        if name == "_build":
            command.add_argument("--identity", type=Path, required=True)
    command = commands.add_parser(
        "candidate", help="Seal a complete set of group outputs"
    )
    command.add_argument("--directory", type=Path, required=True)
    command = commands.add_parser(
        "download", help="Download public candidate assets anonymously"
    )
    command.add_argument("--repo", default="cashubtc/cdk")
    command.add_argument("--tag", required=True)
    command.add_argument("--sha256", required=True)
    command.add_argument("--output", type=Path, required=True)
    command = commands.add_parser(
        "verify", help="Rebuild from the candidate's exact source commit"
    )
    command.add_argument("--candidate", type=Path, required=True)
    command.add_argument("--sha256", required=True)
    command.add_argument("--group", action="append", choices=GROUPS)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument("--source-repo", default="https://github.com/cashubtc/cdk.git")
    command = commands.add_parser(
        "collect", help="Validate trusted receipts; require every group"
    )
    command.add_argument("--candidate", type=Path, required=True)
    command.add_argument("--sha256", required=True)
    command.add_argument("--receipts", type=Path, required=True)
    command.add_argument("--output", type=Path, required=True)
    command = commands.add_parser(
        "publish", help="Publish existing verified files from a local machine"
    )
    command.add_argument("--candidate", type=Path, required=True)
    command.add_argument("--sha256", required=True)
    command.add_argument("--collected", type=Path, required=True)
    command.add_argument(
        "--destination",
        choices=("dart", "go", "kotlin", "swift", "maven"),
        required=True,
    )
    command.add_argument(
        "--confirm", required=True, help="Exact candidate manifest SHA-256"
    )
    command.add_argument(
        "--output",
        type=Path,
        required=True,
        help="Persistent publication/retry state directory",
    )
    command.add_argument(
        "--deployment-id",
        help="Recover an interrupted Maven upload using its Portal deployment ID",
    )
    return result


def main():
    args = parser().parse_args()
    for key, value in vars(args).items():
        if isinstance(value, Path):
            setattr(args, key, value.resolve())
    if args.command == "groups":
        print("\n".join(supported_groups()))
    elif args.command == "build":
        build(args.root, args.group, args.output, args.candidate)
    elif args.command == "_build":
        Builder(
            args.root, args.group, args.output, read_json(args.identity), args.candidate
        ).execute()
    elif args.command == "candidate":
        candidate_manifest(args.root, args.directory)
        print(f"Manifest SHA-256: {sha256(args.directory / 'candidate.json')}")
    elif args.command == "download":
        download(args.repo, args.tag, args.sha256, args.output)
    elif args.command == "verify":
        verify(args.candidate, args.sha256, args.group, args.output, args.source_repo)
    elif args.command == "collect":
        collect(args.candidate, args.sha256, args.receipts, args.output)
    elif args.command == "publish":
        from publish import publish

        publish(args)


if __name__ == "__main__":
    try:
        main()
    except (
        ReleaseError,
        subprocess.CalledProcessError,
        OSError,
        ValueError,
        KeyError,
    ) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
