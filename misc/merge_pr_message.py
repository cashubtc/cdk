#!/usr/bin/env python3
"""Read PR context for a local merge without changing the hosting service."""

import argparse
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request


def parse_pr_url(url):
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username
            or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("Use a full HTTPS PR URL without credentials, query or fragment")
    match = re.fullmatch(r"/([\w.-]+)/([\w.-]+)/(pull|pulls)/([1-9][0-9]*)/?", parsed.path)
    if not match:
        raise ValueError("Expected https://HOST/OWNER/REPO/pull[s]/NUMBER")
    owner, repo, kind, number = match.groups()
    github = parsed.hostname == "github.com"
    if kind != ("pull" if github else "pulls"):
        raise ValueError("Use /pull/ for GitHub and /pulls/ for Forgejo")
    return parsed, owner, repo, number, github


def repository_identity(url):
    if "://" not in url:
        match = re.fullmatch(r"(?:[^@]+@)?([^:]+):(.+)", url)
        if not match:
            raise ValueError("PR metadata requires an HTTPS or SSH upstream remote")
        host, path = match.groups()
    else:
        parsed = urllib.parse.urlsplit(url)
        host, path = parsed.hostname, parsed.path
    return host, path.strip("/").removesuffix(".git")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # Do not forward an optional private-repository token to another host.
        return None


def fetch_pr(parsed, owner, repo, number, github):
    endpoint = f"repos/{owner}/{repo}/pulls/{number}"
    if github:
        result = subprocess.run(
            ["gh", "api", "--hostname", "github.com", endpoint],
            check=True, text=True, stdout=subprocess.PIPE,
        )
        return json.loads(result.stdout)
    headers = {"Accept": "application/json"}
    if token := os.environ.get("FORGEJO_TOKEN"):
        headers["Authorization"] = f"token {token}"
    request = urllib.request.Request(
        f"https://{parsed.netloc}/api/v1/{endpoint}", headers=headers,
    )
    with urllib.request.build_opener(NoRedirect).open(request, timeout=30) as response:
        return json.load(response)


def format_message(data, url, head, number, override):
    if data.get("state") != "open" or data.get("merged"):
        raise ValueError("PR is closed or already merged")
    if data.get("draft"):
        raise ValueError("PR is still a draft")
    if data.get("base", {}).get("ref") != "main":
        raise ValueError(f"PR targets {data.get('base', {}).get('ref')!r}, not main")
    pr_head = data.get("head", {}).get("sha")
    if pr_head != head:
        raise ValueError(
            f"Revision resolves to {head}, but PR #{number} points to {pr_head}. "
            "Fetch and review the PR tip, then use its exact commit hash. "
            "A JJ change ID may refer to a rewritten version of that commit."
        )
    if override:
        summary = override
    else:
        title = data.get("title")
        if not isinstance(title, str):
            raise ValueError("PR response is missing a valid title")
        title = " ".join("".join(char for char in title
                                 if char.isprintable() or char.isspace()).split())
        if not title:
            raise ValueError("PR response is missing a valid title")
        summary = f"Merge PR #{number}: {title}"
    return f"{summary}\n\nPull-request: {url}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("url")
    parser.add_argument("head")
    parser.add_argument("remote_url")
    parser.add_argument("--message", default="", help="Replace fetched title with this summary")
    args = parser.parse_args()
    try:
        parsed, owner, repo, number, github = parse_pr_url(args.url)
        if repository_identity(args.remote_url) != (parsed.hostname, f"{owner}/{repo}"):
            raise ValueError("PR URL must belong to the selected upstream remote")
        data = fetch_pr(parsed, owner, repo, number, github)
        print(format_message(data, args.url, args.head, number, args.message))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"Cannot read PR context: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
