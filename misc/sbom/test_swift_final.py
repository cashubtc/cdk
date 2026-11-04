"""The Android finalizer also requires exact, correctly labelled Apple archive slices."""

import io
import json
from pathlib import Path
import plistlib
import shutil
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from apple_diagnostics import parser
from consumer import swift_consumer
from finalize import finalize
from package_inventory import native_slices
from provenance import digest, read_run
import sbom
from test_release_evidence import complete_run


GROUPS = [
    ("ios-arm64", "ios", "", {"arm64": "aarch64-apple-ios"}),
    ("ios-arm64_x86_64-simulator", "ios", "simulator", {"arm64": "aarch64-apple-ios-sim", "x86_64": "x86_64-apple-ios"}),
    ("macos-arm64_x86_64", "macos", "", {"arm64": "aarch64-apple-darwin", "x86_64": "x86_64-apple-darwin"}),
]


def fat(archives):
    table, payload = b"", b""
    offset = 8 + 20 * len(archives)
    for arch, content in archives.items():
        padding = (-offset) % 8
        payload += bytes(padding)
        offset += padding
        table += struct.pack(">5I", {"arm64": 0x100000c, "x86_64": 0x1000007}[arch], 0 if arch == "arm64" else 3, offset, len(content), 3)
        payload += content
        offset += len(content)
    return struct.pack(">2I", 0xcafebabe, len(archives)) + table + payload


def fixture(root, payload=b"fixture"):
    originals, builds, contents = [], [], {}
    for _, _, _, targets in GROUPS:
        for target in targets.values():
            original = root / (target + ".a")
            original.write_bytes(b"!<arch>\n" + payload + b":" + target.encode())
            contents[target] = original.read_bytes()
            build = root / target
            index = complete_run(build)
            native = {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
                      "metadata": {"component": {"type": "file", "name": original.name, "bom-ref": "native"},
                                   "properties": [{"name": "cdk:artifact:sha256", "value": digest(original)},
                                                  {"name": "cdk:target", "value": target}]},
                      "components": [], "dependencies": [{"ref": "native", "dependsOn": []}]}
            sbom.write_json(build / "native.cdx.json", native)
            index["documents"] = [{"path": "native.cdx.json", "kind": "native-build-evidence", "sha256": digest(build / "native.cdx.json")}]
            index["artifacts"] = [{"name": original.name, "sha256": digest(original), "target": target,
                                   "native_documents": ["native.cdx.json"]}]
            sbom.write_json(build / "index.json", index)
            originals.append(original)
            builds.append(build)
    package = root / "CashuDevKitFFI.xcframework.zip"
    entries, libraries = {}, []
    for identifier, platform, variant, targets in GROUPS:
        framework = f"CashuDevKitFFI.xcframework/{identifier}/CashuDevKitFFI.framework"
        binaries = {arch: contents[target] for arch, target in targets.items()}
        entries[framework + "/CashuDevKitFFI"] = fat(binaries) if len(binaries) > 1 else next(iter(binaries.values()))
        entries[framework + "/Info.plist"] = plistlib.dumps({"CFBundleExecutable": "CashuDevKitFFI", "CFBundlePackageType": "FMWK",
            "CFBundleSupportedPlatforms": [{("ios", ""): "iPhoneOS", ("ios", "simulator"): "iPhoneSimulator", ("macos", ""): "MacOSX"}[(platform, variant)]]})
        entries[framework + "/Headers/Cdk.h"] = b"// generated header"
        entries[framework + "/Modules/module.modulemap"] = b"framework module CashuDevKitFFI {}"
        libraries.append({"LibraryIdentifier": identifier, "LibraryPath": "CashuDevKitFFI.framework",
                          "SupportedPlatform": platform, "SupportedArchitectures": list(targets),
                          **({"SupportedPlatformVariant": variant} if variant else {})})
    entries["CashuDevKitFFI.xcframework/Info.plist"] = plistlib.dumps({"AvailableLibraries": libraries, "XCFrameworkFormatVersion": "1.0"})
    return originals, builds, package, entries


def write_zip(path, entries):
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in entries.items():
            archive.writestr(name, content)


def convert(arguments, cwd, env=None):
    if arguments[:2] == ["cyclonedx", "convert"]:
        shutil.copyfile(arguments[arguments.index("--input-file") + 1], arguments[arguments.index("--output-file") + 1])
    elif arguments[:2] == ["git", "rev-parse"]:
        return "fixture-revision"
    elif arguments[:2] not in (["git", "status"], ["cyclonedx", "validate"]):
        raise AssertionError(f"Unexpected fixture command: {arguments}")
    return ""


