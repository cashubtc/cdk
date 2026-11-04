"""Publish verified candidate bytes without invoking a compiler or packager."""

import base64
import fcntl
import hashlib
import json
import os
import re
import shutil
import time
import urllib.error
import urllib.parse
import urllib.request

from model import (
    ReleaseError,
    archive,
    check_candidate,
    check_collection,
    expected,
    extract,
    inputs,
    inventory,
    read_json,
    recipe_identity,
    require,
    run,
    sha256,
    source_identity,
    tree_inventory,
    write_json,
)


def save(path, value):
    temporary = path.with_suffix(".new")
    write_json(temporary, value)
    with temporary.open("rb") as handle:
        os.fsync(handle.fileno())
    temporary.replace(path)


def tree(directory):
    return tree_inventory(directory, (".git",))


def public_json(url):
    try:
        with urllib.request.urlopen(url, timeout=60) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise


def github(args, manifest, state, state_path):
    destination = args.destination
    repo = inputs(args.root)["destinations"][destination]
    require(
        re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo),
        "Invalid publication repository",
    )
    tag = "v" + manifest["source"]["version"]
    package = f"{destination}.tar.gz"
    stage = args.output / "package"
    if stage.exists():
        shutil.rmtree(stage)
    extract(args.candidate / package, stage)
    provenance = {
        "candidate_sha256": args.sha256,
        "source": manifest["source"],
        "package": inventory(args.candidate, [package]),
    }
    write_json(stage / "cdk-release.json", provenance)
    checkout = args.output / "repository"
    if not checkout.exists():
        run(["git", "clone", "--", f"https://github.com/{repo}.git", checkout])
    remote_tag = run(
        ["git", "ls-remote", "--tags", "origin", f"refs/tags/{tag}"],
        cwd=checkout,
        capture=True,
    )
    if remote_tag:
        run(["git", "fetch", "origin", f"refs/tags/{tag}"], cwd=checkout)
        run(["git", "checkout", "--detach", "FETCH_HEAD"], cwd=checkout)
        require(
            tree(checkout) == tree(stage),
            f"Existing {repo} {tag} does not contain this candidate; refusing to overwrite",
        )
    else:
        # This is an isolated publication checkout. The release is a tag, with
        # no implicit merge or rewrite of the destination's default branch.
        for path in checkout.iterdir():
            if path.name == ".git":
                continue
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()
        shutil.copytree(stage, checkout, dirs_exist_ok=True, symlinks=True)
        run(["git", "add", "--all"], cwd=checkout)
        if run(["git", "diff", "--cached", "--name-only"], cwd=checkout, capture=True):
            run(
                [
                    "git",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-m",
                    f"release: {tag}",
                ],
                cwd=checkout,
            )
        commit = run(["git", "rev-parse", "HEAD"], cwd=checkout, capture=True)
        state["commit"] = commit
        save(state_path, state)
        # Push the immutable remote tag directly; never force an existing tag.
        run(["git", "push", "origin", f"{commit}:refs/tags/{tag}"], cwd=checkout)
    asset_names = {package}
    if destination == "dart":
        asset_names |= {
            name
            for group in manifest["groups"].values()
            for name in group["files"]
            if name.startswith("dart-")
        }
    elif destination == "swift":
        asset_names.add("CashuDevKitFFI.xcframework.zip")
    elif destination == "kotlin":
        asset_names |= {
            name
            for name in expected("android", manifest["source"]["version"])
            if name.startswith("cdk-android-")
        }
    release = public_json(f"https://api.github.com/repos/{repo}/releases/tags/{tag}")
    if release is None:
        command = [
            "gh",
            "release",
            "create",
            tag,
            "--repo",
            repo,
            "--verify-tag",
            "--title",
            f"Release {tag}",
            "--notes",
            f"CDK source: {manifest['source']['commit']}\nCandidate manifest SHA-256: {args.sha256}",
        ]
        if "-" in manifest["source"]["version"]:
            command.append("--prerelease")
        run([*command, *(args.candidate / name for name in sorted(asset_names))])
    else:
        require(
            not release.get("draft"),
            "Existing release is a draft; inspect it before resuming",
        )
        remote_assets = {item["name"]: item for item in release["assets"]}
        require(
            set(remote_assets) <= asset_names, "Existing release has unexpected assets"
        )
        for name in sorted(asset_names):
            if name not in remote_assets:
                # Recover an interrupted release upload without replacing assets.
                run(
                    [
                        "gh",
                        "release",
                        "upload",
                        tag,
                        "--repo",
                        repo,
                        args.candidate / name,
                    ]
                )
                continue
            with urllib.request.urlopen(
                remote_assets[name]["browser_download_url"], timeout=120
            ) as response:
                actual = hashlib.file_digest(response, "sha256").hexdigest()
            require(
                actual == sha256(args.candidate / name),
                f"Published {name} differs; refusing to overwrite",
            )
    state["status"] = "published"
    save(state_path, state)
    print(f"Published {repo} {tag} from the verified candidate")


def portal(endpoint, credentials, data=b"", content_type=None):
    request = urllib.request.Request(
        "https://central.sonatype.com/api/v1/publisher/" + endpoint,
        data=data,
        headers={"Authorization": "Bearer " + credentials},
        method="POST",
    )
    if content_type:
        request.add_header("Content-Type", content_type)
    with urllib.request.urlopen(request, timeout=180) as response:
        return response.read()


