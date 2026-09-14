#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Run frozen T2 blocks in independent processes, after all compilation finishes."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import gzip
import hashlib
from itertools import product
import json
import os
from pathlib import Path
import platform
import subprocess
import time

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]
VARIANTS = ["whole"] + [v for period in [32, 64, 128] for v in [f"rr-{period}", *[f"drr-{period}-{w}" for w in ["111", "211", "121", "112"]]]]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources():
    paths = sorted((ROOT / "crates").rglob("*.rs")) + sorted((ROOT / "crates").rglob("Cargo.toml"))
    paths += [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "crates/rm-simulator-server/assets/checkpoint-dictionary.zstd", HERE / "plan.json", HERE / "run.py", HERE / "summarize.py"]
    return {str(p.relative_to(ROOT)): sha(p) for p in paths}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--workers", type=int, default=3)
    parser.add_argument("--pilot", action="store_true")
    args = parser.parse_args()
    assert 1 <= args.workers <= 3
    binary, out = args.binary.resolve(), args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    variants = ["whole", "rr-128", "drr-128-111"] if args.pilot else VARIANTS
    blocks = [(12, "limited")] if args.pilot else list(product([2, 12], ["clean", "rtt", "limited"]))
    command = [str(binary), "--ignored", "--exact", "section_topics::delivery::trials::tuning::t2::t2_block", "--nocapture"]
    metadata = {
        "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "machine": platform.machine(), "binary_sha256": sha(binary), "source_sha256": sources(),
        "plan": json.loads((HERE / "plan.json").read_text()), "workers": args.workers,
        "pilot": args.pilot, "variants": variants, "blocks": blocks, "command": command,
        "complete": False, "case_sha256": {},
    }
    def write_metadata():
        (out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    write_metadata()
    started = time.monotonic()

    def block(spec):
        players, profile = spec
        start = time.monotonic()
        environment = dict(os.environ, RM_NET_CODEC="zstd-dict", RM_NET_ZSTD_LEVEL="3", RM_NET_DEFLATE_LEVEL="1", RM_SECTION_T2_PLAYERS=str(players), RM_SECTION_T2_PROFILE=profile, RM_SECTION_T2_VARIANTS=",".join(variants))
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        log, hashes = [], {}
        assert process.stdout is not None
        for line in process.stdout:
            if line.startswith("RM_SECTION_T2 "):
                raw = line.removeprefix("RM_SECTION_T2 ").encode()
                row = json.loads(raw)
                assert row["players"] == players and row["profile"] == profile and row["variant"] in variants
                name = f"{players}-{profile}-{row['variant']}.json.gz"
                assert name not in hashes
                (out / name).write_bytes(gzip.compress(raw, mtime=0))
                hashes[name] = sha(out / name)
                log.append(f"RM_SECTION_T2_ARTIFACT {name}\n")
                print(f"{name} {time.monotonic()-start:.1f}s elapsed in block", flush=True)
            else:
                log.append(line)
        result = process.wait()
        (out / f"{players}-{profile}-output.txt").write_text("".join(log).rstrip() + "\n")
        return {"players": players, "profile": profile, "wall_seconds": time.monotonic()-start,
                "complete": result == 0 and "test result: ok." in "".join(log) and len(hashes) == len(variants), "case_sha256": hashes}

    with ThreadPoolExecutor(max_workers=args.workers) as pool:
        metadata["block_results"] = list(pool.map(block, blocks))
    for result in metadata["block_results"]:
        metadata["case_sha256"].update(result["case_sha256"])
    metadata["wall_seconds"] = time.monotonic() - started
    unchanged = sources() == metadata["source_sha256"] and sha(binary) == metadata["binary_sha256"]
    metadata["complete"] = unchanged and all(b["complete"] for b in metadata["block_results"])
    write_metadata()
    assert metadata["complete"], "incomplete run or changed source/build; retained artifacts"
    print(f"Complete: {len(metadata['case_sha256'])} runs in {metadata['wall_seconds']:.1f}s wall time", flush=True)


if __name__ == "__main__":
    main()
