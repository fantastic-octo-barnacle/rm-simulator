<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Standalone gameplay engine

`rm-simulator-gameplay` runs a match without Bevy, Rapier, CAD, networking or
host time. It is pinned to the English RoboMaster 2026 University Championship
Rule Manual V2.1.0, dated 2026-07-17. It does not silently adopt later releases.

```sh
just gameplay-test
just gameplay-demo
```

The demo completes a BO3 match, including setup, initialization, countdown,
base destruction, result confirmation and the next round. Tests cover timed
purchases, income, respawns, rebuilding, heat, eligibility and tick partitioning.

Read this guide before changing either gameplay integration.

## Contents

- [Live integration and ownership](#live-integration-and-ownership)
- [API and clock](#api-and-clock)
- [Rule coverage and unsupported elements](#rule-coverage-and-unsupported-elements)
- [Implemented consequences](#implemented-consequences)
- [Result comparison](#result-comparison)
- [Ambiguities and application choices](#ambiguities-and-application-choices)

## Live integration and ownership

The `live::Resources` component is connected to the world referee and therefore
to both the app and server.

| Owner | Responsibility |
|---|---|
| World referee | Phase and round clock |
| `live::Resources` | Per-chassis allowances and successful shot counts for 17 mm and 42 mm, free-camera shot counts and each team's gold; no independent clock or physics authority |
| World code | Robot, base and outpost damage and rune activation |
| `rm-simulator-physics` | Dynamics, raw contacts and prescribed armor motion |
| Standalone `Game` | The larger engine described below; remains standalone |

`RefereeSnapshot.gameplay` exposes the live state over GNS UDP and `GET /api/state`.
The HTTP page's ammo/economy and equipment forms send ordinary referee commands
through `POST /api/command` or `POST /api/referee`. Clients and hosts must use
matching builds; `PROTOCOL_VERSION` in the server's `protocol.rs` is authoritative. Pilots and spectators cannot submit these edits.

| Live control | Behavior |
|---|---|
| Ammo policy | Toggle allowance enforcement and set initial allowance for both calibers. Defaults are zero, with enforcement off for practice. |
| Income policy | Toggle automatic income and set initial, minute and final-minute grants. Defaults follow Table 5-5. Disabled grants are skipped without back-payment. |
| Resupply prices | Set price per projectile, initially 1 gold for 17 mm and 10 for 42 mm, based on Table 5-6 local exchange. |
| Robot resources | Set current allowances and shot counters for an existing chassis. |
| Team gold | Set either team's current balance. |
| Referee resupply | Immediately purchase ammo for the selected robot using team gold and configured prices. This operator action does not claim RFID eligibility, minimum exchange units, exchange limits or remote delivery. Insufficient funds or overflow reject without changing resources. |
| Free-camera counters | Edit separately tracked shots without a chassis. |
| Outpost HP | Set 0..1500 HP on the physical world outpost; zero destroys it and positive HP revives it. |
| Rune opportunities | Set a team's current opportunity count, including zero. Scheduled opportunities still arrive. |
| Rune buff | Override defense, reported attack/cooling and duration. Zero duration clears it. Cancels the current activation attempt and darkens the rune; does not simulate activation or spend an opportunity. |

Policy edits affect future operations. Initial allowances apply on start/reset
and admission of new chassis; current balances have their own controls. Match
start/reset clears resource counters and gold, preserves policy and restores
physical outposts to 1500 HP. Leaving removes the chassis's resources. Only
successful physical shots during Running consume allowance and increase counts;
invalid launches and defeated shooters do neither. Free-camera shots remain
unrestricted. Pausing freezes income; clock skips grant all crossed enabled
income boundaries when the referee next ticks. Ending freezes resource tracking.

The HUD displays the resource snapshots. Form inputs are not overwritten by
polling; use their Load current buttons to replace draft values with live state.
Configuration is in memory and does not survive host restart.

### Outside the live integration

The following remain outside live integration: automatic
respawn, heat and power enforcement, XP, performance tables, zone detection,
remote purchases, full match adjudication, and attack/cooling buff effects. A
future adapter must assign one owner to each rule before connecting more of the
standalone engine; it must not apply physical damage or advance the clock twice.

The live adapter and standalone engine share the Table 5-5 income schedule,
default grant amounts, and the allowance/shot-count update for one launch. The
live adapter still owns its tunable economy settings, while the standalone
engine keeps the fixed competition policy and its additional heat, experience,
and over-allowance consequences. A conformance test runs the same income
boundaries and launch sequence through both paths.

## API and clock

Construct `Game::new(Config)` with a fixed roster and explicit performance
parameters. Robot ids must be unique and HP positive. The roster supports
Hero, Engineer, Infantry, Drone, Sentry, Dart and Radar. These records do not
create chassis, so unsupported equipment can exist in a headless scenario.
There is no referee robot. Dynamic pilot admission remains the existing
server's responsibility until that adapter is implemented.

`Game::command(Command)` validates an input and commits it atomically. An error
leaves the game unchanged, including pending deliveries and event ids.
`Game::can_launch` includes match phase as well as robot permissions.
`Game::step(ticks)` advances an explicit number of 1 ms ticks. Withholding ticks
pauses the game. Splitting the same elapsed ticks cannot change the result.
`BeginRound` runs 180 s setup, 15 s initialization and 5 s countdown before the
420 s round. `BeginCountdown` is an explicit practice shortcut. `EndRound`
ends a running round early; `ConfirmResult` records its result once.

A BO2 ends after two confirmed rounds. A BO3 or BO5 ends after two or three
wins respectively; draws require another round. `BeginRound` after confirmation
clears round resources, timers and observations while retaining result history.
`ResetMatch` clears that history and restores the configured roster. The global
simulation tick never goes backwards. Event ids restart on match reset.

Snapshots expose teams, robots, pending purchases, buffs, observations, round
results and the latest 512 events. Event retention is an application policy;
record input commands and their tick positions if you need a complete replay.
Snapshots and commands serialize through serde. Snapshots are read-only views,
not a trusted save-game loading API; clone a `Game` to branch a scenario.

`Game::step` skips directly across setup, initialization, countdown, and phases
whose state is frozen. Running gameplay retains exact 1 ms processing. Pending
deliveries remain ordered by due tick, so the running loop only removes the due
prefix instead of rebuilding the complete queue each millisecond.

`Game::command` validates and commits combat-state observations,
disconnections, validated launches, and observed launches without cloning the
whole game. These paths have no fallible operation after their validation.
Commands with several dependent writes retain the clone-and-commit transaction,
which keeps rejection atomic without a general rollback system.

Run the standard-library microbenchmark with:

```sh
cargo run -p rm-simulator-gameplay --release --example benchmark
```

On the development machine on 2026-09-11, three runs put one coarse 200,000-tick
setup-to-running advance at 93-104 us, versus 631-666 us for 200,000 one-tick
calls. Twenty thousand direct combat-state commands took 5.54-5.62 ms; wrapping
each in the former full-clone transaction pattern took 21.9-23.8 ms with a full
512-event history. This is a reproducible microbenchmark rather than a stable
performance target; hardware and allocator results will differ.

## Rule coverage and unsupported elements

[`coverage::RULES`](../crates/rm-simulator-gameplay/src/coverage.rs) is the
complete inventory of gameplay mechanism groups in sections 5.1 through 5.8,
plus the competition process and human adjudication in sections 6 through 9.
It names each source section, implementation status and remaining work.

| Status | Meaning |
|---|---|
| `Implemented` | The engine executes the listed state transitions. The entry also names exclusions. |
| `External` | An authoritative caller must supply the specified observation; the engine applies the listed consequences. |
| `Tracked` | State can be recorded but the mechanic's consequences are not automatically executed. |

`Observe` records a mechanic, optional team and robot, a state name and named
measurements. Include units in measurement keys. The engine stamps the current
round tick and replaces the previous observation for the same mechanic/team/
robot. Missing observations mean **unknown**, not inactive or unsupported
hardware absent. Consult the coverage register separately to determine support.
Observations never grant rewards, inflict damage or change permissions.

All field buff point types have structured zone contacts, including base,
resupply, outpost, central and trapezoid highlands, road, elevated crossing,
launch ramp, tunnel, assembly and fortress. Radar decoding, dart windows,
engineer assemblies, sentry poses, hero deployment, drone counters, module
faults, power telemetry, inspections, timeouts, penalties and appeals can be
recorded even when there is no simulated equipment or sensor for them.

Mechanics outside the live integration are named under
[Outside the live integration](#outside-the-live-integration).

## Implemented consequences

- Income follows Table 5-5 at elapsed 1, 61, 121, 181, 241, 301 and 361 seconds.
  Table 5-6 purchases enforce costs and per-team ammo limits. Remote ammo and
  HP arrive six seconds later. Defeat cancels pending HP without refund.
- Sections 5.2.1 and 5.2.2 provide resupply healing, normal and paid respawn,
  accelerated respawn progress, weakened launch restrictions and invincibility.
  The caller supplies out-of-combat and irregular-disconnection observations.
- Section 5.3.2 supplies initial ammo, launch consumption and accumulated sentry
  resupply. The Hero's special 42 mm immunity mechanism is not yet enforced.
- Section 5.3.3 certified assembly completions provide recurring income,
  level-cap increases, defense, base HP and overflow shield. Availability times,
  preceding level completion and the single level-4 completion are checked.
  Robotic arm poses, shared-core exclusion and assembly failure penalties are
  still external; a player must not be allowed to certify their own completion.
- Section 5.4.1 and Table 5-11 apply shot XP and explicit XP awards up to the
  current cap. Kill attribution, shared XP and Tables 5-12 through 5-15's
  performance changes are not automatic. Performance remains caller-configured.
- Section 5.5.1 supplies base HP, virtual shield, outpost protection, destruction,
  cumulative base-loss rebuild opportunities and individual uninterrupted
  rebuild scans, with a strict five-minute cutoff. Rotation remains outside
  this crate, including the manual's acceleration and stopping conditions.
- Section 5.5.3.1 applies the strongest defense and vulnerability independently.
  Penalties, disconnection and darts bypass defense. Incoming `Damage.amount`
  must already include attacker-side effects. `Buff.attack_pct` is tracked for
  that caller; it is not multiplied into damage a second time here.
- Section 5.1.3 adds heat per detected launch, cools at 10 Hz and maintains
  independent temporary and permanent launch locks. Table 5-3 speed observations
  set independent launch locks. `Launch` is a validated action; `ObserveLaunch`
  accounts for an actual shot even if it happened while locked or over allowance.
  Both update launch counters. An adapter must report each shot only once.
- Section 5.6.3 supplies drone air-support time, minute grants, paid continuation
  and a launch gate. Landing-pad checks, laser and radar countermeasures remain
  external. Section 5.6.7 tracks chassis energy and resupply recharge, but does
  not alter physical motor limits. Buffer power and instant-respawn power boosts
  remain unsupported.

## Result comparison

The section 5.8 result comparison excludes virtual shields from base HP,
counts effective attack damage including shield loss and penalties, and
excludes collision/disconnection damage from the attack total. Confirming an
ambiguous result requires `Adjudicate`; the engine never guesses a winner.

## Ambiguities and application choices

- The printed section 5.8 only compares unequal base HP when a base is destroyed.
  It does not state how unequal surviving bases decide a round. It also does
  not clarify comparisons after destroyed outposts have been rebuilt. These
  cases return `NeedsRefereeDecision` rather than inventing a corrected rule.
- Section 5.1.3's prose says `Q1 > Q2`, while Figure 5-1 says `Q1 >= Q2`.
  The engine follows the figure for the permanent heat lock.
- Assembly income settles on round-aligned ten-second boundaries. The manual
  specifies the interval but not its phase relative to each assembly completion.
- Respawn progress accrues continuously on the world clock. The remaining-time
  input to its formula is rounded to seconds as section 5.2.2 specifies;
  fractional timer units are retained. Low maximum HP configurations respawn
  with at least one HP so a positive recovery cannot leave them defeated.
- Resupply recovery uses round-aligned one-second boundaries. Cooling uses
  round-aligned 100 ms boundaries. Expirations are processed before other timed
  effects, and round end takes precedence over a delivery at exactly 7:00.
- A destroyed base ends the round immediately after that damage command.
  Simultaneous physical hits must be ordered by the caller; batching simultaneous
  destruction is not implemented.
- Outpost rebuild requires continuous detected RFID contact. The two-second
  occupation grace period does not count as continued scanning.

These choices are documented and tested where they affect observable state.
