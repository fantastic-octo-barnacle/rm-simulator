#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Process/console/UDP fixture. No simulator physics or GNS behavior is modeled."""
import argparse
import http.server
import json
import os
import socket
import socketserver
import threading
import time


class LoopbackHTTPServer(http.server.HTTPServer):
    """Bind the numeric fixture address without a reverse-DNS lookup."""

    def server_bind(self):
        # HTTPServer.server_bind calls getfqdn, which can stall on macOS CI:
        # https://github.com/actions/setup-python/issues/1223
        socketserver.TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address


def address(value):
    host, port = value.split(':')
    return host, int(port)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--listen')
    parser.add_argument('--http')
    parser.add_argument('--connect')
    parser.add_argument('--console')
    args, _ = parser.parse_known_args()
    stop = threading.Event()
    received_at = [time.monotonic()]
    stall_after = int(os.environ.get('RM_FIXTURE_STALL_AFTER', 0))
    stall_s = float(os.environ.get('RM_FIXTURE_STALL_S', 0))
    stall_count = int(os.environ.get('RM_FIXTURE_STALL_COUNT', 1))
    replies, stalls = [0], [0]
    if args.listen:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
            udp.bind(address(args.listen))
            udp.settimeout(.05)
            def echo():
                while not stop.is_set():
                    try:
                        payload, peer = udp.recvfrom(65535)
                        udp.sendto(payload, peer)
                    except socket.timeout:
                        pass
            thread = threading.Thread(target=echo, daemon=True)
            thread.start()
            class Handler(http.server.BaseHTTPRequestHandler):
                def do_GET(self):
                    self.send_response(200)
                    self.end_headers()
                    self.wfile.write(b'{}')
                def log_message(self, *_):
                    pass
            LoopbackHTTPServer(address(args.http), Handler).serve_forever()
    else:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp, socket.socket() as listener:
            udp.connect(address(args.connect))
            udp.settimeout(.02)
            def traffic():
                while not stop.is_set():
                    udp.send(b'x' * 256)
                    try:
                        udp.recv(65535)
                        received_at[0] = time.monotonic()
                    except (socket.timeout, ConnectionRefusedError):
                        pass
                    stop.wait(.01)
            thread = threading.Thread(target=traffic, daemon=True)
            thread.start()
            listener.bind(address(args.console))
            listener.listen()
            listener.settimeout(.05)
            try:
                while not stop.is_set():
                    try:
                        connection, _ = listener.accept()
                    except socket.timeout:
                        continue
                    with connection, connection.makefile('rwb') as stream:
                        for line in stream:
                            message = json.loads(line)
                            # Optional scripted console stall, so tests can exercise the
                            # harness' sample-gap handling without a real slow frame.
                            replies[0] += 1
                            if stall_after and replies[0] > stall_after and \
                                    (not stall_count or stalls[0] < stall_count):
                                stalls[0] += 1
                                time.sleep(stall_s)
                            state = {'ready': True, 'shots_fired':0,
                                     'network': {'rtt_ms':42, 'checkpoint_gap_ms':
                                                 (time.monotonic() - received_at[0]) * 1000}}
                            stream.write((json.dumps({'id':message['id'], 'ok':True,'result':state})+'\n').encode())
                            stream.flush()
                            if message['command']['type'] == 'quit':
                                stop.set()
                                break
            finally:
                stop.set()
                thread.join(timeout=1)


if __name__ == '__main__':
    main()
