#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate the frozen T2 matrix, Pareto comparisons and prespecified nominations."""
import gzip
import hashlib
from itertools import product
import json
import math
from pathlib import Path
import sys

ROOT = Path(sys.argv[1]).resolve()
meta = json.loads((ROOT / "metadata.json").read_text())
assert meta["complete"] and not meta["pilot"]
rows = {}
for name, digest in meta["case_sha256"].items():
    raw = (ROOT / name).read_bytes()
    assert hashlib.sha256(raw).hexdigest() == digest
    r = json.loads(gzip.decompress(raw))
    key = (r["players"], r["profile"], r["variant"])
    assert key not in rows
    rows[key] = r
    assert r["compression"] == {"mode": "ZstdDictionary", "level": 3} and r["prepared_dictionary_by_copy"]
    assert r["warmup_ms"] == 10016 and r["measured_ms"] == 60000
    assert r["seed_id"] == 101 and r["workload_seed"] == 0x10000000 + 101 and r["impairment_seed"] == 0x20000000 + 101
    assert r["shots_launched_measured"] == 1250
    assert r["controls"]["confirmation_before_pong"] and r["controls"]["offered_pairs"] == 13
    assert r["metrics"]["peak_queued_payload_bytes"] <= 2 << 20
    assert r["metrics"]["peak_partial_frame_bytes"] <= 4 << 20
    if r["diagnostics"] is not None:
        d = r["diagnostics"]
        assert sum(c["sent_payload_bytes"] + c["fragment_header_bytes"] for c in d["classes"]) + d["shared_datagram_header_bytes"] == d["downstream_bytes"] == r["sender_lifetime"]["sent_bytes"]
        assert d["downstream_bytes"] <= r["downstream_budget_bytes_s"] * 70.016
        for c in d["classes"]:
            assert c["offered_payload_bytes"] - c["duplicate_suppressed_payload_bytes"] - c["replaced_unsent_payload_bytes"] == c["fully_sent_payload_bytes"] + c["queued_payload_bytes_at_end"]
        assert r["upstream_lifetime_bytes_datagrams"][0] <= 10240 * 70.016
        for event in d["checkpoint_events"]:
            assert event["source_capture_ms"] <= event["encoded_ms"] <= event["first_manifest_ms"] <= event["usable_ms"]
        del d["transfer_events"], d["checkpoint_events"]
blocks = list(product([2, 12], ["clean", "rtt", "limited"]))
variants = meta["variants"]
assert set(rows) == {(p, n, v) for (p, n), v in product(blocks, variants)} and len(rows) == 96
for players in [2, 12]:
    assert len({r["captured_stream_sha256"] for r in rows.values() if r["players"] == players}) == 1
for profile in ["clean", "rtt", "limited"]:
    assert len({r["impairment_sha256"] for r in rows.values() if r["profile"] == profile}) == 1
for p, profile in blocks:
    assert "test result: ok." in (ROOT / f"{p}-{profile}-output.txt").read_text()
    assert sorted(rows[p, profile, v]["execution_ordinal"] for v in variants) == list(range(16))


def objectives(r):
    m, e, c, d = r["metrics"], r["errors"], r["controls"], r["coverage_denominators"]
    shown = d["whole_shown_projectile_samples" if r["variant"] == "whole" else "section_shown_projectile_samples"]
    values = [m["downstream_bytes"], m["upstream_bytes"], m["checkpoint_age_ms"]["p95"], m["available_chassis_age_ms"]["p95"], e["projectile_age_ms"]["p95"], e["body_m"]["p95"], e["aim_rad"]["p95"], e["projectile_m"]["p95"], e["missing_robot_samples"] / d["truth_robot_samples"], e["missing_projectile_samples"] / d["truth_projectile_samples"], e["stale_projectile_samples"] / shown if shown else 1., c["pong_latency_ms"]["p95"]]
    assert all(math.isfinite(x) for x in values)
    return values


def eligible(r):
    c, m = r["controls"], r["metrics"]
    return c["pending_confirmations_at_end"] == c["pending_pongs_at_end"] == 0 and m["samples_without_checkpoint"] == 0 and m["delivered_checkpoints"] > 0


def frontier(vectors):
    return sorted(v for v, x in vectors.items() if not any(w != v and all(a <= b for a, b in zip(y, x)) and any(a < b for a, b in zip(y, x)) for w, y in vectors.items()))

