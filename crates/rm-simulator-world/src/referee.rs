// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Match referee on the field clock: teams, the round timer, Power Rune
//! availability and buffs, outpost ownership, and the standalone gameplay
//! engine that owns robot, base and outpost HP.
//!
//! Sources in the RMUC 2026 rule manual (V2.1.0): section 6.5 (five-second
//! countdown), 6.6 (seven-minute round), 5.5.2 (rune opportunities at 0:00
//! and 1:30 for the Small Rune, 3:00, 4:15 and 5:30 for the Big Rune, the
//! 20 s activating window, the Small Rune's 25 % defense buff for 45 s,
//! Table 5-16 and Table 5-17 for the Big Rune's buff from the average ring
//! and the number of lit arms, and the detection rings that remain after
//! each Big Rune activation), and 5.5.1 (outposts).
//!
//! Damage, experience, levels, heat, respawn, weakness, outpost protection
//! and rebuilds, and the section 5.8 round result come from
//! [`rm_simulator_gameplay::Game`], which the referee drives on the round clock
//! at its own 1 ms resolution. The field mirrors the game's base and outpost HP
//! onto its physical objects.
//!
//! Outside a match (`Idle`) the runes run their training policy and the game
//! is free practice: strikes deal damage, but nothing protects a base,
//! experience is not earned and a defeated robot waits for an operator.
//! `StartMatch` switches the runes to referee control and seeds their target
//! streams from the config's seed: dark until a team spends an opportunity,
//! then Activating for at most 20 s, then Activated for the buff's duration,
//! then unavailable. `ResetMatch` drops the seeds again. The referee never
//! reads host time; it advances only through `tick`.
use crate::{
    ArmorHit, ArmorTarget,
    rune::{HitOutcome, Rune, RuneError, RuneKind, RuneState},
};
use rm_simulator_gameplay as gp;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Section 6.5.
pub const COUNTDOWN_NS: u64 = 5_000_000_000;
/// Section 6.6.
pub const ROUND_NS: u64 = 420_000_000_000;
/// Section 5.5.2: an Activating rune not activated within 20 s reverts.
pub const RUNE_ACTIVATING_WINDOW_NS: u64 = 20_000_000_000;
/// Section 5.5.2: Small Rune until three minutes, Big Rune afterwards.
pub const BIG_RUNE_STAGE_NS: u64 = 180_000_000_000;
/// Section 5.5.2: one Small Rune opportunity at the start and at 1:30.
pub const SMALL_RUNE_OPPORTUNITY_NS: [u64; 2] = [0, 90_000_000_000];
/// Section 5.5.2: one Big Rune opportunity at 3:00, 4:15 and 5:30.
pub const BIG_RUNE_OPPORTUNITY_NS: [u64; 3] = [180_000_000_000, 255_000_000_000, 330_000_000_000];
/// Section 5.5.2: Small Rune buff.
pub const SMALL_RUNE_DEFENSE_PCT: u32 = 25;
/// Section 5.5.2: the Small Rune's 25 % defense buff lasts 45 s.
pub const SMALL_RUNE_BUFF_NS: u64 = 45_000_000_000;
/// Section 5.5.1: the outpost's middle armor stops rotating at 3:00.
pub const OUTPOST_ROTOR_STOP_NS: u64 = 180_000_000_000;
/// Table 5-2: HP a robot loses when one of its armor modules collides.
pub const COLLISION_DAMAGE_HP: u32 = 2;
/// Ten rings across the 150 mm effective radius, ring 10 innermost; the
/// width was read off Figure 5-18, the text only gives 1 mm radial accuracy.
pub const RING_WIDTH_M: f64 = 0.015;
/// Recent events kept in the snapshot.
const EVENT_MEMORY: usize = 48;
/// Recent events the game keeps between two referee drains. The referee
/// translates them after every call, so this only has to cover one call.
const GAME_EVENT_MEMORY: usize = 64;

pub use rm_simulator_physics::Team;

/// The robot classes a chassis can be recorded as. Every chassis names its
/// class in its [`crate::ChassisPlacement`]; the class fixes the gameplay
/// robot's weapon, levelling and default performance (section 5.4.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RobotKind {
    /// Hero: 42 mm gun.
    Hero,
    /// Engineer: no launcher.
    Engineer,
    /// Infantry: 17 mm gun. The default class of a placement that names none.
    #[default]
    Infantry,
    /// Sentry: 17 mm gun.
    Sentry,
    /// Aerial drone: 17 mm gun that needs air support to fire.
    Drone,
}
impl RobotKind {
    /// The gameplay engine's robot type for this class.
    pub fn gameplay(self) -> gp::RobotKind {
        match self {
            Self::Hero => gp::RobotKind::Hero,
            Self::Engineer => gp::RobotKind::Engineer,
            Self::Infantry => gp::RobotKind::Infantry,
            Self::Sentry => gp::RobotKind::Sentry,
            Self::Drone => gp::RobotKind::Drone,
        }
    }
    fn from_gameplay(kind: gp::RobotKind) -> Self {
        match kind {
            gp::RobotKind::Hero => Self::Hero,
            gp::RobotKind::Engineer => Self::Engineer,
            gp::RobotKind::Sentry => Self::Sentry,
            gp::RobotKind::Drone => Self::Drone,
            _ => Self::Infantry,
        }
    }
}

/// Performance for robots the section 5.4.2 tables do not cover. Hero,
/// Infantry, Engineer and Sentry use their rulebook values unless the
/// placement selects a performance; anything else uses `fallback`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotConfig {
    /// HP, power, heat limit and cooling for a robot without rulebook values.
    pub fallback: gp::Stats,
}
impl Default for RobotConfig {
    /// A 200 HP robot with a 100 heat limit cooling at 20 per second.
    fn default() -> Self {
        Self {
            fallback: gp::Stats {
                max_hp: 200,
                chassis_power_w: 0,
                heat_limit: 100,
                cooling_per_s: 20,
            },
        }
    }
}

/// Match rules for one field: who owns each rune and outpost, what every
/// chassis becomes, how long the round lasts and the seed the runes draw from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefereeConfig {
    /// Owner of each field rune, by rune index.
    pub rune_teams: Vec<Team>,
    /// Owner of each field outpost, by outpost index; at most one per team.
    pub outpost_teams: Vec<Team>,
    /// Performance for robot classes without rulebook values.
    pub robot: RobotConfig,
    /// Round length in ns. Section 6.6 gives 7 min; the constructor refuses
    /// zero and anything longer.
    pub round_ns: u64,
    /// Countdown before the round in ns. Section 6.5 gives 5 s.
    pub countdown_ns: u64,
    /// Seed for target selection and Big Rune motion parameters.
    pub seed: u64,
}
impl Default for RefereeConfig {
    fn default() -> Self {
        Self::alternating(1, 2)
    }
}
impl RefereeConfig {
    /// Rune and outpost `i` belong to red when `i` is even, else blue.
    pub fn alternating(runes: usize, outposts: usize) -> Self {
        let team = |i: usize| {
            if i.is_multiple_of(2) {
                Team::Red
            } else {
                Team::Blue
            }
        };
        Self::owned(
            (0..runes).map(team).collect(),
            (0..outposts).map(team).collect(),
        )
    }

    /// Explicit owners per rune and outpost index, with the default robot,
    /// clock and seed.
    pub fn owned(rune_teams: Vec<Team>, outpost_teams: Vec<Team>) -> Self {
        Self {
            rune_teams,
            outpost_teams,
            robot: RobotConfig::default(),
            round_ns: ROUND_NS,
            countdown_ns: COUNTDOWN_NS,
            seed: 0x5EED_2026,
        }
    }
}

/// Where the match is in the section 6.5 and 6.6 sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchPhase {
    /// No match: runes run their training policy and damage is free practice.
    Idle,
    /// A started match waiting out its countdown.
    Countdown,
    /// The round clock is running.
    Running,
    /// The round ended by timeout, base destruction or an operator stop; the
    /// round clock is frozen at that instant.
    Finished,
}

/// Which Power Rune stage the round is in. Every rune converts at 3:00
/// (section 5.5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuneStage {
    /// From the start until 3:00: one target at a time.
    Small,
    /// From 3:00 to the end: pairs of targets.
    Big,
}
impl RuneStage {
    /// The rune mode this stage runs.
    pub fn kind(self) -> RuneKind {
        match self {
            RuneStage::Small => RuneKind::Small,
            RuneStage::Big => RuneKind::Big,
        }
    }
}

/// An active rune buff (section 5.5.2). The gameplay engine applies the
/// attack, defense and cooling effects to the whole team; this record drives
/// the rune's presentation. Times are on the round clock
/// (`RefereeSnapshot::match_time_ns`), so a `SkipTo` ages the buff with the round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuffState {
    /// Which stage granted the buff.
    pub source: RuneStage,
    /// Share of incoming damage removed, in percent.
    pub defense_pct: u32,
    /// Projectile damage multiplier in percent; 100 is no attack buff.
    pub attack_pct: u32,
    /// Barrel cooling multiplier.
    pub cooling_multiplier: u32,
    /// Round clock time the buff was granted.
    pub started_ns: u64,
    /// Round clock time the buff ends.
    pub expires_ns: u64,
}

/// One team's round state: rune opportunities, activations and the current buff.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamSnapshot {
    /// Which team this is.
    pub team: Team,
    /// Unspent Small and Big Rune opportunities; they accumulate.
    pub rune_opportunities: u32,
    /// Big Rune activations completed, which narrows the detecting rings.
    pub big_rune_activations: u32,
    /// Round clock time the rune was triggered, while it is Activating.
    pub rune_activating_since_ns: Option<u64>,
    /// The team's active buff, when it has one.
    pub buff: Option<BuffState>,
}

/// One chassis' robot record, summarised from the gameplay engine's state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobotSnapshot {
    /// Chassis id the robot was opened under.
    pub id: u32,
    /// Team the chassis plays for.
    pub team: Team,
    /// Configured robot class.
    pub kind: RobotKind,
    /// Remaining HP; zero means defeated.
    pub hp: u32,
    /// Maximum HP at the robot's current level (section 5.4.2).
    pub max_hp: u32,
    /// Current level (Table 5-11).
    pub level: u8,
    /// Whether the robot is weakened after a respawn (section 5.2.2).
    pub weakened: bool,
}
impl RobotSnapshot {
    /// Whether the robot has any HP left.
    pub fn alive(&self) -> bool {
        self.hp > 0
    }
}

