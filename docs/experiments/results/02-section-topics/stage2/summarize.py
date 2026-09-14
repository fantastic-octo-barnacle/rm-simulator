# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate and summarize all stage-2 trial cells without hiding stale state."""
import json
from pathlib import Path
from statistics import mean

root = Path(__file__).parent
text = (root / "trial-output.txt").read_text()
rows = [json.loads(line.split(" ", 1)[1]) for line in text.splitlines() if line.startswith("RM_SECTION_STAGE2 ")]
profiles = ("clean", "rtt40", "rtt100_loss", "blackout", "limited")
expected = {(players, profile, seed) for players in (2, 12) for profile in profiles for seed in range(1, 6)}
actual = [(row["players"], row["profile"], row["seed"]) for row in rows]
if set(actual) != expected or len(actual) != len(expected) or "test result: ok." not in text:
    raise ValueError(f"incomplete trial matrix: {len(rows)}/50 rows or no passing test footer")
for row in rows:
    if row["measured_ms"] != 60000 or row["warmup_ms"] != 10016 or row["offered_checkpoints"] != 1875:
        raise ValueError("unexpected duration or publication count")
    if row["shots_launched_measured"] != 1250:
        raise ValueError("firing workload did not execute all expected launches")
    peers = [r for r in rows if (r["players"], r["seed"]) == (row["players"], row["seed"])]
    if len({r["captured_stream_sha256"] for r in peers}) != 1:
        raise ValueError("profiles did not use the same captured source")
(root / "runs.jsonl").write_text("".join(json.dumps(row, sort_keys=True) + "\n" for row in rows))
print("| Robots | Link | Whole → sections KiB/s | Downstream change range | p95 checkpoint age change range | Mean checkpoints whole → sections |")
print("|---:|---|---:|---:|---:|---:|")
for players in (2, 12):
    for profile in profiles:
        group = [r for r in rows if r["players"] == players and r["profile"] == profile]
        down = [r["downstream_change_percent"] for r in group]
        age = [r["p95_checkpoint_age_delta_ms"] for r in group]
        rates = [mean(r[codec]["downstream_bytes"] / 60 / 1024 for r in group) for codec in ("whole", "sections")]
        counts = [mean(r[codec]["delivered_checkpoints"] for r in group) for codec in ("whole", "sections")]
        print(f"| {players} | {profile} | {rates[0]:.2f} → {rates[1]:.2f} | {min(down):+.2f}…{max(down):+.2f}% | {min(age):+.0f}…{max(age):+.0f} ms | {counts[0]:.0f} → {counts[1]:.0f} |")
print()
for codec in ("whole", "sections"):
    print(f"{codec}: {sum(r[codec]['delivered_checkpoints'] for r in rows):,} measured deliveries checked exactly against the captured player representation.")
print(f"Peak section cache payload: {max(r['sections']['peak_cached_section_bytes'] for r in rows):,} bytes.")
print(f"Peak queued payload: {max(r['sections']['peak_queued_payload_bytes'] for r in rows):,} bytes.")
print(f"Peak partial-frame allocation: {max(r['sections']['peak_partial_frame_bytes'] for r in rows):,} bytes.")
