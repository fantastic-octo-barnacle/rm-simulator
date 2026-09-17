// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
use crate::coverage::Mechanic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One of the two competing sides. Slots in two-element arrays follow
/// [`Team::BOTH`], red first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Team {
    /// Red team.
    Red,
    /// Blue team.
    Blue,
}
impl Team {
    /// Both teams in slot order: red, then blue.
    pub const BOTH: [Self; 2] = [Self::Red, Self::Blue];
    /// Slot of this team in two-element per-team arrays; red is 0, blue is 1.
    pub const fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Blue => 1,
        }
    }
    /// The opposing team.
    pub const fn other(self) -> Self {
        match self {
            Self::Red => Self::Blue,
            Self::Blue => Self::Red,
        }
    }
}
/// Robot types a roster can hold. The kind fixes which calibers the robot can
/// fire, whether it gains levels and whether it drives on the ground.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RobotKind {
    /// Hero, the 42 mm shooter.
    Hero,
    /// Engineer, which rebuilds a destroyed own outpost in 5 s instead of 10 s
    /// (section 5.5.1).
    Engineer,
    /// Infantry, a 17 mm ground robot that gains levels.
    Infantry,
    /// Drone, a 17 mm aerial robot whose shots require active air support.
    Drone,
    /// Sentry, a 17 mm ground robot that starts with 300 rounds and claims
    /// accumulated resupply.
    Sentry,
    /// Dart launcher, tracked through observations rather than launched here.
    Dart,
    /// Radar, tracked through observations.
    Radar,
}
impl RobotKind {
    /// Whether the kind drives on the field. Ground robots alone respawn on a
    /// timer, rebuild outposts and heal at resupply.
    pub fn ground(self) -> bool {
        matches!(
            self,
            Self::Hero | Self::Engineer | Self::Infantry | Self::Sentry
        )
    }
    /// Whether the kind gains levels from experience.
    pub fn levels(self) -> bool {
        matches!(self, Self::Hero | Self::Infantry | Self::Drone)
    }
    /// Whether the kind fires the caliber: Hero 42 mm, Infantry, Sentry and
    /// Drone 17 mm.
    pub fn shoots(self, caliber: Caliber) -> bool {
        matches!(
            (self, caliber),
            (Self::Hero, Caliber::Mm42)
                | (Self::Infantry | Self::Sentry | Self::Drone, Caliber::Mm17)
        )
    }
}
/// Projectile caliber. The index matches the per-caliber arrays: 17 mm first,
/// 42 mm second.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Caliber {
    /// 17 mm projectile.
    Mm17,
    /// 42 mm projectile.
    Mm42,
}
impl Caliber {
    /// Slot of this caliber in two-element arrays; 17 mm is 0, 42 mm is 1.
    pub const fn index(self) -> usize {
        match self {
            Self::Mm17 => 0,
            Self::Mm42 => 1,
        }
    }
}

