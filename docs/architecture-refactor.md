<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Simulation architecture

Physics and prescribed motion live in `rm-simulator-physics`. The complete
`Field` facade in `rm-simulator-world` coordinates them with live rules and the
referee on one explicit clock (a fixed 128 Hz tick). Rendering consumes caller-owned appearance
and has no dependency on physics or gameplay. The server host worker owns each
live simulation and roster; app systems and transports submit commands to it.

The authoritative engineering rules behind this layout are in `AGENTS.md`; this
guide explains how the pieces fit together. Each crate's own `README.md` states
what it owns, its module layout and its permitted dependencies, and
`scripts/check-module-dependencies.py` enforces the boundaries in CI.

See [physics reuse](physics-reuse.md) for the public integration API and examples,
and [development](development.md) for the targets that build and test the
workspace. The broader standalone gameplay engine remains only partly integrated
through `live::Resources`; extracting physics did not enable additional live
rules. [Gameplay](gameplay.md) covers that engine's own coverage.

## Ownership

| Data or decision | Owner |
|---|---|
| Rigid bodies, suspension, ballistics, raw contacts | Physics library |
| Prescribed rotor/rail motion and fitted armor geometry | Physics library |
| Activation, HP, detection intervals, damage, match state | World rules and referee |
| Motion stop decisions and resolved mechanism positions | World, supplied to physics |
| Complete restore and tick orchestration | `Field` facade |
| Entity lifecycle, transforms and materials | Renderer |
| Snapshot-to-appearance adaptation and presentation clock | App |
| Roster, command authority and transport delivery | Server host |

The physics library depends only on Rapier and serde. Its RM-specific caliber
lookup helpers remain available for source compatibility; physical stepping does
not apply their damage or detection policy. The renderer depends on neither
physics nor world. `scripts/check-module-dependencies.py` enforces these boundaries.

## Tick order

For an active physical world, `Field::step` performs these operations in order:

1. Capture target start poses from current authoritative state.
2. Advance the tick and rune state.
3. Apply referee transitions, then advance any changed rune at the same time.
4. Resolve mechanism positions and target end poses.
5. Step chassis and projectiles together through one physical tick.
6. Process contacts in returned order: disabled armor, detection, damage or
   activation, then referee observation.
7. Observe outpost destruction, resolve changed mechanisms, disable defeated
   chassis, and expire old hit reports.

With no bodies and no referee, rune state advances directly to the target tick.
A referee keeps the tick loop active even when physics is idle. Mechanism state
is resolved only when articulated scenery exists. Commands and hits can change
motion at the current timestamp, so each tick rebuilds start poses from current
state. Scoring must not be deferred to another tick by an asynchronous queue.

## Restore and prediction

`Field::restore` rebuilds the complete simulation with shared collision geometry.
Both host and client prediction use the ordinary command, fire and step paths.
A checkpoint preserves hidden rule state, identities, detection history and
motion stops. Restore rebuilds solver warm starts, contact manifolds and the
projectile touched-armor cache; these retain their documented residuals.
Rebuild when the checkpoint changes, not once per rendered frame.

The extracted types retain the existing world import paths where practical.
Outpost checkpoints keep their serialized field names through the flattened
`RotorMotion`. Raw wire IDs and `ArmorTarget` remain unchanged. See the reuse
guide for the replacement of referee-dependent geometry queries with
`StaticGeometry::at_mechanisms`.

## App and ECS state

`PredictionState` owns the replay worker, geometry, checkpoint context, input
history, accepted prediction and counters inside `Session`. Embedded and remote
sessions both use `Client`; the embedded host still owns its simulation worker.

Chassis ingestion builds an ID index once per frame and resolves `PresentedPose`,
armor state and equipment-light state components. Transform and material systems
consume changed components. Existing part markers remain available, and absent
scene input leaves the visuals untouched. Projectile spheres retain their pool.

Tests cover unchanged transforms, first-frame poses, wheel visibility, light
states, chassis removal, legacy outpost serialization, tick partitioning and
complete-world replay. Before/after measurements and their limits are in the
[dated refactor comparison](performance.md#refactor-comparison). The comparison
is a regression check, not evidence of a renderer or gameplay speedup.
