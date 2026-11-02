"""Offline tests for collection integrity and failure behavior."""

import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import sbom


def bom():
    return {
        "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
        "metadata": {"component": {"bom-ref": "root", "type": "library", "name": "cdk", "version": "1"}},
        "components": [{"bom-ref": "dependency", "type": "library", "name": "dep", "version": "2"}],
        "dependencies": [{"ref": "root", "dependsOn": ["dependency"]},
                         {"ref": "dependency", "dependsOn": []}],
    }


def rust_metadata():
    def package(identifier, name, version, kind="lib", links=None):
        return {"id": identifier, "name": name, "version": version,
                "targets": [{"kind": [kind]}], "links": links}

    def edge(identifier, kind=None):
        return {"pkg": identifier, "dep_kinds": [{"kind": kind}]}

    return {
        "workspace_members": ["root"],
        "packages": [package("root", "cdk", "1", "bin"), package("dependency", "dep", "2"),
                     package("derive", "serde_derive", "1", "proc-macro"),
                     package("helper", "syn", "2"), package("shared", "shared", "1"),
                     package("build", "build-tool", "1"),
                     package("buildleaf", "native-tool", "1", links="host-library"),
                     package("dev", "dev-only", "1")],
        "resolve": {"nodes": [
            {"id": "root", "deps": [edge("dependency"), edge("derive"), edge("build", "build"), edge("dev", "dev")]},
            {"id": "dependency", "deps": [edge("shared")]},
            {"id": "derive", "deps": [edge("helper")]},
            {"id": "helper", "deps": [edge("shared"), edge("buildleaf")]},
            {"id": "shared", "deps": []}, {"id": "build", "deps": [edge("buildleaf")]},
            {"id": "buildleaf", "deps": []}, {"id": "dev", "deps": []},
        ]},
    }


