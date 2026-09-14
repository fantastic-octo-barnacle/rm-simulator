<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Documentation

Start with the [project README](../README.md) for installation, controls,
asset discovery and current server/client behavior. Code and the active guides
below describe the live implementation. The records section retains measurements
and design decisions for their named revision, not current API guarantees.

## Current guides

| Topic | Guide |
|---|---|
| Open issues and measurement gaps | [Known issues](../KNOWN_ISSUES.md) |
| Crate ownership, ticks, restore and ECS | [Simulation architecture](architecture-refactor.md) |
| Standalone physics and optional rendering | [Physics reuse](physics-reuse.md) |
| Contribution workflow and checks | [Contributing](../CONTRIBUTING.md) |
| License, provenance and third-party notices | [Notices](../NOTICE.md) |
| Implemented live rules and assumptions | [Referee rules](referee-rules.md) |
| Standalone gameplay and its live integration | [Gameplay](gameplay.md) |
| App automation, headless rendering and screenshots | [Console](console.md) |
| Packet metadata and local channel diagnostics | [Network tracing](network-tracing.md) |
| Asset contracts and composition | [Semantic assets](semantic-assets.md) |
| Reproducible simplification and installation | [Field detail](field-detail.md) |
| Robot geometry and artwork assumptions | [Robot equipment](robot-equipment.md) |
| CPU measurements and reproducibility | [Performance](performance.md) |
| Renderer benchmark commands and interpretation | [Render benchmark](render-benchmark.md) |
| Optional Steam integration | [Steam](steam.md) |

## Experiment records

- [Bandwidth experiments](bandwidth-experiments.md) records the isolated trials
  and the experiments integrated into the live wire, with their measured savings
  and the remaining bandwidth target. Treat its rates, asset hashes and revision
  ids as evidence for the named revision, not as properties of the current build.

Dated investigation logs, trial transcripts and per-experiment result files are
not kept here. They remain in Git history and in the pull requests that produced
the changes they describe; read them there when a claim needs its raw evidence.

Keep dated evidence tied to its original build. Update the current overview or
add a superseded notice when behavior changes; do not relabel old measurements
as results from the latest protocol or renderer.
