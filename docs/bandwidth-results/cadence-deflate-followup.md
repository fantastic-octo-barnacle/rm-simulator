<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Owner cadence and checkpoint deflate follow-up

Measurement date: 14 September 2026. Reference: `b003489`, protocol 29,
including acknowledged RMO4 owner configuration references and RMI3 inputs.
The user-confirmed cadence is **15.625 Hz** world
checkpoints (64 ms), with **31.25 Hz** owner updates (32 ms).

The five-seed clean matrix reduced mean downstream offered traffic from
**1,255.0 → 710.0 → 662.0 kbps per client**, including the IPv4/UDP allowance:
43.4% from cadence, then another 6.8% from deflate 4 (47.3% combined).
This two-client workload **does not meet the 200 kbps target**. Production
cadence and compression defaults remain unchanged.

## Method

Three release builds, compiled before measurement:

| Build | Owner publication | World publication | Checkpoint compression |
|---|---|---|---|
| reference | Every 32 ms host publication | Every publication | deflate 1 |
| cadence | Every publication | Alternate publications | deflate 1 |
| deflate4 | Every publication | Alternate publications | deflate 4 |

The cadence prototype is transport-local: owner processing occurs before the
alternate-publication filter. Reliable confirmations bypass the filter. It
preserves the host's publication period, owner configuration acknowledgements,
input compression, precision, pacing budgets and 32-encoded-frame baseline
rotation. Coalescing or congestion can lower actual delivery rates; these are
nominal production cadences. The level-4 prototype changes the `UdpSnapshot`
envelope compressor, including delta/independent selection and baseline
messages. It does not change input or full reliable-confirmation compression.
Neither prototype is enabled in production defaults.

Exact experimental patches are [cadence-64ms.patch](cadence-64ms.patch) and
[checkpoint-deflate4.patch](checkpoint-deflate4.patch). Apply the latter on top
of the former for the combined candidate. These are measurement prototypes,
not an approved production implementation.

Each live run uses two GPU-rendered clients in headless window mode, normal
scripted strafe/fire for 39 s,
then released controls, with 10 s warmup, 60 s active measurement and 10 s
recovery. Setup/warmup are unimpaired. The machine is an Apple M3 Pro with 18 GiB RAM, macOS 26.6.2 arm64, using
Rust 1.98.1. No networking environment overrides were set.
The clean matrix rotates build order
across seeds 2026–2030. The two capacity profiles run the combined candidate
at the same five seeds, alternating profile order. They keep a healthy control
peer; the impaired peer has 25 ms delay plus uniform 0–10 ms jitter and 1% loss
in each direction, unlimited upstream and either 200 or 100 kbps downstream.
The proxy queue wait is bounded to 250 ms. Downstream capacity becomes unlimited
at 60 s, while delay/loss remain during recovery. These are per-peer caps, not
a shared server-egress test.

Frozen diagnostic gates: zero unresolved outcomes, zero additional remote
presentation underruns, maximum sampled owner-or-world receive age 300 ms, at least
120 active samples per client and at most 20 missing console samples. Missing
checkpoint/presentation metrics fail. The existing `checkpoint_gap_ms` gate also
refreshes on owner anchors; it cannot bound complete-context starvation. The
report separately measures `collision_context_gap_ms` and complete arrivals,
while retaining the original frozen scenario checks in the raw reports. The
checked-in scenarios add a separate 300 ms complete-context gap check after
this attribution correction. This changes only report evaluation, not workload
or traffic. Its post-run evaluation uses the already captured samples; original
`summary.json` statuses are preserved. These gates are stricter than the smoke
scenarios; they do not cover all gameplay invariants or establish playability.

Rates are decimal kbps. Proxy received bytes are offered UDP payload, forwarded
bytes are delivered UDP payload; accounted service includes a 28-byte IPv4/UDP
allowance per packet. Native GNS traffic and retries pass through the proxy.
An offered rate over budget is a failure even if a cap forces forwarded traffic
below budget. Report each direction separately; their sum is the aggregate
interpretation. Initial synchronization is excluded from steady-state rates.
The workload is not the controlled adjacent-armor experiment and does not prove
populated robot-hit recovery or click-to-visible-hit latency.

## Live clean-link results

Rates below include the 28-byte IPv4/UDP allowance. Each seed row is the
mean of its two peers’ active-window offered downstream rates.