class IntegrityTests(unittest.TestCase):
    def test_graph(self):
        sbom.check_graph(bom())

    def test_dangling_reference(self):
        document = bom()
        document["dependencies"][0]["dependsOn"].append("missing")
        with self.assertRaisesRegex(sbom.SbomError, "Dangling"):
            sbom.check_graph(document)

    def test_duplicate_reference(self):
        document = bom()
        document["components"].append(copy.deepcopy(document["components"][0]))
        with self.assertRaisesRegex(sbom.SbomError, "Duplicate"):
            sbom.check_graph(document)

    def test_generator_subject_duplicate_normalized_without_losing_edges(self):
        document = bom()
        duplicate = copy.deepcopy(document["metadata"]["component"])
        duplicate["group"] = ""
        document["components"].append(duplicate)
        sbom.normalize_subject(document)
        sbom.check_graph(document)
        self.assertEqual(len(document["components"]), 1)
        document["components"].append(dict(duplicate, version="different"))
        with self.assertRaisesRegex(sbom.SbomError, "Conflicting"):
            sbom.normalize_subject(document)

    def test_pub_resolved_transitive_edges_are_preserved(self):
        document = bom()
        document["components"].append({"bom-ref": "transitive", "name": "transitive", "version": "3"})
        graph = {"packages": [
            {"name": "cdk", "version": "1", "kind": "root", "dependencies": ["dep"]},
            {"name": "dep", "version": "2", "kind": "direct", "dependencies": ["transitive"]},
            {"name": "transitive", "version": "3", "kind": "transitive", "dependencies": []},
        ]}
        sbom.dart_dependency_graph(document, graph)
        sbom.check_graph(document)
        self.assertIn({"ref": "dependency", "dependsOn": ["transitive"]}, document["dependencies"])
        graph["packages"][2]["source"] = "git"
        with self.assertRaisesRegex(sbom.SbomError, "non-hosted"):
            sbom.dart_dependency_graph(document, graph)

    def test_missing_subject(self):
        document = bom()
        del document["metadata"]
        with self.assertRaises(sbom.SbomError):
            sbom.check_graph(document)

    def test_transitive_package_required(self):
        with self.assertRaisesRegex(sbom.SbomError, "transitive@3"):
            sbom.require_components(bom(), {("dep", "2"), ("transitive", "3")}, "Dart")

    def test_scoped_go_identity_is_not_just_basename(self):
        document = bom()
        document["components"][0]["group"] = "example.org/owner"
        sbom.require_components(document, {("example.org/owner/dep", "2")}, "Go")
        with self.assertRaises(sbom.SbomError):
            sbom.require_components(document, {("example.org/other/dep", "2")}, "Go")

    def test_go_target_mapping_and_json_stream(self):
        environment = sbom.go_environment("aarch64-apple-darwin")
        self.assertEqual(environment["GOOS"], "darwin")
        self.assertEqual(environment["GOARCH"], "arm64")
        self.assertEqual(environment["CGO_ENABLED"], "1")
        self.assertEqual(list(sbom.json_stream('{"name": "a"}\n {"name": "b"}\n')),
                         [{"name": "a"}, {"name": "b"}])
        with self.assertRaisesRegex(sbom.SbomError, "No Go binding platform mapping"):
            sbom.go_environment("aarch64-linux-android")

    def test_target_subcomponents_are_not_dependency_packages(self):
        document = bom()
        document["metadata"]["component"]["components"] = [{"name": "cdk_ffi", "version": "1"}]
        self.assertEqual(len(sbom.package_components(document)), 2)
        self.assertEqual(len(list(sbom.components(document))), 3)

    def test_projects_preserve_spaces(self):
        projects = sbom.parse_projects(["dart=/some/path with spaces"])
        self.assertEqual(projects["dart"], Path("/some/path with spaces"))

    def test_invalid_or_duplicate_project(self):
        for values in (["dart"], ["unknown=/somewhere"], ["go="], ["go=/a", "go=/b"]):
            with self.subTest(values=values), self.assertRaises(sbom.SbomError):
                sbom.parse_projects(values)

    def test_feature_arguments(self):
        args = sbom.parser().parse_args(["cdk", "--target", "wasm32-unknown-unknown",
                                        "--no-default-features", "--features", "wallet,nostr"])
        self.assertEqual(sbom.cargo_options(args), ["--target", "wasm32-unknown-unknown",
                                                  "--no-default-features", "--features", "wallet,nostr"])

    def test_mixed_library_and_binary_builds_binary(self):
        package = {"name": "cdk-mintd", "targets": [
            {"kind": ["lib"], "crate_types": ["lib"]},
            {"kind": ["bin"], "crate_types": ["bin"]},
            {"kind": ["custom-build"], "crate_types": ["bin"]},
        ]}
        self.assertEqual(sbom.cargo_build_targets(package), ["--bins"])

    def test_ffi_library_preferred_over_bindgen_binary(self):
        package = {"name": "cdk-ffi", "targets": [
            {"kind": ["cdylib", "staticlib", "rlib"], "crate_types": ["cdylib", "staticlib", "rlib"]},
            {"kind": ["bin"], "crate_types": ["bin"]},
        ]}
        self.assertEqual(sbom.cargo_build_targets(package), ["--lib"])

    def test_rlib_only_builds_library(self):
        package = {"name": "cdk", "targets": [
            {"kind": ["lib"], "crate_types": ["lib"]},
            {"kind": ["example"], "crate_types": ["bin"]},
        ]}
        self.assertEqual(sbom.cargo_build_targets(package), ["--lib"])

    def test_proc_macros_and_their_dependencies_are_build_time(self):
        roles = sbom.rust_dependency_roles(rust_metadata(), {"name": "cdk"})
        self.assertEqual({identifier for identifier, role in roles.items() if role == "runtime"},
                         {"root", "dependency", "shared"})
        self.assertEqual({identifier for identifier, role in roles.items() if role == "build"},
                         {"derive", "helper", "build", "buildleaf"})
        self.assertNotIn("dev", roles)

    def test_runtime_use_is_not_lost_when_also_used_by_a_macro(self):
        metadata = rust_metadata()
        metadata["resolve"]["nodes"][0]["deps"].append({"pkg": "helper", "dep_kinds": [{"kind": None}]})
        roles = sbom.rust_dependency_roles(metadata, {"name": "cdk"})
        self.assertEqual(roles["helper"], "runtime")
        self.assertEqual(roles["buildleaf"], "runtime")
        self.assertEqual(roles["derive"], "build")

    def test_macro_role_uses_target_kind_not_package_name(self):
        metadata = rust_metadata()
        metadata["packages"][1]["name"] = "looks_like_derive"
        roles = sbom.rust_dependency_roles(metadata, {"name": "cdk"})
        self.assertEqual(roles["dependency"], "runtime")

    def test_rust_inventory_keeps_build_dependencies_with_correct_scope(self):
        metadata = rust_metadata()
        document = bom()
        for package in metadata["packages"]:
            if package["id"] not in {"root", "dependency", "dev"}:
                document["components"].append({"bom-ref": package["id"], "type": "library",
                                               "name": package["name"], "version": package["version"]})
        sbom.classify_rust_bom(document, metadata, {"name": "cdk"})
        indexed = {item["bom-ref"]: item for item in sbom.package_components(document)}
        self.assertEqual(indexed["derive"]["scope"], "excluded")
        self.assertEqual(indexed["helper"]["scope"], "excluded")
        self.assertEqual(indexed["shared"]["scope"], "required")
        self.assertIn({"name": "cdk:rust:dependency-role", "value": "build"}, indexed["derive"]["properties"])

    def test_runtime_classification_requires_resolved_graph(self):
        metadata = rust_metadata()
        metadata["resolve"] = None
        with self.assertRaisesRegex(sbom.SbomError, "resolved Cargo graph is missing"):
            sbom.rust_dependency_roles(metadata, {"name": "cdk"})

    def test_workspace_selection_includes_wrappers(self):
        packages = [{"name": name, "id": name} for name in ["cdk", *sbom.WRAPPERS.values()]]
        metadata = {"workspace_members": [item["id"] for item in packages], "packages": packages}
        selected, languages = sbom.selection("all", metadata)
        self.assertEqual(len(selected), 5)
        self.assertEqual(set(languages), set(sbom.LANGUAGES))
        selected, languages = sbom.selection("rust", metadata)
        self.assertEqual(len(selected), 5)
        self.assertEqual(languages, [])
        selected, languages = sbom.selection("cdk-ffi-go", metadata)
        self.assertEqual(selected[0]["name"], "cdk-ffi-go")
        self.assertEqual(languages, ["go"])
        with self.assertRaisesRegex(sbom.SbomError, "Unknown scope"):
            sbom.selection("typo", metadata)

    def test_staging_excludes_caches_and_does_not_modify_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source"
            (source / "target").mkdir(parents=True)
            (source / "Cargo.lock").write_text("locked")
            (source / "target" / "large").write_text("ignored")
            destination = sbom.copy_project(source, root / "staged")
            (destination / "Cargo.lock").write_text("changed")
            self.assertFalse((destination / "target").exists())
            self.assertEqual((source / "Cargo.lock").read_text(), "locked")

    def test_missing_tools_fail(self):
        collector = sbom.Collector(sbom.parser().parse_args(["cdk"]))
        with patch("sbom.shutil.which", return_value=None):
            with self.assertRaisesRegex(sbom.SbomError, "Missing tools"):
                collector.preflight([])

    def test_generated_manifests_are_required(self):
        with tempfile.TemporaryDirectory() as directory:
            collector = sbom.Collector(sbom.parser().parse_args(["go"]), Path(directory))
            with patch("sbom.shutil.which", return_value="/bin/tool"):
                with self.assertRaisesRegex(sbom.SbomError, "assembled binding package"):
                    collector.preflight(["go"])

    def test_android_sdk_is_not_silently_skipped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "bindings/kotlin"
            project.mkdir(parents=True)
            (project / "settings.gradle.kts").write_text("// fixture")
            collector = sbom.Collector(sbom.parser().parse_args(["kotlin"]), root)
            with patch("sbom.shutil.which", return_value="/bin/tool"), patch.dict("os.environ", {}, clear=True):
                with self.assertRaisesRegex(sbom.SbomError, "ANDROID_HOME"):
                    collector.preflight(["kotlin"])

    def test_command_failure_propagates(self):
        with self.assertRaisesRegex(sbom.SbomError, "exit 7"):
            sbom.run(["python3", "-c", "raise SystemExit(7)"], Path.cwd())

    def test_native_evidence_bound_to_artifact_and_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native = root / "native.json"
            document = bom()
            document["metadata"]["properties"] = [
                {"name": "cdk:artifact:sha256", "value": "abc"},
                {"name": "cdk:target", "value": "x86_64-unknown-linux-gnu"},
            ]
            document["components"][0]["properties"] = [{"name": "cdk:rust:links", "value": "sqlite3"}]
            sbom.write_json(native, document)
            args = sbom.parser().parse_args(["cdk", "--native-bom", str(native),
                                            "--target", "x86_64-unknown-linux-gnu"])
            collector = sbom.Collector(args)
            with patch.object(collector, "document", return_value=document):
                entry = {}
                collector.native_evidence({"sqlite3"}, "abc", entry)
                self.assertEqual(len(entry["native_documents"]), 1)
                with self.assertRaisesRegex(sbom.SbomError, "Native library evidence is missing"):
                    collector.native_evidence({"sqlite3"}, "different-artifact", {})
                args.target = "aarch64-linux-android"
                with self.assertRaisesRegex(sbom.SbomError, "target does not match"):
                    collector.native_evidence({"sqlite3"}, "abc", {})

    def test_failed_run_has_incomplete_index(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
                (root / name).write_text("fixture")
            package = {"name": "cdk", "id": "cdk", "manifest_path": str(root / "Cargo.toml")}
            metadata = {"workspace_members": ["cdk"], "packages": [package]}
            args = sbom.parser().parse_args(["cdk", "--target", "host", "--output", str(root / "output")])
            collector = sbom.Collector(args, root)
            def fake_run(command, cwd, env=None):
                return json.dumps(metadata) if command[:2] == ["cargo", "metadata"] else "revision"
            with patch("sbom.run", side_effect=fake_run), patch.object(collector, "preflight"), \
                    patch.object(collector, "rust", side_effect=sbom.SbomError("resolution failed")):
                with self.assertRaisesRegex(sbom.SbomError, "resolution failed"):
                    collector.collect()
            index = sbom.read_json(root / "output/index.json")
            self.assertEqual(index["status"], "incomplete")
            self.assertEqual(index["error"], "resolution failed")
            self.assertFalse(list((root / "output").glob(".staging-*")))

    def test_dart_resolver_evidence_and_transitive_coverage(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "bindings/dart"
            project.mkdir(parents=True)
            (project / "pubspec.yaml").write_text("name: cdk\n")
            staging = root / "staging"
            staging.mkdir()
            collector = sbom.Collector(sbom.parser().parse_args(["dart"]), root)
            collector.output = root / "output"
            collector.output.mkdir()
            graph = {"packages": [{"name": "cdk", "version": "1", "kind": "root"},
                                  {"name": "dep", "version": "2", "kind": "direct"},
                                  {"name": "transitive", "version": "3", "kind": "transitive"}]}
            document = bom()
            document["components"].append({"bom-ref": "transitive", "name": "transitive", "version": "3"})
            commands = []
            def fake_run(command, cwd, env=None):
                commands.append(command)
                if command[:3] == ["dart", "pub", "get"]:
                    (cwd / "pubspec.lock").write_text("resolved lock")
                if command[:3] == ["dart", "pub", "deps"]:
                    return json.dumps(graph)
                if command[0] == "cdxgen":
                    sbom.write_json(cwd / "ecosystem.cdx.json", document)
                return ""
            with patch("sbom.run", side_effect=fake_run), patch.object(collector, "document", return_value=document):
                collector.language("dart", staging)
            self.assertTrue((collector.output / "evidence/dart/pubspec.lock").exists())
            self.assertEqual(len(commands), 3)
            self.assertFalse((project / "pubspec.lock").exists())
            self.assertIn("--fail-on-error", commands[-1])

    def test_go_replacements_do_not_silently_drop_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "bindings/go"
            project.mkdir(parents=True)
            (project / "go.mod").write_text("module example.org/cdk\n")
            staging = root / "staging"
            staging.mkdir()
            collector = sbom.Collector(sbom.parser().parse_args(["go"]), root)
            module = {"Path": "example.org/module", "Version": "v1", "Replace": {"Path": "../local"}}
            with patch("sbom.run", return_value=json.dumps(module)):
                with self.assertRaisesRegex(sbom.SbomError, "replacements"):
                    collector.language("go", staging)

    def test_swift_empty_external_dependency_graph_is_valid(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "bindings/swift"
            project.mkdir(parents=True)
            (project / "Package.swift").write_text("// release package fixture\n")
            staging = root / "staging"
            staging.mkdir()
            collector = sbom.Collector(sbom.parser().parse_args(["swift"]), root)
            collector.output = root / "output"
            collector.output.mkdir()
            with patch("sbom.run", return_value=json.dumps({"dependencies": []})), \
                    patch.object(collector, "document", return_value=bom()):
                collector.language("swift", staging)
            self.assertTrue((collector.output / "evidence/swift/Package.swift").exists())

    def test_kotlin_generates_both_runtime_configurations(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "bindings/kotlin"
            project.mkdir(parents=True)
            (project / "gradlew").write_text("fixture")
            (project / "settings.gradle.kts").write_text("// fixture\n")
            staging = root / "staging"
            staging.mkdir()
            collector = sbom.Collector(sbom.parser().parse_args(["kotlin"]), root)
            collector.output = root / "output"
            module = {"group": "", "name": "dep", "version": "2", "dependencies": []}
            def fake_run(args, cwd, env=None):
                for name in ("cdk-android", "cdk-jvm"):
                    sbom.write_json(cwd / name / "build/reports/sbom/resolved.json", {"graphs": [
                        {"role": "product", "modules": [module], "artifacts": []},
                    ]})
            # Empty group is not a Maven coordinate; use a real group in the fixture.
            module["group"] = "org.example"
            document_bom = bom()
            document_bom["components"][0]["group"] = "org.example"
            with patch("sbom.run", side_effect=fake_run) as command, \
                    patch.object(collector, "document", return_value=document_bom) as document:
                collector.language("kotlin", staging)
            args = command.call_args.args[0]
            self.assertIn(":cdk-android:cdkSbomEvidence", args)
            self.assertIn(":cdk-jvm:cdkSbomEvidence", args)
            self.assertEqual(document.call_count, 11)

    def test_existing_artifact_does_not_rebuild(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "cdk.so"
            artifact.write_bytes(b"auditable artifact fixture")
            args = sbom.parser().parse_args(["cdk", "--build", "--existing", "--artifact", str(artifact),
                                            "--target", "host", "--profile", "release"])
            collector = sbom.Collector(args, root)
            collector.output = root / "output"
            sbom.write_json(collector.output / "rust/cdk.cdx.json", bom())
            (collector.output / "evidence").mkdir()
            package = {"name": "cdk", "id": "cdk", "targets": []}
            scanned = bom()
            scanned["components"].append({"name": "shared", "version": "1"})
            with patch("sbom.run") as command, patch.object(collector, "document", return_value=scanned), \
                    patch.object(collector, "native_evidence") as native:
                collector.build(package, rust_metadata())
            native.assert_not_called()
            self.assertEqual(command.call_count, 1)
            self.assertEqual(command.call_args.args[0][0], "syft")
            self.assertEqual(collector.index["artifacts"][0]["sha256"], sbom.digest(artifact))

    def test_missing_genuine_runtime_dependency_still_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "cdk.so"
            artifact.write_bytes(b"fixture")
            args = sbom.parser().parse_args(["cdk", "--build", "--existing", "--artifact", str(artifact),
                                            "--target", "host", "--profile", "release"])
            collector = sbom.Collector(args, root)
            collector.output = root / "output"
            (collector.output / "evidence").mkdir(parents=True)
            scanned = bom()
            scanned["components"] = []
            with patch("sbom.run"), patch.object(collector, "document", return_value=scanned):
                with self.assertRaisesRegex(sbom.SbomError, "dep@2"):
                    collector.build({"name": "cdk", "id": "root", "targets": []}, rust_metadata())

    def test_existing_output_not_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / "index.json"
            marker.write_text("user data")
            args = sbom.parser().parse_args(["cdk", "--target", "host", "--output", str(root)])
            collector = sbom.Collector(args, root)
            metadata = {"workspace_members": ["cdk"], "packages": [{"name": "cdk", "id": "cdk"}]}
            with patch("sbom.run", return_value=json.dumps(metadata)), patch.object(collector, "preflight"):
                with self.assertRaises(FileExistsError):
                    collector.collect()
            self.assertEqual(marker.read_text(), "user data")


if __name__ == "__main__":
    unittest.main()
