#!/usr/bin/env python3
"""Black-box NIP-87 test using mintd, nostr-rs-relay and the independent nak client.

Build mintd with sqlite,fakewallet,mint-discovery first. All services bind only
loopback, use disposable databases, and are stopped even when an assertion fails.
No Python packages or additional Cargo dependencies are required.
"""

import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import time
import urllib.error
import urllib.request

# Public test vectors, used only in this disposable fake-payment mint.
TEST_MNEMONIC = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
TEST_NOSTR_KEY = "0" * 63 + "1"
TEST_NOSTR_PUBKEY = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_for(check, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}")


class TestRun:
    def __init__(self, mintd, output):
        self.mintd = str(mintd.resolve())
        self.output = output.resolve()
        self.output.mkdir(parents=True, exist_ok=False)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith("CDK_MINTD_")}
        self.env.update(E2E_MINT_MNEMONIC=TEST_MNEMONIC, E2E_NOSTR_KEY=TEST_NOSTR_KEY)
        self.mint_port, self.relay_port = free_port(), free_port()
        while self.mint_port == self.relay_port:
            self.relay_port = free_port()
        self.mint_url = f"http://127.0.0.1:{self.mint_port}"
        self.relay_url = f"ws://127.0.0.1:{self.relay_port}"
        self.children = []
        self.logs = []
        self.results = []
        self.work = self.output / "mint"
        self.work.mkdir()
        (self.output / "relay").mkdir()
        self.relay_config = self.output / "relay.toml"
        self.relay_config.write_text(f'[network]\naddress = "127.0.0.1"\nport = {self.relay_port}\n')
        self.document = self.output / "mint.toml"

    def run(self, args, **kwargs):
        result = subprocess.run(args, env=self.env, cwd=self.output, capture_output=True,
                                text=True, timeout=15, **kwargs)
        if result.returncode:
            raise AssertionError(f"Command {Path(args[0]).name} failed: {result.stderr}")
        return result.stdout

    def spawn(self, name, args):
        path = self.output / f"{name}.log"
        log = path.open("w")
        self.logs.append(log)
        process = subprocess.Popen(args, env=self.env, cwd=self.output, stdout=log, stderr=log)
        self.children.append(process)
        return process, path

    def stop(self, process):
        started = time.monotonic()
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
            process.wait(timeout=8)
        assert process.returncode == 0, f"Process exited with {process.returncode}"
        return time.monotonic() - started

    def cleanup(self):
        for process in reversed(self.children):
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
        for log in self.logs:
            log.close()

    def start_relay(self, name):
        process, _ = self.spawn(name, [shutil.which("nostr-rs-relay"), "--config",
                                     str(self.relay_config), "--db", str(self.output / "relay")])

        def ready():
            assert process.poll() is None, "relay exited during startup (see log)"
            try:
                with socket.create_connection(("127.0.0.1", self.relay_port), timeout=0.5):
                    return True
            except OSError:
                return False

        wait_for(ready, "relay startup")
        return process

    def configure(self, name, initialize=False, enabled=True):
        document = f'''[info]
url = "{self.mint_url}"
listen_host = "127.0.0.1"
listen_port = {self.mint_port}
mnemonic = "env:E2E_MINT_MNEMONIC"
[info.logging]
output = "stderr"
console_level = "info"
[mint_info]
name = "{name}"
description = "NIP-87 end-to-end test"
icon_url = "https://example.invalid/mint.png"
[payment_backend]
backend = "fakewallet"
[fake_wallet]
[database]
engine = "sqlite"
'''
        if enabled:
            document += f'''[mint_discovery]
secret_key = "env:E2E_NOSTR_KEY"
relays = ["{self.relay_url}"]
network = "regtest"
'''
        self.document.write_text(document)
        args = [self.mintd, "--work-dir", str(self.work), "config"]
        args += ["init", "--new-mint"] if initialize else ["apply"]
        self.run(args + ["--file", str(self.document)])
        stored = self.run([self.mintd, "--work-dir", str(self.work), "config", "show"])
        assert TEST_NOSTR_KEY not in stored and TEST_MNEMONIC not in stored

    def info(self):
        try:
            with urllib.request.urlopen(self.mint_url + "/v1/info", timeout=1) as response:
                return json.load(response)
        except (urllib.error.URLError, TimeoutError):
            return None

    def start_mint(self, name):
        process, log = self.spawn(name, [self.mintd, "--work-dir", str(self.work)])

        def ready():
            assert process.poll() is None, "mint exited during startup (see log)"
            return self.info()

        info = wait_for(ready, "mint HTTP readiness")
        return process, log, info

    def events(self, pubkey=None):
        args = [shutil.which("nak"), "req", "-k", "38172", "-a", TEST_NOSTR_PUBKEY]
        if pubkey:
            args += ["-d", pubkey]
        output = self.run(args + [self.relay_url])
        return [json.loads(line) for line in output.splitlines() if line.strip()]

    def announcement(self, info, name, timeout=15):
        def matching():
            events = self.events(info["pubkey"])
            if len(events) == 1 and json.loads(events[0]["content"]).get("name") == name:
                return events[0]
            return None

        event = wait_for(matching, f"announcement {name}", timeout)
        self.run([shutil.which("nak"), "verify"], input=json.dumps(event))
        assert event["kind"] == 38172 and event["pubkey"] == TEST_NOSTR_PUBKEY
        tags = {tag[0]: tag[1:] for tag in event["tags"]}
        assert tags["d"] == [info["pubkey"]]
        assert tags["u"] == [self.mint_url]
        assert tags["n"] == ["regtest"]
        supported = {1, 2, 3, 6}
        for number, settings in info["nuts"].items():
            if number in ("4", "5"):
                enabled = bool(settings.get("methods")) and not settings.get("disabled", False)
            elif number == "15":
                enabled = bool(settings)
            elif number == "19":
                enabled = bool(settings.get("cached_endpoints"))
            elif number in ("21", "22", "29"):
                enabled = bool(settings)
            else:
                enabled = bool(settings.get("supported"))
            if enabled:
                supported.add(int(number))
        assert tags["nuts"] == [",".join(map(str, sorted(supported)))]
        metadata = json.loads(event["content"])
        assert metadata["name"] == info["name"] == name
        assert metadata["about"] == info.get("description_long", info["description"])
        assert metadata["picture"] == info["icon_url"]
        assert metadata["website"] == self.mint_url
        (self.output / f"{name}-event.json").write_text(json.dumps(event, indent=2) + "\n")
        (self.output / f"{name}-info.json").write_text(json.dumps(info, indent=2) + "\n")
        return event

    def passed(self, name):
        self.results.append(name)
        print(f"PASS: {name}", flush=True)

    def test(self):
        relay = self.start_relay("relay-initial")
        self.configure("initial", initialize=True)
        assert not self.events(), "config init must not publish"
        mint, log, info = self.start_mint("mint-initial")
        first = self.announcement(info, "initial")
        wait_for(lambda: "acknowledged by all relays" in log.read_text(), "relay ACK")
        self.passed("CLI initialization, HTTP metadata, relay storage and independent signature verification")
        self.stop(mint)

        # Deliberately restart without a timestamp sleep: mintd must avoid ties.
        self.configure("updated")
        mint, log, updated_info = self.start_mint("mint-updated")
        updated = self.announcement(updated_info, "updated")
        assert updated["id"] != first["id"] and updated["created_at"] > first["created_at"]
        assert updated_info["pubkey"] == info["pubkey"]
        assert len(self.events()) == 1, "relay must retain only the current replaceable event"
        self.passed("config apply and restart replace the announcement with a stable identity")
        self.stop(mint)
        self.stop(relay)

        self.configure("recovered")
        mint, log, recovered_info = self.start_mint("mint-outage")
        wait_for(lambda: "Retrying mint discovery" in log.read_text(), "failed relay attempt", 40)
        assert self.info()["name"] == "recovered", "relay outage must not block HTTP"
        relay = self.start_relay("relay-recovered")
        recovered = self.announcement(recovered_info, "recovered", 90)
        assert recovered["created_at"] > updated["created_at"]
        wait_for(lambda: "acknowledged by all relays" in log.read_text(), "recovered ACK")
        self.passed("real relay outage does not block mint HTTP; the one-minute retry recovers")
        self.stop(mint)
        self.stop(relay)

        mint, log, _ = self.start_mint("mint-cancel")
        wait_for(lambda: "Retrying mint discovery" in log.read_text(), "retry backoff", 40)
        elapsed = self.stop(mint)
        assert elapsed < 5, f"shutdown took {elapsed:.1f}s during retry backoff"
        self.passed("SIGINT cancels publication during backoff and exits promptly")

        relay = self.start_relay("relay-disabled")
        self.configure("disabled", enabled=False)
        mint, log, _ = self.start_mint("mint-disabled")
        assert self.info()["name"] == "disabled"
        time.sleep(1)
        assert [event["id"] for event in self.events()] == [recovered["id"]]
        assert "Mint discovery" not in log.read_text()
        self.passed("removing discovery configuration prevents publication without deleting old events")
        self.stop(mint)
        self.stop(relay)
        summary = {"status": "passed", "checks": self.results,
                   "relay_version": self.run([shutil.which("nostr-rs-relay"), "--version"]).strip(),
                   "client_version": self.run([shutil.which("nak"), "--version"]).strip()}
        (self.output / "result.json").write_text(json.dumps(summary, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mintd", type=Path, default=Path("target/debug/cdk-mintd"))
    parser.add_argument("--output-dir", type=Path, required=True, help="new directory for isolated databases and logs")
    args = parser.parse_args()
    for tool in ("nostr-rs-relay", "nak"):
        if not shutil.which(tool):
            parser.error(f"{tool} must be installed and on PATH")
    if not args.mintd.is_file():
        parser.error("build mintd with sqlite,fakewallet,mint-discovery first")
    run = TestRun(args.mintd, args.output_dir)
    try:
        run.test()
    finally:
        run.cleanup()


if __name__ == "__main__":
    main()
