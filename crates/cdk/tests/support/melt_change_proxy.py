"""Loopback-only fault proxy for real-mint change recovery regressions."""
import http.client
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

upstream = urlsplit(sys.argv[1])
faults = False
requests = []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, data, status=200):
        body = json.dumps(data).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        self.forward()

    def do_POST(self):
        self.forward()

    def forward(self):
        global faults
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        if self.path == '/test/requests':
            return self.reply(requests)
        if self.path == '/test/faults':
            faults = json.loads(body)['enabled']
            return self.reply({})
        record = {'method': self.command, 'path': self.path, 'forwarded': False}
        requests.append(record)
        if faults and self.path == '/v1/restore':
            return self.reply({'outputs': [], 'signatures': []})
        conn = http.client.HTTPConnection(upstream.hostname, upstream.port, timeout=15)
        headers = {key: self.headers[key] for key in ('Content-Type', 'Prefer') if key in self.headers}
        try:
            conn.request(self.command, upstream.path.rstrip('/') + self.path, body, headers)
            response = conn.getresponse()
            content = response.read()
            record['forwarded'] = True
            if faults and self.command == 'POST' and self.path == '/v1/melt/bolt11':
                # The mint has committed; lose only its response.
                self.send_response(response.status)
                self.send_header('Content-Length', str(len(content) + 1))
                self.end_headers()
                self.close_connection = True
                return
            if faults and self.path.startswith('/v1/melt/quote/bolt11/'):
                payload = json.loads(content)
                payload.pop('change', None)
                return self.reply(payload, response.status)
            self.send_response(response.status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(content)))
            self.end_headers()
            self.wfile.write(content)
        finally:
            conn.close()


server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
print(server.server_port, flush=True)
server.serve_forever()
