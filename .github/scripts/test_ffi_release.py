"""Regression checks for release notes and the actual workflow merge commands."""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parent
ROOT = SCRIPTS.parent.parent
SPEC = importlib.util.spec_from_file_location("notes", SCRIPTS / "ffi-release-notes.py")
NOTES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTES)
SHA = "a" * 40
PREVIOUS_SHA = "b" * 40


def release(
    tag, *, prerelease=False, draft=False, body="", published="2026-10-01T00:00:00Z"
):
    return {
        "tag_name": tag,
        "prerelease": prerelease,
        "draft": draft,
        "body": body,
        "published_at": published,
    }


class ReleaseNotesTests(unittest.TestCase):
    def generate(
        self, releases, *, tag="v0.18.3", nightly=False, prerelease=False, response=None
    ):
        self.calls = []

        def gh(command, **kwargs):
            self.calls.append(command)
            if "/releases?" in command[2]:
                data = releases
            elif response is not None:
                data = response
            else:
                data = {
                    "body": "## What's Changed\n* CDK wallet fix\n\n**Full Changelog**: https://github.com/cashubtc/cdk/compare/v0.18.2...v0.18.3"
                }
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps(data))

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "notes.md"
            argv = [
                "ffi-release-notes.py",
                "--binding-repo",
                "cashubtc/cdk-kotlin",
                "--source-repo",
                "cashubtc/cdk",
                "--tag",
                tag,
                "--source-ref",
                SHA if nightly else tag,
                "--output",
                str(output),
            ]
            if nightly:
                argv += ["--nightly"]
            if prerelease:
                argv += ["--prerelease"]
            with (
                patch.object(sys, "argv", argv),
                patch.object(NOTES.subprocess, "run", side_effect=gh),
            ):
                NOTES.main()
            return output.read_text()

    def test_stable_uses_binding_baseline_and_cdk_notes_across_pages(self):
        body = self.generate(
            [
                [
                    release("v0.18.4"),
                    release("v0.18.3"),
                    release("v0.19.0-rc.0", prerelease=True),
                ],
                [
                    release("v0.18.3-nightly.20261001.gaaaaaaa", prerelease=True),
                    release("v0.18.2"),
                ],
            ]
        )
        self.assertIn("CDK wallet fix", body)
        self.assertIn("cashubtc/cdk/compare/v0.18.2...v0.18.3", body)
        self.assertIn("--paginate", self.calls[0])
        self.assertIn("previous_tag_name=v0.18.2", self.calls[1])
        self.assertEqual(self.calls[1][2], "repos/cashubtc/cdk/releases/generate-notes")

    def test_stable_excludes_rc_even_if_not_marked_prerelease(self):
        self.generate([[release("v0.18.2"), release("v0.18.3-rc.1")]])
        self.assertIn("previous_tag_name=v0.18.2", self.calls[1])

    def test_binding_that_skips_a_version_includes_intervening_cdk_changes(self):
        self.generate([[release("v0.18.1")]])
        self.assertEqual(
            self.calls[0][2], "repos/cashubtc/cdk-kotlin/releases?per_page=100"
        )
        self.assertIn("previous_tag_name=v0.18.1", self.calls[1])

    def test_rc_orders_numeric_identifiers(self):
        self.generate(
            [
                [
                    release("v0.18.2"),
                    release("v0.18.3-rc.9", prerelease=True),
                    release("v0.18.3-rc.10", prerelease=True),
                ]
            ],
            tag="v0.18.3-rc.11",
            prerelease=True,
        )
        self.assertIn("previous_tag_name=v0.18.3-rc.10", self.calls[1])

    def test_backport_excludes_higher_version_even_if_published_earlier(self):
        self.generate(
            [
                [
                    release("v0.18.3"),
                    release("v0.17.6"),
                    release("v0.17.7", draft=True),
                    release("not-a-version"),
                ]
            ],
            tag="v0.17.8",
        )
        self.assertIn("previous_tag_name=v0.17.6", self.calls[1])

    def test_initial_release_does_not_guess_a_baseline(self):
        body = self.generate([[]])
        self.assertIn("Initial binding release", body)
        self.assertEqual(len(self.calls), 1)

    def test_nightly_uses_source_sha_and_all_compare_pages(self):
        body = self.generate(
            [
                [release("v0.18.3", published="2026-10-03T00:00:00Z")],
                [
                    release(
                        "v0.18.0-nightly.20261002.gbbbbbbb",
                        prerelease=True,
                        body=f"Nightly bindings built from CDK commit {PREVIOUS_SHA}: https://example.com",
                    )
                ],
            ],
            tag="v0.18.0-nightly.20261004.gaaaaaaa",
            nightly=True,
            prerelease=True,
            response=[
                {
                    "commits": [
                        {
                            "sha": SHA,
                            "html_url": "https://example.com/commit",
                            "commit": {"message": "CDK fix\n\nDetails"},
                        }
                    ]
                },
                {
                    "commits": [
                        {
                            "sha": PREVIOUS_SHA,
                            "html_url": "https://example.com/other",
                            "commit": {"message": "Another fix"},
                        }
                    ]
                },
            ],
        )
        self.assertIn(f"Nightly bindings built from CDK commit {SHA}", body)
        self.assertIn(f"/compare/{PREVIOUS_SHA}...{SHA}", body)
        self.assertIn("Another fix", body)
        self.assertNotIn("Details", body)

    def test_first_nightly_compares_to_latest_stable(self):
        body = self.generate(
            [[release("v0.18.2"), release("v0.19.0-rc.1", prerelease=True)]],
            nightly=True,
            response=[{"commits": []}],
        )
        self.assertIn(f"/compare/v0.18.2...{SHA}", body)
        self.assertIn("No CDK source changes", body)

    def test_missing_nightly_provenance_fails(self):
        with self.assertRaisesRegex(ValueError, "Missing CDK source commit"):
            self.generate(
                [[release("v0.18.0-nightly.20261002.gbbbbbbb", prerelease=True)]],
                nightly=True,
            )

    def test_api_errors_propagate(self):
        with (
            patch.object(
                NOTES.subprocess,
                "run",
                side_effect=subprocess.CalledProcessError(1, "gh"),
            ),
            self.assertRaises(subprocess.CalledProcessError),
        ):
            NOTES.api("repos/cashubtc/cdk-kotlin/releases")


