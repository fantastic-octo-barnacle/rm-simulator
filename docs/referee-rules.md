<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- Copyright (c) 2026 hxyulin <hxyulin@proton.me> -->
# Referee rules digest

What the simulator's referee implements, taken from the RoboMaster 2026
University Championship Rule Manual V2.1.0 (2026-07-17), so the manual does
not have to be re-read for every change. Section, table and figure numbers
point back at the source; "assumption" marks anything the manual does not
say. Live policy is in `crates/rm-simulator-world/src/referee.rs`, `rune.rs`,
`base.rs`, `outpost.rs` and `scoring.rs`. Dynamics, raw contacts and prescribed
motion live in `rm-simulator-physics`. The base damage table below is a limited
V2.2.0 source update; the other clauses retain their documented baseline.

The referee runs the `rm-simulator-gameplay` engine (`Game`) as the authority
for every rule below except the clock's rune schedule and rune activation,
which stay in the world. See [gameplay.md](gameplay.md) for the engine's
implemented rules, its integration boundary and the manual's ambiguous
clauses.

## Contents

- [Match clock (sections 6.5 and 6.6)](#match-clock-sections-65-and-66)
- [Team assignment (section 4.3.2.2, section 5.5.1)](#team-assignment-section-4322-section-551)
- [Rune stages and opportunities (section 5.5.2)](#rune-stages-and-opportunities-section-552)
- [Rune activation mechanics (section 5.5.2.1)](#rune-activation-mechanics-section-5521)
- [Rune buffs (Tables 5-16 and 5-17)](#rune-buffs-tables-5-16-and-5-17)
- [Outposts (section 5.5.1, Tables 5-1 and 5-2, Figure 5-16)](#outposts-section-551-tables-5-1-and-5-2-figure-5-16)
- [Robots (Tables 5-1, 5-2 and 5-11 to 5-14, Figure 5-16)](#robots-tables-5-1-5-2-and-5-11-to-5-14-figure-5-16)
- [Death, respawn and weakness (section 5.2.2)](#death-respawn-and-weakness-section-522)
- [Heat, allowance and economy (sections 5.1.3, 5.3)](#heat-allowance-and-economy-sections-513-53)
- [Victory (section 5.8)](#victory-section-58)
- [Operator overrides](#operator-overrides)
- [Live base scoring and training bots](#live-base-scoring-and-training-bots)
- [Assumptions, not rules](#assumptions-not-rules)
- [Citation index](#citation-index)

## Match clock (sections 6.5 and 6.6)

| Item | Value | Source |
|---|---|---|
| Countdown before the round | 5 s | 6.5 |
| Round length | 7 min | 6.6 |
| Phases modelled | Idle, Countdown, Running, Finished | |
| Rounds | One; no BO2/BO3/BO5 series | assumption |

Idle is free practice: damage applies, but nothing is counted (no heat,
allowance, experience, respawn timer or base-loss accounting) and outpost
protection does not cover the base. The engine ticks at 1 ms; the referee
advances it to the world's round time at the start of each 128 Hz tick.

`StartMatch` enters Countdown; the round clock starts at 0:00 when Running
begins. `SkipTo` jumps the round clock forward, at most to the end of the
round, and grants every opportunity that would have arrived; buffs and the
20 s activation window run on the round clock, so they age with the skip,
while a rune's own 2.5 s hit windows keep world time. `StartMatch` seeds the
runes' target streams from the referee config's seed; `ResetMatch` returns to
Idle and gives the runes back their training policy (Small Rune, lowest
unhit blade, restart after completion).

## Team assignment (section 4.3.2.2, section 5.5.1)

The rune has a red side and a blue side; each team activates its own face
and each outpost stands on its team's half. The manual's field figures
place one team per end but name no axis; the V1.2.0 CAD paints its red
markings on the +x half and its blue markings on the −x half, so the
layout (`layout::side_team`) takes +x as red. A rune face belongs to the
team whose half its front normal faces; an outpost to the team whose half
it stands on. With the shipped package that is one face and one outpost
each. Any other ownership goes in `FieldConfig.referee` (rune and outpost
teams by index; `RefereeConfig::alternating` is the index-based fallback
used by tests). Rune targets and outpost light bars render in the owner's
colour.

## Rune stages and opportunities (section 5.5.2)

| Match time | Event |
|---|---|
| 0:00 | Small Rune stage; each team gets one Small Rune opportunity |
| 1:30 | One more Small Rune opportunity per team |
| 3:00 | Runes convert to Big Rune (keeping hub, orbit and angle); one Big Rune opportunity per team |
| 4:15 | One Big Rune opportunity per team |
| 5:30 | One Big Rune opportunity per team |
| 7:00 | Round over |

Opportunities accumulate; unused ones are not lost. Spending one puts the
team's rune into Activating. An Activating rune that is not fully activated
within 20 s reverts to Inactive and the opportunity is gone. An Activated
rune holds until the referee changes it.

Rune states: Inactive (arms dark, spinning, hits ignored), Activating
(targets lit in turn), Activated (all arms lit). The manual does not say the
Activated arms flash; Figure 5-23 shows them lit. The app's three 2 Hz
blinks are an assumption (`--rune-flash-hz`, `--rune-flashes`).

## Rune activation mechanics (section 5.5.2.1)

- Small Rune: constant speed π/3 rad/s. One target lit at a time; it must be
  hit within 2.5 s or progress resets. Hitting an unlit arm resets progress.
  Five arms in sequence activate the rune. Targets are chosen from a seeded
  stream so every peer sees the same order.
- Big Rune: speed `a·sin(ω·t) + 2.090 − a` rad/s with `a ∈ [0.780, 1.045]`
  and `ω ∈ [1.884, 2.000]`, drawn once per activation from the seeded
  stream. Five groups of two lit targets; the first is required, the second
  scores a bonus within one second.
- Only 17 mm projectiles above 12 m/s normal speed count, inside the 300 mm
  effective disk (150 mm radius, Table 5-1, Figure 5-18).
- Rings: ten rings 15 mm wide across the 150 mm radius, ring 10 at the
  centre (width read off Figure 5-18; the text only gives 1 mm radial
  accuracy). After one Big Rune activation the rune detects only rings 4 to
  10; after two, only rings 7 to 10. Hits outside are rejected as
  `DisabledRing`.

## Rune buffs (Tables 5-16 and 5-17)

Small Rune: 25 % defense for 45 s for the activating team.

Big Rune: buff from the average ring of the recorded hits and the number of
arms lit (5 to 10). Duration by arms lit:

| Arms lit | 5 | 6 | 7 | 8 | 9 | 10 |
|---|---|---|---|---|---|---|
| Duration | 30 s | 35 s | 40 s | 45 s | 50 s | 60 s |

Buff by average ring:

| Average ring | Attack | Defense | Cooling |
|---|---|---|---|
| ≤ 3 | 150 % | 25 % | 1× |
| 3 to 7 | 150 % | 25 % | 2× |
| 7 to 8 | 200 % | 25 % | 2× |
| 8 to 9 | 200 % | 25 % | 3× |
| > 9 | 300 % | 50 % | 5× |

A Small Rune hit that lands on the wrong arm clears the recorded rings. All
three shares apply (section 5.5.3.1): attack scales the team's projectile
damage, defense reduces damage to its bases, outposts and robots, and the
cooling multiplier speeds its barrels' cooling. When buffs overlap the
strongest of each kind applies. Penalty damage ignores defense. An activation
also rewards experience (section 5.5.2): the Small Rune raises the team's
experience rate while its buff lasts, the Big Rune shares a fixed award among
the team's living robots.

## Outposts (section 5.5.1, Tables 5-1 and 5-2, Figure 5-16)

| Item | Value |
|---|---|
| HP | 1500; 750 after a rebuild |
| Rotor speed | 0.8π rad/s after a 5 s spin-up from the round start |
| Rotor stop | At first destruction (holds the angle) or at 3:00 while alive (returns to its initial position over 10 s) |
| Detection area | 101 × 94 mm effective rectangle of the middle armor |
| Detection speed | > 12 m/s (17 mm), > 10 m/s (42 mm) normal speed |
| Detection interval | 50 ms (17 mm), 200 ms (42 mm) per module |
| Damage | 20 HP (17 mm), 200 HP (42 mm); ×1.5 in the 10 mm centre square |

In a match the rotor rests through the countdown and stays stopped for the rest
of the round once it stops; a rebuilt outpost does not spin again. The spin-up
and homing durations are assumptions: the manual says the rotor accelerates and
returns, not how fast. Outside a match the rotor turns at full speed and
restoring HP resumes it.

Destroying an outpost opens its base to damage (`OutpostDestroyed`). Every
1000 HP a base loses in a round gives its team one rebuild opportunity. A
living robot that stays 10 s on its team's destroyed Outpost Buff Point,
before 5:00, rebuilds it with one opportunity (`OutpostRebuilt`). A team's
rotor also stops once the other team's Base Protective Armor expands.

## Buff points (section 5.5.3, Figure 5-24)

A robot occupies a buff point while its chassis body centre lies over the
point's card area and at most 0.45 m above its floor; the status outlives the
last detection by 2 s (section 5.5.3.1). The rulebook prints no coordinates,
so the areas in `rm_simulator_world::zones` were read off Figure 5-24,
registered to the field by the outposts and bases (about 2 cm residual) and
checked against the CAD markings; edges are good to about ±5 cm. Red's areas
lie on +x and blue's are their point mirror. They apply only on the loaded
arena; a field without terrain has none. Weakened or disconnected robots gain
no point effects other than clearing weakness.

| Point | Effect |
|---|---|
| Base (own) | 50 % defense; fourfold respawn progress |
| Resupply (own) | Resupply healing, fourfold respawn progress, exchanges when zone-only exchange is on |
| Trapezoid-Shaped Elevated Ground (own) | 50 % defense |
| Central Elevated Ground | 25 % defense for Hero, Infantry and Sentry; the earliest team there holds it |
| Outpost | 25 % defense and clears weakness on an occupiable point: the own point while the outpost stands, or before 5:00 a destroyed opposing outpost's point while the own outpost stands; the own destroyed point rebuilds |
| Fortress (own) | Once the own outpost is destroyed, the earliest own occupant gets 50 % defense, a heat cooling bonus of `min(Δ/40, 75)` per second and a team reserve of `min(100 + 2⌊Δ/15⌋, 500)` allowance units spent before its own (1 per 17 mm, 10 per 42 mm), Δ being the base HP lost |
| Fortress (opponent) | From 3:00, with its owner's outpost destroyed: 100 % vulnerability; 20 s of occupation (kept paused 3 s after an interruption) expands the owner's Base Protective Armor (`BaseArmorExpanded`) |

Terrain crossings (section 5.5.3.5, `TerrainCrossing`) detect pads in order
within a window; any other point interrupts a crossing, except that the
higher Elevated Ground pad is also the Central Elevated Ground point.

| Crossing | Pads | Window | Buff |
|---|---|---|---|
| Road | lower then upper | 3 s | 25 % defense for 5 s; 15 s before it grants again |
| Elevated Ground | lower then higher | 5 s | 25 % defense for 30 s |
| Launch Ramp | take-off then landing | 10 s | 25 % defense for 30 s |
| Tunnel | an end, the middle, the other end, either way | 3 s | 50 % defense for 10 s and double heat cooling for 120 s |

Repeating a Road, Elevated Ground or Launch Ramp crossing while such a buff
lasts raises it to 50 % for the longer of its remaining and initial duration.
A defeat clears crossing buffs. The Assembly Zone is not placed (engineer only).

## Robots (Tables 5-1, 5-2 and 5-11 to 5-14, Figure 5-16)

Every chassis on the field is a robot: the referee opens a record under the
chassis id and team when the field adds it (`RobotJoined`) and drops it when
the chassis leaves. Each `ChassisPlacement` names its `RobotKind` (Infantry or
Hero) and an optional `performance` type; without one the section 5.4.2
default applies (a long-range Hero, an HP-focused cooling-focused Infantry).
HP, heat limit and cooling come from Tables 5-12 to 5-14 at the robot's
level. Pilots pick a type with `--performance`; it cannot change while a
round runs.

Experience follows section 5.4.1 and Table 5-11: launches, damage dealt and
kills award it, up to level 10 (`LevelUp`). A level-up raises current HP by the
maximum HP gained.

A chassis carries four small armor modules (front, left, back, right) on its
body sides, leaning back 15° (an assumption; the manual gives no single
angle). They detect like the outpost's small armor: the 101 × 94 mm area of
Figure 5-16, above the Table 5-1 normal speeds (12 m/s for 17 mm, 10 m/s for
42 mm), at most once per 50 ms (17 mm) or 200 ms (42 mm) per module. A
detected strike removes 20 HP (17 mm) or 200 HP (42 mm, Table 5-2), scaled by
the shooter's attack buff and the target's defense buff; `RobotDamaged` names
the chassis that fired. 42 mm strikes score only while the attacking team
has a Hero that launched 42 mm recently; a Hero launching past its allowance
(with enforcement on) or three times after a defeat suspends that (section
5.3.2).

An armor module struck by scenery or another robot faster than 1.5 m/s along
its normal loses 2 HP (collision damage, section 5.1.1), at most once per
module per 50 ms, only while Running. The speed threshold is an assumption.

## Death, respawn and weakness (section 5.2.2)

At zero HP a robot is defeated (`RobotDefeated`): its drive is cut, its aim
freezes, it cannot fire and it absorbs no further damage. In a running round
it respawns where it stands (`RobotRespawned`), after 10 s plus a tenth of the
elapsed round in seconds plus 20 s per earlier paid respawn. Standing in its
own base, resupply or living outpost zone accelerates the timer fourfold, as
does a base below 2000 HP.

A respawned robot comes back with 10 % of its maximum HP, weakened and
invincible for 30 s. Weakened robots cannot fire. Reaching an own living
outpost zone clears weakness (`WeaknessCleared`). A paid instant respawn
(`InstantRespawn`, `80 × started minutes + 20 × level` gold) restores full HP
with 3 s of weakness and invincibility.

Respawning in place is an assumption: the manual respawns robots in their
base, which the simulator does not locate. Outside a match pilots still revive
themselves from the defeat menu, restoring only HP; during a match the host
refuses that and the respawn timer applies.

## Heat, allowance and economy (sections 5.1.3, 5.3)

Heat is always on in a match: each 17 mm launch adds 10 heat, each 42 mm launch
100, cooling runs at 10 Hz, and a barrel over its limit locks launches until it
cools; overshooting the limit by a further 100 (17 mm) or 200 (42 mm) locks
the barrel for the round (Figure 5-1's `Q1 >= Q2`).
Every launch in a match consumes allowance and counts toward experience.
Refusing launches at zero allowance is the policy toggle `enforce_allowance`,
off by default; `exchange_requires_zone` (also off) requires a service
zone for exchanges.

Table 5-5 grants income at elapsed 1, 61, 121, 181, 241, 301 and 361 seconds.
Pilots exchange one Table 5-6 unit with O (ten 17 mm rounds for 10 gold) or
I (one 42 mm round for 10 gold); a robot class that cannot fire the caliber is
refused. Per-team exchange limits apply. Remote exchanges, HP purchases and
the Assembly Zone are not connected.

## Victory (section 5.8)

A destroyed base ends the round. At 7:00 the round is compared in order: base
HP, outposts, attack damage, then remaining robot HP. The result travels as
`RoundResult`. When the manual does not decide the comparison the result is
`NeedsRefereeDecision` and a referee settles it with `Adjudicate`.

## Operator overrides

`SetBaseHp`, `SetOutpostHp`, `SetGold`, `SetAllowance`, `SetPolicy`,
`SetPerformance`, `SetRobotHp`, `DamageRobot` (penalty damage),
`ReviveRobot`, `ClearWeakened`, `InstantRespawn`, `BuyAmmo`,
`SetRuneOpportunities` and `SetRuneBuff` edit the match directly. A zero
`SetOutpostHp` counts as a destruction. `SetRuneBuff` cancels an in-progress
activation. These are simulator controls, not additional rules.

## Live base scoring and training bots

Section 5.5.1 supplies 5,000 base HP, a separate initial 150-point shield, and
outpost protection. Live bases spend shield before HP, receive the team's
defense buff, and end Running on destruction. Idle training bypasses outpost
protection. Start/reset restores HP and shield; `SetBaseHp` is an operator override.
The CAD shields still physically block covered lower plates.

The base-specific damage table was checked in the locally available V2.2.0 manual,
Table 5-2: 17 mm does 5 HP to the upper front and 20 to the other five plates;
42 mm does 200. Section 5.5.1's 10 mm centre square multiplies damage by 1.5,
rounded to the nearest HP. Detection geometry/cadence reuse the small-armor path.
The seventh, dart plate accepts
20/200 projectile damage without the centre bonus solely as a requested training
override. Real dart launch/target-mode rules are not implemented here.

`base_layout.rs` fits the six optical sheets and dart light plane from the verified
rm-map-tools package. It uses the exported rail travel, including for prediction
and scoring. Lower placements inherit the exporter's approximate reconstruction.
`base_targets --check` reproduces the fit and tests contacts with full CAD collision.

Bots are privileged training chassis, capped at 32. Constant spin commands use the
ordinary motors and suspension on world ticks. They have normal HP, stop on defeat,
respawn like any robot, never shoot, and are removed independently of connected pilots.

## Assumptions, not rules

- Activated rune arms blink three times at 2 Hz, then stay lit.
- A struck armor module shows grey for 50 ms (`--hit-flash-ms`).
- Ring width 15 mm (figure reading).
- Robots respawn where they fell; the manual respawns them in the base.
- Buff point areas are read off Figure 5-24 (±5 cm); a robot counts when
  its body centre is over the area and within 0.45 m above its floor.
- A team's six Tunnel pads form two tunnels of three pads each; Launch Ramp
  pads are crossed in the jump's direction.
- The Fortress reserve is one pool per team per round, and its Δ is the base
  HP the team has lost.
- Rotor spin-up takes 5 s and homing 10 s.
- Collision damage needs 1.5 m/s along the armor normal.
- One round per match.
- Robot damage rounding to the nearest HP follows the manual's rounding
  note in its terms section; the exact rounding rule for buffs is not
  spelled out for every case.

## Citation index

| Citation | Clause it maps to |
|---|---|
| Section 4.3.2.2 | Team assignment: one team per end, rune face and outpost ownership |
| Section 5.1.1 | Collision damage |
| Section 5.1.3 | Barrel heat, cooling and locks |
| Section 5.2.2 | Respawn timer, weakness, invincibility, paid respawn |
| Section 5.3.2 | Allowance, Hero 42 mm suspension |
| Section 5.4.1 | Experience |
| Section 5.4.2 | Performance types |
| Section 5.5.1 | Outpost HP, rotor start and stop; base HP, shield and outpost protection; rebuild opportunities and scans; base damage centre square |
| Section 5.5.3.1 | Strongest attack, defense and cooling buffs; the 2 s occupation expiry |
| Sections 5.5.3.2 to 5.5.3.9 | Buff point effects, terrain crossings and the Fortress |
| Section 5.8 | Round result |
| Section 5.5.2 | Rune stages, opportunities and buff sources |
| Section 5.5.2.1 | Rune activation mechanics and ring restrictions |
| Section 6.5 | Countdown before the round, 5 s |
| Section 6.6 | Round length, 7 min |
| Table 5-1 | Detection speeds and the rune's 17 mm > 12 m/s criterion |
| Table 5-2 | Damage values for the outpost, robots and (in V2.2.0) bases |
| Table 5-5 | Income schedule |
| Table 5-6 | Resupply exchange |
| Table 5-7 | Initial allowances |
| Table 5-11 | Experience per level |
| Tables 5-12 to 5-14 | Hero and Infantry HP, heat limit and cooling by performance type and level |
| Table 5-16 | Big Rune buff from the number of lit arms |
| Table 5-17 | Big Rune buff from the average hit ring |
| Figure 5-16 | Armor detection area, 101 × 94 mm |
| Figure 5-18 | Rune effective disk and ring widths |
| Figure 5-23 | Activated rune arms are lit |
| Figure 5-24 | Buff point card areas (read off the drawing) |
