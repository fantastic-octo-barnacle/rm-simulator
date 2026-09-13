// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
use crate::policy::{IncomeSchedule, record_ammo_launch};
use crate::*;
use serde::{Deserialize, Serialize};

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
    /// Clear every result and restore the configured roster, keeping the
    /// simulation tick.
    ResetMatch,
    /// Detected damage after attacker buffs, before target defenses. Caller
    /// handles detection, attacker effects and special target immunities.
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
        /// arrives six seconds later. False requires an own service zone.
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
    /// may launch in the running round.
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
///     Caliber, Command, Config, Game, RobotConfig, RobotKind, Team, COUNTDOWN_TICKS,
/// };
///
/// let mut game = Game::new(Config {
///     robots: vec![RobotConfig {
///         id: 7,
///         team: Team::Red,
///         kind: RobotKind::Sentry,
///         max_hp: 400,
///         heat_limit: 100,
///         cooling_per_s: 20,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Game {
    config: Config,
    state: Snapshot,
}
impl Game {
    /// Build a game from a fixed roster. Returns [`Error::Invalid`] when an id
    /// repeats or a robot's `max_hp` is zero.
    ///
    /// ```
    /// use rm_simulator_gameplay::{Config, Error, Game, RobotConfig, RobotKind, Team};
    ///
    /// let infantry = RobotConfig {
    ///     id: 1,
    ///     team: Team::Red,
    ///     kind: RobotKind::Infantry,
    ///     max_hp: 200,
    ///     heat_limit: 100,
    ///     cooling_per_s: 20,
    /// };
    /// let roster = Config {
    ///     robots: vec![infantry.clone(), infantry],
    ///     ..Config::default()
    /// };
    /// assert_eq!(Game::new(roster), Err(Error::Invalid));
    /// ```
    pub fn new(config: Config) -> Result<Self, Error> {
        let mut ids = std::collections::BTreeSet::new();
        if config
            .robots
            .iter()
            .any(|r| r.max_hp == 0 || !ids.insert(r.id))
        {
            return Err(Error::Invalid);
        }
        let state = Self::fresh_state(&config);
        Ok(Self { config, state })
    }
    /// Current state. The reference is read-only and valid until the next
    /// command or step.
    pub fn snapshot(&self) -> &Snapshot {
        &self.state
    }
    /// Roster and match format the game was built with.
    pub fn config(&self) -> &Config {
        &self.config
    }
    /// Match-aware launch permission for adapters; robot-local locks alone
    /// do not include round phase.
    ///
    /// ```
    /// use rm_simulator_gameplay::{
    ///     Caliber, Command, Config, Game, RobotConfig, RobotKind, Team, COUNTDOWN_TICKS,
    /// };
    ///
    /// # let mut game = Game::new(Config {
    /// #     robots: vec![RobotConfig {
    /// #         id: 4,
    /// #         team: Team::Red,
    /// #         kind: RobotKind::Sentry,
    /// #         max_hp: 400,
    /// #         heat_limit: 100,
    /// #         cooling_per_s: 20,
    /// #     }],
    /// #     ..Config::default()
    /// # })?;
    /// assert!(!game.can_launch(4, Caliber::Mm17));
    /// game.command(Command::BeginCountdown)?;
    /// game.step(COUNTDOWN_TICKS)?;
    /// assert!(game.can_launch(4, Caliber::Mm17));
    /// game.command(Command::EndRound)?;
    /// assert!(!game.can_launch(4, Caliber::Mm17));
    /// # Ok::<(), rm_simulator_gameplay::Error>(())
    /// ```
    pub fn can_launch(&self, robot: u32, caliber: Caliber) -> bool {
        self.state.phase == Phase::Running
            && self.robot_index(robot).is_ok_and(|i| {
                self.state.robots[i].can_launch(self.state.round_elapsed_ticks, caliber)
            })
    }
    fn fresh_state(config: &Config) -> Snapshot {
        Snapshot {
            tick: 0,
            phase: Phase::Idle,
            phase_elapsed_ticks: 0,
            round: 0,
            round_elapsed_ticks: 0,
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
                outpost_rebuild_opportunities: 0,
                attack_damage: 0,
                level_cap: 5,
                assembly_completions: [0; 4],
                assembly_income_per_10_s: 0,
                assembly_defense_pct: 0,
            }),
            robots: config
                .robots
                .iter()
                .map(|r| RobotState {
                    config: r.clone(),
                    hp: r.max_hp,
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
                })
                .collect(),
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
    fn emit(&mut self, kind: EventKind) {
        let state = &mut self.state;
        state.recent_events.push(Event {
            id: state.next_event_id,
            tick: state.tick,
            round_ticks: state.round_elapsed_ticks,
            kind,
        });
        state.next_event_id += 1;
        // Application event-retention policy, not a rulebook constant.
        if state.recent_events.len() > 512 {
            state.recent_events.remove(0);
        }
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
    fn eligible(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        r.alive() && !r.irregularly_disconnected && !r.weakened
    }
    fn own_service_zone(&self, i: usize) -> bool {
        let r = &self.state.robots[i];
        let team = r.config.team;
        r.in_zone(ZoneKind::Base, team)
            || r.in_zone(ZoneKind::Resupply, team)
            || (r.in_zone(ZoneKind::Outpost, team) && self.state.teams[team.index()].outpost_hp > 0)
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
    ///     Caliber, Command, Config, Error, Game, RobotConfig, RobotKind, Team, COUNTDOWN_TICKS,
    /// };
    ///
    /// # let mut game = Game::new(Config {
    /// #     robots: vec![RobotConfig {
    /// #         id: 1,
    /// #         team: Team::Red,
    /// #         kind: RobotKind::Infantry,
    /// #         max_hp: 200,
    /// #         heat_limit: 100,
    /// #         cooling_per_s: 20,
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
        ) {
            self.apply(command)?;
            self.emit(EventKind::CommandAccepted);
            return Ok(());
        }
        let mut next = self.clone();
        next.apply(command)?;
        next.emit(EventKind::CommandAccepted);
        *self = next;
        Ok(())
    }
    fn apply(&mut self, command: Command) -> Result<(), Error> {
        match command {
            Command::BeginRound | Command::BeginCountdown => {
                if !matches!(self.state.phase, Phase::Idle | Phase::Confirmed) {
                    return Err(Error::Phase);
                }
                let fresh = Self::fresh_state(&self.config);
                self.state.teams = fresh.teams;
                self.state.robots = fresh.robots;
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
                self.state = Self::fresh_state(&self.config);
                self.state.tick = tick;
                self.state.pending_deliveries.clear();
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
            Command::Damage {
                target,
                amount,
                kind,
                attacker,
            } => self.damage(target, amount, kind, attacker)?,
            Command::ZoneDetection {
                robot,
                zone,
                detected,
            } => {
                let i = self.robot_index(robot)?;
                let zones = &mut self.state.robots[i].zones;
                if let Some(contact) = zones.iter_mut().find(|c| c.zone == zone) {
                    contact.detected = detected;
                    // Section 5.5.3.1: two-second occupation expiry delay. Repeated
                    // false samples must not extend the original deadline.
                    if detected {
                        contact.expires_ticks = None;
                    } else if contact.expires_ticks.is_none() {
                        contact.expires_ticks = Some(now + 2 * SECOND_TICKS);
                    }
                } else if detected {
                    zones.push(ZoneContact {
                        zone,
                        detected: true,
                        expires_ticks: None,
                    });
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
                self.damage(
                    Target::Robot(robot),
                    u32::MAX,
                    DamageKind::Penalty,
                    Some(team.other()),
                )?;
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
                    || (!remote && !self.own_service_zone(i))
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
                let cost = now.div_ceil(60 * SECOND_TICKS) as u32 * 80 + u32::from(r.level) * 20;
                self.spend(r.config.team, cost)?;
                self.state.robots[i].instant_respawns += 1;
                self.respawn(i, true);
            }
            Command::Launch { robot, caliber } => {
                let i = self.robot_index(robot)?;
                if !self.state.robots[i].can_launch(now, caliber) {
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
                self.target_team(buff.target)?;
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
    fn record_launch(&mut self, i: usize, caliber: Caliber) {
        // Sections 5.1.3, 5.3.2 and 5.4.1: sensor-detected launches count
        // even if a physical launcher fired while locked or without allowance.
        let r = &mut self.state.robots[i];
        let index = caliber.index();
        if record_ammo_launch(&mut r.allowance[index], &mut r.shots_launched[index]) {
            r.shots_over_allowance[index] = r.shots_over_allowance[index].saturating_add(1);
        }
        r.heat_tenths =
            r.heat_tenths
                .saturating_add(if caliber == Caliber::Mm17 { 100 } else { 1000 });
        let extra = if caliber == Caliber::Mm17 { 100 } else { 200 };
        // Figure 5-1 uses >= Q2, unlike the neighboring prose's > Q2.
        if r.heat_tenths >= (u64::from(r.config.heat_limit) + extra) * 10 {
            r.heat_locked_for_round = true;
        }
        if r.heat_tenths > u64::from(r.config.heat_limit) * 10 {
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
    fn damage(
        &mut self,
        target: Target,
        amount: u32,
        kind: DamageKind,
        attacker: Option<Team>,
    ) -> Result<(), Error> {
        let team = self.target_team(target)?;
        if let Target::Robot(id) = target {
            let r = &self.state.robots[self.robot_index(id)?];
            if !r.alive()
                || (matches!(
                    kind,
                    DamageKind::Projectile | DamageKind::Dart | DamageKind::Collision
                ) && r.invincible_until_ticks > self.state.round_elapsed_ticks)
            {
                return Ok(());
            }
        }
        if matches!(target, Target::Base(_))
            && self.state.teams[team.index()].outpost_hp > 0
            && matches!(
                kind,
                DamageKind::Projectile | DamageKind::Dart | DamageKind::Collision
            )
        {
            return Ok(());
        }
        let mut defense = 0;
        let mut vulnerability = 0;
        if matches!(kind, DamageKind::Projectile | DamageKind::Collision) {
            defense = self.state.teams[team.index()].assembly_defense_pct;
            for buff in self.state.buffs.iter().filter(|b| b.target == target) {
                defense = defense.max(buff.defense_pct);
                vulnerability = vulnerability.max(buff.vulnerability_pct);
            }
            if let Target::Robot(id) = target {
                let i = self.robot_index(id)?;
                if self.eligible(i)
                    && self.state.robots[i].config.kind.ground()
                    && self.state.robots[i].in_zone(ZoneKind::Base, team)
                {
                    defense = defense.max(50);
                }
            }
        }
        // Attack buffs require a shooter-specific source. `amount` therefore
        // already includes any attacker effect certified by the caller; only
        // target defenses/vulnerability are applied here.
        let scaled = (u64::from(amount) * u64::from(100 - defense + vulnerability) + 50) / 100;
        let amount = scaled.min(u64::from(u32::MAX)) as u32;
        let mut shield = 0;
        let removed = match target {
            Target::Robot(id) => {
                let i = self.robot_index(id)?;
                let r = &mut self.state.robots[i];
                let removed = amount.min(r.hp);
                r.hp -= removed;
                if r.hp == 0 {
                    r.heat_tenths = 0;
                    r.overheated = false;
                    r.air_support_active = false;
                    r.rebuild_progress_ticks = 0;
                    if r.config.kind.ground() {
                        // Section 5.2.2: round elapsed seconds, rounded to nearest
                        // second; timer units accumulate at 1/s or 4/s.
                        let elapsed_s =
                            (self.state.round_elapsed_ticks + SECOND_TICKS / 2) / SECOND_TICKS;
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
                    self.state.buffs.retain(|b| b.target != target);
                    self.emit(EventKind::RobotDefeated(id));
                }
                removed
            }
            Target::Outpost(_) => {
                let t = &mut self.state.teams[team.index()];
                let removed = amount.min(t.outpost_hp);
                t.outpost_hp -= removed;
                if t.outpost_hp == 0 {
                    t.outpost_ever_destroyed = true;
                }
                removed
            }
            Target::Base(_) => {
                let t = &mut self.state.teams[team.index()];
                shield = amount.min(t.base_shield_hp);
                t.base_shield_hp -= shield;
                let removed = (amount - shield).min(t.base_hp);
                t.base_hp -= removed;
                let previous = t.base_hp_lost / 1000;
                t.base_hp_lost = t.base_hp_lost.saturating_add(removed);
                t.outpost_rebuild_opportunities += t.base_hp_lost / 1000 - previous;
                removed
            }
        };
        if matches!(
            kind,
            DamageKind::Projectile | DamageKind::Dart | DamageKind::Penalty
        ) {
            // Section 2 counts penalties as opponent attack damage; explicit
            // same-team attribution never credits the damaged side.
            let credited = attacker.filter(|a| *a != team).unwrap_or(team.other());
            self.state.teams[credited.index()].attack_damage +=
                u64::from(removed) + u64::from(shield);
        }
        self.emit(EventKind::Damage {
            target,
            hp: removed,
            shield,
            kind,
        });
        if self.state.teams.iter().any(|t| t.base_hp == 0) {
            self.finish();
        }
        Ok(())
    }
    fn award_experience(&mut self, i: usize, tenths: u32) {
        // Table 5-11; experience stops accumulating at the assembly level cap
        // (section 5.3.3). Table 5-12..15 performance changes are external.
        const LEVEL_XP: [u32; 10] = [
            0, 5500, 11000, 16500, 22000, 27500, 33000, 38500, 44000, 50000,
        ];
        let cap = self.state.teams[self.state.robots[i].config.team.index()].level_cap;
        let r = &mut self.state.robots[i];
        if !r.config.kind.levels() || !r.alive() || r.level >= cap {
            return;
        }
        r.experience_tenths = r
            .experience_tenths
            .saturating_add(tenths)
            .min(LEVEL_XP[usize::from(cap - 1)]);
        let before = r.level;
        while r.level < cap && r.experience_tenths >= LEVEL_XP[usize::from(r.level)] {
            r.level += 1;
        }
        if r.level != before {
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
    fn update_weakness(&mut self, i: usize) {
        let now = self.state.round_elapsed_ticks;
        if self.state.robots[i].weakened
            && self.state.robots[i].weakened_until_ticks.is_none()
            && self.state.robots[i].alive()
            && self.own_service_zone(i)
        {
            let r = &mut self.state.robots[i];
            r.weakened = false;
            let minimum = r.respawned_at_ticks.unwrap_or(now) + 10 * SECOND_TICKS;
            r.invincible_until_ticks = r.invincible_until_ticks.min(minimum.max(now));
        }
    }
    fn respawn(&mut self, i: usize, instant: bool) {
        let now = self.state.round_elapsed_ticks;
        let r = &mut self.state.robots[i];
        r.hp = if instant {
            r.config.max_hp
        } else {
            ((u64::from(r.config.max_hp) + 5) / 10).max(1) as u32
        };
        r.respawn = None;
        r.weakened = true;
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
        for i in 0..self.state.robots.len() {
            self.state.robots[i]
                .zones
                .retain(|c| c.expires_ticks.is_none_or(|expires| expires > now));
            let r = &mut self.state.robots[i];
            if r.weakened_until_ticks.is_some_and(|t| t <= now) {
                r.weakened = false;
                r.weakened_until_ticks = None;
            }
            if now.is_multiple_of(100) {
                let multiplier = self
                    .state
                    .buffs
                    .iter()
                    .filter(|b| b.target == Target::Robot(r.config.id))
                    .map(|b| b.cooling_multiplier)
                    .max()
                    .unwrap_or(1)
                    .max(1);
                r.heat_tenths = r
                    .heat_tenths
                    .saturating_sub(u64::from(r.config.cooling_per_s) * u64::from(multiplier));
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
                let accelerated = r.zones.iter().any(|z| {
                    z.zone
                        == Zone {
                            kind: ZoneKind::Resupply,
                            owner: r.config.team,
                        }
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
                    let heal = ((u64::from(r.config.max_hp) * percent + 50) / 100) as u32;
                    r.hp = r.hp.saturating_add(heal).min(r.config.max_hp);
                    if r.config.kind == RobotKind::Sentry {
                        let allowance = (now / (60 * SECOND_TICKS)) as u32 * 100;
                        r.allowance[0] += allowance - r.sentry_resupply_claimed;
                        r.sentry_resupply_claimed = allowance;
                    }
                }
                let t = &mut self.state.teams[team.index()];
                let scan = r.zones.iter().any(|z| {
                    z.detected
                        && z.zone
                            == Zone {
                                kind: ZoneKind::Outpost,
                                owner: team,
                            }
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
                let r = &mut self.state.robots[i];
                match delivery.kind {
                    DeliveryKind::Ammo(caliber, amount) => {
                        r.allowance[caliber.index()] =
                            r.allowance[caliber.index()].saturating_add(amount)
                    }
                    DeliveryKind::Hp if r.alive() && !r.irregularly_disconnected => {
                        r.hp = (u64::from(r.hp) * 160 + 50)
                            .div_euclid(100)
                            .min(u64::from(r.config.max_hp)) as u32
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
