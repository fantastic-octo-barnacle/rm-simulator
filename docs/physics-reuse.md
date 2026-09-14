<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Reusing physics and armor motion

`rm-simulator-physics` depends only on Rapier and serde. It contains chassis
suspension and drive, gimbal dynamics, projectile flight, raw armor contacts,
shared collision geometry and analytic armor motion. It does not depend on
world, gameplay, server, rendering, a CAD loader or host time. Its
[crate README](../crates/rm-simulator-physics/README.md) lists its modules and
permitted dependencies.

The library retains RoboMaster-specific physical profiles and target labels.
`Caliber` includes the existing nominal detection/damage lookup helpers for
source compatibility, but the physical step does not apply HP, buffs, activation,
detection intervals or match rules. The caller controls chassis disable state.
This is a reusable RM physical library, not a general physics-engine abstraction.

## Contents

- [Headless use](#headless-use)
- [Restore and prediction](#restore-and-prediction)
- [Optional rendering](#optional-rendering)

## Headless use

```sh
cargo run -p rm-simulator-physics --example moving_armor --locked
```

The example creates a moving armor face, launches ten projectiles and advances
1,000 explicit ticks. It requires no CAD, window, match or server. Raw contacts
report target identity, local contact position and normal speed. Consumers may
use these for their own sensors, detection policy or scoring.

Construct `WorldPhysics` with initial `TargetFace` poses and a catch-floor height.
Supply static meshes in metres, FLU coordinates, through `add_static_mesh` or
`add_boundary_mesh`. No default arena or file path is built into the library.
Use shared `StaticGeometry` captures for collision queries or rebuilding bodies.

For each tick:

1. Write start poses with `TargetFrames::begin`.
2. Advance caller-owned motion and write end poses with `TargetFrames::update`.
3. Supply resolved `MechanismState` if the world contains articulated scenery.
4. Call `WorldPhysics::step(start_time_ns, &frames)` once, then process contacts
   in returned order before beginning another tick.

`TargetFrames` validates target identity and ordering at both endpoints. External
targets have fixed membership for the lifetime of those frames. Chassis admission
and removal use the physical world's own methods. Do not change target ordering
without rebuilding the corresponding physical target bodies.

`RotorMotion` produces outpost angles and armor poses without HP. Set
`stopped_at_ns` when the consuming application decides motion should stop.
`SinusoidalMotion` computes the Big Rune speed profile; the world layer retains
rule-range validation, seeded selection and activation transitions. Rune pose
helpers are available under `motion::rune`. `MechanismState` carries already
resolved door states and dart-rail position. Physics never asks a referee why
those states changed.

Time arguments are simulation time. The caller owns monotonicity, overflow checks,
fixed-tick scheduling and the motion state associated with each checkpoint.
Lengths and rotations use FLU metres and normalized wxyz quaternions. Existing
physical assumptions, projectile lifetime/capacity and launch validation remain
unchanged by extraction.

## Restore and prediction

The simulator still uses `Field::restore`, which rebuilds a complete physical
world plus its rules. It restores chassis and projectiles through the extracted
library's ordinary restore methods, inserts the shared geometry and restores
identity counters before continuing normal stepping. Prediction uses that same
facade, not a reduced local physics implementation.

Standalone users can retain `chassis_snapshots`, projectile `snapshot`, geometry,
`next_chassis_id`, `launched`, current target/motion state and the simulation time.
Rebuild with `insert_geometry`, `restore_chassis`, `restore_projectile`,
`restore_identity` and `mark_synced`, then resolve mechanisms at checkpoint time.
Do this when the checkpoint changes, not once per rendered frame. Solver warm
starts, contact manifolds and each projectile's touched-armor cache are rebuilt;
restored contact residuals retain the existing simulator limitations.

The world facade re-exports existing `Pose`, `Team`, chassis and projectile types.
Serialized hit reports and outpost restore fields retain their previous shape.
Presentation clearance now takes `StaticGeometry::at_mechanisms`; the world
adapter `referee::mechanism_view` converts referee snapshots into its input.

## Optional rendering

`rm-simulator-render` remains a separate Bevy library with no physics dependency.
The consumer converts physical poses and its own appearance choices into
`SceneState`. The app's `physical_scene` example demonstrates that composition
without calling `Field`, the referee, CAD loading or server APIs:

```sh
cargo run -p rm-simulator-app --example physical_scene --locked
```

This demonstration advances 16 simulation milliseconds per rendered frame.
Applications that need real-time pacing should supply their own tick accumulator.
The renderer never advances physics or rules.

Chassis snapshot ingestion builds an ID index once per update and resolves
`PresentedPose`, armor state and equipment-light state components. Transform and
material systems consume changed components. Required components keep manually
spawned chassis parts usable in headless tests; the existing ID/part markers
remain available. The projectile visual pool remains unchanged.

CAD package parsing remains in the server crate. Consumers that need package
loading can implement it outside physics or use that existing adapter. This
refactor does not introduce another asset format, publish crates or add sibling
repository dependencies.

Run the crate's own tests with `cargo test -p rm-simulator-physics --locked`;
[development](development.md) lists the workspace targets. The architecture
boundaries that keep this crate reusable are stated in
[simulation architecture](architecture-refactor.md).
