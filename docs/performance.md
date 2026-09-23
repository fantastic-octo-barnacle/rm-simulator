<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Engine performance checks

Runnable CPU probes and the workloads they measure. Results depend on the
compiler, hardware and asset package. [Measuring a candidate](#measuring-a-candidate)
explains how to compare builds.

## Engine microbenchmarks

Run the dependency-free examples in release mode:

```sh
cargo run --release -p rm-simulator-world --example step_cost
cargo run --release -p rm-simulator-gameplay --example benchmark
```

The world example compares 20,000 requested ticks over several runs. These
cases use no external CAD terrain; they distinguish empty fast-forward from
referee-only work and active physics, not a loaded match's frame rate.

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
Each sample advances 16 ordinary ticks; pilots drive and rotate, and fire
17 mm shots every 128 ms. The probe reports median, p95, p99 and maximum step,
snapshot and restore time, plus step batches exceeding 16 ms. Loading and startup
settling happen before sampling. Snapshot/restore measurements are separate from
step samples, but their contention is included in concurrent wall time. This is
an unpaced CPU probe, not a networking or renderer benchmark, nor a guarantee of
production server capacity. Include host/transport overhead and operating-system
headroom before selecting a matches-per-server limit.

## Measuring a candidate

Build before timing, keep the same compiler profile and checksummed field package,
and run each candidate on the target hardware without another build or renderer
competing for CPU. Record median and tail times, loading memory, and the counts
of step batches that exceed the available frame budget. Measure client GPU frame
time separately with the [render benchmark](render-benchmark.md). A headless
server probe does not establish rendering performance or network capacity.
