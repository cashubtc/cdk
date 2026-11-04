#!/usr/bin/env python3
"""Install a downstream AAR consumer only on an explicitly selected/owned emulator."""

import argparse
import hashlib
import os
from pathlib import Path
import shutil
import socket
import subprocess
import time
import uuid
import zipfile

from sbom import ROOT, digest, write_json


def command(arguments, environment, log, cwd=None, stdin=None, timeout=300):
    result = subprocess.run(list(map(str, arguments)), cwd=cwd, env=environment, input=stdin,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=timeout, check=False)
    with log.open("a") as stream:
        stream.write(result.stdout)
    if result.returncode:
        raise ValueError(f"Android consumer command failed; see {log}")
    return result.stdout


def verify_apk_library(aar, apk):
    with zipfile.ZipFile(aar) as source, zipfile.ZipFile(apk) as application:
        with source.open("jni/x86_64/libcdk_ffi_kotlin.so") as stream:
            expected = hashlib.file_digest(stream, "sha256").hexdigest()
        with application.open("lib/x86_64/libcdk_ffi_kotlin.so") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if actual != expected:
            raise ValueError("Consumer APK native bytes differ from the final AAR")
        return actual


def test(args):
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"status": "incomplete", "abi": "x86_64", "api": "34", "aar_sha256": digest(args.aar)}
    environment = dict(os.environ, ANDROID_HOME=str(args.sdk), ANDROID_SDK_ROOT=str(args.sdk),
                       ANDROID_USER_HOME=str(output / "android-home"),
                       ANDROID_EMULATOR_HOME=str(output / "android-home"),
                       ANDROID_AVD_HOME=str(output / "android-home/avd"))
    emulator = None
    log = output / "consumer.log"
    emulator_log = None
    try:
        if not args.serial and not args.start_emulator:
            raise ValueError("Select --serial emulator-N or explicitly request --start-emulator")
        adb = args.sdk / "platform-tools/adb"
        if args.start_emulator:
            if args.serial or args.port % 2 or not 5554 <= args.port <= 5682:
                raise ValueError("Managed emulator needs its own even console port in 5554..5682")
            for port in (args.port, args.port + 1):
                with socket.socket() as probe:
                    probe.bind(("127.0.0.1", port))
            manager = args.sdk / "cmdline-tools/latest/bin/avdmanager"
            if not manager.is_file():
                choices = sorted(args.sdk.glob("cmdline-tools/*/bin/avdmanager"))
                if not choices:
                    raise ValueError("No SDK avdmanager; use the pinned sbom-android shell")
                manager = choices[-1]
            name = "cdk-sbom-" + uuid.uuid4().hex
            image = "system-images;android-34;google_apis;x86_64"
            Path(environment["ANDROID_AVD_HOME"]).mkdir(parents=True)
            command([manager, "create", "avd", "--name", name, "--package", image, "--device", "pixel_5"],
                    environment, log, stdin="no\n")
            if not (Path(environment["ANDROID_AVD_HOME"]) / (name + ".ini")).is_file():
                raise ValueError("AVD registration is missing from the private emulator directory")
            emulator_log = (output / "emulator.log").open("w")
            emulator = subprocess.Popen([str(args.sdk / "emulator/emulator"), "-avd", name, "-port", str(args.port),
                                         "-no-window", "-no-audio", "-no-boot-anim", "-no-snapshot", "-gpu", "swiftshader_indirect"],
                                        env=environment, stdout=emulator_log, stderr=subprocess.STDOUT)
            serial = "emulator-" + str(args.port)
        else:
            serial = args.serial
            if not serial.startswith("emulator-"):
                raise ValueError("This smoke test never installs applications on physical devices")
        project = output / "project"
        shutil.copytree(ROOT / "misc/sbom/android-consumer", project)
        libraries = project / "app/libs"
        libraries.mkdir()
        shutil.copyfile(args.aar, libraries / "cdk.aar")
        command(["bash", args.gradle_wrapper.resolve(), "-p", project, "--no-daemon", ":app:assembleDebug", ":app:assembleDebugAndroidTest"],
                environment, log, timeout=600)
        deadline = time.monotonic() + 240
        while True:
            if emulator and emulator.poll() is not None:
                raise ValueError("Managed Android emulator exited; see emulator.log")
            try:
                command([adb, "-s", serial, "wait-for-device"], environment, log, timeout=5)
                break
            except subprocess.TimeoutExpired:
                if time.monotonic() > deadline:
                    raise ValueError("Android emulator did not become available")
        deadline = time.monotonic() + 240
        while command([adb, "-s", serial, "shell", "getprop", "sys.boot_completed"], environment, log, timeout=20).strip() != "1":
            if time.monotonic() > deadline or emulator and emulator.poll() is not None:
                raise ValueError("Managed Android emulator did not finish booting")
            time.sleep(3)
        app = project / "app/build/outputs/apk/debug/app-debug.apk"
        tests = project / "app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk"
        native_checksum = verify_apk_library(args.aar, app)
        abi = command([adb, "-s", serial, "shell", "getprop", "ro.product.cpu.abi"], environment, log).strip()
        api = command([adb, "-s", serial, "shell", "getprop", "ro.build.version.sdk"], environment, log).strip()
        if (abi, api) != ("x86_64", "34"):
            raise ValueError("Android consumer requires an x86_64 API 34 emulator")
        for apk in (app, tests):
            command([adb, "-s", serial, "install", "-r", apk], environment, log)
        text = command([adb, "-s", serial, "shell", "am", "instrument", "-w", "-r",
                        "org.cashudevkit.sbomconsumer.test/androidx.test.runner.AndroidJUnitRunner"], environment, log)
        if "OK (1 test)" not in text or "FAILURES" in text or "INSTRUMENTATION_FAILED" in text:
            raise ValueError("Android native-call instrumentation did not pass")
        if digest(args.aar) != result["aar_sha256"]:
            raise ValueError("Consumer AAR changed during the test")
        result.update(status="passed", app_sha256=digest(app), test_apk_sha256=digest(tests), serial=serial,
                      packaged_native_sha256=native_checksum,
                      build_fingerprint=command([adb, "-s", serial, "shell", "getprop", "ro.build.fingerprint"], environment, log).strip(),
                      scope="one downstream x86_64 Android API 34 consumer, not all device/ABI combinations")
    except (ValueError, OSError, KeyError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        result["error"] = str(error)
        raise
    finally:
        if emulator:
            emulator.terminate()
            try:
                emulator.wait(timeout=20)
            except subprocess.TimeoutExpired:
                emulator.kill()
                emulator.wait()
        if emulator_log:
            emulator_log.close()
        write_json(output / "consumer-result.json", result)
    print(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aar", required=True, type=Path)
    parser.add_argument("--sdk", type=Path, default=Path(os.environ.get("ANDROID_HOME", "/nonexistent-sdk")))
    parser.add_argument("--gradle-wrapper", type=Path, default=ROOT / "bindings/kotlin/gradlew")
    parser.add_argument("--serial")
    parser.add_argument("--start-emulator", action="store_true")
    parser.add_argument("--port", type=int, default=5560)
    parser.add_argument("--output", required=True, type=Path)
    try:
        test(parser.parse_args())
        return 0
    except (ValueError, OSError, KeyError, zipfile.BadZipFile, subprocess.SubprocessError) as error:
        print(f"Android consumer test incomplete: {error}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
