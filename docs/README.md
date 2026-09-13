<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Documentation

Start with the [project README](../README.md) for installation, controls,
asset discovery and current server/client behavior. Code and the active guides
below describe the live implementation. Historical plans retain measurements and
design decisions for their named revision, not current API guarantees.

## Current guides

| Topic | Guide |
|---|---|
| Crate ownership, ticks, restore and ECS | [Simulation architecture](architecture-refactor.md) |
| Standalone physics and optional rendering | [Physics reuse](physics-reuse.md) |
| Contribution workflow and checks | [Contributing](../CONTRIBUTING.md) |
| License, provenance and third-party notices | [Notices](../NOTICE.md) |
| Implemented live rules and assumptions | [Referee rules](referee-rules.md) |
| Standalone gameplay and its live integration | [Gameplay](gameplay.md) |
| App automation, headless rendering and screenshots | [Console](console.md) |
| Network trials and diagnostics | [Harness and stats](network-harness-and-stats.md) |
| Asset contracts and composition | [Semantic assets](semantic-assets.md) |
| Reproducible simplification and installation | [Field detail](field-detail.md) |
| Robot geometry and artwork assumptions | [Robot equipment](robot-equipment.md) |
| CPU measurements and reproducibility | [Performance](performance.md) |
| Renderer benchmark commands and interpretation | [Render benchmark](render-benchmark.md) |
| Optional Steam integration | [Steam](steam.md) |

The network harness guide includes historical trial results and original design
slices. Its opening run instructions describe the implemented harness. Asset
counts and performance values are specific to the package hashes and revisions
recorded with them; do not assume a local package still matches those captures.

## Implementation records and historical investigations

- [UDP impairment and version compatibility tests](network-stress-2026-09-13.md)

- [Hit feedback and auto-aim latency](latency-investigation-2026-09-13.md) records
  first-release loopback measurements, code findings and proposed fixes.
- [Networking implementation record](networking-implementation-plan.md) separates
  implemented stages from old proposals and outstanding acceptance work.
- [Networking roadmap](multiplayer-networking.md) retains the protocol 17 baseline
  and experiments, with a current status summary at the top.
- [Superseded prediction plan](client-prediction-refactor-plan.md) points to its
  replacement documentation and Git history.
- [Shooter-view checkpoint](shooter-view-validation-checkpoint.md) describes the
  historical protocol 15 implementation, whose hit policy has been removed.
- [Scene reference audit](scene-reference-audit.md) records the September 11 fixes.
- [Render effects](render-effects-2026-09-12.md) records measured preset experiments.

Keep dated evidence tied to its original build. Update the current overview or
add a superseded notice when behavior changes; do not relabel old measurements
as results from the latest protocol or renderer.
