<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Engine performance checks

The runnable CPU probes, their commands, and the environment they target. Each
probe runs against the live codebase; the numbers it reports depend on compiler,
hardware and workload. [Performance targets](#performance-targets) states what
the measurements must establish. [Dated evidence](#dated-evidence) below holds
measurements for named revisions and hardware; treat those numbers as evidence
for that revision, not as properties of the current build.

## Engine microbenchmarks

Run the dependency-free examples in release mode:

```sh
cargo run --release -p rm-simulator-world --example step_cost
cargo run --release -p rm-simulator-gameplay --example benchmark
```

The world example compares 20,000 requested ticks, taking the best of five
runs. On the development machine during this pass, the empty field took about
0.001 ms, an idle referee took 0.182 ms, and one chassis took 73.0 ms. These
cases use no external CAD terrain. They distinguish the empty fast-forward
path from referee-only work and active physics, not a loaded match's frame rate.
The idle referee cost does not justify a more complex scheduler at present.

The gameplay example compares a setup-to-running step with the same elapsed
span submitted one tick at a time. It also compares simple commands with a
full-game clone transaction, using a saturated 512-event history. The latter
models the previous transaction cost; it is not a second implementation of
the rules. See [gameplay](gameplay.md) for the measurements and retained
transaction boundaries. Results depend on compiler, hardware and workload.

## Loaded field and concurrent matches

The server example loads the checksummed collision package and runs independent
matches on separate threads, sharing immutable geometry as prediction does:

```sh
cargo run --release -p rm-simulator-server --example match_cost -- local-assets/field 1 8 1000
cargo run --release -p rm-simulator-server --example match_cost -- local-assets/field 4 8 1000
```

Arguments are CAD path, concurrent matches, pilots per match, and sample count.
Each sample advances 16 ordinary 1 ms ticks; pilots drive and rotate, and fire
17 mm shots every 128 ms. The probe reports median, p95, p99 and maximum step,
snapshot and restore time, plus step batches exceeding 16 ms. Loading and startup
settling happen before sampling. Snapshot/restore measurements are separate from
step samples, but their contention is included in concurrent wall time. This is
an unpaced CPU probe, not a networking or renderer benchmark, nor a guarantee of
production server capacity. Include host/transport overhead and operating-system
headroom before selecting a matches-per-server limit.

## Snapshot coalescing

Periodic network snapshots coalesce in each peer's outbox before enqueueing.
Tests keep the writer idle during 1,000 host publications and verify that only
the newest periodic state remains. Separate tests retain ordered confirmation
snapshots and Pongs. This bounds queued state on both transports, but it does
not reduce latency for bytes already in flight.

Since then the networking passes have measured high RTT, jitter and packet loss
under controlled impairment and addressed presentation and input responsiveness:
clients no longer render received snapshots directly. They interpolate remote
poses through a delayed view buffer and predict their own chassis locally, and
the gameplay transport is framed UDP. Periodic checkpoints are delta-coded
against acknowledged baselines and confirmation checkpoints stay independent, so
the ordered delta chain that once bound a peer's bytes to its previously
transmitted frame has been removed.

## Performance targets

The initial targets are a 3070 Ti at 1080p/60 Hz, under 4 GB steady application
RAM and under 8 GB loading peak, with VRAM measured separately. Geometry targets
are under 100k placed collision triangles and under 500k placed visual triangles
at standard detail. Triangle budgets do not replace frame-time or driving tests.
Use the 5950X server and the 12900K laptop to establish their own CPU and frame-time
baselines. The development Mac's results cannot establish those machines' capacity.

## Dated evidence

Measurements for a named revision and machine. They are regression references,
not current guarantees; re-measure on the target hardware before relying on them.

### Controlled package comparison (2026-09-12)

One release binary on an Apple M3 Pro with 18 GiB RAM ran each package
sequentially, with other builds stopped: one match, eight driving/firing pilots,
1,000 samples of 16 ticks. These are single runs, without renderer or transport.

| Measurement | Previous package | Standard package |
|---|---:|---:|
| Collision package load | 1.242 s | 0.381 s |
| Process peak resident memory | 600 MiB | 195 MiB |
| Step median | 2.754 ms | 2.752 ms |
| Step p95 | 3.396 ms | 3.393 ms |
| Step p99 | 3.797 ms | 3.719 ms |
| Step maximum | 5.633 ms | 4.636 ms |
| Batches over 16 ms | 0/1,000 | 0/1,000 |

The package reduced loading time and memory substantially. Typical simulation
CPU time was essentially unchanged; the small p99 difference is insufficient
to claim a repeatable improvement. This comparison isolates package choice,
not the separate AABB or dynamics changes. Further CPU profiling and repeated
concurrent-match measurements on the target machines remain necessary. Peak
resident memory here covers the headless probe, not the interactive application.

Manifest SHA-256 identifiers for reproduction:

- Previous: `c4faa6e59809169c4431f2ef5f1f05e296599bf7be8825122ca57e2047363be2`
- Standard: `78860dc0bb74f671a827436751ca322ee22593902c7f0e1f34d34bf273e1425b`

### Architecture refactor baseline (2026-09-12)

Captured before changing physics or renderer implementation, at revision
`4d6214beab1fea1bc8c7c6948a4352c14d0c123f`. Apple M3 Pro, 18 GiB RAM,
macOS 26.6.2, Rust 1.98.1, release profile. The Mac was on battery power.
Three sequential runs used the same checksummed `local-assets/field` package;
compilation completed before measurements. These are local comparisons, not
capacity figures for the target Windows/Linux machines.

Commands, binary/asset hashes and machine metadata for the refactor baseline were
retained with the runs. Percentiles are reported by
the existing probes; no independent samples were combined into a new percentile.

| Loaded match measurement | Run 1 | Run 2 | Run 3 |
|---|---:|---:|---:|
| Step median, ms per 16 ticks | 2.741 | 2.748 | 2.771 |
| Step p95, ms per 16 ticks | 3.322 | 3.308 | 3.326 |
| Step p99, ms per 16 ticks | 3.499 | 3.481 | 3.524 |
| Snapshot median, ms | 0.002 | 0.002 | 0.002 |
| Restore median, ms | 0.058 | 0.058 | 0.059 |
| Step batches over 16 ms | 0/1000 | 0/1000 | 0/1000 |

The first microbenchmark run took 79.870 ms for 20,000 one-chassis ticks,
0.179 ms for the referee-only case, and about 0.001 ms for empty fast-forward.
The last number is near timing resolution and does not represent real per-tick
physics work. Replay medians in that run were 0.25, 0.54, 0.87 and 1.92 ms for
32, 64, 128 and 200 ms windows. Full outputs include corrections and maxima.

The original CAD renderer was captured separately on Metal, CPU-only, offscreen,
High at 1920×1080, with 10 seconds warmup and 20 seconds sampling. Its CPU frame
interval median was 2.844 ms, p95 3.520 ms and p99 4.690 ms across 6,903 samples.
The renderer report recorded the resolved settings and asset hashes. Raw frames
and the inspected screenshot are retained locally at
`/tmp/rm-architecture-baseline/render`.
This scene has no chassis or projectiles and cannot measure the ECS changes.
GPU timings are unavailable in this Mac capture.

#### Refactor comparison

After the graphics build finished, the retained original binaries and the final
candidate were run in alternating groups, three times each, with no concurrent
builds or rendering. Each cell below is the range of the three reported values,
not a pooled percentile. The comparison metadata and commands
record binary hashes, a Rust-source fingerprint and power state. The same asset
package and release build settings were used.

| Measurement | Original control | Final refactor |
|---|---:|---:|
| Loaded step median, ms / 16 ticks | 2.742 to 2.797 | 2.723 to 2.763 |
| Loaded step p95, ms / 16 ticks | 3.333 to 3.438 | 3.310 to 3.361 |
| Loaded step p99, ms / 16 ticks | 3.569 to 3.929 | 3.565 to 3.717 |
| Snapshot median, ms | 0.002 | 0.002 |
| Restore median, ms | 0.062 to 0.065 | 0.058 to 0.063 |
| One chassis, ms / 20,000 ticks | 79.949 to 80.561 | 79.053 to 79.505 |
| Referee only, ms / 20,000 ticks | 0.178 to 0.184 | 0.182 to 0.199 |
| 200 ms replay median, ms | 1.91 to 1.94 | 1.94 to 2.00 |

All six loaded-match runs had zero batches over 16 ms. The 200 ms replay probe
reported the same maximum positional correction, 0.000078 m, in every run.
Loaded stepping and restore remain close to the original. Small differences in
these battery-powered runs do not establish a speedup; the replay and referee
microbenchmarks also show small increases. Performance remains a regression
check for this architectural change, not its claimed benefit.

The first candidate unnecessarily resolved mechanism state in worlds without
moving scenery. Making that conditional removed most of the extra referee-only
work. The initial comparison was retained separately from the final candidate.

The chassis ECS change was checked with headless component tests and inspected
hit/defeated robot screenshots at `/tmp/rm-architecture-robots-hit.png` and
`/tmp/rm-architecture-robots-defeated.png`. The optional physical scene also ran
for five seconds without a startup or simulation error. These checks validate
behavior and appearance; they are not an animated-scene performance benchmark.
