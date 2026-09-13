#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Send ordered app console commands; waits for readiness before executing them."""
import argparse
import json
import socket
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7790)
    parser.add_argument("--timeout", type=float, default=130)
    parser.add_argument("--no-wait", action="store_true", help="skip the initial ready command")
    parser.add_argument("commands", nargs="*", help="JSON command objects; otherwise read JSON lines from stdin")
    args = parser.parse_args()
    lines = args.commands if args.commands else sys.stdin
    commands = [json.loads(line) for line in lines if line.strip()]
    if not args.no_wait:
        commands.insert(0, {"type": "ready"})
    with socket.create_connection((args.host, args.port), args.timeout) as connection:
        with connection.makefile("rwb") as stream:
            for request_id, command in enumerate(commands, 1):
                request = {"id": request_id, "command": command}
                stream.write((json.dumps(request, allow_nan=False) + "\n").encode())
                stream.flush()
                line = stream.readline(4 * 1024 * 1024 + 1)
                if not line or len(line) > 4 * 1024 * 1024:
                    raise RuntimeError("console disconnected or sent an oversized response")
                reply = json.loads(line)
                print(json.dumps(reply), flush=True)
                if reply.get("id") != request_id:
                    raise RuntimeError("console response ID does not match the request")
                if not reply.get("ok"):
                    return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, RuntimeError) as error:
        print(f"console: {error}", file=sys.stderr)
        sys.exit(1)
