"""Release bundles preserve evidence bytes, assurance labels and safe paths."""

import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from package_evidence import package
from provenance import digest
from test_release_evidence import complete_run
import sbom


class EvidencePackageTests(unittest.TestCase):
    def completed(self, root):
        index = complete_run(root)
        index["assurance"] = {"dimensions": {"final_bytes": {"status": "verified"}}}
        sbom.write_json(root / "index.json", index)
        return index

    def test_versioned_archive_preserves_layout_and_indexed_hashes(self):
        for version, dependencies in (("1.2.3", []), ("2.0.0-rc.1+build.7", [{"name": "new-dependency", "version": "9"}])):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "evidence"
                index = self.completed(source)
                sbom.write_json(source / "proof.json", {"version": version, "dependencies": dependencies})
                index["evidence"][0]["sha256"] = digest(source / "proof.json")
                sbom.write_json(source / "index.json", index)
                name = f"example-wallet-v{version}-linux-sbom"
                archive, checksum = package(source, root / "packages", name)
                self.assertEqual(archive.name, name + ".tar.gz")
                self.assertEqual(checksum.read_text(), f"{digest(archive)}  {archive.name}\n")
                with tarfile.open(archive) as bundle:
                    self.assertEqual(bundle.extractfile(name + "/index.json").read(), (source / "index.json").read_bytes())
                    for record in index["documents"] + index["evidence"]:
                        data = bundle.extractfile(name + "/" + record["path"]).read()
                        self.assertEqual(hashlib.sha256(data).hexdigest(), record["sha256"])
                self.assertEqual(json.loads((source / "index.json").read_text()), index)

    def test_diagnostics_retain_failed_reports_and_exclude_only_the_compiler_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "diagnostics"
            (source / "run/cargo-target").mkdir(parents=True)
            (source / "run/cargo-target/huge-binary").write_bytes(b"not release evidence")
            report = source / "run/report.json"
            report.write_text('{"status":"failed","release_ready":false}')
            (source / "bootstrap.log").write_text("stripping failed")
            name = "cdk-ffi-swift-v0.18.0-aarch64-apple-darwin-release-ffi-sbom-diagnostics"
            archive, _ = package(source, root / "packages", name, "diagnostics", ["run/cargo-target"])
            with tarfile.open(archive) as bundle:
                self.assertFalse(any("cargo-target" in member.name for member in bundle))
                self.assertEqual(bundle.extractfile(name + "/run/report.json").read(), report.read_bytes())
                self.assertIn("failed", bundle.extractfile(name + "/bootstrap.log").read().decode())
            with self.assertRaises(ValueError):
                package(source, root / "mislabelled", "cdk-ffi-swift-v0.18.0-macos-sbom")

    def test_incomplete_modified_or_unverified_runs_cannot_be_release_packages(self):
        for failure in ("incomplete", "modified", "unverified"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "evidence"
                index = self.completed(source)
                if failure == "incomplete":
                    index["status"] = "incomplete"
                elif failure == "modified":
                    (source / "proof.json").write_text("changed")
                else:
                    index["assurance"]["dimensions"]["final_bytes"]["status"] = "not_verified"
                sbom.write_json(source / "index.json", index)
                with self.assertRaises(ValueError):
                    package(source, root / "packages", "cdk-ffi-go-v0.18.0-linux-sbom")
                self.assertFalse((root / "packages").exists())

    def test_unsafe_names_symlinks_exclusions_and_existing_outputs_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "evidence"
            self.completed(source)
            name = "cdk-ffi-go-v0.18.0-linux-sbom"
            for unsafe in ("../escape-sbom", "bad/name-sbom", "bad\nname-sbom", "no-assurance-label", "-hidden-sbom"):
                with self.subTest(name=unsafe), self.assertRaises(ValueError):
                    package(source, root / "packages", unsafe)
            with self.assertRaisesRegex(ValueError, "cannot omit"):
                package(source, root / "packages", name, exclude=["proof.json"])
            with self.assertRaisesRegex(ValueError, "outside"):
                package(source, source / "packages", name)
            for unsafe in ("/absolute", "../parent", "."):
                with self.subTest(exclude=unsafe), self.assertRaisesRegex(ValueError, "safe relative"):
                    package(source, root / "packages", name + "-diagnostics", "diagnostics", [unsafe])
            link = source / "outside-link"
            link.symlink_to(root / "outside")
            with self.assertRaisesRegex(ValueError, "Unsafe"):
                package(source, root / "packages", name)
            link.unlink()
            output = root / "packages"
            output.mkdir()
            sentinel = output / "user-file"
            sentinel.write_text("user evidence")
            with self.assertRaises(FileExistsError):
                package(source, output, name)
            self.assertEqual(sentinel.read_text(), "user evidence")

    def test_changing_or_adding_evidence_during_packaging_cannot_produce_a_checksum(self):
        for change in ("modify", "add"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = root / "evidence"
                self.completed(source)
                original_add = tarfile.TarFile.add

                def changed(bundle, *args, **kwargs):
                    original_add(bundle, *args, **kwargs)
                    path = source / ("proof.json" if change == "modify" else "new-evidence.json")
                    path.write_text("evidence changed while writing the archive")

                with patch.object(tarfile.TarFile, "add", changed), self.assertRaisesRegex(ValueError, "changed during"):
                    package(source, root / "packages", "cdk-ffi-go-v0.18.0-linux-sbom")
                self.assertFalse(list((root / "packages").glob("*.sha256")))


if __name__ == "__main__":
    unittest.main()
