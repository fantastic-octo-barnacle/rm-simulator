<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Known issues

Networking issues still open as of 14 September 2026, with the protocol 29
bandwidth integration. Measurements below retain their original revisions.
This list covers the current networking investigation, not every limitation of
competition-rule enforcement. Fixed hit delivery, auto-aim sampling, lobby
compatibility and local TCP overhead are recorded in [CHANGELOG.md](CHANGELOG.md).

| Id | Issue | Status |
|---|---|---|
| NET-001 | Downstream traffic exceeds the bandwidth target | Open |
| NET-002 | Shot outcomes remain delayed on a fast local connection | Open; attribution incomplete |
| NET-003 | Constrained links develop severe latency and incomplete outcomes | Open |

## NET-001: Downstream traffic exceeds the bandwidth target

**Status: open.**

A listen host with one remote client on the same machine averaged approximately
103 kbps upstream and 855 kbps downstream during the 12-second movement/firing
trial. These are UDP proxy payload rates, excluding IP/UDP headers. Including
the proxy's header allowance gives approximately 115 kbps upstream and
882 kbps downstream. Rates are decimal kilobits per second, not kilobytes per
second.

The desired budget is 100–200 kbps per player. Downstream traffic needs roughly
a 4–9× reduction to meet that budget in this workload. Earlier PR #2 trials
measured about 804–840 kbps downstream; the tracing/channel work has not
established a remote bandwidth improvement. Raising LAN allowances relieved
queue pressure but did not reduce the data produced.

Protocol 29 integrates experiments 1 and 7: acknowledged owner configuration
references (RMO4) and lossless compact input batches (RMI3), plus the attribution
probe. Cadence, baseline rotation, precision, outcome recovery and pacing defaults
are unchanged. These changes mitigate this issue; they do not close it.

The isolated experiment 1 real-UDP pair measured 707.3 → 612.6 kbps downstream
(−13.4%); the candidate was 631.4 kbps including the proxy header allowance.
Experiment 7 measured 76.4 → 59.6 kbps upstream under firing in the in-process
probe. These are different trials, not a measured combined total or a protocol 29
on-wire result. See the [experiment record](docs/bandwidth-experiments.md).

The subsequent five-seed, two-client clean matrix measured the current
owner-configuration-reference build at 1,255.0 kbps downstream per client,
710.0 kbps with 31.25 Hz owner / 15.625 Hz world cadence, and 662.0 kbps after
also changing checkpoint deflate from level 1 to 4 (historical: DEFLATE was
removed in protocol 35). These include a 28-byte
IPv4/UDP allowance per packet. They are a sustained two-client workload, not a
rerun of the historical one-client trial above. The combined experimental
reduction is 47.3%, but still exceeds 200 kbps by more than threefold. Remote
presentation age also increases. Neither cadence change is enabled in production,
and the deflate change no longer applies; see the [experiment record](docs/bandwidth-experiments.md).

Capacity trials reject acceptance: the combined candidate's 200 kbps trials
had 4.1–4.9 s shot-confirmation p95 and long complete-context stalls. The 100 kbps
profile also caused console-timeout aborts. Preserve these failures and continue
validation of populated robot-hit recovery, packet-specific configuration/ACK
loss, reconnect, releases and broader client/platform loads before acceptance.

## NET-002: Shot outcomes remain delayed on a fast local connection

**Status: open; attribution incomplete.**

The single-player channel smoke test measured a 0.017 ms p95 command queue wait
but approximately 51 ms p95 from client submission to the first shot outcome
published into its inbox. Removing TCP therefore did not remove the remaining
delay.

In the pre-integration listen-host trial, application RTT was 5.95 ms median and
9.78 ms p95, while session-reported shot confirmation was 56.1 ms median and
57.8 ms p95. The local inbox trace and session confirmation have different
endpoints and must not be treated as interchangeable measurements. Neither
measures click-to-visible hit latency or projectile flight time.

Investigate the input lead, intended shot execution time, host application and
client consumption stages before changing scheduling. Preserve movement/fire
ordering and simulation-time cadence. Scheduling is a candidate cause, not a
completed attribution of the full delay.

## NET-003: Constrained links develop severe latency and incomplete outcomes

