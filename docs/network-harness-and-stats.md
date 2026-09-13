<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Network harness and in-game stats design

Status checked 12 September 2026. The harness, local stats overlay and input
execution feedback are implemented. Live protocol is 26; protocol 17/18 references
below identify earlier measurements or original design slices. Exact-tick owner correction measurements use
a bounded prediction history. The sections below retain the broader measurement
design; complete per-event traces, hit disagreement and measured input-to-photon
latency remain unavailable. See the [roadmap](multiplayer-networking.md).

## Run the implemented harness

Requires Python 3.9 or newer, the external CAD package and explicitly built matching
server/app binaries. The runner never builds them or changes system networking.
The standalone tests need neither Rust binaries nor CAD:

```sh
just network-test
just network-trial scripts/network-scenarios/smoke-single.json --output /tmp/rm-network-smoke --screenshot
```

Pass `--network-stats compact` or `--network-stats detailed` to include the player
overlay in a trial. The manifest records `RM_NET_UP_KIB_S`, `RM_NET_DOWN_KIB_S`
and `RM_NET_FULL_CHECKPOINTS` when set.

The output directory must not already exist. The single-client smoke run measures
eight seconds of driving/firing with a 256 kbps upstream cap, 512 kbps downstream
cap, delay/jitter and a half-second upstream blackout, followed by two seconds of
released controls. It verifies operation, not good gameplay on that connection.
For a direct one-client comparison, use `direct-single.json`.
For two rendered clients, use `smoke.json`. The `baseline.json`, `constrained.json`
and `burst-recovery.json` scenarios run 60 seconds with a healthy comparison peer.
The default client remains headless but still uses the GPU and full CAD.

```sh
python3 scripts/network-harness.py scripts/network-scenarios/constrained.json --validate
just network-trial scripts/network-scenarios/baseline.json --output /tmp/rm-network-baseline --seed 19
just network-trial scripts/network-scenarios/burst-recovery.json --output /tmp/rm-network-manual --manual
```

Manual mode opens the first client visibly and suppresses scripted inputs for all
clients; it still ends after the scenario duration and recovery period. Pass
`--server`, `--app`, `--cad-assets` and `--build-label` to compare explicit builds.
The manifest records hashes, checkout version/dirty status, selected renderer
environment variables, effective profiles, process arguments and allocated ports.
The protocol source version describes the checkout; binary hashes identify builds.
The game's normal handshake rejects incompatible client/server binaries.

Setup and warmup are unimpeded so assets load and clients join before the trial.
Impairments and scenario events start together afterward. These runs therefore
include the transition into the configured network conditions; they do not test
an impaired handshake or assume timing estimates have already settled. Recovery
releases controls while retaining the link profile. Use finite blackout intervals
or `rate_changes` to explicitly restore the link. The standalone proxy starts its
impairment clock on the first packet and can test startup impairment separately.

Scenario JSON accepts `schema_version: 1`, name, integer seed, warmup/duration/recovery
seconds, sample Hz, one to twelve clients, ordered events, an optional shared
downstream bottleneck and an optional `expectations` block. Each client has a unique
name, team and upstream/downstream profile objects. Events contain `at_s`, client name
and an existing console command. Supported commands are capture, key, mouse_button,
mouse_motion, release_inputs, inspect, camera, spawn and pause. See the checked-in
scenarios for complete examples.

### Expectations and run status

`expectations` turns a trial into an assertion. It holds an optional
`baseline_tolerance` and 1 to 64 named checks, each a comparison against one value
in `summary.json`:

```json
"expectations": {
  "baseline_tolerance": 0.25,
  "checks": [
    {"name": "no_unresolved_shot_outcomes", "metric": "unresolved_shot_outcomes_delta",
     "op": "==", "value": 0, "per_client": true},
    {"name": "checkpoint_gap_bounded", "metric": "checkpoint_gap_ms.max",
     "op": "<=", "value": 10000, "per_client": true, "on_missing": "skip"},
    {"name": "impaired_peer_kept_sampling", "metric": "active_samples",
     "op": ">=", "value": 120, "client": "impaired"}
  ]
}
```