| Seed | Reference kbps | Cadence kbps | Cadence + deflate 4 kbps |
|---|---:|---:|---:|
| 2026 | 1264.0 | 710.0 | 662.4 |
| 2027 | 1246.1 | 706.8 | 661.8 |
| 2028 | 1259.9 | 701.2 | 674.1 |
| 2029 | 1241.0 | 716.8 | 653.2 |
| 2030 | 1264.0 | 715.1 | 658.6 |

The next table takes the mean of per-peer run rates; aggregate means upstream
plus downstream for one peer, and host downstream sums the two peers.

| Build | Up kbps | Down kbps | Aggregate kbps/peer | Host down kbps | Firing-only down kbps/peer, range |
|---|---:|---:|---:|---:|---:|
| reference | 126.7 | 1255.0 | 1381.7 | 2510.0 | 1671.6–1680.0 |
| cadence | 126.8 | 710.0 | 836.8 | 1420.0 | 924.9–936.0 |
| deflate4 | 126.8 | 662.0 | 788.8 | 1324.0 | 858.1–873.1 |

Production counters arrive in host telemetry; their rates use first-to-last
active console samples, so the roughly one-second telemetry sampling phase can
affect short-window estimates. The app’s actual complete-receive count is also
shown. Rates below are ranges across the ten peer-runs per build.

| Build | Produced owner Hz | Produced world Hz | Complete received Hz | Owner payload kbps | Selected checkpoint kbps |
|---|---:|---:|---:|---:|---:|
| reference | 29.7–30.2 | 29.7–30.2 | 29.9–30.1 | 105.4–107.3 | 991.7–1013.1 |
| cadence | 29.7–30.3 | 14.9–15.2 | 15.0–15.1 | 105.6–107.6 | 498.2–510.8 |
| deflate4 | 29.7–30.3 | 14.8–15.1 | 15.0–15.1 | 105.5–107.6 | 454.3–470.4 |

Latency entries are ranges of per-peer-run p95s, not percentiles pooled or
averaged across runs. Arrival interval means time between complete checkpoints;
the sampled receive-age gate is a different metric.

| Build | App RTT p95 ms | Shot confirmation p95 ms | Checkpoint interval p95 ms | Worst complete interval ms | Correction position p95 m | Additional underruns | Unresolved outcomes |
|---|---:|---:|---:|---:|---:|---:|---:|
| reference | 9.1–11.8 | 58.1–63.9 | 35.9–37.0 | 79.9 | 1.7e-07–2.44e-06 | 9 | 0 |
| cadence | 8.0–11.5 | 57.9–80.9 | 69.9–70.8 | 95.4 | 2.64e-07–3.27e-06 | 3 | 0 |
| deflate4 | 7.6–11.3 | 57.7–75.6 | 69.6–70.7 | 90.8 | 2.41e-07–5.06e-06 | 0 | 0 |

| Build | Remote presentation age p95 ms | Prediction backlog p95 ms | Complete-context age p95 ms |
|---|---:|---:|---:|
| reference | 60.0–69.0 | 21.0–24.0 | 32.0–33.0 |
| cadence | 91.0–100.0 | 20.0–24.0 | 34.0–65.0 |
| deflate4 | 90.0–95.0 | 21.0–24.0 | 34.0–35.0 |

Peak roughly one-second offered downstream windows were 1,731–1,803 kbps for
reference, 989–1,027 kbps for cadence, and 909–961 kbps for deflate4. These use
actual consecutive proxy-report timestamps.

The increased remote presentation age is a real cadence tradeoff even when
launch confirmations remain prompt. It measures deliberate buffering, not RTT
or input-to-photon latency. Correction values above are sampled same-tick
comparisons, not visible camera displacement.

## Constrained capacity and recovery

The combined candidate is tested against absolute capacity and recovery gates;
there is no matched capped reference/cadence-only matrix here. Offered traffic
includes GNS adaptation/retries. Reduced offered traffic under a cap cannot be
interpreted as an encoding saving when complete world progress or firing stalls.

