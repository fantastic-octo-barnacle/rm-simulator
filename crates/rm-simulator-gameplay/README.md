<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# rm-simulator-gameplay

`rm-simulator-gameplay` owns the deterministic RMUC 2026 match engine that the
world referee runs as its live rules authority. It runs a match
without physics, Rapier, CAD, networking, rendering or host time, and is pinned to
the English RoboMaster 2026 University Championship Rule Manual V2.1.0
(2026-07-17). It sits at the bottom of the layering: the world crate depends on
it, and it depends on no other simulator crate.

## Modules

| File | Owns |
|---|---|
| `src/engine.rs` | `Game`, `Command` and `Error`: the transactional match loop and its validated inputs. |
| `src/state.rs` | The state model: `Team`, `RobotKind`, `Caliber`, `Config`, `Phase`, `Snapshot`, `Event`, rounds, deliveries and zone contacts. |
| `src/zones.rs` | Section 5.5.3 buff point constants, terrain crossing courses and the Fortress cooling and reserve formulas. |
| `src/performance.rs` | `Performance`, `Stats` and the Hero and Infantry types: HP, chassis power, heat limit and cooling by level (Tables 5-12 to 5-14). |
| `src/coverage.rs` | `RULES`, the inventory of sections 5.1-5.8 and 6-9, with each group's section, `Support` status and remaining work. |
| `src/policy.rs` | Internal allowance and income policies. |

`Game::command` validates and commits atomically; a rejected command leaves the
game unchanged. `Game::step` advances explicit 1 ms ticks, and splitting the same
elapsed ticks cannot change the result. The clock constants are `TICK_NS`,
`SECOND_TICKS`, `SETUP_TICKS` (sections 6.3, 6.4, 6.5 and 6.6, 180 s),
`INITIALIZATION_TICKS` (section 6.4, 15 s), `COUNTDOWN_TICKS` (section 6.5, 5 s)
and `ROUND_TICKS` (section 6.6, 420 s). `BASE_HP`, `BASE_SHIELD_HP` and
`OUTPOST_HP` come from section 5.5.1.

## Dependencies

`serde` and `thiserror` only. The crate never depends on `rm-simulator-physics`,
`rm-simulator-world`, `rm-simulator-render`, `rm-simulator-server`,
`rm-simulator-app`, Rapier or Bevy. The world crate is its only in-workspace
consumer and re-exports it as `rm_simulator_world::gameplay`.

## Testing

`cargo test -p rm-simulator-gameplay --locked` (or `just gameplay-test`) runs the
unit tests in `engine/tests.rs` and `performance.rs`. Tests cover timed purchases,
income, damage, experience, performance, respawns, rebuilding, heat, eligibility
and tick partitioning.

Doctests live in `engine.rs`, `state.rs`, `performance.rs` and `coverage.rs`, including
the `coverage::RULES` assertions that every mechanic group appears exactly once
and carries its section.

`just gameplay-demo` runs `examples/match.rs`, a complete headless BO3 match that
ends in base destruction and result confirmation. `examples/benchmark.rs` is a
standard-library microbenchmark for coarse stepping and command transaction cost:

```sh
cargo run -p rm-simulator-gameplay --release --example benchmark --locked
```

The rule inventory, integration responsibilities and documented ambiguities are
in [docs/gameplay.md](../../docs/gameplay.md), and the referee clauses the live
integration implements are digested in
[docs/referee-rules.md](../../docs/referee-rules.md).
