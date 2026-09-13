<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Bandwidth integration follow-up

Short local validation of experiments 1 and 7 together (protocol 29), following
`5c4891a`. Event measurements were committed as `f31160e`; the subsequent tracing
classification correction is `e678491`. These are smoke trials, not the full
multi-seed acceptance matrix. Matching protocol 27 baseline binaries came from
`3a41fcc` with identical event measurement instrumentation applied. Release
binaries were built before timing and identified by hashes in the local reports.

## RTT check

The first eight-second pair exceeded the 25% maximum-RTT comparison threshold
(9.12 → 11.70 ms), with only eight/seven individual probes. A 22-second repeat
reversed the run order (current first) and obtained 21 probes per build:

| Metric | Baseline | Integrated |
|---|---:|---:|
| RTT median | 4.42 ms | 4.66 ms |
| RTT p95 | 7.17 ms | 7.53 ms |
| RTT maximum | 7.60 ms | 7.89 ms |
| Shot confirmation p95 | 58.82 ms | 59.15 ms |
| Complete-checkpoint interval p95 | 36.37 ms | 36.66 ms |

The RTT regression did not repeat. This is evidence against a consistent
regression, not a guarantee across machines or links. The old generic relative
comparison also flagged near-zero numerical correction errors; those historical
failed comparisons remain recorded, not relabelled as passing.

## Why the original armor workload failed

The two clients spawned on opposite halves at x=±9 m. Strafing moved them in
opposite world-y directions; auto aim acquired a rune for one pilot and searched
or reported no shot for the other. Extending a straight forward drive from 3.5 to
5 seconds left both chassis at x≈±5.10 m. An offline clearance probe using the
verified CAD collision meshes confirmed that the raised central terrain blocked
the low cross-field armor sightlines. This was not evidence of lost fire inputs.

The controlled robot-armor probe uses two real clients in adjacent red-team spawn
slots, normal backward driving, and manual fire. It changes neither placement
permissions nor scoring rules. A Python controller reads the actual chassis
poses, chooses the nearest armor face and updates aim while the chassis settles
from its turn. Same-team manual fire deliberately isolates robot scoring and
network delivery; it does not validate enemy auto-aim acquisition.

Temporary probe assertions were corrected before the final run: the transient
snapshot hit list is not a cumulative history; received contact events include
rejected strikes; and startup underruns must not be counted as active-workload
underruns. Earlier raw failed reports are retained. The probe checks cumulative
hits, target HP, confirmed launches, displayed detected hits and active counter
deltas instead.

The final corrected run **passed**: each pilot confirmed eight launches and
scored eight hits on the other robot (16 detected hits total). Each target went
from 200 to 120 HP. Both clients displayed all 16 detected hits, with zero
unresolved outcomes and zero new presentation underruns during the active
workload. Both clients moved before firing. This confirms a populated
robot-armor workload; it is not an end-to-end GPU scanout latency measurement.

## Setup and reconnect under impairment

A separate Python probe uses the production harness process/console/proxy helpers
but arms impairment before the first UDP packet. Each connection sees a 400 ms
initial blackout, 25 ms delay plus up to 5 ms jitter and 10% loss in both
directions. Two successive connections use seeds 2026/2027 and the same pilot
name. Both baseline and integrated builds passed:

| Build | Initial/rejoin readiness | Owner anchors observed | Chassis IDs |
|---|---|---|---|
| Baseline | 5.79 / 1.94 s | 194 / 117 | 0 → 1 |
| Integrated | 2.45 / 1.94 s | 108 / 114 | 0 → 1 |

Each connection received complete checkpoints, drove more than 1.9 m, stopped
below 0.1 m/s after release, and had its chassis removed after departure.
Readiness includes process and scene startup; anchor counts cover different
elapsed intervals and are not rate comparisons.

An initial integrated run exceeded an ad hoc five-second departure deadline on
its second connection. GNS itself has a five-second connected timeout, so the
matched follow-up allowed two seconds of scheduling grace. Both matched runs
removed each chassis within 16 ms after process exit. The initial timeout is
retained as a limitation; no transport setting was changed to obtain the result.
Random loss exercises setup but does not prove that a particular configuration
or acknowledgement packet was dropped. Targeted configuration/ACK-loss recovery
remains covered by the deterministic codec test, not a packet-specific GNS trial.

## Tracing correction and remaining work

The investigation found that network tracing recognized only RMO3/RMI2, causing
RMO4 owner anchors and RMI3 inputs to be labelled `control`. `e678491` recognizes
both old and new formats and adds a regression test. Total bytes were unchanged;
pre-fix per-class counters must not be used as attribution evidence. `just verify`
passed, including the new test.

NET-001 remains open: the bandwidth target has not been met. NET-003 remains open:
the matched blackout firing trial produced six/seven unresolved outcomes on
baseline/integrated builds. Continue to keep that failed gate; the setup/rejoin
smoke does not test firing during a blackout. Broader seeds, constrained capacity,
multiple concurrent handshakes and manual playability review remain outstanding.

## Local artifacts

Raw captures and scratch investigation scripts are outside Git:

- RTT: `/tmp/rm-rtt-baseline-repeat`, `/tmp/rm-rtt-current-repeat`.
- Initial departure timeout: `/tmp/rm-setup-reconnect-run`.
- Matched setup/rejoin: `/tmp/rm-setup-reconnect-baseline`,
  `/tmp/rm-setup-reconnect-current`; script `/tmp/rm-setup-reconnect.py`.
- Controlled armor: `/tmp/rm-adjacent-robot-armor-acceptance`; script
  `/tmp/rm-robot-armor-probe.py`.
- Clearance sweep: `/tmp/rm-clearance-probe.rs`, `/tmp/rm-clearance-ground.log`.

The temporary scripts take `--binaries` and `--output`, retain JSON state and
hashes, and clean up their owned processes. These paths are local evidence, not
permanent download URLs.
