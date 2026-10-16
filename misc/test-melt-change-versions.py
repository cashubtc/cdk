#!/usr/bin/env python3
"""Run the wallet change regression against pinned, independent mint releases."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import socket
import subprocess
import tempfile
import time
import urllib.request

DIGESTS = {
    'aarch64-apple-darwin': {
        '0.17.7': '39aa38db625d99217ddb40cb905a5f0d4e162bc3fc6406cf51b34a25b41f433d',
        '0.18.1': '5f44b2215ccbada9051296a973d4577e755ea300831c31a08e6025348c7f4b67',
    },
    'x86_64': {
        '0.17.7': 'af2e186081c28a60114b716ed252e01fc0659477fdf6c9c2b7cd3d4a8b287f5d',
        '0.18.1': '52502f148e937aef5fb9d457cf696e12c826f905f228db10b9a9a7720823f488',
    },
}


def run_matrix(binary_dir, offline):
    target = {('Darwin', 'arm64'): 'aarch64-apple-darwin',
              ('Linux', 'x86_64'): 'x86_64'}[(platform.system(), platform.machine())]
    binary_dir.mkdir(parents=True, exist_ok=True)
    processes = []
    with tempfile.TemporaryDirectory(prefix='cdk-change-matrix-') as temporary:
        root = Path(temporary)
        try:
            urls = []
            for version, digest in DIGESTS[target].items():
                name = f'cdk-mintd-{version}-{target}'
                binary = binary_dir / name
                if not binary.exists():
                    subprocess.run(['gh', 'release', 'download', 'v' + version, '-R', 'cashubtc/cdk',
                                    '--pattern', name, '--dir', str(binary_dir)], check=True)
                if hashlib.sha256(binary.read_bytes()).hexdigest() != digest:
                    raise RuntimeError('Mint binary checksum mismatch: ' + name)
                binary.chmod(0o755)
                with socket.socket() as listener:
                    listener.bind(('127.0.0.1', 0))
                    port = listener.getsockname()[1]
                url = f'http://127.0.0.1:{port}'
                work = root / version
                work.mkdir()
                config = work / 'config.toml'
                config.write_text(f'''[info]
url = "{url}"
listen_host = "127.0.0.1"
listen_port = {port}
mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
input_fee_ppk = 0
[mint_info]
name = "Versioned recovery test mint"
[database]
engine = "sqlite"
[[ln]]
ln_backend = "fakewallet"
unit = "sat"
min_mint = 1
max_mint = 1000000
min_melt = 1
max_melt = 1000000
[fake_wallet]
supported_units = ["sat"]
fee_percent = 0.0
reserve_fee_min = 10
min_delay_time = 0
max_delay_time = 0
''')
                args = [str(binary), '--work-dir', str(work)]
                if version == '0.18.1':
                    migrated = work / 'migrated.toml'
                    subprocess.run(args + ['config', 'migrate', '--file', str(config), '--output', str(migrated)], check=True, stdout=subprocess.DEVNULL)
                    subprocess.run(args + ['config', 'init', '--new-mint', '--file', str(migrated)], check=True, stdout=subprocess.DEVNULL)
                log = open(work / 'mint.log', 'w')
                process = subprocess.Popen(args + ['--enable-logging'], stdout=log, stderr=subprocess.STDOUT)
                processes.append((process, log, work))
                for _ in range(100):
                    if process.poll() is not None:
                        raise RuntimeError((work / 'mint.log').read_text())
                    try:
                        with urllib.request.urlopen(url + '/v1/info', timeout=1) as response:
                            if response.status == 200:
                                break
                    except OSError:
                        time.sleep(0.1)
                else:
                    raise RuntimeError('Mint did not become ready: ' + version)
                urls.append(url)
            env = os.environ.copy()
            env['CDK_MELT_CHANGE_MINTS'] = ','.join(urls)
            command = ['cargo', 'test', '-p', 'cdk', '--no-default-features', '--features', 'wallet',
                       '--test', 'melt_change_versions']
            if offline:
                command.append('--offline')
            subprocess.run(command + ['--', '--ignored', '--nocapture'], env=env, check=True, timeout=600)
        finally:
            for process, log, work in processes:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                log.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary-dir', type=Path, default=Path(tempfile.gettempdir()) / 'cdk-change-binaries')
    parser.add_argument('--offline', action='store_true')
    options = parser.parse_args()
    run_matrix(options.binary_dir.resolve(), options.offline)
