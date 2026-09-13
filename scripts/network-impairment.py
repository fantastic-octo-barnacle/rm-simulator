#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Local TCP line-delay proxy or UDP packet-loss proxy.

TCP mode delays intact protocol lines and models ordered recovery stalls.
UDP mode drops real datagrams and applies random delay in each direction.
Both modes bound pending bytes. UDP mode supports one local client.
"""
import argparse
import asyncio
import json
import signal

from network_harness_lib import Profile, ProxyFarm, load_json, number

MAX_LINE_BYTES = 4 << 20
MAX_PENDING_BYTES = 8 << 20


async def relay(reader, writer, args, label):
    queue = asyncio.Queue(maxsize=64)
    pending_bytes = 0

    async def receive():
        nonlocal pending_bytes
        loop = asyncio.get_running_loop()
        tail = 0.0
        count = 0
        while True:
            line = await reader.readline()
            if not line:
                await queue.put(None)
                return
            if len(line) > MAX_LINE_BYTES or not line.endswith(b"\n"):
                raise ValueError("invalid or oversized protocol line")
            jitter = (0, args.jitter_ms, args.jitter_ms / 5, args.jitter_ms * 3 / 5)[count % 4]
            count += 1
            recovery = args.recovery_ms if args.stall_every and count % args.stall_every == 0 else 0
            tail = max(tail, loop.time() + (args.delay_ms + jitter + recovery) / 1000)
            if pending_bytes + len(line) > MAX_PENDING_BYTES:
                raise ValueError("proxy pending-byte limit exceeded")
            pending_bytes += len(line)
            await queue.put((tail, line))

    async def send():
        nonlocal pending_bytes
        loop = asyncio.get_running_loop()
        while True:
            item = await queue.get()
            if item is None:
                return
            due, line = item
            await asyncio.sleep(max(0, due - loop.time()))
            writer.write(line)
            await asyncio.wait_for(writer.drain(), timeout=5)
            pending_bytes -= len(line)

    tasks = [
        asyncio.create_task(receive(), name=f"{label}-receive"),
        asyncio.create_task(send(), name=f"{label}-send"),
    ]
    try:
        await asyncio.gather(*tasks)
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)


async def connected(reader, writer, args):
    upstream = None
    tasks = []
    try:
        remote_reader, upstream = await asyncio.wait_for(
            asyncio.open_connection("127.0.0.1", args.target_port, limit=MAX_LINE_BYTES), 5
        )
        print("client connected through impairment proxy", flush=True)
        tasks = [
            asyncio.create_task(relay(reader, upstream, args, "upstream")),
            asyncio.create_task(relay(remote_reader, writer, args, "downstream")),
        ]
        done, _ = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
        for task in done:
            task.result()
    except Exception as error:
        print(f"proxy connection ended: {error}", flush=True)
    finally:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        for stream in (writer, upstream):
            if stream is not None:
                stream.close()
                try:
                    await stream.wait_closed()
                except OSError:
                    pass
        print("client disconnected", flush=True)


async def run(args):
    server = await asyncio.start_server(
        lambda r, w: connected(r, w, args), "127.0.0.1", args.listen_port, limit=MAX_LINE_BYTES
    )
    print(
        f"127.0.0.1:{args.listen_port} -> 127.0.0.1:{args.target_port}; "
        f"each direction: {args.delay_ms} ms delay, 0..{args.jitter_ms} ms scripted jitter, "
        f"{args.recovery_ms} ms recovery every {args.stall_every} lines (0 disables)",
        flush=True,
    )
    async with server:
        await server.serve_forever()



def run_udp(args):
    """Keep the standalone one-client CLI; the harness uses the same proxy model."""
    common = {"delay_ms": args.delay_ms, "jitter_ms": args.jitter_ms,
              "loss_percent": args.loss_percent, "rate_kbps": args.rate_kbps,
              "queue_bytes": args.queue_bytes, "queue_wait_ms": args.queue_wait_ms,
              "duplicate_percent": args.duplicate_percent,
              "reorder_percent": args.reorder_percent, "reorder_delay_ms": args.reorder_delay_ms,
              "burst_good_ms": args.burst_good_ms, "burst_bad_ms": args.burst_bad_ms}
    profiles = {direction: dict(common) for direction in ("upstream", "downstream")}
    if args.profile:
        supplied = load_json(args.profile)
        if not isinstance(supplied, dict) or set(supplied) - {"upstream", "downstream"}:
            raise ValueError("profile file supports upstream and downstream only")
        for direction in profiles:
            if not isinstance(supplied.get(direction, {}), dict):
                raise ValueError("directional profile must be an object")
            profiles[direction].update(supplied.get(direction, {}))
    for direction, profile in profiles.items():
        for field in ("delay_ms", "jitter_ms", "loss_percent", "rate_kbps"):
            override = getattr(args, direction + "_" + field)
            if override is not None:
                profile[field] = override
        if args.blackout_duration_ms and args.blackout_direction in (direction, "both"):
            profile["blackouts"] = [*profile.get("blackouts", []),
                                    [args.blackout_after_ms / 1000, args.blackout_duration_ms / 1000]]
        Profile.parse(profile)

    def report(sample):
        packets = sample["peers"]["client"]
        print(json.dumps({**sample, "packets": packets,
                          "queued_bytes": sum(p["pending_bytes"] for p in packets.values())}), flush=True)

    proxy = ProxyFarm([{"name": "client", "listen_port": args.listen_port,
                        "target_port": args.target_port, **profiles}],
                      seed=args.seed, report=report, start_on_packet=True)
    print(json.dumps({"event": "ready", "listen_port": proxy.ports[0],
                      "target_port": args.target_port, "seed": args.seed,
                      "profiles": profiles, "clock": "seconds since first packet",
                      "accounting": "UDP payload plus profile overhead_bytes, default 28 IPv4/UDP bytes"}), flush=True)
    old = signal.signal(signal.SIGTERM, lambda *_: proxy.stop.set())
    try:
        proxy.start()
        while not proxy.stop.wait(0.2):
            pass
        if proxy.error:
            raise RuntimeError(str(proxy.error))
    finally:
        proxy.close()
        signal.signal(signal.SIGTERM, old)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--transport", choices=("tcp", "udp"), default="tcp")
    parser.add_argument("--loss-percent", type=float, default=0)
    parser.add_argument("--blackout-after-ms", type=float, default=0,
                        help="Start one packet blackout this long after the first UDP packet")
    parser.add_argument("--blackout-duration-ms", type=float, default=0)
    parser.add_argument("--blackout-direction", choices=("both", "upstream", "downstream"), default="both")
    parser.add_argument("--seed", type=int, default=2026)
    parser.add_argument("--listen-port", type=int, default=7701)
    parser.add_argument("--target-port", type=int, default=7700)
    parser.add_argument("--delay-ms", type=float, default=100)
    parser.add_argument("--jitter-ms", type=float, default=40)
    parser.add_argument("--stall-every", type=int, default=10)
    parser.add_argument("--recovery-ms", type=float, default=200)
    parser.add_argument("--profile", help="JSON object with upstream/downstream profile overrides")
    parser.add_argument("--rate-kbps", type=float, help="Each direction, decimal kbps; default unlimited")
    parser.add_argument("--queue-bytes", type=int, default=256 * 1024)
    parser.add_argument("--queue-wait-ms", type=float, default=250)
    parser.add_argument("--duplicate-percent", type=float, default=0)
    parser.add_argument("--reorder-percent", type=float, default=0)
    parser.add_argument("--reorder-delay-ms", type=float, default=0)
    parser.add_argument("--burst-good-ms", type=float, default=0, help="Mean good-state duration, exponential")
    parser.add_argument("--burst-bad-ms", type=float, default=0, help="Mean 100%% loss-state duration")
    for direction in ("upstream", "downstream"):
        for name in ("delay-ms", "jitter-ms", "loss-percent", "rate-kbps"):
            parser.add_argument(f"--{direction}-{name}", type=float)
    args = parser.parse_args()
    try:
        for key in ('delay_ms', 'jitter_ms', 'recovery_ms', 'blackout_after_ms', 'blackout_duration_ms'):
            number(getattr(args, key), key)
        number(args.listen_port, 'listen port', 0, 65535)
        number(args.target_port, 'target port', 1, 65535)
    except ValueError as error:
        parser.error(str(error))
    if any(value < 0 for value in (args.delay_ms, args.jitter_ms, args.stall_every, args.recovery_ms, args.blackout_after_ms, args.blackout_duration_ms)):
        parser.error("impairment parameters must be nonnegative")
    if not 0 <= args.loss_percent <= 100:
        parser.error("loss-percent must be between 0 and 100")
    if args.transport == "tcp" and (
        args.loss_percent or args.blackout_duration_ms or args.profile or args.rate_kbps is not None
        or args.duplicate_percent or args.reorder_percent or args.burst_good_ms or args.burst_bad_ms
        or any(getattr(args, direction + "_" + field) is not None
               for direction in ("upstream", "downstream")
               for field in ("delay_ms", "jitter_ms", "loss_percent", "rate_kbps"))
    ):
        parser.error("datagram impairment options require --transport udp")
    try:
        if args.transport == "udp":
            run_udp(args)
        else:
            asyncio.run(run(args))
    except KeyboardInterrupt:
        pass
    except (ValueError, OSError, RuntimeError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
