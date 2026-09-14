<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-world

`rm-simulator-world` owns the complete `Field` facade: runes, outposts, bases,
chassis and projectiles on an explicit 1 ms clock, with live rules, scoring,
referee integration and whole-world restore. It coordinates the physical world
from `rm-simulator-physics` with the live economy from `rm-simulator-gameplay`,
and it never touches a renderer, a host clock, a transport or a background thread.

## Modules

| File | Owns |
|---|---|
| `src/lib.rs` | `Field`, `FieldConfig`, `FieldSnapshot`, `FieldRestore` and `FieldError`; target-frame refresh, hit scoring order, `step`/`step_with_hits`, `snapshot` and `restore`. |
| `src/rune.rs` | Deterministic Power Rune rules (section 5.5.2): `Rune`, `SmallRune`, `BigRune`, activation, seeded targets, rings and the training policy. |
| `src/outpost.rs` | Prescribed outpost rotation and the shared target/guard geometry. |
| `src/base.rs` | Live base armor: `BaseConfig`, `BaseSnapshot` and the seven scoring plates. |
| `src/referee.rs` | The match referee: `Referee`, teams, the round clock, rune opportunities and buffs, outpost observation and per-robot HP. |
| `src/projectile.rs` | Physical exports plus `ArmorHit` and `Rejection`, the serialized contact report. |
| `src/scoring.rs` | Private: contact detection and the gameplay consequences applied in physical contact order. |
| `src/chassis.rs` | Compatibility re-exports of the physics chassis library. |

`Field::step` advances explicit ticks and is independent of how those ticks are
partitioned. With no projectiles, chassis or referee it advances rune state
straight to the target tick; a referee keeps the loop active. Every snapshot
carries the rules' hidden state in `FieldRestore`, and `Field::restore` rebuilds a
whole field from one plus the shared `StaticGeometry` and floor height. Solver
state, such as contact manifolds and warm starts, is rebuilt rather than restored.

## Dependencies

`rm-simulator-gameplay`, `rm-simulator-physics`, `serde` and `thiserror`. The
crate never depends on Bevy, `rm-simulator-render`, `rm-simulator-server` or
`rm-simulator-app`, and it never reads host time. `rm-simulator-server` and
`rm-simulator-app` are its consumers.

## Testing

`cargo test -p rm-simulator-world --locked` (or `just world-test`) builds fields
from a `FieldConfig` and steps them. Coverage includes tick-partition invariance,
restore and replay, outpost protection and destruction, rune stages and
conversion, referee match flow and buffs, base plates and shield, chassis join,
leave and defeat, ramp climbing, dynamic and fixed geometry captures, and
projectile retirement replay in `tests/retirement_replay.rs`.

Doctests in `lib.rs`, `rune.rs`, `outpost.rs`, `base.rs`, `referee.rs` and
`projectile.rs` exercise field construction, stepping, firing, chassis commands,
referee commands, scoring rejections and restore without a GPU or a socket.

`examples/step_cost.rs` is a small reproducible wall-clock comparison of the main
stepping paths:

```sh
cargo run --release -p rm-simulator-world --example step_cost
```

Rule clauses and their section numbers are digested in
[docs/referee-rules.md](../../docs/referee-rules.md); ownership, tick order and
restore are in
[docs/architecture-refactor.md](../../docs/architecture-refactor.md); the live
gameplay adapter is described in [docs/gameplay.md](../../docs/gameplay.md).
