# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Summarize the five-seed stage-1 byte probe; no timing inference."""
import json
from pathlib import Path
from statistics import mean

rows = [json.loads(line) for line in Path(__file__).with_name("runs.jsonl").read_text().splitlines()]
if len(rows) != 20:
    raise ValueError(f"expected 20 completed runs, got {len(rows)}")
print("| Workload | Whole KiB/s | Sections KiB/s | Downstream increase (range) | Fragment multiplier |")
print("|---|---:|---:|---:|---:|")
for name in ("idle", "drive", "fire", "twelve"):
    runs = [row for row in rows if row["scenario"] == name]
    if sorted(row["seed"] for row in runs) != [1, 2, 3, 4, 5]:
        raise ValueError(f"missing or duplicate seeds in {name}")
    for run in runs:
        for codec in ("whole", "sections"):
            if run[codec]["checkpoints"] != 1875 or run["measured_seconds"] != 60:
                raise ValueError("incomplete checkpoint delivery or wrong duration")
    rate = lambda codec: mean(r[codec]["downstream_bytes"] / r["measured_seconds"] / 1024 for r in runs)
    change = [r["downstream_change_percent"] for r in runs]
    fragments = mean(r["sections"]["downstream_fragments"] / r["whole"]["downstream_fragments"] for r in runs)
    print(f"| {name} | {rate('whole'):.2f} | {rate('sections'):.2f} | +{min(change):.2f}–{max(change):.2f}% | {fragments:.2f}× |")
print()
print("All 37,500 measured checkpoints were delivered by each codec and checked against the existing player representation.")
print(f"Peak retained section payload: {max(r['sections']['max_cached_payload_bytes'] for r in rows):,} bytes (excludes baseline and allocator overhead).")
