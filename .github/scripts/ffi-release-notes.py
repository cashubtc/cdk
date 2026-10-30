"""Generate binding release notes from the corresponding CDK source history."""

import argparse
import json
import re
import subprocess
from pathlib import Path
from urllib.parse import quote


def api(endpoint, *, paginate=False, fields=None):
    command = ["gh", "api", endpoint]
    if paginate:
        command += ["--paginate", "--slurp"]
    if fields is not None:
        command += ["--method", "POST"]
        for key, value in fields.items():
            command += ["--raw-field", f"{key}={value}"]
    result = subprocess.run(command, check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def version(tag):
    """SemVer ordering, including numeric prerelease identifiers (rc.9 < rc.10)."""
    match = re.fullmatch(
        r"v(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?",
        tag,
    )
    if not match:
        raise ValueError(f"Not a version tag: {tag}")
    major, minor, patch, prerelease = match.groups()
    identifiers = tuple(
        (0, int(part)) if part.isdigit() else (1, part)
        for part in (prerelease or "").split(".")
    )
    return int(major), int(minor), int(patch), prerelease is None, identifiers


def previous_release(releases, tag, *, nightly, prerelease):
    releases = [r for r in releases if not r["draft"] and r["tag_name"] != tag]
    if nightly:
        # Nightly version cores can lag stable patch releases. Publication order,
        # rather than SemVer, identifies the preceding nightly source snapshot.
        nightlies = [r for r in releases if "-nightly." in r["tag_name"]]
        candidates = nightlies or [r for r in releases if not r["prerelease"]]
        return max(candidates, key=lambda r: r["published_at"], default=None)

    candidates = []
    for release in releases:
        name = release["tag_name"]
        if "-nightly." in name:
            continue
        try:
            candidate = version(name)
        except ValueError:
            continue
        if not prerelease and (release["prerelease"] or not candidate[3]):
            continue
        if candidate < version(tag):
            candidates.append(release)
    return max(candidates, key=lambda r: version(r["tag_name"]), default=None)


def source_ref(release):
    if "-nightly." not in release["tag_name"]:
        return release["tag_name"]
    # Existing nightlies already carry this provenance sentence. Never use the
    # binding repo's target_commitish: it is a commit in a different repository.
    match = re.search(
        r"Nightly bindings built from CDK commit ([0-9a-f]{40})\b",
        release["body"] or "",
    )
    if not match:
        raise ValueError(f"Missing CDK source commit in {release['tag_name']}")
    return match[1]


def generate_notes(args):
    pages = api(f"repos/{args.binding_repo}/releases?per_page=100", paginate=True)
    releases = [release for page in pages for release in page]
    previous = previous_release(
        releases, args.tag, nightly=args.nightly, prerelease=args.prerelease
    )
    source_url = f"https://github.com/{args.source_repo}"
    if args.nightly:
        notes = (
            f"Nightly bindings built from CDK commit {args.source_ref}: "
            f"{source_url}/commit/{args.source_ref}\n\n"
        )
    else:
        notes = f"Bindings built from CDK [{args.source_ref}]({source_url}/releases/tag/{args.source_ref}).\n\n"

    if previous is None:
        return (
            notes
            + "Initial binding release; no previous published binding release to compare.\n"
        )

    base = source_ref(previous)
    comparison = f"{quote(base, safe='')}...{quote(args.source_ref, safe='')}"
    if args.nightly:
        # CDK has no nightly tags. The compare API accepts exact source SHAs and
        # pagination keeps the changelog complete even across a long gap.
        pages = api(
            f"repos/{args.source_repo}/compare/{comparison}?per_page=100", paginate=True
        )
        changes = [
            f"* {commit['commit']['message'].splitlines()[0]} ([{commit['sha'][:7]}]({commit['html_url']}))"
            for page in pages
            for commit in page["commits"]
        ]
        notes += "## CDK changes\n\n" + ("\n".join(changes) or "No CDK source changes.")
        notes += f"\n\n**Full Changelog**: {source_url}/compare/{comparison}\n"
    else:
        # Select the baseline in the binding repo (which may skip CDK releases),
        # but generate the actual changelog in the source repo. Supplying the
        # previous tag avoids GitHub's ancestry-based automatic selection.
        generated = api(
            f"repos/{args.source_repo}/releases/generate-notes",
            fields={
                "tag_name": args.tag,
                "target_commitish": args.source_ref,
                "previous_tag_name": base,
            },
        )
        notes += generated["body"] + "\n"
    return notes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binding-repo", required=True)
    parser.add_argument("--source-repo", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--source-ref", required=True)
    parser.add_argument("--nightly", action="store_true")
    parser.add_argument("--prerelease", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    # Write only after every API request succeeds; publication must fail closed
    # rather than silently falling back to an unrelated changelog baseline.
    args.output.write_text(generate_notes(args), encoding="utf-8")


if __name__ == "__main__":
    main()
