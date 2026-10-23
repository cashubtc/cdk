"""Tests for PR context embedded in signed local merges.

Run with: python3 -m unittest discover -s misc/tests -p 'test_merge_pr_message.py'
"""

import importlib.util
import io
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "merge_pr_message", Path(__file__).parents[1] / "merge_pr_message.py"
)
pr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pr)


class MergePrMessageTests(unittest.TestCase):
    def setUp(self):
        self.head = "a" * 40
        self.data = {
            "state": "open", "base": {"ref": "main"},
            "head": {"sha": self.head}, "title": "Fix parsing",
            "body": "Reason\n\n- Preserve `literal` text and $(commands).",
        }

    def test_both_provider_urls(self):
        for url, github in [
            ("https://github.com/cashubtc/cdk/pull/123", True),
            ("https://git.cashu.dev/cashudevkit/cdk/pulls/456", False),
        ]:
            with self.subTest(url=url):
                parsed, owner, repo, number, actual = pr.parse_pr_url(url)
                self.assertEqual(actual, github)
                result = pr.format_message(self.data, url, self.head, number, "")
                self.assertEqual(result, f"Merge PR #{number}: Fix parsing\n\n"
                                 f"Pull-request: {url}")
                self.assertEqual(pr.repository_identity(f"git@{parsed.hostname}:{owner}/{repo}.git"),
                                 (parsed.hostname, f"{owner}/{repo}"))

    def test_reject_ambiguous_or_unsafe_urls(self):
        for url in ["123", "http://host/owner/repo/pulls/1",
                    "https://user:secret@host/owner/repo/pulls/1",
                    "https://github.com/o/r/pull/1?x=y", "https://github.com/o/r/pulls/1",
                    "https://host/o/r/pulls/0"]:
            with self.subTest(url=url), self.assertRaises(ValueError):
                pr.parse_pr_url(url)

    def test_reject_changed_tip_or_ineligible_pr(self):
        for field, value in [("state", "closed"), ("draft", True), ("merged", True),
                             ("head", {"sha": "b" * 40}), ("base", {"ref": "other"})]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                pr.format_message({**self.data, field: value}, "url", self.head, "1", "")

    def test_sanitized_message_replaces_title(self):
        result = pr.format_message(self.data, "url", self.head, "1", "Fix a security issue")
        self.assertEqual(result, "Fix a security issue\n\nPull-request: url")

    def test_tip_mismatch_reports_both_hashes(self):
        rewritten = "b" * 40
        with self.assertRaises(ValueError) as error:
            pr.format_message(self.data, "url", rewritten, "123", "")
        self.assertIn(self.head, str(error.exception))
        self.assertIn(rewritten, str(error.exception))
        self.assertIn("JJ change ID", str(error.exception))

    def test_empty_body(self):
        self.data["body"] = None
        self.assertEqual(pr.format_message(self.data, "url", self.head, "1", ""),
                         "Merge PR #1: Fix parsing\n\nPull-request: url")

    def test_github_uses_explicit_host(self):
        with patch.object(pr.subprocess, "run") as run:
            run.return_value.stdout = json.dumps(self.data)
            result = pr.fetch_pr(*pr.parse_pr_url("https://github.com/o/r/pull/1"))
            self.assertEqual(result, self.data)
            self.assertEqual(run.call_args.args[0],
                             ["gh", "api", "--hostname", "github.com", "repos/o/r/pulls/1"])

    def test_forgejo_api_and_auth(self):
        with patch.dict(pr.os.environ, {"FORGEJO_TOKEN": "test-token"}), \
                patch.object(pr.urllib.request, "build_opener") as build:
            build.return_value.open.return_value = io.BytesIO(json.dumps(self.data).encode())
            result = pr.fetch_pr(*pr.parse_pr_url("https://git.cashu.dev/o/r/pulls/2"))
            self.assertEqual(result, self.data)
            request = build.return_value.open.call_args.args[0]
            self.assertEqual(request.full_url, "https://git.cashu.dev/api/v1/repos/o/r/pulls/2")
            self.assertEqual(request.get_header("Authorization"), "token test-token")
            self.assertIs(build.call_args.args[0], pr.NoRedirect)

    def test_wrong_repository_stops_before_network(self):
        args = ["helper", "https://github.com/o/r/pull/1", self.head,
                "git@github.com:someone/else.git"]
        with patch.object(pr.sys, "argv", args), patch.object(pr, "fetch_pr") as fetch, \
                patch.object(pr.sys, "stderr", io.StringIO()):
            self.assertEqual(pr.main(), 1)
            fetch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