/// One roster entry. HP, heat limit and cooling come from `performance` at the
/// robot's current level (section 5.4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotConfig {
    /// Caller-assigned id, unique within the roster.
    pub id: u32,
    /// Team the robot belongs to.
    pub team: Team,
    /// Robot type, which fixes its weapon and level behaviour.
    pub kind: RobotKind,
    /// Performance tables or fixed values; must fit `kind`
    /// ([`crate::Performance::fits`]).
    pub performance: crate::Performance,
}
impl RobotConfig {
    /// A Hero or Infantry with its section 5.4.2 default performance, or any
    /// other kind with `fallback`.
    ///
    /// ```
    /// use rm_simulator_gameplay::{RobotConfig, RobotKind, Stats, Team};
    ///
    /// let fallback = Stats { max_hp: 400, chassis_power_w: 0, heat_limit: 100, cooling_per_s: 20 };
    /// let infantry = RobotConfig::standard(1, Team::Red, RobotKind::Infantry, fallback);
    /// assert_eq!(infantry.performance.stats(1).max_hp, 200);
    /// let sentry = RobotConfig::standard(2, Team::Red, RobotKind::Sentry, fallback);
    /// assert_eq!(sentry.performance.stats(1).max_hp, 400);
    /// ```
    pub fn standard(id: u32, team: Team, kind: RobotKind, fallback: crate::Stats) -> Self {
        Self {
            id,
            team,
            kind,
            performance: crate::Performance::default_for(kind)
                .unwrap_or(crate::Performance::Fixed(fallback)),
        }
    }
}
/// Rule switches a host may relax for practice. Defaults follow the manual.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Whether a zero projectile allowance refuses a launch (section 5.3.2).
    /// When off, allowance is still counted and may saturate at zero, and the
    /// section 5.3.2 over-allowance 42 mm immunity is not triggered.
    pub enforce_allowance: bool,
    /// Whether a local ammo exchange requires an own base, resupply or outpost
    /// zone contact. Hosts without zone detection may turn it off.
    pub exchange_requires_zone: bool,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            enforce_allowance: true,
            exchange_requires_zone: true,
        }
    }
}
/// Match length and its win threshold.
///
/// ```
/// use rm_simulator_gameplay::{Command, Config, Game, MatchFormat, Phase, COUNTDOWN_TICKS};
///
/// let mut game = Game::new(Config {
///     format: MatchFormat::Bo2,
///     ..Config::default()
/// })?;
/// for _ in 0..2 {
///     game.command(Command::BeginCountdown)?;
///     game.step(COUNTDOWN_TICKS)?;
///     game.command(Command::EndRound)?;
///     game.command(Command::ConfirmResult)?;
/// }
/// assert_eq!(game.snapshot().phase, Phase::MatchEnded);
/// // Two drawn rounds leave a Bo2 drawn.
/// assert_eq!(game.snapshot().match_winner, None);
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchFormat {
    /// Best of two: two confirmed rounds end the match, drawn or not.
    Bo2,
    /// Best of three: the first team with two wins takes the match.
    Bo3,
    /// Best of five: the first team with three wins takes the match.
    Bo5,
}
/// Immutable match configuration. [`crate::Game::new`] validates it once; every
/// new round restores robot state from this roster.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Match length and win threshold.
    pub format: MatchFormat,
    /// Initial roster. Ids must be unique and every performance must fit its
    /// kind. [`crate::Command::AddRobot`] and [`crate::Command::RemoveRobot`]
    /// change the roster later.
    pub robots: Vec<RobotConfig>,
    /// How many recent events the snapshot keeps. Application policy, not a
    /// rulebook constant; hosts that serialize snapshots often keep few.
    pub event_memory: usize,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            format: MatchFormat::Bo3,
            robots: Vec::new(),
            event_memory: 512,
        }
    }
}
/// Lifecycle phase of a round. Durations are 180 s setup, 15 s initialization,
/// 5 s countdown and 420 s round (sections 6.3 to 6.6 of the V2.1.0 manual).
///
/// ```
/// use rm_simulator_gameplay::{
///     Command, Config, Game, Phase, COUNTDOWN_TICKS, INITIALIZATION_TICKS, ROUND_TICKS,
///     SETUP_TICKS,
/// };
///
/// let mut game = Game::new(Config::default())?;
/// game.command(Command::BeginRound)?;
/// game.step(SETUP_TICKS)?;
/// assert_eq!(game.snapshot().phase, Phase::Initialization);
/// game.step(INITIALIZATION_TICKS)?;
/// assert_eq!(game.snapshot().phase, Phase::Countdown);
/// game.step(COUNTDOWN_TICKS)?;
/// assert_eq!(game.snapshot().phase, Phase::Running);
/// game.step(ROUND_TICKS)?;
/// assert_eq!(game.snapshot().phase, Phase::RoundEnded);
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// No round started. BeginRound and BeginCountdown are accepted here.
    Idle,
    /// 180 s setup, advancing automatically to initialization.
    Setup,
    /// 15 s initialization, advancing automatically to countdown.
    Initialization,
    /// 5 s countdown, advancing automatically to running.
    Countdown,
    /// Round in progress. Only this phase accepts gameplay commands.
    Running,
    /// Round over, awaiting ConfirmResult or Adjudicate.
    RoundEnded,
    /// Result recorded. BeginRound starts the next round.
    Confirmed,
    /// Match decided. Only ResetMatch leaves this phase.
    MatchEnded,
}
/// Which section 5.8 comparison decided the round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionReason {
    /// A base reached zero HP; the higher remaining base HP wins.
    BaseDestroyed,
    /// Outpost comparison, by current HP or by whether one was ever destroyed.
    Outpost,
    /// Higher effective attack damage wins.
    AttackDamage,
    /// Sum of living ground and drone robot HP decides.
    RobotHp,
    /// Every comparison tied, so the round is a draw.
    Equal,
    /// A referee decided the round through `Command::Adjudicate`.
    Referee,
}
/// Outcome of a round.
///
/// ```
/// use rm_simulator_gameplay::{
///     Command, Config, DamageKind, Error, Game, RoundResult, Target, Team, COUNTDOWN_TICKS,
/// };
///
/// # let mut game = Game::new(Config::default())?;
/// # game.command(Command::BeginCountdown)?;
/// # game.step(COUNTDOWN_TICKS)?;
/// game.command(Command::Damage {
///     target: Target::Outpost(Team::Blue),
///     amount: 1_500,
///     kind: DamageKind::Projectile,
///     attacker: Some(Team::Red),
/// })?;
/// game.command(Command::Damage {
///     target: Target::Base(Team::Blue),
///     amount: 200,
///     kind: DamageKind::Projectile,
///     attacker: Some(Team::Red),
/// })?;
/// game.command(Command::EndRound)?;
/// // Section 5.8 does not say how unequal surviving bases decide a round.
/// assert_eq!(game.snapshot().result, Some(RoundResult::NeedsRefereeDecision));
/// assert_eq!(game.command(Command::ConfirmResult), Err(Error::Ineligible));
/// game.command(Command::Adjudicate {
///     winner: Some(Team::Red),
/// })?;
/// game.command(Command::ConfirmResult)?;
/// assert_eq!(
///     game.snapshot().result,
///     Some(RoundResult::Decided {
///         winner: Some(Team::Red),
///         reason: rm_simulator_gameplay::DecisionReason::Referee,
///     })
/// );
/// # Ok::<(), Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundResult {
    /// A decided result; a `None` winner is a draw.
    Decided {
        /// Winning team, or `None` for a draw.
        winner: Option<Team>,
        /// Comparison that produced the result.
        reason: DecisionReason,
    },
    /// Section 5.8 omits the case of unequal, nonzero base HP and does not
    /// clarify rebuilt outpost comparisons. No winner is invented.
    NeedsRefereeDecision,
}
/// One confirmed round result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundRecord {
    /// One-based round number.
    pub round: u32,
    /// Round ticks elapsed when the round ended.
    pub elapsed_ticks: u64,
    /// Confirmed outcome of the round.
    pub result: RoundResult,
}
/// Field zone types a caller can report contacts for (sections 5.5.3.2 to
/// 5.5.3.9). Contacts are recorded whether or not a rule consumes them.
///
/// ```
/// use rm_simulator_gameplay::{
///     Caliber, Command, Config, Game, RobotConfig, RobotKind, Team, Zone, ZoneKind,
///     COUNTDOWN_TICKS,
/// };
///
/// # let mut game = Game::new(Config {
/// #     robots: vec![RobotConfig {
/// #         id: 1,
/// #         team: Team::Red,
/// #         kind: RobotKind::Infantry,
/// #         performance: rm_simulator_gameplay::Performance::default_for(RobotKind::Infantry).unwrap(),
/// #     }],
/// #     ..Config::default()
/// # })?;
/// # game.command(Command::BeginCountdown)?;
/// # game.step(COUNTDOWN_TICKS)?;
/// # game.step(1_000)?; // income at 1 s funds the exchange
/// let exchange = Command::ExchangeAmmo {
///     robot: 1,
///     caliber: Caliber::Mm17,
///     amount: 10,
///     remote: false,
/// };
/// // A local exchange needs an own base, resupply or living outpost zone.
/// assert!(game.command(exchange.clone()).is_err());
/// game.command(Command::ZoneDetection {
///     robot: 1,
///     zone: Zone {
///         kind: ZoneKind::Base,
///         owner: Team::Red,
///     },
///     detected: true,
/// })?;
/// game.command(exchange)?;
/// assert_eq!(game.snapshot().robots[0].allowance[0], 10);
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ZoneKind {
    /// Base zone. Own-side contact grants 50 percent defense, enables local
    /// exchange and clears weakness.
    Base,
    /// Resupply zone. Contact enables local exchange and healing, accelerates
    /// respawn and grants sentry resupply.
    Resupply,
    /// Outpost zone. A living own outpost enables exchange and clears
    /// weakness; contact also scans a destroyed outpost.
    Outpost,
    /// Central highland. Contact is recorded and generates no buff.
    CentralHighland,
    /// Trapezoid highland. Contact is recorded and generates no buff.
    TrapezoidHighland,
    /// Undulating road. Contact is recorded with no sequencing or reward.
    Road,
    /// Elevated crossing. Contact is recorded with no sequencing or reward.
    ElevatedCrossing,
    /// Launch ramp. Contact is recorded with no sequencing or reward.
    LaunchRamp,
    /// Tunnel. Contact is recorded with no sequencing or reward.
    Tunnel,
    /// Assembly zone. Contact is recorded and generates no buff.
    Assembly,
    /// Fortress. Contact is recorded and generates no buff.
    Fortress,
}
/// A zone identified by kind and owning team.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Zone {
    /// Zone type.
    pub kind: ZoneKind,
    /// Team the zone belongs to.
    pub owner: Team,
}
/// Detected contact with one zone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneContact {
    /// Zone the contact was reported for.
    pub zone: Zone,
    /// Latest sample. `false` keeps the contact until `expires_ticks`.
    pub detected: bool,
    /// Round tick when a false sample removes the contact, two seconds after
    /// the first false sample (section 5.5.3.1). `None` while detected.
    pub expires_ticks: Option<u64>,
}
/// Respawn timer for a defeated ground robot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Respawn {
    /// Round ticks of progress needed to respawn.
    pub required_ticks: u64,
    /// Progress accumulated so far. Resupply contact or a base below 2000 HP
    /// adds 4 per tick instead of 1 (section 5.2.2).
    pub progress_ticks: u64,
}
/// Per-robot state within a round.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotState {
    /// Roster entry this state was created from.
    pub config: RobotConfig,
    /// Current health; zero means defeated.
    pub hp: u32,
    /// Current level, from 1 up to the team's `level_cap` (section 5.4.1).
    pub level: u8,
    /// Tenths of an experience point, to allow the manual's fractional awards.
    pub experience_tenths: u32,
    /// Remaining rounds per caliber in index order; a launch consumes one.
    pub allowance: [u32; 2],
    /// Shots detected per caliber, including shots fired over allowance.
    pub shots_launched: [u64; 2],
    /// Detected shots per caliber fired with zero allowance.
    pub shots_over_allowance: [u64; 2],
    /// Tenths of a heat unit; section 5.1.3 cools at 10 Hz.
    pub heat_tenths: u64,
    /// Set when heat exceeds the limit and cleared when heat cools to zero
    /// (section 5.1.3).
    pub overheated: bool,
    /// Permanent heat lock, set at the limit plus the caliber buffer
    /// (Figure 5-1). It clears only on a new round.
    pub heat_locked_for_round: bool,
    /// Round tick until which launch is locked for an excess launch speed.
    /// Independent of heat.
    pub speed_locked_until_ticks: u64,
    /// Permanent speed lock for a launch far over the limit. It clears only on
    /// a new round.
    pub speed_locked_for_round: bool,
    /// Paid instant respawns this round. Each adds 20 s to a later respawn
    /// timer (section 5.2.2).
    pub instant_respawns: u32,
    /// Respawn timer while a defeated ground robot waits to return. `None` for
    /// aerial and ejected robots.
    pub respawn: Option<Respawn>,
    /// Weakness after a respawn. It blocks launching and exchange until an own
    /// service zone clears it (section 5.2.2).
    pub weakened: bool,
    /// Round tick when an instant respawn's fixed three-second weakness ends.
    /// `None` when zone contact must clear the weakness instead.
    pub weakened_until_ticks: Option<u64>,
    /// Round tick of the last respawn. Clearing weakness keeps invincibility
    /// until at least 10 s after it.
    pub respawned_at_ticks: Option<u64>,
    /// Round tick until which projectile, dart and collision damage are
    /// ignored (section 5.2.2). Penalties still apply.
    pub invincible_until_ticks: u64,
    /// Irregular disconnection. It blocks launching, exchange, respawn
    /// progress and deliveries until cleared (section 5.1.5).
    pub irregularly_disconnected: bool,
    /// Ejected by penalty damage. An ejected robot is not alive, has no
    /// respawn timer and cannot be instantly respawned.
    pub ejected: bool,
    /// Authoritative referee observation; combat classification is not inferred.
    pub out_of_combat: bool,
    /// Zone contacts, including ones kept alive during their expiry delay.
    pub zones: Vec<ZoneContact>,
    /// Continuous outpost rebuild scan progress in round ticks. It resets when
    /// the contact or the robot's eligibility lapses.
    pub rebuild_progress_ticks: u64,
    /// Sentry resupply already granted in whole minutes. The next grant adds
    /// only the difference (section 5.3.2).
    pub sentry_resupply_claimed: u32,
    /// Section 5.6.7. None for robot kinds without chassis energy.
    pub chassis_energy_j: Option<u32>,
    /// Remaining drone air-support time in round ticks. It grows by 20 s at
    /// each whole minute and one second costs one gold (section 5.6.3).
    pub air_support_ticks: u64,
    /// Whether the drone's air support is switched on. A drone without active
    /// support cannot launch.
    pub air_support_active: bool,
    /// Round tick of the latest detected launch per caliber, for the section
    /// 5.1.1 four-second 42 mm rule.
    pub last_launch_ticks: [Option<u64>; 2],
    /// Round tick the robot was last defeated, cleared when it returns.
    pub defeated_at_ticks: Option<u64>,
    /// 42 mm launches detected since the robot was last defeated.
    pub launches_since_defeat: u32,
    /// Section 5.3.2 Hero immunity trigger: set by a 42 mm launch over
    /// allowance or the third launch after defeat, cleared once the robot is
    /// alive with positive 42 mm allowance.
    pub mm42_suspended: bool,
}
impl RobotState {
    /// Whether the robot is on the field: positive HP and not ejected.
    pub fn alive(&self) -> bool {
        self.hp > 0 && !self.ejected
    }
    /// Whether the robot holds a contact with the given zone and owner.
    pub fn in_zone(&self, kind: ZoneKind, owner: Team) -> bool {
        self.zones.iter().any(|c| c.zone == Zone { kind, owner })
    }
    /// Current performance values at the robot's level.
    pub fn stats(&self) -> crate::Stats {
        self.config.performance.stats(self.level)
    }
    /// Robot-local launch permission at round tick `now_ticks`. It requires the
    /// robot to be alive, connected, not weakened, overheated or locked, armed
    /// with the caliber, holding allowance when `enforce_allowance` and, for a
    /// drone, flying with active air support. Match phase is checked by
    /// [`crate::Game::can_launch`].
    pub fn can_launch(&self, now_ticks: u64, caliber: Caliber, enforce_allowance: bool) -> bool {
        self.alive()
            && !self.irregularly_disconnected
            && !self.weakened
            && !self.overheated
            && !self.heat_locked_for_round
            && !self.speed_locked_for_round
            && self.speed_locked_until_ticks <= now_ticks
            && self.config.kind.shoots(caliber)
            && (!enforce_allowance || self.allowance[caliber.index()] > 0)
            && (self.config.kind != RobotKind::Drone || self.air_support_active)
    }
}
/// Per-team state within a round.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamState {
    /// Team this state describes.
    pub team: Team,
    /// Current gold in the team's account (section 5.3.1).
    pub gold: u32,
    /// Rounds bought per caliber this round, against the Table 5-7 to 5-9
    /// exchange limits of 1000 for 17 mm and 100 for 42 mm.
    pub exchanged_allowance: [u32; 2],
    /// Base health. Zero ends the round immediately (section 5.5.1).
    pub base_hp: u32,
    /// Virtual base shield. Damage consumes it before base HP and counts as
    /// attack damage (section 5.5.1).
    pub base_shield_hp: u32,
    /// Cumulative base HP lost this round, which sets rebuild opportunities.
    pub base_hp_lost: u32,
    /// Tracked base-armor state; the engine never changes or reads it.
    pub base_armor_expanded: bool,
    /// Outpost health. Zero means destroyed and enables a rebuild scan
    /// (section 5.5.1).
    pub outpost_hp: u32,
    /// Whether the outpost was destroyed at least once this round. Section 5.8
    /// compares this before current outpost HP.
    pub outpost_ever_destroyed: bool,
    /// Round tick of the outpost's first destruction this round, which stops
    /// its middle armor for good (section 5.5.1).
    pub outpost_first_destroyed_ticks: Option<u64>,
    /// Unspent rebuild opportunities, one per 1000 cumulative base HP lost
    /// (section 5.5.1).
    pub outpost_rebuild_opportunities: u32,
    /// Effective attack damage credited to this team: HP and shield removed by
    /// projectiles, darts and penalties (section 5.8).
    pub attack_damage: u64,
    /// Highest level this team's robots can reach. It starts at 5 and assembly
    /// completions raise it (section 5.3.3).
    pub level_cap: u8,
    /// Assembly completions per level in level order. A repeated completion
    /// pays reduced income.
    pub assembly_completions: [u32; 4],
    /// Gold granted to this team at each round-aligned 10 s boundary while
    /// positive (section 5.3.3).
    pub assembly_income_per_10_s: u32,
    /// Damage reduction in percent from certified assemblies, applied to
    /// projectile and collision damage.
    pub assembly_defense_pct: u32,
    /// Extra experience, in tenths, the current Small Rune buff can still
    /// double; zero without one (section 5.5.2: at most 1,200 points).
    pub rune_bonus_tenths: u32,
    /// Round tick the Small Rune experience bonus ends.
    pub rune_bonus_until_ticks: u64,
}
/// A damage target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    /// A robot, by its configured id.
    Robot(u32),
    /// A team's base.
    Base(Team),
    /// A team's outpost.
    Outpost(Team),
}
/// What a [`Buff`] applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuffTarget {
    /// One robot, by id; dropped when the robot is defeated or removed.
    Robot(u32),
    /// A team's base.
    Base(Team),
    /// A team's outpost.
    Outpost(Team),
    /// Every robot of the team, its base and its outpost (section 5.5.2 rune
    /// buffs), including robots that join while it lasts.
    Team(Team),
}
/// One detected projectile strike on armor. The engine applies Table 5-2,
/// the shooter's attack buff, the section 5.5.1 centre square, the Hero 42 mm
/// immunity of sections 5.1.1 and 5.3.2, target defenses and experience.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectileHit {
    /// Robot, base or outpost whose armor detected the strike.
    pub target: Target,
    /// Caliber detected.
    pub caliber: Caliber,
    /// Robot that launched the projectile, when known. An id no longer in the
    /// roster counts as an unidentified source.
    pub shooter: Option<u32>,
    /// The base's upper front armor module, where 17 mm deals 5 HP (Table
    /// 5-2). Only valid for a base target.
    pub upper_front: bool,
    /// Inside the 10 mm x 10 mm centre square of a base or outpost armor
    /// module, a 150 % attack (section 5.5.1). Only valid for those targets.
    pub critical: bool,
}
/// What a damage command removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// HP removed from the target.
    pub hp: u32,
    /// Base virtual shield removed; zero for other targets.
    pub shield: u32,
}
/// Source of a damage command. The kind selects which defenses apply and
/// whether the damage counts as attack damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DamageKind {
    /// Projectile hit. Defense, vulnerability, base outpost protection and
    /// robot invincibility apply, and the damage is credited as attack damage.
    Projectile,
    /// Dart hit. It bypasses defense and counts as attack damage, but robot
    /// invincibility and base outpost protection still apply.
    Dart,
    /// Collision damage. Defense and invincibility apply and it is never
    /// credited as attack damage.
    Collision,
    /// Referee penalty. It bypasses defense and invincibility and counts as
    /// attack damage.
    Penalty,
    /// Disconnection damage. It bypasses defense and is never credited as
    /// attack damage.
    Disconnection,
}
/// A caller-certified active buff; strongest effects of each type win
/// independently (section 5.5.3.1). Expiry uses round ticks.
///
/// ```
/// use rm_simulator_gameplay::{
///     coverage::Mechanic, Buff, BuffTarget, Command, Config, Game, RobotConfig, RobotKind, Team,
///     COUNTDOWN_TICKS,
/// };
///
/// # let mut game = Game::new(Config {
/// #     robots: vec![RobotConfig {
/// #         id: 1,
/// #         team: Team::Red,
/// #         kind: RobotKind::Infantry,
/// #         performance: rm_simulator_gameplay::Performance::default_for(RobotKind::Infantry).unwrap(),
/// #     }],
/// #     ..Config::default()
/// # })?;
/// # game.command(Command::BeginCountdown)?;
/// # game.step(COUNTDOWN_TICKS)?;
/// let buff = |defense_pct| Buff {
///     target: BuffTarget::Robot(1),
///     source: Mechanic::Rune,
///     attack_pct: 0,
///     defense_pct,
///     vulnerability_pct: 0,
///     cooling_multiplier: 1,
///     expires_ticks: 1_000,
/// };
/// game.command(Command::ApplyBuff(buff(25)))?;
/// game.command(Command::ApplyBuff(buff(50)))?;
/// // One buff per target and source is retained.
/// assert_eq!(game.snapshot().buffs.len(), 1);
/// assert_eq!(game.snapshot().buffs[0].defense_pct, 50);
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Buff {
    /// Robot, base, outpost or whole team the buff applies to.
    pub target: BuffTarget,
    /// Mechanic that granted the buff. One buff per target and source is kept.
    pub source: Mechanic,
    /// Projectile damage multiplier in percent when the target shoots (section
    /// 5.5.3.1); values below 100 mean no attack buff. The strongest applies.
    pub attack_pct: u32,
    /// Damage reduction in percent, at most 100. The strongest defense applies.
    pub defense_pct: u32,
    /// Extra damage taken in percent, added to the scaled damage. The strongest
    /// vulnerability applies.
    pub vulnerability_pct: u32,
    /// Multiplies heat cooling on each 100 ms boundary. The largest value
    /// applies and never drops below 1.
    pub cooling_multiplier: u32,
    /// Round tick at which the buff expires. It must be greater than the
    /// current round tick when applied.
    pub expires_ticks: u64,
}
/// Unsupported elements have explicit unknown/observed state rather than
/// invented defaults. Measurements carry units in their keys, e.g. `angle_rad`.
/// These records never grant rewards or apply penalties by themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementObservation {
    /// Coverage register group this observation belongs to.
    pub mechanic: Mechanic,
    /// Team the observation is about, or `None` when it is not team-specific.
    pub team: Option<Team>,
    /// Robot id the observation is about, or `None` when it is not
    /// robot-specific. A robot must belong to the named team.
    pub robot: Option<u32>,
    /// State name, non-empty and at most 4096 bytes.
    pub state: String,
    /// Named measurements, at most 64 entries with keys up to 128 and values
    /// up to 4096 bytes. Include units in keys.
    pub measurements: BTreeMap<String, String>,
    /// Round tick the observation was recorded at. The engine overwrites the
    /// caller's value.
    pub observed_at_ticks: u64,
}
/// Read-only view of the whole game. It serializes with serde but is not a
/// trusted save-game loading API; clone a [`crate::Game`] to branch a scenario.
///
/// ```
/// use rm_simulator_gameplay::{Command, Config, EventKind, Game, Phase, COUNTDOWN_TICKS};
///
/// # let mut game = Game::new(Config::default())?;
/// # game.command(Command::BeginCountdown)?;
/// # game.step(COUNTDOWN_TICKS)?;
/// let snapshot = game.snapshot();
/// assert_eq!(snapshot.phase, Phase::Running);
/// assert_eq!(snapshot.teams.len(), 2);
/// // Commands are recorded as events in emission order.
/// assert_eq!(
///     snapshot.recent_events[0].kind,
///     EventKind::PhaseChanged(Phase::Countdown)
/// );
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Monotonic simulation tick. It never goes backwards, not even on a match
    /// reset.
    pub tick: u64,
    /// Current lifecycle phase.
    pub phase: Phase,
    /// Ticks spent in the current phase.
    pub phase_elapsed_ticks: u64,
    /// One-based round number, incremented by BeginRound and BeginCountdown.
    pub round: u32,
    /// Ticks elapsed in the running round; frozen outside Running.
    pub round_elapsed_ticks: u64,
    /// Rule switches in force.
    pub policy: Policy,
    /// Per-team state in [`Team::BOTH`] order.
    pub teams: [TeamState; 2],
    /// Per-robot state in join order.
    pub robots: Vec<RobotState>,
    /// Active buffs, one per target and source.
    pub buffs: Vec<Buff>,
    /// Latest observation per mechanic, team and robot.
    pub observations: Vec<ElementObservation>,
    /// Result of the current or most recently ended round; cleared when the
    /// next round begins.
    pub result: Option<RoundResult>,
    /// Confirmed round records in play order.
    pub rounds: Vec<RoundRecord>,
    /// Winner once the match is decided, or `None` for a drawn match.
    pub match_winner: Option<Team>,
    /// Most recent events, capped at `Config::event_memory`. Event retention is
    /// an application policy.
    pub recent_events: Vec<Event>,
    /// Id the next event will receive. Ids restart at zero on a match reset.
    pub next_event_id: u64,
    /// Remote deliveries in due-tick order.
    pub pending_deliveries: Vec<Delivery>,
}
/// One recorded state change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Event id, unique within a match and assigned in emission order.
    pub id: u64,
    /// Simulation tick when the event was emitted.
    pub tick: u64,
    /// Round tick when the event was emitted.
    pub round_ticks: u64,
    /// What changed.
    pub kind: EventKind,
}
/// Kinds of recorded state change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    /// The phase changed to the given phase.
    PhaseChanged(Phase),
    /// A team received gold.
    GoldGranted {
        /// Team that received the gold.
        team: Team,
        /// Gold granted, in whole units.
        amount: u32,
    },
    /// Damage was applied to a target.
    Damage {
        /// Target the damage was applied to.
        target: Target,
        /// Robot credited as the source, when one was identified.
        shooter: Option<u32>,
        /// HP actually removed, after defenses and clamping to remaining HP.
        hp: u32,
        /// Base shield actually removed; always zero for non-base targets.
        shield: u32,
        /// Damage source.
        kind: DamageKind,
    },
    /// A robot reached zero HP.
    RobotDefeated(u32),
    /// A robot returned to the field through its timer or a paid respawn.
    RobotRespawned(u32),
    /// An operator restored a robot outside the respawn rules.
    RobotRevived(u32),
    /// A robot's weakened state ended.
    WeaknessCleared(u32),
    /// A robot joined the roster.
    RobotAdded(u32),
    /// A robot left the roster.
    RobotRemoved(u32),
    /// A team's outpost was rebuilt with 750 HP (section 5.5.1).
    OutpostRebuilt(Team),
    /// A robot reached a new level.
    LevelChanged {
        /// Robot that changed level.
        robot: u32,
        /// New level.
        level: u8,
    },
}

/// What a pending remote delivery carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryKind {
    /// Rounds of the given caliber.
    Ammo(Caliber, u32),
    /// Health recovery to 160 percent of current HP, capped at max HP.
    Hp,
}
/// A remote purchase scheduled to arrive six seconds after it was bought.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// Round tick the delivery becomes due.
    pub at_ticks: u64,
    /// Robot that receives the delivery.
    pub robot: u32,
    /// Payload that arrives.
    pub kind: DeliveryKind,
}
