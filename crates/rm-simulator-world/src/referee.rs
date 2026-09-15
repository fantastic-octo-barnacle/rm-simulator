// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Match referee on the field clock: teams, the round timer, Power Rune
//! availability and buffs, outpost ownership and robot HP.
//!
//! Sources in the RMUC 2026 rule manual (V2.1.0): section 6.5 (five-second
//! countdown), 6.6 (seven-minute round), 5.5.2 (rune opportunities at 0:00
//! and 1:30 for the Small Rune, 3:00, 4:15 and 5:30 for the Big Rune, the
//! 20 s activating window, the Small Rune's 25 % defense buff for 45 s,
//! Table 5-16 and Table 5-17 for the Big Rune's buff from the average ring
//! and the number of lit arms, and the detection rings that remain after
//! each Big Rune activation), and 5.5.1 (outposts).
//!
//! Outside a match (`Idle`) the runes run their training policy so the
//! field stays usable for practice. `StartMatch` switches them to referee
//! control and seeds their target streams from the config's seed: dark
//! until a team spends an opportunity, then Activating for at most 20 s,
//! then Activated for the buff's duration, then unavailable. `ResetMatch`
//! drops the seeds again. The referee never reads host time; it advances
//! only through `tick`.
use crate::{
    ArmorHit, ArmorTarget,
    rune::{HitOutcome, Rune, RuneError, RuneKind, RuneState},
};
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
/// Ten rings across the 150 mm effective radius, ring 10 innermost; the
/// width was read off Figure 5-18, the text only gives 1 mm radial accuracy.
pub const RING_WIDTH_M: f64 = 0.015;
/// Recent events kept in the snapshot.
const EVENT_MEMORY: usize = 48;

pub use rm_simulator_physics::Team;

/// The robot classes a chassis can be recorded as. Every chassis names its
/// class in its [`crate::ChassisPlacement`]; the live referee models no
/// difference between the classes beyond what the record says, and gives
/// each the one configured HP.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RobotKind {
    /// Hero: 42 mm gun.
    Hero,
    /// Engineer.
    Engineer,
    /// Infantry: 17 mm gun. The default class of a placement that names none.
    #[default]
    Infantry,
    /// Sentry.
    Sentry,
    /// Aerial drone.
    Drone,
}

/// The HP every robot that joins the field starts with: the referee opens a
/// record of the placement's [`RobotKind`] with this HP when the field adds a
/// chassis, under the chassis' id and team. One value serves every class; no
/// per-class HP table is modelled.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RobotConfig {
    /// Table 5-12/5-13 level-1 values are typical; no levelling is modelled.
    pub max_hp: u32,
}
impl Default for RobotConfig {
    /// The HP-focused level-1 infantry value (Table 5-13: 200 HP).
    fn default() -> Self {
        Self { max_hp: 200 }
    }
}

/// Match rules for one field: who owns each rune and outpost, what every
/// chassis becomes, how long the round lasts and the seed the runes draw from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefereeConfig {
    /// Owner of each field rune, by rune index.
    pub rune_teams: Vec<Team>,
    /// Owner of each field outpost, by outpost index.
    pub outpost_teams: Vec<Team>,
    /// The HP every chassis' robot starts with; its class comes from the
    /// placement.
    pub robot: RobotConfig,
    /// Round length in ns. Section 6.6 gives 7 min; the constructor accepts at
    /// most one day and refuses zero.
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
    /// No match: runes run their training policy.
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

/// An active rune buff (section 5.5.2). Attack and cooling are reported for
/// clients; only the defense share is applied here, to outpost and robot damage.
/// Times are on the round clock (`RefereeSnapshot::match_time_ns`), so a
/// `SkipTo` ages the buff with the round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuffState {
    /// Which stage granted the buff.
    pub source: RuneStage,
    /// Share of incoming damage removed, in percent.
    pub defense_pct: u32,
    /// Reported attack share; the live simulation does not apply it.
    pub attack_pct: u32,
    /// Reported cooling multiplier; the live simulation does not apply it.
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

