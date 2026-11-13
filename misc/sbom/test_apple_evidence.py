"""Apple byte/parser fixtures on Linux do not establish macOS tool execution."""

import json
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

from apple_evidence import APPLE_TARGETS, macho_loads, platform_path, stub_identity
from native_evidence import NativeCollector, NativeError, sha256
import sbom
from test_native_evidence import registry_fixture


SYSTEM = "/usr/lib/libSystem.B.dylib"
SECURITY = "/System/Library/Frameworks/Security.framework/Versions/A/Security"


def path_command(kind, name):
    minimum = 12 if kind == 0x8000001c else 24
    encoded = name.encode() + b"\0"
    size = (minimum + len(encoded) + 7) // 8 * 8
    return (struct.pack("<3I", kind, size, minimum) + bytes(minimum - 12) + encoded).ljust(size, b"\0")


def macho(imports=(SYSTEM,), platform=1, cpu=0x100000c, rpaths=(), extra=()):
    commands = [struct.pack("<6I", 0x32, 24, platform, 13 << 16, (15 << 16) | (5 << 8), 0)]
    commands += [path_command(0xd, "/build/libself.dylib")]
    commands += [path_command(0xc, name) for name in imports]
    commands += [path_command(0x8000001c, name) for name in rpaths]
    commands += list(extra)
    data = b"".join(commands)
    return struct.pack("<8I", 0xfeedfacf, cpu, 0, 6, len(commands), len(data), 0, 0) + data


def stub(path, name, targets="arm64-macos, x86_64-macos"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f"--- !tapi-tbd\ntbd-version: 4\ntargets: [ {targets} ]\ninstall-name: '{name}'\nexports:\n  - symbols: []\n...\n")