`metric` is a dotted path. A distribution statistic is `<metric>.<p50|p95|p99|max|count>`
(`metrics.` may be written out); anything else names a scalar such as
`active_samples`, `sample_gaps` or a `_delta` counter. `op` is one of `==`, `!=`,
`<`, `<=`, `>`, `>=`. `per_client: true` evaluates the check against every client and
produces one result row each; `client: NAME` evaluates one named client; neither means
the path is read from the summary root (`sample_gap_total`, `proxy_rates.*`). A metric
the run did not collect fails the check unless `on_missing: "skip"`. There is no
expression language: every check is one operator, one path, one number.

Results land in `summary.json` under `checks` (name, client, metric, operator,
threshold, observed value, `passed`/`failed`/`skipped` and a short detail) and are
also tabulated in `summary.md`. The top-level `status` is:

| Status | Meaning | Exit code |
|---|---|---:|
| `passed` | Ran, and every configured check and baseline comparison held | 0 |
| `completed` | Ran with no expectations and no baseline; nothing was asserted | 0 |
| `failed` | A process, the proxy, cleanup, a check or a baseline comparison failed | 1 |

All checked-in scenarios carry the correctness gates the console can currently
support: `unresolved_shot_outcomes_delta == 0`, a generous `checkpoint_gap_ms.max`
bound, a generous `remote_underruns_delta` bound, a minimum active sample count and
a maximum sample-gap count. They are liveness and correctness gates, not playability
thresholds; freeze numeric playability limits only after a recorded baseline.

### Baseline comparison

`--baseline PATH` loads a prior `summary.json` and compares every distribution
statistic present on both sides, per client. A statistic that grew by more than the
relative tolerance is a regression and fails the run. Larger is worse for every
statistic the harness keeps, byte rates included: a run that needs more bandwidth for
the same workload has regressed, and one that needs less has not. A statistic missing
on either side is skipped, never failed. The tolerance comes from
`--baseline-tolerance`, else the scenario's `baseline_tolerance`, else 0.25; a
baseline statistic of zero admits no growth at all. The comparison is recorded under
`baseline` in `summary.json` with the path, tolerance, compared/skipped counts and
every row.

```sh
just network-trial scripts/network-scenarios/baseline.json --output /tmp/rm-network-2 \
    --baseline /tmp/rm-network-1/summary.json --baseline-tolerance 0.2
```

| Profile field | Implemented meaning |
|---|---|
| `delay_ms`, `jitter_ms` | Base propagation delay plus uniform extra jitter |
| `loss_percent` | Independent random path loss after serialization |
| `rate_kbps` | Decimal kilobits/s; null means unlimited |
| `queue_bytes`, `queue_wait_ms` | Byte cap including modeled headers; reject new admission if its predicted queue wait exceeds the limit |
| `overhead_bytes` | Header allowance charged per packet, default 28 for IPv4/UDP |
| `duplicate_percent` | Probability of a second copy, which consumes queue space and bandwidth |
| `reorder_percent`, `reorder_delay_ms` | Probability and extra delay for a selected packet; ordinary jitter can also reorder packets |
| `burst_good_ms`, `burst_bad_ms`, `burst_loss_percent` | Exponential good/bad durations, starting good; bad-state loss defaults to 100% |
| `blackouts` | Array of `[start_s, duration_s]` pairs, relative to active trial start |
| `rate_changes` | Ordered objects with `at_s` and `rate_kbps`; affect new reservations while existing ones keep their schedule |

The shared downstream hop accepts rate and queue fields only and serializes before
each client's downstream link. Packets admitted during blackouts are discarded.
In-flight packets whose modeled transit overlaps a blackout are discarded too;
they never reappear as a recovery burst. Previously reserved service remains
charged. Loss, jitter, duplication, reordering and burst state use independent
seeded random streams per direction and peer.

The old proxy flags remain available, with directional overrides:

```sh
python3 scripts/network-impairment.py --transport udp --listen-port 7841 --target-port 7840 --delay-ms 25 --jitter-ms 15 --upstream-rate-kbps 128 --downstream-rate-kbps 512 --loss-percent 1
```

