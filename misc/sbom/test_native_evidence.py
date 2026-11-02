"""Native coverage fails closed for missing, modified or unrecognized evidence."""

import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from native_evidence import NativeCollector, NativeError, build_records, check_link_overrides, elf_search_paths, nix_identity, sha256, verified_registry_source
import sbom
from test_sbom import bom, rust_metadata


def registry_fixture(root):
    source = root / "registry/src/example-index/libsqlite3-sys-1.0.0"
    source.mkdir(parents=True)
    (source / "sqlite3").mkdir()
    (source / "sqlite3/sqlite3.h").write_text('#define SQLITE_VERSION "3.45.0"\n')
    (source / "Cargo.toml").write_text('[package]\nname="libsqlite3-sys"\nversion="1.0.0"\n')
    archive = root / "registry/cache/example-index/libsqlite3-sys-1.0.0.crate"
    archive.parent.mkdir(parents=True)
    with tarfile.open(archive, "w:gz") as output:
        for relative in ("Cargo.toml", "sqlite3/sqlite3.h"):
            output.add(source / relative, arcname="libsqlite3-sys-1.0.0/" + relative)
    package = {"id": "sqlite", "name": "libsqlite3-sys", "version": "1.0.0", "links": "sqlite3",
               "source": "registry+https://example.invalid/index", "manifest_path": str(source / "Cargo.toml")}
    lock = {"package": [{key: package[key] for key in ("name", "version", "source")} | {"checksum": sha256(archive)}]}
    return source, archive, package, lock


