"""Platform recipes shared by candidate CI and independent local rebuilds."""

import os
from pathlib import Path
import plistlib
import re
import shutil
import tomllib

from model import (
    ANDROID,
    APPLE,
    DART_TARGETS,
    GO_TARGETS,
    WINDOWS,
    archive,
    dart_archive,
    dependencies,
    empty_directory,
    expected,
    export_source,
    extract,
    go_library,
    host,
    inputs,
    inventory,
    library,
    read_json,
    require,
    run,
    scratch,
    supported_groups,
    tree_inventory,
    write_json,
)


def build(root, group, output, candidate=None):
    require(group in supported_groups(), f"{group} cannot be built on {host()}")
    empty_directory(output)
    require(
        not dependencies(group) or candidate,
        f"{group} requires --candidate with imported artifacts",
    )
    work = scratch()
    print(f"Build workspace: {work}", flush=True)
    try:
        identity = export_source(root, work / "source")
        write_json(work / "identity.json", identity)
        if candidate:
            # Validate imported bytes against their producer's recorded source.
            for name in dependencies(group):
                producer = next(
                    g
                    for g in (
                        "generated",
                        "linux-x86_64",
                        "linux-aarch64",
                        "android",
                        "darwin",
                        "windows",
                    )
                    if name in expected(g, identity["version"])
                )
                record = read_json(candidate / f"group-{producer}.json")
                require(
                    record["source"] == identity,
                    f"Imported {name} is from a different source",
                )
                require(
                    record["files"].get(name) == inventory(candidate, [name])[name],
                    f"Imported {name} changed",
                )
        shell = (
            f"bindings-release-{group}"
            if group in ("android", "windows")
            else "bindings-release"
        )
        engine = os.environ.get("CONTAINER_ENGINE", "docker")
        require(
            engine in ("docker", "podman"), "CONTAINER_ENGINE must be docker or podman"
        )
        environment = dict(os.environ)
        environment["CDK_CONTAINER_ENGINE"] = engine
        command = ["nix", "develop", "--no-update-lock-file", "--ignore-environment"]
        # Only transport/host prerequisites cross the environment boundary.
        for name in (
            "HOME",
            "USER",
            "TMPDIR",
            "NIX_REMOTE",
            "CDK_CONTAINER_ENGINE",
            "DOCKER_HOST",
            "DEVELOPER_DIR",
            "CDK_RELEASE_XCODE",
        ):
            if name in environment:
                command.extend(["--keep", name])
        command.extend(
            [
                f"path:{work / 'source'}#{shell}",
                "--command",
                "python3",
                work / "source/scripts/bindings/release.py",
                "--root",
                work / "source",
                "_build",
                "--group",
                group,
                "--output",
                output,
                "--identity",
                work / "identity.json",
            ]
        )
        if candidate:
            command.extend(["--candidate", candidate])
        run(command, env=environment)
    except BaseException:
        print(f"Build failed; diagnostic workspace retained at {work}", flush=True)
        raise
    else:
        shutil.rmtree(work)


