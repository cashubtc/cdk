"""Release contract tests; no builds or network access required."""

import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release_binaries as release


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.directory = Path(self.scratch.name)
        self.info = {"commit": "a" * 40, "version": "0.1.0", "inputs": {}}
        for system, (kind, arch) in release.SYSTEMS.items():
            artifacts = []
            for binary in release.BINARIES:
                name = f"{binary}-0.1.0-{arch}"
                path = self.directory / name
                path.write_bytes(name.encode())
                artifacts.append(
                    {
                        "name": name,
                        "package": f"{binary}-{kind}",
                        "sha256": release.digest(path),
                        "derivation": "/nix/store/test.drv",
                    }
                )
            release.write_json(
                self.directory / f"build-{system}.json",
                {
                    "source": self.info,
                    "system": system,
                    "rebuild_checked": True,
                    "artifacts": artifacts,
                },
            )
        self.args = argparse.Namespace(
            directory=self.directory, tag="v0.1.0", repo="cashubtc/cdk"
        )

    def edit_record(self, fn):
        path = self.directory / "build-x86_64-linux.json"
        data = json.loads(path.read_text())
        fn(data)
        release.write_json(path, data)

    def test_complete_matrix(self):
        self.assertEqual(len(release.inventory(self.directory)[2]), 9)

    def test_missing_platform(self):
        (self.directory / "build-aarch64-darwin.json").unlink()
        with self.assertRaises(FileNotFoundError):
            release.inventory(self.directory)

    def test_tampered_binary(self):
        (self.directory / "cdk-cli-0.1.0-x86_64").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "Checksum"):
            release.inventory(self.directory)

    def test_invalid_records(self):
        mutations = [
            lambda r: r.update(rebuild_checked="false"),
            lambda r: r.update(source={**self.info, "commit": "b" * 40}),
            lambda r: r["artifacts"].pop(),
            lambda r: r["artifacts"].append(r["artifacts"][0]),
            lambda r: r["artifacts"][0].update(name="../outside"),
        ]
        original = (self.directory / "build-x86_64-linux.json").read_text()
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                (self.directory / "build-x86_64-linux.json").write_text(original)
                self.edit_record(mutate)
                with self.assertRaises(ValueError):
                    release.inventory(self.directory)

    def test_tag_version_and_commit(self):
        with patch.object(release, "run", return_value="b" * 40):
            for tag in ("v9.0.0", "v0.1.0"):
                with self.assertRaises(ValueError):
                    release.check_tag(tag, self.info)

    @patch.object(release, "check_tag")
    def test_publication_refuses_existing_different_bytes(self, _check):
        release.assemble(self.args)
        name = "cdk-cli-0.1.0-x86_64"
        calls = []

        def run(*args):
            calls.append(args)
            if args[:2] == ("git", "ls-remote"):
                return self.info["commit"] + "\trefs/tags/v0.1.0"
            if args[:3] == ("gh", "release", "view"):
                return json.dumps({"assets": [{"name": name}]})
            if args[:3] == ("gh", "release", "download"):
                (Path(args[-1]) / name).write_bytes(b"different")
                return ""
            self.fail(f"Unexpected upload: {args}")

        with patch.object(release, "run", side_effect=run):
            with self.assertRaisesRegex(ValueError, "Existing release asset differs"):
                release.publish(self.args)
        self.assertFalse(any("upload" in call for call in calls))

    @patch.object(release, "check_tag")
    def test_publication_resumes_missing_assets(self, _check):
        release.assemble(self.args)
        name = "cdk-cli-0.1.0-x86_64"
        uploads = []

        def run(*args):
            if args[:2] == ("git", "ls-remote"):
                return self.info["commit"] + "\trefs/tags/v0.1.0^{}"
            if args[:3] == ("gh", "release", "view"):
                return json.dumps({"assets": [{"name": name}]})
            if args[:3] == ("gh", "release", "download"):
                (Path(args[-1]) / name).write_bytes(
                    (self.directory / name).read_bytes()
                )
            if args[:3] == ("gh", "release", "upload"):
                uploads.append(args)
            return ""

        with patch.object(release, "run", side_effect=run):
            release.publish(self.args)
        self.assertEqual(len(uploads), 1)
        self.assertNotIn(str(self.directory / name), uploads[0])
        self.assertNotIn("--clobber", uploads[0])
        self.assertEqual(len(uploads[0][6:]), 10)

    @patch.object(release, "check_tag")
    def test_publication_rejects_moved_tag(self, _check):
        release.assemble(self.args)
        with patch.object(
            release, "run", return_value="b" * 40 + "\trefs/tags/v0.1.0"
        ) as run:
            with self.assertRaisesRegex(ValueError, "Remote release tag differs"):
                release.publish(self.args)
        self.assertEqual(run.call_count, 1)

    @patch.object(release, "check_tag")
    def test_publication_checks_manifest_and_checksums_before_network(self, _check):
        release.assemble(self.args)
        for filename in ("BINARY-MANIFEST.json", "SHA256SUMS"):
            path = self.directory / filename
            original = path.read_text()
            path.write_text("{}")
            with patch.object(release, "run") as run:
                with self.assertRaises(ValueError):
                    release.publish(self.args)
                run.assert_not_called()
            path.write_text(original)

    @patch.object(release, "check_tag")
    def test_single_build_records_can_be_published(self, _check):
        for system in release.SYSTEMS:
            path = self.directory / f"build-{system}.json"
            record = json.loads(path.read_text())
            record["rebuild_checked"] = False
            release.write_json(path, record)
        release.assemble(self.args)
        manifest = json.loads((self.directory / "BINARY-MANIFEST.json").read_text())
        self.assertTrue(all(r["rebuild_checked"] is False for r in manifest["builds"]))

    def test_rebuild_is_opt_in(self):
        built = self.directory / "store" / "bin"
        built.mkdir(parents=True)
        (built / "cdk-cli-0.1.0-x86_64").write_bytes(b"binary")
        for check in (False, True):
            with self.subTest(check=check):
                args = argparse.Namespace(
                    directory=self.directory,
                    tag=None,
                    binaries=["cdk-cli"],
                    check_reproducibility=check,
                )
                calls = []

                def run(*command, **_kwargs):
                    calls.append(command)
                    if command[1] == "eval":
                        return "x86_64-linux"
                    return json.dumps(
                        [{"outputs": {"out": str(built.parent)}, "drvPath": "test.drv"}]
                    )

                with (
                    patch.object(release, "source", return_value=self.info),
                    patch.object(
                        release, "git_command", side_effect=lambda *args: ["git", *args]
                    ),
                    patch.object(release, "run", side_effect=run),
                    patch.object(
                        release.subprocess,
                        "run",
                        side_effect=lambda command, **kw: calls.append(command),
                    ),
                ):
                    release.build(args)
                builds = [c for c in calls if list(c[:2]) == ["nix", "build"]]
                self.assertEqual(len(builds), 2 if check else 1)
                self.assertEqual(any("--rebuild" in c for c in builds), check)
                record = json.loads(
                    (self.directory / "build-x86_64-linux.json").read_text()
                )
                self.assertIs(record["rebuild_checked"], check)

    def test_failed_rebuild_does_not_emit_record(self):
        args = argparse.Namespace(
            directory=self.directory,
            tag=None,
            binaries=["cdk-cli"],
            check_reproducibility=True,
        )
        record = self.directory / "build-x86_64-linux.json"
        built = self.directory / "store" / "bin"
        built.mkdir(parents=True)

        def run(*command, **_kwargs):
            if command[1] == "eval":
                return "x86_64-linux"
            return json.dumps(
                [{"outputs": {"out": str(built.parent)}, "drvPath": "test.drv"}]
            )

        calls = []

        def invoke(command, **_kwargs):
            calls.append(command)
            if "--rebuild" in command:
                raise subprocess.CalledProcessError(1, command)

        with (
            patch.object(release, "source", return_value=self.info),
            patch.object(
                release, "git_command", side_effect=lambda *args: ["git", *args]
            ),
            patch.object(release, "run", side_effect=run),
            patch.object(release.subprocess, "run", side_effect=invoke),
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                release.build(args)
        self.assertTrue(any("--rebuild" in command for command in calls))
        self.assertFalse(record.exists())


if __name__ == "__main__":
    unittest.main()