class NativeTests(unittest.TestCase):
    def test_library_filename_is_not_lost_when_same_package_emits_a_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.lock").write_text("version=4\npackage=[]\n")
            profile = root / "target/x86_64-unknown-linux-gnu/release"
            profile.mkdir(parents=True)
            library = profile / "libproduct.rlib"
            library.write_bytes(b"!<arch>\nfixture")
            records = [{"reason": "compiler-artifact", "package_id": "product", "target": {"kind": [kind]},
                        "filenames": [str(path)]}
                       for kind, path in (("lib", library), ("bin", profile / "product"))]
            records.append({"reason": "build-finished", "success": True})
            collector = NativeCollector({}, [], records, "x86_64-unknown-linux-gnu", profile, root, None)
            with patch.dict("os.environ", {"CARGO_HOME": str(root / "cargo-home")}, clear=True):
                document, _ = collector.collect(library, sha256(library))
            self.assertEqual(document["metadata"]["component"]["name"], library.name)

    def test_android_platform_requirements_do_not_invent_a_runtime_os_version(self):
        from test_package_inventory import elf
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ndk = root / "ndk"
            (ndk / "meta").mkdir(parents=True)
            (ndk / "source.properties").write_text("Pkg.Revision = 27.0.12077973\n")
            (ndk / "meta/system_libs.json").write_text('{"libc.so": 1}')
            toolchain = ndk / "toolchains/llvm/prebuilt/linux-x86_64"
            linker = toolchain / "bin/aarch64-linux-android24-clang"
            linker.parent.mkdir(parents=True)
            linker.write_text("NDK linker fixture")
            stub = toolchain / "sysroot/usr/lib/aarch64-linux-android/24/libc.so"
            stub.parent.mkdir(parents=True)
            stub.write_bytes(elf())
            artifact = root / "wallet.so"
            artifact.write_bytes(elf())
            (root / "Cargo.lock").write_text("version=4\npackage=[]\n")
            output = "(NEEDED) Shared library: [libc.so]\n"
            def runner(args, cwd):
                return output
            collector = NativeCollector({}, [], [{"reason": "build-finished", "success": True}],
                                        "aarch64-linux-android", root / "target", root, runner)
            with patch.dict("os.environ", {"ANDROID_NDK_HOME": str(ndk),
                                          "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER": str(linker)}):
                requirements = collector.android_libraries(artifact)
                self.assertEqual(requirements[0]["api"], "24")
                self.assertNotIn("version", collector.components[0])
                self.assertNotIn("hashes", collector.components[0])
                self.assertEqual(requirements[0]["stub"]["sha256"], sha256(stub))
                output = "(NEEDED) Shared library: [libunknown.so]\n"
                with self.assertRaisesRegex(NativeError, "provider evidence is required"):
                    collector.android_libraries(artifact)

    def test_unobserved_linker_inputs_cannot_be_smuggled_through_flags(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict("os.environ", {"CARGO_HOME": str(root / "cargo-home")}, clear=True):
                check_link_overrides(root, "x86_64-unknown-linux-gnu")
                with patch.dict("os.environ", {"RUSTFLAGS": "-C opt-level=3"}):
                    check_link_overrides(root, "x86_64-unknown-linux-gnu")
                for flags in ("-Lnative=/unknown", "-lunknown", "-Clink-arg=-lunknown", "-Ctarget-feature=+crt-static"):
                    with self.subTest(flags=flags), patch.dict("os.environ", {"RUSTFLAGS": flags}):
                        with self.assertRaisesRegex(NativeError, "linker override"):
                            check_link_overrides(root, "x86_64-unknown-linux-gnu")
                (root / ".cargo").mkdir()
                (root / ".cargo/config.toml").write_text('[build]\nrustflags=["-L", "native=/unknown"]\n')
                with self.assertRaisesRegex(NativeError, "linker override"):
                    check_link_overrides(root, "x86_64-unknown-linux-gnu")

    def test_elf_empty_runpath_is_not_a_cwd_entry(self):
        origin = Path("/safe/bin")
        self.assertEqual(elf_search_paths("(RUNPATH) Library runpath: []", origin), [])
        self.assertEqual(elf_search_paths("(RUNPATH) Library runpath: [$ORIGIN/../lib]", origin),
                         [Path("/safe/bin/../lib")])
        for paths in ("/safe/lib:", ":/safe/lib", "/safe/lib::/other", "relative", "$LIB"):
            with self.subTest(paths=paths), self.assertRaisesRegex(NativeError, "unsafe"):
                elf_search_paths(f"(RUNPATH) Library runpath: [{paths}]", origin)
        self.assertEqual(elf_search_paths("(RPATH) Library rpath: [relative]\n(RUNPATH) Library runpath: []", origin), [])

    def test_nix_identity_requires_actual_output_and_version_metadata(self):
        store = Path("/nix/store/hash-not-an-upstream-version")
        legacy = {"derivation": {"env": {"pname": "glibc", "version": "2.42-84"},
                                  "outputs": {"out": {"path": str(store)}}}}
        self.assertEqual(nix_identity(legacy, store), ("glibc", "2.42-84"))
        modern = {"derivations": {"derivation": {"env": {},
                    "structuredAttrs": {"pname": "gcc", "version": "15.3.0"},
                    "outputs": {"libgcc": {"path": store.name}}}}}
        self.assertEqual(nix_identity(modern, store), ("gcc", "15.3.0"))
        with self.assertRaisesRegex(NativeError, "does not identify"):
            nix_identity(modern, Path("/nix/store/another-output"))
        del modern["derivations"]["derivation"]["structuredAttrs"]["version"]
        with self.assertRaisesRegex(NativeError, "does not identify"):
            nix_identity(modern, store)

    def test_build_record_is_complete_and_does_not_dump_environment(self):
        output = '\n'.join(json.dumps(message) for message in [
            {"reason": "build-script-executed", "package_id": "dep", "out_dir": "/out",
             "linked_libs": [], "linked_paths": [], "env": [["SECRET", "must-not-be-recorded"]]},
            {"reason": "build-finished", "success": True},
        ])
        records = build_records(output)
        self.assertNotIn("SECRET", json.dumps(records))
        with self.assertRaises(NativeError):
            build_records(output.splitlines()[0])
        with self.assertRaises(NativeError):
            build_records('{"reason":"build-finished","success":false}')

    def test_registry_sources_are_verified_against_locked_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            source, archive, package, lock = registry_fixture(Path(directory))
            (source / ".cargo-ok").write_text("Cargo extraction marker")
            actual, evidence = verified_registry_source(package, lock)
            self.assertEqual(actual, source)
            self.assertEqual(evidence["sha256"], sha256(archive))
            self.assertEqual(evidence["verified_files"], 2)
            (source / "sqlite3/sqlite3.h").write_text('#define SQLITE_VERSION "invented"\n')
            with self.assertRaisesRegex(NativeError, "Modified native source"):
                verified_registry_source(package, lock)

    def test_modified_archive_and_extra_sources_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            source, archive, package, lock = registry_fixture(Path(directory))
            (source / "added.c").write_text("unreviewed source")
            with self.assertRaisesRegex(NativeError, "Untracked native source"):
                verified_registry_source(package, lock)
            (source / "added.c").unlink()
            archive.write_bytes(b"changed")
            with self.assertRaisesRegex(NativeError, "modified locked source archive"):
                verified_registry_source(package, lock)

    def test_archive_traversal_is_rejected_without_extracting(self):
        with tempfile.TemporaryDirectory() as directory:
            _, archive, package, lock = registry_fixture(Path(directory))
            with tarfile.open(archive, "w:gz") as output:
                member = tarfile.TarInfo("libsqlite3-sys-1.0.0/../../escape")
                member.size = 1
                output.addfile(member, io.BytesIO(b"x"))
            lock["package"][0]["checksum"] = sha256(archive)
            with self.assertRaisesRegex(NativeError, "Invalid path"):
                verified_registry_source(package, lock)
            self.assertFalse((Path(directory) / "escape").exists())

    def test_uncompiled_and_unclassified_links_are_not_confused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.lock").write_text("version = 4\npackage = []\n")
            package = {"id": "unknown", "name": "unknown-native", "links": "unknown"}
            finish = {"reason": "build-finished", "success": True}
            collector = NativeCollector({}, [package], [finish], "target", root / "target", root, None)
            collector.providers_from_build()
            self.assertEqual(collector.providers[0]["classification"], "resolved-only-not-compiled")
            collector.compiled["unknown"] = {"features": []}
            collector.scripts["unknown"] = {"linked_libs": [], "out_dir": str(root / "target/out")}
            with self.assertRaisesRegex(NativeError, "Unclassified"):
                collector.providers_from_build()
            with self.assertRaisesRegex(NativeError, "complete successful"):
                NativeCollector({}, [package], [], "target", root / "target", root, None)

    def test_unresolved_shared_library_fails_without_executing_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "program"
            artifact.write_bytes(b"\x7fELFfixture")
            (root / "Cargo.lock").write_text("version=4\npackage=[]\n")
            calls = []
            def runner(args, cwd):
                calls.append(args)
                return "(NEEDED) Shared library: [libunknown.so.1]\n"
            collector = NativeCollector({}, [], [{"reason": "build-finished", "success": True}],
                                        "x86_64-unknown-linux-gnu", root / "target", root, runner)
            with self.assertRaisesRegex(NativeError, "Cannot resolve shared-library"):
                collector.shared_libraries(artifact)
            self.assertEqual(calls[0][:3], ["readelf", "--dynamic", "--wide"])


    def test_native_version_is_from_header_not_rust_crate_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            _, _, package, lock = registry_fixture(root)
            (root / "Cargo.lock").write_text('version = 4\n[[package]]\nname="libsqlite3-sys"\nversion="1.0.0"\n'
                                             f'source="{package["source"]}"\nchecksum="{lock["package"][0]["checksum"]}"\n')
            out = root / "target/out"
            out.mkdir(parents=True)
            (out / "libsqlite3.a").write_bytes(b"!<arch>\narchive fixture")
            collector = NativeCollector({}, [package], [{"reason": "build-finished", "success": True}],
                                        "target", root / "target", root, None)
            script = {"linked_libs": ["static=sqlite3"], "linked_paths": ["native=" + str(out)], "out_dir": str(out)}
            collector.bundled(package, script, ["bundled"])
            sqlite = next(component for component in collector.components if component["name"] == "sqlite")
            self.assertEqual(sqlite["version"], "3.45.0")
            self.assertEqual(len(collector.providers[0]["archives"]), 1)
            script["linked_libs"] = ["sqlite3"]
            with self.assertRaisesRegex(NativeError, "Unexpected native link flags"):
                collector.bundled(package, script, ["bundled"])

    def test_build_sbom_is_scannable_and_keeps_dual_use_dependencies(self):
        metadata = rust_metadata()
        source = bom()
        for package in metadata["packages"]:
            if package["id"] not in {"root", "dependency", "dev"}:
                source["components"].append({"bom-ref": package["id"], "type": "library",
                                             "name": package["name"], "version": package["version"]})
        sbom.classify_rust_bom(source, metadata, {"name": "cdk"})
        document = sbom.rust_build_bom(source, metadata, {"name": "cdk"})
        sbom.check_graph(document)
        indexed = {component["bom-ref"]: component for component in document["components"]}
        self.assertIn("derive", indexed)
        self.assertIn("helper", indexed)
        self.assertIn("shared", indexed)
        self.assertNotIn("dev", indexed)
        self.assertTrue(all(component["scope"] == "required" for component in indexed.values()))
        self.assertEqual(next(c for c in source["components"] if c["bom-ref"] == "derive")["scope"], "excluded")


if __name__ == "__main__":
    unittest.main()
