#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Run a prebuilt T1 test binary; preserve complete raw cases and provenance."""
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[4]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sources():
    paths = sorted((ROOT / "crates").rglob("*.rs"))
    paths += sorted((ROOT / "crates").rglob("Cargo.toml"))
    paths += [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "crates/rm-simulator-server/assets/checkpoint-dictionary.zstd"]
    return {str(p.relative_to(ROOT)): sha(p) for p in paths}


def main():
    binary = Path(sys.argv[1]).resolve()
    out = Path(sys.argv[2]).resolve()
    codec = sys.argv[3]
    assert codec in {"deflate", "zstd", "zstd-dict"}
    out.mkdir(parents=True, exist_ok=False)
    environment = dict(os.environ, RM_NET_CODEC=codec, RM_NET_DEFLATE_LEVEL="1", RM_NET_ZSTD_LEVEL="3")
    command = [str(binary), "--ignored", "--exact",
               "section_topics::delivery::trials::tuning::run::t1_diagnostics",
               "--nocapture"]
    metadata = {
        "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "machine": platform.machine(), "binary_sha256": sha(binary),
        "source_sha256": sources(), "command": command,
        "compression_environment": {k: environment[k] for k in ["RM_NET_CODEC", "RM_NET_DEFLATE_LEVEL", "RM_NET_ZSTD_LEVEL"]},
        "dictionary_sha256": sha(ROOT / "crates/rm-simulator-server/assets/checkpoint-dictionary.zstd"),
        "scope": "T1 diagnostics, not parameter search; scripted application datagrams; test-only instrumentation",
        "warmup_ms": 10016, "measured_ms": 60000,
        "workload_seed": 71, "impairment_seed": 71 ^ 0x7000,
        "profiles": ["clean", "limited", "blackout"], "players": [2, 12],
        "cadence_ms": [32, 64, 128], "instrumentation_parity": "full run repeated off/on; all outputs except diagnostics must match",
        "case_sha256": {}, "complete": False,
    }
    (out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    log = []
    process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    assert process.stdout is not None
    for line in process.stdout:
        if line.startswith("RM_SECTION_T1 "):
            raw = line.removeprefix("RM_SECTION_T1 ").encode()
            row = json.loads(raw)
            name = f"{row['players']}-{row['profile']}.json.gz"
            (out / name).write_bytes(gzip.compress(raw, mtime=0))
            metadata["case_sha256"][name] = sha(out / name)
            (out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
            log.append(f"RM_SECTION_T1_ARTIFACT {name}\n")
            print(name, "recorded", flush=True)
        else:
            log.append(line)
    result = process.wait()
    (out / "trial-output.txt").write_text("".join(log).rstrip() + "\n")
    assert result == 0 and "test result: ok." in "".join(log), "trial failed; partial artifacts retained"
    assert len(metadata["case_sha256"]) == 6, "incomplete matrix"
    assert sources() == metadata["source_sha256"] and sha(binary) == metadata["binary_sha256"], "build/source changed during trial"
    metadata["complete"] = True
    (out / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")


if __name__ == "__main__":
    main()
