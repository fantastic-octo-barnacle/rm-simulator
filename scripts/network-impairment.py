#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Local UDP packet-loss proxy.

UDP mode drops real datagrams and applies random delay in each direction.
It bounds pending bytes and supports one local client.
"""
import argparse
import json
import signal

from network_harness_lib import Profile, ProxyFarm, load_json, number


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
        for key in ('delay_ms', 'jitter_ms', 'blackout_after_ms', 'blackout_duration_ms'):
            number(getattr(args, key), key)
        number(args.listen_port, 'listen port', 0, 65535)
        number(args.target_port, 'target port', 1, 65535)
    except ValueError as error:
        parser.error(str(error))
    if any(value < 0 for value in (args.delay_ms, args.jitter_ms, args.blackout_after_ms, args.blackout_duration_ms)):
        parser.error("impairment parameters must be nonnegative")
    if not 0 <= args.loss_percent <= 100:
        parser.error("loss-percent must be between 0 and 100")
    try:
        run_udp(args)
    except KeyboardInterrupt:
        pass
    except (ValueError, OSError, RuntimeError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
