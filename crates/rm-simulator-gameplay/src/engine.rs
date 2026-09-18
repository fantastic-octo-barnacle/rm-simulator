// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
use crate::policy::{IncomeSchedule, record_ammo_launch};
use crate::*;
use serde::{Deserialize, Serialize};

/// Table 5-11: total experience, in tenths, required for levels 1 to 10.
const LEVEL_XP_TENTHS: [u32; 10] = [
    0, 5500, 11000, 16500, 22000, 27500, 33000, 38500, 44000, 50000,
];
/// Section 5.5.2: the Small Rune doubles earned experience up to 1,200 points
/// per buff period, in tenths.
const SMALL_RUNE_BONUS_TENTHS: u32 = 12_000;
/// Section 5.5.2: a Large Rune activation shares 750 points, in tenths.
const LARGE_RUNE_TENTHS: u32 = 7_500;
/// Sections 5.1.1 and 5.3.2: seconds without a 42 mm launch, and seconds after
/// defeat, before 42 mm damage stops counting.
const MM42_IDLE_TICKS: u64 = 4 * SECOND_TICKS;
const MM42_DEFEAT_TICKS: u64 = 3 * SECOND_TICKS;

/// Power Rune stage (section 5.5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuneStage {
    /// Small Power Rune: doubles experience, capped at 1,200 points.
    Small,
    /// Large Power Rune: 750 experience points shared by the team.
    Large,
}

/// A validated input or authoritative observation. [`Game::command`] applies
/// one at a time, and a rejected command changes nothing.
///
/// ```
/// use rm_simulator_gameplay::{
///     Command, Config, DamageKind, DecisionReason, Game, Phase, RoundResult, Target, Team,
///     COUNTDOWN_TICKS,
/// };
///
/// # let mut game = Game::new(Config::default())?;
/// # game.command(Command::BeginCountdown)?;
/// # game.step(COUNTDOWN_TICKS)?;
/// // An outpost protects its base, so it must fall first (section 5.5.1).
/// game.command(Command::Damage {
///     target: Target::Outpost(Team::Blue),
///     amount: 1_500,
///     kind: DamageKind::Projectile,
///     attacker: Some(Team::Red),
/// })?;
/// game.command(Command::Damage {
///     target: Target::Base(Team::Blue),
///     amount: 5_150,
///     kind: DamageKind::Projectile,
///     attacker: Some(Team::Red),
/// })?;
/// assert_eq!(game.snapshot().phase, Phase::RoundEnded);
/// assert_eq!(
///     game.snapshot().result,
///     Some(RoundResult::Decided {
///         winner: Some(Team::Red),
///         reason: DecisionReason::BaseDestroyed,
///     })
/// );
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Start a round: 180 s setup, 15 s initialization, 5 s countdown, then the
    /// round. Accepted from Idle and Confirmed.
    BeginRound,
    /// Practice shortcut, explicitly skips setup and initialization.
    BeginCountdown,
    /// End a running round early and determine its result.
    EndRound,
    /// Record a referee decision for a round that returned
    /// [`RoundResult::NeedsRefereeDecision`]. Accepted in RoundEnded.
    Adjudicate {
        /// Team the referee declares the winner, or `None` for a draw.
        winner: Option<Team>,
    },
    /// Record the current result and advance the match. It requires a decided
    /// result.
    ConfirmResult,
    /// Clear every result and return to Idle with the current roster and
    /// policy, keeping the simulation tick.
    ResetMatch,
    /// Add a robot to the roster in any phase. Its id must be new and its
    /// performance must fit its kind.
    AddRobot(RobotConfig),
    /// Remove a robot, its buffs and its pending deliveries, in any phase.
    RemoveRobot {
        /// Robot to remove.
        robot: u32,
    },
    /// Select a robot's performance type (section 5.4.2). Refused while a
    /// round is running; the robot returns to its new maximum HP.
    SetPerformance {
        /// Robot to configure.
        robot: u32,
        /// New performance source; it must fit the robot's kind.
        performance: Performance,
    },
    /// Replace the rule switches, in any phase.
    SetPolicy(Policy),
    /// A detected projectile strike on armor. In Idle it is free practice:
    /// damage without protection, invincibility, experience or respawn timers.
    ProjectileHit(ProjectileHit),
    /// Detected damage after attacker buffs, before target defenses. Caller
    /// handles detection, attacker effects and special target immunities. In
    /// Idle it is free practice like [`Command::ProjectileHit`].
    Damage {
        /// Robot, base or outpost taking the damage.
        target: Target,
        /// Damage before target defenses and vulnerability, already including
        /// attacker-side effects.
        amount: u32,
        /// Damage source, which selects the defenses and the attack credit.
        kind: DamageKind,
        /// Team credited with the damage. `None` credits the opposing team.
        attacker: Option<Team>,
    },
    /// A team activated its Power Rune (section 5.5.2): a team-wide buff for
    /// `duration_ticks`, plus the stage's experience effect.
    RuneActivated {
        /// Team that activated its rune.
        team: Team,
        /// Small or Large Rune.
        stage: RuneStage,
        /// Attack multiplier in percent, at most 1000 (Table 5-16).
        attack_pct: u32,
        /// Defense in percent, at most 100.
        defense_pct: u32,
        /// Cooling multiplier, at most 100.
        cooling_multiplier: u32,
        /// Buff length in round ticks; positive.
        duration_ticks: u64,
    },
    /// Advance the round clock to `round_ticks` without advancing the
    /// simulation tick, processing every timer on the way. Testing and
    /// operator use; `round_ticks` must not be in the past.
    SkipTo {
        /// Round tick to reach, at most [`ROUND_TICKS`].
        round_ticks: u64,
    },
    /// Operator override: set a robot's HP, clamped to its maximum. Zero
    /// defeats it without a destroyer; a positive value on a defeated robot
    /// revives it unweakened.
    SetRobotHp {
        /// Robot to edit.
        robot: u32,
        /// New HP.
        hp: u32,
    },
    /// Operator override: restore a robot to full HP, clearing its respawn
    /// timer, weakness and ejection.
    Revive {
        /// Robot to revive.
        robot: u32,
    },
    /// Operator override: end a robot's weakened state as an own service zone
    /// contact would (section 5.2.2).
    ClearWeakened {
        /// Robot to clear.
        robot: u32,
    },
    /// Operator override: set a base's HP and virtual shield. Zero HP during a
    /// round destroys the base and ends the round.
    SetBase {
        /// Team whose base to edit.
        team: Team,
        /// New HP, at most [`BASE_HP`].
        hp: u32,
        /// New shield, at most [`BASE_HP`].
        shield_hp: u32,
    },
    /// Operator override: set an outpost's HP, at most [`OUTPOST_HP`]. Zero
    /// during a round counts as a destruction.
    SetOutpostHp {
        /// Team whose outpost to edit.
        team: Team,
        /// New HP.
        hp: u32,
    },
    /// Operator override: set a team's gold.
    SetGold {
        /// Team to edit.
        team: Team,
        /// New balance.
        gold: u32,
    },
    /// Operator override: set a robot's projectile allowance.
    SetAllowance {
        /// Robot to edit.
        robot: u32,
        /// New allowance in caliber index order.
        allowance: [u32; 2],
    },
    /// Report a zone contact sample for a robot. A false sample keeps the
    /// contact for two more seconds (section 5.5.3.1).
    ZoneDetection {
        /// Robot the sample is for.
        robot: u32,
        /// Zone kind and owner the sample is for.
        zone: Zone,
        /// Whether the zone is currently detected.
        detected: bool,
    },
    /// Report whether a robot is out of combat, which gates remote purchases.
    CombatState {
        /// Robot the observation is about.
        robot: u32,
        /// True while the robot is out of combat.
        out_of_combat: bool,
    },
    /// Report an irregular disconnection, which blocks launch, exchange,
    /// respawn progress and deliveries.
    Disconnection {
        /// Robot the observation is about.
        robot: u32,
        /// True for an irregular disconnection, false once it is cleared.
        irregular: bool,
    },
    /// Eject a robot with penalty damage and clear its respawn timer.
    Eject {
        /// Robot to eject.
        robot: u32,
    },
    /// Buy rounds for a robot's allowance with team gold (section 5.3.2,
    /// Tables 5-7 to 5-9).
    ExchangeAmmo {
        /// Robot buying the rounds.
        robot: u32,
        /// Caliber bought.
        caliber: Caliber,
        /// Rounds to buy; positive and a multiple of the exchange unit.
        amount: u32,
        /// True for a remote purchase, which requires out-of-combat status and
        /// arrives six seconds later. False requires an own service zone
        /// unless the policy waives it.
        remote: bool,
    },
    /// Buy remote HP recovery for a robot. It requires out-of-combat status and
    /// arrives six seconds later (Table 5-6).
    ExchangeHp {
        /// Robot buying the recovery.
        robot: u32,
    },
    /// Revive a defeated ground robot immediately at full HP for gold
    /// (section 5.2.2).
    InstantRespawn {
        /// Robot to revive.
        robot: u32,
    },
    /// Authorise a launch and account for it. It is rejected unless the robot
    /// may launch now; in Idle it only checks that the robot is alive and armed.
    Launch {
        /// Robot launching.
        robot: u32,
        /// Caliber launched.
        caliber: Caliber,
    },
    /// A sensor-certified shot that already happened, even while locked or
    /// over allowance. Never expose this as a player action.
    ObserveLaunch {
        /// Robot whose launch was detected.
        robot: u32,
        /// Caliber launched.
        caliber: Caliber,
    },
    /// Speeds use mm/s to keep state and comparisons exact.
    LaunchSpeed {
        /// Robot whose launch was measured.
        robot: u32,
        /// Caliber measured.
        caliber: Caliber,
        /// Measured launch speed in mm/s.
        measured_mm_s: u32,
        /// Configured limit in mm/s; zero is rejected.
        limit_mm_s: u32,
        /// True when a Hero 42 mm shot was taken while deployed. Only 42 mm may
        /// set it.
        deployed: bool,
    },
    /// Award experience tenths to a robot, capped by the team's level cap
    /// (Table 5-11).
    AwardExperience {
        /// Robot receiving the award.
        robot: u32,
        /// Experience to add, in tenths of a point.
        tenths: u32,
    },
    /// An authoritative assembly-completion observation, not a player claim.
    AssemblyCompleted {
        /// Team that completed the level.
        team: Team,
        /// Assembly level, 1 to 4. Level 4 can complete only once.
        level: u8,
    },
    /// Replace the buff for its target and source with a certified active buff.
    ApplyBuff(Buff),
    /// Remove the buff for a target and source, if there is one. Accepted in
    /// any phase.
    ClearBuff {
        /// Target the buff applies to.
        target: BuffTarget,
        /// Mechanic that granted it.
        source: coverage::Mechanic,
    },
    /// Switch a drone's air support on or off (section 5.6.3).
    AirSupport {
        /// Drone changing state.
        robot: u32,
        /// True to switch support on, false to switch it off.
        active: bool,
    },
    /// Report chassis energy consumption and resupply recharge in joules
    /// (section 5.6.7).
    Energy {
        /// Robot reporting energy.
        robot: u32,
        /// Energy consumed since the last report, in joules.
        consumed_j: u32,
        /// Surplus recharge in joules. A positive value requires an alive,
        /// unweakened robot in its own resupply zone and adds eight joules of
        /// chassis energy per unit.
        recharge_surplus_j: u32,
    },
    /// Record an observation of an externally owned mechanic. It replaces the
    /// previous observation for the same mechanic, team and robot.
    Observe(ElementObservation),
}
/// Rejection reason from [`Game::new`], [`Game::command`] and [`Game::step`].
/// A rejected call leaves the game unchanged.
///
/// ```
/// use rm_simulator_gameplay::{Command, Config, Error, Game};
///
/// let mut game = Game::new(Config::default())?;
/// // A round can only end while it is running.
/// assert_eq!(game.command(Command::EndRound), Err(Error::Phase));
/// # Ok::<(), Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The command is not valid in the current phase.
    #[error("command is invalid in this phase")]
    Phase,
    /// No robot in the roster has the given id.
    #[error("unknown robot id")]
    Robot,
    /// A configuration or command value is out of range.
    #[error("invalid configuration or command value")]
    Invalid,
    /// The robot cannot perform the action in its current state.
    #[error("robot is not eligible for this action")]
    Ineligible,
    /// The team lacks the gold or allowance the action costs.
    #[error("insufficient gold or allowance")]
    Insufficient,
    /// An exchange, allowance or counter limit would be exceeded.
    #[error("exchange limit exceeded")]
    Limit,
    /// Adding the requested ticks would overflow the simulation clock.
    #[error("clock overflow")]
    ClockOverflow,
}

