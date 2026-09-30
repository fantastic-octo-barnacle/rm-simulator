<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Documentation

Start with the [project README](../README.md) for installation, controls,
asset discovery and current server/client behavior. The guides below describe
the live implementation. Old investigations and measurements are available in
Git history.

## Current guides

| Topic | Guide |
|---|---|
| Contribution rules and review | [Contributing](../CONTRIBUTING.md) |
| Environment, `just` targets, tests and commits | [Development](development.md) |
| CI checks, validation reuse and releases | [CI and releases](releases.md) |
| Crate ownership, ticks, restore and ECS | [Simulation architecture](architecture-refactor.md) |
| Standalone physics and optional rendering | [Physics reuse](physics-reuse.md) |
| Standalone gameplay and its live integration | [Gameplay](gameplay.md) |
| Implemented live rules and assumptions | [Referee rules](referee-rules.md) |
| Command-line options, controls and interface | [App options](app-options.md) |
| App automation, headless rendering and screenshots | [Console](console.md) |
| Testing a custom client over the referee protocol | [Referee link](referee-link.md) |
| Packet metadata and local channel diagnostics | [Network tracing](network-tracing.md) |
| Field package layout, discovery, composition, collision | [Field package](field-package.md) |
| Asset contracts and composition | [Semantic assets](semantic-assets.md) |
| Reproducible simplification and installation | [Field detail](field-detail.md) |
| Robot geometry and artwork assumptions | [Robot equipment](robot-equipment.md) |
| CPU measurements and reproducibility | [Performance](performance.md) |
| Renderer benchmark commands and interpretation | [Render benchmark](render-benchmark.md) |
| Optional Steam integration | [Steam](steam.md) |
| License, provenance and third-party notices | [Notices](../NOTICE.md) |
