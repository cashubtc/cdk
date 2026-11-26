"""Prepare or run one external CDK review. The calling agent owns publication."""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import stat
from string import Template
import subprocess
import sys


SKILL_DIR = Path(__file__).resolve().parents[1]
CDK_ROOT = SKILL_DIR.parents[2]
MODELS = {"claude": "claude-opus-5-5", "codex": "gpt-6.2-sol"}


def git(repo, *args):
    return subprocess.check_output(["git", "-C", repo, *args], text=True).strip()


def check_checkout(repo, head):
    if git(repo, "rev-parse", "--show-toplevel") != str(repo):
        raise ValueError("--repo must be the root of the review worktree")
    if git(repo, "rev-parse", "--abbrev-ref", "HEAD") != "HEAD":
        raise ValueError("use a detached review worktree")
    if git(repo, "rev-parse", "HEAD") != head:
        raise ValueError("the checkout differs from the published head")
    if git(repo, "status", "--porcelain=v1", "--untracked-files=all", "--ignore-submodules=none"):
        raise ValueError("the review worktree has local changes")


def prepare(args):
    repo = args.repo.resolve(strict=True)
    check_checkout(repo, args.head)
    if not re.fullmatch(r"[0-9a-f]{40}", args.base):
        raise ValueError("--base must be a full commit SHA")
    if git(repo, "rev-parse", "--verify", args.base + "^{commit}") != args.base:
        raise ValueError("the requested base commit is unavailable")
    if not re.fullmatch(r"https://github\.com/[A-Za-z0-9-]+/cdk/pull/[1-9][0-9]*", args.pr):
        raise ValueError("use the canonical GitHub URL for the CDK pull request")

    context = args.context.resolve(strict=True)
    output = args.output_dir.resolve()
    for path in (context, output):
        if path.is_relative_to(repo) or path.is_relative_to(CDK_ROOT):
            raise ValueError("store context and output outside the repository")
    info = context.stat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError("use an owner-only context file")
    evidence = context.read_text(encoding="utf-8").strip()
    if not evidence:
        raise ValueError("the review context is empty")

    template = (SKILL_DIR / "references/reviewer-prompt.md").read_text(encoding="utf-8")
    prompt = Template(template).substitute(
        repo=repo, pr=args.pr, base=args.base, head=args.head, round=args.round,
        skill=(SKILL_DIR.parent / "cdk-code-review/SKILL.md").read_text(encoding="utf-8"),
        guide=(CDK_ROOT / "docs/code-review.md").read_text(encoding="utf-8"),
        context=evidence,
    )
    output.mkdir(mode=0o700)
    with (output / "diff.patch").open("xb") as patch:
        subprocess.run(
            ["git", "-C", repo, "diff", "--no-color", "--no-ext-diff", "--no-textconv",
             f"{args.base}...{args.head}", "--"],
            stdout=patch, check=True,
        )
        if patch.tell() == 0:
            raise ValueError("the pinned PR diff is empty")
    (output / "prompt.md").write_text(prompt, encoding="utf-8")
    return repo, output, prompt


def command_for(reviewer, repo):
    if reviewer == "claude":
        return [
            "claude", "--print", "--model", MODELS[reviewer], "--effort", "high",
            "--no-session-persistence", "--safe-mode", "--strict-mcp-config",
            "--permission-mode", "dontAsk", "--tools", "Read,Glob,Grep",
            "--allowedTools", "Read,Glob,Grep", "--add-dir", str(repo),
            "--output-format", "json",
        ]
    return [
        "codex", "exec", "--model", MODELS[reviewer], "-c", 'model_reasoning_effort="high"',
        "-c", 'approval_policy="never"', "--sandbox", "read-only", "--ignore-user-config",
        "--disable", "apps", "--disable", "hooks", "--disable", "plugins",
        "--skip-git-repo-check", "--ephemeral", "--output-last-message", "answer.md", "-",
    ]


def launch(command, output, prompt, timeout):
    environment = os.environ.copy()
    if command[0] == "claude":
        environment.pop("CLAUDECODE", None)
    with (output / "stdout.log").open("xb") as stdout, (output / "stderr.log").open("xb") as stderr:
        with subprocess.Popen(
            command, cwd=output, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr,
            start_new_session=True, env=environment,
        ) as process:
            try:
                process.communicate(prompt.encode("utf-8"), timeout=timeout)
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                os.killpg(process.pid, signal.SIGKILL)
                raise
            if process.returncode:
                raise ValueError(f"{command[0]} exited with {process.returncode}. Inspect the private CLI logs")


def read_report(reviewer, output):
    if reviewer == "codex":
        return (output / "answer.md").read_text(encoding="utf-8").strip()
    result = json.loads((output / "stdout.log").read_text(encoding="utf-8"))
    if not isinstance(result, dict) or result.get("is_error") or result.get("subtype") != "success":
        raise ValueError("Claude Code reported an error. Inspect the private CLI logs")
    usage = result.get("modelUsage") or {}
    if not any(name == MODELS[reviewer] or name.startswith(MODELS[reviewer] + "-") for name in usage):
        raise ValueError("Claude Code did not confirm use of the requested model")
    if result.get("permission_denials"):
        raise ValueError("Claude Code reported denied tool calls. Inspect the private CLI logs")
    body = result.get("result")
    if not isinstance(body, str):
        raise ValueError("Claude Code returned no report")
    return body.strip()


def covers_required_sections(body, round_number):
    lines = body.splitlines()
    if lines[0] != "Review status: complete":
        return False
    required = {"## Standards", "## Spec", "## Bugs and regressions", "## Coverage"}
    if round_number > 1:
        required.add("## Previous findings")
    section = None
    for line in lines[1:]:
        if line.startswith("## "):
            section = line.strip()
        elif line.strip():
            required.discard(section)
    return not required


def positive_int(value):
    number = int(value)
    if number < 1:
        raise argparse.ArgumentTypeError("expected a positive integer")
    return number


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--reviewer", required=True, choices=MODELS)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--pr", required=True)
    parser.add_argument("--base", required=True)
    parser.add_argument("--head", required=True)
    parser.add_argument("--round", required=True, type=positive_int)
    parser.add_argument("--context", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=positive_int, default=1800)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    repo, output, prompt = prepare(args)
    command = command_for(args.reviewer, repo)
    (output / "command.json").write_text(json.dumps(command, indent=2) + "\n", encoding="utf-8")
    if args.dry_run:
        print("Prepared private inputs. No reviewer launched.")
        return 0

    launch(command, output, prompt, args.timeout_seconds)
    check_checkout(repo, args.head)
    body = read_report(args.reviewer, output)
    if not body:
        raise ValueError("the reviewer returned an empty report")
    (output / "review.md").write_text(body + "\n", encoding="utf-8")
    if not covers_required_sections(body, args.round):
        raise ValueError("partial review saved to review.md. Check its coverage")
    print("Private review saved to review.md. Inspect coverage before publication.")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(f"External review incomplete: {error}")
