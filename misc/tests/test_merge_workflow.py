"""Exercise local merge publication against a rejecting Git remote."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "merge.sh"


class MergeWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cdk-merge-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.repo = self.root / "repo"
        self.remote = self.root / "remote.git"
        self.scratch = self.root / "scratch with spaces"
        self.scratch.mkdir()
        config = self.root / "jj.toml"
        key = self.root / "signing-key"
        config.write_text(
            '[user]\nname = "Merge test"\nemail = "merge@example.com"\n'
            '[signing]\nbehavior = "drop"\nbackend = "ssh"\n'
            f'key = {json.dumps(str(key))}\n'
            '[git]\nsubprocess = true\n'
        )
        self.env = {**os.environ, "JJ_CONFIG": str(config),
                    "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                    "TMPDIR": str(self.scratch)}
        self.run_command("ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(key))
        self.run_command("git", "init", "--bare", str(self.remote))
        self.run_command("jj", "git", "init", str(self.repo))
        (self.repo / "file").write_text("base\n")
        self.jj("describe", "-m", "Base")
        self.jj("bookmark", "create", "main")
        self.base = self.revision("main")
        self.jj("git", "remote", "add", "upstream", str(self.remote))
        self.jj("git", "push", "--remote", "upstream", "--bookmark", "main")
        self.jj("new", "main", "-m", "Reviewed change")
        (self.repo / "feature").write_text("feature\n")
        self.head = self.revision("@")
        self.hook = self.remote / "hooks" / "pre-receive"
        self.hook.write_text("#!/bin/sh\necho 'Test rejection' >&2\nexit 1\n")
        self.hook.chmod(0o755)

    def run_command(self, *args, cwd=None, check=True):
        result = subprocess.run(args, cwd=cwd or self.root, env=self.env,
                                text=True, capture_output=True, timeout=30)
        if check:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def jj(self, *args):
        return self.run_command("jj", *args, cwd=self.repo).stdout.strip()

    def revision(self, rev):
        return self.jj("log", "--no-graph", "-r", rev, "-T", "commit_id")

    def merge(self):
        return self.run_command("bash", str(SCRIPT), self.head, "--publish",
                                cwd=self.repo, check=False)

    def reject_merge(self):
        result = self.merge()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Test rejection", result.stderr)
        self.assertIn("jj git push --remote upstream --bookmark main", result.stderr)
        self.assertIn(f"jj bookmark set main --allow-backwards -r {self.base}", result.stderr)
        signed = self.revision("main")
        self.assertNotEqual(signed, self.base)
        self.assertEqual(self.revision("main@upstream"), self.base)
        self.assertIn(f"Reviewed-tip: {self.head}", result.stdout)
        self.assertIn(f"Base: {self.base}", result.stdout)
        self.assertTrue(list(self.scratch.glob("cdk-merge.*/workspace")))
        return signed

    def test_retry_same_signed_merge_after_rejection(self):
        signed = self.reject_merge()
        self.hook.unlink()
        self.jj("git", "push", "--remote", "upstream", "--bookmark", "main")
        self.assertEqual(self.revision("main@upstream"), signed)

    def test_pr_message_starting_with_dash(self):
        remote_url = "https://github.com/example/repo.git"
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        # Supply forge identity while keeping all Git transport local.
        jj = bin_dir / "jj"
        jj.write_text(
            '#!/bin/sh\nif [ "$*" = "git remote list" ]; then\n'
            f"  echo 'upstream {remote_url}'\nelse\n"
            f'  exec {shlex.quote(shutil.which("jj"))} "$@"\nfi\n'
        )
        jj.chmod(0o755)
        gh = bin_dir / "gh"
        gh.write_text(
            "#!/usr/bin/env python3\nimport json\n"
            f"print(json.dumps({{'state': 'open', 'base': {{'ref': 'main'}}, "
            f"'head': {{'sha': '{self.head}'}}, 'title': 'Original title'}}))\n"
        )
        gh.chmod(0o755)
        self.env["PATH"] = f"{bin_dir}{os.pathsep}{self.env['PATH']}"
        url = "https://github.com/example/repo/pull/1"
        for message in ["-hotfix", "-hotfix follow-up"]:
            with self.subTest(message=message):
                result = self.run_command(
                    "bash", str(SCRIPT), self.head, "--pr", url,
                    "--message", message, cwd=self.repo,
                )
                signed = result.stdout.split("Signed merge: ", 1)[1].splitlines()[0]
                description = self.jj("log", "--no-graph", "-r", signed,
                                      "-T", "description")
                self.assertEqual(description,
                                 f"{message}\n\nPull-request: {url}\n\n"
                                 f"Reviewed-tip: {self.head}\nBase: {self.base}")

    def test_reset_and_prepare_new_merge_after_rejection(self):
        signed = self.reject_merge()
        retry = self.merge()
        self.assertIn("Local main differs from the merge base", retry.stderr)
        self.assertEqual(self.revision("main"), signed)
        self.jj("git", "fetch", "--remote", "upstream", "--branch", "main")
        self.jj("bookmark", "set", "main", "--allow-backwards", "-r", self.base)
        self.hook.unlink()
        result = self.merge()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Published merge", result.stdout)
        self.assertEqual(self.revision("main"), self.revision("main@upstream"))
        self.assertEqual(self.revision(f"merge/{signed[:12]}"), signed)

    def test_conflicted_ancestor_blocks_clean_merge(self):
        (self.repo / "file").write_text("left\n")
        left = self.revision("@")
        self.jj("new", self.base, "-m", "Right")
        (self.repo / "file").write_text("right\n")
        right = self.revision("@")
        self.jj("new", left, right, "-m", "Conflicted intermediate")
        self.assertTrue(self.revision("@ & conflicts()"))
        self.jj("new", "-m", "Resolve conflict")
        (self.repo / "file").write_text("resolved\n")
        self.head = self.revision("@")
        self.assertEqual(self.revision("@ & conflicts()"), "")
        result = self.merge()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("a commit being introduced has conflicts", result.stderr)
        self.assertNotIn("Commit message to be signed", result.stdout)
        self.assertEqual(self.revision("main"), self.base)
