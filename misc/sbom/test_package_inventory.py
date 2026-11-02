"""Packaging and independent-resolution checks must not accept partial inventories."""

import io
import gzip
import json
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch
import zipfile

from package_inventory import archive_natives, binary_identity, file_bom, go_bom, native_files
from rust_inventory import resolve_consumer
from assemble_go import assemble
import sbom
from test_sbom import bom


def elf(machine=183):
    return b"\x7fELF\x02\x01" + bytes(12) + machine.to_bytes(2, "little") + bytes(44)


class PackageInventoryTests(unittest.TestCase):
    def test_native_inside_nested_aar_is_inventoried_with_container_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "package.aar"
            nested = io.BytesIO()
            with zipfile.ZipFile(nested, "w") as output:
                output.writestr("native/arm64/libdispatch.so", elf())
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("libs/dispatch.jar", nested.getvalue())
                output.writestr("jni/x86_64/libwallet.so", elf(62))
            records = archive_natives(archive, "pkg:maven/org.example/dispatch@1", "container-hash")
            self.assertEqual(len(records), 2)
            self.assertEqual(records[0]["path"], "libs/dispatch.jar!native/arm64/libdispatch.so")
            self.assertEqual(records[0]["container_sha256"], "container-hash")
            self.assertEqual(records[0]["architecture"], "aarch64")
            self.assertEqual(records[1]["architecture"], "x86_64")
            sbom.check_graph(file_bom("fixture", records))
            self.assertEqual(list(native_files(Path(directory), include_archives=True)), [archive])

    def test_unsafe_and_unknown_native_archive_entries_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "package.aar"
            for name, data in (("../lib.so", elf()), ("jni/arm64/lib.so", b"not a native binary")):
                with self.subTest(name=name):
                    with zipfile.ZipFile(archive, "w") as output:
                        output.writestr(name, data)
                    with self.assertRaises(ValueError):
                        archive_natives(archive, "owner", "hash")

    def test_go_inventory_retains_selected_only_modules_and_identifies_stdlib(self):
        document = go_bom([
            {"Main": True, "Path": "example.org/wallet"},
            {"Path": "example.org/direct", "Version": "v1.0.0", "Sum": "h1:example"},
            {"Path": "example.org/build-only", "Version": "v2.0.0"},
        ], [{"Module": {"Path": "example.org/direct"}}], "go1.26.0")
        sbom.check_graph(document)
        items = {item["name"]: item for item in document["components"]}
        self.assertEqual(items["stdlib"]["version"], "go1.26.0")
        self.assertEqual(items["example.org/build-only"]["scope"], "required")
        self.assertIn({"name": "cdk:go:module-role", "value": "selected-only"},
                      items["example.org/build-only"]["properties"])
        with self.assertRaises(ValueError):
            go_bom([], [], "go1.26.0")

    def test_independent_gradle_graph_detects_missing_transitive_package(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            collector = sbom.Collector(sbom.parser().parse_args(["kotlin"]), root)
            collector.output = root / "output"
            graph = {"graphs": [{"role": "product", "artifacts": [], "modules": [
                {"group": "org.example", "name": "dep", "version": "2", "dependencies": ["org.example:transitive:3"]},
                {"group": "org.example", "name": "transitive", "version": "3", "dependencies": []},
            ]}]}
            sbom.write_json(root / "cdk-android/build/reports/sbom/resolved.json", graph)
            document = bom()
            document["components"][0]["group"] = "org.example"
            with patch.object(collector, "document", return_value=document):
                with self.assertRaisesRegex(sbom.SbomError, "transitive@3"):
                    collector.kotlin_documents(root)

    def test_isolated_consumer_uses_requested_features_and_rejects_lock_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            workspace = root / "staging"
            workspace.mkdir()
            lock = 'version=4\n[[package]]\nname="cdk"\nversion="1.0.0"\n'
            (workspace / "Cargo.lock").write_text(lock)
            args = sbom.parser().parse_args(["cdk", "--target", "x86_64-unknown-linux-gnu",
                                            "--no-default-features", "--features", "wallet,nostr"])
            args.source_root = root
            package = {"name": "cdk", "manifest_path": str(root / "crates/cdk/Cargo.toml")}
            def runner(command, cwd):
                manifest = tomllib.loads((workspace / ".sbom-consumer/Cargo.toml").read_text())
                self.assertFalse(manifest["dependencies"]["product"]["default-features"])
                self.assertEqual(manifest["dependencies"]["product"]["features"], ["wallet", "nostr"])
                return json.dumps({"packages": [
                    {"name": "cdk-sbom-consumer", "id": "consumer"},
                    {"name": "cdk", "id": "product", "manifest_path": str(workspace / "crates/cdk/Cargo.toml")},
                ], "resolve": {"root": "consumer", "nodes": [{"id": "consumer"}, {"id": "product"}]}})
            metadata, consumer_lock = resolve_consumer(workspace, package, args, runner)
            self.assertEqual(metadata["workspace_members"], ["product"])
            self.assertEqual([item["id"] for item in metadata["packages"]], ["product"])
            self.assertEqual((workspace / "Cargo.lock").read_text(), lock)
            # A resolver may add the synthetic root, never update an actual dependency.
            other = root / "second"
            other.mkdir()
            (other / "Cargo.lock").write_text(lock)
            def changed(command, cwd):
                (other / ".sbom-consumer/Cargo.lock").write_text(lock.replace('version="1.0.0"', 'version="2.0.0"'))
                return "{}"
            with self.assertRaisesRegex(ValueError, "changed locked dependency"):
                resolve_consumer(other, package, args, changed)

    def test_invalid_elf_header_is_not_accepted_as_native_provenance(self):
        with self.assertRaises(ValueError):
            binary_identity(b"\x7fELFbad")

    def test_compressed_native_files_and_extensionless_binaries_are_inventoried(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "native.jar"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("linux/arm64/libtor.so.gz", gzip.compress(elf()))
                output.writestr("bin/wallet", elf(62))
                output.writestr("classes/Example.class", b"\xca\xfe\xba\xbe\x00\x00\x00\x3d")
            records = archive_natives(archive, "owner", "hash")
            self.assertEqual(len(records), 2)
            self.assertEqual(records[0]["encoding"], "gzip")
            self.assertEqual(records[0]["architecture"], "aarch64")
            self.assertNotEqual(records[0]["sha256"], records[0]["uncompressed_sha256"])
            self.assertEqual(records[1]["path"], "bin/wallet")
            (root / "wallet").write_bytes(elf(62))
            self.assertEqual(list(native_files(root)), [root / "wallet"])

    def test_packaged_library_must_match_the_verified_build_bytes(self):
        collector = sbom.Collector(sbom.parser().parse_args(["go", "--build"]))
        collector.index["artifacts"] = [{"sha256": "verified"}]
        collector.packaged_records = [{"sha256": "unverified", "path": "native/linux_amd64/libcdk_ffi_go.so"}]
        with self.assertRaisesRegex(sbom.SbomError, "does not match a verified build artifact"):
            collector.verify_packaged_artifacts()
        collector.packaged_records[0]["sha256"] = "verified"
        collector.verify_packaged_artifacts()
        self.assertTrue(collector.index["coverage"]["packaged_native_build_hashes_checked"])

    def test_gradle_scp_url_is_preserved_and_corrected_not_dropped(self):
        document = bom()
        component = document["metadata"]["component"]
        component["externalReferences"] = [{"type": "vcs", "url": "ssh://git@github.com:cashubtc/cdk.git"}]
        sbom.normalize_vcs_urls(document)
        self.assertEqual(component["externalReferences"][0]["url"], "ssh://git@github.com/cashubtc/cdk.git")
        self.assertIn({"name": "cdk:original-vcs-url", "value": "ssh://git@github.com:cashubtc/cdk.git"}, component["properties"])

    def test_duplicate_evidence_path_cannot_invalidate_an_earlier_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            collector = sbom.Collector(sbom.parser().parse_args(["cdk"]))
            collector.output = Path(directory)
            collector.generated_evidence("proof.json", {"original": True})
            with self.assertRaisesRegex(sbom.SbomError, "Duplicate evidence path"):
                collector.generated_evidence("proof.json", {"replacement": True})
            self.assertEqual(sbom.read_json(collector.output / "evidence/proof.json"), {"original": True})

    def test_compilation_inventory_keeps_source_only_packages_separate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            collector = sbom.Collector(sbom.parser().parse_args(["cdk", "--target", "x86_64-unknown-linux-gnu"]), root)
            collector.output = root / "output"
            collector.staged_workspace = root / "staged"
            sbom.write_json(collector.output / "rust/cdk.cdx.json", bom())
            product = {"id": "root", "name": "cdk", "version": "1",
                       "manifest_path": str(root / "staged/crates/cdk/Cargo.toml")}
            dependency = {"id": "dependency", "name": "dep", "version": "2", "source": "registry+example",
                          "manifest_path": "/cargo/dep/Cargo.toml"}
            optional = {"id": "source-only", "name": "optional", "version": "3", "source": "registry+example",
                        "manifest_path": "/cargo/optional/Cargo.toml"}
            records = [
                {"reason": "compiler-artifact", "package_id": "actual-root",
                 "manifest_path": str(root / "crates/cdk/Cargo.toml"), "target": {"name": "cdk", "kind": ["lib"]},
                 "features": ["wallet"], "filenames": [str(root / "target/x86_64-unknown-linux-gnu/debug/libcdk.rlib")]},
                {"reason": "compiler-artifact", "package_id": "dependency",
                 "target": {"name": "dep", "kind": ["lib"]}, "features": [],
                 "filenames": [str(root / "target/x86_64-unknown-linux-gnu/debug/deps/libdep.rlib")]},
            ]
            metadata = {"packages": [product, dependency, optional]}
            with patch.object(collector, "document"):
                runtime, host = collector.compilation_inventory(product, metadata, records, "debug")
            self.assertEqual({item["id"] for item in runtime}, {"actual-root", "dependency"})
            self.assertEqual(host, [])
            source_only = sbom.read_json(collector.output / "evidence/resolved-not-compiled.json")
            self.assertEqual(source_only[0]["name"], "optional")
            self.assertEqual(sbom.read_json(collector.output / "rust/cdk.cdx.json"), bom())
            records.append(dict(records[1], package_id="unresolved-package"))
            with self.assertRaisesRegex(sbom.SbomError, "absent from the resolved inventory"):
                collector.compilation_inventory(product, metadata, records, "debug")

    def test_go_assembly_uses_real_inputs_and_does_not_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bindings = root / "generated"
            bindings.mkdir()
            (bindings / "cdk_ffi.go").write_text("package cdk_ffi\n")
            (bindings / "cdk_ffi.h").write_text("// generated header\n")
            library = root / "libcdk_ffi_go.so"
            library.write_bytes(elf(62))
            output = root / "assembled"
            assemble(bindings, {"x86_64-unknown-linux-gnu": library}, output, "github.com/cashubtc/cdk-go")
            destination = output / "bindings/cdkffi/native/linux_amd64/libcdk_ffi_go.so"
            self.assertEqual(destination.read_bytes(), library.read_bytes())
            self.assertIn("-lcdk_ffi_go", (output / "bindings/cdkffi/link_linux_amd64.go").read_text())
            self.assertEqual((output / "go.mod").read_text(), "module github.com/cashubtc/cdk-go\n\ngo 1.22\n")
            with self.assertRaises(FileExistsError):
                assemble(bindings, {"x86_64-unknown-linux-gnu": library}, output, "github.com/cashubtc/cdk-go")


if __name__ == "__main__":
    unittest.main()