`--profile FILE` accepts upstream/downstream objects with the fields above. Explicit
directional CLI values override the file, which overrides symmetric CLI defaults.
TCP retains its existing line-delay model and rejects UDP-specific loss/rate flags.

Artifacts are `manifest.json`, `samples.jsonl`, `events.jsonl`, `proxy.jsonl`, process
logs, `summary.json`, `summary.md` and optional screenshots. Rates use actual
timestamps, not assumed reporting intervals. Reports separate application-probe
RTT, missing checkpoints, local rejections and proxy drop reasons. Shot counters
in current snapshots are global, not per-player launch confirmations. Console
commands can wait for existing barriers; events record actual send/completion
times and reports include console query durations. A trace's requested event time
is not proof that the input executed at that instant.

Each log is capped at 64 MiB with dropped-record/byte counts. Summaries retain at
most 10,000 reduced client samples and 4,096 proxy snapshots, and report overwritten
client samples. These limits bound long runs; preserve raw logs when assessing
truncated runs. p95 requires at least 20 observations and p99 at least 100. The
`unavailable` list is computed from what the run actually collected, plus the labels
no source publishes at all: hit disagreement, input-to-photon latency, a complete
per-event input trace and per-client shots fired. Shot counters remain global; the
console exposes no per-player launch counter to make them per client.

Sampling is scheduled against the planned instant rather than the previous reply, so
the effective rate matches `sample_hz`. A console query that misses its deadline
(`--console-timeout`, default 2 s) records a sample gap instead of ending the trial:
gaps appear per client as `sample_gaps`, for the run as `sample_gap_total`, and in
`events.jsonl`. Skipped sample slots are counted as `sample_schedule_skips`. Five
consecutive failed queries on one console still fail the run. Passed means the
configured expectations held; completed means the scenario ran with none configured.
Neither is a full gameplay correctness or playability verdict.

Verification on macOS used the actual CAD and unchanged protocol 17 binaries.
All 52 standalone tests passed, including real UDP pacing, lifecycle failures,
expectation evaluation, baseline comparison and console sample gaps.
Direct and constrained single-client runs completed; the constrained run serviced
about 63.7 kB/s downstream including modeled headers against a 512 kbps cap of
64 kB/s, and recorded checkpoint gaps up to 2.78 seconds. This exposes existing
network behavior, not an improvement to it. Two simultaneous rendered clients
hit a Metal counter-buffer allocation/device-loss error on this machine. The
harness captured failure and cleaned up. Multi-client process orchestration,
shared caps, startup failure, interruption and port cleanup are covered with
lightweight console/UDP fixtures; these fixtures do not simulate GNS or physics.
Multi-rendered-client GPU validation remains outstanding on a suitable machine.

The sections below retain the design for shared telemetry, the in-game display
and further acceptance work. Full per-shot timing, same-tick correction error,
kernel emulation and the new player controls are not part of this implementation.

## One measurement path for players and tests

Collect transport counters on network workers, execution counters on the host,
and prediction/presentation measurements on their existing workers. Publish a
bounded typed stats snapshot to the app. The HUD, console and harness consume
that same snapshot. A console read or opening a panel must not request a full
server snapshot, capture collision geometry, rebuild physics or change gameplay.

Local offline play reports Local and leaves network-only values unavailable.
TCP reports the measurements it supports; it must not claim zero packet loss
because its ordered stream hides packet recovery. Remote UDP exposes native
transport estimates only with their actual definitions and supported fields.

## Player display

Keep the existing small warning toast, visible even when stats are off. The
Network stats control cycles Off, Compact and Detailed modes in P settings and F3. The display remains non-modal and does not capture drive/fire
input. Startup option: `--network-stats off|compact|detailed`, default
Off. These controls are implemented; no new key binding is required.

Compact mode occupies at most two lines below the scoreboard, clear of the toast,
minimap and aiming region. Use readable text and units; color supplements labels.
An illustrative layout, with invented values:

```text
Ping 82 ms   Loss est. in 2% / out --
Receive 46 KiB/s   Send 9 KiB/s   Updates 31/s
```

