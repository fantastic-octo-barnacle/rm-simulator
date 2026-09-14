<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-physics

`rm-simulator-physics` is a reusable, gameplay-free Rapier library: wheel and body
dynamics, projectile flight, raw armor contacts, shared collision geometry and
prescribed mechanism motion. It knows no match rules, CAD package, socket, Bevy or
host clock; callers supply target endpoint poses and resolved mechanism state and
step it once per explicit tick. It sits under the world crate and depends on no
other simulator crate.

## Modules

| File | Owns |
|---|---|
| `src/chassis.rs` | The four-wheel chassis: a dynamic body cuboid and a turret cuboid, ray-cast wheels with spring/damper suspension and ideal omni tyres, four armor housings, gimbal dynamics and the shared drivetrain power budget. `ChassisConfig` presets are the omni Infantry and the mecanum Hero. |
| `src/projectile.rs` | `WorldPhysics`, `Shot`, `Caliber`, `Contact`, `TargetFace`, `TargetFrames`, `StaticGeometry` and `ProjectilePolicy`: projectile flight, scoring faces and raw contacts in physical order. |
| `src/geometry.rs` | Shared collision captures and read-only clearance queries consumed by the world facade. |
| `src/motion.rs` | Prescribed rotor and rail motion (`RotorMotion`, `SinusoidalMotion`, `MechanismState`) and fitted armor geometry; rune pose helpers live under `motion::rune`. |

`lib.rs` also holds the clock contract: `DEFAULT_TICK_NS`, `tick_ns()`,
`valid_tick_ns`, `set_tick_ns`, `OFFERED_RATES_HZ` (1000, 500, 250 and 128 Hz)
and `tick_ns_for_hz`/`hz_for_tick_ns`, plus the FLU `Pose` and `Team`. Time
arguments count ticks of `tick_ns()`, and the value freezes on first read so that
no two parts of one process disagree. The step applies no HP, buffs, activation
or detection intervals; the world crate owns those decisions.

## Dependencies

`rapier3d-f64` and `serde`. The crate never depends on `rm-simulator-world`,
`rm-simulator-gameplay`, `rm-simulator-server`, `rm-simulator-render` or Bevy,
and it never reads host time. `rm-simulator-world` is its only normal consumer;
`rm-simulator-app` takes it as a dev-dependency for the `physical_scene` example.

## Testing

`cargo test -p rm-simulator-physics --locked` runs the in-module tests in
`chassis.rs` and `projectile.rs` and the integration test
`tests/projectile_retirement.rs`. Coverage spans chassis settling on the floor,
holonomic drive, motor limits, gimbal rate limiting and aim freezing, the shared
power budget, scoring-face acceptance, closing speed, projectile bounds and
retirement, and target-frame identity and ordering.

`lib.rs`, `chassis.rs`, `geometry.rs`, `motion.rs` and `projectile.rs` carry
doctests; the crate-level example launches from a muzzle and asserts one contact
over a 200-tick flight.

`examples/moving_armor.rs` creates a moving armor face, launches ten projectiles
and advances 1,000 explicit ticks without CAD, a window or a server:

```sh
cargo run -p rm-simulator-physics --example moving_armor --locked
```

[docs/physics-reuse.md](../../docs/physics-reuse.md) documents the headless
stepping contract, the restore procedure and optional rendering.
