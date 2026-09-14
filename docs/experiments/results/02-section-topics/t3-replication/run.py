#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Run frozen T3 replication blocks in independent processes, after all compilation finishes."""
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
VARIANTS = ["drr-128-211", "completion-128-211"]


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
    args = parser.parse_args()
    assert 1 <= args.workers <= 3
    binary, out = args.binary.resolve(), args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    variants = VARIANTS
    blocks = list(product([102, 103], [2, 12], ["clean", "rtt", "limited"]))
    command = [str(binary), "--ignored", "--exact", "section_topics::delivery::trials::tuning::t3::t3_block", "--nocapture"]
    metadata = {
        "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "machine": platform.machine(), "binary_sha256": sha(binary), "source_sha256": sources(),
        "plan": json.loads((HERE / "plan.json").read_text()), "workers": args.workers,
        "variants": variants, "blocks": blocks, "command": command,
        "complete": False, "case_sha256": {},
    }
    def write_metadata():
        (out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    write_metadata()
    started = time.monotonic()

    def block(spec):
        seed, players, profile = spec
        start = time.monotonic()
        environment = dict(os.environ, RM_NET_CODEC="zstd-dict", RM_NET_ZSTD_LEVEL="3", RM_NET_DEFLATE_LEVEL="1", RM_SECTION_T3_SEED=str(seed), RM_SECTION_T3_PLAYERS=str(players), RM_SECTION_T3_PROFILE=profile)
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        log, hashes = [], {}
        assert process.stdout is not None
        for line in process.stdout:
            if line.startswith("RM_SECTION_T3 "):
                raw = line.removeprefix("RM_SECTION_T3 ").encode()
                row = json.loads(raw)
                assert row["seed_id"] == seed and row["players"] == players and row["profile"] == profile and row["variant"] in variants
                name = f"{seed}-{players}-{profile}-{row['variant']}.json.gz"
                assert name not in hashes
                (out / name).write_bytes(gzip.compress(raw, mtime=0))
                hashes[name] = sha(out / name)
                log.append(f"RM_SECTION_T3_ARTIFACT {name}\n")
                print(f"{name} {time.monotonic()-start:.1f}s elapsed in block", flush=True)
            else:
                log.append(line)
        result = process.wait()
        (out / f"{seed}-{players}-{profile}-output.txt").write_text("".join(log).rstrip() + "\n")
        return {"seed_id": seed, "players": players, "profile": profile, "wall_seconds": time.monotonic()-start,
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