def maven(args, manifest, state, state_path):
    username = os.environ.get("SONATYPE_USERNAME")
    password = os.environ.get("SONATYPE_PASSWORD")
    key = os.environ.get("CDK_SIGNING_KEY")
    require(
        username and password and key,
        "Set SONATYPE_USERNAME, SONATYPE_PASSWORD, and CDK_SIGNING_KEY locally",
    )
    credentials = base64.b64encode(f"{username}:{password}".encode()).decode()
    ver = manifest["source"]["version"]
    name = f"cdk-{ver}-{args.sha256}"
    deployment = state.get("deployment")
    if args.deployment_id:
        require(state.get("upload_started"), "No interrupted upload to recover")
        require(
            re.fullmatch(r"[0-9a-fA-F-]{36}", args.deployment_id),
            "Invalid deployment ID",
        )
        require(
            not deployment or deployment == args.deployment_id,
            "Deployment ID differs from recorded upload",
        )
        result = json.loads(portal("status?id=" + args.deployment_id, credentials))
        require(
            result["deploymentName"] == name,
            "Deployment belongs to a different candidate",
        )
        deployment = state["deployment"] = args.deployment_id
        save(state_path, state)
    if not deployment:
        require(
            not state.get("upload_started"),
            "Previous upload outcome is unknown. Retrieve its deployment ID from Central and resume with --deployment-id; do not upload again",
        )
        bundle_root = args.output / "maven"
        if bundle_root.exists():
            shutil.rmtree(bundle_root)
        destination = bundle_root / "org/cashudevkit/cdk-android" / ver
        destination.mkdir(parents=True)
        names = sorted(
            n for n in expected("android", ver) if n.startswith("cdk-android-")
        )
        for filename in names:
            artifact = destination / filename
            shutil.copyfile(args.candidate / filename, artifact)
            run(["gpg", "--armor", "--detach-sign", "--local-user", key, artifact])
            for algorithm in ("md5", "sha1"):
                value = hashlib.new(
                    algorithm, artifact.read_bytes(), usedforsecurity=False
                ).hexdigest()
                artifact.with_name(filename + "." + algorithm).write_text(value)
        # Only the outer transport bundle and signatures are newly created.
        # All five unsigned Maven payloads remain identical to the candidate.
        bundle = args.output / "central-bundle.zip"
        archive(bundle_root, bundle, manifest["source"]["epoch"])
        state["bundle_sha256"] = sha256(bundle)
        state["upload_started"] = True
        save(state_path, state)
        boundary = "cdk-" + args.sha256
        body = (
            f'--{boundary}\r\nContent-Disposition: form-data; name="bundle"; filename="central-bundle.zip"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode()
            + bundle.read_bytes()
            + f"\r\n--{boundary}--\r\n".encode()
        )
        deployment = (
            portal(
                "upload?"
                + urllib.parse.urlencode(
                    {"name": name, "publishingType": "USER_MANAGED"}
                ),
                credentials,
                body,
                f"multipart/form-data; boundary={boundary}",
            )
            .decode()
            .strip()
        )
        require(
            re.fullmatch(r"[0-9a-fA-F-]{36}", deployment),
            "Unexpected Central deployment ID; inspect the Portal before retrying",
        )
        state["deployment"] = deployment
        save(state_path, state)
    for _ in range(60):
        result = json.loads(portal("status?id=" + deployment, credentials))
        require(
            result["deploymentName"] == name, "Deployment candidate identity mismatch"
        )
        status = result["deploymentState"]
        print(f"Maven deployment {deployment}: {status}", flush=True)
        if status == "PUBLISHED":
            state["status"] = "published"
            save(state_path, state)
            return
        require(
            status != "FAILED",
            "Central validation failed: " + json.dumps(result.get("errors", {})),
        )
        if status == "VALIDATED":
            portal("deployment/" + deployment, credentials)
        time.sleep(5)
    raise ReleaseError(
        f"Deployment {deployment} is still processing; rerun the same command to resume"
    )


def publish(args):
    require(
        not any(
            os.environ.get(name, "").lower() not in ("", "false", "0")
            for name in ("CI", "GITHUB_ACTIONS", "GITLAB_CI", "TF_BUILD")
        ),
        "Final publication is local only",
    )
    require(args.confirm == args.sha256, "Confirm the exact manifest SHA-256")
    manifest = check_candidate(args.candidate, args.sha256)
    check_collection(args.candidate, args.sha256, args.collected)
    source, _ = source_identity(args.root)
    require(
        recipe_identity(args.root, source) == manifest["source"],
        "Publish from the candidate's exact recipe checkout",
    )
    args.output.mkdir(parents=True, exist_ok=True)
    with (args.output / "publish.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ReleaseError(
                "Another publisher is using this state directory"
            ) from error
        state_path = args.output / "publication.json"
        state = (
            read_json(state_path)
            if state_path.exists()
            else {
                "schema": 1,
                "candidate_sha256": args.sha256,
                "destination": args.destination,
            }
        )
        require(
            state.get("schema") == 1
            and state.get("candidate_sha256") == args.sha256
            and state.get("destination") == args.destination,
            "Publication directory belongs to a different candidate or destination",
        )
        save(state_path, state)
        if args.destination == "maven":
            maven(args, manifest, state, state_path)
        else:
            github(args, manifest, state, state_path)
