"""Release assurances, byte transformations and signer policy have independent checks."""

import copy
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from assurance import assessment, inherited_dimensions, require
from elf_evidence import elf_runtime, verify_postprocessing
from finalize import check_platform, finalize
from link_evidence import archive_members
from provenance import digest, key_fingerprint, read_run, sign_run, verify_run
import sbom
import android_consumer


def elf(path, text=b"runtime", audit=b"audit", debug=b"debug"):
    """A minimal ELF with distinguishable runtime, audit and debug sections."""
    names = b"\0.text\0.dep-v0\0.debug_info\0.shstrtab\0"
    payloads = [text, audit, debug, names]
    offsets = []
    cursor = 120
    for payload in payloads:
        offsets.append(cursor)
        cursor += len(payload)
    shoff = cursor
    header = b"\x7fELF\x02\x01\x01" + bytes(9) + struct.pack("<HHIQQQIHHHHHH", 3, 62, 1, 0, 64, shoff, 0, 64, 56, 1, 64, 5, 4)
    program = struct.pack("<IIQQQQQQ", 1, 5, 0, 4096, 4096, 120 + len(text), 120 + len(text), 4096)
    sections = bytes(64)
    for name, kind, flags, address, offset, payload in zip(
            (1, 7, 15, 27), (1, 1, 1, 3), (6, 0, 0, 0), (4216, 0, 0, 0), offsets, payloads):
        sections += struct.pack("<IIQQQQIIQQ", name, kind, flags, address, offset, len(payload), 0, 0, 1, 0)
    path.write_bytes(header + program + b"".join(payloads) + sections)


def complete_run(root):
    root.mkdir()
    evidence = root / "proof.json"
    evidence.write_text('{"proof":true}\n')
    index = {"status": "complete", "configuration": {"target": "x86_64-unknown-linux-gnu", "profile": "release"},
             "source": {"revision": "abc", "dirty": False}, "documents": [],
             "evidence": [{"path": "proof.json", "sha256": digest(evidence)}], "artifacts": []}
    sbom.write_json(root / "index.json", index)
    return index