/// One chassis' robot record: identity, team, class and HP.
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
    /// HP the robot started the match with.
    pub max_hp: u32,
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
    /// A base lost HP or shield to a detected strike.
    BaseDamaged {
        /// Team whose base was hit.
        team: Team,
        /// HP plus shield removed, after the team's defense buff.
        amount: u32,
        /// Base HP after the hit.
        hp: u32,
        /// Base shield after the hit.
        shield_hp: u32,
        /// The chassis whose projectile did it; `None` for a referee command.
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
        /// HP removed, after the team's defense buff.
        amount: u32,
        /// Robot HP after the hit.
        hp: u32,
        /// The chassis whose projectile did it; `None` for a referee command.
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
    /// A defeated robot was restored above zero HP.
    RobotRevived {
        /// Chassis id of the robot.
        robot: u32,
    },
    /// An outpost reached zero HP.
    OutpostDestroyed {
        /// Outpost index in the field's outpost list.
        outpost: u32,
        /// Team that owned it.
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
/// robot records, ownership and the recent events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefereeSnapshot {
    /// Operator mechanism overrides, indexed red then blue.
    #[serde(default)]
    pub base_open: [bool; 2],
    /// Dart door overrides, indexed red then blue; open by default.
    #[serde(default)]
    pub dart_door_open: [bool; 2],
    /// Gold, counters, allowances and income policy for both teams.
    pub gameplay: rm_simulator_gameplay::live::Resources,
    /// Where the match is.
    pub phase: MatchPhase,
    /// Elapsed round time; frozen at the end of a finished match.
    pub match_time_ns: u64,
    /// Round time left, `round_ns` minus `match_time_ns`.
    pub remaining_ns: u64,
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
    DART_TARGET_PERIOD_NS, Mechanism, MechanismState, dart_target_fraction,
};

/// Resolve match decisions before physics reads mechanism motion.
pub fn mechanism_state(referee: Option<&Referee>, time_ns: u64) -> MechanismState {
    MechanismState {
        base_open: referee.map_or([false; 2], |r| r.base_open),
        dart_door_open: referee.map_or([true; 2], |r| r.dart_door_open),
        dart_target_fraction: dart_target_fraction(time_ns),
    }
}
/// Resolve public match state for presentation clearance queries.
pub fn mechanism_view(referee: Option<&RefereeSnapshot>, time_ns: u64) -> MechanismState {
    MechanismState {
        base_open: referee.map_or([false; 2], |r| r.base_open),
        dart_door_open: referee.map_or([true; 2], |r| r.dart_door_open),
        dart_target_fraction: dart_target_fraction(time_ns),
    }
}

/// Operator and organiser inputs. `Field::referee_command` applies base and
/// outpost HP to the physical objects before forwarding; `Referee::command`
/// refuses those two variants on its own.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum RefereeCommand {
    /// Set a live base's HP and shield directly.
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
    /// Gold, allowance or counter edits for the resource tracker.
    Gameplay(rm_simulator_gameplay::live::Edit),
    /// Set an outpost's HP directly; restoration resumes its rotor.
    SetOutpostHp {
        /// Outpost index in the field's outpost list.
        outpost: u32,
        /// New HP, at most `outpost::INITIAL_HP`.
        hp: u32,
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
        /// Reported attack share in percent, at most 1000.
        attack_pct: u32,
        /// Reported cooling multiplier, at most 100.
        cooling_multiplier: u32,
        /// Buff duration in ns, at most the round length.
        duration_ns: u64,
    },
    /// Idle -> Countdown -> Running.
    StartMatch,
    /// Skip the round clock forward (testing), at most to the end of the
    /// round. Missed opportunities are granted, and buffs and activation
    /// windows, which run on the round clock, age with it; the runes' own
    /// hit windows keep world time.
    SkipTo {
        /// Round time to jump to, at most `round_ns`.
        match_time_ns: u64,
    },
    /// End a counting-down or running match now.
    FinishMatch,
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
    /// Apply damage to a robot (through its team's defense buff).
    DamageRobot {
        /// Chassis id of the robot.
        robot: u32,
        /// HP to remove before the buff; refused for an already defeated robot.
        amount: u32,
    },
    /// Restore a robot to full HP, keeping its position.
    ReviveRobot {
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

/// The match authority: teams, the countdown and round clock, rune
/// opportunities and buffs, outpost protection, robot HP and the live resource
/// tracker. It advances only through [`Referee::tick`] and [`Referee::command`],
/// which take explicit timestamps; it never reads host time.
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
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Referee {
    base_open: [bool; 2],
    dart_door_open: [bool; 2],
    gameplay: rm_simulator_gameplay::live::Resources,
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
    robots: Vec<RobotSnapshot>,
    outposts_destroyed: Vec<bool>,
    events: VecDeque<TimedEvent>,
    now_ns: u64,
}

impl Referee {
    /// `runes` and `outposts` are the field's counts; the config must own
    /// each of them.
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
        if config.round_ns == 0 || config.round_ns > 24 * 3_600_000_000_000 {
            return Err("referee round length must be within a day");
        }
        if config.robot.max_hp == 0 {
            return Err("referee robots need some HP");
        }
        Ok(Self {
            base_open: [false; 2],
            dart_door_open: [true; 2],
            gameplay: Default::default(),
            training_kinds: rune_kinds,
            phase: MatchPhase::Idle,
            phase_started_ns: 0,
            match_started_ns: 0,
            skipped_ns: 0,
            finished_match_time_ns: 0,
            schedule_index: 0,
            stage: RuneStage::Small,
            teams: [TeamState::fresh(), TeamState::fresh()],
            robots: Vec::new(),
            outposts_destroyed: vec![false; outposts],
            events: VecDeque::new(),
            now_ns: 0,
            config,
        })
    }
    /// Refuse a launch the live resource policy blocks. Outside a running
    /// round, and while enforcement is off, every launch is allowed.
    pub fn check_launch(
        &self,
        shooter: Option<u32>,
        caliber: crate::Caliber,
    ) -> Result<(), &'static str> {
        if self.phase == MatchPhase::Running {
            self.gameplay.check_launch(shooter, live_caliber(caliber))
        } else {
            Ok(())
        }
    }
    /// Charge a successful launch to the shooter's team during a running round.
    pub fn record_launch(&mut self, shooter: Option<u32>, caliber: crate::Caliber) {
        if self.phase == MatchPhase::Running {
            self.gameplay.launch(shooter, live_caliber(caliber));
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
    /// Defense buff share protecting outpost `index` right now.
    pub fn outpost_defense_pct(&self, index: usize) -> u32 {
        self.config
            .outpost_teams
            .get(index)
            .and_then(|team| self.teams[team.index()].buff)
            .map_or(0, |buff| buff.defense_pct)
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

    /// Advance to `now_ns` (never backwards), driving the runes it owns.
    pub fn tick(&mut self, now_ns: u64, runes: &mut [Rune]) -> Result<(), RuneError> {
        if now_ns < self.now_ns {
            return Err(RuneError::TimeReversal);
        }
        self.now_ns = now_ns;
        if self.phase == MatchPhase::Countdown
            && now_ns - self.phase_started_ns >= self.config.countdown_ns
        {
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
        self.gameplay
            .advance_to(match_time_ns.min(self.config.round_ns));
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
        if match_time_ns >= self.config.round_ns {
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
                    self.push_event(RefereeEvent::RuneActivated {
                        team,
                        stage,
                        arms,
                        average_ring,
                        buff,
                    });
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
            rune.deactivate(now_ns)?;
            self.push_event(RefereeEvent::BuffExpired { team });
        }
        Ok(())
    }

    /// Record a scored contact (called by the field after scoring).
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

    /// Defense share protecting `team`'s base right now, in percent; zero
    /// without a buff.
    pub fn base_defense_pct(&self, team: Team) -> u32 {
        self.teams[team.index()]
            .buff
            .map_or(0, |buff| buff.defense_pct)
    }
    /// Section 5.5.1 outpost protection, disabled in Idle training.
    pub fn base_protected(&self, team: Team) -> bool {
        self.phase != MatchPhase::Idle
            && self
                .config
                .outpost_teams
                .iter()
                .zip(&self.outposts_destroyed)
                .any(|(owner, dead)| *owner == team && !dead)
    }
    /// Record a base strike the field already applied to the physical base.
    /// Reaching zero HP finishes a running match.
    pub fn observe_base_hit(
        &mut self,
        team: Team,
        amount: u32,
        hp: u32,
        shield_hp: u32,
        shooter: Option<u32>,
    ) {
        if amount == 0 {
            return;
        }
        self.push_event(RefereeEvent::BaseDamaged {
            team,
            amount,
            hp,
            shield_hp,
            shooter,
        });
        if hp == 0 {
            self.push_event(RefereeEvent::BaseDestroyed { team });
            if self.phase == MatchPhase::Running {
                self.finished_match_time_ns = self.match_time(self.now_ns);
                self.phase = MatchPhase::Finished;
                self.push_event(RefereeEvent::MatchFinished);
            }
        }
    }
    /// Note outposts whose HP reached zero since the last call.
    pub fn observe_outposts(&mut self, destroyed: impl IntoIterator<Item = bool>) {
        for (index, now) in destroyed.into_iter().enumerate() {
            if index < self.outposts_destroyed.len() && now && !self.outposts_destroyed[index] {
                self.outposts_destroyed[index] = true;
                let team = self.config.outpost_teams[index];
                self.base_open[team.index()] = true;
                self.push_event(RefereeEvent::OutpostDestroyed {
                    outpost: index as u32,
                    team,
                });
            }
            if index < self.outposts_destroyed.len() {
                self.outposts_destroyed[index] = now;
            }
        }
    }

    /// Apply an operator command at `now_ns`, driving the runes it owns. A
    /// timestamp before the referee's current one, and any command the current
    /// phase does not allow, is refused with a message and no other change.
    /// `SetBaseHp` and `SetOutpostHp` must go through `Field::referee_command`.
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
        match command {
            RefereeCommand::Gameplay(edit) => {
                if matches!(edit, rm_simulator_gameplay::live::Edit::BuyAmmo { .. })
                    && self.phase != MatchPhase::Running
                {
                    return Err("the round is not running");
                }
                self.gameplay.edit(edit)
            }
            RefereeCommand::SetBaseHp { .. } => Err("base HP must be applied through Field"),
            RefereeCommand::SetBaseOpen { team, open } => {
                self.base_open[team.index()] = open;
                Ok(())
            }
            RefereeCommand::SetDartDoorOpen { team, open } => {
                self.dart_door_open[team.index()] = open;
                Ok(())
            }
            RefereeCommand::SetOutpostHp { .. } => Err("edit outpost HP through the field"),
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
                Ok(())
            }
            RefereeCommand::StartMatch => {
                if self.phase != MatchPhase::Idle {
                    return Err("a match is already in progress; reset it first");
                }
                for team in &mut self.teams {
                    *team = TeamState::fresh();
                }
                for robot in &mut self.robots {
                    robot.hp = robot.max_hp;
                }
                self.base_open = [false; 2];
                self.dart_door_open = [true; 2];
                self.gameplay.reset();
                self.stage = RuneStage::Small;
                self.schedule_index = 0;
                self.outposts_destroyed.fill(false);
                for (index, rune) in runes.iter_mut().enumerate() {
                    if index < self.config.rune_teams.len() {
                        rune.set_auto_restart(false);
                        rune.set_seed(self.rune_seed(index));
                        rune.convert(RuneKind::Small, now_ns).map_err(rune_error)?;
                        rune.deactivate(now_ns).map_err(rune_error)?;
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
                self.skipped_ns += match_time_ns.min(self.config.round_ns) - current;
                Ok(())
            }
            RefereeCommand::FinishMatch => {
                if !matches!(self.phase, MatchPhase::Countdown | MatchPhase::Running) {
                    return Err("no match to finish");
                }
                self.finish(now_ns, runes).map_err(rune_error)
            }
            RefereeCommand::ResetMatch => {
                if self.phase == MatchPhase::Idle {
                    return Err("no match to reset");
                }
                for team in &mut self.teams {
                    *team = TeamState::fresh();
                }
                for robot in &mut self.robots {
                    robot.hp = robot.max_hp;
                }
                self.base_open = [false; 2];
                self.dart_door_open = [true; 2];
                self.gameplay.reset();
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
                self.robot_index(robot)?;
                if self.robot_defeated(robot) == Some(true) {
                    return Err("the robot is already defeated");
                }
                self.hit_robot(robot, amount, None);
                Ok(())
            }
            RefereeCommand::ReviveRobot { robot } => {
                let index = self.robot_index(robot)?;
                self.robots[index].hp = self.robots[index].max_hp;
                self.push_event(RefereeEvent::RobotRevived { robot });
                Ok(())
            }
            RefereeCommand::SetRobotHp { robot, hp } => {
                let index = self.robot_index(robot)?;
                let was_defeated = self.robots[index].hp == 0;
                self.robots[index].hp = hp.min(self.robots[index].max_hp);
                if was_defeated && hp > 0 {
                    self.push_event(RefereeEvent::RobotRevived { robot });
                } else if !was_defeated && hp == 0 {
                    self.push_event(RefereeEvent::RobotDefeated { robot });
                }
                Ok(())
            }
        }
    }

    fn robot_index(&self, robot: u32) -> Result<usize, &'static str> {
        self.robots
            .iter()
            .position(|r| r.id == robot)
            .ok_or("unknown robot id")
    }
    /// Give a chassis its robot record of class `kind` at full HP. An id
    /// already present is an error; the field never reuses one.
    pub fn add_robot(
        &mut self,
        robot: u32,
        team: Team,
        kind: RobotKind,
    ) -> Result<(), &'static str> {
        if self.robot_index(robot).is_ok() {
            return Err("a robot with that id already exists");
        }
        let RobotConfig { max_hp } = self.config.robot;
        self.gameplay.add_robot(
            robot,
            match team {
                Team::Red => rm_simulator_gameplay::Team::Red,
                Team::Blue => rm_simulator_gameplay::Team::Blue,
            },
        );
        self.robots.push(RobotSnapshot {
            id: robot,
            team,
            kind,
            hp: max_hp,
            max_hp,
        });
        self.push_event(RefereeEvent::RobotJoined { robot, team });
        Ok(())
    }
    /// Drop a chassis' robot record when the field removes the chassis. The
    /// resource tracker forgets the same id.
    pub fn remove_robot(&mut self, robot: u32) -> Result<(), &'static str> {
        let index = self.robot_index(robot)?;
        self.robots.remove(index);
        self.gameplay.remove_robot(robot);
        self.push_event(RefereeEvent::RobotLeft { robot });
        Ok(())
    }
    /// Borrow the current robot records without copying resources or event history.
    pub fn robots(&self) -> &[RobotSnapshot] {
        &self.robots
    }
    /// Whether the robot is at zero HP; `None` for an unknown id.
    pub fn robot_defeated(&self, robot: u32) -> Option<bool> {
        self.robots
            .iter()
            .find(|r| r.id == robot)
            .map(|r| r.hp == 0)
    }
    /// Deal `amount` to a robot through its team's defense buff and report
    /// it; returns the HP actually removed (zero for an unknown or
    /// defeated robot, which absorbs nothing).
    pub fn hit_robot(&mut self, robot: u32, amount: u32, shooter: Option<u32>) -> u32 {
        let Ok(index) = self.robot_index(robot) else {
            return 0;
        };
        if self.robots[index].hp == 0 {
            return 0;
        }
        let team = self.robots[index].team;
        let defense = self.teams[team.index()].buff.map_or(0, |b| b.defense_pct);
        let amount = defended(amount, defense).min(self.robots[index].hp);
        self.robots[index].hp -= amount;
        let hp = self.robots[index].hp;
        self.push_event(RefereeEvent::RobotDamaged {
            robot,
            amount,
            hp,
            shooter,
        });
        if hp == 0 {
            self.push_event(RefereeEvent::RobotDefeated { robot });
        }
        amount
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
        self.phase = MatchPhase::Finished;
        self.phase_started_ns = now_ns;
        self.push_event(RefereeEvent::MatchFinished);
        Ok(())
    }

    /// The referee's public state at its current timestamp, including the
    /// event list. Taking one never advances the clock.
    pub fn snapshot(&self) -> RefereeSnapshot {
        let match_time_ns = self.match_time(self.now_ns);
        RefereeSnapshot {
            base_open: self.base_open,
            dart_door_open: self.dart_door_open,
            gameplay: self.gameplay.clone(),
            phase: self.phase,
            match_time_ns,
            remaining_ns: self.config.round_ns.saturating_sub(match_time_ns),
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
            robots: self.robots.clone(),
            rune_teams: self.config.rune_teams.clone(),
            outpost_teams: self.config.outpost_teams.clone(),
            events: self.events.iter().cloned().collect(),
        }
    }
}

fn live_caliber(caliber: crate::Caliber) -> rm_simulator_gameplay::Caliber {
    match caliber {
        crate::Caliber::Mm17 => rm_simulator_gameplay::Caliber::Mm17,
        crate::Caliber::Mm42 => rm_simulator_gameplay::Caliber::Mm42,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Pose, rune::SmallRune};

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
            .add_robot(0, Team::Red, RobotKind::Infantry)
            .unwrap();
        referee.add_robot(1, Team::Blue, RobotKind::Hero).unwrap();
        referee.events.clear();
        // Each record carries the class its placement named, at the one
        // configured HP.
        assert_eq!(referee.robots()[0].kind, RobotKind::Infantry);
        assert_eq!(referee.robots()[1].kind, RobotKind::Hero);
        assert_eq!(referee.robots()[1].max_hp, referee.robots()[0].max_hp);
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
        lifeless.robot.max_hp = 0;
        assert!(Referee::new(lifeless, vec![RuneKind::Small; 2], 2).is_err());
        let mut twice = referee();
        assert!(twice.add_robot(0, Team::Red, RobotKind::Infantry).is_err());
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
        referee.tick(6_999_999_999, &mut runes).unwrap();
        assert_eq!(referee.phase(), MatchPhase::Countdown);
        referee.tick(7_000_000_000, &mut runes).unwrap();
        let snapshot = referee.snapshot();
        assert_eq!(snapshot.phase, MatchPhase::Running);
        assert_eq!(snapshot.match_time_ns, 0);
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
        assert_eq!(referee.outpost_defense_pct(0), 25);
        assert_eq!(referee.outpost_defense_pct(1), 0);
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
        assert_eq!(referee.outpost_defense_pct(0), 0);
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
        referee.observe_outposts([false, true]);
        referee.observe_outposts([false, true]);
        let snapshot = referee.snapshot();
        let destroyed: Vec<_> = snapshot
            .events
            .iter()
            .filter(|e| matches!(e.event, RefereeEvent::OutpostDestroyed { .. }))
            .collect();
        assert_eq!(destroyed.len(), 1);
        assert_eq!(snapshot.base_open, [false, true]);
        assert!(matches!(
            destroyed[0].event,
            RefereeEvent::OutpostDestroyed {
                outpost: 1,
                team: Team::Blue
            }
        ));
        assert_eq!(destroyed[0].match_time_ns, Some(2_800_000_002));
        // Finish early and the clock freezes.
        referee
            .command(RefereeCommand::FinishMatch, 8_000_000_000, &mut runes)
            .unwrap();
        assert_eq!(referee.snapshot().match_time_ns, 3_000_000_000);
        assert_eq!(referee.tick(1, &mut runes), Err(RuneError::TimeReversal));
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
        // Damage to a buffed team's robot is reduced by the defense share.
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
        assert_eq!(referee.snapshot().robots[0].hp, 125);
        assert_eq!(
            referee.snapshot().events.last().unwrap().event,
            RefereeEvent::RobotDamaged {
                robot: 0,
                amount: 75,
                hp: 125,
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
