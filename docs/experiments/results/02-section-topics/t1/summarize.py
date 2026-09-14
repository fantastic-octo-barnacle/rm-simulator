#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# Copyright (c) 2026 hxyulin <hxyulin@proton.me>
"""Validate T1 raw cases and regenerate compact rows and diagnostic tables."""
import gzip
import hashlib
import json
import sys
from itertools import product
from pathlib import Path

ROOT = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else Path(__file__).resolve().parent
metadata = json.loads((ROOT / "metadata.json").read_text())
assert metadata["complete"]
assert "test result: ok." in (ROOT / "trial-output.txt").read_text()
rows = []
for name, digest in metadata["case_sha256"].items():
    raw = (ROOT / name).read_bytes()
    assert hashlib.sha256(raw).hexdigest() == digest
    row = json.loads(gzip.decompress(raw))
    assert row["warmup_ms"] == 10016 and row["measured_ms"] == 60000
    assert row["shots_launched_measured"] == 1250
    if "compression_environment" in metadata:
        codec = metadata["compression_environment"]["RM_NET_CODEC"]
        assert row["compression"] == {"mode": {"deflate": "Deflate", "zstd": "Zstd", "zstd-dict": "ZstdDictionary"}[codec], "level": 1 if codec == "deflate" else 3}
    diag = row["diagnostics"]
    assert sum(c["sent_payload_bytes"] + c["fragment_header_bytes"] for c in diag["classes"]) + diag["shared_datagram_header_bytes"] == diag["downstream_bytes"] == row["sender_lifetime"]["sent_bytes"]
    for c in diag["classes"]:
        assert c["offered_payload_bytes"] - c["duplicate_suppressed_payload_bytes"] - c["replaced_unsent_payload_bytes"] == c["fully_sent_payload_bytes"] + c["queued_payload_bytes_at_end"]
    assert row["sections"]["peak_queued_payload_bytes"] <= 2 << 20
    assert row["sections"]["peak_partial_frame_bytes"] <= 4 << 20
    assert row["upstream_lifetime_bytes_datagrams"][0] <= 10240 * 70.016
    assert diag["downstream_bytes"] <= row["downstream_budget_bytes_s"] * 70.016
    for path in ["whole_controls", "section_controls"]:
        assert row[path]["confirmation_before_pong"]
        assert row[path]["pending_confirmations_at_end"] == row[path]["pending_pongs_at_end"] == 0
    for event in diag["checkpoint_events"]:
        assert event["source_capture_ms"] <= event["encoded_ms"] <= event["first_manifest_ms"] <= event["usable_ms"]
    # Compact rows omit only detailed events, which remain in the hashed raw case.
    del diag["transfer_events"], diag["checkpoint_events"]
    rows.append(row)
assert {(r["players"], r["profile"]) for r in rows} == set(product([2, 12], ["clean", "limited", "blackout"]))
assert len(rows) == 6
for players in [2, 12]:
    assert len({r["captured_stream_sha256"] for r in rows if r["players"] == players}) == 1
for profile in ["clean", "limited", "blackout"]:
    assert len({r["impairment_sha256"] for r in rows if r["profile"] == profile}) == 1
rows.sort(key=lambda r: (r["players"], r["profile"]))
(ROOT / "runs.jsonl").write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in rows))
print("robots profile | bytes change | p95 checkpoint/chassis ms whole -> section | p95 Pong ms whole -> section")
for r in rows:
    a, b = r["whole"], r["sections"]
    print(r["players"], r["profile"], f"{r['downstream_change_percent']:+.2f}%",
          a["checkpoint_age_ms"]["p95"], b["checkpoint_age_ms"]["p95"],
          a["available_chassis_age_ms"]["p95"], b["available_chassis_age_ms"]["p95"],
          r["whole_controls"]["pong_latency_ms"]["p95"], r["section_controls"]["pong_latency_ms"]["p95"])
    d = r["diagnostics"]
    triggers = [(c["name"], n) for c, n in zip(d["classes"], d["checkpoint_completion_trigger_counts"]) if n]
    print("  completion triggers:", triggers, "encode delay p95:", d["source_capture_to_encode_ms"]["p95"])
    for c in d["classes"]:
        if c["sent_payload_bytes"] > 10000:
            print(" ", c["name"], "KiB:", round((c["sent_payload_bytes"] + c["fragment_header_bytes"])/1024, 1),
                  "queue wait p95:", c["enqueue_to_first_send_ms"]["p95"],
                  "service gap max:", c["max_backlogged_service_gap_ms"],
                  "discarded partial KiB:", round(c["discarded_partial_received_payload_bytes"]/1024, 1))
    print("  blackout recovery:", r["blackout_recovery_ms_checkpoint_chassis_projectiles"])
print("Validated six cases, byte ledgers, coverage metadata, controls and trace hashes.")