/// A complete round lifecycle with explicit inputs for physical observations.
/// The snapshot is read-only; construct a game from validated configuration.
/// Clone a game to branch a deterministic scenario, replay commands to restore it.
///
/// ```
/// use rm_simulator_gameplay::{
///     Caliber, Command, Config, Game, Performance, RobotConfig, RobotKind, Stats, Team,
///     COUNTDOWN_TICKS,
/// };
///
/// let mut game = Game::new(Config {
///     robots: vec![RobotConfig {
///         id: 7,
///         team: Team::Red,
///         kind: RobotKind::Sentry,
///         performance: Performance::Fixed(Stats {
///             max_hp: 400,
///             chassis_power_w: 100,
///             heat_limit: 100,
///             cooling_per_s: 20,
///         }),
///     }],
///     ..Config::default()
/// })?;
/// game.command(Command::BeginCountdown)?;
/// game.step(COUNTDOWN_TICKS)?;
/// game.command(Command::Launch {
///     robot: 7,
///     caliber: Caliber::Mm17,
/// })?;
/// assert_eq!(game.snapshot().robots[0].shots_launched[0], 1);
/// # Ok::<(), rm_simulator_gameplay::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    config: Config,
    state: Snapshot,
}
impl Game {
    /// Build a game from an initial roster. Returns [`Error::Invalid`] when an
    /// id repeats or a robot's performance does not fit its kind.
    ///
    /// ```
    /// use rm_simulator_gameplay::{Config, Error, Game, HeroType, Performance, RobotConfig, RobotKind, Team};
    ///
    /// let infantry = RobotConfig {
    ///     id: 1,
    ///     team: Team::Red,
    ///     kind: RobotKind::Infantry,
    ///     performance: Performance::Hero(HeroType::MeleeFocused),
    /// };
    /// // Hero tables cannot describe an Infantry.
    /// let roster = Config {
    ///     robots: vec![infantry],
    ///     ..Config::default()
    /// };
    /// assert_eq!(Game::new(roster), Err(Error::Invalid));
    /// ```
    pub fn new(config: Config) -> Result<Self, Error> {
        let mut ids = std::collections::BTreeSet::new();
        if config
            .robots
            .iter()
            .any(|r| !r.performance.fits(r.kind) || !ids.insert(r.id))
        {
            return Err(Error::Invalid);
        }
        let mut state = Self::fresh_state(Policy::default());
        state.robots = config.robots.iter().map(Self::fresh_robot).collect();
        Ok(Self { config, state })
    }
    /// Current state. The reference is read-only and valid until the next
    /// command or step.
    pub fn snapshot(&self) -> &Snapshot {
        &self.state
    }
    /// Match format, initial roster and event retention the game was built with.
    pub fn config(&self) -> &Config {
        &self.config
    }
    /// Match-aware launch permission for adapters; robot-local locks alone
    /// do not include round phase. Idle practice allows any living robot to
    /// fire its caliber.
    ///
    /// ```
    /// use rm_simulator_gameplay::{
    ///     Caliber, Command, Config, Game, RobotConfig, RobotKind, Stats, Team, COUNTDOWN_TICKS,
    /// };
    ///
    /// # let fixed = Stats { max_hp: 400, chassis_power_w: 0, heat_limit: 100, cooling_per_s: 20 };
    /// # let mut game = Game::new(Config {
    /// #     robots: vec![RobotConfig::standard(4, Team::Red, RobotKind::Sentry, fixed)],
    /// #     ..Config::default()
    /// # })?;
    /// assert!(game.can_launch(4, Caliber::Mm17));
    /// game.command(Command::BeginCountdown)?;
    /// assert!(!game.can_launch(4, Caliber::Mm17));
    /// game.step(COUNTDOWN_TICKS)?;
    /// assert!(game.can_launch(4, Caliber::Mm17));
    /// game.command(Command::EndRound)?;
    /// assert!(!game.can_launch(4, Caliber::Mm17));
    /// # Ok::<(), rm_simulator_gameplay::Error>(())
    /// ```
    pub fn can_launch(&self, robot: u32, caliber: Caliber) -> bool {
        let Ok(i) = self.robot_index(robot) else {
            return false;
        };
        let r = &self.state.robots[i];
        match self.state.phase {
            Phase::Idle => r.alive() && r.config.kind.shoots(caliber),
            Phase::Running => r.can_launch(
                self.state.round_elapsed_ticks,
                caliber,
                self.state.policy.enforce_allowance && !self.reserve_covers(i, caliber),
            ),
            _ => false,
        }
    }
    /// Apply a projectile strike and report what it removed. Equivalent to
    /// [`Command::ProjectileHit`].
    ///
    /// ```
    /// use rm_simulator_gameplay::{
    ///     Applied, Caliber, Command, Config, Game, ProjectileHit, Target, Team, COUNTDOWN_TICKS,
    /// };
    ///
    /// let mut game = Game::new(Config::default())?;
    /// game.command(Command::BeginCountdown)?;
    /// game.step(COUNTDOWN_TICKS)?;
    /// let hit = ProjectileHit {
    ///     target: Target::Outpost(Team::Blue),
    ///     caliber: Caliber::Mm17,
    ///     shooter: None,
    ///     upper_front: false,
    ///     critical: true,
    /// };
    /// // Table 5-2's 20 HP, times 150 % in the centre square (section 5.5.1).
    /// assert_eq!(game.projectile_hit(hit)?, Applied { hp: 30, shield: 0 });
    /// # Ok::<(), rm_simulator_gameplay::Error>(())
    /// ```
    pub fn projectile_hit(&mut self, hit: ProjectileHit) -> Result<Applied, Error> {
        let applied = self.hit(hit)?;
        Ok(applied)
    }
    /// Apply damage with an explicit amount and report what it removed.
    /// Equivalent to [`Command::Damage`].
    pub fn damage(
        &mut self,
        target: Target,
        amount: u32,
        kind: DamageKind,
        attacker: Option<Team>,
    ) -> Result<Applied, Error> {
        self.accepts_damage()?;
        self.target_team(target)?;
        Ok(self.apply_damage(target, u64::from(amount) * 200, kind, attacker, None, None))
    }
    fn fresh_state(policy: Policy) -> Snapshot {
        Snapshot {
            tick: 0,
            phase: Phase::Idle,
            phase_elapsed_ticks: 0,
            round: 0,
            round_elapsed_ticks: 0,
            policy,
            teams: Team::BOTH.map(|team| TeamState {
                team,
                gold: 0,
                exchanged_allowance: [0; 2],
                base_hp: BASE_HP,
                base_shield_hp: BASE_SHIELD_HP,
                base_hp_lost: 0,
                base_armor_expanded: false,
                outpost_hp: OUTPOST_HP,
                outpost_ever_destroyed: false,
                outpost_first_destroyed_ticks: None,
                outpost_rebuild_opportunities: 0,
                attack_damage: 0,
                level_cap: 5,
                assembly_completions: [0; 4],
                assembly_income_per_10_s: 0,
                assembly_defense_pct: 0,
                rune_bonus_tenths: 0,
                rune_bonus_until_ticks: 0,
                fortress_reserve_used: 0,
            }),
            robots: Vec::new(),
            buffs: Vec::new(),
            observations: Vec::new(),
            result: None,
            rounds: Vec::new(),
            match_winner: None,
            recent_events: Vec::new(),
            next_event_id: 0,
            pending_deliveries: Vec::new(),
        }
    }
    fn fresh_robot(r: &RobotConfig) -> RobotState {
        RobotState {
            config: r.clone(),
            hp: r.performance.stats(1).max_hp,
            level: 1,
            experience_tenths: 0,
            allowance: match r.kind {
                RobotKind::Sentry => [300, 0],
                RobotKind::Drone => [750, 0],
                _ => [0; 2],
            },
            shots_launched: [0; 2],
            shots_over_allowance: [0; 2],
            heat_tenths: 0,
            overheated: false,
            heat_locked_for_round: false,
            speed_locked_until_ticks: 0,
            speed_locked_for_round: false,
            instant_respawns: 0,
            respawn: None,
            weakened: false,
            weakened_until_ticks: None,
            respawned_at_ticks: None,
            invincible_until_ticks: 0,
            irregularly_disconnected: false,
            ejected: false,
            out_of_combat: false,
            zones: Vec::new(),
            rebuild_progress_ticks: 0,
            sentry_resupply_claimed: 0,
            chassis_energy_j: matches!(
                r.kind,
                RobotKind::Hero | RobotKind::Infantry | RobotKind::Sentry
            )
            .then_some(20_000),
            air_support_ticks: if r.kind == RobotKind::Drone {
                30 * SECOND_TICKS
            } else {
                0
            },
            air_support_active: false,
            last_launch_ticks: [None; 2],
            defeated_at_ticks: None,
            launches_since_defeat: 0,
            mm42_suspended: false,
            crossing: None,
            crossing_defense_pct: 0,
            crossing_defense_until_ticks: 0,
            tunnel_cooling_until_ticks: 0,
            road_buff_ready_ticks: 0,
            fortress_capture_ticks: 0,
            fortress_capture_retained_until_ticks: None,
        }
    }
    fn emit(&mut self, kind: EventKind) {
        let state = &mut self.state;
        state.recent_events.push(Event {
            id: state.next_event_id,
            tick: state.tick,
            round_ticks: state.round_elapsed_ticks,
            kind,
        });
        state.next_event_id += 1;
        let excess = state
            .recent_events
            .len()
            .saturating_sub(self.config.event_memory);
        state.recent_events.drain(..excess);
    }
    fn phase(&mut self, phase: Phase) {
        self.state.phase = phase;
        self.state.phase_elapsed_ticks = 0;
        self.emit(EventKind::PhaseChanged(phase));
    }
    fn robot_index(&self, id: u32) -> Result<usize, Error> {
        self.state
            .robots
            .iter()
            .position(|r| r.config.id == id)
            .ok_or(Error::Robot)
    }
    fn running(&self) -> Result<(), Error> {
        if self.state.phase == Phase::Running {
            Ok(())
        } else {
            Err(Error::Phase)
        }
    }
    fn accepts_damage(&self) -> Result<(), Error> {
        if matches!(self.state.phase, Phase::Running | Phase::Idle) {
            Ok(())
        } else {
            Err(Error::Phase)
        }
    }
    fn eligible(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        r.alive() && !r.irregularly_disconnected && !r.weakened
    }
    fn own_service_zone(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        let team = r.config.team;
        r.in_zone(ZoneKind::Base, team)
            || r.in_zone(ZoneKind::Resupply, team)
            || self.in_occupiable_outpost_zone(i)
    }
    /// Section 5.5.3.6: the own living outpost's point, or within the first
    /// five minutes the opponent's destroyed outpost's point while the own
    /// outpost lives.
    fn in_occupiable_outpost_zone(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        let team = r.config.team;
        let own_alive = self.state.teams[team.index()].outpost_hp > 0;
        own_alive
            && (r.in_zone(ZoneKind::Outpost, team)
                || (r.in_zone(ZoneKind::Outpost, team.other())
                    && self.state.teams[team.other().index()].outpost_hp == 0
                    && self.state.round_elapsed_ticks < zones::OPPONENT_OUTPOST_ZONE_UNTIL_TICKS))
    }
    /// Hero, Infantry and Sentry, the robots that contest the central highland
    /// and the fortress (sections 5.5.3.3 and 5.5.3.9).
    fn contests_points(kind: RobotKind) -> bool {
        matches!(
            kind,
            RobotKind::Hero | RobotKind::Infantry | RobotKind::Sentry
        )
    }
    /// Section 5.5.3.3: robot `i` occupies a central highland point unless an
    /// eligible robot of the other team holds an earlier contact with it.
    fn occupies_central_highland(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        if !Self::contests_points(r.config.kind) || !self.eligible(i) {
            return false;
        }
        Team::BOTH.into_iter().any(|side| {
            let Some(since) = r.zone_since(ZoneKind::CentralHighland, side) else {
                return false;
            };
            !self.state.robots.iter().enumerate().any(|(j, other)| {
                other.config.team != r.config.team
                    && Self::contests_points(other.config.kind)
                    && self.eligible(j)
                    && other
                        .zone_since(ZoneKind::CentralHighland, side)
                        .is_some_and(|s| s < since)
            })
        })
    }
    /// Section 5.5.3.9: the one own robot that holds `team`'s fortress buff,
    /// the earliest eligible occupant once the team's outpost is destroyed.
    fn fortress_holder(&self, team: Team) -> Option<usize> {
        if self.state.teams[team.index()].outpost_hp > 0 {
            return None;
        }
        self.state
            .robots
            .iter()
            .enumerate()
            .filter(|(i, r)| {
                r.config.team == team && Self::contests_points(r.config.kind) && self.eligible(*i)
            })
            .filter_map(|(i, r)| {
                r.zone_since(ZoneKind::Fortress, team)
                    .map(|since| (since, r.config.id, i))
            })
            .min()
            .map(|(_, _, i)| i)
    }
    /// Section 5.5.3.9: whether robot `i` occupies the opponent's fortress,
    /// which is open from 3:00 once that opponent's outpost is destroyed and
    /// until its base armor is expanded.
    fn occupies_opponent_fortress(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        let owner = &self.state.teams[r.config.team.other().index()];
        Self::contests_points(r.config.kind)
            && self.eligible(i)
            && r.in_zone(ZoneKind::Fortress, owner.team)
            && self.state.round_elapsed_ticks >= zones::OPPONENT_FORTRESS_FROM_TICKS
            && owner.outpost_hp == 0
            && !owner.base_armor_expanded
    }
    /// Reserved fortress allowance units `team` has left (section 5.5.3.9).
    /// Assumption: the reserve is one pool per round, sized by the base HP
    /// lost so far.
    pub fn fortress_reserve_left(&self, team: Team) -> u32 {
        let t = &self.state.teams[team.index()];
        zones::fortress_reserve(t.base_hp_lost).saturating_sub(t.fortress_reserve_used)
    }
    /// Whether robot `i`'s next `caliber` launch draws on the fortress reserve.
    fn reserve_covers(&self, i: usize, caliber: Caliber) -> bool {
        let team = self.state.robots[i].config.team;
        let cost = if caliber == Caliber::Mm42 { 10 } else { 1 };
        self.fortress_holder(team) == Some(i) && self.fortress_reserve_left(team) >= cost
    }
    /// Defense and vulnerability robot `i` gains from the points it occupies
    /// and its terrain crossing buff (section 5.5.3).
    fn zone_effects(&self, i: usize) -> (u32, u32) {
        let now = self.state.round_elapsed_ticks;
        let r = &self.state.robots[i];
        let team = r.config.team;
        let mut defense = 0;
        if r.crossing_defense_until_ticks > now {
            defense = r.crossing_defense_pct;
        }
        if !self.eligible(i) || !r.config.kind.ground() {
            return (defense, 0);
        }
        if r.in_zone(ZoneKind::Base, team) {
            defense = defense.max(zones::BASE_ZONE_DEFENSE_PCT);
        }
        if r.in_zone(ZoneKind::TrapezoidHighland, team) {
            defense = defense.max(zones::TRAPEZOID_HIGHLAND_DEFENSE_PCT);
        }
        if self.in_occupiable_outpost_zone(i) {
            defense = defense.max(zones::OUTPOST_ZONE_DEFENSE_PCT);
        }
        if self.occupies_central_highland(i) {
            defense = defense.max(zones::CENTRAL_HIGHLAND_DEFENSE_PCT);
        }
        if self.fortress_holder(team) == Some(i) {
            defense = defense.max(zones::FORTRESS_DEFENSE_PCT);
        }
        let vulnerability = if self.occupies_opponent_fortress(i) {
            zones::FORTRESS_VULNERABILITY_PCT
        } else {
            0
        };
        (defense, vulnerability)
    }
    /// Heat cooling per second for robot `i` right now: the strongest
    /// multiplier, or the fortress's additive bonus when that is larger
    /// (sections 5.5.3.5 and 5.5.3.9).
    pub fn cooling_per_s(&self, robot: u32) -> u32 {
        let Ok(i) = self.robot_index(robot) else {
            return 0;
        };
        let now = self.state.round_elapsed_ticks;
        let r = &self.state.robots[i];
        let base = r.stats().cooling_per_s;
        let mut multiplier = self
            .buffs_on(Target::Robot(robot))
            .map(|b| b.cooling_multiplier)
            .max()
            .unwrap_or(1)
            .max(1);
        if r.tunnel_cooling_until_ticks > now {
            multiplier = multiplier.max(zones::TUNNEL_COOLING_MULTIPLIER);
        }
        let team = r.config.team;
        let bonus = if self.fortress_holder(team) == Some(i) {
            zones::fortress_cooling_bonus(self.state.teams[team.index()].base_hp_lost)
        } else {
            0
        };
        base.saturating_mul(multiplier)
            .max(base.saturating_add(bonus))
    }
    /// Advance terrain crossing progress for a new contact (section 5.5.3.5).
    /// Any other card detected along the way abandons the crossing.
    fn cross(&mut self, i: usize, zone: Zone) {
        let now = self.state.round_elapsed_ticks;
        let eligible = self.eligible(i) && self.state.robots[i].config.kind.ground();
        let r = &mut self.state.robots[i];
        let Some((course, position)) = zones::course_position(zone.kind, zone.pad) else {
            // Assumption: the central highland shares the cards of the higher
            // Elevated Ground pads (section 5.5.3.3), so it never interrupts.
            let shared = zone.kind == ZoneKind::CentralHighland
                && r.crossing
                    .is_some_and(|p| p.zone.kind == ZoneKind::ElevatedCrossing);
            if !shared {
                r.crossing = None;
            }
            return;
        };
        if !eligible {
            r.crossing = None;
            return;
        }
        let def = zones::courses(zone.kind)[usize::from(course)];
        let last = def.pads.len() - 1;
        if let Some(p) = r.crossing
            && p.zone.kind == zone.kind
            && p.zone.owner == zone.owner
            && p.course == course
            && now <= p.started_ticks + def.window_ticks
        {
            let done = usize::from(p.done);
            let expected = if p.reversed { last - done } else { done };
            if position == expected {
                if done == last {
                    r.crossing = None;
                    self.grant_crossing(i, zone.kind, def);
                } else {
                    r.crossing = Some(CrossingProgress {
                        done: p.done + 1,
                        ..p
                    });
                }
                return;
            }
            let visited = if p.reversed {
                position > last - done
            } else {
                position < done
            };
            if visited {
                return;
            }
        }
        let start = |reversed| CrossingProgress {
            zone,
            course,
            done: 1,
            reversed,
            started_ticks: now,
        };
        r.crossing = if position == 0 {
            Some(start(false))
        } else if def.reversible && position == last {
            Some(start(true))
        } else {
            None
        };
    }
    fn grant_crossing(&mut self, i: usize, kind: ZoneKind, def: zones::Course) {
        let now = self.state.round_elapsed_ticks;
        let r = &mut self.state.robots[i];
        if kind == ZoneKind::Road {
            if now < r.road_buff_ready_ticks {
                return;
            }
            r.road_buff_ready_ticks = now + zones::ROAD_BUFF_COOLDOWN_TICKS;
        }
        // Section 5.5.3.5: a repeated Launch Ramp, Elevated Ground or Road
        // buff while one lasts gives 50 %, for the longer remaining time.
        let active = r.crossing_defense_until_ticks > now;
        let pct = if active && kind != ZoneKind::Tunnel {
            zones::CROSSING_STACKED_DEFENSE_PCT
        } else {
            def.defense_pct
        };
        r.crossing_defense_pct = if active {
            r.crossing_defense_pct.max(pct)
        } else {
            pct
        };
        r.crossing_defense_until_ticks =
            r.crossing_defense_until_ticks.max(now + def.defense_ticks);
        if kind == ZoneKind::Tunnel {
            r.tunnel_cooling_until_ticks = r
                .tunnel_cooling_until_ticks
                .max(now + zones::TUNNEL_COOLING_TICKS);
        }
        let robot = r.config.id;
        self.emit(EventKind::TerrainCrossing { robot, kind });
    }
    /// Section 5.5.3.9: advance each robot's opponent fortress capture timer
    /// and expand the owner's base armor after 20 s.
    fn tick_fortress(&mut self, now: u64) {
        for i in 0..self.state.robots.len() {
            let occupying = self.occupies_opponent_fortress(i);
            let r = &mut self.state.robots[i];
            if occupying {
                r.fortress_capture_ticks += 1;
                r.fortress_capture_retained_until_ticks = None;
                if r.fortress_capture_ticks >= zones::FORTRESS_CAPTURE_TICKS {
                    let owner = r.config.team.other();
                    self.state.teams[owner.index()].base_armor_expanded = true;
                    for r in &mut self.state.robots {
                        if r.config.team != owner {
                            r.fortress_capture_ticks = 0;
                            r.fortress_capture_retained_until_ticks = None;
                        }
                    }
                    self.emit(EventKind::BaseArmorExpanded(owner));
                }
            } else if r.fortress_capture_ticks > 0 {
                match r.fortress_capture_retained_until_ticks {
                    None => {
                        r.fortress_capture_retained_until_ticks =
                            Some(now + zones::FORTRESS_CAPTURE_RETAIN_TICKS)
                    }
                    Some(until) if now >= until => {
                        r.fortress_capture_ticks = 0;
                        r.fortress_capture_retained_until_ticks = None;
                    }
                    Some(_) => {}
                }
            }
        }
    }
    fn spend(&mut self, team: Team, cost: u32) -> Result<(), Error> {
        let gold = &mut self.state.teams[team.index()].gold;
        *gold = gold.checked_sub(cost).ok_or(Error::Insufficient)?;
        Ok(())
    }
    fn grant(&mut self, team: Team, amount: u32) {
        self.state.teams[team.index()].gold =
            self.state.teams[team.index()].gold.saturating_add(amount);
        self.emit(EventKind::GoldGranted { team, amount });
    }
    /// Apply one command and commit it atomically. Rejection is atomic,
    /// including gold, pending deliveries and event ids.
    ///
    /// ```
    /// use rm_simulator_gameplay::{
    ///     Caliber, Command, Config, Error, Game, Performance, RobotConfig, RobotKind, Team,
    ///     COUNTDOWN_TICKS,
    /// };
    ///
    /// # let mut game = Game::new(Config {
    /// #     robots: vec![RobotConfig {
    /// #         id: 1,
    /// #         team: Team::Red,
    /// #         kind: RobotKind::Infantry,
    /// #         performance: Performance::default_for(RobotKind::Infantry).unwrap(),
    /// #     }],
    /// #     ..Config::default()
    /// # })?;
    /// # game.command(Command::BeginCountdown)?;
    /// # game.step(COUNTDOWN_TICKS)?;
    /// let before = game.snapshot().clone();
    /// // An infantry cannot fire 42 mm, so the launch is rejected.
    /// let rejected = game.command(Command::Launch {
    ///     robot: 1,
    ///     caliber: Caliber::Mm42,
    /// });
    /// assert_eq!(rejected, Err(Error::Ineligible));
    /// assert_eq!(game.snapshot(), &before);
    /// # Ok::<(), Error>(())
    /// ```
    pub fn command(&mut self, command: Command) -> Result<(), Error> {
        // These handlers validate everything before their first mutation.
        // Keep their implementation in `apply` so both paths use the same rules.
        if matches!(
            command,
            Command::CombatState { .. }
                | Command::Disconnection { .. }
                | Command::Launch { .. }
                | Command::ObserveLaunch { .. }
                | Command::ProjectileHit(_)
        ) {
            return self.apply(command);
        }
        let mut next = self.clone();
        next.apply(command)?;
        *self = next;
        Ok(())
    }
    fn apply(&mut self, command: Command) -> Result<(), Error> {
        match command {
            Command::BeginRound | Command::BeginCountdown => {
                if !matches!(self.state.phase, Phase::Idle | Phase::Confirmed) {
                    return Err(Error::Phase);
                }
                let fresh = Self::fresh_state(self.state.policy);
                self.state.teams = fresh.teams;
                self.state.robots = self
                    .state
                    .robots
                    .iter()
                    .map(|r| Self::fresh_robot(&r.config))
                    .collect();
                self.state.buffs.clear();
                self.state.observations.clear();
                self.state.pending_deliveries.clear();
                self.state.round = self.state.round.checked_add(1).ok_or(Error::Invalid)?;
                self.state.round_elapsed_ticks = 0;
                self.state.result = None;
                self.phase(if command == Command::BeginRound {
                    Phase::Setup
                } else {
                    Phase::Countdown
                });
            }
            Command::ResetMatch => {
                let tick = self.state.tick;
                let mut fresh = Self::fresh_state(self.state.policy);
                fresh.robots = self
                    .state
                    .robots
                    .iter()
                    .map(|r| Self::fresh_robot(&r.config))
                    .collect();
                fresh.tick = tick;
                self.state = fresh;
            }
            Command::EndRound => {
                self.running()?;
                self.finish();
            }
            Command::Adjudicate { winner } => {
                if self.state.phase != Phase::RoundEnded {
                    return Err(Error::Phase);
                }
                self.state.result = Some(RoundResult::Decided {
                    winner,
                    reason: DecisionReason::Referee,
                });
            }
            Command::ConfirmResult => {
                if self.state.phase != Phase::RoundEnded {
                    return Err(Error::Phase);
                }
                let Some(result @ RoundResult::Decided { .. }) = self.state.result else {
                    return Err(Error::Ineligible);
                };
                self.state.rounds.push(RoundRecord {
                    round: self.state.round,
                    elapsed_ticks: self.state.round_elapsed_ticks,
                    result,
                });
                let mut wins = [0; 2];
                for r in &self.state.rounds {
                    if let RoundResult::Decided {
                        winner: Some(team), ..
                    } = r.result
                    {
                        wins[team.index()] += 1;
                    }
                }
                let done = match self.config.format {
                    MatchFormat::Bo2 => self.state.rounds.len() >= 2,
                    MatchFormat::Bo3 => wins.contains(&2),
                    MatchFormat::Bo5 => wins.contains(&3),
                };
                if done {
                    self.state.match_winner = match wins[0].cmp(&wins[1]) {
                        std::cmp::Ordering::Greater => Some(Team::Red),
                        std::cmp::Ordering::Less => Some(Team::Blue),
                        _ => None,
                    };
                    self.phase(Phase::MatchEnded);
                } else {
                    self.phase(Phase::Confirmed);
                }
            }
            Command::Observe(mut observation) => {
                if let Some(id) = observation.robot {
                    let i = self.robot_index(id)?;
                    if observation
                        .team
                        .is_some_and(|team| team != self.state.robots[i].config.team)
                    {
                        return Err(Error::Invalid);
                    }
                }
                if observation.state.is_empty()
                    || observation.state.len() > 4096
                    || observation.measurements.len() > 64
                    || observation
                        .measurements
                        .iter()
                        .any(|(k, v)| k.len() > 128 || v.len() > 4096)
                {
                    return Err(Error::Invalid);
                }
                observation.observed_at_ticks = self.state.round_elapsed_ticks;
                self.state.observations.retain(|o| {
                    (o.mechanic, o.team, o.robot)
                        != (observation.mechanic, observation.team, observation.robot)
                });
                self.state.observations.push(observation);
            }
            Command::AddRobot(config) => {
                if !config.performance.fits(config.kind) || self.robot_index(config.id).is_ok() {
                    return Err(Error::Invalid);
                }
                let id = config.id;
                self.state.robots.push(Self::fresh_robot(&config));
                self.emit(EventKind::RobotAdded(id));
            }
            Command::RemoveRobot { robot } => {
                let i = self.robot_index(robot)?;
                self.state.robots.remove(i);
                self.state.pending_deliveries.retain(|d| d.robot != robot);
                self.state
                    .buffs
                    .retain(|b| b.target != BuffTarget::Robot(robot));
                self.emit(EventKind::RobotRemoved(robot));
            }
            Command::SetPerformance { robot, performance } => {
                let i = self.robot_index(robot)?;
                if self.state.phase == Phase::Running {
                    return Err(Error::Phase);
                }
                if !performance.fits(self.state.robots[i].config.kind) {
                    return Err(Error::Invalid);
                }
                let r = &mut self.state.robots[i];
                r.config.performance = performance;
                if r.alive() {
                    r.hp = r.stats().max_hp;
                }
            }
            Command::SetPolicy(policy) => self.state.policy = policy,
            Command::ClearBuff { target, source } => self
                .state
                .buffs
                .retain(|b| (b.target, b.source) != (target, source)),
            Command::ProjectileHit(hit) => {
                self.hit(hit)?;
            }
            Command::Damage {
                target,
                amount,
                kind,
                attacker,
            } => {
                self.damage(target, amount, kind, attacker)?;
            }
            Command::Launch { robot, caliber } if self.state.phase == Phase::Idle => {
                let i = self.robot_index(robot)?;
                let r = &self.state.robots[i];
                if !r.alive() || !r.config.kind.shoots(caliber) {
                    return Err(Error::Ineligible);
                }
            }
            Command::ObserveLaunch { robot, caliber } if self.state.phase == Phase::Idle => {
                let i = self.robot_index(robot)?;
                if !self.state.robots[i].config.kind.shoots(caliber) {
                    return Err(Error::Invalid);
                }
            }
            Command::SetRobotHp { robot, hp } => {
                let i = self.robot_index(robot)?;
                let r = &self.state.robots[i];
                let max = r.stats().max_hp;
                if hp == 0 {
                    if r.alive() {
                        // An operator defeat bypasses defenses and invincibility
                        // and gives nobody experience or attack credit.
                        let scaled = u64::from(r.hp) * 200;
                        self.apply_damage(
                            Target::Robot(robot),
                            scaled,
                            DamageKind::Disconnection,
                            None,
                            None,
                            None,
                        );
                    }
                } else if self.state.robots[i].alive() {
                    self.state.robots[i].hp = hp.min(max);
                } else {
                    self.revive(i, hp.min(max));
                }
            }
            Command::Revive { robot } => {
                let i = self.robot_index(robot)?;
                let max = self.state.robots[i].stats().max_hp;
                self.revive(i, max);
            }
            Command::ClearWeakened { robot } => {
                let i = self.robot_index(robot)?;
                if self.state.robots[i].weakened {
                    self.clear_weakness(i);
                }
            }
            Command::SetBase {
                team,
                hp,
                shield_hp,
            } => {
                if hp > BASE_HP || shield_hp > BASE_HP {
                    return Err(Error::Invalid);
                }
                let t = &mut self.state.teams[team.index()];
                t.base_hp = hp;
                t.base_shield_hp = shield_hp;
                if hp == 0 && self.state.phase == Phase::Running {
                    self.finish();
                }
            }
            Command::SetOutpostHp { team, hp } => {
                if hp > OUTPOST_HP {
                    return Err(Error::Invalid);
                }
                let now = self.state.round_elapsed_ticks;
                let running = self.state.phase == Phase::Running;
                let t = &mut self.state.teams[team.index()];
                t.outpost_hp = hp;
                if hp == 0 && running {
                    t.outpost_ever_destroyed = true;
                    t.outpost_first_destroyed_ticks.get_or_insert(now);
                }
            }
            Command::SetGold { team, gold } => self.state.teams[team.index()].gold = gold,
            Command::SetAllowance { robot, allowance } => {
                let i = self.robot_index(robot)?;
                self.state.robots[i].allowance = allowance;
            }
            other => {
                self.running()?;
                self.gameplay_command(other)?;
            }
        }
        Ok(())
    }
    fn gameplay_command(&mut self, command: Command) -> Result<(), Error> {
        let now = self.state.round_elapsed_ticks;
        match command {
            Command::RuneActivated {
                team,
                stage,
                attack_pct,
                defense_pct,
                cooling_multiplier,
                duration_ticks,
            } => {
                if duration_ticks == 0
                    || attack_pct > 1000
                    || defense_pct > 100
                    || cooling_multiplier > 100
                {
                    return Err(Error::Invalid);
                }
                let expires_ticks = now.checked_add(duration_ticks).ok_or(Error::Invalid)?;
                let target = BuffTarget::Team(team);
                self.state
                    .buffs
                    .retain(|b| (b.target, b.source) != (target, coverage::Mechanic::Rune));
                self.state.buffs.push(Buff {
                    target,
                    source: coverage::Mechanic::Rune,
                    attack_pct,
                    defense_pct,
                    vulnerability_pct: 0,
                    cooling_multiplier,
                    expires_ticks,
                });
                match stage {
                    RuneStage::Small => {
                        let t = &mut self.state.teams[team.index()];
                        t.rune_bonus_tenths = SMALL_RUNE_BONUS_TENTHS;
                        t.rune_bonus_until_ticks = expires_ticks;
                    }
                    RuneStage::Large => self.share_experience(
                        |r| r.config.team == team && r.alive() && r.config.kind.levels(),
                        u64::from(LARGE_RUNE_TENTHS),
                    ),
                }
            }
            Command::SkipTo { round_ticks } => {
                if round_ticks < now || round_ticks > ROUND_TICKS {
                    return Err(Error::Invalid);
                }
                while self.state.phase == Phase::Running
                    && self.state.round_elapsed_ticks < round_ticks
                {
                    self.state.round_elapsed_ticks += 1;
                    self.state.phase_elapsed_ticks += 1;
                    if self.state.round_elapsed_ticks >= ROUND_TICKS {
                        self.finish();
                    } else {
                        self.tick_round();
                    }
                }
            }
            Command::ZoneDetection {
                robot,
                zone,
                detected,
            } => {
                let i = self.robot_index(robot)?;
                if zone.pad != 0 && zones::course_position(zone.kind, zone.pad).is_none() {
                    return Err(Error::Invalid);
                }
                let zones = &mut self.state.robots[i].zones;
                let arrived = detected && !zones.iter().any(|c| c.zone == zone);
                if let Some(contact) = zones.iter_mut().find(|c| c.zone == zone) {
                    contact.detected = detected;
                    // Section 5.5.3.1: two-second occupation expiry delay. Repeated
                    // false samples must not extend the original deadline.
                    if detected {
                        contact.expires_ticks = None;
                    } else if contact.expires_ticks.is_none() {
                        contact.expires_ticks = Some(now + zones::ZONE_EXPIRY_TICKS);
                    }
                } else if detected {
                    zones.push(ZoneContact {
                        zone,
                        detected: true,
                        expires_ticks: None,
                        since_ticks: now,
                    });
                }
                if arrived {
                    self.cross(i, zone);
                }
                self.update_weakness(i);
            }
            Command::CombatState {
                robot,
                out_of_combat,
            } => {
                let i = self.robot_index(robot)?;
                self.state.robots[i].out_of_combat = out_of_combat;
            }
            Command::Disconnection { robot, irregular } => {
                let i = self.robot_index(robot)?;
                self.state.robots[i].irregularly_disconnected = irregular;
            }
            Command::Eject { robot } => {
                let i = self.robot_index(robot)?;
                let team = self.state.robots[i].config.team;
                self.apply_damage(
                    Target::Robot(robot),
                    u64::from(u32::MAX) * 200,
                    DamageKind::Penalty,
                    Some(team.other()),
                    None,
                    None,
                );
                self.state.robots[i].ejected = true;
                self.state.robots[i].respawn = None;
            }
            Command::ExchangeAmmo {
                robot,
                caliber,
                amount,
                remote,
            } => {
                let i = self.robot_index(robot)?;
                let r = &self.state.robots[i];
                if !self.eligible(i)
                    || !r.config.kind.shoots(caliber)
                    || r.config.kind == RobotKind::Drone
                    || (remote && !r.out_of_combat)
                    || (!remote
                        && self.state.policy.exchange_requires_zone
                        && !self.own_service_zone(i))
                {
                    return Err(Error::Ineligible);
                }
                let team = r.config.team;
                let (unit, cost, limit) = match (caliber, remote) {
                    (Caliber::Mm17, false) => (10, 10, 1000),
                    (Caliber::Mm17, true) => (100, 150, 1000),
                    (Caliber::Mm42, false) => (1, 10, 100),
                    (Caliber::Mm42, true) => (10, 150, 100),
                };
                if amount == 0 || !amount.is_multiple_of(unit) {
                    return Err(Error::Invalid);
                }
                let count = self.state.teams[team.index()].exchanged_allowance[caliber.index()]
                    .checked_add(amount)
                    .ok_or(Error::Limit)?;
                if count > limit {
                    return Err(Error::Limit);
                }
                self.spend(
                    team,
                    (amount / unit).checked_mul(cost).ok_or(Error::Invalid)?,
                )?;
                self.state.teams[team.index()].exchanged_allowance[caliber.index()] = count;
                if remote {
                    self.state.pending_deliveries.push(Delivery {
                        at_ticks: now + 6 * SECOND_TICKS,
                        robot,
                        kind: DeliveryKind::Ammo(caliber, amount),
                    });
                } else {
                    self.state.robots[i].allowance[caliber.index()] =
                        self.state.robots[i].allowance[caliber.index()].saturating_add(amount);
                }
            }
            Command::ExchangeHp { robot } => {
                let i = self.robot_index(robot)?;
                let r = &self.state.robots[i];
                if !self.eligible(i)
                    || !matches!(
                        r.config.kind,
                        RobotKind::Hero | RobotKind::Infantry | RobotKind::Sentry
                    )
                    || !r.out_of_combat
                {
                    return Err(Error::Ineligible);
                }
                // Table 5-6: 50 + ceil(elapsed seconds / 60 * 20).
                self.spend(
                    r.config.team,
                    50 + (now * 20).div_ceil(60 * SECOND_TICKS) as u32,
                )?;
                self.state.pending_deliveries.push(Delivery {
                    at_ticks: now + 6 * SECOND_TICKS,
                    robot,
                    kind: DeliveryKind::Hp,
                });
            }
            Command::InstantRespawn { robot } => {
                let i = self.robot_index(robot)?;
                let r = &self.state.robots[i];
                if r.alive() || !r.config.kind.ground() || r.ejected || r.irregularly_disconnected {
                    return Err(Error::Ineligible);
                }
                self.spend(r.config.team, self.instant_respawn_cost(i))?;
                self.state.robots[i].instant_respawns += 1;
                self.respawn(i, true);
            }
            Command::Launch { robot, caliber } => {
                let i = self.robot_index(robot)?;
                if !self.state.robots[i].can_launch(
                    now,
                    caliber,
                    self.state.policy.enforce_allowance && !self.reserve_covers(i, caliber),
                ) {
                    return Err(Error::Ineligible);
                }
                self.record_launch(i, caliber);
            }
            Command::ObserveLaunch { robot, caliber } => {
                let i = self.robot_index(robot)?;
                if !self.state.robots[i].config.kind.shoots(caliber) {
                    return Err(Error::Invalid);
                }
                self.record_launch(i, caliber);
            }
            Command::LaunchSpeed {
                robot,
                caliber,
                measured_mm_s,
                limit_mm_s,
                deployed,
            } => {
                let i = self.robot_index(robot)?;
                if limit_mm_s == 0
                    || !self.state.robots[i].config.kind.shoots(caliber)
                    || (deployed && caliber != Caliber::Mm42)
                {
                    return Err(Error::Invalid);
                }
                if measured_mm_s > limit_mm_s {
                    let m = u64::from(measured_mm_s);
                    let l = u64::from(limit_mm_s);
                    let seconds = match caliber {
                        Caliber::Mm17 if m - l < 5000 => 15,
                        Caliber::Mm17 if m - l < 10000 => 20,
                        Caliber::Mm17 => 0,
                        Caliber::Mm42 if deployed && m <= 18000 => 15,
                        Caliber::Mm42 if deployed => 0,
                        Caliber::Mm42 if m * 10 <= l * 11 => 15,
                        Caliber::Mm42 if m * 10 <= l * 12 => 20,
                        _ => 0,
                    };
                    let r = &mut self.state.robots[i];
                    if seconds == 0 {
                        r.speed_locked_for_round = true;
                    } else {
                        r.speed_locked_until_ticks =
                            r.speed_locked_until_ticks.max(now + seconds * SECOND_TICKS);
                    }
                }
            }
            Command::AwardExperience { robot, tenths } => {
                let i = self.robot_index(robot)?;
                self.award_experience(i, tenths);
            }
            Command::AssemblyCompleted { team, level } => self.assembly(team, level)?,
            Command::ApplyBuff(buff) => {
                if let BuffTarget::Robot(id) = buff.target {
                    self.robot_index(id)?;
                }
                if buff.expires_ticks <= now
                    || buff.defense_pct > 100
                    || buff.attack_pct > 1000
                    || buff.vulnerability_pct > 1000
                    || buff.cooling_multiplier > 100
                {
                    return Err(Error::Invalid);
                }
                self.state
                    .buffs
                    .retain(|b| (b.target, b.source) != (buff.target, buff.source));
                self.state.buffs.push(buff);
            }
            Command::AirSupport { robot, active } => {
                let i = self.robot_index(robot)?;
                let r = &self.state.robots[i];
                if r.config.kind != RobotKind::Drone || !self.eligible(i) {
                    return Err(Error::Ineligible);
                }
                if active
                    && r.air_support_ticks == 0
                    && self.state.teams[r.config.team.index()].gold == 0
                {
                    return Err(Error::Insufficient);
                }
                self.state.robots[i].air_support_active = active;
            }
            Command::Energy {
                robot,
                consumed_j,
                recharge_surplus_j,
            } => {
                let i = self.robot_index(robot)?;
                let r = &mut self.state.robots[i];
                if recharge_surplus_j > 0
                    && (!r.in_zone(ZoneKind::Resupply, r.config.team) || !r.alive() || r.weakened)
                {
                    return Err(Error::Ineligible);
                }
                let energy = r.chassis_energy_j.as_mut().ok_or(Error::Ineligible)?;
                *energy = energy
                    .saturating_sub(consumed_j)
                    .saturating_add(recharge_surplus_j.saturating_mul(8))
                    .min(40_000);
            }
            _ => return Err(Error::Phase),
        }
        Ok(())
    }
    /// Table 5-6: ceil(elapsed seconds / 60) x 80 + level x 20 gold.
    ///
    /// Rounds up on whole elapsed round ticks, so the first second costs one
    /// minute's price.
    pub fn instant_respawn_cost_for(&self, robot: u32) -> Option<u32> {
        self.robot_index(robot)
            .ok()
            .map(|i| self.instant_respawn_cost(i))
    }
    fn instant_respawn_cost(&self, i: usize) -> u32 {
        let now = self.state.round_elapsed_ticks;
        now.div_ceil(60 * SECOND_TICKS) as u32 * 80 + u32::from(self.state.robots[i].level) * 20
    }
    fn record_launch(&mut self, i: usize, caliber: Caliber) {
        // Sections 5.1.3, 5.3.2 and 5.4.1: sensor-detected launches count
        // even if a physical launcher fired while locked or without allowance.
        let now = self.state.round_elapsed_ticks;
        let enforce = self.state.policy.enforce_allowance;
        let heat_limit = self.state.robots[i].stats().heat_limit;
        let reserve = self.state.phase == Phase::Running && self.reserve_covers(i, caliber);
        if reserve {
            // Section 5.5.3.9: the fortress holder spends reserved allowance
            // first, 1 unit per 17 mm and 10 per 42 mm projectile.
            let team = self.state.robots[i].config.team;
            self.state.teams[team.index()].fortress_reserve_used +=
                if caliber == Caliber::Mm42 { 10 } else { 1 };
        }
        let r = &mut self.state.robots[i];
        let index = caliber.index();
        let over = if reserve {
            r.shots_launched[index] = r.shots_launched[index].saturating_add(1);
            false
        } else {
            record_ammo_launch(&mut r.allowance[index], &mut r.shots_launched[index])
        };
        if over {
            r.shots_over_allowance[index] = r.shots_over_allowance[index].saturating_add(1);
        }
        r.last_launch_ticks[index] = Some(now);
        if caliber == Caliber::Mm42 && r.config.kind == RobotKind::Hero {
            // Section 5.3.2: over allowance, or a third launch after defeat,
            // suspends the team's 42 mm damage.
            if !r.alive() {
                r.launches_since_defeat = r.launches_since_defeat.saturating_add(1);
                if r.launches_since_defeat >= 3 {
                    r.mm42_suspended = true;
                }
            }
            if over && enforce {
                r.mm42_suspended = true;
            }
        }
        r.heat_tenths = r.heat_tenths.saturating_add(caliber.launch_heat_tenths());
        let extra = if caliber == Caliber::Mm17 { 100 } else { 200 };
        // Figure 5-1 uses >= Q2, unlike the neighboring prose's > Q2.
        if r.heat_tenths >= (u64::from(heat_limit) + extra) * 10 {
            r.heat_locked_for_round = true;
        }
        if r.heat_tenths > u64::from(heat_limit) * 10 {
            r.overheated = true;
        }
        let xp = if r.config.kind == RobotKind::Hero {
            100
        } else {
            10
        };
        self.award_experience(i, xp);
    }
    fn target_team(&self, target: Target) -> Result<Team, Error> {
        Ok(match target {
            Target::Base(t) | Target::Outpost(t) => t,
            Target::Robot(id) => self.state.robots[self.robot_index(id)?].config.team,
        })
    }
    /// Buffs that apply to `target`: its own, plus its team's.
    fn buffs_on(&self, target: Target) -> impl Iterator<Item = &Buff> + '_ {
        let (own, team) = match target {
            Target::Robot(id) => (
                BuffTarget::Robot(id),
                self.robot_index(id)
                    .ok()
                    .map(|i| self.state.robots[i].config.team),
            ),
            Target::Base(t) => (BuffTarget::Base(t), Some(t)),
            Target::Outpost(t) => (BuffTarget::Outpost(t), Some(t)),
        };
        self.state.buffs.iter().filter(move |b| {
            b.target == own || team.is_some_and(|t| b.target == BuffTarget::Team(t))
        })
    }
    /// The strongest attack multiplier, in percent, on a robot's projectiles
    /// (section 5.5.3.1); 100 without an attack buff.
    ///
    /// ```
    /// use rm_simulator_gameplay::{Command, Config, Game, RobotConfig, RobotKind, RuneStage, Stats, Team, COUNTDOWN_TICKS};
    ///
    /// # let fixed = Stats { max_hp: 400, chassis_power_w: 0, heat_limit: 100, cooling_per_s: 20 };
    /// let mut game = Game::new(Config {
    ///     robots: vec![RobotConfig::standard(1, Team::Red, RobotKind::Infantry, fixed)],
    ///     ..Config::default()
    /// })?;
    /// game.command(Command::BeginCountdown)?;
    /// game.step(COUNTDOWN_TICKS)?;
    /// assert_eq!(game.attack_pct(1), 100);
    /// game.command(Command::RuneActivated {
    ///     team: Team::Red,
    ///     stage: RuneStage::Large,
    ///     attack_pct: 300,
    ///     defense_pct: 50,
    ///     cooling_multiplier: 5,
    ///     duration_ticks: 60_000,
    /// })?;
    /// assert_eq!(game.attack_pct(1), 300);
    /// # Ok::<(), rm_simulator_gameplay::Error>(())
    /// ```
    pub fn attack_pct(&self, robot: u32) -> u32 {
        self.buffs_on(Target::Robot(robot))
            .map(|b| b.attack_pct)
            .fold(100, u32::max)
    }
    /// Defense and vulnerability percentages protecting `target` against
    /// projectile or collision damage right now (section 5.5.3.1): the
    /// strongest of each, independently.
    pub fn defense_pct(&self, target: Target) -> (u32, u32) {
        let Ok(team) = self.target_team(target) else {
            return (0, 0);
        };
        let mut defense = self.state.teams[team.index()].assembly_defense_pct;
        let mut vulnerability = 0;
        for buff in self.buffs_on(target) {
            defense = defense.max(buff.defense_pct);
            vulnerability = vulnerability.max(buff.vulnerability_pct);
        }
        if let Target::Robot(id) = target
            && let Ok(i) = self.robot_index(id)
        {
            let (zone_defense, zone_vulnerability) = self.zone_effects(i);
            defense = defense.max(zone_defense);
            vulnerability = vulnerability.max(zone_vulnerability);
        }
        (defense.min(100), vulnerability)
    }
    /// Whether 42 mm projectiles from `attacking` currently damage the other
    /// team: a Hero of that team fired 42 mm within four seconds (section
    /// 5.1.1) and has not been defeated for three seconds or launched in a
    /// way that suspends it (section 5.3.2). Always true in Idle practice.
    pub fn mm42_effective(&self, attacking: Team) -> bool {
        if self.state.phase == Phase::Idle {
            return true;
        }
        let now = self.state.round_elapsed_ticks;
        self.state.robots.iter().any(|h| {
            h.config.team == attacking
                && h.config.kind == RobotKind::Hero
                && h.last_launch_ticks[Caliber::Mm42.index()]
                    .is_some_and(|t| now < t.saturating_add(MM42_IDLE_TICKS))
                && !h.mm42_suspended
                && h.defeated_at_ticks
                    .is_none_or(|d| h.alive() || now < d.saturating_add(MM42_DEFEAT_TICKS))
        })
    }
    fn hit(&mut self, hit: ProjectileHit) -> Result<Applied, Error> {
        self.accepts_damage()?;
        let team = self.target_team(hit.target)?;
        if (hit.upper_front && !matches!(hit.target, Target::Base(_)))
            || (hit.critical && matches!(hit.target, Target::Robot(_)))
        {
            return Err(Error::Invalid);
        }
        // All validation is above; nothing below can fail.
        let shooter = hit.shooter.and_then(|id| self.robot_index(id).ok());
        let attacking = shooter.map_or(team.other(), |i| self.state.robots[i].config.team);
        if hit.caliber == Caliber::Mm42 && attacking != team && !self.mm42_effective(attacking) {
            return Ok(Applied::default());
        }
        // Table 5-2.
        let raw: u64 = match (hit.target, hit.caliber) {
            (_, Caliber::Mm42) => 200,
            (Target::Base(_), Caliber::Mm17) if hit.upper_front => 5,
            (_, Caliber::Mm17) => 20,
        };
        let attack = match (shooter, self.state.phase) {
            (Some(i), Phase::Running) => u64::from(self.attack_pct(self.state.robots[i].config.id)),
            _ => 100,
        };
        // Damage in units of 1/200 HP: the attack percent and the section
        // 5.5.1 150 % centre square scale it before rounding once.
        let scaled = raw * attack * if hit.critical { 3 } else { 2 };
        let credited = Some(attacking);
        Ok(self.apply_damage(
            hit.target,
            scaled,
            DamageKind::Projectile,
            credited,
            shooter,
            Some(hit.caliber),
        ))
    }
    /// `scaled` is the damage in units of 1/200 HP before target defenses.
    fn apply_damage(
        &mut self,
        target: Target,
        scaled: u64,
        kind: DamageKind,
        attacker: Option<Team>,
        shooter: Option<usize>,
        caliber: Option<Caliber>,
    ) -> Applied {
        let practice = self.state.phase == Phase::Idle;
        let Ok(team) = self.target_team(target) else {
            return Applied::default();
        };
        let now = self.state.round_elapsed_ticks;
        if let Target::Robot(id) = target {
            let Ok(i) = self.robot_index(id) else {
                return Applied::default();
            };
            let r = &self.state.robots[i];
            if !r.alive()
                || (matches!(
                    kind,
                    DamageKind::Projectile | DamageKind::Dart | DamageKind::Collision
                ) && r.invincible_until_ticks > now)
            {
                return Applied::default();
            }
        }
        if !practice
            && matches!(target, Target::Base(_))
            && self.state.teams[team.index()].outpost_hp > 0
            && matches!(
                kind,
                DamageKind::Projectile | DamageKind::Dart | DamageKind::Collision
            )
        {
            return Applied::default();
        }
        let (defense, vulnerability) =
            if matches!(kind, DamageKind::Projectile | DamageKind::Collision) {
                self.defense_pct(target)
            } else {
                (0, 0)
            };
        let factor = 100 - u64::from(defense) + u64::from(vulnerability);
        let amount = (scaled.saturating_mul(factor) + 10_000) / 20_000;
        let amount = amount.min(u64::from(u32::MAX)) as u32;
        let shooter_id = shooter.map(|i| self.state.robots[i].config.id);
        let mut shield = 0;
        let mut defeated = None;
        let removed = match target {
            Target::Robot(id) => {
                let i = self.robot_index(id).expect("validated above");
                let r = &mut self.state.robots[i];
                let removed = amount.min(r.hp);
                r.hp -= removed;
                if r.hp == 0 {
                    defeated = Some(i);
                }
                removed
            }
            Target::Outpost(_) => {
                let t = &mut self.state.teams[team.index()];
                let removed = amount.min(t.outpost_hp);
                t.outpost_hp -= removed;
                if t.outpost_hp == 0 && !practice {
                    t.outpost_ever_destroyed = true;
                    t.outpost_first_destroyed_ticks.get_or_insert(now);
                }
                removed
            }
            Target::Base(_) => {
                let t = &mut self.state.teams[team.index()];
                shield = amount.min(t.base_shield_hp);
                t.base_shield_hp -= shield;
                let removed = (amount - shield).min(t.base_hp);
                t.base_hp -= removed;
                if !practice {
                    let previous = t.base_hp_lost / 1000;
                    t.base_hp_lost = t.base_hp_lost.saturating_add(removed);
                    t.outpost_rebuild_opportunities += t.base_hp_lost / 1000 - previous;
                }
                removed
            }
        };
        if !practice
            && matches!(
                kind,
                DamageKind::Projectile | DamageKind::Dart | DamageKind::Penalty
            )
        {
            // Section 2 counts penalties as opponent attack damage; explicit
            // same-team attribution never credits the damaged side.
            let credited = attacker.filter(|a| *a != team).unwrap_or(team.other());
            self.state.teams[credited.index()].attack_damage +=
                u64::from(removed) + u64::from(shield);
        }
        self.emit(EventKind::Damage {
            target,
            shooter: shooter_id,
            hp: removed,
            shield,
            kind,
        });
        if !practice {
            self.damage_experience(target, team, removed + shield, kind, shooter, caliber);
        }
        if let Some(i) = defeated {
            self.defeat(i, shooter, kind, caliber);
        }
        if !practice && self.state.teams.iter().any(|t| t.base_hp == 0) {
            self.finish();
        }
        Applied {
            hp: removed,
            shield,
        }
    }
    /// Whether robot `i` can take experience as the identified source of
    /// damage to `team`'s targets (section 5.4.1).
    fn credited_source(&self, i: usize, team: Team) -> bool {
        let r = &self.state.robots[i];
        r.config.team != team && r.alive() && r.config.kind.levels()
    }
    /// Robots of `team` that share experience for an unidentified source.
    fn sharers(team: Team, caliber: Option<Caliber>) -> impl Fn(&RobotState) -> bool {
        move |r: &RobotState| {
            r.config.team == team
                && r.alive()
                && r.config.kind.levels()
                && caliber.is_none_or(|c| {
                    r.config.kind.shoots(c)
                        && (r.config.kind != RobotKind::Drone || r.air_support_active)
                })
        }
    }
    fn damage_experience(
        &mut self,
        target: Target,
        team: Team,
        amount: u32,
        kind: DamageKind,
        shooter: Option<usize>,
        caliber: Option<Caliber>,
    ) {
        // Section 5.4.1: 4 points per robot HP, 2 per outpost HP, 1 per 2 base
        // HP rounded up. Collision is not attack damage; dart experience is
        // section 5.6.5's and not generated here.
        if amount == 0 || !matches!(kind, DamageKind::Projectile | DamageKind::Penalty) {
            return;
        }
        let tenths = match target {
            Target::Robot(_) => u64::from(amount) * 40,
            Target::Outpost(_) => u64::from(amount) * 20,
            Target::Base(_) => u64::from(amount.div_ceil(2)) * 10,
        };
        if kind == DamageKind::Projectile
            && let Some(i) = shooter
            && self.credited_source(i, team)
        {
            self.award_experience(i, u32::try_from(tenths).unwrap_or(u32::MAX));
        } else {
            let caliber = if kind == DamageKind::Projectile {
                caliber
            } else {
                None
            };
            self.share_experience(Self::sharers(team.other(), caliber), tenths);
        }
    }
    /// Split `tenths` over the robots `share` selects, each share rounded up
    /// to a tenth (section 5.4.1).
    fn share_experience(&mut self, share: impl Fn(&RobotState) -> bool, tenths: u64) {
        let receivers: Vec<usize> = (0..self.state.robots.len())
            .filter(|&i| share(&self.state.robots[i]))
            .collect();
        if receivers.is_empty() {
            return;
        }
        let each = tenths.div_ceil(receivers.len() as u64);
        for i in receivers {
            self.award_experience(i, u32::try_from(each).unwrap_or(u32::MAX));
        }
    }
    fn level_for(tenths: u64) -> u8 {
        LEVEL_XP_TENTHS
            .iter()
            .rposition(|&need| tenths >= u64::from(need))
            .map_or(1, |i| i as u8 + 1)
    }
    /// Robot `i` reached zero HP: reset heat, start the respawn timer, drop
    /// its own buffs and remote HP and award section 5.4.1 kill experience.
    fn defeat(
        &mut self,
        i: usize,
        shooter: Option<usize>,
        kind: DamageKind,
        caliber: Option<Caliber>,
    ) {
        let practice = self.state.phase == Phase::Idle;
        let now = self.state.round_elapsed_ticks;
        let r = &mut self.state.robots[i];
        let id = r.config.id;
        let team = r.config.team;
        // Section 5.4.1: Engineer and Sentry count as level 1.
        let victim_level = u64::from(if r.config.kind.levels() { r.level } else { 1 });
        r.heat_tenths = 0;
        r.overheated = false;
        r.air_support_active = false;
        r.rebuild_progress_ticks = 0;
        // Section 5.5.3.5: defeat removes every terrain crossing buff.
        r.crossing = None;
        r.crossing_defense_until_ticks = 0;
        r.tunnel_cooling_until_ticks = 0;
        r.defeated_at_ticks = Some(now);
        r.launches_since_defeat = 0;
        if r.config.kind.ground() && !practice {
            // Section 5.2.2: round elapsed seconds, rounded to nearest
            // second; timer units accumulate at 1/s or 4/s.
            let elapsed_s = (now + SECOND_TICKS / 2) / SECOND_TICKS;
            let required_ticks = 10 * SECOND_TICKS
                + elapsed_s * SECOND_TICKS / 10
                + 20 * u64::from(r.instant_respawns) * SECOND_TICKS;
            r.respawn = Some(Respawn {
                required_ticks,
                progress_ticks: 0,
            });
        }
        self.state
            .pending_deliveries
            .retain(|d| d.robot != id || !matches!(d.kind, DeliveryKind::Hp));
        self.state
            .buffs
            .retain(|b| b.target != BuffTarget::Robot(id));
        self.emit(EventKind::RobotDefeated(id));
        // Only combat defeats award kill experience; disconnection and operator
        // defeats do not.
        if practice
            || !matches!(
                kind,
                DamageKind::Projectile | DamageKind::Collision | DamageKind::Penalty
            )
        {
            return;
        }
        // 50 x victim level x (1 + 0.2 x level difference), in tenths.
        let kill =
            |victim: u64, destroyer: u64| 100 * victim * (5 + victim.saturating_sub(destroyer));
        match shooter {
            Some(s) if kind == DamageKind::Projectile && self.credited_source(s, team) => {
                let destroyer = u64::from(self.state.robots[s].level);
                let tenths = kill(victim_level, destroyer);
                self.award_experience(s, u32::try_from(tenths).unwrap_or(u32::MAX));
            }
            _ if kind == DamageKind::Projectile && caliber.is_some() => {
                let share = Self::sharers(team.other(), caliber);
                let (count, total) = self
                    .state
                    .robots
                    .iter()
                    .filter(|r| share(r))
                    .fold((0u64, 0u64), |(n, xp), r| {
                        (n + 1, xp + u64::from(r.experience_tenths))
                    });
                if let Some(average) = total.checked_div(count) {
                    let destroyer = u64::from(Self::level_for(average));
                    self.share_experience(share, kill(victim_level, destroyer));
                }
            }
            // Assumption: with no destroyer and no projectile the manual gives
            // no destroyer level; the level difference counts as zero.
            _ => self.share_experience(
                Self::sharers(team.other(), None),
                kill(victim_level, victim_level),
            ),
        }
    }
    fn award_experience(&mut self, i: usize, tenths: u32) {
        // Table 5-11; experience stops accumulating at the assembly level cap
        // (section 5.3.3). Level-ups raise current HP by the maximum HP gained
        // (section 5.4.2).
        let now = self.state.round_elapsed_ticks;
        let team = self.state.robots[i].config.team.index();
        let cap = self.state.teams[team].level_cap;
        let r = &self.state.robots[i];
        if !r.config.kind.levels() || !r.alive() || r.level >= cap {
            return;
        }
        let t = &mut self.state.teams[team];
        let bonus = if now < t.rune_bonus_until_ticks {
            tenths.min(t.rune_bonus_tenths)
        } else {
            0
        };
        t.rune_bonus_tenths -= bonus;
        let r = &mut self.state.robots[i];
        r.experience_tenths = r
            .experience_tenths
            .saturating_add(tenths)
            .saturating_add(bonus)
            .min(LEVEL_XP_TENTHS[usize::from(cap - 1)]);
        let before = r.level;
        let before_hp = r.stats().max_hp;
        while r.level < cap && r.experience_tenths >= LEVEL_XP_TENTHS[usize::from(r.level)] {
            r.level += 1;
        }
        if r.level != before {
            r.hp =
                r.hp.saturating_add(r.stats().max_hp.saturating_sub(before_hp));
            let level = r.level;
            let robot = r.config.id;
            self.emit(EventKind::LevelChanged { robot, level });
        }
    }
    fn assembly(&mut self, team: Team, level: u8) -> Result<(), Error> {
        // Section 5.3.3: availability opens at 0, 1, 2, 3 minutes. The caller
        // certifies pose sequence, robot eligibility and shared-core interlocks.
        if !(1..=4).contains(&level)
            || self.state.round_elapsed_ticks < u64::from(level - 1) * 60 * SECOND_TICKS
        {
            return Err(Error::Invalid);
        }
        let t = &mut self.state.teams[team.index()];
        let index = usize::from(level - 1);
        if (index > 0 && t.assembly_completions[index - 1] == 0)
            || (level == 4 && t.assembly_completions[3] > 0)
        {
            return Err(Error::Ineligible);
        }
        let first = t.assembly_completions[index] == 0;
        t.assembly_completions[index] = t.assembly_completions[index]
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let income = if first {
            [50, 25, 25, 50][index]
        } else {
            [5, 10, 15, 0][index]
        };
        t.assembly_income_per_10_s = t
            .assembly_income_per_10_s
            .checked_add(income)
            .ok_or(Error::Limit)?;
        if first {
            match level {
                2 => t.level_cap = 7,
                3 => {
                    t.level_cap = 10;
                    t.assembly_defense_pct = 25;
                }
                4 => {
                    t.assembly_defense_pct = 50;
                    let healed = t.base_hp + 2000;
                    t.base_hp = healed.min(BASE_HP);
                    t.base_shield_hp += healed.saturating_sub(BASE_HP);
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn clear_weakness(&mut self, i: usize) {
        let now = self.state.round_elapsed_ticks;
        let r = &mut self.state.robots[i];
        r.weakened = false;
        r.weakened_until_ticks = None;
        let minimum = r.respawned_at_ticks.unwrap_or(now) + 10 * SECOND_TICKS;
        r.invincible_until_ticks = r.invincible_until_ticks.min(minimum.max(now));
        let id = r.config.id;
        self.emit(EventKind::WeaknessCleared(id));
    }
    fn update_weakness(&mut self, i: usize) {
        if self.state.robots[i].weakened
            && self.state.robots[i].weakened_until_ticks.is_none()
            && self.state.robots[i].alive()
            && self.own_service_zone(i)
        {
            self.clear_weakness(i);
        }
    }
    fn revive(&mut self, i: usize, hp: u32) {
        let r = &mut self.state.robots[i];
        let was_alive = r.alive();
        r.hp = hp.max(1);
        r.ejected = false;
        r.respawn = None;
        r.weakened = false;
        r.weakened_until_ticks = None;
        r.defeated_at_ticks = None;
        r.launches_since_defeat = 0;
        if !was_alive {
            let id = r.config.id;
            self.emit(EventKind::RobotRevived(id));
        }
    }
    fn respawn(&mut self, i: usize, instant: bool) {
        let now = self.state.round_elapsed_ticks;
        let r = &mut self.state.robots[i];
        let max_hp = r.stats().max_hp;
        r.hp = if instant {
            max_hp
        } else {
            ((u64::from(max_hp) + 5) / 10).max(1) as u32
        };
        r.respawn = None;
        r.weakened = true;
        r.defeated_at_ticks = None;
        r.launches_since_defeat = 0;
        r.respawned_at_ticks = Some(now);
        r.weakened_until_ticks = instant.then_some(now + 3 * SECOND_TICKS);
        r.invincible_until_ticks = now + if instant { 3 } else { 30 } * SECOND_TICKS;
        let id = r.config.id;
        self.emit(EventKind::RobotRespawned(id));
        if !instant {
            self.update_weakness(i);
        }
    }
    /// Advance explicit 1 ms ticks. Clock overflow rejects before mutation.
    /// Tick partitioning cannot change income, healing or delivery boundaries.
    ///
    /// ```
    /// use rm_simulator_gameplay::{Command, Config, Game, Phase, COUNTDOWN_TICKS};
    ///
    /// # let mut coarse = Game::new(Config::default())?;
    /// # coarse.command(Command::BeginCountdown)?;
    /// let mut fine = coarse.clone();
    /// coarse.step(COUNTDOWN_TICKS)?;
    /// for _ in 0..COUNTDOWN_TICKS {
    ///     fine.step(1)?;
    /// }
    /// assert_eq!(coarse.snapshot(), fine.snapshot());
    /// assert_eq!(coarse.snapshot().phase, Phase::Running);
    /// # Ok::<(), rm_simulator_gameplay::Error>(())
    /// ```
    pub fn step(&mut self, ticks: u64) -> Result<(), Error> {
        let end = self
            .state
            .tick
            .checked_add(ticks)
            .ok_or(Error::ClockOverflow)?;
        while self.state.tick < end {
            match self.state.phase {
                Phase::Setup | Phase::Initialization | Phase::Countdown => {
                    let (duration, next) = match self.state.phase {
                        Phase::Setup => (SETUP_TICKS, Phase::Initialization),
                        Phase::Initialization => (INITIALIZATION_TICKS, Phase::Countdown),
                        _ => (COUNTDOWN_TICKS, Phase::Running),
                    };
                    let advance =
                        (duration - self.state.phase_elapsed_ticks).min(end - self.state.tick);
                    self.state.tick += advance;
                    self.state.phase_elapsed_ticks += advance;
                    if self.state.phase_elapsed_ticks == duration {
                        self.phase(next);
                    }
                }
                Phase::Running => {
                    self.state.tick += 1;
                    self.state.round_elapsed_ticks += 1;
                    self.state.phase_elapsed_ticks += 1;
                    // End-of-round wins over timers at exactly 7:00.
                    if self.state.round_elapsed_ticks >= ROUND_TICKS {
                        self.finish();
                    } else {
                        self.tick_round();
                    }
                }
                _ => self.state.tick = end,
            }
        }
        Ok(())
    }
    fn tick_round(&mut self) {
        let now = self.state.round_elapsed_ticks;
        let enforce = self.state.policy.enforce_allowance;
        self.state.buffs.retain(|b| b.expires_ticks > now);
        // Table 5-5: 6:59 initial grant, then 5:59 through 0:59.
        if let Some(amount) = IncomeSchedule::DEFAULT.amount_at(now) {
            for team in Team::BOTH {
                self.grant(team, amount);
            }
        }
        // Assumption: assembly income settles on round-aligned 10 s boundaries.
        if now.is_multiple_of(10 * SECOND_TICKS) {
            for team in Team::BOTH {
                let amount = self.state.teams[team.index()].assembly_income_per_10_s;
                if amount > 0 {
                    self.grant(team, amount);
                }
            }
        }
        self.tick_fortress(now);
        for i in 0..self.state.robots.len() {
            let cooling_per_s = if now.is_multiple_of(100) {
                self.cooling_per_s(self.state.robots[i].config.id)
            } else {
                0
            };
            let stats = self.state.robots[i].stats();
            self.state.robots[i]
                .zones
                .retain(|c| c.expires_ticks.is_none_or(|expires| expires > now));
            let r = &mut self.state.robots[i];
            if r.weakened_until_ticks.is_some_and(|t| t <= now) {
                r.weakened = false;
                r.weakened_until_ticks = None;
            }
            // Section 5.3.2: three seconds after a Hero's defeat its team's
            // 42 mm damage stops until it is alive with allowance again.
            if r.config.kind == RobotKind::Hero {
                if !r.alive()
                    && r.defeated_at_ticks
                        .is_some_and(|d| now >= d.saturating_add(MM42_DEFEAT_TICKS))
                {
                    r.mm42_suspended = true;
                } else if r.mm42_suspended
                    && r.alive()
                    && (!enforce || r.allowance[Caliber::Mm42.index()] > 0)
                {
                    r.mm42_suspended = false;
                }
            }
            if now.is_multiple_of(100) {
                r.heat_tenths = r.heat_tenths.saturating_sub(u64::from(cooling_per_s));
                if r.heat_tenths == 0 {
                    r.overheated = false;
                }
            }
            if r.config.kind == RobotKind::Drone && now.is_multiple_of(60 * SECOND_TICKS) {
                r.air_support_ticks += 20 * SECOND_TICKS;
            }
            if r.air_support_active && r.alive() && !r.irregularly_disconnected {
                if r.air_support_ticks == 0 {
                    let gold = &mut self.state.teams[r.config.team.index()].gold;
                    if *gold > 0 {
                        *gold -= 1;
                        r.air_support_ticks = SECOND_TICKS;
                    } else {
                        r.air_support_active = false;
                    }
                }
                if r.air_support_active {
                    r.air_support_ticks -= 1;
                }
            }
            if !r.alive()
                && !r.ejected
                && !r.irregularly_disconnected
                && let Some(timer) = &mut r.respawn
            {
                let accelerated =
                    r.zones.iter().any(|z| {
                        z.zone.kind == ZoneKind::Resupply && z.zone.owner == r.config.team
                    }) || self.state.teams[r.config.team.index()].base_hp < 2000;
                timer.progress_ticks += if accelerated { 4 } else { 1 };
                if timer.progress_ticks >= timer.required_ticks {
                    self.respawn(i, false);
                }
            }
            self.update_weakness(i);
            if self.eligible(i) {
                let r = &mut self.state.robots[i];
                let team = r.config.team;
                if r.config.kind.ground()
                    && r.in_zone(ZoneKind::Resupply, team)
                    && now.is_multiple_of(SECOND_TICKS)
                {
                    let percent = if now > 240 * SECOND_TICKS && r.out_of_combat {
                        25
                    } else {
                        10
                    };
                    let heal = ((u64::from(stats.max_hp) * percent + 50) / 100) as u32;
                    r.hp = r.hp.saturating_add(heal).min(stats.max_hp);
                    if r.config.kind == RobotKind::Sentry {
                        let allowance = (now / (60 * SECOND_TICKS)) as u32 * 100;
                        r.allowance[0] += allowance - r.sentry_resupply_claimed;
                        r.sentry_resupply_claimed = allowance;
                    }
                }
                let t = &mut self.state.teams[team.index()];
                let scan = r.zones.iter().any(|z| {
                    z.detected && z.zone.kind == ZoneKind::Outpost && z.zone.owner == team
                });
                if r.config.kind.ground()
                    && scan
                    && t.outpost_hp == 0
                    && t.outpost_rebuild_opportunities > 0
                    && now < 300 * SECOND_TICKS
                {
                    r.rebuild_progress_ticks += 1;
                    let required = if r.config.kind == RobotKind::Engineer {
                        5
                    } else {
                        10
                    } * SECOND_TICKS;
                    if r.rebuild_progress_ticks >= required {
                        t.outpost_hp = 750;
                        t.outpost_rebuild_opportunities -= 1;
                        r.rebuild_progress_ticks = 0;
                        self.emit(EventKind::OutpostRebuilt(team));
                    }
                } else {
                    r.rebuild_progress_ticks = 0;
                }
            } else {
                self.state.robots[i].rebuild_progress_ticks = 0;
            }
        }
        let due = self
            .state
            .pending_deliveries
            .partition_point(|delivery| delivery.at_ticks <= now);
        debug_assert!(
            self.state
                .pending_deliveries
                .windows(2)
                .all(|pair| pair[0].at_ticks <= pair[1].at_ticks)
        );
        let deliveries: Vec<_> = self.state.pending_deliveries.drain(..due).collect();
        for delivery in deliveries {
            if let Ok(i) = self.robot_index(delivery.robot) {
                let max_hp = self.state.robots[i].stats().max_hp;
                let r = &mut self.state.robots[i];
                match delivery.kind {
                    DeliveryKind::Ammo(caliber, amount) => {
                        r.allowance[caliber.index()] =
                            r.allowance[caliber.index()].saturating_add(amount)
                    }
                    DeliveryKind::Hp if r.alive() && !r.irregularly_disconnected => {
                        r.hp = (u64::from(r.hp) * 160 + 50)
                            .div_euclid(100)
                            .min(u64::from(max_hp)) as u32
                    }
                    _ => {}
                }
            }
        }
    }
    fn finish(&mut self) {
        self.state.result = Some(self.determine_result());
        self.state.pending_deliveries.clear();
        self.state.buffs.clear();
        for t in &mut self.state.teams {
            t.rune_bonus_tenths = 0;
        }
        for r in &mut self.state.robots {
            r.air_support_active = false;
        }
        self.phase(Phase::RoundEnded);
    }
    /// Section 5.8 in its printed order, with explicit referee resolution for
    /// missing/ambiguous comparisons. Base virtual shield is excluded.
    fn determine_result(&self) -> RoundResult {
        use std::cmp::Ordering;
        let [red, blue] = &self.state.teams;
        let compare = |r: u64, b: u64, reason| RoundResult::Decided {
            winner: match r.cmp(&b) {
                Ordering::Greater => Some(Team::Red),
                Ordering::Less => Some(Team::Blue),
                Ordering::Equal => None,
            },
            reason,
        };
        if red.base_hp != blue.base_hp {
            return if red.base_hp == 0 || blue.base_hp == 0 {
                compare(
                    u64::from(red.base_hp),
                    u64::from(blue.base_hp),
                    DecisionReason::BaseDestroyed,
                )
            } else {
                RoundResult::NeedsRefereeDecision
            };
        }
        if !red.outpost_ever_destroyed
            && !blue.outpost_ever_destroyed
            && red.outpost_hp != blue.outpost_hp
        {
            return compare(
                u64::from(red.outpost_hp),
                u64::from(blue.outpost_hp),
                DecisionReason::Outpost,
            );
        }
        if red.outpost_ever_destroyed != blue.outpost_ever_destroyed {
            return compare(
                u64::from(!red.outpost_ever_destroyed),
                u64::from(!blue.outpost_ever_destroyed),
                DecisionReason::Outpost,
            );
        }
        if red.outpost_ever_destroyed
            && blue.outpost_ever_destroyed
            && (red.outpost_hp > 0 || blue.outpost_hp > 0)
        {
            return RoundResult::NeedsRefereeDecision;
        }
        if red.attack_damage != blue.attack_damage {
            return compare(
                red.attack_damage,
                blue.attack_damage,
                DecisionReason::AttackDamage,
            );
        }
        let hp = |team| {
            self.state
                .robots
                .iter()
                .filter(|r| {
                    r.config.team == team
                        && r.alive()
                        && (r.config.kind.ground() || r.config.kind == RobotKind::Drone)
                })
                .map(|r| u64::from(r.hp))
                .sum::<u64>()
        };
        if hp(Team::Red) != hp(Team::Blue) {
            return compare(hp(Team::Red), hp(Team::Blue), DecisionReason::RobotHp);
        }
        RoundResult::Decided {
            winner: None,
            reason: DecisionReason::Equal,
        }
    }
}

#[cfg(test)]
mod tests;
