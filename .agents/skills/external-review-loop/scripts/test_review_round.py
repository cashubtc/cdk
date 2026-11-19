"""Offline launcher checks. Fake Git and reviewer CLIs run only in a temporary directory."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


LAUNCHER = Path(__file__).with_name("review_round.py").resolve()
BASE, HEAD = "1" * 40, "2" * 40
REPORT = """Review status: complete
## Standards
No findings.
## Spec
No findings.
## Bugs and regressions
No findings.
## Coverage
Read the diff and inspected the caller.
"""

GIT = """
import os, sys
repo, command, *args = sys.argv[2:]
if command == 'rev-parse':
    answers = {
        '--show-toplevel': repo,
        '--abbrev-ref': os.getenv('BRANCH', 'HEAD'),
        'HEAD': '2' * 40,
        '--verify': '1' * 40,
    }
    print(answers[args[0]])
elif command == 'status':
    print(os.getenv('DIRTY', ''))
elif command == 'diff':
    assert args[-2:] == ['1' * 40 + '...' + '2' * 40, '--']
    print('pinned diff')
else:
    sys.exit(1)
"""

REVIEWER = """
import json, os, sys, time
from pathlib import Path
args = sys.argv[1:]
Path('called.json').write_text(json.dumps(args))
assert sys.stdin.read() == Path('prompt.md').read_text()
assert Path('diff.patch').read_text() == 'pinned diff\\n'
time.sleep(int(os.getenv('DELAY', 0)))
if 'FAIL' in os.environ:
    sys.exit(3)
report = os.environ['REPORT']
if Path(sys.argv[0]).name == 'codex':
    Path(args[args.index('--output-last-message') + 1]).write_text(report)
else:
    print(json.dumps({
        'subtype': 'success', 'result': report,
        'modelUsage': {os.getenv('MODEL', args[args.index('--model') + 1]): {}},
    }))
"""


class LauncherTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.repo = self.root / "review checkout"
        self.repo.mkdir()
        self.context = self.root / "context.md"
        self.context.write_text("Check the linked issue's acceptance criteria.")
        self.context.chmod(0o600)
        for name, script in [("git", GIT), ("codex", REVIEWER), ("claude", REVIEWER)]:
            path = self.root / name
            path.write_text(f"#!{sys.executable}\n" + script)
            path.chmod(0o700)
        self.env = {"PATH": str(self.root), "REPORT": REPORT}

    def run_round(self, *extra, reviewer="codex", folder="round"):
        self.output = self.root / folder
        return subprocess.run(
            [sys.executable, LAUNCHER, "--repo", self.repo, "--context", self.context,
             "--output-dir", self.output, "--reviewer", reviewer, "--base", BASE,
             "--head", HEAD, "--pr", "https://github.com/cashubtc/cdk/pull/1",
             "--round", "1", *extra],
            env=self.env, text=True, capture_output=True,
        )

    def test_both_reviewers(self):
        for reviewer, model, effort in [
            ("claude", "claude-opus-5-5", "high"),
            ("codex", "gpt-6.2-sol", 'model_reasoning_effort="high"'),
        ]:
            with self.subTest(reviewer=reviewer):
                result = self.run_round(reviewer=reviewer, folder=f"round-{reviewer}")
                self.assertEqual(result.returncode, 0, result.stderr)
                args = json.loads((self.output / "called.json").read_text())
                self.assertEqual(args[args.index("--model") + 1], model)
                self.assertIn(effort, args)
                self.assertEqual((self.output / "review.md").read_text(), REPORT)
                for path in [self.output, *self.output.iterdir()]:
                    self.assertEqual(path.stat().st_mode & 0o077, 0)

    def test_dry_run_renders_cdk_sources_without_launch(self):
        result = self.run_round("--dry-run")
        self.assertEqual(result.returncode, 0, result.stderr)
        prompt = (self.output / "prompt.md").read_text()
        for expected in ("name: cdk-code-review", "# CDK Code Review Guidelines",
                         self.context.read_text(), BASE, HEAD, str(self.repo)):
            self.assertIn(expected, prompt)
        self.assertFalse((self.output / "called.json").exists())
        self.assertFalse((self.output / "review.md").exists())
        self.assertNotEqual(self.run_round().returncode, 0)  # Preserve prior artifacts.

    def test_invalid_inputs_prevent_launch(self):
        for extra, env in [
            ([], {"BRANCH": "main"}), ([], {"DIRTY": "?? private.txt"}),
            (["--head", BASE], {}), (["--base", "main"], {}),
            (["--output-dir", str(self.repo / "round")], {}),
        ]:
            with self.subTest(extra=extra, env=env):
                self.env.update(env)
                self.assertNotEqual(self.run_round(*extra).returncode, 0)
                self.assertFalse(self.output.exists())
                for key in env:
                    del self.env[key]
        self.context.chmod(0o644)
        self.assertNotEqual(self.run_round().returncode, 0)
        self.assertFalse(self.output.exists())

    def test_incomplete_followup_preserves_report(self):
        result = self.run_round("--round", "2")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual((self.output / "review.md").read_text(), REPORT)

    def test_empty_coverage_is_incomplete(self):
        self.env["REPORT"] = REPORT.split("## Coverage")[0] + "## Coverage\n"
        self.assertNotEqual(self.run_round().returncode, 0)
        self.assertTrue((self.output / "review.md").exists())

    def test_wrong_model_failure_and_timeout(self):
        for key, value in [("MODEL", "different-model"), ("FAIL", "1"), ("DELAY", "30")]:
            with self.subTest(key=key):
                self.env[key] = value
                result = self.run_round("--timeout-seconds", "1", reviewer="claude", folder=key)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((self.output / "review.md").exists())
                del self.env[key]


if __name__ == "__main__":
    unittest.main()
