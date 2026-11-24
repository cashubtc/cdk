"""Release contracts: reproducible packaging, coverage, identity, and safe retries."""

import argparse
import io
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import unittest
from unittest.mock import patch
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import model
import publish


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.work = model.scratch()
        self.addCleanup(shutil.rmtree, self.work)
        self.candidate = self.work / "candidate"
        self.candidate.mkdir()
        self.receipts = self.work / "receipts"
        self.receipts.mkdir()
        self.source = {
            "commit": "a" * 40,
            "epoch": 1700000000,
            "version": "0.18.0",
            "inputs": {"flake.lock": "b" * 64},
        }
        for group in model.GROUPS:
            for name in model.expected(group, "0.18.0"):
                (self.candidate / name).write_bytes(f"payload:{name}".encode())
        self.groups = {}
        for group in model.GROUPS:
            record = {
                "schema": 1,
                "group": group,
                "source": self.source,
                "files": model.inventory(
                    self.candidate, model.expected(group, "0.18.0")
                ),
                "imports": model.inventory(self.candidate, model.dependencies(group)),
            }
            self.groups[group] = record
            model.write_json(self.candidate / f"group-{group}.json", record)
        self.manifest = {"schema": 1, "source": self.source, "groups": self.groups}
        model.write_json(self.candidate / "candidate.json", self.manifest)
        self.checksum = model.sha256(self.candidate / "candidate.json")
        for group, record in self.groups.items():
            host = {"os": "Linux", "arch": "x86_64"}
            if group == "darwin":
                host = {"os": "Darwin", "arch": "aarch64"}
            elif group == "linux-aarch64":
                host["arch"] = "aarch64"
            model.write_json(
                self.receipts / f"receipt-{group}.json",
                {
                    **record,
                    "candidate_sha256": self.checksum,
                    "host": host,
                },
            )

    def collect(self):
        ledger = self.work / "collected.json"
        model.collect(self.candidate, self.checksum, self.receipts, ledger)
        return ledger

    def test_complete_candidate_and_coverage_accepted(self):
        ledger = self.collect()
        model.check_collection(self.candidate, self.checksum, ledger)

    def test_changed_artifact_rejected_after_collection(self):
        ledger = self.collect()
        (self.candidate / "go.tar.gz").write_bytes(b"substituted binary")
        with self.assertRaisesRegex(model.ReleaseError, "hashes or sizes"):
            model.check_collection(self.candidate, self.checksum, ledger)

    def test_missing_native_receipt_not_covered_by_package_assembly(self):
        (self.receipts / "receipt-darwin.json").unlink()
        with self.assertRaisesRegex(
            model.ReleaseError, "Missing independent comparisons: darwin"
        ):
            self.collect()

    def test_wrong_import_hash_rejected_even_when_output_matches(self):
        path = self.receipts / "receipt-packages.json"
        receipt = model.read_json(path)
        first = next(iter(receipt["imports"]))
        receipt["imports"][first]["sha256"] = "0" * 64
        model.write_json(path, receipt)
        with self.assertRaisesRegex(model.ReleaseError, "import mismatch"):
            self.collect()

    def test_receipts_from_other_candidate_rejected(self):
        path = self.receipts / "receipt-android.json"
        receipt = model.read_json(path)
        receipt["candidate_sha256"] = "0" * 64
        model.write_json(path, receipt)
        with self.assertRaisesRegex(model.ReleaseError, "different candidate"):
            self.collect()

    def test_darwin_receipt_cannot_claim_linux_host(self):
        path = self.receipts / "receipt-darwin.json"
        receipt = model.read_json(path)
        receipt["host"] = {"os": "Linux", "arch": "x86_64"}
        model.write_json(path, receipt)
        with self.assertRaisesRegex(model.ReleaseError, "host cannot verify"):
            self.collect()

    def test_manifest_cannot_omit_a_target(self):
        record = self.groups["android"]
        record["files"].pop("cdk-android-0.18.0.aar")
        model.write_json(self.candidate / "group-android.json", record)
        model.write_json(self.candidate / "candidate.json", self.manifest)
        checksum = model.sha256(self.candidate / "candidate.json")
        with self.assertRaisesRegex(model.ReleaseError, "Incomplete output inventory"):
            model.check_candidate(self.candidate, checksum)

    def test_manifest_checksum_checked_before_json_parsing(self):
        (self.candidate / "candidate.json").write_text("not json")
        with self.assertRaisesRegex(model.ReleaseError, "checksum mismatch"):
            model.check_candidate(self.candidate, self.checksum)

    def test_symlink_cannot_replace_candidate_file(self):
        path = self.candidate / "generated.tar"
        path.unlink()
        path.symlink_to(self.candidate / "go.tar.gz")
        with self.assertRaisesRegex(model.ReleaseError, "regular artifact"):
            model.check_candidate(self.candidate, self.checksum)

    def test_collection_flag_does_not_bypass_receipt_validation(self):
        ledger = self.collect()
        value = model.read_json(ledger)
        value["receipts"].pop("windows")
        value["verified"] = True
        model.write_json(ledger, value)
        with self.assertRaisesRegex(model.ReleaseError, "windows"):
            model.check_collection(self.candidate, self.checksum, ledger)

    def test_archives_ignore_creation_order_timestamps_and_umask(self):
        trees = []
        for index, order in enumerate((("b", "a"), ("a", "b"))):
            tree = self.work / f"tree-{index}"
            tree.mkdir()
            for name in order:
                path = tree / name
                path.write_text(f"content:{name}")
                path.chmod(0o600 if index else 0o644)
                os.utime(path, (100 + index, 100 + index))
            (tree / "link").symlink_to("a")
            trees.append(tree)
        for extension in ("tar", "tar.gz", "zip"):
            outputs = []
            for index, tree in enumerate(trees):
                path = self.work / f"archive-{index}.{extension}"
                model.archive(tree, path, 1700000000)
                outputs.append(path.read_bytes())
            self.assertEqual(*outputs)
            unpacked = self.work / f"unpacked-{extension}"
            model.extract(self.work / f"archive-0.{extension}", unpacked)
            self.assertEqual((unpacked / "link").read_text(), "content:a")

    def test_tar_cannot_write_outside_destination(self):
        path = self.work / "unsafe.tar"
        with tarfile.open(path, "w") as packed:
            info = tarfile.TarInfo("../outside")
            info.size = 4
            packed.addfile(info, io.BytesIO(b"oops"))
        with self.assertRaises(tarfile.FilterError):
            model.extract(path, self.work / "unpack")
        self.assertFalse((self.work / "outside").exists())

    def test_zip_cannot_write_through_escaping_link(self):
        path = self.work / "unsafe.zip"
        with zipfile.ZipFile(path, "w") as packed:
            info = zipfile.ZipInfo("link")
            info.external_attr = 0o120777 << 16
            packed.writestr(info, "../outside")
            packed.writestr("link/file", "oops")
        with self.assertRaisesRegex(model.ReleaseError, "Unsafe archive link"):
            model.extract(path, self.work / "unpack")

    def test_duplicate_json_keys_are_rejected(self):
        path = self.work / "duplicate.json"
        path.write_text('{"files": {}, "files": {"extra": true}}')
        with self.assertRaisesRegex(model.ReleaseError, "Duplicate JSON"):
            model.read_json(path)

    def test_publish_rejects_ci_before_any_external_action(self):
        with (
            patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}),
            patch.object(publish, "github") as action,
        ):
            with self.assertRaisesRegex(model.ReleaseError, "local only"):
                publish.publish(argparse.Namespace())
            action.assert_not_called()

    def test_maven_unknown_upload_is_not_retried(self):
        args = argparse.Namespace(
            root=self.work,
            candidate=self.candidate,
            sha256=self.checksum,
            output=self.work / "publish",
            deployment_id=None,
        )
        args.output.mkdir()
        state = {"upload_started": True}
        with (
            patch.dict(
                os.environ,
                {
                    "SONATYPE_USERNAME": "test",
                    "SONATYPE_PASSWORD": "test",
                    "CDK_SIGNING_KEY": "test",
                },
            ),
            patch.object(publish, "portal") as portal,
        ):
            with self.assertRaisesRegex(model.ReleaseError, "outcome is unknown"):
                publish.maven(
                    args, self.manifest, state, args.output / "publication.json"
                )
            portal.assert_not_called()

    def test_maven_resume_never_rebuilds_or_reuploads(self):
        args = argparse.Namespace(
            root=self.work,
            candidate=self.candidate,
            sha256=self.checksum,
            output=self.work / "publish",
            deployment_id=None,
        )
        args.output.mkdir()
        state = {"deployment": "12345678-abcd-abcd-abcd-123456789012"}
        response = {
            "deploymentName": f"cdk-0.18.0-{self.checksum}",
            "deploymentState": "PUBLISHED",
        }
        with (
            patch.dict(
                os.environ,
                {
                    "SONATYPE_USERNAME": "test",
                    "SONATYPE_PASSWORD": "test",
                    "CDK_SIGNING_KEY": "test",
                },
            ),
            patch.object(
                publish, "portal", return_value=json.dumps(response).encode()
            ) as portal,
            patch.object(publish, "run") as command,
        ):
            publish.maven(args, self.manifest, state, args.output / "publication.json")
            command.assert_not_called()
            self.assertEqual(portal.call_count, 1)
            self.assertEqual(state["status"], "published")


if __name__ == "__main__":
    unittest.main()
