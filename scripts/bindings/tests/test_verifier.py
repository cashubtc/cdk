"""Exercise verifier orchestration with a real local Git recipe repository."""

from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import model
import release


class VerifierTests(unittest.TestCase):
    def setUp(self):
        self.work = model.scratch()
        self.addCleanup(shutil.rmtree, self.work)
        self.repo = self.work / "repository"
        self.repo.mkdir()
        (self.repo / "bindings/release").mkdir(parents=True)
        (self.repo / "Cargo.toml").write_text(
            '[workspace.package]\nversion = "0.18.0"\n'
        )
        for name in (
            "Cargo.lock",
            "flake.lock",
            "rust-toolchain.toml",
            "bindings/release/inputs.json",
        ):
            (self.repo / name).write_text("fixture\n")
        model.run(["git", "init", "-q", "--initial-branch=main"], cwd=self.repo)
        model.run(["git", "add", "."], cwd=self.repo)
        model.run(
            [
                "git",
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture",
            ],
            cwd=self.repo,
        )
        source, _ = model.source_identity(self.repo)
        self.identity = model.recipe_identity(self.repo, source)
        self.candidate = self.work / "candidate"
        self.candidate.mkdir()
        self.records = {}
        for group in model.GROUPS:
            for name in model.expected(group, "0.18.0"):
                (self.candidate / name).write_bytes(f"built:{name}".encode())
        for group in model.GROUPS:
            record = {
                "schema": 1,
                "group": group,
                "source": self.identity,
                "files": model.inventory(
                    self.candidate, model.expected(group, "0.18.0")
                ),
                "imports": model.inventory(self.candidate, model.dependencies(group)),
            }
            self.records[group] = record
            model.write_json(self.candidate / f"group-{group}.json", record)
        model.write_json(
            self.candidate / "candidate.json",
            {"schema": 1, "source": self.identity, "groups": self.records},
        )
        self.checksum = model.sha256(self.candidate / "candidate.json")
        self.output = self.work / "verification"
        self.invocations = []

    def fake_build(self, args, **kwargs):
        if str(args[0]).endswith("scripts/bindings/release"):
            self.invocations.append(args)
            group = args[args.index("--group") + 1]
            destination = Path(args[args.index("--output") + 1])
            destination.mkdir()
            for name in model.expected(group, "0.18.0"):
                shutil.copyfile(self.candidate / name, destination / name)
            model.write_json(destination / f"group-{group}.json", self.records[group])
            return None
        return model.run(args, **kwargs)

    def test_verifier_uses_candidate_checkout_and_writes_only_matching_receipts(self):
        with (
            patch.object(release, "run", side_effect=self.fake_build),
            patch.object(release, "supported_groups", return_value=("generated",)),
        ):
            release.verify(
                self.candidate, self.checksum, None, self.output, str(self.repo)
            )
        receipt = model.read_json(self.output / "receipt-generated.json")
        self.assertEqual(receipt["candidate_sha256"], self.checksum)
        self.assertEqual(receipt["source"], self.identity)
        executable = Path(self.invocations[0][0])
        self.assertEqual(executable.parent.parent.parent.name, "recipe")
        self.assertFalse(executable.is_relative_to(self.repo))

    def test_changed_rebuild_has_no_receipt_and_other_groups_continue(self):
        def tamper(args, **kwargs):
            result = self.fake_build(args, **kwargs)
            if (
                str(args[0]).endswith("scripts/bindings/release")
                and "generated" in args
            ):
                destination = Path(args[args.index("--output") + 1])
                (destination / "generated.tar").write_bytes(
                    b"different compilation result"
                )
            return result

        retained = self.work / "retained"
        retained.mkdir()
        with (
            patch.object(release, "run", side_effect=tamper),
            patch.object(release, "scratch", return_value=retained),
            patch.object(
                release, "supported_groups", return_value=("generated", "windows")
            ),
        ):
            with self.assertRaisesRegex(model.ReleaseError, "Failed groups: generated"):
                release.verify(
                    self.candidate, self.checksum, None, self.output, str(self.repo)
                )
        self.assertFalse((self.output / "receipt-generated.json").exists())
        self.assertTrue((self.output / "receipt-windows.json").exists())
        self.assertEqual(len(self.invocations), 2)

    def test_input_identity_mismatch_stops_before_recipe_execution(self):
        manifest = model.read_json(self.candidate / "candidate.json")
        manifest["source"]["inputs"]["flake.lock"] = "0" * 64
        for group, record in manifest["groups"].items():
            record["source"] = manifest["source"]
            model.write_json(self.candidate / f"group-{group}.json", record)
        model.write_json(self.candidate / "candidate.json", manifest)
        checksum = model.sha256(self.candidate / "candidate.json")
        retained = self.work / "retained"
        retained.mkdir()
        with (
            patch.object(release, "run", side_effect=self.fake_build),
            patch.object(release, "scratch", return_value=retained),
            patch.object(release, "supported_groups", return_value=("generated",)),
        ):
            with self.assertRaisesRegex(model.ReleaseError, "identity mismatch"):
                release.verify(
                    self.candidate, checksum, None, self.output, str(self.repo)
                )
        self.assertFalse(self.invocations)


if __name__ == "__main__":
    unittest.main()
