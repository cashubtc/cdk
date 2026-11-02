"""Build and run a real downstream Go executable against final packaged library bytes."""

import hashlib
import json
from pathlib import Path
import shutil
import subprocess
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
