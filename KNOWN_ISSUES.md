<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Known issues

Networking issues still open as of 13 September 2026, with the protocol 29
bandwidth integration. Measurements below retain their original revisions.
This list covers the current networking investigation, not every limitation of
competition-rule enforcement. Fixed hit delivery, auto-aim sampling, lobby
compatibility and local TCP overhead are recorded in [CHANGELOG.md](CHANGELOG.md).

## NET-001: Downstream traffic exceeds the bandwidth target

**Status: open.** A listen host with one remote client on the same machine
averaged approximately 103 kbps upstream and 855 kbps downstream during the
12-second movement/firing trial. These are UDP proxy payload rates, excluding
IP/UDP headers. Including the proxy's header allowance gives approximately
115 kbps upstream and 882 kbps downstream. Rates are decimal kilobits per second,
not kilobytes per second.

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
on-wire result. See the [experiment record](docs/bandwidth-experiments.md) and
[original UDP trial](docs/bandwidth-results/harness-trial-exp1.md).

Next, validate the combined build under real UDP loss, delay, blackouts and
bandwidth caps, including lost configuration acknowledgements, reconnect and lost
input releases. Run matched multi-seed and multi-client trials with populated hit
and shot-result histories. Then trial 31.25 Hz owner / 15.625 Hz world cadence
and measure deflate level 4 on the selected stream. Neither change is enabled.

## NET-002: Shot outcomes remain delayed on a fast local connection

**Status: open; attribution incomplete.** The single-player channel smoke test
measured a 0.017 ms p95 command queue wait but approximately 51 ms p95 from client
submission to the first shot outcome published into its inbox. Removing TCP
therefore did not remove the remaining delay.

In the pre-integration listen-host trial, application RTT was 5.95 ms median and 9.78 ms
p95, while session-reported shot confirmation was 56.1 ms median and 57.8 ms p95.
The local inbox trace and session confirmation have different endpoints and must
not be treated as interchangeable measurements. Neither measures click-to-visible
hit latency or projectile flight time.

Investigate the input lead, intended shot execution time, host application and
client consumption stages before changing scheduling. Preserve movement/fire
ordering and simulation-time cadence. Scheduling is a candidate cause, not a
completed attribution of the full delay.

## NET-003: Constrained links develop severe latency and incomplete outcomes

**Status: open.** Earlier protocol 27 UDP proxy trials showed:

| Trial | Application RTT, median / p95 | Shot confirmation, median / p95 | Unresolved shot outcomes |
|---|---:|---:|---:|
| Moderate delay/loss | 96 / 200 ms | 133 / 136 ms | 0 |
| Bandwidth-limited | 1,036 / 2,250 ms | 911 / 1,558 ms | 0 |
| Severe impairment with blackout | 1,732 / 2,590 ms | 1,174 / 3,398 ms | 14 |

These are historical impaired-client results, not reruns of `7d07f1c`.
The profiles and limitations are recorded in the
[network stress investigation](docs/network-stress-2026-09-13.md). Zero unresolved
outcomes does not make a one-second confirmation delay playable. Queue-wait drops
were observed in the constrained trials, but the contribution of each application
and native transport queue still needs measurement.

Use the new traces to locate backlog, then test traffic reduction and pacing
changes under bandwidth caps, loss, jitter and blackouts. Keep reliable outcomes
ordered and preserve world-update progress. Repeat across seeds and client counts;
verify recovery as well as behavior during impairment.

## Measurement gaps and deferred work

- The latest smoke tests establish functionality, not an across-platform or
  multi-seed performance result. Trace-enabled versus trace-disabled overhead
  has not been benchmarked.
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
and the [network harness guide](docs/network-harness-and-stats.md) for repeatable
trials. Keep raw traces, logs and generated measurement dumps outside Git.
