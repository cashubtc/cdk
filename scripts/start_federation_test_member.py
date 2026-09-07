#!/usr/bin/env python3
"""Import a generated regtest member configuration and replace this process with mintd."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib


def set_value(document, section, key, value):
    """Change one test-fixture setting without rewriting secret references."""
    header = re.search(rf"(?m)^\[{re.escape(section)}\]\s*$", document)
    if header is None:
        return document.rstrip() + f"\n\n[{section}]\n{key} = {json.dumps(value)}\n"
    start = header.end()
    following = re.search(r"(?m)^\[", document[start:])
    end = start + following.start() if following else len(document)
    body = document[start:end]
    pattern = rf"(?m)^{re.escape(key)}\s*=.*$"
    assignment = f"{key} = {json.dumps(value)}"
    if re.search(pattern, body):
        body = re.sub(pattern, lambda _: assignment, body)
    else:
        body = "\n" + assignment + body
    return document[:start] + body + document[end:]


def main():
    if len(sys.argv) != 3:
        raise ValueError("usage: start_federation_test_member.py CONFIG WORK_DIR")
    config_path, work_dir = map(Path, sys.argv[1:])
    document = config_path.read_text()
    # These are fixture controls, explicitly imported into the authoritative
    # document. Production mintd startup does not read runtime overrides.
    overrides = (
        ("CDK_MINTD_FAKE_WALLET_MIN_DELAY", "fake_wallet", "min_delay_time", int),
        ("CDK_MINTD_FAKE_WALLET_MAX_DELAY", "fake_wallet", "max_delay_time", int),
        ("CDK_MINTD_FEDERATION_PUBLIC_CONFIG_PATH", "federation", "public_config_path", str),
        ("CDK_MINTD_FEDERATION_PRIVATE_CONFIG_PATH", "federation", "private_config_path", str),
        ("CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS", "federation.peer_sync", "operation_submission_timeout_secs", int),
    )
    for variable, section, key, convert in overrides:
        if variable in os.environ:
            document = set_value(document, section, key, convert(os.environ[variable]))

    database = tomllib.loads(document)["database"]
    environment = os.environ.copy()
    environment["CDK_MINTD_DATABASE"] = database["engine"]
    if database["engine"] == "postgres":
        postgres = database["postgres"]
        reference = postgres["url"]
        if reference.startswith("file:"):
            url = Path(reference[5:]).read_text().strip()
        elif reference.startswith("env:"):
            url = environment[reference[4:]]
        else:
            raise ValueError("generated PostgreSQL URL must be a secret reference")
        environment["CDK_MINTD_POSTGRES_URL"] = url
        for key in ("tls_mode", "max_connections", "connection_timeout_seconds"):
            if key in postgres:
                environment[f"CDK_MINTD_POSTGRES_{key.upper()}"] = str(postgres[key])

    work_dir.mkdir(parents=True, exist_ok=True)
    imported = work_dir / "runtime-import.toml"
    descriptor = os.open(imported, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(descriptor, "w") as output:
        output.write(document)
    mintd = environment.get("CDK_MINTD_BIN", "target/debug/cdk-mintd")
    command = [mintd, "--work-dir", str(work_dir)]
    initialized = subprocess.run(
        command + ["config", "show"], env=environment,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=False,
    ).returncode == 0
    operation = ["apply"] if initialized else ["init", "--new-mint"]
    subprocess.run(command + ["config", *operation, "--file", str(imported)], env=environment, check=True)
    os.execvpe(mintd, command, environment)


if __name__ == "__main__":
    main()