class ReleaseEvidenceTests(unittest.TestCase):
    def test_debug_changes_are_not_runtime_changes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            original, final = root / "original", root / "final"
            elf(original)
            elf(final, debug=b"different debug content")
            self.assertEqual(verify_postprocessing(original, final)["method"], "elf-mapped-segments-and-embedded-audit-equivalence-v2")
            elf(final, text=b"changed runtime")
            with self.assertRaisesRegex(ValueError, "loader-visible"):
                verify_postprocessing(original, final)
            elf(final, audit=b"changed package evidence")
            with self.assertRaisesRegex(ValueError, "embedded audit"):
                verify_postprocessing(original, final)

    def test_malformed_elf_does_not_get_transform_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "bad"
            path.write_bytes(b"\x7fELF\x02\x01" + bytes(10))
            with self.assertRaises(ValueError):
                elf_runtime(path)

    def test_complete_mapped_bytes_and_program_offsets_are_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            original, final = Path(directory) / "original", Path(directory) / "final"
            elf(original)
            content = bytearray(original.read_bytes())
            shoff = struct.unpack_from("<Q", content, 40)[0]
            # Leave one mapped byte outside the allocated section's declared size.
            struct.pack_into("<Q", content, shoff + 64 + 32, 6)
            original.write_bytes(content)
            changed = bytearray(content)
            changed[126] ^= 1
            final.write_bytes(changed)
            with self.assertRaisesRegex(ValueError, "loader-visible"):
                verify_postprocessing(original, final)
            changed = bytearray(content)
            struct.pack_into("<Q", changed, 64 + 8, 1)
            final.write_bytes(changed)
            with self.assertRaises(ValueError):
                verify_postprocessing(original, final)

    def test_only_section_table_header_fields_can_change(self):
        with tempfile.TemporaryDirectory() as directory:
            original, final = Path(directory) / "original", Path(directory) / "final"
            elf(original)
            for offset in (24, 32, 48, 52, 54):
                changed = bytearray(original.read_bytes())
                changed[offset] ^= 1
                final.write_bytes(changed)
                with self.subTest(offset=offset), self.assertRaises(ValueError):
                    verify_postprocessing(original, final)

    def test_private_avd_directory_matches_sdk_manager_default(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "input.aar"
            archive.write_bytes(b"fixture")
            manager = root / "sdk/cmdline-tools/latest/bin/avdmanager"
            manager.parent.mkdir(parents=True)
            manager.touch()
            args = SimpleNamespace(aar=archive, output=root / "result", sdk=root / "sdk", serial=None,
                                   start_emulator=True, port=5560, gradle_wrapper=root / "gradlew")

            def create(arguments, environment, *unused, **kwargs):
                self.assertEqual(Path(environment["ANDROID_AVD_HOME"]), Path(environment["ANDROID_USER_HOME"]) / "avd")
                self.assertEqual(environment["ANDROID_EMULATOR_HOME"], environment["ANDROID_USER_HOME"])
                name = arguments[arguments.index("--name") + 1]
                (Path(environment["ANDROID_AVD_HOME"]) / (name + ".ini")).touch()

            with patch("android_consumer.command", side_effect=create), patch("android_consumer.socket.socket"), \
                    patch("android_consumer.subprocess.Popen", side_effect=OSError("fixture stop")):
                with self.assertRaisesRegex(OSError, "fixture stop"):
                    android_consumer.test(args)

    def test_android_apk_must_execute_the_final_aar_library_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            aar, apk = root / "release.aar", root / "app.apk"
            with zipfile.ZipFile(aar, "w") as archive:
                archive.writestr("jni/x86_64/libcdk_ffi_kotlin.so", b"verified native bytes")
            with zipfile.ZipFile(apk, "w") as archive:
                archive.writestr("lib/x86_64/libcdk_ffi_kotlin.so", b"changed native bytes")
            with self.assertRaisesRegex(ValueError, "native bytes differ"):
                android_consumer.verify_apk_library(aar, apk)
            with zipfile.ZipFile(apk, "w") as archive:
                archive.writestr("lib/x86_64/libcdk_ffi_kotlin.so", b"verified native bytes")
            self.assertEqual(len(android_consumer.verify_apk_library(aar, apk)), 64)

    def test_assurance_cannot_promote_file_coverage_to_upstream_identity(self):
        index = {"documents": [{"kind": "rust-consumer-resolved"}], "coverage": {"packaged_native_build_hashes_checked": True},
                 "distributions": [{"path": "release.aar"}]}
        result = assessment(index)
        require(result, ["resolved_dependencies", "final_bytes", "packaged_files"])
        self.assertEqual(result["overall"], "partial")
        with self.assertRaisesRegex(ValueError, "upstream_native_identities=partial"):
            require(result, ["upstream_native_identities"])
        index["coverage"]["external_native_assertions"] = True
        with self.assertRaisesRegex(ValueError, "final_bytes=partial"):
            require(assessment(index), ["final_bytes"])

    def test_abi_label_must_match_original_target(self):
        record = {"path": "release.aar!jni/x86/libwallet.so"}
        artifact = {"target": "aarch64-linux-android", "name": "libwallet.so"}
        with self.assertRaisesRegex(ValueError, "ABI"):
            check_platform(record, artifact)
        artifact["target"] = "i686-linux-android"
        check_platform(record, artifact)
        with self.assertRaisesRegex(ValueError, "ABI"):
            check_platform({"path": "release.aar!jni/unsupported/libwallet.so"}, artifact)

    def test_finalization_does_not_invent_observed_compilation(self):
        inventory = {"documents": [{"kind": "go-resolved"}]}
        self.assertEqual(assessment(inventory)["dimensions"]["resolved_dependencies"]["status"], "verified")
        result = inherited_dimensions([inventory])
        self.assertEqual(result["compiled_packages"]["status"], "not_observed")
        self.assertEqual(result["linker_inputs"]["status"], "build_records_only")
        build = {"coverage": {"compiled_packages": {"target": 1}, "linker_maps": 1}}
        self.assertEqual(inherited_dimensions([build])["compiled_packages"]["status"], "observed")
        self.assertEqual(inherited_dimensions([build, inventory])["compiled_packages"]["status"], "not_observed")

    def test_link_map_retains_named_archive_contributions(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "libcrypto.a"
            archive.write_bytes(b"!<arch>\nfixture")
            records = archive_members(f" 0000 0000 {archive}(cipher.o)\n 0000 {archive}(hash.o)\n")
            self.assertEqual(records[0]["members"], ["cipher.o", "hash.o"])
            self.assertEqual(records[0]["sha256"], digest(archive))

    def test_modified_or_escaping_build_evidence_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "run"
            index = complete_run(root)
            read_run(root)
            (root / "proof.json").write_text("modified")
            with self.assertRaisesRegex(ValueError, "hash does not match"):
                read_run(root)
            index["evidence"][0]["path"] = "../outside"
            sbom.write_json(root / "index.json", index)
            with self.assertRaisesRegex(ValueError, "Unsafe"):
                read_run(root)

    @unittest.skipUnless(shutil.which("openssl"), "requires OpenSSL")
    def test_signature_requires_authorized_builder_target_and_unchanged_index(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "run"
            index = complete_run(root)
            key, public = base / "key.pem", base / "public.pem"
            subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", key], check=True, capture_output=True)
            key.chmod(0o600)
            subprocess.run(["openssl", "pkey", "-in", key, "-pubout", "-out", public], check=True, capture_output=True)
            policy = {"version": 1, "profiles": ["release"], "keys": [{
                "public_key": public.name, "sha256": key_fingerprint(public), "builder": "test-builder",
                "targets": ["x86_64-unknown-linux-gnu"]}]}
            policy_path = base / "policy.json"
            sbom.write_json(policy_path, policy)
            sign_run(root, key, "test-builder")
            self.assertEqual(verify_run(root, policy_path)["status"], "authenticated")
            bad = copy.deepcopy(policy)
            bad["keys"][0]["targets"] = ["aarch64-linux-android"]
            sbom.write_json(policy_path, bad)
            with self.assertRaisesRegex(ValueError, "not authorized for this target"):
                verify_run(root, policy_path)
            sbom.write_json(policy_path, policy)
            index["source"]["revision"] = "different"
            sbom.write_json(root / "index.json", index)
            with self.assertRaisesRegex(ValueError, "exact index"):
                verify_run(root, policy_path)

    def test_finalizer_missing_original_leaves_incomplete_index(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            complete_run(base / "run")
            original = base / "original"
            original.write_bytes(b"unknown artifact")
            args = SimpleNamespace(output=base / "output", build_run=[base / "run"], original=[original],
                                   inventory_run=[], trust_policy=None, distribution=[], strip_tool=None,
                                   go_project=None, go_runner=None, go_runner_arg=[], require_assurance=["final_bytes"])
            with self.assertRaisesRegex(ValueError, "not uniquely bound"):
                finalize(args)
            self.assertEqual(sbom.read_json(base / "output/index.json")["status"], "incomplete")

    def test_android_consumer_refuses_physical_devices_without_installing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "input.aar"
            archive.write_bytes(b"fixture")
            args = SimpleNamespace(aar=archive, output=root / "result", sdk=root / "sdk", serial="physical-phone",
                                   start_emulator=False, port=5560)
            with patch("android_consumer.command") as command:
                with self.assertRaisesRegex(ValueError, "physical devices"):
                    android_consumer.test(args)
            command.assert_not_called()
            self.assertEqual(sbom.read_json(root / "result/consumer-result.json")["status"], "incomplete")

    @unittest.skipUnless(os.environ.get("CDK_SBOM_REAL_GRADLE_TESTS") == "1", "opt-in real Gradle integration")
    def test_maven_guard_checks_the_actual_staged_aar(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "settings.gradle.kts").write_text('rootProject.name="publication-check"\n')
            (root / "build.gradle.kts").write_text('''plugins { `maven-publish` }
group = "org.example"
version = "1"
publishing {
    publications { create<MavenPublication>("release") { artifact(file("payload.aar")) } }
    repositories { maven { name = "test"; url = uri(layout.buildDirectory.dir("repository")) } }
}
''')
            archive = root / "payload.aar"
            archive.write_bytes(b"the exact verified release bytes")
            environment = dict(os.environ, CDK_SBOM_RELEASE_AAR_SHA256="0" * 64)
            arguments = ["bash", str(sbom.ROOT / "bindings/kotlin/gradlew"), "-p", str(root), "--offline", "--no-daemon",
                         "--init-script", str(sbom.ROOT / "misc/sbom/release-check.init.gradle"),
                         "publishReleasePublicationToTestRepository"]
            failed = subprocess.run(arguments, env=environment, capture_output=True, text=True, timeout=180)
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("AAR bytes differ", failed.stdout + failed.stderr)
            self.assertFalse(list((root / "build/repository").rglob("*.aar")))
            environment["CDK_SBOM_RELEASE_AAR_SHA256"] = digest(archive)
            passed = subprocess.run(arguments, env=environment, capture_output=True, text=True, timeout=180)
            self.assertEqual(passed.returncode, 0, passed.stdout + passed.stderr)
            published = list((root / "build/repository").rglob("*.aar"))
            self.assertEqual(len(published), 1)
            self.assertEqual(digest(published[0]), digest(archive))


if __name__ == "__main__":
    unittest.main()
