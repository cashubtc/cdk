#!/usr/bin/env python3
"""Replay signed DNS wire responses through the real native CDK bindings (macOS).

No payment is executed. Only a localhost mint is allowed; interpose.c rejects
other remote sockets. The committed fixtures require no network access.
"""
import argparse
import asyncio
import http.server
import json
import os
from pathlib import Path
import shutil
import socket
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading

HERE = Path(__file__).resolve().parent
OFFER = os.environ.get('CDK_OFFER') or 'lno1pqqq5xj5wajkcan9gdshx6pq23jhxarfdenjqstyv3ex2umnzcss80xkrjkyrjk43u5dgu8f6a450fg2cnjtg7lhg76c3gtk5gdhshns'
ADDRESS = 'bc1p73p4sc52mdamccfrth4gn8wull0h7ywkzzn6d8e2mxqr4m7evdqqwusn64'
VALID_OFFER = 'lno1zcss9mk8y3wkklfvevcrszlmu23kfrxh49px20665dqwmn4p72pksese'
LNURL = 'lnurl1dp68gurn8ghj7anvwshxwef09emk2mrv944kummhdchkcmn4wfk8qte3xf6x2um50n5cup'


def recv_exact(sock, count):
    result = b''
    while len(result) < count:
        chunk = sock.recv(count - len(result))
        if not chunk:
            raise EOFError()
        result += chunk
    return result


async def native_child():
    import cdk_ffi as c
    mode = os.environ['CDK_TEST_MODE']
    wallet = c.Wallet(
        mint_url=os.environ['CDK_MINT_URL'], unit=c.CurrencyUnit.SAT(),
        mnemonic='abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about',
        store=c.sqlite_wallet_store(':memory:'),
        config=c.WalletConfig(target_proof_count=None),
    )
    if mode in ('expired', 'invalid', 'invalid-offer'):
        try:
            await wallet.melt_bip353_quote(os.environ.get('CDK_RECIPIENT', 'test@twelve.cash'), c.Amount(value=100000), c.BitcoinNetwork.BITCOIN)
        except c.FfiError as error:
            message = str(error)
            if mode == 'invalid-offer':
                assert 'InvalidBolt12(InvalidSemantics(InvalidAmount))' in message, message
            else:
                assert 'Failed to resolve BIP353' in message, message
                assert ('expired' if mode == 'expired' else 'signature validation failed') in message, message
            print(mode, message)
            return
        raise AssertionError('Invalid DNSSEC accepted')
    if mode == 'baseline':
        # Establish the same bug independently of the public record's invalid offer.
        try:
            await c.parse_bip321_payment_instruction(
                f'bitcoin:?lno={VALID_OFFER}&lnurl={LNURL}', c.BitcoinNetwork.BITCOIN)
        except c.FfiError as error:
            assert 'LNURL resolution not supported' in str(error), str(error)
        else:
            raise AssertionError('Expected mixed-method parsing to fail before the fix')
        try:
            await wallet.melt_bip353_quote(os.environ.get('CDK_RECIPIENT', 'test@twelve.cash'), c.Amount(value=100000), c.BitcoinNetwork.BITCOIN)
        except c.FfiError as error:
            assert 'LNURL resolution not supported' in str(error), str(error)
            print('baseline reproduced:', error)
            return
        raise AssertionError('Expected the pre-fix failure')

    if mode == 'success':
        # Fixed LDK test vector, independent of current public DNS contents.
        valid_offer = VALID_OFFER
        for body, extra in [('', ''), ('', f'&lnurl={LNURL}'), (ADDRESS, f'&lnurl={LNURL}')]:
            parsed = await c.parse_bip321_payment_instruction(
                f'bitcoin:{body}?lno={valid_offer}{extra}', c.BitcoinNetwork.BITCOIN)
            assert parsed.bolt12_offers == [valid_offer]
            assert len(parsed.onchain_addresses) == bool(body)
        for params in [f'lnurl={LNURL}', f'lno={valid_offer}&req-unknown=1&lnurl={LNURL}',
                       f'lno={valid_offer}&req-lnurl={LNURL}', f'lno=bad&lnurl={LNURL}']:
            try:
                await c.parse_bip321_payment_instruction(f'bitcoin:?{params}', c.BitcoinNetwork.BITCOIN)
            except c.FfiError as error:
                assert 'Failed to parse BIP321' in str(error), str(error)
            else:
                raise AssertionError(f'Invalid or unsupported instruction accepted: {params}')

    # Exercise actual generated bindings and the complete signed-DNS path.
    parsed = await c.resolve_bip353_payment_instruction(wallet, os.environ.get('CDK_RECIPIENT', 'test@twelve.cash'), c.BitcoinNetwork.BITCOIN)
    assert parsed.bolt12_offers == [OFFER]
    assert parsed.onchain_addresses == json.loads(os.environ['CDK_ADDRESSES'])
    try:
        quote = await wallet.melt_bip353_quote(os.environ.get('CDK_RECIPIENT', 'test@twelve.cash'), c.Amount(value=100000), c.BitcoinNetwork.BITCOIN)
    except c.FfiError as error:
        assert mode == 'mint-error', str(error)
        assert 'fixture mint failure' in str(error), str(error)
        assert 'BIP353' not in str(error) and 'BIP321' not in str(error), str(error)
        print('mint error preserved:', error)
        return
    assert mode == 'success'
    assert quote.request == OFFER and quote.amount.value == 100
    print('native signed-DNS BOLT12 quote succeeded')


class DnsServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


class DnsHandler(socketserver.BaseRequestHandler):
    def handle(self):
        while True:
            try:
                size = struct.unpack('!H', recv_exact(self.request, 2))[0]
                query = recv_exact(self.request, size)
            except EOFError:
                return
            key = query[2:].hex()
            server = self.server
            assert key in server.transcript, f'Unrecorded DNS question: {query.hex()}'
            response = bytes.fromhex(server.transcript[key])
            server.queries += 1
            if server.mode == 'invalid':
                # Corrupt signed TXT RDATA, preserving all wire framing.
                response = response.replace(b'bitcoin:', b'bitcoio:')
            response = query[:2] + response[2:]
            self.request.sendall(struct.pack('!H', len(response)) + response)


class MintHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        assert self.path == '/v1/melt/quote/bolt12', self.path
        body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        assert body['request'] == self.server.offer and body['unit'] == 'sat', body
        self.server.requests.append(body)
        if self.server.mode == 'mint-error':
            self.send_response(400)
            response = {'code': 10000, 'detail': 'fixture mint failure'}
        else:
            self.send_response(200)
            response = {'quote': 'fixture-quote', 'amount': 100, 'fee_reserve': 1,
                        'state': 'UNPAID', 'expiry': 2000000000}
        encoded = json.dumps(response).encode()
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bindings', type=Path, required=True)
    parser.add_argument('--library', type=Path, required=True)
    parser.add_argument('--baseline', action='store_true')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('The DNS socket/clock replay shim currently supports macOS only')
    with tempfile.TemporaryDirectory(prefix='cdk-bip353-native-') as temp_dir:
        temp = Path(temp_dir)
        shutil.copy2(args.bindings / 'cdk_ffi.py', temp)
        shutil.copy2(args.library, temp / 'libcdk_ffi.dylib')
        shim = temp / 'interpose.dylib'
        subprocess.run(['cc', '-dynamiclib', '-Wall', '-Werror', str(HERE / 'interpose.c'), '-o', str(shim)], check=True)
        modes = ['baseline'] if args.baseline else [
            'success', 'mint-error', 'invalid', 'expired', 'invalid-offer'
        ]
        for mode in modes:
            with DnsServer(('127.0.0.1', 0), DnsHandler) as dns, http.server.ThreadingHTTPServer(('127.0.0.1', 0), MintHandler) as mint:
                fixture_name = 'twelve.json' if mode in ('baseline', 'invalid-offer') else 'matt.json'
                current = json.loads((HERE / 'fixtures' / fixture_name).read_text())
                dns.transcript = current['responses']
                dns.mode, dns.queries = mode, 0
                mint.mode, mint.requests = mode, []
                mint.offer = current['offer']
                for server in (dns, mint):
                    threading.Thread(target=server.serve_forever, daemon=True).start()
                env = dict(os.environ, PYTHONPATH=str(temp), DYLD_INSERT_LIBRARIES=str(shim),
                           CDK_DNS_PORT=str(dns.server_address[1]),
                           CDK_FIXTURE_TIME=str(current['time'] + (30 * 86400 if mode == 'expired' else 0)),
                           CDK_RECIPIENT=current['recipient'], CDK_OFFER=current['offer'],
                           CDK_ADDRESSES=json.dumps(current['addresses']),
                           CDK_TEST_MODE=mode, CDK_MINT_URL=f'http://127.0.0.1:{mint.server_port}')
                result = subprocess.run([sys.executable, str(HERE / 'run.py'), '--child'], env=env, capture_output=True, text=True, timeout=60)
                print(result.stdout, end='')
                print(result.stderr, end='', file=sys.stderr)
                assert result.returncode == 0, f'{mode} child failed'
                assert 'UNEXPECTED_REMOTE_CONNECT' not in result.stderr, 'Recipient network access attempted'
                assert dns.queries > 0, 'DNS socket shim was not exercised'
                assert len(mint.requests) == (1 if mode in ('success', 'mint-error') else 0), mint.requests
                dns.shutdown()
                mint.shutdown()


if __name__ == '__main__':
    if sys.argv[1:] == ['--child']:
        asyncio.run(native_child())
    else:
        main()
