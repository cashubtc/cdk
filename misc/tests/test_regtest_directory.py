"""Launcher tests without Bitcoin, Lightning, cargo builds, or a terminal.

Run with: python3 -m unittest discover -s misc/tests -p 'test_regtest_directory.py'
"""

import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import tempfile
import unittest


class RegtestDirectoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="cdk-regtest-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.state = self.root / "environment"
        source = Path(__file__).parents[1] / "interactive_regtest_mprocs.sh"
        self.script = self.root / "launcher.sh"
        # Isolate the global helper state so tests cannot affect a real regtest.
        self.script.write_text(source.read_text().replace(
            "/tmp/cdk_regtest_env", str(self.state)))
        self.command("mprocs", "exit 0\n")
        self.command("cargo", """
if [ "${FAIL_BUILD:-}" = 1 ]; then exit 1; fi
if [ "$1" = run ]; then
    exec python3 -c '
import os, signal, time
signal.signal(signal.SIGINT, lambda *_: exit(0))
with open(os.environ["CDK_ITESTS_DIR"] + "/progress_pipe", "w") as pipe:
    pipe.write("checkpoint1\\n")
time.sleep(60)
'
fi
""")
        self.command("sleep", f'exec "{shutil.which("sleep")}" 0.01\n')
        self.env = {
            **os.environ,
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "TMPDIR": str(self.root),
        }

    def command(self, name, body):
        path = self.bin / name
        path.write_text("#!/usr/bin/env bash\nset -e\n" + body)
        path.chmod(0o755)

    def launch(self, *args, **env):
        return subprocess.run(
            ["bash", str(self.script), "sqlite", "127.0.0.1", *args],
            cwd=self.root, env={**self.env, **env}, capture_output=True,
            text=True, timeout=15,
        )

    def test_persistent_directory_survives_repeated_runs(self):
        data = self.root / "persistent data"
        data.mkdir()
        sentinel = data / "wallet-data"
        sentinel.write_text("keep me")
        for _ in range(2):
            # Simulate synchronization files left behind by an interrupted run.
            os.mkfifo(data / "progress_pipe")
            (data / "signal_received").touch()
            result = self.launch(str(data))
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(sentinel.read_text(), "keep me")
            self.assertTrue((data / "cln_mint/config.toml").is_file())
            commands = [json.loads(line.strip().removeprefix("shell: "))
                        for line in (data / "mprocs.yaml").read_text().splitlines()
                        if line.strip().startswith("shell: ")]
            self.assertEqual(shlex.split(commands[0]),
                             [str(data / "start_cln_mint.sh")])
            self.assertFalse((data / "progress_pipe").exists())
            self.assertFalse((data / "signal_received").exists())
            self.assertFalse(self.state.exists())

    def test_relative_directory_is_created_and_resolved(self):
        result = self.launch("new environment")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        data = self.root / "new environment"
        self.assertTrue(data.is_dir())
        self.assertIn(f"Data directory: {data}", result.stdout)
        self.assertIn(str(data / "cln_mint"),
                      (data / "start_cln_mint.sh").read_text())

    def test_temporary_directory_is_removed(self):
        result = self.launch()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        path = next(line.removeprefix("Data directory: ")
                    for line in result.stdout.splitlines()
                    if line.startswith("Data directory: "))
        self.assertFalse(Path(path).exists())

    def test_build_failure_preserves_persistent_data(self):
        data = self.root / "persistent"
        data.mkdir()
        sentinel = data / "wallet-data"
        sentinel.write_text("keep me")
        result = self.launch(str(data), FAIL_BUILD="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(sentinel.read_text(), "keep me")

    def test_setup_failure_is_reported_without_waiting_for_timeout(self):
        self.command("cargo", 'if [ "$1" = run ]; then exit 37; fi\n')
        result = self.launch("persistent")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("exited during startup", result.stdout)
        self.assertNotIn("Timeout waiting", result.stdout)
        self.assertTrue((self.root / "persistent").is_dir())

    def test_running_environment_is_not_disturbed(self):
        state = f'export CDK_REGTEST_PID="{os.getpid()}"\n'
        self.state.write_text(state)
        result = self.launch("another-environment")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already running", result.stdout)
        self.assertEqual(self.state.read_text(), state)
        self.assertFalse((self.root / "another-environment").exists())


if __name__ == "__main__":
    unittest.main()