| Cap kbps | Seed | Offered / forwarded down kbps | App RTT p95 ms | Shot p95 ms | Worst complete interval ms | Expired-unresolved active / recovery | Underruns |
|---|---|---:|---:|---:|---:|---:|---:|
| 200 | 2026 | 832.6 / 198.1 | 2370.4 | 4609.3 | 38638.4 | 20 / 20 | 1 |
| 200 | 2027 | 844.3 / 197.1 | 2411.8 | 4894.7 | 37585.3 | 48 / 48 | 1 |
| 200 | 2028 | 818.1 / 197.6 | 2848.9 | 4124.6 | 37841.1 | 0 / 0 | 0 |
| 200 | 2029 | 827.6 / 197.3 | 2837.3 | 4750.5 | 37229.8 | 61 / 61 | 1 |
| 200 | 2030 | 851.4 / 197.6 | 2482.8 | 4642.4 | 39334.2 | 29 / 29 | 2 |
| 100 | 2026 | 2326.9 / 99.2 | unavailable | unavailable | 4446.9 | 184 / unavailable | 0 |
| 100 | 2027 | 2199.6 / 99.4 | unavailable | unavailable | 4090.6 | 177 / unavailable | 0 |
| 100 | 2028 | 1972.1 / 99.1 | unavailable | unavailable | 5116.1 | 192 / unavailable | 0 |
| 100 | 2029 | 614.0 / 96.9 | unavailable | unavailable | unavailable | unavailable | unavailable |
| 100 | 2030 | 2274.6 / 99.0 | unavailable | unavailable | 5725.9 | 192 / unavailable | 0 |

At 200 kbps, impaired peers completed 0.2–0.2 checkpoints/s, with 24456 downstream proxy drops across five seeds. Final recovery sampled complete-context receive age was 0.4–22.6 ms. Available healthy-control observations had 0 unresolved outcomes and 1 underruns; their app RTT p95 range was 9.3–10.9 ms.

At 100 kbps, impaired peers completed 0.1–0.2 checkpoints/s, with 51329 downstream proxy drops across five seeds. Recovery was unmeasured because every run aborted before restoration. Available healthy-control observations had 0 unresolved outcomes and 2 underruns; their app RTT p95 range was 9.0–11.6 ms.

The 200 kbps sampled complete-context gaps reached 33.0–33.9 seconds,
while the last complete-arrival intervals reached 37.2–39.3 seconds. After
capacity returned, the final recovery samples had fresh complete context and
zero pending shots. The expiry counter is cumulative: its unchanged value does
not mean that previously expired local flights were retrospectively recovered.

All five 100 kbps trials aborted after five consecutive impaired-console timeouts,
before capacity restoration. Four supplied only 0–16 shot-confirmation events
and 2–3 RTT events, below the 20-event p95 threshold. Their sampled context gaps
reached about 29–30 seconds; the shorter completed-arrival intervals in the table
do not include the still-open final silence. Seed 2029 supplied no impaired
active samples (and one healthy sample), so its outcome and latency metrics are
unavailable. Its proxy window was about 16 s; the other aborted proxy windows
were about 54 s. These truncated rates are not full 60-second workload means.
Zero recorded underruns during a stalled or unobservable session is not evidence
of smooth presentation.

The original diagnostic status passed all five combined clean runs, two of five
cadence-only runs and one of five reference runs. The reference and cadence-only
failures were underruns. At 200 kbps, seed 2028 also passed the original checks
despite a 33.5 s sampled context gap and 4.1 s shot p95. The added complete-context
check fails every capped run (missing data fails seed 2029 at 100 kbps) and passes
all clean runs. Raw statuses remain unchanged; additional evaluations are retained
as `complete-context-check.json`. No recorded latency-event history IDs were lost.
Every run reported no owned-process cleanup errors, including aborted trials.

| Cap kbps | Impaired offered up kbps | Impaired offered up + down kbps | Total host offered down kbps |
|---|---:|---:|---:|
| 200 | 185.4–196.0 | 1014.2–1041.7 | 1449.7–1484.6 |
| 100 | 121.7–175.8 | 735.7–2499.2 | 1005.0–2960.9 |

Capped offered traffic includes retransmissions and transport adaptation. It is
not a proxy for the unconstrained encoder cost. Proxy drops were overwhelmingly
queue-wait drops; attribution between application and native queues still needs
investigation. These tests do not close the earlier blackout-recovery failure,
validate packet-specific setup/ACK loss, or measure robot-hit disagreement.

## Identical selected-envelope compression sweep

All 37,520 compression/inflation checks passed (four workloads × 938 envelopes
× two levels × five repeats). This is an in-process codec experiment, not a
12-client native GNS trial. Firing occurs every 128 ms for one shooter; this
is a different load from the two rendered clients' live sustained fire.

