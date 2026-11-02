"""Build and run a real downstream Go executable against final packaged library bytes."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import uuid


def go_consumer(project, library, target, output, runner=None, runner_args=()):
    from sbom import copy_project, digest, go_environment, run

    environment = go_environment(target)
    environment.update(GOWORK="off", GOTOOLCHAIN="local")
    for name in ("LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT", "DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH"):
        environment.pop(name, None)
    if environment.get("CGO_LDFLAGS"):
        raise ValueError("Final Go consumer tests require no ambient CGO_LDFLAGS override")
    host = run(["go", "env", "GOHOSTOS", "GOHOSTARCH"], project, environment).splitlines()
    if host != [environment["GOOS"], environment["GOARCH"]] and not runner:
        raise ValueError("Cross-target Go consumer execution requires an explicit emulator/runner")
    with tempfile.TemporaryDirectory(prefix=".consumer-", dir=output) as temporary:
        root = copy_project(project, Path(temporary) / "project")
        native = root / "bindings/cdkffi/native" / (environment["GOOS"] + "_" + environment["GOARCH"]) / library.name
        if not native.is_file():
            raise ValueError("Assembled Go package has no matching target-native slot")
        shutil.copyfile(library, native)
        module = json.loads(run(["go", "list", "-m", "-json"], root, environment))["Path"]
        directory = root / ("sbom_consumer_" + uuid.uuid4().hex)
        directory.mkdir()
        source = 'package main\nimport cdk ' + json.dumps(module + "/bindings/cdkffi") + '\nfunc main() {\n mnemonic, err := cdk.GenerateMnemonic()\n if err != nil || mnemonic == "" { panic("native consumer call failed") }\n}\n'
        (directory / "main.go").write_text(source)
        executable = Path(temporary) / "consumer"
        run(["go", "build", "-mod=readonly", "-o", executable, "./" + directory.name], root, environment)
        command = ([str(runner), *runner_args] if runner else []) + [str(executable)]
        subprocess.run(command, cwd=root, env=environment, check=True, timeout=120,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        return {"status": "passed", "target": target, "native_sha256": digest(native),
                "consumer_sha256": digest(executable), "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
                "go_version": run(["go", "version"], root, environment).strip(),
                "runner": str(runner) if runner else "native-host", "runner_arguments": list(runner_args),
                 "loader_overrides": "cleared", "test": "standalone downstream executable; offline mnemonic native call"}


def swift_consumer(project, output):
    from sbom import copy_project, digest, run

    if sys.platform != "darwin":
        raise ValueError("Swift package execution requires a real macOS host")
    environment = {key: value for key, value in os.environ.items() if not key.startswith(("DYLD_", "LD_"))}
    with tempfile.TemporaryDirectory(prefix=".swift-consumer-", dir=output) as temporary:
        root = copy_project(project, Path(temporary) / "package")
        graph = json.loads(run(["swift", "package", "show-dependencies", "--format", "json"], root, environment))
        # ponytail: this release has no external Swift packages; inventory any future additions before accepting them.
        if graph.get("dependencies") != []:
            raise ValueError("External Swift dependencies require a resolved Swift inventory")
        consumer = Path(temporary) / "consumer"
        sources = consumer / "Sources/Smoke"
        sources.mkdir(parents=True)
        (consumer / "Package.swift").write_text('''// swift-tools-version: 5.9
import PackageDescription
let package = Package(name: "Smoke", platforms: [.macOS(.v13)],
    dependencies: [.package(name: "cdk-swift", path: "../package")],
    targets: [.executableTarget(name: "Smoke", dependencies: [.product(name: "Cdk", package: "cdk-swift")])])
''')
        source = "import Cdk\nlet mnemonic = try generateMnemonic()\nprecondition(!mnemonic.isEmpty)\n"
        (sources / "main.swift").write_text(source)
        run(["swift", "build", "--package-path", consumer, "-c", "release"], root, environment)
        binary = Path(run(["swift", "build", "--package-path", consumer, "-c", "release", "--show-bin-path"], root, environment).strip()) / "Smoke"
        subprocess.run([binary], cwd=root, env=environment, check=True, timeout=120)
        return {"status": "passed", "scope": "native-host macOS only; no iOS execution claimed",
                "archive_sha256": digest(root / "CashuDevKitFFI.xcframework.zip"), "consumer_sha256": digest(binary),
                "source_sha256": hashlib.sha256(source.encode()).hexdigest(),
                "swift_version": run(["swift", "--version"], root, environment).strip(), "loader_overrides": "cleared"}