eligible_variants = [v for v in variants if all(eligible(rows[p, n, v]) for p, n in blocks)]
vectors = {v: [x for p, n in blocks for x in objectives(rows[p, n, v])] for v in eligible_variants}
global_frontier = frontier(vectors)
per_cell = {f"{p}-{n}": frontier({v: objectives(rows[p,n,v]) for v in variants if eligible(rows[p,n,v])}) for p,n in blocks}


def checkpoint_ratio(v):
    return max(rows[p,n,v]["metrics"]["checkpoint_age_ms"]["p95"] / rows[p,n,"whole"]["metrics"]["checkpoint_age_ms"]["p95"] for p,n in blocks)


candidates = [v for v in global_frontier if v != "whole"]
nominees = ["rr-128"]
if candidates:
    low_bytes = min(candidates, key=lambda v: (max(rows[p,"clean",v]["metrics"]["downstream_bytes"] / rows[p,"clean","whole"]["metrics"]["downstream_bytes"] for p in [2,12]), checkpoint_ratio(v), v))
    low_pong = min(candidates, key=lambda v: (max(rows[p,"limited",v]["controls"]["pong_latency_ms"]["p95"] for p in [2,12]), checkpoint_ratio(v), v))
    nominees = list(dict.fromkeys(nominees + [low_bytes, low_pong]))
assert len(nominees) <= 3
paired = []
for p,n in blocks:
    base = objectives(rows[p,n,"whole"])
    for v in variants:
        x = objectives(rows[p,n,v])
        paired.append({"players":p,"profile":n,"variant":v,"minus_whole": [a-b for a,b in zip(x,base)], "downstream_ratio":x[0]/base[0]})
interactions = []
for p,n in blocks:
    for weight in ["111","211","121","112"]:
        effects = {}
        for period in [32,64,128]:
            a,b = objectives(rows[p,n,f"drr-{period}-{weight}"]), objectives(rows[p,n,f"rr-{period}"])
            effects[str(period)] = [x-y for x,y in zip(a,b)]
        interactions.append({"players":p,"profile":n,"weights":weight,"drr_minus_rotation_by_checkpoint_ms":effects,"effect128_minus_effect32":[a-b for a,b in zip(effects["128"],effects["32"])]})
# This is only an offline screen of necessary conditions, not the prediction gate.
approaching = []
for v in eligible_variants:
    if v == "whole": continue
    if all(rows[2,n,v]["metrics"]["downstream_bytes"] <= .5*rows[2,n,"whole"]["metrics"]["downstream_bytes"] and all(a <= b for a,b in zip(objectives(rows[2,n,v])[2:],objectives(rows[2,n,"whole"])[2:])) for n in ["clean","rtt","limited"]):
        approaching.append(v)
summary={"scope":"single-seed exploratory screen; no independent uncertainty estimate or statistical winner", "objective_order":meta["plan"]["pareto_objectives_minimize"], "eligible_variants":eligible_variants,"global_frontier":global_frontier,"per_cell_frontiers":per_cell,"nominations":nominees,"offline_gate_candidates":approaching,"paired_differences":paired,"interactions":interactions}
(ROOT/"analysis.json").write_text(json.dumps(summary,indent=2)+"\n")
(ROOT/"runs.jsonl.gz").write_bytes(gzip.compress("".join(json.dumps(rows[k],sort_keys=True)+"\n" for k in sorted(rows)).encode(), mtime=0))
lines=["# T2 screening results", "", "One fresh seed; 96 runs; simulated 60-second windows. No statistical winner or production adoption claim.", "", f"Global Pareto set: {', '.join(global_frontier)}.", "", f"Prespecified nominations: {', '.join(nominees)}.", "", f"Offline gate candidates: {approaching}.", "", "| Robots | Profile | Variant | Downstream vs whole | p95 checkpoint ms | p95 chassis ms | p95 projectile error m | Missing projectiles | p95 Pong ms |", "|---|---|---|---:|---:|---:|---:|---:|---:|"]
for p,n in blocks:
    for v in variants:
        r=rows[p,n,v];x=objectives(r);b=objectives(rows[p,n,"whole"])
        lines.append(f"| {p} | {n} | {v} | {100*(x[0]/b[0]-1):+.2f}% | {x[2]:.0f} | {x[3]:.0f} | {x[7]:.3f} | {100*x[9]:.2f}% | {x[11]:.0f} |")
(ROOT/"summary.md").write_text("\n".join(lines)+"\n")
print(f"Validated {len(rows)} cases; eligible {len(eligible_variants)}; global frontier {global_frontier}; nominations {nominees}; offline gate {approaching}.")
print(f"Elapsed wall time {meta['wall_seconds']:.1f}s with {meta['workers']} workers.")