class SwiftFinalTests(unittest.TestCase):
    def test_all_five_slices_use_the_existing_finalizer_and_keep_exact_hashes(self):
        for payload in (b"original library code", b"changed library code\x00with new symbols"):
            with self.subTest(payload=payload), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                originals, builds, package, entries = fixture(root, payload)
                write_zip(package, entries)
                args = SimpleNamespace(output=root / "final", build_run=builds, original=originals, inventory_run=[], trust_policy=None,
                                       distribution=[package], strip_tool=None, go_project=None, require_assurance=["final_bytes"])
                with patch.object(sbom, "run", side_effect=convert), patch("finalize.run", side_effect=convert):
                    finalize(args)
                _, index = read_run(args.output)
                self.assertEqual({item["target"] for item in index["artifacts"]}, {target for *_, targets in GROUPS for target in targets.values()})
                self.assertTrue(all(item["verification"] == "exact-build-bytes" for item in index["artifacts"]))
                self.assertEqual({item["sha256"] for item in index["artifacts"]}, {digest(path) for path in originals})

    def test_changed_swapped_missing_or_extra_native_files_fail_closed(self):
        for failure in ("changed", "swapped", "missing", "extra", "traversal", "metadata"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                originals, builds, package, entries = fixture(root)
                ios = "CashuDevKitFFI.xcframework/ios-arm64/CashuDevKitFFI.framework/CashuDevKitFFI"
                if failure == "changed":
                    entries[ios] += b"modified"
                elif failure == "swapped":
                    entries[ios] = (root / "aarch64-apple-ios-sim.a").read_bytes()
                elif failure == "missing":
                    del entries[ios]
                elif failure == "metadata":
                    del entries["CashuDevKitFFI.xcframework/Info.plist"]
                else:
                    entries["../injected.a" if failure == "traversal" else "extra.a"] = b"!<arch>\nextra"
                write_zip(package, entries)
                args = SimpleNamespace(output=root / "final", build_run=builds, original=originals, inventory_run=[], trust_policy=None,
                                       distribution=[package], strip_tool=None, go_project=None, require_assurance=["final_bytes"])
                with patch.object(sbom, "run", side_effect=convert), patch("finalize.run", side_effect=convert), \
                        self.assertRaises((ValueError, KeyError)):
                    finalize(args)
                self.assertEqual(json.loads((args.output / "index.json").read_text())["status"], "incomplete")

    def test_universal_slice_table_cannot_hide_overlaps_or_duplicate_architectures(self):
        original = fat({"arm64": b"!<arch>\narm", "x86_64": b"!<arch>\nintel"})
        for offset, value in ((28, 0x100000c), (32, 8), (36, 48), (16, 0), (20, 0xffffffff)):
            content = bytearray(original)
            struct.pack_into(">I", content, offset, value)
            with self.subTest(offset=offset), self.assertRaises(ValueError):
                native_slices(io.BytesIO(content), "framework", "owner", "hash")

    def test_swift_consumer_is_offline_native_host_only_and_clears_loader_overrides(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            project = root / "package"
            project.mkdir()
            (project / "Package.swift").write_text("// actual package")
            (project / "CashuDevKitFFI.xcframework.zip").write_bytes(b"the verified ZIP")

            def command(arguments, cwd, env):
                self.assertFalse(any(key.startswith(("DYLD_", "LD_")) for key in env))
                if "show-dependencies" in arguments:
                    return '{"dependencies":[]}'
                if "--show-bin-path" in arguments:
                    binary = Path(arguments[arguments.index("--package-path") + 1]) / ".build/release/Smoke"
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b"native executable")
                    return str(binary.parent)
                return "Swift fixture"

            with patch("consumer.sys.platform", "darwin"), patch.object(sbom, "run", side_effect=command), \
                    patch("consumer.subprocess.run") as execute, patch.dict("os.environ", {"DYLD_LIBRARY_PATH": "/wrong"}):
                result = swift_consumer(project, root)
                execute.assert_called_once()
                self.assertEqual(result["archive_sha256"], digest(project / "CashuDevKitFFI.xcframework.zip"))
                self.assertIn("no iOS execution", result["scope"])
            for graph in ('{"dependencies":[{"name":"unaccounted"}]}', '{}'):
                with patch("consumer.sys.platform", "darwin"), patch.object(sbom, "run", return_value=graph), \
                        self.assertRaisesRegex(ValueError, "resolved Swift inventory"):
                    swift_consumer(project, root)
            with patch("consumer.sys.platform", "linux"), self.assertRaisesRegex(ValueError, "real macOS"):
                swift_consumer(project, root)

    def test_isolated_command_parser_preserves_tool_arguments(self):
        args = parser().parse_args(["exec", "--target", "aarch64-apple-darwin", "python3", "finalize.py", "--distribution", "release"])
        self.assertEqual(args.arguments, ["python3", "finalize.py", "--distribution", "release"])


if __name__ == "__main__":
    unittest.main()
