<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Superseded client prediction plan

Current runtime behavior is described in the [README](../README.md#server-and-clients).
The [networking implementation record](networking-implementation-plan.md) records
implemented delivery, scheduling, remote buffering and diagnostics, plus their
validation limits. Multi-seed multiplayer acceptance remains outstanding.

`Field::restore` rebuilds the complete field using the extracted physical library.
Both chassis reconciliation and provisional shots use the ordinary world paths.
See [physics reuse](physics-reuse.md) for that boundary and
[the harness guide](network-harness-and-stats.md) for measurements.

The earlier continuous-prediction plan is preserved in Git history through
`47f3a78`. The [networking roadmap](multiplayer-networking.md) retains its protocol
17 baseline and experiments. The [shooter-view checkpoint](shooter-view-validation-checkpoint.md)
is a historical protocol 15 record; its hit policy is not used by current gameplay.
Release packaging and publishing remain paused.
