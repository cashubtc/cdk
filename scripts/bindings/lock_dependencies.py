"""Refresh reviewed language dependency locks using disposable projects."""

import os
from pathlib import Path
import shutil
import sys

from model import require, run, scratch


def main(root):
    require(
        os.environ.get("CDK_RELEASE_ENV") == "1", "Use lock-dependencies through Nix"
    )
    work = scratch()
    print(f"Dependency maintenance workspace: {work}", flush=True)
    project = work / "kotlin"
    shutil.copytree(
        root / "bindings/kotlin",
        project,
        ignore=shutil.ignore_patterns("rust", "build", ".gradle", "local.properties"),
    )
    for module in ("cdk-jvm", "cdk-android"):
        source = project / module / "src"
        if source.exists():
            shutil.rmtree(source)
        source = source / "main/kotlin/org/cashudevkit"
        source.mkdir(parents=True)
        (source / "LockInputs.kt").write_text(
            "package org.cashudevkit\nclass LockInputs\n"
        )
    tests = project / "cdk-jvm/src/test/kotlin/org/cashudevkit"
    tests.mkdir(parents=True)
    (tests / "LockInputsTest.kt").write_text(
        "package org.cashudevkit\nclass LockInputsTest\n"
    )
    # Android compiles the shared cdk-jvm source set, so avoid duplicate classes.
    shutil.rmtree(project / "cdk-android/src/main/kotlin")
    env = dict(os.environ)
    env["GRADLE_USER_HOME"] = str(work / "gradle-home")
    env["ANDROID_USER_HOME"] = str(work / "android-home")
    env["DASH__SUPPRESS_ANALYTICS"] = "true"
    java_home = work / "java-home"
    java_home.mkdir()
    run(
        [
            env["CDK_RELEASE_GRADLE"],
            f"-Duser.home={java_home}",
            "-Dorg.gradle.internal.http.connectionTimeout=10000",
            "-Dorg.gradle.internal.http.socketTimeout=60000",
            "--no-daemon",
            "--no-parallel",
            "-Pkotlin.compiler.execution.strategy=in-process",
            "--write-locks",
            "--write-verification-metadata",
            "sha256",
            "-PcdkUnsigned=true",
            f"-PcdkReleaseRepository={work / 'maven'}",
            ":cdk-jvm:test",
            ":cdk-android:publishReleasePublicationToCandidateRepository",
        ],
        cwd=project,
        env=env,
    )
    for relative in (
        "gradle/verification-metadata.xml",
        "cdk-jvm/gradle.lockfile",
        "cdk-android/gradle.lockfile",
    ):
        shutil.copyfile(project / relative, root / "bindings/kotlin" / relative)
    dart = work / "dart"
    dart.mkdir()
    shutil.copyfile(root / "bindings/dart/pubspec.yaml", dart / "pubspec.yaml")
    env["PUB_CACHE"] = str(work / "pub-cache")
    run(["dart", "pub", "get"], cwd=dart, env=env)
    shutil.copyfile(dart / "pubspec.lock", root / "bindings/release/dart-deps.lock")
    shutil.rmtree(work)
    print(
        "Review the Gradle locks, verification metadata, and Dart dependency lock before committing."
    )


if __name__ == "__main__":
    main(Path(sys.argv[1]).resolve())