class AppleEvidenceTests(unittest.TestCase):
    def fixture(self, root, host=False, target="aarch64-apple-darwin"):
        (root / "Cargo.lock").write_text("version=4\npackage=[]\n")
        developer = root / "Xcode.app/Contents/Developer"
        sdk = developer / "SDKs/Test.sdk"
        sdk.mkdir(parents=True)
        (sdk / "SDKSettings.json").write_text('{"Version":"15.5"}')
        stub(sdk / "usr/lib/libSystem.B.tbd", SYSTEM)
        stub(sdk / "System/Library/Frameworks/Security.framework/Security.tbd", SECURITY)
        profile = root / "target/release"
        profile.mkdir(parents=True)
        artifact = profile / "libwallet.dylib"
        artifact.write_bytes(macho(imports=(SYSTEM, SECURITY)))
        records = [{"reason": "compiler-artifact", "package_id": "wallet", "filenames": [str(artifact)],
                    "target": {"kind": ["proc-macro" if host else "cdylib"]}},
                   {"reason": "build-finished", "success": True}]
        calls = []

        def runner(arguments, cwd):
            calls.append(arguments)
            self.assertEqual(arguments[:2], ["/usr/bin/xcrun", "--sdk"])
            return {"--show-sdk-path": str(sdk), "--show-sdk-version": "15.5",
                    "--show-sdk-build-version": "24F74"}[arguments[-1]]

        collector = NativeCollector({}, [], records, target, profile, root, runner, host=host)
        return collector, artifact, developer, sdk, calls

    def test_load_commands_keep_requirements_not_dylib_self_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "image"
            for target, (_, arch, platform) in APPLE_TARGETS.items():
                path.write_bytes(macho(platform=platform, cpu=0x100000c if arch == "arm64" else 0x1000007))
                loads = macho_loads(path, target)
                self.assertEqual(loads["imports"], [{"install_name": SYSTEM, "load_command": "0xc"}])
                self.assertEqual(loads["platform"], platform)
                self.assertEqual(loads["sdk"], "15.5.0")

    def test_wrong_arch_platform_truncation_and_ambiguous_commands_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "image"
            for data in (macho(cpu=0x1000007), macho(platform=2), macho()[:-1],
                         macho(extra=[struct.pack("<6I", 0x32, 24, 1, 0, 0, 0)]),
                         macho(extra=[struct.pack("<2I", 0xc, 0)]), b"!<arch>\n"):
                path.write_bytes(data)
                with self.subTest(data=data[:32]), self.assertRaises(ValueError):
                    macho_loads(path, "aarch64-apple-darwin")

    def test_sdk_requirements_are_hashed_but_not_upstream_versions_or_runtime_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            c, artifact, developer, sdk, calls = self.fixture(Path(directory))
            with patch.dict("os.environ", {"DEVELOPER_DIR": str(developer)}, clear=True), \
                    patch("native_evidence.check_link_overrides"):
                document, evidence = c.collect(artifact, sha256(artifact))
            self.assertEqual(len(document["components"]), 2)
            sbom.check_graph(document)
            for component in document["components"]:
                self.assertNotIn("version", component)
                self.assertNotIn("hashes", component)
            self.assertEqual(evidence["shared_libraries"][0]["stub"]["sha256"], sha256(sdk / "usr/lib/libSystem.B.tbd"))
            self.assertEqual(evidence["apple_images"][str(artifact)]["minimum_os"], "13.0.0")
            self.assertTrue(all("readelf" not in call for call in calls))

    def test_host_build_tools_use_apple_inspection_too(self):
        with tempfile.TemporaryDirectory() as directory:
            c, artifact, developer, _, _ = self.fixture(Path(directory), host=True)
            with patch.dict("os.environ", {"DEVELOPER_DIR": str(developer)}, clear=True), \
                    patch("native_evidence.check_link_overrides"):
                document, evidence = c.collect_build_environment("0" * 64)
            self.assertEqual(evidence["host_tools"][0]["sha256"], sha256(artifact))
            self.assertEqual(len(evidence["shared_libraries"]), 2)
            sbom.check_graph(document)

    def test_missing_stubs_wrong_install_names_and_wrong_sdk_fail(self):
        for failure in ("missing", "identity", "platform", "sdk"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                c, artifact, developer, sdk, _ = self.fixture(Path(directory))
                path = sdk / "usr/lib/libSystem.B.tbd"
                if failure == "missing":
                    path.unlink()
                elif failure == "identity":
                    stub(path, "/usr/lib/another.dylib")
                elif failure == "platform":
                    stub(path, SYSTEM, "arm64-ios")
                else:
                    c.apple.sdk = {"path": str(sdk), "version": "26.4"}
                with patch.dict("os.environ", {"DEVELOPER_DIR": str(developer)}, clear=True), self.assertRaises(ValueError):
                    c.shared_libraries(artifact)

    def test_unknown_nonplatform_target_import_is_not_exempted(self):
        with tempfile.TemporaryDirectory() as directory:
            c, artifact, developer, _, _ = self.fixture(Path(directory))
            for name in ("@rpath/libunknown.dylib", "/opt/homebrew/lib/libunknown.dylib", "/usr/lib/../local/libunknown.dylib"):
                artifact.write_bytes(macho(imports=(name,)))
                with patch.dict("os.environ", {"DEVELOPER_DIR": str(developer)}, clear=True), \
                        self.subTest(name=name), self.assertRaisesRegex(ValueError, "provider evidence"):
                    c.shared_libraries(artifact)
            self.assertFalse(platform_path("/usr/lib//libSystem.B.dylib"))

    def test_rust_host_library_requires_manifest_and_is_not_called_upstream_llvm(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            c, artifact, developer, _, _ = self.fixture(root, host=True)
            sysroot = root / "toolchain"
            library = sysroot / "lib/libLLVM.dylib"
            library.parent.mkdir(parents=True)
            library.write_bytes(macho(imports=(SYSTEM,)))
            manifest = sysroot / "lib/rustlib/manifest-rustc-host"
            manifest.parent.mkdir()
            manifest.write_text("file:lib/libLLVM.dylib\n")
            c.apple.sysroot = sysroot
            artifact.write_bytes(macho(imports=("@rpath/libLLVM.dylib",)))
            with patch.dict("os.environ", {"DEVELOPER_DIR": str(developer),
                                            "DYLD_FALLBACK_LIBRARY_PATH": str(sysroot / "lib")}, clear=True):
                evidence = c.shared_libraries(artifact)
                self.assertEqual(evidence[0]["manifest"]["sha256"], sha256(manifest))
                self.assertNotIn("version", c.components[0])
                manifest.write_text("file:lib/different.dylib\n")
                with self.assertRaisesRegex(ValueError, "manifest"):
                    c.shared_libraries(artifact)

    def test_framework_build_flags_require_registry_sources_and_no_search_override(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            c, _, developer, _, _ = self.fixture(root)
            _, _, package, lock = registry_fixture(root)
            c.lock = lock
            c.runtime = [package]
            c.compiled[package["id"]] = {"features": []}
            c.scripts[package["id"]] = {"linked_libs": ["framework=Security"], "linked_paths": []}
            with patch.dict("os.environ", {"DEVELOPER_DIR": str(developer)}, clear=True):
                c.providers_from_build()
                self.assertEqual(c.providers[0]["classification"], "apple-sdk-link-requirements")
                c.scripts[package["id"]]["linked_paths"] = ["framework=/unknown"]
                with self.assertRaisesRegex(NativeError, "search overrides"):
                    c.providers_from_build()
                with self.assertRaisesRegex(ValueError, "Unclassified"):
                    c.apple.link_requirements(["dylib=unknown"])

    def test_tapi_v5_and_v4_select_target_not_compatibility_version(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "library.tbd"
            document = {"tapi_tbd_version": 5, "main_library": {
                "target_info": [{"target": "arm64-ios-simulator"}],
                "install_names": [{"name": SYSTEM}], "current_versions": [{"version": "999.0"}]}}
            path.write_text(json.dumps(document))
            self.assertEqual(stub_identity(path, "aarch64-apple-ios-sim"), SYSTEM)
            with self.assertRaises(ValueError):
                stub_identity(path, "aarch64-apple-ios")
            stub(path, SYSTEM, "arm64-ios-simulator")
            self.assertEqual(stub_identity(path, "aarch64-apple-ios-sim"), SYSTEM)

    def test_preflight_selects_host_and_target_tools_without_elf_exemption(self):
        for platform, target, expected in (("darwin", "aarch64-apple-darwin", {"xcrun"}),
                                           ("darwin", None, {"xcrun"}),
                                           ("darwin", "x86_64-unknown-linux-gnu", {"xcrun", "readelf"}),
                                           ("linux", "aarch64-apple-darwin", {"xcrun", "readelf"}),
                                           ("linux", "aarch64-linux-android", {"readelf"}),
                                           ("linux", "x86_64-unknown-linux-gnu", {"readelf"})):
            args = sbom.parser().parse_args(["cdk", "--build"] + (["--target", target] if target else []))
            c = sbom.Collector(args)
            found = set()
            with patch.object(sbom.sys, "platform", platform), \
                    patch.object(sbom.shutil, "which", side_effect=lambda tool: found.add(tool)), \
                    self.subTest(platform=platform, target=target), self.assertRaises(sbom.SbomError):
                c.preflight([])
            self.assertEqual(found & {"xcrun", "readelf"}, expected)


if __name__ == "__main__":
    unittest.main()