**Status: open.**

Earlier protocol 27 UDP proxy trials showed:

| Trial | Application RTT, median / p95 | Shot confirmation, median / p95 | Unresolved shot outcomes |
|---|---:|---:|---:|
| Moderate delay/loss | 96 / 200 ms | 133 / 136 ms | 0 |
| Bandwidth-limited | 1,036 / 2,250 ms | 911 / 1,558 ms | 0 |
| Severe impairment with blackout | 1,732 / 2,590 ms | 1,174 / 3,398 ms | 14 |

These are historical impaired-client results, not reruns of `7d07f1c`.
The impairment profiles live in `scripts/network-scenarios/`; their trial logs
are retained in Git history. Zero unresolved outcomes does not make a
one-second confirmation delay playable. Queue-wait drops were observed in the
constrained trials, but the contribution of each application and native
transport queue still needs measurement.

### Matched short blackout trial after bandwidth integration

The failure also occurs in the baseline. A matched real-UDP two-client trial
used baseline `3a41fcc` (protocol 27) and the integrated bandwidth changes at
`5c4891a` (protocol 29), both with the same event measurement instrumentation
subsequently committed as `f31160e`. Each run used seed 2026, 1 s warmup,
12 s active and 3 s recovery. One client had a clean link; the other had 35 ms
per-direction delay plus up to 15 ms jitter, 2% loss, downstream reordering and
duplication, and a 500 ms blackout starting at 3 s. Movement was released during
the blackout; firing resumed afterward. Setup and warmup were unimpaired.

| Impaired-client metric | Baseline | Integrated |
|---|---:|---:|
| Unresolved shot outcomes | 6 | 7 |
| Longest complete-checkpoint arrival interval | 593 ms | 590 ms |
| Per-shot confirmation p95 | 157 ms | 170 ms |
| Presentation underruns | 10 | 10 |

Both runs failed the zero-unresolved-outcomes gate. Counts remained 6 and 7 at
the final recovery sample; both healthy clients had zero unresolved outcomes.
No event-history samples were lost. The evidence establishes an existing
blackout-recovery failure, but one run per build cannot establish whether the
one-outcome difference is meaningful or identify the cause. An unresolved local
outcome does not by itself establish that an authoritative hit was lost.

This is **not a blocker to continuing the remaining bandwidth trials**. Keep
NET-003 open and preserve the failed gate; reproducing it on the baseline does
not turn either trial into a pass or establish impaired-link acceptance. Short
targeted robot-armor and impaired setup/rejoin probes are retained in Git history.
Packet-specific setup/ACK loss over GNS and broader recovery validation remain
outstanding; random setup loss is not proof of dropping a particular ACK.

Raw artifacts remain outside Git in `/tmp/rm-short-two-client-blackout-baseline`
and `/tmp/rm-short-two-client-blackout-run`; each contains the scenario, binary
hashes, samples and summary. They are local captures, not permanent report URLs.

Use the new traces to locate backlog, then test traffic reduction and pacing
changes under bandwidth caps, loss, jitter and blackouts. Keep reliable outcomes
ordered and preserve world-update progress. Repeat across seeds and client counts;
verify recovery as well as behavior during impairment.

## Measurement gaps and deferred work

- The historical five-seed cadence/deflate matrix covers two rendered clients on one
  Apple M3 Pro. Cross-platform and twelve-real-client performance remain
  unmeasured. Trace-enabled versus trace-disabled overhead has not been benchmarked.
- The listen-host trial measured remote presentation age at 52 ms median and
  61 ms p95. That is deliberate buffering, not packet RTT. Its responsiveness
  versus jitter tradeoff still needs broader validation; it is not by itself
  proof of an interpolation bug.
- End-to-end click-to-visible-hit latency and hit disagreement under impairment
  have not been measured. Shot confirmation acknowledges a launch outcome, not
  an armor impact.
- Historical hit rewind remains deferred until delivery, scheduling and bandwidth
  work have been evaluated. It is not an approved fix for these issues yet.

See [network tracing](docs/network-tracing.md) for capture and summary commands
and `scripts/network-scenarios/` for repeatable
trials. Keep raw traces, logs and generated measurement dumps outside Git.
