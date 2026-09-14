#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Check DEFLATE regression equivalence and print paired Zstd T1 measurements."""
import gzip
import hashlib
import json
from pathlib import Path

BASE = Path(__file__).resolve().parent.parent


def read(name):
    root = BASE / name
    metadata = json.loads((root / "metadata.json").read_text())
    assert metadata["complete"]
    rows = {}
    for filename, digest in metadata["case_sha256"].items():
        data = (root / filename).read_bytes()
        assert hashlib.sha256(data).hexdigest() == digest
        row = json.loads(gzip.decompress(data))
        rows[row["players"], row["profile"]] = row
    assert len(rows) == 6
    return metadata, rows


old_meta, old = read("t1")
deflate_meta, deflate = read("t1-deflate-rebased")
zstd_meta, zstd = read("t1-zstd-dict")
assert deflate.keys() == zstd.keys() == old.keys()
assert deflate_meta["source_sha256"] == zstd_meta["source_sha256"]
assert deflate_meta["binary_sha256"] == zstd_meta["binary_sha256"]
assert deflate_meta["dictionary_sha256"] == zstd_meta["dictionary_sha256"]
print("robots profile | section versus whole bytes % | p95 checkpoint/chassis/Pong ms whole -> section | Zstd byte change % whole/section versus DEFLATE")
for key in sorted(deflate):
    d, z = deflate[key], zstd[key]
    comparable = dict(d)
    del comparable["compression"]
    assert comparable == old[key], f"DEFLATE regression: {key}"
    assert d["compression"] == {"mode": "Deflate", "level": 1}
    assert z["compression"] == {"mode": "ZstdDictionary", "level": 3}
    for field in ["captured_stream_sha256", "impairment_sha256", "cadence", "seed", "warmup_ms", "measured_ms", "downstream_budget_bytes_s"]:
        assert d[field] == z[field]
    change = [100 * (z[path]["downstream_bytes"] / d[path]["downstream_bytes"] - 1) for path in ["whole", "sections"]]
    ages = [f"{z['whole'][field]['p95']:.0f}->{z['sections'][field]['p95']:.0f}" for field in ["checkpoint_age_ms", "available_chassis_age_ms"]]
    ages += [f"{z['whole_controls']['pong_latency_ms']['p95']:.0f}->{z['section_controls']['pong_latency_ms']['p95']:.0f}"]
    print(*key, f"{z['downstream_change_percent']:+.2f}%", *ages, *(f"{v:+.2f}%" for v in change))
print("All six rebased DEFLATE rows exactly match historical T1 after removing the new codec label; Zstd source, impairment, build and cadence pairing verified.")