Ping means transport RTT. Until available, explicitly show App RTT for the
existing reliable probe instead. Loss is shown only if the provider supplies a
usable estimate; otherwise show --. A tooltip or detail label gives the source and
window. Do not convert a generic connection-quality score into a loss percentage.

Detailed mode adds a bounded 60-second history for RTT, receive rate, update gaps
and correction size, plus current input lead, remote interpolation delay,
prediction backlog, send queue delay, late inputs and shot outcomes. Put FPS/frame
time beside these so a rendering slowdown is distinguishable from late delivery.
Expose textual latest/p95 values with the graphs. Gaps in measurement draw gaps,
not zeros. Keep protocol revisions and raw IDs in the developer detail view.

Existing warning states stay distinct:

| State | Evidence and behavior |
|---|---|
| High latency | Current behavior uses response delay of at least 200 ms; detail identifies reliable app RTT versus a pending-probe lower bound |
| Connection interrupted | No new checkpoint for at least 250 ms; silence does not prove the connection closed |
| Disconnected | Transport confirms closure; retain the window and mark previous stats stale |
| Local | Embedded offline host; network rates and loss are not applicable |

High loss and congestion can be added as short warning reasons once their metrics
are validated. Initial trial thresholds are loss estimate over 5% or send queue
delay over 50 ms sustained for two seconds, clearing only after five seconds below
half those thresholds. These are tuning candidates. Never infer a cause merely
from a large correction. Prefer one warning with precedence Disconnected,
Connection interrupted, then the measured degradation reason. Preserve other
reasons in details. A paused simulation must not be classified as disconnected
because its simulation tick is unchanged; track host-update receipt separately.

## Measurement contract

Propose a versioned `NetworkStats` DTO in the server crate, composed with local
prediction and frame metrics in the app. It is a diagnostics contract, separate
from authoritative `SimulationState`. The same serialized representation appears
under console `state.network` and in report samples. Preserve existing console
fields while adding clearly named replacements; document later removals.

Every sample carries schema version, connection generation, transport, local
monotonic sample time, observation-window duration and freshness. Numeric fields
use explicit units. Unsupported or unsampled values are null with a reason,
not zero. Counters reset on a new connection generation. Life/epoch transitions
segment input and correction statistics, and counter resets never produce negative
rates. Sample counts accompany percentiles. Do not average per-window percentiles
to calculate a run percentile; retain bounded histograms or raw event samples.

| Measurement | Definition and source |
|---|---|
| Transport RTT, ms | Native transport estimate, with its provider/window; not one-way delay |
| App response RTT, ms | Local send-to-receipt time for a correlated probe, including queues, server work and retries |
| Pending response age, ms | Elapsed local time for an unanswered probe, a lower bound rather than a completed RTT |
| Arrival variation, ms | Variation of arrival intervals relative to source send intervals, with source/window stated; separate from RTT variation |
| Send/receive rate, bytes/s | Deltas of cumulative transport byte counters over actual local elapsed time; specify whether native accounting includes framing/retries |
| Application bytes by class | Encoded bytes submitted and decoded bytes received, separately; include compact/full/delta, inputs, shots, results, control and telemetry |
| Loss estimate, directional | Provider-defined estimate if available; include window/age and which endpoint observed it |
| Message/fragment failures | Separate incomplete expiry, stale revision discard, decode failure and local congestion drop counters |
| Update delivery | Source send rate if known, complete receive rate, apply rate, receipt gap and time since last usable owner correction |
| State age estimate, ms | Estimated server time minus source tick, carrying clock uncertainty and pause state; not receipt gap |
| Send backlog | Pending reliable/unreliable bytes, estimated native queue time and age of oldest application item |
| Input delivery | Sequence sampled/received/applied, intended and actual tick, late/missing counts and stop-lease expiry |
| Input arrival margin, ms | Intended simulation tick minus host tick at receipt; negative means late |
| Prediction | Target tick, completed tick, backlog, continuation limit, context age and worker duration |
| Corrections | Same-tick pre-reconciliation position/angle/velocity/aim differences, count and distribution; visible camera adjustment is separate |
| Remote presentation | Buffer delay, source age, underrun duration, extrapolation duration and hold duration |
| Shots | IDs attempted, received if acknowledged, executed, rejected by reason, unresolved and locally expired; retry count and execution/confirmation delay |
| Host/client load | Host advance/broadcast work, simulation lag, frame time and prediction work; distinguish CPU delay from network delay |