| Workload | Selected kbps, level 1 → 4 | Saving | Fragments, level 1 → 4 | Compression mean µs, level 1 → 4 | Inflation mean µs, level 1 → 4 |
|---|---:|---:|---:|---:|---:|
| 0 moving players, firing=false | 36.8 → 33.0 | 10.3% | 968 → 968 | 5.6 → 8.4 | 3.8 → 3.6 |
| 2 moving players, firing=false | 144.1 → 119.7 | 16.9% | 1906 → 998 | 13.9 → 23.3 | 9.5 → 8.5 |
| 12 moving players, firing=false | 472.4 → 297.2 | 37.1% | 3847 → 2862 | 43.0 → 69.8 | 30.1 → 21.2 |
| 2 moving players, firing=true | 312.7 → 274.0 | 12.4% | 2833 → 2792 | 28.3 → 58.6 | 19.6 → 18.5 |

Bytes exclude application fragment headers, owner updates, control and network
headers. Fragments use 1,000-byte payload chunks. CPU entries are the median
of five per-repeat means, not a pooled latency percentile. Level 4 costs more
compression work. These timings do not quantify the total encoder CPU increase:
they exclude JSON and delta construction and measure only one selected envelope.
The firing stream's deflate reduction alone is insufficient to meet 200 kbps,
even before its owner/control/network traffic is included.

## Reproduce

Build the reference and each patched candidate in separate checkouts, and copy
matching app/server binaries into separate local directories. Do not compile
while timing. For example, after building the selected checkout:

```sh
cargo build --release --locked -p rm-simulator-app -p rm-simulator-server
python3 scripts/network-harness.py scripts/network-scenarios/cadence-clean.json \
  --server /path/to/build/rm-simulator-server --app /path/to/build/rm-simulator \
  --seed 2026 --output /tmp/rm-cadence-clean-2026 --build-label 'release: selected revision and patch'
```

Repeat seeds 2026–2030 for all three clean builds. For the combined candidate,
repeat with `cadence-cap200.json` and `cadence-cap100.json`. The checked-in
scenarios preserve the exact workload and include the additional complete-context
freshness gate described above.

The isolated same-envelope sweep runs on the unpatched reference encoder:

```sh
cargo run --release --locked -p rm-simulator-server --example network_bandwidth -- --deflate-sweep
```

It records 938 checkpoints over 60.032 simulated seconds at 64 ms intervals,
with immediate baseline feedback and no CAD or native transport. Each selected
level-1 envelope is recompressed at levels 1 and 4 in five alternating-order
repeats; every inflation must reproduce the exact bytes. This holds delta/full
selection fixed. Timings cover compression/inflation alone, excluding JSON,
delta construction, simulation and rendering. The live level-4 candidate can
make different delta/full choices and is measured separately.

## Manual play with progressively constrained networking

From the repository root, use the measured combined candidate binaries while
these local artifacts exist:

```sh
trial_build=/tmp/rm-cadence-20260914/deflate4
python3 scripts/network-harness.py scripts/network-scenarios/cadence-play.json \
  --manual --network-stats detailed \
  --server "$trial_build/rm-simulator-server" --app "$trial_build/rm-simulator" \
  --output "/tmp/rm-cadence-play-$(date +%s)"
```

This launches one visible pilot through the proxy. After the 10 s warmup,
downstream capacity is 512 kbps for 60 s, then 200 kbps for 60 s, then 100 kbps
for 60 s, then unlimited for 60 s. Both directions retain 25 ms delay plus
0–10 ms jitter and 1% loss; upstream capacity is unlimited. Controls are yours,
and the harness records stats and closes its processes after recovery. Try
movement/fire together, release during congestion, aim changes and ramps, then
compare behavior when capacity returns. The 100 kbps automated trials aborted
on console timeouts; this manual profile can also end before its final stage
if the console stops responding. Shot confirmation is a launch outcome,
not an armor-impact latency measurement. Manual results are subjective play
feedback, not a matched automated bandwidth comparison.

Set `trial_build` to the `reference` or `cadence` directory for comparison.
Rebuild from the recorded revision/patches if the local binaries are removed.
Avoid running this concurrently with a timing matrix on the same machine.

## Local artifacts

Raw data, logs, binary hashes, manifests, candidate patches and the sequential
matrix driver are under `/tmp/rm-cadence-20260914`. Each run has its own directory.
These are local evidence, not permanent download URLs. The manifest records
platform, CAD checksums, binary SHA-256, environment and exact scenario. The
checkout also had unrelated `.gitignore` and `docs/naming-shortlist.md` edits;
those were preserved and are not part of these experiments.
