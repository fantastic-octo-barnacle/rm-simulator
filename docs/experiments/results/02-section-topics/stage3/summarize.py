# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate the screening and selected follow-up; report both benefits and costs."""
import json
from itertools import product
from pathlib import Path
from statistics import mean

root = Path(__file__).parent
profiles = ("clean", "rtt40", "rtt100_loss", "blackout", "limited")


def read(name, expected, duration, warmup):
    text = (root / name).read_text()
    rows = [json.loads(line.split(" ", 1)[1]) for line in text.splitlines() if line.startswith("RM_SECTION_STAGE3 ")]
    keys = [(r["players"], r["profile"], r["seed"], r["cadence"]["chassis_ms"], r["cadence"]["projectiles_ms"], r["cadence"]["checkpoint_ms"]) for r in rows]
    assert len(keys) == len(expected) and set(keys) == expected and "test result: ok." in text, "incomplete matrix"
    for r in rows:
        assert r["measured_ms"] == duration and r["warmup_ms"] == warmup
        assert r["offered_source_captures"] == duration // 16
        assert r["shots_launched_measured"] == (1250 if duration == 60000 else 210)
        peers = [p for p in rows if (p["players"], p["seed"]) == (r["players"], r["seed"])]
        assert len({p["captured_stream_sha256"] for p in peers}) == 1
        for codec in ("whole", "sections"):
            # Burst allowance plus two milliseconds of window-boundary rounding.
            assert r[codec]["downstream_bytes"] <= r["downstream_budget_bytes_s"] * (duration / 1000 + .034) + 1024
            assert r[codec]["upstream_bytes"] <= r["upstream_budget_bytes_s"] * (duration / 1000 + .034) + 1024
        assert r["sections"]["peak_queued_payload_bytes"] <= 2 << 20
        assert r["sections"]["peak_partial_frame_bytes"] <= 4 << 20
    return rows


screen = read("screen-output.txt", set(product((2, 12), ("clean", "rtt100_loss", "limited"), (1,), (16, 32), (32, 64), (32, 64, 128))), 10000, 2016)
rows = read("trial-output.txt", set(product((2, 12), profiles, range(1, 6), (32,), (64,), (128,))), 60000, 10016)
for name, data in (("screen.jsonl", screen), ("runs.jsonl", rows)):
    (root / name).write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in data))

print("Selected cadence: chassis/projectiles/checkpoint = 32/64/128 ms")
print("| Robots | Link | Whole → sections KiB/s | Downstream change range | p95 checkpoint age change | p95 chassis age change |")
print("|---:|---|---:|---:|---:|---:|")
for players, profile in product((2, 12), profiles):
    group = [r for r in rows if (r["players"], r["profile"]) == (players, profile)]
    rates = [mean(r[c]["downstream_bytes"] / 60 / 1024 for r in group) for c in ("whole", "sections")]
    def span(key):
        v = [r[key] for r in group]
        return f"{min(v):+.2f}…{max(v):+.2f}"
    print(f"| {players} | {profile} | {rates[0]:.2f} → {rates[1]:.2f} | {span('downstream_change_percent')}% | {span('p95_checkpoint_age_delta_ms')} ms | {span('p95_available_chassis_age_delta_ms')} ms |")
print("\nPose error: mean of each seed's p95, matched identities only; missing/stale counts retained in JSON.")
print("| Robots | Link | Body error whole → sections (m) | Aim error (rad) | Projectile error (m) |")
print("|---:|---|---:|---:|---:|")
for players, profile in product((2, 12), profiles):
    group = [r for r in rows if (r["players"], r["profile"]) == (players, profile)]
    cells = []
    for metric in ("body_m", "aim_rad", "projectile_m"):
        values = [mean(r[c][metric]["p95"] for r in group) for c in ("whole_pose_error", "section_pose_error")]
        cells.append(f"{values[0]:.4f} → {values[1]:.4f}")
    print(f"| {players} | {profile} | " + " | ".join(cells) + " |")
for codec in ("whole", "sections"):
    print(f"{codec}: {sum(r[codec]['delivered_checkpoints'] for r in rows):,} measured exact checkpoint deliveries.")
print("Peak section payload cache / send queue / partial allocation:", *(max(r['sections'][k] for r in rows) for k in ('peak_cached_section_bytes', 'peak_queued_payload_bytes', 'peak_partial_frame_bytes')))
