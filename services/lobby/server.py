# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Ephemeral lobby directory. No game traffic, accounts, or passwords."""
import ipaddress
import json
import secrets
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TTL = 45
MAX_LOBBIES = 512
LOCK = threading.Lock()
LOBBIES = {}


def prune(now):
    for token in list(LOBBIES):
        if LOBBIES[token][1] <= now:
            del LOBBIES[token]


def listing(data, peer):
    name = data.get('name')
    port = data.get('port')
    protocol = data.get('protocol')
    if not isinstance(name, str) or not 1 <= len(name.strip()) <= 64 or any(ord(c) < 32 for c in name):
        raise ValueError('Lobby name must be 1 to 64 printable characters')
    if type(port) is not int or not 1 <= port <= 65535:
        raise ValueError('Invalid game port')
    if type(protocol) is not int or not 1 <= protocol <= 10000:
        raise ValueError('Invalid protocol')
    if data.get('transport') not in ('gns', 'tcp') or type(data.get('locked')) is not bool:
        raise ValueError('Invalid transport or lock flag')
    address = str(ipaddress.ip_address(data.get('ip') or peer))
    if not ipaddress.ip_address(address).is_global:
        raise ValueError('Public lobbies need a public IP address')
    return dict(name=name.strip(), address=f'[{address}]:{port}' if ':' in address else f'{address}:{port}',
                protocol=protocol, transport=data['transport'], locked=data['locked'], lan=False)


class Handler(BaseHTTPRequestHandler):
    server_version = 'RM-Lobby/1'

    def setup(self):
        super().setup()
        self.connection.settimeout(5)

    def log_message(self, *_):
        pass  # Tokens must never enter access logs.

    def reply(self, status, data):
        body = json.dumps(data, separators=(',', ':')).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        with LOCK:
            prune(time.monotonic())
            if self.path == '/health':
                self.reply(200, {'ok': True})
            elif self.path == '/v1/lobbies':
                self.reply(200, [entry[0] for entry in LOBBIES.values()])
            else:
                self.reply(404, {'error': 'Not found'})

    def do_POST(self):
        try:
            size = int(self.headers.get('Content-Length', '0'))
            if not 0 < size <= 2048:
                raise ValueError('Invalid body size')
            data = json.loads(self.rfile.read(size))
            if not isinstance(data, dict):
                raise ValueError('Expected object')
            with LOCK:
                now = time.monotonic()
                prune(now)
                if self.path == '/v1/lobbies':
                    peer = self.client_address[0]
                    if len(LOBBIES) >= MAX_LOBBIES or sum(x[2] == peer for x in LOBBIES.values()) >= 8:
                        self.reply(429, {'error': 'Lobby limit reached'})
                        return
                    entry = listing(data, peer)
                    token = secrets.token_hex(32)
                    LOBBIES[token] = (entry, now + TTL, peer)
                    self.reply(201, {'token': token})
                elif self.path in ('/v1/heartbeat', '/v1/remove'):
                    token = data.get('token')
                    if not isinstance(token, str) or token not in LOBBIES:
                        self.reply(404, {'error': 'Listing expired'})
                        return
                    entry, _, peer = LOBBIES[token]
                    if self.path == '/v1/remove':
                        del LOBBIES[token]
                    else:
                        LOBBIES[token] = (entry, now + TTL, peer)
                    self.reply(200, {'ok': True})
                else:
                    self.reply(404, {'error': 'Not found'})
        except (ValueError, TypeError, OSError):
            self.reply(400, {'error': 'Invalid request'})


class Server(ThreadingHTTPServer):
    daemon_threads = True
    slots = threading.BoundedSemaphore(32)

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            request.close()
            return
        try:
            super().process_request(request, address)
        except Exception:
            self.slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()


if __name__ == '__main__':
    Server(('0.0.0.0', 7791), Handler).serve_forever()