/// Something the referee decided or observed, kept in the snapshot's event
/// list for the panel. Events are informational; the authoritative state is
/// the snapshot's own fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RefereeEvent {
    /// A base lost HP or shield.
    BaseDamaged {
        /// Team whose base was hit.
        team: Team,
        /// HP plus shield removed, after the team's defenses.
        amount: u32,
        /// Base HP after the hit.
        hp: u32,
        /// Base shield after the hit.
        shield_hp: u32,
        /// The chassis whose projectile did it; `None` otherwise.
        shooter: Option<u32>,
    },
    /// A base reached zero HP.
    BaseDestroyed {
        /// Team whose base was destroyed.
        team: Team,
    },
    /// `StartMatch` entered Countdown.
    CountdownStarted,
    /// The countdown elapsed and the round clock started.
    MatchStarted,
    /// The round ended.
    MatchFinished,
    /// The section 5.8 result of the round, or a referee's decision.
    RoundResult(gp::RoundResult),
    /// `ResetMatch` returned the field to Idle.
    MatchReset,
    /// The round reached 3:00 and the runes converted to Big.
    RuneStage(RuneStage),
    /// A team received a rune opportunity.
    RuneOpportunity {
        /// Team that received it.
        team: Team,
        /// Stage the opportunity belongs to.
        stage: RuneStage,
        /// Opportunities the team now holds.
        total: u32,
    },
    /// A team spent an opportunity and its rune began Activating.
    RuneActivating {
        /// Team that spent the opportunity.
        team: Team,
    },
    /// Twenty seconds passed without activating the rune (section 5.5.2).
    RuneActivationExpired {
        /// Team whose attempt expired.
        team: Team,
    },
    /// A rune completed and granted its buff.
    RuneActivated {
        /// Team that activated it.
        team: Team,
        /// Stage the rune was in.
        stage: RuneStage,
        /// Arms lit: blades hit on a Small Rune, rings recorded on a Big Rune.
        arms: u32,
        /// Mean ring of the recorded hits; zero when none were recorded.
        average_ring: f64,
        /// The buff that was granted.
        buff: BuffState,
    },
    /// A team's buff ran out and its rune went dark.
    BuffExpired {
        /// Team whose buff expired.
        team: Team,
    },
    /// A robot lost HP.
    RobotDamaged {
        /// Chassis id of the robot.
        robot: u32,
        /// HP removed, after defenses.
        amount: u32,
        /// Robot HP after the hit.
        hp: u32,
        /// The chassis whose projectile did it; `None` otherwise.
        shooter: Option<u32>,
    },
    /// A chassis joined the field and got a robot record.
    RobotJoined {
        /// Chassis id the robot was opened under.
        robot: u32,
        /// Team the chassis plays for.
        team: Team,
    },
    /// A chassis left the field and its robot record was dropped.
    RobotLeft {
        /// Chassis id that left.
        robot: u32,
    },
    /// A robot reached zero HP.
    RobotDefeated {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// An operator restored a defeated robot.
    RobotRevived {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// A defeated robot returned through its respawn timer or a paid
    /// instant respawn (section 5.2.2).
    RobotRespawned {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// A robot's weakened state ended.
    WeaknessCleared {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// A robot reached a new level (Table 5-11).
    LevelUp {
        /// Chassis id of the robot.
        robot: u32,
        /// New level.
        level: u8,
    },
    /// An outpost reached zero HP.
    OutpostDestroyed {
        /// Outpost index in the field's outpost list.
        outpost: u32,
        /// Team that owned it.
        team: Team,
    },
    /// A destroyed outpost was rebuilt with 750 HP (section 5.5.1).
    OutpostRebuilt {
        /// Team whose outpost was rebuilt.
        team: Team,
    },
    /// A robot completed a terrain crossing and gained its buff (section
    /// 5.5.3.5).
    TerrainCrossing {
        /// Robot that crossed.
        robot: u32,
        /// Crossing kind.
        kind: gp::ZoneKind,
    },
    /// Twenty seconds of opposing Fortress occupation expanded a team's Base
    /// Protective Armor (section 5.5.3.9).
    BaseArmorExpanded {
        /// Team whose base armor expanded.
        team: Team,
    },
}

/// One event with the field time, and the round time when a round was running.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedEvent {
    /// Field time the event was recorded at.
    pub time_ns: u64,
    /// Round time, when the round was running.
    pub match_time_ns: Option<u64>,
    /// What happened.
    pub event: RefereeEvent,
}

/// The referee's public state at one tick: the round clock, both teams, the
/// robot records, ownership, the gameplay state and the recent events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefereeSnapshot {
    /// Whether each base's protective armor is open or opening, indexed red
    /// then blue: an operator override or a Fortress capture.
    pub base_open: [bool; 2],
    /// When each base set off toward its `base_open` state, on the field
    /// clock; `None` once it rests there. See [`Self::base_open_fraction`].
    pub base_moved_ns: [Option<u64>; 2],
    /// Dart door overrides, indexed red then blue; open by default.
    pub dart_door_open: [bool; 2],
    /// When both Dart Detection Modules started sweeping along their rails,
    /// on the field clock; `None` while they rest, the default. See
    /// [`Self::dart_target_position`].
    pub dart_target_since_ns: Option<u64>,
    /// The gameplay engine's whole state: gold, HP, experience, heat,
    /// respawn timers, buffs and the round result.
    pub game: gp::Snapshot,
    /// Where the match is.
    pub phase: MatchPhase,
    /// Elapsed round time; frozen at the end of a finished match.
    pub match_time_ns: u64,
    /// Round time left, `round_ns` minus `match_time_ns`.
    pub remaining_ns: u64,
    /// Section 6.5 countdown time left while in [`MatchPhase::Countdown`];
    /// zero in every other phase.
    pub countdown_remaining_ns: u64,
    /// Small Rune stage or Big Rune stage.
    pub stage: RuneStage,
    /// Both teams, indexed red then blue.
    pub teams: [TeamSnapshot; 2],
    /// Every chassis' robot record, in join order.
    pub robots: Vec<RobotSnapshot>,
    /// Owner of each rune, by rune index.
    pub rune_teams: Vec<Team>,
    /// Owner of each outpost, by outpost index.
    pub outpost_teams: Vec<Team>,
    /// Oldest first.
    pub events: Vec<TimedEvent>,
}

pub use rm_simulator_physics::motion::{
    BASE_TRAVEL_NS, DART_TARGET_PERIOD_NS, Mechanism, MechanismState, dart_target_fraction,
    dart_target_position,
};

impl RefereeSnapshot {
    /// Eased travel of team `index`'s base armor at field time `time_ns`, 0
    /// shut to 1 open, moving over [`BASE_TRAVEL_NS`].
    pub fn base_open_fraction(&self, index: usize, time_ns: u64) -> f64 {
        rm_simulator_physics::motion::base_open_fraction(
            self.base_open[index],
            self.base_moved_ns[index],
            time_ns,
        )
    }
    /// Dart rail position at field time `time_ns`, 0 at rest to 1.
    pub fn dart_target_position(&self, time_ns: u64) -> f64 {
        dart_target_position(self.dart_target_since_ns, time_ns)
    }
}

/// Resolve match decisions before physics reads mechanism motion.
pub fn mechanism_state(referee: Option<&Referee>, time_ns: u64) -> MechanismState {
    MechanismState {
        base_open_fraction: referee.map_or([0.0; 2], |r| {
            std::array::from_fn(|i| {
                rm_simulator_physics::motion::base_open_fraction(
                    r.base_open[i],
                    r.base_moved_ns[i],
                    time_ns,
                )
            })
        }),
        dart_door_open: referee.map_or([true; 2], |r| r.dart_door_open),
        dart_target_fraction: dart_target_position(
            referee.and_then(|r| r.dart_target_since_ns),
            time_ns,
        ),
    }
}
/// Resolve public match state for presentation clearance queries.
pub fn mechanism_view(referee: Option<&RefereeSnapshot>, time_ns: u64) -> MechanismState {
    MechanismState {
        base_open_fraction: referee.map_or([0.0; 2], |r| {
            std::array::from_fn(|i| r.base_open_fraction(i, time_ns))
        }),
        dart_door_open: referee.map_or([true; 2], |r| r.dart_door_open),
        dart_target_fraction: referee.map_or(0.0, |r| r.dart_target_position(time_ns)),
    }
}

/// Operator and organiser inputs. `Field::referee_command` mirrors base and
/// outpost HP onto the physical objects after the referee applies them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RefereeCommand {
    /// Set a base's HP and shield directly; zero HP in a round ends it.
    SetBaseHp {
        /// Team whose base to edit.
        team: Team,
        /// New HP, at most `base::INITIAL_HP`.
        hp: u32,
        /// New shield, at most `base::INITIAL_HP`.
        shield_hp: u32,
    },
    /// Manual CAD mechanism override, independent of match rules.
    SetBaseOpen {
        /// Team whose base cover to move.
        team: Team,
        /// Whether the cover stands open.
        open: bool,
    },
    /// Manual CAD mechanism override for the dart door.
    SetDartDoorOpen {
        /// Team whose dart door to move.
        team: Team,
        /// Whether the door stands open.
        open: bool,
    },
    /// Start or stop both Dart Detection Modules sweeping along their rails.
    /// They rest by default; a sweep sets off from rest, and stopping returns
    /// them there. An app training control; the rulebook's dart target modes
    /// are not modelled.
    SetDartTargetMoving {
        /// Whether the targets sweep.
        moving: bool,
    },
    /// Set an outpost's HP directly. In training a restored tower spins
    /// again; in a match a stopped rotor stays stopped.
    SetOutpostHp {
        /// Outpost index in the field's outpost list.
        outpost: u32,
        /// New HP, at most `outpost::INITIAL_HP`.
        hp: u32,
    },
    /// Set a team's gold.
    SetGold {
        /// Team to edit.
        team: Team,
        /// New balance.
        gold: u32,
    },
    /// Set a robot's projectile allowance, 17 mm then 42 mm.
    SetAllowance {
        /// Chassis id of the robot.
        robot: u32,
        /// New allowance in caliber order.
        allowance: [u32; 2],
    },
    /// Replace the rule switches; the live default leaves allowance
    /// unenforced and lets pilots buy ammunition anywhere.
    SetPolicy(gp::Policy),
    /// Select a robot's performance type (section 5.4.2); refused while a
    /// round is running.
    SetPerformance {
        /// Chassis id of the robot.
        robot: u32,
        /// New performance; it must fit the robot's class.
        performance: gp::Performance,
    },
    /// Buy rounds for a robot's allowance with team gold at its own
    /// service zone (section 5.3.2, Tables 5-7 to 5-9).
    BuyAmmo {
        /// Chassis id of the robot.
        robot: u32,
        /// Caliber bought.
        caliber: crate::Caliber,
        /// Rounds to buy; a multiple of 10 for 17 mm.
        amount: u32,
    },
    /// Set a team's unspent rune opportunities.
    SetRuneOpportunities {
        /// Team to edit.
        team: Team,
        /// Opportunities the team should hold.
        opportunities: u32,
    },
    /// Operator override; zero duration clears the buff and deactivates the rune.
    SetRuneBuff {
        /// Team to grant the buff to.
        team: Team,
        /// Defense share in percent, at most 100.
        defense_pct: u32,
        /// Attack multiplier in percent, at most 1000.
        attack_pct: u32,
        /// Cooling multiplier, at most 100.
        cooling_multiplier: u32,
        /// Buff duration in ns, at most the round length.
        duration_ns: u64,
    },
    /// Idle -> Countdown -> Running.
    StartMatch,
    /// Skip the round clock forward (testing), at most to the end of the
    /// round. Missed opportunities are granted, and buffs, activation
    /// windows and every gameplay timer age with it; the runes' own hit
    /// windows keep world time.
    SkipTo {
        /// Round time to jump to, at most `round_ns`.
        match_time_ns: u64,
    },
    /// End a counting-down or running match now.
    FinishMatch,
    /// Decide a finished round the section 5.8 comparison could not.
    Adjudicate {
        /// Winning team, or `None` for a draw.
        winner: Option<Team>,
    },
    /// Back to Idle: runes return to their training policy.
    ResetMatch,
    /// Spend one opportunity to put the team's rune into Activating.
    ActivateRune {
        /// Team whose rune to trigger.
        team: Team,
    },
    /// Add one opportunity without spending anything.
    GrantRuneOpportunity {
        /// Team to grant it to.
        team: Team,
    },
    /// Referee penalty damage to a robot: no defense or invincibility applies.
    DamageRobot {
        /// Chassis id of the robot.
        robot: u32,
        /// HP to remove; refused for an already defeated robot.
        amount: u32,
    },
    /// Restore a robot to full HP in place, clearing its respawn timer and
    /// weakness.
    ReviveRobot {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// Revive a defeated robot at once for gold (section 5.2.2).
    InstantRespawn {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// End a robot's weakened state as an own service zone would.
    ClearWeakened {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// Set a robot's HP, clamped to its maximum; zero defeats it.
    SetRobotHp {
        /// Chassis id of the robot.
        robot: u32,
        /// New HP.
        hp: u32,
    },
}

/// The clock an absolute referee timestamp is measured on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampClock {
    /// The field's own tick clock (`FieldSnapshot::time_ns`).
    Field,
    /// The round clock (`RefereeSnapshot::match_time_ns`), which `SkipTo`
    /// moves and which stops when a round finishes.
    Round,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TeamState {
    rune_opportunities: u32,
    big_rune_activations: u32,
    activating_since_ns: Option<u64>,
    buff: Option<BuffState>,
    /// Rings struck during the current activation attempt.
    attempt_rings: Vec<u32>,
    last_completed_groups: u32,
}
impl TeamState {
    fn fresh() -> Self {
        Self {
            rune_opportunities: 0,
            big_rune_activations: 0,
            activating_since_ns: None,
            buff: None,
            attempt_rings: Vec::new(),
            last_completed_groups: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scheduled {
    SmallOpportunity,
    BigStage,
    BigOpportunity,
}
const SCHEDULE: [(u64, Scheduled); 6] = [
    (SMALL_RUNE_OPPORTUNITY_NS[0], Scheduled::SmallOpportunity),
    (SMALL_RUNE_OPPORTUNITY_NS[1], Scheduled::SmallOpportunity),
    (BIG_RUNE_STAGE_NS, Scheduled::BigStage),
    (BIG_RUNE_OPPORTUNITY_NS[0], Scheduled::BigOpportunity),
    (BIG_RUNE_OPPORTUNITY_NS[1], Scheduled::BigOpportunity),
    (BIG_RUNE_OPPORTUNITY_NS[2], Scheduled::BigOpportunity),
];

/// Ring struck at a signed width/height offset from the target centre.
///
/// ```rust
/// use rm_simulator_world::referee::ring_of;
///
/// // Ring 10 is the centre; each 15 mm band outward drops one ring.
/// assert_eq!(ring_of([0.0, 0.0]), 10);
/// assert_eq!(ring_of([0.02, 0.0]), 9);
/// assert_eq!(ring_of([0.15, 0.0]), 1);
/// ```
pub fn ring_of(offset_m: [f64; 2]) -> u32 {
    let radius = offset_m[0].hypot(offset_m[1]);
    (10 - (radius / RING_WIDTH_M).floor().min(9.0) as u32).clamp(1, 10)
}

/// Damage after a defense buff, rounded to the nearest integer (section 2 terms: HP loss is rounded).
///
/// ```rust
/// use rm_simulator_world::referee::defended;
///
/// // 25 % defense keeps three quarters of the damage.
/// assert_eq!(defended(20, 25), 15);
/// // 7.5 HP rounds up to 8.
/// assert_eq!(defended(10, 25), 8);
/// assert_eq!(defended(20, 0), 20);
/// ```
pub fn defended(damage: u32, defense_pct: u32) -> u32 {
    let kept = u64::from(damage) * u64::from(100 - defense_pct.min(100));
    u32::try_from((kept + 50) / 100).unwrap_or(u32::MAX)
}

/// Table 5-16 and Table 5-17: Big Rune buff from the average ring and the lit
/// arm count, granted at round clock time `match_time_ns`.
///
/// ```rust
/// use rm_simulator_world::referee::{big_rune_buff, RuneStage};
///
/// // An average ring above 9 gives 300 % attack, 50 % defense and 5x cooling.
/// let buff = big_rune_buff(9.5, 10, 0);
/// assert_eq!(buff.source, RuneStage::Big);
/// assert_eq!(buff.defense_pct, 50);
/// assert_eq!(buff.attack_pct, 300);
/// assert_eq!(buff.cooling_multiplier, 5);
/// assert_eq!(buff.expires_ns, 60_000_000_000);
/// // The arm count sets the duration: eight arms last 45 s.
/// assert_eq!(big_rune_buff(5.0, 8, 0).expires_ns, 45_000_000_000);
/// ```
pub fn big_rune_buff(average_ring: f64, arms: u32, match_time_ns: u64) -> BuffState {
    let (attack_pct, defense_pct, cooling_multiplier) = if average_ring <= 3.0 {
        (150, 25, 1)
    } else if average_ring <= 7.0 {
        (150, 25, 2)
    } else if average_ring <= 8.0 {
        (200, 25, 2)
    } else if average_ring <= 9.0 {
        (200, 25, 3)
    } else {
        (300, 50, 5)
    };
    let duration_s = match arms.clamp(5, 10) {
        5 => 30,
        6 => 35,
        7 => 40,
        8 => 45,
        9 => 50,
        _ => 60,
    };
    BuffState {
        source: RuneStage::Big,
        defense_pct,
        attack_pct,
        cooling_multiplier,
        started_ns: match_time_ns,
        expires_ns: match_time_ns + duration_s * 1_000_000_000,
    }
}

/// World team to gameplay team.
pub fn game_team(team: Team) -> gp::Team {
    match team {
        Team::Red => gp::Team::Red,
        Team::Blue => gp::Team::Blue,
    }
}
fn world_team(team: gp::Team) -> Team {
    match team {
        gp::Team::Red => Team::Red,
        gp::Team::Blue => Team::Blue,
    }
}
/// World caliber to gameplay caliber.
pub fn game_caliber(caliber: crate::Caliber) -> gp::Caliber {
    match caliber {
        crate::Caliber::Mm17 => gp::Caliber::Mm17,
        crate::Caliber::Mm42 => gp::Caliber::Mm42,
    }
}
fn game_error(error: gp::Error) -> &'static str {
    match error {
        gp::Error::Phase => "the command is not valid in this match phase",
        gp::Error::Robot => "unknown robot id",
        gp::Error::Invalid => "invalid value",
        gp::Error::Ineligible => "the robot is not eligible for that",
        gp::Error::Insufficient => "not enough gold or allowance",
        gp::Error::Limit => "exchange limit reached",
        gp::Error::ClockOverflow => "gameplay clock overflow",
    }
}

/// The match authority: teams, the countdown and round clock, rune
/// opportunities and buffs, and the gameplay engine that owns robot, base and
/// outpost HP. It advances only through [`Referee::tick`] and
/// [`Referee::command`], which take explicit timestamps; it never reads host
/// time.
///
/// ```rust
/// use rm_simulator_world::referee::Referee;
/// use rm_simulator_world::rune::SmallRune;
/// use rm_simulator_world::{RefereeCommand, RefereeConfig, Rune, RuneKind};
///
/// let mut referee =
///     Referee::new(RefereeConfig::alternating(1, 2), vec![RuneKind::Small], 2).unwrap();
/// let mut runes = vec![Rune::Small(SmallRune::default())];
/// referee.command(RefereeCommand::StartMatch, 0, &mut runes).unwrap();
/// // Five seconds of countdown, then a fresh seven-minute round clock.
/// referee.tick(5_000_000_000, &mut runes).unwrap();
/// let snapshot = referee.snapshot();
/// assert_eq!(snapshot.match_time_ns, 0);
/// assert_eq!(snapshot.remaining_ns, 420_000_000_000);
/// // Skipping to 1:30 grants the opportunity that would have arrived there.
/// referee
///     .command(
///         RefereeCommand::SkipTo {
///             match_time_ns: 90_000_000_000,
///         },
///         5_000_000_000,
///         &mut runes,
///     )
///     .unwrap();
/// referee.tick(5_000_000_001, &mut runes).unwrap();
/// assert_eq!(referee.snapshot().teams[0].rune_opportunities, 2);
/// // The gameplay clock skipped with it.
/// assert_eq!(referee.game().snapshot().round_elapsed_ticks, 90_000);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Referee {
    base_open: [bool; 2],
    /// Field time each base set off toward `base_open`; `None` at rest.
    base_moved_ns: [Option<u64>; 2],
    dart_door_open: [bool; 2],
    /// Field time the dart targets started sweeping; `None` at rest.
    dart_target_since_ns: Option<u64>,
    game: gp::Game,
    /// Id of the first game event not yet translated into a referee event.
    game_events_seen: u64,
    config: RefereeConfig,
    /// Kind each rune had before the match, restored on reset.
    training_kinds: Vec<RuneKind>,
    phase: MatchPhase,
    phase_started_ns: u64,
    match_started_ns: u64,
    /// Round time skipped by `SkipTo`.
    skipped_ns: u64,
    finished_match_time_ns: u64,
    schedule_index: usize,
    stage: RuneStage,
    teams: [TeamState; 2],
    events: VecDeque<TimedEvent>,
    now_ns: u64,
}

impl Referee {
    /// `runes` and `outposts` are the field's counts; the config must own
    /// each of them, and no team may own more than one outpost.
    pub fn new(
        config: RefereeConfig,
        rune_kinds: Vec<RuneKind>,
        outposts: usize,
    ) -> Result<Self, &'static str> {
        if config.rune_teams.len() != rune_kinds.len() {
            return Err("referee rune_teams must name an owner for every rune");
        }
        if config.outpost_teams.len() != outposts {
            return Err("referee outpost_teams must name an owner for every outpost");
        }
        if Team::BOTH.iter().any(|team| {
            config
                .outpost_teams
                .iter()
                .filter(|owner| *owner == team)
                .count()
                > 1
        }) {
            return Err("a team can own at most one outpost");
        }
        if config.round_ns == 0 || config.round_ns > ROUND_NS {
            return Err("referee round length must be positive and at most seven minutes");
        }
        if config.robot.fallback.max_hp == 0 {
            return Err("referee robots need some HP");
        }
        let mut game = gp::Game::new(gp::Config {
            format: gp::MatchFormat::Bo2,
            robots: Vec::new(),
            event_memory: GAME_EVENT_MEMORY,
        })
        .map_err(game_error)?;
        game.command(gp::Command::SetPolicy(gp::Policy {
            enforce_allowance: false,
            exchange_requires_zone: false,
        }))
        .map_err(game_error)?;
        Ok(Self {
            base_open: [false; 2],
            base_moved_ns: [None; 2],
            dart_door_open: [true; 2],
            dart_target_since_ns: None,
            game,
            game_events_seen: 0,
            training_kinds: rune_kinds,
            phase: MatchPhase::Idle,
            phase_started_ns: 0,
            match_started_ns: 0,
            skipped_ns: 0,
            finished_match_time_ns: 0,
            schedule_index: 0,
            stage: RuneStage::Small,
            teams: [TeamState::fresh(), TeamState::fresh()],
            events: VecDeque::new(),
            now_ns: 0,
            config,
        })
    }
    /// The gameplay engine: HP, gold, experience, heat, respawn and results.
    pub fn game(&self) -> &gp::Game {
        &self.game
    }
    fn robot_state(&self, robot: u32) -> Option<&gp::RobotState> {
        self.game
            .snapshot()
            .robots
            .iter()
            .find(|r| r.config.id == robot)
    }
    /// Refuse a launch the rules block: a defeated, weakened, overheated or
    /// locked robot, a caliber the robot has no launcher for, a launch outside
    /// Idle practice or a running round, and a zero allowance when the policy
    /// enforces it. Shots without a known shooter are always allowed.
    pub fn check_launch(
        &self,
        shooter: Option<u32>,
        caliber: crate::Caliber,
    ) -> Result<(), &'static str> {
        let Some(robot) = shooter.and_then(|id| self.robot_state(id)) else {
            return Ok(());
        };
        let caliber = game_caliber(caliber);
        if self.game.can_launch(robot.config.id, caliber) {
            return Ok(());
        }
        let snapshot = self.game.snapshot();
        let now = snapshot.round_elapsed_ticks;
        Err(if !robot.alive() {
            "the robot is defeated"
        } else if !robot.config.kind.shoots(caliber) {
            "the robot has no launcher for that caliber"
        } else if !matches!(snapshot.phase, gp::Phase::Idle | gp::Phase::Running) {
            "the round is not running"
        } else if robot.weakened {
            "the robot is weakened"
        } else if robot.overheated || robot.heat_locked_for_round {
            "the barrel is overheated"
        } else if robot.speed_locked_for_round || robot.speed_locked_until_ticks > now {
            "launching is locked"
        } else if robot.irregularly_disconnected {
            "the robot is disconnected"
        } else if robot.config.kind == gp::RobotKind::Drone && !robot.air_support_active {
            "the drone has no air support"
        } else {
            "no projectile allowance"
        })
    }
    /// Account a launch the field made: allowance, heat and launch experience.
    pub fn record_launch(&mut self, shooter: Option<u32>, caliber: crate::Caliber) {
        if let Some(robot) = shooter
            && self.robot_state(robot).is_some()
        {
            let _ = self.game.command(gp::Command::Launch {
                robot,
                caliber: game_caliber(caliber),
            });
            self.drain_game_events();
        }
    }
    /// The rules this referee was built with.
    pub fn config(&self) -> &RefereeConfig {
        &self.config
    }
    /// Where the match is in the countdown and round sequence.
    pub fn phase(&self) -> MatchPhase {
        self.phase
    }
    /// Seed for rune `index`'s own stream.
    pub fn rune_seed(&self, index: usize) -> u64 {
        self.config
            .seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(index as u64 + 1)
    }
    fn rune_of(&self, team: Team) -> Option<usize> {
        self.config
            .rune_teams
            .iter()
            .position(|owner| *owner == team)
    }
    fn team_of_rune(&self, rune: u32) -> Option<Team> {
        self.config.rune_teams.get(rune as usize).copied()
    }
    /// Outpost index `team` owns, when it has one.
    pub fn outpost_of(&self, team: Team) -> Option<usize> {
        self.config
            .outpost_teams
            .iter()
            .position(|owner| *owner == team)
    }
    /// A base's HP and shield in the gameplay engine.
    pub fn base_hp(&self, team: Team) -> (u32, u32) {
        let state = &self.game.snapshot().teams[team.index()];
        (state.base_hp, state.base_shield_hp)
    }
    /// An outpost's HP in the gameplay engine; `None` for an index out of range.
    pub fn outpost_hp(&self, outpost: usize) -> Option<u32> {
        let team = *self.config.outpost_teams.get(outpost)?;
        Some(self.game.snapshot().teams[team.index()].outpost_hp)
    }
    /// Innermost ring that still detects on rune `index` (section 5.5.2:
    /// rings 4..10 after one Big Rune activation, 7..10 after two).
    pub fn rune_min_ring(&self, index: u32) -> u32 {
        let Some(team) = self.team_of_rune(index) else {
            return 1;
        };
        if self.stage != RuneStage::Big {
            return 1;
        }
        match self.teams[team.index()].big_rune_activations {
            0 => 1,
            1 => 4,
            _ => 7,
        }
    }
    /// Round clock time at the referee's own current time, in nanoseconds:
    /// running time plus skips, the frozen round time once finished, and zero
    /// before a round starts. `RefereeSnapshot::match_time_ns` reports it.
    pub fn match_time_ns(&self) -> u64 {
        self.match_time(self.now_ns)
    }
    /// Visit every absolute timestamp on `clock`, in nanoseconds and in a
    /// fixed order, so a wire codec can rewrite them as tick-relative codes
    /// and back.
    ///
    /// On [`StampClock::Field`]: the referee's current time, the phase start,
    /// the round start, then each event's time. On [`StampClock::Round`]: per
    /// team the activation start and the buff's start and end, then each
    /// event's round time. The skipped round time, a finished round's frozen
    /// time and the gameplay engine's millisecond round ticks are not
    /// nanosecond stamps and are not visited, so [`Referee::match_time_ns`]
    /// only needs the field clock stamps to be mapped back. A referee whose
    /// stamps were rewritten is meaningless until they are mapped back.
    pub fn for_each_stamp_mut(&mut self, clock: StampClock, visit: &mut dyn FnMut(&mut u64)) {
        match clock {
            StampClock::Field => {
                visit(&mut self.now_ns);
                visit(&mut self.phase_started_ns);
                visit(&mut self.match_started_ns);
                for moved in self.base_moved_ns.iter_mut().flatten() {
                    visit(moved);
                }
                if let Some(since) = &mut self.dart_target_since_ns {
                    visit(since);
                }
                for event in &mut self.events {
                    visit(&mut event.time_ns);
                }
            }
            StampClock::Round => {
                for team in &mut self.teams {
                    if let Some(since) = &mut team.activating_since_ns {
                        visit(since);
                    }
                    if let Some(buff) = &mut team.buff {
                        visit(&mut buff.started_ns);
                        visit(&mut buff.expires_ns);
                    }
                }
                for event in &mut self.events {
                    if let Some(match_time) = &mut event.match_time_ns {
                        visit(match_time);
                    }
                }
            }
        }
    }
    /// When the dart targets started sweeping, on the field clock; `None`
    /// while they rest.
    pub fn dart_target_since_ns(&self) -> Option<u64> {
        self.dart_target_since_ns
    }
    /// Send team's base armor toward `open` from where it is now, so a
    /// reversal mid-travel continues smoothly.
    fn set_base_open(&mut self, team: Team, open: bool) {
        let i = team.index();
        if self.base_open[i] == open {
            return;
        }
        let travel = rm_simulator_physics::motion::base_travel(
            self.base_open[i],
            self.base_moved_ns[i],
            self.now_ns,
        );
        let done = if open { travel } else { 1.0 - travel };
        self.base_open[i] = open;
        self.base_moved_ns[i] = Some(
            self.now_ns
                .saturating_sub((done * BASE_TRAVEL_NS as f64).round() as u64),
        );
    }
    fn match_time(&self, now_ns: u64) -> u64 {
        match self.phase {
            MatchPhase::Running => now_ns
                .saturating_sub(self.match_started_ns)
                .saturating_add(self.skipped_ns),
            MatchPhase::Finished => self.finished_match_time_ns,
            _ => 0,
        }
    }
    fn push_event(&mut self, event: RefereeEvent) {
        let match_time_ns = matches!(self.phase, MatchPhase::Running | MatchPhase::Finished)
            .then(|| self.match_time(self.now_ns));
        self.events.push_back(TimedEvent {
            time_ns: self.now_ns,
            match_time_ns,
            event,
        });
        while self.events.len() > EVENT_MEMORY {
            self.events.pop_front();
        }
    }
    /// Translate the game events recorded since the last drain.
    fn drain_game_events(&mut self) {
        let seen = self.game_events_seen;
        let snapshot = self.game.snapshot();
        let fresh: Vec<gp::Event> = snapshot
            .recent_events
            .iter()
            .filter(|event| event.id >= seen)
            .cloned()
            .collect();
        self.game_events_seen = snapshot.next_event_id;
        for event in fresh {
            match event.kind {
                gp::EventKind::Damage {
                    target,
                    shooter,
                    hp,
                    shield,
                    ..
                } if hp + shield > 0 => match target {
                    gp::Target::Robot(robot) => {
                        let now_hp = self.robot_state(robot).map_or(0, |r| r.hp);
                        self.push_event(RefereeEvent::RobotDamaged {
                            robot,
                            amount: hp,
                            hp: now_hp,
                            shooter,
                        });
                    }
                    gp::Target::Base(team) => {
                        let team = world_team(team);
                        let (base_hp, shield_hp) = self.base_hp(team);
                        self.push_event(RefereeEvent::BaseDamaged {
                            team,
                            amount: hp + shield,
                            hp: base_hp,
                            shield_hp,
                            shooter,
                        });
                        if base_hp == 0 {
                            self.push_event(RefereeEvent::BaseDestroyed { team });
                        }
                    }
                    gp::Target::Outpost(team) => {
                        let team = world_team(team);
                        if self.game.snapshot().teams[team.index()].outpost_hp == 0 {
                            self.outpost_destroyed(team);
                        }
                    }
                },
                gp::EventKind::RobotDefeated(robot) => {
                    self.push_event(RefereeEvent::RobotDefeated { robot })
                }
                gp::EventKind::RobotRespawned(robot) => {
                    self.push_event(RefereeEvent::RobotRespawned { robot })
                }
                gp::EventKind::RobotRevived(robot) => {
                    self.push_event(RefereeEvent::RobotRevived { robot })
                }
                gp::EventKind::WeaknessCleared(robot) => {
                    self.push_event(RefereeEvent::WeaknessCleared { robot })
                }
                gp::EventKind::OutpostRebuilt(team) => {
                    self.push_event(RefereeEvent::OutpostRebuilt {
                        team: world_team(team),
                    })
                }
                gp::EventKind::LevelChanged { robot, level } => {
                    self.push_event(RefereeEvent::LevelUp { robot, level })
                }
                gp::EventKind::TerrainCrossing { robot, kind } => {
                    self.push_event(RefereeEvent::TerrainCrossing { robot, kind })
                }
                gp::EventKind::BaseArmorExpanded(team) => {
                    let team = world_team(team);
                    self.set_base_open(team, true);
                    self.push_event(RefereeEvent::BaseArmorExpanded { team });
                }
                _ => {}
            }
        }
    }
    /// Announce an outpost's destruction.
    fn outpost_destroyed(&mut self, team: Team) {
        if let Some(outpost) = self.outpost_of(team) {
            self.push_event(RefereeEvent::OutpostDestroyed {
                outpost: outpost as u32,
                team,
            });
        }
    }
    /// Bring a running game's round clock up to `match_time_ns`.
    fn advance_game(&mut self, match_time_ns: u64) {
        let snapshot = self.game.snapshot();
        if snapshot.phase != gp::Phase::Running {
            return;
        }
        let target = match_time_ns.min(self.config.round_ns) / gp::TICK_NS;
        let behind = target.saturating_sub(snapshot.round_elapsed_ticks);
        if behind > 0 {
            // Stepping a running game only fails on clock overflow, which the
            // round length bounds.
            let _ = self.game.step(behind);
            self.drain_game_events();
        }
    }

    /// Advance to `now_ns` (never backwards), driving the runes it owns and
    /// the gameplay engine's round clock.
    pub fn tick(&mut self, now_ns: u64, runes: &mut [Rune]) -> Result<(), RuneError> {
        if now_ns < self.now_ns {
            return Err(RuneError::TimeReversal);
        }
        self.now_ns = now_ns;
        // A base that finished its travel rests, so its stamp stops travelling.
        for moved in &mut self.base_moved_ns {
            if moved.is_some_and(|t| now_ns.saturating_sub(t) >= BASE_TRAVEL_NS) {
                *moved = None;
            }
        }
        if self.phase == MatchPhase::Countdown
            && now_ns - self.phase_started_ns >= self.config.countdown_ns
        {
            let remaining =
                gp::COUNTDOWN_TICKS.saturating_sub(self.game.snapshot().phase_elapsed_ticks);
            let _ = self.game.step(remaining);
            self.drain_game_events();
            self.phase = MatchPhase::Running;
            self.phase_started_ns = now_ns;
            self.match_started_ns = now_ns;
            self.skipped_ns = 0;
            self.push_event(RefereeEvent::MatchStarted);
        }
        if self.phase != MatchPhase::Running {
            return Ok(());
        }
        let match_time_ns = self.match_time(now_ns);
        self.advance_game(match_time_ns);
        while let Some((at_ns, item)) = SCHEDULE.get(self.schedule_index).copied()
            && at_ns <= match_time_ns
        {
            self.schedule_index += 1;
            match item {
                Scheduled::SmallOpportunity => self.grant_all(RuneStage::Small),
                Scheduled::BigOpportunity => self.grant_all(RuneStage::Big),
                Scheduled::BigStage => {
                    self.stage = RuneStage::Big;
                    for (index, rune) in runes.iter_mut().enumerate() {
                        if index < self.config.rune_teams.len() {
                            rune.convert(RuneKind::Big, now_ns)?;
                        }
                    }
                    for team in &mut self.teams {
                        team.activating_since_ns = None;
                        team.attempt_rings.clear();
                    }
                    self.push_event(RefereeEvent::RuneStage(RuneStage::Big));
                }
            }
        }
        for team in Team::BOTH {
            self.tick_team(team, now_ns, match_time_ns, runes)?;
        }
        if match_time_ns >= self.config.round_ns || self.game.snapshot().phase != gp::Phase::Running
        {
            self.finish(now_ns, runes)?;
        }
        Ok(())
    }

    fn grant_all(&mut self, stage: RuneStage) {
        for team in Team::BOTH {
            self.teams[team.index()].rune_opportunities = self.teams[team.index()]
                .rune_opportunities
                .saturating_add(1);
            let total = self.teams[team.index()].rune_opportunities;
            self.push_event(RefereeEvent::RuneOpportunity { team, stage, total });
        }
    }

    fn tick_team(
        &mut self,
        team: Team,
        now_ns: u64,
        match_time_ns: u64,
        runes: &mut [Rune],
    ) -> Result<(), RuneError> {
        let Some(index) = self.rune_of(team) else {
            return Ok(());
        };
        let rune = &mut runes[index];
        rune.advance_to(now_ns)?;
        if let Some(since) = self.teams[team.index()].activating_since_ns {
            match rune.state() {
                RuneState::Activated => {
                    let rings = std::mem::take(&mut self.teams[team.index()].attempt_rings);
                    let arms = rings.len() as u32;
                    let average_ring = if rings.is_empty() {
                        0.0
                    } else {
                        rings.iter().sum::<u32>() as f64 / rings.len() as f64
                    };
                    let buff = match self.stage {
                        RuneStage::Small => BuffState {
                            source: RuneStage::Small,
                            defense_pct: SMALL_RUNE_DEFENSE_PCT,
                            attack_pct: 100,
                            cooling_multiplier: 1,
                            started_ns: match_time_ns,
                            expires_ns: match_time_ns + SMALL_RUNE_BUFF_NS,
                        },
                        RuneStage::Big => big_rune_buff(average_ring, arms, match_time_ns),
                    };
                    let slot = &mut self.teams[team.index()];
                    slot.activating_since_ns = None;
                    slot.buff = Some(buff);
                    if self.stage == RuneStage::Big {
                        slot.big_rune_activations += 1;
                    }
                    let stage = self.stage;
                    let _ = self.game.command(gp::Command::RuneActivated {
                        team: game_team(team),
                        stage: match stage {
                            RuneStage::Small => gp::RuneStage::Small,
                            RuneStage::Big => gp::RuneStage::Large,
                        },
                        attack_pct: buff.attack_pct,
                        defense_pct: buff.defense_pct,
                        cooling_multiplier: buff.cooling_multiplier,
                        duration_ticks: (buff.expires_ns - buff.started_ns)
                            .div_ceil(gp::TICK_NS)
                            .max(1),
                    });
                    self.push_event(RefereeEvent::RuneActivated {
                        team,
                        stage,
                        arms,
                        average_ring,
                        buff,
                    });
                    self.drain_game_events();
                }
                RuneState::Activating if match_time_ns - since >= RUNE_ACTIVATING_WINDOW_NS => {
                    rune.deactivate(now_ns)?;
                    let slot = &mut self.teams[team.index()];
                    slot.activating_since_ns = None;
                    slot.attempt_rings.clear();
                    self.push_event(RefereeEvent::RuneActivationExpired { team });
                }
                RuneState::Activating => {
                    // A timeout inside the rune wiped its progress: forget the rings.
                    let completed = rune.snapshot().completed_groups;
                    let slot = &mut self.teams[team.index()];
                    if completed < slot.last_completed_groups {
                        slot.attempt_rings.clear();
                    }
                    slot.last_completed_groups = completed;
                }
                RuneState::Inactive => {
                    self.teams[team.index()].activating_since_ns = None;
                }
            }
        }
        if let Some(buff) = self.teams[team.index()].buff
            && match_time_ns >= buff.expires_ns
        {
            self.teams[team.index()].buff = None;
            runes[index].deactivate(now_ns)?;
            self.clear_game_rune_buff(team);
            self.push_event(RefereeEvent::BuffExpired { team });
        }
        Ok(())
    }
    fn clear_game_rune_buff(&mut self, team: Team) {
        let _ = self.game.command(gp::Command::ClearBuff {
            target: gp::BuffTarget::Team(game_team(team)),
            source: gp::coverage::Mechanic::Rune,
        });
    }

    /// Record a scored rune contact (called by the field after scoring).
    pub fn observe_hit(&mut self, hit: &ArmorHit) {
        let ArmorTarget::Rune { rune, .. } = hit.target else {
            return;
        };
        if !hit.detected || self.phase != MatchPhase::Running {
            return;
        }
        let Some(team) = self.team_of_rune(rune) else {
            return;
        };
        let slot = &mut self.teams[team.index()];
        if slot.activating_since_ns.is_none() {
            return;
        }
        match hit.rune_outcome {
            Some(
                HitOutcome::GroupHit { .. }
                | HitOutcome::Accepted { .. }
                | HitOutcome::Activated { .. },
            ) => {
                slot.attempt_rings.push(ring_of(hit.local_offset_m));
            }
            Some(HitOutcome::WrongBlade { .. }) => slot.attempt_rings.clear(),
            _ => {}
        }
    }

    /// Apply a detected projectile strike through the gameplay engine and
    /// report what it removed. Idle is free practice; outside Idle and a
    /// running round a strike removes nothing.
    pub fn projectile_hit(&mut self, hit: gp::ProjectileHit) -> gp::Applied {
        let applied = self.game.projectile_hit(hit).unwrap_or_default();
        self.drain_game_events();
        applied
    }
    /// Apply damage of `kind` that already includes attacker effects, as
    /// [`gp::Command::Damage`] does, crediting the target's opponent.
    pub fn damage(&mut self, target: gp::Target, amount: u32, kind: gp::DamageKind) -> gp::Applied {
        let applied = self
            .game
            .damage(target, amount, kind, None)
            .unwrap_or_default();
        self.drain_game_events();
        applied
    }
    /// Report whether a robot stands on a buff point (section 5.5.3). Only a
    /// running round records zone contacts, and only a change is forwarded.
    pub fn observe_zone(&mut self, robot: u32, zone: gp::Zone, detected: bool) {
        if self.phase != MatchPhase::Running {
            return;
        }
        let Some(state) = self.robot_state(robot) else {
            return;
        };
        let recorded = state
            .zones
            .iter()
            .any(|contact| contact.zone == zone && contact.detected);
        if recorded != detected {
            let _ = self.game.command(gp::Command::ZoneDetection {
                robot,
                zone,
                detected,
            });
            self.drain_game_events();
        }
    }

    /// Apply an operator command at `now_ns`, driving the runes it owns. A
    /// timestamp before the referee's current one, and any command the current
    /// phase does not allow, is refused with a message and no other change.
    pub fn command(
        &mut self,
        command: RefereeCommand,
        now_ns: u64,
        runes: &mut [Rune],
    ) -> Result<(), &'static str> {
        if now_ns < self.now_ns {
            return Err("referee time cannot move backwards");
        }
        self.now_ns = now_ns;
        let rune_error = |_: RuneError| "rune rejected the referee's request";
        let result = match command {
            RefereeCommand::SetBaseHp {
                team,
                hp,
                shield_hp,
            } => self.game.command(gp::Command::SetBase {
                team: game_team(team),
                hp,
                shield_hp,
            }),
            RefereeCommand::SetBaseOpen { team, open } => {
                self.set_base_open(team, open);
                Ok(())
            }
            RefereeCommand::SetDartDoorOpen { team, open } => {
                self.dart_door_open[team.index()] = open;
                Ok(())
            }
            RefereeCommand::SetDartTargetMoving { moving } => {
                if !moving {
                    self.dart_target_since_ns = None;
                } else if self.dart_target_since_ns.is_none() {
                    self.dart_target_since_ns = Some(self.now_ns);
                }
                Ok(())
            }
            RefereeCommand::SetOutpostHp { outpost, hp } => {
                let team = *self
                    .config
                    .outpost_teams
                    .get(outpost as usize)
                    .ok_or("unknown outpost")?;
                let before = self.game.snapshot().teams[team.index()].outpost_hp;
                self.game
                    .command(gp::Command::SetOutpostHp {
                        team: game_team(team),
                        hp,
                    })
                    .map_err(|_| "outpost HP must be at most 1500")?;
                if before > 0 && hp == 0 {
                    self.outpost_destroyed(team);
                }
                Ok(())
            }
            RefereeCommand::SetGold { team, gold } => self.game.command(gp::Command::SetGold {
                team: game_team(team),
                gold,
            }),
            RefereeCommand::SetAllowance { robot, allowance } => self
                .game
                .command(gp::Command::SetAllowance { robot, allowance }),
            RefereeCommand::SetPolicy(policy) => self.game.command(gp::Command::SetPolicy(policy)),
            RefereeCommand::SetPerformance { robot, performance } => self
                .game
                .command(gp::Command::SetPerformance { robot, performance }),
            RefereeCommand::BuyAmmo {
                robot,
                caliber,
                amount,
            } => self.game.command(gp::Command::ExchangeAmmo {
                robot,
                caliber: game_caliber(caliber),
                amount,
                remote: false,
            }),
            RefereeCommand::SetRuneOpportunities {
                team,
                opportunities,
            } => {
                self.teams[team.index()].rune_opportunities = opportunities;
                Ok(())
            }
            RefereeCommand::SetRuneBuff {
                team,
                defense_pct,
                attack_pct,
                cooling_multiplier,
                duration_ns,
            } => {
                if self.phase != MatchPhase::Running {
                    return Err("the round is not running");
                }
                if defense_pct > 100
                    || attack_pct > 1000
                    || cooling_multiplier > 100
                    || duration_ns > self.config.round_ns
                {
                    return Err("invalid rune buff parameters");
                }
                let index = self.rune_of(team).ok_or("the team has no rune")?;
                runes[index].deactivate(now_ns).map_err(rune_error)?;
                let started_ns = self.match_time(now_ns);
                let slot = &mut self.teams[team.index()];
                slot.activating_since_ns = None;
                slot.attempt_rings.clear();
                slot.buff = (duration_ns > 0).then_some(BuffState {
                    source: self.stage,
                    defense_pct,
                    attack_pct,
                    cooling_multiplier,
                    started_ns,
                    expires_ns: started_ns + duration_ns,
                });
                if duration_ns > 0 {
                    let now_ticks = self.game.snapshot().round_elapsed_ticks;
                    self.game.command(gp::Command::ApplyBuff(gp::Buff {
                        target: gp::BuffTarget::Team(game_team(team)),
                        source: gp::coverage::Mechanic::Rune,
                        attack_pct,
                        defense_pct,
                        vulnerability_pct: 0,
                        cooling_multiplier,
                        expires_ticks: now_ticks + duration_ns.div_ceil(gp::TICK_NS).max(1),
                    }))
                } else {
                    self.clear_game_rune_buff(team);
                    Ok(())
                }
            }
            RefereeCommand::StartMatch => {
                if self.phase != MatchPhase::Idle {
                    return Err("a match is already in progress; reset it first");
                }
                for team in &mut self.teams {
                    *team = TeamState::fresh();
                }
                // A new or reset match starts with the bases shut, not
                // closing.
                self.base_open = [false; 2];
                self.base_moved_ns = [None; 2];
                self.dart_door_open = [true; 2];
                self.stage = RuneStage::Small;
                self.schedule_index = 0;
                for (index, rune) in runes.iter_mut().enumerate() {
                    if index < self.config.rune_teams.len() {
                        rune.set_auto_restart(false);
                        rune.set_seed(self.rune_seed(index));
                        rune.convert(RuneKind::Small, now_ns).map_err(rune_error)?;
                        rune.deactivate(now_ns).map_err(rune_error)?;
                    }
                }
                self.reset_game();
                let _ = self.game.command(gp::Command::BeginCountdown);
                // A team the field gives no outpost has none to protect its base.
                for team in Team::BOTH {
                    if self.outpost_of(team).is_none() {
                        let _ = self.game.command(gp::Command::SetOutpostHp {
                            team: game_team(team),
                            hp: 0,
                        });
                    }
                }
                self.phase = MatchPhase::Countdown;
                self.phase_started_ns = now_ns;
                self.push_event(RefereeEvent::CountdownStarted);
                Ok(())
            }
            RefereeCommand::SkipTo { match_time_ns } => {
                if self.phase != MatchPhase::Running {
                    return Err("the round is not running");
                }
                let current = self.match_time(now_ns);
                if match_time_ns < current {
                    return Err("the round clock cannot move backwards");
                }
                self.advance_game(current);
                let target = match_time_ns.min(self.config.round_ns);
                self.skipped_ns += target - current;
                if self.game.snapshot().phase == gp::Phase::Running {
                    let _ = self.game.command(gp::Command::SkipTo {
                        round_ticks: target / gp::TICK_NS,
                    });
                }
                Ok(())
            }
            RefereeCommand::FinishMatch => {
                if !matches!(self.phase, MatchPhase::Countdown | MatchPhase::Running) {
                    return Err("no match to finish");
                }
                self.finish(now_ns, runes).map_err(rune_error)?;
                Ok(())
            }
            RefereeCommand::Adjudicate { winner } => {
                if self.phase != MatchPhase::Finished {
                    return Err("no finished round to decide");
                }
                self.game
                    .command(gp::Command::Adjudicate {
                        winner: winner.map(game_team),
                    })
                    .map_err(game_error)?;
                if let Some(result) = self.game.snapshot().result {
                    self.push_event(RefereeEvent::RoundResult(result));
                }
                Ok(())
            }
            RefereeCommand::ResetMatch => {
                if self.phase == MatchPhase::Idle {
                    return Err("no match to reset");
                }
                for team in &mut self.teams {
                    *team = TeamState::fresh();
                }
                // A new or reset match starts with the bases shut, not
                // closing.
                self.base_open = [false; 2];
                self.base_moved_ns = [None; 2];
                self.dart_door_open = [true; 2];
                self.stage = RuneStage::Small;
                self.schedule_index = 0;
                for (index, rune) in runes.iter_mut().enumerate() {
                    if let Some(kind) = self.training_kinds.get(index) {
                        rune.convert(*kind, now_ns).map_err(rune_error)?;
                        rune.set_auto_restart(true);
                        rune.clear_seed();
                        rune.deactivate(now_ns).map_err(rune_error)?;
                        rune.activate(now_ns).map_err(rune_error)?;
                    }
                }
                self.reset_game();
                self.phase = MatchPhase::Idle;
                self.phase_started_ns = now_ns;
                self.push_event(RefereeEvent::MatchReset);
                Ok(())
            }
            RefereeCommand::ActivateRune { team } => {
                if self.phase != MatchPhase::Running {
                    return Err("the round is not running");
                }
                let index = self.rune_of(team).ok_or("the team has no rune")?;
                if self.teams[team.index()].rune_opportunities == 0 {
                    return Err("the team has no rune opportunity");
                }
                if runes[index].state() != RuneState::Inactive {
                    return Err("the team's rune is not inactive");
                }
                if self.teams[team.index()].buff.is_some() {
                    return Err("the team's rune buff is still active");
                }
                runes[index].activate(now_ns).map_err(rune_error)?;
                let match_time_ns = self.match_time(now_ns);
                let slot = &mut self.teams[team.index()];
                slot.rune_opportunities -= 1;
                slot.activating_since_ns = Some(match_time_ns);
                slot.attempt_rings.clear();
                slot.last_completed_groups = 0;
                self.push_event(RefereeEvent::RuneActivating { team });
                Ok(())
            }
            RefereeCommand::GrantRuneOpportunity { team } => {
                self.teams[team.index()].rune_opportunities = self.teams[team.index()]
                    .rune_opportunities
                    .saturating_add(1);
                let total = self.teams[team.index()].rune_opportunities;
                let stage = self.stage;
                self.push_event(RefereeEvent::RuneOpportunity { team, stage, total });
                Ok(())
            }
            RefereeCommand::DamageRobot { robot, amount } => {
                let state = self.robot_state(robot).ok_or("unknown robot id")?;
                if !state.alive() {
                    return Err("the robot is already defeated");
                }
                self.game
                    .damage(
                        gp::Target::Robot(robot),
                        amount,
                        gp::DamageKind::Penalty,
                        None,
                    )
                    .map(|_| ())
            }
            RefereeCommand::ReviveRobot { robot } => {
                self.game.command(gp::Command::Revive { robot })
            }
            RefereeCommand::InstantRespawn { robot } => {
                self.game.command(gp::Command::InstantRespawn { robot })
            }
            RefereeCommand::ClearWeakened { robot } => {
                self.game.command(gp::Command::ClearWeakened { robot })
            }
            RefereeCommand::SetRobotHp { robot, hp } => {
                self.game.command(gp::Command::SetRobotHp { robot, hp })
            }
        };
        self.drain_game_events();
        result.map_err(game_error)
    }
    /// The game back to Idle practice with its roster at full health.
    fn reset_game(&mut self) {
        self.drain_game_events();
        let _ = self.game.command(gp::Command::ResetMatch);
        self.game_events_seen = self.game.snapshot().next_event_id;
    }

    /// Give a chassis its robot record of class `kind` at full HP, with the
    /// selected performance or the class default. An id already present is an
    /// error; the field never reuses one.
    pub fn add_robot(
        &mut self,
        robot: u32,
        team: Team,
        kind: RobotKind,
        performance: Option<gp::Performance>,
    ) -> Result<(), &'static str> {
        if self.robot_state(robot).is_some() {
            return Err("a robot with that id already exists");
        }
        let kind = kind.gameplay();
        let performance = performance
            .or_else(|| gp::Performance::default_for(kind))
            .unwrap_or(gp::Performance::Fixed(self.config.robot.fallback));
        self.game
            .command(gp::Command::AddRobot(gp::RobotConfig {
                id: robot,
                team: game_team(team),
                kind,
                performance,
            }))
            .map_err(|_| "the performance does not fit the robot class")?;
        self.drain_game_events();
        self.push_event(RefereeEvent::RobotJoined { robot, team });
        Ok(())
    }
    /// Drop a chassis' robot record when the field removes the chassis.
    pub fn remove_robot(&mut self, robot: u32) -> Result<(), &'static str> {
        self.game
            .command(gp::Command::RemoveRobot { robot })
            .map_err(game_error)?;
        self.drain_game_events();
        self.push_event(RefereeEvent::RobotLeft { robot });
        Ok(())
    }
    /// Every robot record, summarised, in join order.
    pub fn robots(&self) -> impl Iterator<Item = RobotSnapshot> + '_ {
        self.game.snapshot().robots.iter().map(|r| RobotSnapshot {
            id: r.config.id,
            team: world_team(r.config.team),
            kind: RobotKind::from_gameplay(r.config.kind),
            hp: if r.ejected { 0 } else { r.hp },
            max_hp: r.stats().max_hp,
            level: r.level,
            weakened: r.weakened,
        })
    }
    /// Whether the robot is off the field; `None` for an unknown id.
    pub fn robot_defeated(&self, robot: u32) -> Option<bool> {
        self.robot_state(robot).map(|r| !r.alive())
    }

    fn finish(&mut self, now_ns: u64, runes: &mut [Rune]) -> Result<(), RuneError> {
        self.finished_match_time_ns = self.match_time(now_ns).min(self.config.round_ns);
        for (index, rune) in runes.iter_mut().enumerate() {
            if index < self.config.rune_teams.len() {
                rune.deactivate(now_ns)?;
            }
        }
        for team in &mut self.teams {
            team.activating_since_ns = None;
            team.buff = None;
            team.attempt_rings.clear();
        }
        if self.game.snapshot().phase == gp::Phase::Countdown {
            let remaining =
                gp::COUNTDOWN_TICKS.saturating_sub(self.game.snapshot().phase_elapsed_ticks);
            let _ = self.game.step(remaining);
        }
        if self.game.snapshot().phase == gp::Phase::Running {
            let _ = self.game.command(gp::Command::EndRound);
        }
        self.phase = MatchPhase::Finished;
        self.phase_started_ns = now_ns;
        self.drain_game_events();
        self.push_event(RefereeEvent::MatchFinished);
        if let Some(result) = self.game.snapshot().result {
            self.push_event(RefereeEvent::RoundResult(result));
        }
        Ok(())
    }

    /// The referee's public state at its current timestamp, including the
    /// event list. Taking one never advances the clock.
    pub fn snapshot(&self) -> RefereeSnapshot {
        let match_time_ns = self.match_time(self.now_ns);
        RefereeSnapshot {
            base_open: self.base_open,
            base_moved_ns: self.base_moved_ns,
            dart_door_open: self.dart_door_open,
            dart_target_since_ns: self.dart_target_since_ns,
            game: self.game.snapshot().clone(),
            phase: self.phase,
            match_time_ns,
            remaining_ns: self.config.round_ns.saturating_sub(match_time_ns),
            countdown_remaining_ns: if self.phase == MatchPhase::Countdown {
                self.config
                    .countdown_ns
                    .saturating_sub(self.now_ns.saturating_sub(self.phase_started_ns))
            } else {
                0
            },
            stage: self.stage,
            teams: std::array::from_fn(|i| {
                let team = Team::BOTH[i];
                let state = &self.teams[i];
                TeamSnapshot {
                    team,
                    rune_opportunities: state.rune_opportunities,
                    big_rune_activations: state.big_rune_activations,
                    rune_activating_since_ns: state.activating_since_ns,
                    buff: state.buff,
                }
            }),
            robots: self.robots().collect(),
            rune_teams: self.config.rune_teams.clone(),
            outpost_teams: self.config.outpost_teams.clone(),
            events: self.events.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pose, rune::SmallRune};
    use rm_simulator_gameplay as gp;

    fn runes() -> Vec<Rune> {
        vec![
            Rune::Small(SmallRune::from_cad(Pose::at([6.0, 0.0, 1.6])).unwrap()),
            Rune::Small(SmallRune::from_cad(Pose::at([6.0, 0.0, 1.6])).unwrap()),
        ]
    }
    /// A referee with robot 0 on red and robot 1 on blue, as if two chassis
    /// had joined; the join events are cleared.
    fn referee() -> Referee {
        let mut referee = Referee::new(
            RefereeConfig::alternating(2, 2),
            vec![RuneKind::Small, RuneKind::Small],
            2,
        )
        .unwrap();
        referee
            .add_robot(0, Team::Red, RobotKind::Infantry, None)
            .unwrap();
        referee
            .add_robot(1, Team::Blue, RobotKind::Hero, None)
            .unwrap();
        referee.events.clear();
        // Each record carries the class its placement named, with its
        // section 5.4.2 default performance: both start at 200 HP.
        let robots: Vec<_> = referee.robots().collect();
        assert_eq!(robots[0].kind, RobotKind::Infantry);
        assert_eq!(robots[1].kind, RobotKind::Hero);
        assert_eq!((robots[0].max_hp, robots[1].max_hp), (200, 200));
        referee
    }
    /// Hit every lit blade of `rune` once per 100 ms from `t`, at `offset`
    /// from the target centre; returns the time of the last hit.
    fn activate_by_hits(
        referee: &mut Referee,
        runes: &mut [Rune],
        rune: usize,
        mut t: u64,
        offset_m: [f64; 2],
    ) -> u64 {
        for _ in 0..12 {
            referee.tick(t, runes).unwrap();
            let Some(blade) = runes[rune].snapshot().active_blade else {
                break;
            };
            let outcome = runes[rune].hit(t, blade).unwrap();
            referee.observe_hit(&ArmorHit {
                time_ns: t,
                projectile: 0,
                shooter: None,
                caliber: crate::Caliber::Mm17,
                target: ArmorTarget::Rune {
                    rune: rune as u32,
                    blade,
                },
                position_m: [0.0; 3],
                local_offset_m: offset_m,
                normal_speed_m_s: 20.0,
                detected: true,
                rejection: None,
                rune_outcome: Some(outcome),
                damage: 0,
            });
            t += 100_000_000;
            if matches!(outcome, HitOutcome::GroupHit { .. }) {
                // Let the group's one-second window close.
                t += 1_000_000_001;
            }
        }
        t
    }

    #[test]
    fn rings_and_defense_follow_the_tables() {
        assert_eq!(ring_of([0.0, 0.0]), 10);
        assert_eq!(ring_of([0.014, 0.0]), 10);
        assert_eq!(ring_of([0.0, 0.016]), 9);
        assert_eq!(ring_of([0.10, 0.10]), 1);
        assert_eq!(ring_of([0.149, 0.0]), 1);
        assert_eq!(defended(200, 25), 150);
        assert_eq!(defended(20, 25), 15);
        assert_eq!(defended(10, 25), 8);
        assert_eq!(defended(10, 0), 10);
        assert_eq!(defended(10, 100), 0);
        let low = big_rune_buff(2.0, 5, 0);
        assert_eq!(
            (low.attack_pct, low.defense_pct, low.cooling_multiplier),
            (150, 25, 1)
        );
        assert_eq!(low.expires_ns, 30_000_000_000);
        let top = big_rune_buff(9.5, 10, 1);
        assert_eq!(
            (top.attack_pct, top.defense_pct, top.cooling_multiplier),
            (300, 50, 5)
        );
        assert_eq!(top.expires_ns, 60_000_000_001);
        assert_eq!(big_rune_buff(7.5, 7, 0).expires_ns, 40_000_000_000);
        assert_eq!(big_rune_buff(8.5, 12, 0).cooling_multiplier, 3);
    }

    #[test]
    fn configuration_is_validated() {
        assert!(
            Referee::new(
                RefereeConfig::alternating(1, 2),
                vec![RuneKind::Small; 2],
                2
            )
            .is_err()
        );
        assert!(
            Referee::new(
                RefereeConfig::alternating(2, 1),
                vec![RuneKind::Small; 2],
                2
            )
            .is_err()
        );
        let mut lifeless = RefereeConfig::alternating(2, 2);
        lifeless.robot.fallback.max_hp = 0;
        assert!(Referee::new(lifeless, vec![RuneKind::Small; 2], 2).is_err());
        // One outpost per team: the gameplay engine keeps one per team.
        let shared = RefereeConfig::owned(vec![], vec![Team::Red, Team::Red]);
        assert!(Referee::new(shared, vec![], 2).is_err());
        let mut longer = RefereeConfig::alternating(2, 2);
        longer.round_ns = ROUND_NS + 1;
        assert!(Referee::new(longer, vec![RuneKind::Small; 2], 2).is_err());
        let mut twice = referee();
        assert!(
            twice
                .add_robot(0, Team::Red, RobotKind::Infantry, None)
                .is_err()
        );
        // Hero tables cannot describe an Infantry.
        assert!(
            twice
                .add_robot(
                    5,
                    Team::Red,
                    RobotKind::Infantry,
                    Some(gp::Performance::Hero(gp::HeroType::MeleeFocused))
                )
                .is_err()
        );
        assert!(twice.remove_robot(0).is_ok() && twice.remove_robot(0).is_err());
        let mut long = RefereeConfig::alternating(2, 2);
        long.round_ns = 0;
        assert!(Referee::new(long, vec![RuneKind::Small; 2], 2).is_err());
        assert!(referee().snapshot().robots.iter().all(|r| r.hp == 200));
    }

    #[test]
    fn match_flow_grants_opportunities_and_runs_the_small_rune_buff() {
        let mut referee = referee();
        let mut runes = runes();
        // Idle: the runes run their training policy.
        referee.tick(1_000_000_000, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Activating);
        assert_eq!(
            referee.command(
                RefereeCommand::ActivateRune { team: Team::Red },
                1_000_000_000,
                &mut runes
            ),
            Err("the round is not running")
        );
        referee
            .command(RefereeCommand::StartMatch, 2_000_000_000, &mut runes)
            .unwrap();
        assert_eq!(referee.phase(), MatchPhase::Countdown);
        assert_eq!(runes[0].state(), RuneState::Inactive);
        assert!(
            referee
                .command(RefereeCommand::StartMatch, 2_000_000_000, &mut runes)
                .is_err()
        );
        referee.tick(4_500_000_000, &mut runes).unwrap();
        assert_eq!(referee.snapshot().countdown_remaining_ns, 2_500_000_000);
        referee.tick(6_999_999_999, &mut runes).unwrap();
        assert_eq!(referee.phase(), MatchPhase::Countdown);
        referee.tick(7_000_000_000, &mut runes).unwrap();
        let snapshot = referee.snapshot();
        assert_eq!(snapshot.phase, MatchPhase::Running);
        assert_eq!(snapshot.match_time_ns, 0);
        assert_eq!(snapshot.countdown_remaining_ns, 0);
        assert_eq!(snapshot.teams[0].rune_opportunities, 1);
        assert_eq!(snapshot.teams[1].rune_opportunities, 1);
        assert_eq!(snapshot.stage, RuneStage::Small);
        // Blue spends its opportunity and lets it expire after 20 s.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Blue },
                8_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(runes[1].state(), RuneState::Activating);
        assert_eq!(runes[0].state(), RuneState::Inactive);
        assert!(
            referee
                .command(
                    RefereeCommand::ActivateRune { team: Team::Blue },
                    8_000_000_000,
                    &mut runes
                )
                .is_err()
        );
        referee.tick(27_999_999_999, &mut runes).unwrap();
        assert_eq!(runes[1].state(), RuneState::Activating);
        referee.tick(28_000_000_000, &mut runes).unwrap();
        assert_eq!(runes[1].state(), RuneState::Inactive);
        assert_eq!(referee.snapshot().teams[1].rune_opportunities, 0);
        assert!(matches!(
            referee.snapshot().events.last().unwrap().event,
            RefereeEvent::RuneActivationExpired { team: Team::Blue }
        ));
        // Red activates its rune by hitting: 25 % defense for 45 s, then unavailable.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                30_000_000_000,
                &mut runes,
            )
            .unwrap();
        let last = activate_by_hits(&mut referee, &mut runes, 0, 30_100_000_000, [0.0, 0.0]);
        referee.tick(last, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Activated);
        let red = referee.snapshot().teams[0].clone();
        let buff = red.buff.expect("buff granted");
        assert_eq!(buff.defense_pct, 25);
        assert_eq!(buff.source, RuneStage::Small);
        assert_eq!(buff.expires_ns - buff.started_ns, SMALL_RUNE_BUFF_NS);
        let defense = |referee: &Referee, team| {
            referee
                .game()
                .defense_pct(gp::Target::Outpost(game_team(team)))
                .0
        };
        assert_eq!(defense(&referee, Team::Red), 25);
        assert_eq!(defense(&referee, Team::Blue), 0);
        assert!(red.rune_activating_since_ns.is_none());
        // Buff times are on the round clock, which started at 7 s world time.
        referee
            .tick(7_000_000_000 + buff.expires_ns - 1, &mut runes)
            .unwrap();
        assert_eq!(runes[0].state(), RuneState::Activated);
        referee
            .tick(7_000_000_000 + buff.expires_ns, &mut runes)
            .unwrap();
        assert_eq!(runes[0].state(), RuneState::Inactive);
        assert!(referee.snapshot().teams[0].buff.is_none());
        assert_eq!(defense(&referee, Team::Red), 0);
        // The 1:30 opportunity arrives on the round clock.
        referee
            .tick(7_000_000_000 + 89_999_999_999, &mut runes)
            .unwrap();
        assert_eq!(referee.snapshot().teams[0].rune_opportunities, 0);
        referee
            .tick(7_000_000_000 + 90_000_000_000, &mut runes)
            .unwrap();
        assert_eq!(referee.snapshot().teams[0].rune_opportunities, 1);
        assert_eq!(referee.snapshot().teams[1].rune_opportunities, 1);
        // Robot damage passes through the (absent) buff and defeats at zero.
        referee
            .command(
                RefereeCommand::DamageRobot {
                    robot: 0,
                    amount: 150,
                },
                100_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().robots[0].hp, 50);
        referee
            .command(
                RefereeCommand::DamageRobot {
                    robot: 0,
                    amount: 150,
                },
                100_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().robots[0].hp, 0);
        assert!(
            referee
                .command(
                    RefereeCommand::DamageRobot {
                        robot: 0,
                        amount: 1
                    },
                    100_000_000_000,
                    &mut runes
                )
                .is_err()
        );
        referee
            .command(
                RefereeCommand::ReviveRobot { robot: 0 },
                100_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().robots[0].hp, 200);
        assert!(
            referee
                .command(
                    RefereeCommand::ReviveRobot { robot: 9 },
                    100_000_000_000,
                    &mut runes
                )
                .is_err()
        );
        // Reset returns the runes to training.
        referee
            .command(RefereeCommand::ResetMatch, 101_000_000_000, &mut runes)
            .unwrap();
        assert_eq!(referee.phase(), MatchPhase::Idle);
        assert_eq!(runes[0].state(), RuneState::Activating);
        assert_eq!(runes[0].kind(), RuneKind::Small);
        assert_eq!(referee.snapshot().match_time_ns, 0);
    }

    #[test]
    fn big_rune_stage_converts_runes_and_scores_from_rings() {
        let mut referee = referee();
        let mut runes = runes();
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        assert_eq!(referee.phase(), MatchPhase::Running);
        assert!(
            referee
                .command(
                    RefereeCommand::SkipTo { match_time_ns: 1 },
                    4_000_000_000,
                    &mut runes
                )
                .is_err()
        );
        referee
            .command(
                RefereeCommand::SkipTo {
                    match_time_ns: 179_999_999_999,
                },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        assert_eq!(referee.snapshot().stage, RuneStage::Small);
        assert_eq!(referee.snapshot().teams[0].rune_opportunities, 2);
        referee.tick(5_000_000_001, &mut runes).unwrap();
        let snapshot = referee.snapshot();
        assert_eq!(snapshot.stage, RuneStage::Big);
        assert_eq!(snapshot.match_time_ns, 180_000_000_000);
        assert_eq!(snapshot.teams[0].rune_opportunities, 3);
        assert_eq!(runes[0].kind(), RuneKind::Big);
        assert_eq!(runes[0].state(), RuneState::Inactive);
        assert_eq!(referee.rune_min_ring(0), 1);
        // Activate with centre hits: five required hits, ring 10 each.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                6_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(runes[0].state(), RuneState::Activating);
        let last = activate_by_hits(&mut referee, &mut runes, 0, 6_100_000_000, [0.0, 0.0]);
        referee.tick(last, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Activated);
        let red = referee.snapshot().teams[0].clone();
        let buff = red.buff.expect("big rune buff");
        assert_eq!(buff.source, RuneStage::Big);
        assert_eq!(
            (buff.attack_pct, buff.defense_pct, buff.cooling_multiplier),
            (300, 50, 5)
        );
        assert_eq!(buff.expires_ns - buff.started_ns, 30_000_000_000);
        assert_eq!(red.big_rune_activations, 1);
        assert_eq!(referee.rune_min_ring(0), 4);
        assert_eq!(referee.rune_min_ring(1), 1);
        assert!(matches!(
            referee
                .snapshot()
                .events
                .iter()
                .rev()
                .find(|e| matches!(e.event, RefereeEvent::RuneActivated { .. }))
                .unwrap()
                .event,
            RefereeEvent::RuneActivated { arms: 5, .. }
        ));
        // Outer-ring hits after the buff: average ring 1, weaker buff. The
        // round clock started at 5 s world time.
        let expiry_world_ns = 5_000_000_000 + buff.expires_ns;
        referee.tick(expiry_world_ns, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Inactive);
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                expiry_world_ns + 1,
                &mut runes,
            )
            .unwrap();
        let last = activate_by_hits(
            &mut referee,
            &mut runes,
            0,
            expiry_world_ns + 100_000_000,
            [0.14, 0.0],
        );
        referee.tick(last, &mut runes).unwrap();
        let weak = referee.snapshot().teams[0].buff.expect("second buff");
        assert_eq!(
            (weak.attack_pct, weak.defense_pct, weak.cooling_multiplier),
            (150, 25, 1)
        );
        assert_eq!(referee.rune_min_ring(0), 7);
        // The round ends at seven minutes and everything goes dark.
        referee
            .command(
                RefereeCommand::SkipTo {
                    match_time_ns: ROUND_NS - 1,
                },
                last,
                &mut runes,
            )
            .unwrap();
        referee.tick(last + 1, &mut runes).unwrap();
        let end = referee.snapshot();
        assert_eq!(end.phase, MatchPhase::Finished);
        assert_eq!(end.match_time_ns, ROUND_NS);
        assert_eq!(end.remaining_ns, 0);
        assert!(end.teams[0].buff.is_none());
        assert_eq!(runes[0].state(), RuneState::Inactive);
        referee.tick(last + 5_000_000_000, &mut runes).unwrap();
        assert_eq!(referee.snapshot().match_time_ns, ROUND_NS);
        assert!(
            referee
                .command(
                    RefereeCommand::FinishMatch,
                    last + 5_000_000_000,
                    &mut runes
                )
                .is_err()
        );
    }

    #[test]
    fn wrong_blade_and_timeouts_forget_the_rings_and_outposts_are_reported() {
        let mut referee = referee();
        let mut runes = runes();
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Blue },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        // One good hit, then a wrong blade.
        let blade = runes[1].snapshot().active_blade.unwrap();
        let outcome = runes[1].hit(5_100_000_000, blade).unwrap();
        let hit = |outcome, blade| ArmorHit {
            time_ns: 5_100_000_000,
            projectile: 0,
            shooter: None,
            caliber: crate::Caliber::Mm17,
            target: ArmorTarget::Rune { rune: 1, blade },
            position_m: [0.0; 3],
            local_offset_m: [0.0, 0.0],
            normal_speed_m_s: 20.0,
            detected: true,
            rejection: None,
            rune_outcome: Some(outcome),
            damage: 0,
        };
        referee.observe_hit(&hit(outcome, blade));
        assert_eq!(referee.teams[1].attempt_rings.len(), 1);
        let wrong = (runes[1].snapshot().active_blade.unwrap() + 1) % 5;
        let outcome = runes[1].hit(5_200_000_000, wrong).unwrap();
        assert!(matches!(outcome, HitOutcome::WrongBlade { .. }));
        referee.observe_hit(&hit(outcome, wrong));
        assert!(referee.teams[1].attempt_rings.is_empty());
        // A hit, then the 2.5 s window lapses inside the rune.
        let blade = runes[1].snapshot().active_blade.unwrap();
        let outcome = runes[1].hit(5_300_000_000, blade).unwrap();
        referee.observe_hit(&hit(outcome, blade));
        referee.tick(5_300_000_000, &mut runes).unwrap();
        assert_eq!(referee.teams[1].attempt_rings.len(), 1);
        referee.tick(7_800_000_002, &mut runes).unwrap();
        assert_eq!(runes[1].snapshot().completed_groups, 0);
        assert!(referee.teams[1].attempt_rings.is_empty());
        // Hits on the other team's rune, or while not activating, are ignored.
        referee.observe_hit(&ArmorHit {
            target: ArmorTarget::Rune { rune: 0, blade: 0 },
            ..hit(
                HitOutcome::Accepted {
                    blade: 0,
                    next_blade: 1,
                },
                0,
            )
        });
        assert!(referee.teams[0].attempt_rings.is_empty());
        for _ in 0..2 {
            referee
                .command(
                    RefereeCommand::SetOutpostHp { outpost: 1, hp: 0 },
                    7_800_000_002,
                    &mut runes,
                )
                .unwrap();
        }
        let snapshot = referee.snapshot();
        let destroyed: Vec<_> = snapshot
            .events
            .iter()
            .filter(|e| matches!(e.event, RefereeEvent::OutpostDestroyed { .. }))
            .collect();
        assert_eq!(destroyed.len(), 1);
        // Section 5.5.3.9: a lost outpost alone does not expand the base armor.
        assert_eq!(snapshot.base_open, [false, false]);
        assert!(matches!(
            destroyed[0].event,
            RefereeEvent::OutpostDestroyed {
                outpost: 1,
                team: Team::Blue
            }
        ));
        assert_eq!(destroyed[0].match_time_ns, Some(2_800_000_002));
        // Red's tower still stands and covers its base; blue's does not.
        assert_eq!(base_strike(&mut referee, Team::Red), 0);
        assert_eq!(base_strike(&mut referee, Team::Blue), 20);
        // Finish early and the clock freezes.
        referee
            .command(RefereeCommand::FinishMatch, 8_000_000_000, &mut runes)
            .unwrap();
        assert_eq!(referee.snapshot().match_time_ns, 3_000_000_000);
        assert_eq!(referee.tick(1, &mut runes), Err(RuneError::TimeReversal));
    }

    /// One unattributed 17 mm strike on a lower plate of `team`'s base;
    /// returns the HP plus shield it removed.
    fn base_strike(referee: &mut Referee, team: Team) -> u32 {
        let applied = referee.projectile_hit(gp::ProjectileHit {
            target: gp::Target::Base(game_team(team)),
            caliber: gp::Caliber::Mm17,
            shooter: None,
            upper_front: false,
            critical: false,
        });
        applied.hp + applied.shield
    }

    #[test]
    fn base_cover_follows_the_outposts() {
        let mut referee = referee();
        let mut runes = runes();
        // Idle is training: no tower protects a base there.
        assert_eq!(base_strike(&mut referee, Team::Red), 20);
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        // The countdown takes no damage at all.
        assert_eq!(base_strike(&mut referee, Team::Blue), 0);
        referee.tick(5_000_000_000, &mut runes).unwrap();
        // Both towers stand, so both bases are covered.
        assert_eq!(base_strike(&mut referee, Team::Red), 0);
        assert_eq!(base_strike(&mut referee, Team::Blue), 0);
        // Losing red's tower drops only red's cover.
        referee
            .command(
                RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(base_strike(&mut referee, Team::Red), 20);
        assert_eq!(base_strike(&mut referee, Team::Blue), 0);
        assert_eq!(referee.snapshot().base_open, [false, false]);
    }

    #[test]
    fn hostile_amounts_and_skips_cannot_overflow() {
        assert_eq!(defended(u32::MAX, 0), u32::MAX);
        assert_eq!(defended(u32::MAX, 25), 3_221_225_471);
        assert_eq!(defended(u32::MAX, 100), 0);
        let mut referee = referee();
        let mut runes = runes();
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        referee
            .command(
                RefereeCommand::DamageRobot {
                    robot: 0,
                    amount: u32::MAX,
                },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().robots[0].hp, 0);
        // A skip past the end of the round stops at the end of the round.
        referee
            .command(
                RefereeCommand::SkipTo {
                    match_time_ns: u64::MAX,
                },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().match_time_ns, ROUND_NS);
        referee.tick(5_000_000_001, &mut runes).unwrap();
        assert_eq!(referee.phase(), MatchPhase::Finished);
        referee.tick(u64::MAX, &mut runes).unwrap();
        assert_eq!(referee.snapshot().match_time_ns, ROUND_NS);
    }

    #[test]
    fn skipping_ages_buffs_and_activation_windows() {
        let mut referee = referee();
        let mut runes = runes();
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        let last = activate_by_hits(&mut referee, &mut runes, 0, 5_100_000_000, [0.0, 0.0]);
        referee.tick(last, &mut runes).unwrap();
        assert!(referee.snapshot().teams[0].buff.is_some());
        // Skipping to 1:30 grants the opportunity and lets the 45 s buff lapse.
        referee
            .command(
                RefereeCommand::SkipTo {
                    match_time_ns: 90_000_000_000,
                },
                last,
                &mut runes,
            )
            .unwrap();
        referee.tick(last + 1, &mut runes).unwrap();
        let red = referee.snapshot().teams[0].clone();
        assert!(red.buff.is_none());
        assert_eq!(red.rune_opportunities, 1);
        assert_eq!(runes[0].state(), RuneState::Inactive);
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                last + 1,
                &mut runes,
            )
            .unwrap();
        // Skipping 20 s while Activating spends the window.
        referee
            .command(
                RefereeCommand::SkipTo {
                    match_time_ns: 110_000_000_001,
                },
                last + 1,
                &mut runes,
            )
            .unwrap();
        referee.tick(last + 2, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Inactive);
        assert!(
            referee.snapshot().teams[0]
                .rune_activating_since_ns
                .is_none()
        );
        assert!(
            referee
                .snapshot()
                .events
                .iter()
                .any(|e| e.event == RefereeEvent::RuneActivationExpired { team: Team::Red })
        );
    }

    #[test]
    fn base_armor_travels_and_reverses_continuously() {
        let mut runes = runes();
        let mut referee = referee();
        let open = |open| RefereeCommand::SetBaseOpen {
            team: Team::Red,
            open,
        };
        let fraction = |r: &Referee, t| r.snapshot().base_open_fraction(0, t);
        assert_eq!(fraction(&referee, 0), 0.0);
        referee.command(open(true), 1_000, &mut runes).unwrap();
        let half = 1_000 + BASE_TRAVEL_NS / 2;
        assert_eq!(fraction(&referee, half), 0.5);
        // Reversing at the half-way point closes from where it is.
        referee.command(open(false), half, &mut runes).unwrap();
        assert!((fraction(&referee, half) - 0.5).abs() < 1e-9);
        let quarter = half + BASE_TRAVEL_NS / 4;
        assert!(fraction(&referee, quarter) < 0.5);
        assert!(fraction(&referee, quarter) > 0.0);
        // It settles shut and stops carrying a stamp.
        let shut = half + BASE_TRAVEL_NS / 2;
        referee.tick(shut, &mut runes).unwrap();
        assert_eq!(referee.snapshot().base_moved_ns, [None; 2]);
        assert_eq!(fraction(&referee, shut + 1), 0.0);
        // Repeating the resting state starts no travel.
        referee.command(open(false), shut + 2, &mut runes).unwrap();
        assert_eq!(referee.snapshot().base_moved_ns[0], None);
    }

    #[test]
    fn robot_commands_and_granted_opportunities_are_reported() {
        let mut referee = referee();
        let mut runes = runes();
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        referee
            .command(
                RefereeCommand::GrantRuneOpportunity { team: Team::Blue },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        let snapshot = referee.snapshot();
        assert_eq!(snapshot.teams[1].rune_opportunities, 2);
        assert_eq!(
            snapshot.events.last().unwrap().event,
            RefereeEvent::RuneOpportunity {
                team: Team::Blue,
                stage: RuneStage::Small,
                total: 2
            }
        );
        // SetRobotHp clamps to the maximum and reports defeat and revival.
        referee
            .command(
                RefereeCommand::SetRobotHp { robot: 1, hp: 0 },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(
            referee.snapshot().events.last().unwrap().event,
            RefereeEvent::RobotDefeated { robot: 1 }
        );
        referee
            .command(
                RefereeCommand::SetRobotHp {
                    robot: 1,
                    hp: 9_999,
                },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        let snapshot = referee.snapshot();
        assert_eq!(snapshot.robots[1].hp, snapshot.robots[1].max_hp);
        assert_eq!(
            snapshot.events.last().unwrap().event,
            RefereeEvent::RobotRevived { robot: 1 }
        );
        assert!(
            referee
                .command(
                    RefereeCommand::SetRobotHp { robot: 9, hp: 1 },
                    5_000_000_000,
                    &mut runes
                )
                .is_err()
        );
        // Penalty damage ignores the defense buff a team holds.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        let last = activate_by_hits(&mut referee, &mut runes, 0, 5_100_000_000, [0.0, 0.0]);
        referee.tick(last, &mut runes).unwrap();
        assert_eq!(referee.snapshot().teams[0].buff.unwrap().defense_pct, 25);
        assert_eq!(referee.snapshot().robots[0].team, Team::Red);
        referee
            .command(
                RefereeCommand::DamageRobot {
                    robot: 0,
                    amount: 100,
                },
                last,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().robots[0].hp, 100);
        assert_eq!(
            referee.snapshot().events.last().unwrap().event,
            RefereeEvent::RobotDamaged {
                robot: 0,
                amount: 100,
                hp: 100,
                shooter: None,
            }
        );
    }

    #[test]
    fn big_stage_ends_attempts_expires_windows_and_counts_bonus_arms() {
        let mut referee = referee();
        let mut runes = runes();
        referee
            .command(RefereeCommand::StartMatch, 0, &mut runes)
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        referee
            .command(
                RefereeCommand::SkipTo {
                    match_time_ns: 179_999_999_999,
                },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        referee.tick(5_000_000_000, &mut runes).unwrap();
        // An attempt still running at 3:00 is lost with the conversion.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                5_000_000_000,
                &mut runes,
            )
            .unwrap();
        assert_eq!(referee.snapshot().teams[0].rune_opportunities, 1);
        referee.tick(5_000_000_001, &mut runes).unwrap();
        let red = referee.snapshot().teams[0].clone();
        assert_eq!(referee.snapshot().stage, RuneStage::Big);
        assert!(red.rune_activating_since_ns.is_none());
        assert_eq!(red.rune_opportunities, 2);
        assert_eq!(runes[0].kind(), RuneKind::Big);
        assert_eq!(runes[0].state(), RuneState::Inactive);
        // A Big Rune attempt also expires after 20 s.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                6_000_000_000,
                &mut runes,
            )
            .unwrap();
        referee.tick(25_999_999_999, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Activating);
        referee.tick(26_000_000_000, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Inactive);
        assert_eq!(
            referee.snapshot().events.last().unwrap().event,
            RefereeEvent::RuneActivationExpired { team: Team::Red }
        );
        assert_eq!(referee.snapshot().teams[0].rune_opportunities, 1);
        // Hitting both targets of every group lights ten arms: the longest buff.
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                27_000_000_000,
                &mut runes,
            )
            .unwrap();
        let mut t = 27_100_000_000;
        for _ in 0..6 {
            referee.tick(t, &mut runes).unwrap();
            let active = runes[0].snapshot().active_blades;
            if !active.iter().any(|lit| *lit) {
                break;
            }
            for blade in (0..5u32).filter(|b| active[*b as usize]) {
                let outcome = runes[0].hit(t, blade).unwrap();
                assert!(matches!(outcome, HitOutcome::GroupHit { .. }));
                referee.observe_hit(&ArmorHit {
                    time_ns: t,
                    projectile: 0,
                    shooter: None,
                    caliber: crate::Caliber::Mm17,
                    target: ArmorTarget::Rune { rune: 0, blade },
                    position_m: [0.0; 3],
                    local_offset_m: [0.0, 0.0],
                    normal_speed_m_s: 20.0,
                    detected: true,
                    rejection: None,
                    rune_outcome: Some(outcome),
                    damage: 0,
                });
                t += 50_000_000;
            }
            t += 1_000_000_001;
        }
        referee.tick(t, &mut runes).unwrap();
        assert_eq!(runes[0].state(), RuneState::Activated);
        let buff = referee.snapshot().teams[0].buff.expect("big rune buff");
        assert_eq!(buff.expires_ns - buff.started_ns, 60_000_000_000);
        assert_eq!(
            (buff.attack_pct, buff.defense_pct, buff.cooling_multiplier),
            (300, 50, 5)
        );
        assert!(referee.snapshot().events.iter().any(|e| matches!(
            e.event,
            RefereeEvent::RuneActivated {
                team: Team::Red,
                arms: 10,
                ..
            }
        )));
    }

    #[test]
    fn runes_are_seeded_for_the_match_and_train_unseeded() {
        let mut referee = referee();
        let mut runes = runes();
        // Training: the lowest unhit blade, in order.
        referee.tick(1_000_000_000, &mut runes).unwrap();
        assert_eq!(runes[0].snapshot().active_blade, Some(0));
        runes[0].hit(1_000_000_000, 0).unwrap();
        assert_eq!(runes[0].snapshot().active_blade, Some(1));
        referee
            .command(RefereeCommand::StartMatch, 2_000_000_000, &mut runes)
            .unwrap();
        referee.tick(7_000_000_000, &mut runes).unwrap();
        referee
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                7_000_000_000,
                &mut runes,
            )
            .unwrap();
        let mut order = Vec::new();
        for i in 0..5u64 {
            let blade = runes[0].snapshot().active_blade.unwrap();
            order.push(blade);
            runes[0]
                .hit(7_000_000_000 + i * 100_000_000, blade)
                .unwrap();
        }
        assert_ne!(
            order,
            vec![0, 1, 2, 3, 4],
            "seeded order is not the training order"
        );
        // The same seed replays the same order.
        let mut again = super::tests::referee();
        let mut again_runes = super::tests::runes();
        again
            .command(RefereeCommand::StartMatch, 2_000_000_000, &mut again_runes)
            .unwrap();
        again.tick(7_000_000_000, &mut again_runes).unwrap();
        again
            .command(
                RefereeCommand::ActivateRune { team: Team::Red },
                7_000_000_000,
                &mut again_runes,
            )
            .unwrap();
        for (i, expected) in order.iter().enumerate() {
            assert_eq!(again_runes[0].snapshot().active_blade, Some(*expected));
            again_runes[0]
                .hit(7_000_000_000 + i as u64 * 100_000_000, *expected)
                .unwrap();
        }
        // Reset returns to the training order.
        referee
            .command(RefereeCommand::ResetMatch, 8_000_000_000, &mut runes)
            .unwrap();
        assert_eq!(runes[0].snapshot().active_blade, Some(0));
        runes[0].hit(8_000_000_000, 0).unwrap();
        assert_eq!(runes[0].snapshot().active_blade, Some(1));
    }
}