Missing application revisions are not measured packet loss: the server may coalesce
updates before sending, GNS may packetize them differently, and the client may
intentionally discard stale state. Repeated input samples likewise are not extra
player commands. Count logical inputs and wire messages separately.

For correction error, retain a bounded ring of predicted robot states keyed by
exact simulation tick and epoch, initially two seconds of lightweight 1 ms robot
state with a fixed memory cap. Compare an incoming checkpoint against the matching
pre-correction state, never against the current rendered pose. If history was
revised, tag the prediction generation. If absent, count an unavailable comparison.
Do not subtract transforms sampled at different ticks or smooth the measured error.
Retain no CAD meshes or full world snapshots in this metrics ring.

For shot metrics, distinguish intended-to-executed tick difference from local
intent-to-confirmation elapsed time. Cross-machine timestamps need an explicit
mapping and uncertainty; never subtract unrelated monotonic clocks. A missing
confirmation is unresolved until a defined deadline, not proof the server never
fired. Record authoritative and predicted contacts by shot/target/life identity to
measure disagreement separately from launch acceptance. Verify that tracing can
observe local contacts without restoring their removed visual markers.

## Collection and overhead

- Network workers sample native connection status at most four times per second.
  Event counters increment where events already occur. Reuse status queries already
  needed for congestion decisions. Verify native field semantics against the linked
  GNS version; [Valve's types documentation](https://partner.steamgames.com/doc/api/steamnetworkingtypes)
  is a reference, not a guarantee every field is exposed by the Rust wrapper.
- Publish through a latest-value slot or bounded queue. The app reads without
  waiting on transport or host work. HUD text/graph data refresh at four Hz and
  only update changed values. Store 240 display samples for 60 seconds.
- Derive one-second rates from cumulative counters, using real elapsed time.
  Detailed event histograms use a stated longer window. Keep RTT/provider windows
  explicit even when they differ from display windows.
- Locally collected stats require no extra network traffic. Additional server
  feedback is a compact per-peer aggregate, piggybacked at most once per second
  with an initial payload budget of 256 bytes/s. It is replaceable, lower priority
  than gameplay and counted as telemetry. Per-shot IDs use gameplay outcomes where
  available. Full event traces stay local to each process during harness runs.
- Server-only observations enter through typed host requests/publications. Socket
  workers never borrow the Simulation. World code supplies explicit-tick facts and
  never reads a host clock. Render remains passive; app owns HUD adaptation.
- Basic bounded counters remain available with the overlay hidden. Full traces are
  opt-in, written by a bounded worker; when full, drop diagnostic records and count
  drops instead of blocking gameplay. Limit file size/duration. No automatic upload.
- Benchmark Off, Compact and Detailed modes. Initial overhead targets are under
  0.2 ms p95 additional frame-facing work and under 2 MiB retained stats memory per
  client. These are acceptance candidates to validate on the baseline machine.

New on-wire timing/feedback fields require a protocol version change. Initial
local instrumentation can run on protocol 17 with unavailable server fields marked
honestly. Do not require all future replication changes just to ship the display.

## Harness structure

Use three layers, sharing scenarios where their time models allow it:

1. Deterministic Rust tests exercise production input scheduling, codecs, outboxes,
   acknowledgement/baseline recovery and shot lifecycle with explicit ticks and
   scripted delivery. They test invariants, not native GNS timing or visual feel.
2. A local Python runner launches the real server, proxies and clients. Extend
   `scripts/network-impairment.py`; add a small runner for process lifecycle,
   scenarios and reports. Use rendered clients plus the existing console for local
   prediction and HUD tests. Lightweight protocol clients may add host load but
   cannot stand in for rendered-client responsiveness measurements.
3. Manual play mode launches a visible client through the same scenario and leaves
   its stats available. Later validate representative cases with isolated Linux
   netem and separate machines. Native congestion control, OS scheduling and shared
   CPU/GPU contention are outside deterministic-test guarantees.

Each client gets its own proxy endpoint and independent directional random streams.
Keep one healthy control peer in multiplayer runs. Model both per-peer access
bottlenecks and a shared server egress budget; otherwise many-player server
saturation is missed. Compare a healthy peer against its own matched-load baseline.

The runner allocates isolated ports, waits for ready signals with timeouts, records
startup separately, and stops only processes it launched. Graceful stop has a
bounded escalation. Clean up on failures and interruption. Reuse explicitly
selected builds and check protocol compatibility. Do not silently rebuild, clean,
change system network settings or publish anything.

## Impairment model

Each direction specifies base delay and jitter distribution, rate in decimal kbps,
finite queue bytes, maximum queue wait, random loss, burst state, reordering,
duplication and timed blackout intervals. A missing rate cap means unlimited,
not zero capacity. Preserve the old proxy flags as aliases for simple symmetric
profiles. Report the expanded effective configuration.

Start with a FIFO bottleneck whose packet service time is charged from UDP payload
length plus configured IP/UDP overhead. State the assumed IPv4/IPv6 header size;
exclude Ethernet and other overhead unless configured. Serialize at the configured
rate before adding propagation delay/jitter. Queue overflow drops new arrivals;
expired queued packets have a separate drop reason. Do not let packets accumulate
beyond the time/byte limits. Configure queue persistence explicitly across blackout
start/end so outages cannot release an accidental unlimited burst.

Count intentional path loss after bottleneck service in the initial model, so lost
packets still consume link capacity. Duplicates must also be charged for service.
Use independent random streams for each direction and impairment type. For burst
loss, support a seeded good/bad-state model plus exact timed blackouts. Make
reordering and delay distributions explicit. Record whether a loss/queue drop
occurred before or after serialization. This is a documented model, not a claim
that every real router behaves this way.

Validate serialization, queue limits, duplicate charging and transition timing
with synthetic datagrams and a fake clock. Count received, forwarded and dropped
packets/bytes by reason and direction, plus queue age. Record actual elapsed time;
never assume each printed report covers exactly five seconds. Real UDP delivery
and GNS retries pass through the proxy unchanged.

Linux netem offers delay, rate, loss and reordering for later kernel-level checks.
Its timer, offload and TCP placement limitations must be recorded with results.
See the [netem manual](https://man7.org/linux/man-pages/man8/tc-netem.8.html).

## Scenarios and workloads

Version scenario files as JSON. Record seeds, duration, warmup, per-direction
impairments, client roles, initial state, timed inputs and expected invariants.
Use independent workload and network seeds. The following values are experiments,
not supported-network promises. RTT below is base propagation RTT, before jitter
and queuing; loss applies independently in each direction unless overridden.

| Scenario | Base RTT | Additional jitter each way | Loss | Up/down cap |
|---|---:|---:|---:|---:|
| Direct baseline | 0 ms | 0 | 0 | Unlimited |
| Decent | 50 ms | Uniform 0 to 15 ms | 1% | Unlimited |
| Poor | 150 ms | Uniform 0 to 50 ms | 10% | Unlimited |
| Latency sweep | 80 / 200 / 400 ms | 0 | 0 | Unlimited |
| Loss sweep | 80 ms | 0 | 1 / 5 / 10 / 50% | Unlimited |
| Bandwidth sweep | 80 ms | 0 | 0 | 128/512 and 256/1,024 kbps |
| Combined constraints | 150 ms | Uniform 0 to 50 ms | 10% | 128/512 kbps |
| Severe recovery | 300 ms | Uniform 0 to 50 ms | 50% | Unlimited initially |
| Bursts and outages | 80 ms | 0 | 100 to 300 ms bursts; 0.5 / 1 / 2 s outages | Unlimited initially |

Repeat burst/outage cases upstream-only, downstream-only and both. Add unequal
one-way delay, bandwidth changes mid-session, sudden restoration and queue buildup.
Exercise pause, respawn, disconnect and fresh join around those transitions.

Workloads cover idle, straight drive and key release, rapid direction changes,
ramp/wall contact, robot contact, aim sweeps, single shots and sustained firing at
static and moving targets. Include fire while moving and while releasing controls.
Test two real clients first, then 12-player load with client type recorded. Run
selected cases with collider overlay and stats modes on/off to expose observer cost.

After all clients are ready, use at least 10 seconds of warmup, 60 seconds of active
measurement and 10 seconds of recovery for baseline comparisons. Use at least five
network seeds for an experiment; report failures and censored/unresolved events.
Extend runs if there are too few events for tail statistics. A deterministic trace
must replay identically. Native runs with a shared seed need not have identical
packet loss because packetization and scheduling change; compare distributions.

## Reports and gates

Write a manifest, timestamped sample JSONL, event JSONL, process logs and a summary
JSON/Markdown into a caller-selected artifact directory. Include commit/dirty state,
binary hashes, protocol/schema versions, CAD manifest hashes, OS/architecture,
build profile, workload and impairment settings, seeds, process layout and metric
accounting definitions. Keep CAD files external. Optional screenshots/video support
visual review; they do not replace timing data. Never require files in /tmp from a
previous session to reproduce a scenario.

Summaries show p50/p95/p99 where sample counts justify them, maxima, sample counts,
time in degraded states and incomplete/failed runs. Include both per-peer rates and
total server traffic, correction distributions, input execution lateness, release
delay, shot execution/confirmation delay, rejections by reason, hit disagreement,
update gaps, queue age, recovery time and CPU/frame costs. Measure input-to-first
predicted scene publication as a responsiveness proxy; call actual input-to-photon
latency unmeasured unless a suitable capture method is used.

Hard correctness gates apply to every scenario: no duplicate shots/damage/ammo
charges, unauthorized ownership changes, cross-life input, replay of finalized
movement or unbounded queues. A lost release eventually invokes the stop lease.
Future input, queue wait, shot age and prediction continuation stay bounded.
Recovery cannot replay an outage's expired shots. Client closure and process
cleanup must finish within their declared timeouts.

Of those, the checked-in scenarios currently assert what the console publishes:
every launched shot reaches a decided outcome (`unresolved_shot_outcomes_delta == 0`),
checkpoint delivery stays bounded, remote presentation is not starved, and the
clients stayed sampleable. Duplicate shots/damage/ammo charges, ownership changes,
cross-life input and finalized-movement replay have no console-visible counter and
remain unasserted; they need either new diagnostics or the deterministic socket-free
end-to-end test. Process cleanup and declared timeouts are asserted by the standalone
test suite rather than by a scenario check.

Measurement gates require known synthetic rate/delay/loss fixtures to match
counters, hidden/open stats to share definitions, no extra snapshot/debug traffic
when opening the HUD, and no gameplay stall when diagnostics queues fill. Stale,
unavailable and paused values must render correctly. Visually inspect the overlay
at supported window sizes and verify it never blocks input or aiming.

Freeze numeric correction, expiry, recovery and overhead thresholds after the
baseline, before accepting a networking experiment: record a run, then add the
frozen numbers as scenario `expectations` and pass that run's `summary.json` to
`--baseline`. A candidate must improve its target metric without violating the
correctness gates or materially regressing a healthy peer. Severe-loss failures remain visible in reports even when the profile
is classified as recovery-only.

## Original implementation slices (historical)

1. Add typed local counters/freshness and console output, retaining protocol 17.
2. Build Compact/Detailed HUD modes from that data; verify frame cost and layout.
3. Extend the proxy with tested directional rate/queue/burst models and add the
   isolated runner, scenario schema and report format.
4. Add bounded host input/shot telemetry and event correlation with an explicit
   protocol update, then collect the multi-client baseline. Record this as an
   instrumentation-only build preserving protocol 17 gameplay, and retain an
   uninstrumented comparison to measure observer cost.
5. Use the same harness and player metrics for each roadmap experiment.

Extend existing transport, session, prediction, HUD and console modules where
practical. Add a dedicated stats module only to keep the shared contract small;
no new crate or external telemetry service is needed. Documentation and designs
alone do not authorize release packaging or claim these features are available.