def merge_step(workflow):
    step = workflow.split("      - name: Merge to default branch\n", 1)[1].split(
        "      - name:", 1
    )[0]
    return "\n".join(
        line[10:] for line in step.split("        run: |\n", 1)[1].splitlines()
    )


class WorkflowAncestryTests(unittest.TestCase):
    def test_tagged_commits_survive_two_releases_in_every_workflow(self):
        # Execute each real merge step in a disposable repository. Checking file
        # contents alone would miss the original squash-induced ancestry loss.
        for language in ("dart", "go", "kotlin", "swift"):
            with (
                self.subTest(language=language),
                tempfile.TemporaryDirectory() as directory,
            ):
                directory = Path(directory)
                remote = directory / "remote.git"
                work = directory / "work"
                peer = directory / "peer"
                env = dict(
                    os.environ,
                    GIT_CONFIG_NOSYSTEM="1",
                    GIT_CONFIG_GLOBAL=os.devnull,
                    GIT_AUTHOR_NAME="Test",
                    GIT_AUTHOR_EMAIL="test@example.com",
                    GIT_COMMITTER_NAME="Test",
                    GIT_COMMITTER_EMAIL="test@example.com",
                )

                def git(*args, cwd=work, env=env):
                    return subprocess.run(
                        ["git", *args],
                        cwd=cwd,
                        env=env,
                        check=True,
                        capture_output=True,
                        text=True,
                    ).stdout.strip()

                git(
                    "init",
                    "--bare",
                    "--initial-branch=main",
                    str(remote),
                    cwd=directory,
                )
                git("clone", str(remote), str(work), cwd=directory)
                git("commit", "--allow-empty", "-m", "Initial")
                git("push", "origin", "main")
                git("remote", "set-head", "origin", "--auto")
                git("clone", str(remote), str(peer), cwd=directory)
                workflow = (
                    ROOT / f".github/workflows/{language}-publish.yml"
                ).read_text()
                self.assertIn(
                    "if: ${{ !inputs.nightly }}",
                    workflow.split("- name: Merge to default branch", 1)[1],
                )
                self.assertIn(
                    '--notes-file "${GITHUB_WORKSPACE}/release-notes.md"', workflow
                )
                self.assertNotIn("--generate-notes", workflow)
                for tag in ("v0.18.4", "v0.18.5"):
                    branch = f"release/{tag}"
                    git("checkout", "-b", branch)
                    git("commit", "--allow-empty", "-m", f"Artifacts for {tag}")
                    git("tag", tag)
                    git("push", "origin", branch)
                    # Simulate main advancing while a release builds.
                    git("pull", "--ff-only", "origin", "main", cwd=peer)
                    git(
                        "commit",
                        "--allow-empty",
                        "-m",
                        "Concurrent main update",
                        cwd=peer,
                    )
                    git("push", "origin", "main", cwd=peer)
                    step_env = dict(
                        env,
                        TAG=tag,
                        RELEASE_BRANCH=branch,
                        FFI_DEPLOY_KEY="test",
                        GITHUB_ENV=str(directory / "env"),
                    )
                    result = subprocess.run(
                        ["bash", "-euo", "pipefail", "-c", merge_step(workflow)],
                        cwd=work,
                        env=step_env,
                        check=False,
                        capture_output=True,
                        text=True,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                    git("merge-base", "--is-ancestor", tag, "origin/main")
                git("merge-base", "--is-ancestor", "v0.18.4", "v0.18.5")


if __name__ == "__main__":
    unittest.main()
