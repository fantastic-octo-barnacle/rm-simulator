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

The `rm-simulator-gameplay::live` component now supplies resource tracking to
this referee. The separate full-match engine also extends standalone coverage. See [gameplay.md](gameplay.md) for its implemented
rules, integration boundary and ambiguous clauses in the English manual.

## Match clock (sections 6.5 and 6.6)

| Item | Value | Source |
|---|---|---|
| Countdown before the round | 5 s | 6.5 |
| Round length | 7 min | 6.6 |
| Phases modelled | Idle, Countdown, Running, Finished | |

`StartMatch` enters Countdown; the round clock starts at 0:00 when Running
begins. `SkipTo` jumps the round clock forward, at most to the end of the
round, and grants every opportunity that would have arrived; buffs and the
20 s activation window run on the round clock, so they age with the skip,
while a rune's own 2.5 s hit windows keep world time. `StartMatch` seeds the
runes' target streams from the referee config's seed; `ResetMatch` returns to
Idle and gives the runes back their training policy (Small Rune, lowest
unhit blade, restart after completion).

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
- Only 17 mm projectiles above 12 m/s normal speed count, inside the 150 mm
  effective disk (Table 5-1, Figure 5-18).
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

A Small Rune hit that lands on the wrong arm clears the recorded rings. In
the simulator only the defense share does anything: it scales damage to the
team's bases, outposts and robots, rounded to the
nearest HP. Attack and cooling are carried in the snapshot for clients.

## Outposts (section 5.5.1, Tables 5-1 and 5-2, Figure 5-16)

| Item | Value |
|---|---|
| HP | 1500 |
| Rotor speed | 0.8π rad/s, frozen when destroyed |
| Detection area | 101 × 94 mm effective rectangle of the middle armor |
| Detection speed | > 12 m/s (17 mm), > 10 m/s (42 mm) normal speed |
| Detection interval | 50 ms (17 mm), 200 ms (42 mm) per module |
| Damage | 20 HP (17 mm), 200 HP (42 mm); ×1.5 in the 10 mm centre square |

The referee applies the owning team's defense buff to outpost damage and
reports `OutpostDestroyed` when HP reaches zero.

## Robots (Tables 5-1, 5-2 and 5-13, Figure 5-16)

Every chassis on the field is a robot: the referee opens an HP record under
the chassis id and team when the field adds it (`RobotJoined`) and drops it
when the chassis leaves. All robots are the one `RobotConfig { kind, max_hp
}` of the configuration, by default a level-1 HP-focused infantry with
200 HP (Table 5-13); no levelling is modelled.

A chassis carries four small armor modules (front, left, back, right) on its
body sides, leaning back 15° (an assumption; the manual gives no single
angle). They detect like the outpost's small armor: the 101 × 94 mm area of
Figure 5-16, above the Table 5-1 normal speeds (12 m/s for 17 mm, 10 m/s for
42 mm), at most once per 50 ms (17 mm) or 200 ms (42 mm) per module. A
detected strike removes 10 HP (17 mm) or 100 HP (42 mm), the Table 5-2
values as recalled, through the owning team's defense buff; `RobotDamaged`
names the chassis that fired. At zero HP the robot is defeated
(`RobotDefeated`): its drive is cut, its aim freezes and it cannot fire until
`ReviveRobot` or `SetRobotHp` restores it. Hosted pilots explicitly request revival through the defeat menu
as a training shortcut, restoring only HP and retaining position and ammo/gold.
The separate **Reset robot to spawn** button in the F3 debug panel also returns
the robot upright to its original spawn. This is a server policy, not the competition respawn rule;
standalone world rules still retain defeated robots until explicitly revived. A defeated robot absorbs no
further damage. `DamageRobot` applies the same path without a shooter.

The sentry and full competition progression remain outside this live referee.
Live base scoring is described below.

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

## Assumptions, not rules

- Activated rune arms blink three times at 2 Hz, then stay lit.
- A struck armor module shows grey for 50 ms (`--hit-flash-ms`).
- Ring width 15 mm (figure reading).
- Robot damage rounding to the nearest HP follows the manual's rounding
  note in its terms section; the exact rounding rule for buffs is not
  spelled out for every case.

## Live resources and operator overrides

The referee drives `rm-simulator-gameplay::live::Resources` on its round clock.
Table 5-5 supplies default income at elapsed 1, 61, 121, 181, 241, 301 and
361 seconds. Income amounts and enablement are editable in the HTTP panel.
Disabled grants are skipped; clock skips process crossed boundaries on the next
tick. Pause freezes them. Start/reset clears gold and counts but preserves policy.

Successful Running-phase shots consume one allowance and increment the matching
17 mm or 42 mm count. Exhaustion blocks fire only when enabled in the panel;
tracking alone defaults on, enforcement off. Invalid and defeated-shooter launches
do not count. Initial allowances default to zero for the simulated infantry,
from Table 5-7, and apply on start/reset and robot admission. Operators can edit
current allowances/counters and perform immediate resupply against team gold at
configurable per-projectile prices. This is an operator facility, not a claim
that RFID, exchange limits or remote purchases are implemented.

`SetOutpostHp` changes the physical outpost, including destruction and revival.
Start/reset restores 1500 HP. `SetRuneOpportunities` adjusts availability;
`SetRuneBuff` overrides defense, reported attack/cooling and expiry, cancelling
an in-progress activation. These are simulator controls, not additional rules.
Live base damage now follows the base scoring path described below.

Pilots can use O/I to buy one 17 mm/42 mm round during Running. Purchases use
team gold and the configured prices, defaulting to 1/10, and fail atomically
when funds are insufficient. The host restricts purchases to the sender's chassis.


## Live base scoring and training bots

Section 5.5.1 supplies 5,000 base HP, a separate initial 150-point shield, and
outpost protection. Live bases spend shield before HP, receive the existing team
defense buff, and end Running on destruction. Idle training bypasses outpost
protection. Start/reset restores HP and shield; `SetBaseHp` is an operator override.
The CAD shields still physically block covered lower plates.

The base-specific damage table was checked in the locally available V2.2.0 manual,
Table 5-2: 17 mm does 5 HP to the upper front and 20 to the other five plates;
42 mm does 200. Section 5.5.1's 10 mm centre square multiplies damage by 1.5,
rounded to the nearest HP. Detection geometry/cadence reuse the small-armor path.
Unrelated robot damage constants are unchanged. The seventh, dart plate accepts
20/200 projectile damage without the centre bonus solely as a requested training
override. Real dart launch/target-mode rules are not implemented here.

`base_layout.rs` fits the six optical sheets and dart light plane from the verified
rm-map-tools package. It uses the exported rail travel, including for prediction
and scoring. Lower placements inherit the exporter's approximate reconstruction.
`base_targets --check` reproduces the fit and tests contacts with full CAD collision.

Bots are privileged training chassis, capped at 32. Constant spin commands use the
ordinary motors and suspension on world ticks. They have normal HP, stop on defeat,
resume after revival, never shoot, and are removed independently of connected pilots.