class Builder:
    def __init__(self, root, group, output, identity, candidate):
        require(
            os.environ.get("CDK_RELEASE_ENV") == "1",
            "Build through the pinned Nix entrypoint",
        )
        self.root, self.group, self.output = root, group, output
        self.identity, self.candidate = identity, candidate
        self.config = inputs(root)
        self.work = root.parent
        self.generated = self.work / "generated"
        if candidate:
            extract(candidate / "generated.tar", self.generated)
        target_parent = Path("/data/rust/targets")
        # A fresh target and no compiler cache are required for independent builds.
        # Never clean the workstation's shared Cargo target directory.
        self.target = (
            target_parent / self.work.name
            if target_parent.is_dir() and os.access(target_parent, os.W_OK)
            else self.work / "target"
        )
        empty_directory(self.target)
        self.env = dict(os.environ)
        cargo_home = self.work / "cargo-home"
        cargo_home.mkdir()
        vendor = Path(self.env["CDK_RELEASE_VENDOR"])
        shutil.copyfile(vendor / "config.toml", cargo_home / "config.toml")
        mappings = [
            (str(self.work), "/build"),
            (str(self.target), "/build/target"),
            (str(vendor), "/vendor"),
        ]
        mappings += [
            (str(p.resolve()), f"/vendor/{p.name}")
            for p in sorted(vendor.iterdir())
            if p.is_dir()
        ]
        flags = " ".join(f"--remap-path-prefix={a}={b}" for a, b in mappings)
        cflags = " ".join(f"-ffile-prefix-map={a}={b}" for a, b in mappings)
        self.env.update(
            {
                "CARGO_HOME": str(cargo_home),
                "CARGO_TARGET_DIR": str(self.target),
                "RUSTC_WRAPPER": "",
                "RUSTC_WORKSPACE_WRAPPER": "",
                "RUSTFLAGS": flags,
                "CFLAGS": cflags,
                "CXXFLAGS": cflags,
                "SOURCE_DATE_EPOCH": str(identity["epoch"]),
                "ZERO_AR_DATE": "1",
                "TZ": "UTC",
                "LC_ALL": "C",
                "CARGO_NET_OFFLINE": "true",
                "GOCACHE": str(self.work / "go-cache"),
                "GOPATH": str(self.work / "go"),
                "GRADLE_USER_HOME": str(self.work / "gradle-home"),
                "ANDROID_USER_HOME": str(self.work / "android-home"),
                "DASH__SUPPRESS_ANALYTICS": "true",
            }
        )
        if group == "generated":
            # Metadata-only host binaries retain release cfgs, but optimizing
            # their implementation does not affect the extracted interface.
            self.env["CARGO_PROFILE_RELEASE_OPT_LEVEL"] = "0"
        with (root / "rust-toolchain.toml").open("rb") as handle:
            rust_version = tomllib.load(handle)["toolchain"]["channel"]
        require(
            run(["rustc", "--version"], env=self.env, capture=True).split()[1]
            == rust_version,
            "Rust version does not match the recipe",
        )

    def command(self, args, cwd=None, capture=False, env=None):
        return run(args, cwd=cwd or self.root, env=env or self.env, capture=capture)

    def cargo(self, language, target=None, *, binary=None, extra=()):
        # Metadata extraction needs a host library, not distribution LTO. It
        # retains the same release cfgs and features as the shipped library.
        profile = "release" if self.group == "generated" else self.config["profile"]
        command = ["cargo"]
        if target == WINDOWS:
            command.append("xwin")
        command += ["build", "--frozen", "--offline", "-p", f"cdk-ffi-{language}"]
        if binary:
            command += ["--release", "--bin", binary]
        else:
            command += ["--profile", profile, "--lib"]
        if target:
            command += ["--target", target]
        self.command([*command, *extra])
        if binary:
            directory = self.target / target if target else self.target
            return directory / "release" / binary
        return self.target / target / profile / library(language, target)

    def template(self, language, destination):
        shutil.copytree(
            self.root / "bindings" / language,
            destination,
            ignore=shutil.ignore_patterns(
                "rust",
                "target",
                ".git",
                ".github",
                ".dart_tool",
                ".gradle",
                "build",
                "prebuilt",
                "rust-toolchain.toml",
                "pubspec.lock",
                "local.properties",
            ),
        )
        for name in ("LICENSE", "LICENSE-MIT", "LICENSE-APACHE"):
            if (self.root / name).is_file():
                shutil.copyfile(self.root / name, destination / name)
        if language == "go":
            # Monorepo tests are copied into the generated package for the
            # consumer check, not shipped as a broken root-level Go package.
            (destination / "cdk_test.go").unlink(missing_ok=True)
        if language == "dart":
            path = destination / "pubspec.yaml"
            path.write_text(
                re.sub(
                    r"(?m)^version:.*$",
                    f'version: "{self.identity["version"]}"',
                    path.read_text(),
                )
            )
            shutil.copytree(
                self.generated / "dart",
                destination / "lib/src/generated",
                dirs_exist_ok=True,
            )
        elif language == "kotlin":
            path = destination / "gradle.properties"
            path.write_text(
                re.sub(
                    r"(?m)^VERSION_NAME=.*$",
                    f"VERSION_NAME={self.identity['version']}",
                    path.read_text(),
                )
            )
            shutil.copytree(
                self.generated / "kotlin",
                destination / "cdk-jvm/src/main/kotlin",
                dirs_exist_ok=True,
            )

    def package_dart(self, target, binary):
        stage = self.work / f"dart-{target}"
        self.template("dart", stage)
        destination = stage / "prebuilt" / target
        destination.mkdir(parents=True)
        shutil.copyfile(binary, destination / library("dart", target))
        archive(stage, self.output / dart_archive(target), self.identity["epoch"])

    def generated_sources(self):
        self.generated.mkdir()
        host_target = "x86_64-unknown-linux-gnu"
        # These host libraries are used only to read UniFFI metadata. They are
        # never distributed as Linux-native payloads.
        for language in ("dart", "go", "kotlin", "swift"):
            native = self.cargo(language, host_target)
            output = self.generated / language
            output.mkdir()
            if language == "go":
                self.command(
                    [
                        "uniffi-bindgen-go",
                        native,
                        "--library",
                        "--config",
                        self.root / "bindings/go/uniffi.toml",
                        "--out-dir",
                        output,
                    ]
                )
            elif language == "swift":
                generator = self.cargo(
                    language, host_target, binary="uniffi-bindgen-swift"
                )
                self.command(
                    [
                        generator,
                        native,
                        output,
                        "--swift-sources",
                        "--headers",
                        "--modulemap",
                        "--module-name",
                        "CashuDevKitFFI",
                        "--modulemap-filename",
                        "module.modulemap",
                    ]
                )
                for path in output.glob("*.swift"):
                    text = path.read_text()
                    for module in ("cdk_ffiFFI", "cdkFFI"):
                        text = text.replace(
                            f"canImport({module})", "canImport(CashuDevKitFFI)"
                        ).replace(f"import {module}", "import CashuDevKitFFI")
                    path.write_text(text)
            else:
                generator = self.cargo(
                    language, host_target, binary=f"uniffi-bindgen-{language}"
                )
                if language == "dart":
                    self.command(
                        [generator, native, "--out-dir", output],
                        cwd=self.root / "bindings/dart/rust",
                    )
                else:
                    self.command(
                        [
                            generator,
                            "generate",
                            "--library",
                            native,
                            "--language",
                            "kotlin",
                            "--config",
                            self.root / "bindings/kotlin/rust/uniffi.toml",
                            "--out-dir",
                            output,
                            "--no-format",
                        ]
                    )
            require(any(output.rglob("*")), f"Generator produced no {language} sources")
        archive(self.generated, self.output / "generated.tar", self.identity["epoch"])

    def linux(self):
        arch = self.group[6:]
        target = f"{arch}-unknown-linux-gnu"
        engine = shutil.which(self.env.get("CDK_CONTAINER_ENGINE", "docker"))
        require(
            engine,
            "Docker or CONTAINER_ENGINE=podman is required for the pinned Linux sysroot",
        )
        image = self.config["manylinux"][arch]
        self.command([engine, "pull", image])
        container_env = {
            "CDK_RELEASE_RUST": self.env["CDK_RELEASE_RUST"],
            "CARGO_HOME": "/work/cargo-home",
            "CARGO_TARGET_DIR": "/target",
            "RUSTFLAGS": self.env["RUSTFLAGS"]
            + " --remap-path-prefix=/work=/build --remap-path-prefix=/target=/build/target",
            "CFLAGS": self.env["CFLAGS"]
            + " -ffile-prefix-map=/work=/build -ffile-prefix-map=/target=/build/target",
            "SOURCE_DATE_EPOCH": self.env["SOURCE_DATE_EPOCH"],
            "LC_ALL": "C",
            "TZ": "UTC",
            "CDK_TARGET": target,
            "CDK_PROFILE": self.config["profile"],
        }
        command = [
            engine,
            "run",
            "--rm",
            "--network=none",
            "--read-only",
            "--security-opt",
            "label=disable",
            "--tmpfs",
            "/tmp:exec",
            "--user",
            f"{os.getuid()}:{os.getgid()}",
        ]
        if "podman" in Path(engine).name:
            command += ["--userns=keep-id"]
        command += [
            "-v",
            "/nix/store:/nix/store:ro",
            "-v",
            f"{self.work}:/work",
            "-v",
            f"{self.target}:/target",
            "-w",
            "/work/source",
        ]
        for key, value in container_env.items():
            command += ["-e", f"{key}={value}"]
        command += [
            image,
            "/bin/bash",
            "-euc",
            r"""
export PATH="$CDK_RELEASE_RUST/bin:/opt/rh/gcc-toolset-14/root/usr/bin:/usr/local/bin:/usr/bin:/bin"
export CC=gcc CXX=g++ AR=ar CXXFLAGS="$CFLAGS" ZERO_AR_DATE=1
for language in dart go; do
  cargo build --frozen --offline --profile "$CDK_PROFILE" --lib -p "cdk-ffi-$language" --target "$CDK_TARGET"
  file="/target/$CDK_TARGET/$CDK_PROFILE/libcdk_ffi_$language.so"
  strip --strip-all "$file"
  if readelf -d "$file" | grep /nix/store; then
    echo 'Nix runtime dependency in distributable library' >&2; exit 1
  fi
  /opt/python/cp310-cp310/bin/python -c 'import ctypes,sys; ctypes.CDLL(sys.argv[1])' "$file"
done
""",
        ]
        self.command(command)
        self.package_dart(
            target,
            self.target / target / self.config["profile"] / library("dart", target),
        )
        self.dart_consumer(self.work / f"dart-{target}")
        shutil.copyfile(
            self.target / target / self.config["profile"] / library("go", target),
            self.output / go_library(target),
        )

    def dart_consumer(self, project):
        shutil.copyfile(
            self.root / "bindings/release/dart-deps.lock", project / "pubspec.lock"
        )
        self.env["PUB_CACHE"] = str(self.work / "pub-cache")
        self.command(["dart", "pub", "get", "--enforce-lockfile"], cwd=project)
        self.command(["dart", "test", "test/wallet_test.dart"], cwd=project)

    def android(self):
        ndk = (
            Path(self.env["ANDROID_NDK_HOME"])
            / "toolchains/llvm/prebuilt/linux-x86_64/bin"
        )
        project = self.work / "kotlin"
        self.template("kotlin", project)
        flags = self.env["RUSTFLAGS"]
        cflags = self.env["CFLAGS"]
        self.env["RUSTFLAGS"] = (
            flags
            + " -C link-arg=-Wl,-z,max-page-size=16384 -C link-arg=-Wl,-z,common-page-size=16384"
        )
        for target, abi in ANDROID.items():
            prefix = target.replace(
                "armv7-linux-androideabi", "armv7a-linux-androideabi"
            )
            compiler = ndk / f"{prefix}{self.config['android']['api']}-clang"
            self.env[f"CARGO_TARGET_{target.upper().replace('-', '_')}_LINKER"] = str(
                compiler
            )
            self.env[f"CC_{target.replace('-', '_')}"] = str(compiler)
            self.env[f"AR_{target.replace('-', '_')}"] = str(ndk / "llvm-ar")
            self.env["CFLAGS"] = cflags
            for language in (
                ("kotlin", "dart") if target != "i686-linux-android" else ("kotlin",)
            ):
                native = self.cargo(language, target)
                self.command([ndk / "llvm-strip", "--strip-all", native])
                if target in ("aarch64-linux-android", "x86_64-linux-android"):
                    self.command(
                        [
                            "bash",
                            self.root
                            / ".github/scripts/check-android-elf-alignment.sh",
                            native,
                        ]
                    )
                if language == "dart":
                    self.package_dart(target, native)
                else:
                    destination = project / "cdk-android/src/main/jniLibs" / abi
                    destination.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(native, destination / native.name)
        self.env["RUSTFLAGS"] = flags
        self.gradle(project)
        archive(project, self.output / "kotlin.tar.gz", self.identity["epoch"])

    def gradle(self, project):
        repository = self.work / "maven"
        java_home = self.work / "java-home"
        java_home.mkdir()
        require(
            (project / "gradle/verification-metadata.xml").is_file(),
            "Missing Gradle dependency verification metadata",
        )
        self.command(
            [
                self.env["CDK_RELEASE_GRADLE"],
                f"-Duser.home={java_home}",
                "-Dorg.gradle.internal.http.connectionTimeout=10000",
                "-Dorg.gradle.internal.http.socketTimeout=60000",
                "--no-daemon",
                "--no-build-cache",
                "--no-configuration-cache",
                "--dependency-verification=strict",
                "--no-parallel",
                "-Pkotlin.compiler.execution.strategy=in-process",
                "-PcdkUnsigned=true",
                f"-PcdkReleaseRepository={repository}",
                ":cdk-android:publishReleasePublicationToCandidateRepository",
            ],
            cwd=project,
        )
        for name in expected("android", self.identity["version"]):
            if name.startswith("cdk-android-"):
                matches = list(repository.rglob(name))
                require(len(matches) == 1, f"Expected one Maven artifact {name}")
                shutil.copyfile(matches[0], self.output / name)
        for path in list(project.rglob("build")) + [project / ".gradle"]:
            if path.is_dir():
                shutil.rmtree(path)

    def windows(self):
        cache = self.work / "xwin"
        shutil.copytree(
            self.env["CDK_RELEASE_WINDOWS_SDK"], cache / "xwin", symlinks=True
        )
        for path in cache.rglob("*"):
            if not path.is_symlink():
                path.chmod(path.stat().st_mode | 0o200)
        self.env.update(
            {
                "XWIN_CACHE_DIR": str(cache),
                "XWIN_ARCH": "x86_64",
                "XWIN_VERSION": self.config["windows"]["visual_studio"],
                "XWIN_CRT_VERSION": self.config["windows"]["crt"],
                "XWIN_SDK_VERSION": self.config["windows"]["sdk"],
            }
        )
        # PDB identities include linker inputs outside Rust's path remapping.
        # These distributions do not ship PDBs, so omit their debug directory.
        self.env["RUSTFLAGS"] += " -C link-arg=/Brepro -C link-arg=/debug:none"
        for language in ("dart", "go"):
            native = self.cargo(language, WINDOWS)
            if language == "dart":
                self.package_dart(WINDOWS, native)
            else:
                shutil.copyfile(native, self.output / go_library(WINDOWS))

    def darwin(self):
        apple = self.config["apple"]
        # Nix's SDK hook may set DEVELOPER_DIR for its own compiler. Restore the
        # host selection captured before entering either Nix shell.
        self.env.pop("DEVELOPER_DIR", None)
        if self.env.get("CDK_RELEASE_XCODE"):
            self.env["DEVELOPER_DIR"] = self.env["CDK_RELEASE_XCODE"]
        require(
            self.command(["/usr/bin/xcodebuild", "-version"], capture=True)
            == f"Xcode {apple['xcode']}\nBuild version {apple['build']}",
            "Xcode version/build does not match release inputs",
        )
        for sdk, ver in apple["sdks"].items():
            require(
                self.command(
                    ["/usr/bin/xcrun", "--sdk", sdk, "--show-sdk-version"], capture=True
                )
                == ver,
                f"Unexpected {sdk} SDK",
            )
        for key in ("SDKROOT", "NIX_CFLAGS_COMPILE", "NIX_LDFLAGS"):
            self.env.pop(key, None)
        self.env.update(
            {
                "CC": "/usr/bin/clang",
                "CXX": "/usr/bin/clang++",
                "AR": "/usr/bin/ar",
                "MACOSX_DEPLOYMENT_TARGET": apple["macos_min"],
                "IPHONEOS_DEPLOYMENT_TARGET": apple["ios_min"],
                "CARGO_PROFILE_RELEASE_STRIP": "none",
                f"CARGO_PROFILE_{self.config['profile'].upper().replace('-', '_')}_STRIP": "none",
            }
        )
        # Use Apple's SDK discovery; Nix still provides Rust and all shared tools.
        apple_tools = self.work / "apple-tools"
        apple_tools.mkdir()
        (apple_tools / "xcrun").symlink_to("/usr/bin/xcrun")
        self.env["PATH"] = f"{apple_tools}:{self.env['PATH']}"
        base_flags = self.env["RUSTFLAGS"]
        base_cflags = self.env["CFLAGS"]
        for target in APPLE:
            self.env[f"CARGO_TARGET_{target.upper().replace('-', '_')}_LINKER"] = (
                "/usr/bin/clang"
            )
            sdk = (
                "macosx"
                if target.endswith("darwin")
                else "iphoneos"
                if target == "aarch64-apple-ios"
                else "iphonesimulator"
            )
            self.env["SDKROOT"] = self.command(
                ["/usr/bin/xcrun", "--sdk", sdk, "--show-sdk-path"], capture=True
            )
            self.env["CFLAGS"] = (
                base_cflags + f" -ffile-prefix-map={self.env['SDKROOT']}=/apple-sdk"
            )
            self.env["CXXFLAGS"] = self.env["CFLAGS"]
            for language in (
                ("dart", "go", "swift")
                if target.endswith("darwin")
                else ("dart", "swift")
            ):
                install_name = (
                    "@rpath/CashuDevKitFFI.framework/Versions/A/CashuDevKitFFI"
                    if language == "swift"
                    else f"@rpath/{library(language, target)}"
                )
                self.env["RUSTFLAGS"] = (
                    base_flags
                    + f" -C link-arg=-Wl,-S -C link-arg=-Wl,-install_name,{install_name}"
                )
                native = self.cargo(language, target)
                links = self.command(["/usr/bin/otool", "-L", native], capture=True)
                require(
                    "/nix/store" not in links,
                    "Nix runtime dependency in Darwin library",
                )
                if language == "dart":
                    self.package_dart(target, native)
                    if target == "aarch64-apple-darwin":
                        self.dart_consumer(self.work / f"dart-{target}")
                elif language == "go":
                    shutil.copyfile(native, self.output / go_library(target))
        self.swift_package()

    def swift_package(self):
        stage = self.work / "swift"
        self.template("swift", stage)
        sources = stage / "Sources/Cdk"
        sources.mkdir(parents=True, exist_ok=True)
        for path in self.generated.joinpath("swift").glob("*.swift"):
            shutil.copyfile(path, sources / path.name)
        xcroot = self.work / "xcframework"
        xc = xcroot / "CashuDevKitFFI.xcframework"
        slices = (
            ("ios-arm64", ["aarch64-apple-ios"], "ios", None),
            (
                "ios-arm64_x86_64-simulator",
                ["aarch64-apple-ios-sim", "x86_64-apple-ios"],
                "ios",
                "simulator",
            ),
            ("macos-arm64_x86_64", list(APPLE[:2]), "macos", None),
        )
        libraries = []
        for identifier, targets, osname, variant in slices:
            framework = xc / identifier / "CashuDevKitFFI.framework"
            content = framework / "Versions/A" if osname == "macos" else framework
            (content / "Headers").mkdir(parents=True)
            (content / "Modules").mkdir()
            binaries = [
                self.target
                / t
                / self.config["profile"]
                / (library("swift", t) if osname == "macos" else "libcdk_ffi_swift.a")
                for t in targets
            ]
            if len(binaries) == 1:
                shutil.copyfile(binaries[0], content / "CashuDevKitFFI")
            else:
                self.command(
                    [
                        "/usr/bin/lipo",
                        "-create",
                        *binaries,
                        "-output",
                        content / "CashuDevKitFFI",
                    ]
                )
            for header in self.generated.joinpath("swift").glob("*.h"):
                shutil.copyfile(header, content / "Headers" / header.name)
            headers = sorted(p.name for p in (content / "Headers").glob("*.h"))
            require(headers, "Missing Swift FFI headers")
            (content / "Modules/module.modulemap").write_text(
                "framework module CashuDevKitFFI {\n"
                + "".join(f'  header "{h}"\n' for h in headers)
                + "  export *\n}\n"
            )
            plist_name = (
                "Info-macOS.plist"
                if osname == "macos"
                else "Info-iOSSimulator.plist"
                if variant
                else "Info-iOS.plist"
            )
            plist = (
                (self.root / "bindings/swift/resources" / plist_name)
                .read_text()
                .replace(
                    "__CDK_SWIFT_FRAMEWORK_VERSION__",
                    self.identity["version"].split("-")[0],
                )
            )
            plist_data = plistlib.loads(plist.encode())
            minimum_key = (
                "LSMinimumSystemVersion" if osname == "macos" else "MinimumOSVersion"
            )
            plist_data[minimum_key] = self.config["apple"][
                "macos_min" if osname == "macos" else "ios_min"
            ]
            plist = plistlib.dumps(plist_data)
            if osname == "macos":
                (content / "Resources").mkdir()
                (content / "Resources/Info.plist").write_bytes(plist)
                (framework / "Versions/Current").symlink_to("A")
                for name in ("CashuDevKitFFI", "Headers", "Modules", "Resources"):
                    (framework / name).symlink_to(f"Versions/Current/{name}")
                self.command(
                    [
                        "/usr/bin/codesign",
                        "--force",
                        "--timestamp=none",
                        "--sign",
                        "-",
                        framework,
                    ]
                )
            else:
                (content / "Info.plist").write_bytes(plist)
            entry = {
                "LibraryIdentifier": identifier,
                "LibraryPath": "CashuDevKitFFI.framework",
                "SupportedArchitectures": ["arm64"]
                if len(targets) == 1
                else ["arm64", "x86_64"],
                "SupportedPlatform": osname,
            }
            if variant:
                entry["SupportedPlatformVariant"] = variant
            libraries.append(entry)
        (xc / "Info.plist").write_bytes(
            plistlib.dumps(
                {
                    "AvailableLibraries": libraries,
                    "CFBundlePackageType": "XFWK",
                    "XCFrameworkFormatVersion": "1.0",
                }
            )
        )
        archive(
            xcroot,
            self.output / "CashuDevKitFFI.xcframework.zip",
            self.identity["epoch"],
        )
        shutil.copyfile(
            self.output / "CashuDevKitFFI.xcframework.zip",
            stage / "CashuDevKitFFI.xcframework.zip",
        )
        swift_manifest = """// swift-tools-version: 5.9
import PackageDescription
let package = Package(
    name: "cdk-swift", platforms: [.iOS("@IOS@"), .macOS("@MACOS@")],
    products: [.library(name: "Cdk", targets: ["Cdk"])],
    targets: [
        .binaryTarget(name: "CashuDevKitFFI", path: "CashuDevKitFFI.xcframework.zip"),
        .target(name: "Cdk", dependencies: ["CashuDevKitFFI"], path: "Sources/Cdk")
    ]
)
"""
        (stage / "Package.swift").write_text(
            swift_manifest.replace("@IOS@", self.config["apple"]["ios_min"]).replace(
                "@MACOS@", self.config["apple"]["macos_min"]
            )
        )
        self.env["SDKROOT"] = self.command(
            ["/usr/bin/xcrun", "--sdk", "macosx", "--show-sdk-path"], capture=True
        )
        self.command(
            [
                "/usr/bin/xcrun",
                "swift",
                "build",
                "--package-path",
                stage,
                "--scratch-path",
                self.work / "swift-consumer",
                "--disable-sandbox",
            ]
        )
        consumer = self.work / "swift-smoke"
        (consumer / "Sources/Smoke").mkdir(parents=True)
        (consumer / "Package.swift").write_text("""// swift-tools-version: 5.9
import PackageDescription
let package = Package(
    name: "Smoke", platforms: [.macOS(.v13)],
    dependencies: [.package(name: "Candidate", path: "../swift")],
    targets: [.executableTarget(name: "Smoke", dependencies: [.product(name: "Cdk", package: "Candidate")])]
)
""")
        (consumer / "Sources/Smoke/Smoke.swift").write_text("""import Cdk
@main struct Smoke {
    static func main() async throws {
        let wallet = try Wallet(
            mintUrl: "https://mint.example.invalid", unit: .sat,
            mnemonic: try generateMnemonic(), store: .sqlite(path: ":memory:"),
            config: WalletConfig(targetProofCount: nil)
        )
        let balance = try await wallet.totalBalance()
        precondition(balance.value == 0)
    }
}
""")
        self.command(
            [
                "/usr/bin/xcrun",
                "swift",
                "run",
                "--package-path",
                consumer,
                "--scratch-path",
                self.work / "swift-smoke-build",
                "--disable-sandbox",
                "Smoke",
            ]
        )
        archive(stage, self.output / "swift.tar.gz", self.identity["epoch"])

    def packages(self):
        dart = self.work / "dart"
        self.template("dart", dart)
        for target in DART_TARGETS:
            unpacked = self.work / f"unpack-{target}"
            extract(self.candidate / dart_archive(target), unpacked)
            # Every platform must package exactly the same generated sources and
            # project files. Only its prebuilt directory may differ.
            require(
                tree_inventory(unpacked, ("prebuilt",))
                == tree_inventory(dart, ("prebuilt",)),
                f"Dart platform sources differ: {target}",
            )
            shutil.copytree(unpacked / "prebuilt" / target, dart / "prebuilt" / target)
        archive(dart, self.output / "dart.tar.gz", self.identity["epoch"])
        go = self.work / "go-package"
        self.template("go", go)
        package = go / "bindings/cdkffi"
        package.mkdir(parents=True, exist_ok=True)
        generated = self.generated / "go/cdkffi"
        for name in ("cdk_ffi.go", "cdk_ffi.h"):
            shutil.copyfile(generated / name, package / name)
        major = int(self.identity["version"].split(".")[0])
        module = (
            "github.com/"
            + self.config["destinations"]["go"]
            + (f"/v{major}" if major >= 2 else "")
        )
        (go / "go.mod").write_text(f"module {module}\n\ngo 1.22\n")
        for target in GO_TARGETS:
            arch = "amd64" if target.startswith("x86_64") else "arm64"
            osname = (
                "windows"
                if "windows" in target
                else "darwin"
                if "apple" in target
                else "linux"
            )
            native = package / "native" / f"{osname}_{arch}"
            native.mkdir(parents=True)
            shutil.copyfile(
                self.candidate / go_library(target), native / library("go", target)
            )
            flags = f"-L${{SRCDIR}}/native/{osname}_{arch} -lcdk_ffi_go"
            if osname != "windows":
                flags += f" -Wl,-rpath,${{SRCDIR}}/native/{osname}_{arch} -lm"
                if osname == "linux":
                    flags += " -ldl"
            (package / f"link_{osname}_{arch}.go").write_text(
                f'//go:build {osname} && {arch}\n\npackage cdk_ffi\n\n// #cgo LDFLAGS: {flags}\nimport "C"\n'
            )
        self.command(["go", "build", "./bindings/cdkffi"], cwd=go)
        # Exercise the packaged native library through generated Go methods.
        test = package / "candidate_test.go"
        shutil.copyfile(self.root / "bindings/go/cdk_test.go", test)
        self.command(
            [
                "go",
                "test",
                "./bindings/cdkffi",
                "-run",
                "Test(InitialBalanceIsZero|InMemorySqliteConcurrentAccess|RateLimit)",
                "-count=1",
            ],
            cwd=go,
        )
        test.unlink()
        archive(go, self.output / "go.tar.gz", self.identity["epoch"])

    def execute(self):
        if self.group == "generated":
            self.generated_sources()
        elif self.group.startswith("linux-"):
            self.linux()
        else:
            getattr(self, self.group)()
        names = {path.name for path in self.output.iterdir()}
        require(
            names == expected(self.group, self.identity["version"]),
            f"Unexpected {self.group} outputs: {names}",
        )
        record = {
            "schema": 1,
            "group": self.group,
            "source": self.identity,
            "files": inventory(self.output, names),
            "imports": inventory(self.candidate, dependencies(self.group))
            if self.candidate
            else {},
        }
        write_json(self.output / f"group-{self.group}.json", record)
        shutil.rmtree(self.target)
