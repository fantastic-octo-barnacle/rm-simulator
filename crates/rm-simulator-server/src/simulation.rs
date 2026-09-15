// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The authoritative field with pause and pacing state, and the chassis
//! spawner that gives players a body. The host clock is converted to ticks
//! here, never inside the world crate.
use crate::cad_assets::CadAssets;
use crate::layout::{
    ChassisSpawner, LayoutOptions, SPAWN_SLOT_SPACING_M, add_terrain, field_config, load_terrain,
};
use crate::protocol::{Command, Robot, WeaponConfig};
use rm_simulator_world::{
    ChassisConfig, Field, FieldError, FieldSnapshot, RobotKind, Team, tick_ns,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// Catch-up cap so a stalled host does not spiral the world clock.
pub const MAX_ADVANCE_NS: u64 = 250_000_000;
/// World time one manual step covers while paused, in nanoseconds. The step is
/// a duration, not a tick count, so it covers at least 16 ms at every physics
/// rate; a rate whose ticks do not divide it rounds up to the next boundary.
pub const STEP_NS: u64 = 16_000_000;

/// Ticks in one manual step while paused: the first tick boundary at or after
/// [`STEP_NS`] at the current rate, so a step is never shorter than 16 ms.
///
/// ```
/// use rm_simulator_server::simulation::{STEP_NS, step_ticks};
///
/// let tick_ns = rm_simulator_world::tick_ns();
/// assert!(step_ticks() * tick_ns >= STEP_NS);
/// assert!((step_ticks() - 1) * tick_ns < STEP_NS);
/// ```
pub fn step_ticks() -> u64 {
    STEP_NS.div_ceil(tick_ns()).max(1)
}

/// What clients see: the field plus the host's pacing state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimulationState {
    /// Chassis ids of the training bots, ascending.
    #[serde(default)]
    pub bots: Vec<u32>,
    /// Host publication identity; zero for an unpublished operator capture.
    pub snapshot_id: u64,
    /// Counter bumped on every pause transition. Inputs and shots stamped with
    /// another epoch are refused.
    pub input_epoch: u64,
    /// Each shooter's newest 32 results, newest first.
    pub shot_results: Vec<crate::protocol::ShotResult>,
    /// Whether the host is paused.
    pub paused: bool,
    /// The whole field at the publication tick.
    pub field: FieldSnapshot,
}

/// Construction events shared by the window's loading screen and headless host.
#[derive(Debug, PartialEq)]
pub enum BuildProgress {
    /// Terrain files are being read.
    ReadingTerrain,
    /// The physics world and the rules are being built.
    BuildingPhysics,
    /// Terrain finished loading; the string describes the loaded mesh.
    TerrainReady(String),
    /// The chassis spawner is being prepared.
    PreparingChassis,
}

/// Accepted shot journal, bounded to the latest 256 shots. Times are simulation times
/// except the explicitly client-relative clock. Projectile ID links to impact records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FireRecord {
    /// Actual initial speed after sampling, in m/s.
    pub launch_speed_m_s: f64,
    /// Chassis id of the shooter.
    pub shooter: u32,
    /// Id of the projectile in the field snapshot.
    pub projectile_id: u64,
    /// Simulation time at which the shot fired, in ns.
    pub accepted_time_ns: u64,
    /// Muzzle pose the host fired from, in FLU metres and wxyz.
    pub authoritative_muzzle_pose: rm_simulator_world::Pose,
    /// Client-reported timing kept for diagnostics; firing ignores it.
    pub client_timing: Option<crate::protocol::FireTiming>,
}

/// The authoritative field with the host's pause, pacing, input and shot state.
/// One host worker owns the only live value. World time moves only through
/// [`advance`](Simulation::advance) and [`step`](Simulation::step), and only in
/// whole 1 ms ticks.
pub struct Simulation {
    /// Lobby password every remote seat must present. The embedded owner seat
    /// is exempt because it already holds the simulation.
    pub(crate) password: String,
    bots: std::collections::BTreeSet<u32>,
    /// Ordered so that snapshot bytes never depend on hash iteration order.
    pub(crate) input_streams: BTreeMap<u32, crate::input_stream::InputStream>,
    shot_results: BTreeMap<u32, VecDeque<crate::protocol::ShotResult>>,
    last_shot_ids: BTreeMap<u32, u64>,
    pending_shots: Vec<Command>,
    completed_shots: VecDeque<crate::protocol::ShotResult>,
    prediction_scene: Option<crate::prediction::PredictionScene>,
    fire_records: VecDeque<FireRecord>,
    field: Field,
    paused: bool,
    input_epoch: u64,
    /// Host time not yet turned into a whole tick.
    carry_ns: u64,
    /// How players get a chassis; without one everybody spectates.
    spawner: Option<ChassisSpawner>,
    weapon: WeaponConfig,
    weapon_limits: crate::protocol::WeaponLimits,
    pilot_weapons: BTreeMap<u32, WeaponConfig>,
    /// The robot each pilot's chassis was spawned as; a bot or a chassis
    /// spawned by configuration alone has no entry.
    robots: BTreeMap<u32, Robot>,
    /// Intended times of each shooter's recent shots, on the pilot's timeline.
    fired_ns: BTreeMap<u32, VecDeque<u64>>,
    pilot_spawns: BTreeMap<u32, rm_simulator_world::Pose>,
}

impl Simulation {
    /// Build the CAD field and optional chassis spawner without opening sockets
    /// or advancing time. The caller chooses whether to run this on a worker.
    /// No player is added until a spawn method is called.
    pub fn from_cad(
        cad: &CadAssets,
        options: &LayoutOptions,
        chassis: Option<ChassisConfig>,
        paused: bool,
        mut progress: impl FnMut(BuildProgress),
    ) -> anyhow::Result<Self> {
        let terrain = if options.terrain {
            progress(BuildProgress::ReadingTerrain);
            Some(load_terrain(cad)?)
        } else {
            None
        };
        progress(BuildProgress::BuildingPhysics);
        let mut config = field_config(cad, options);
        config.bases = crate::base_layout::load(cad)?;
        let mut field = Field::new(&config)?;
        if let Some(terrain) = &terrain {
            add_terrain(&mut field, terrain)?;
            progress(BuildProgress::TerrainReady(terrain.describe()));
        }
        let mut simulation = Self::new(field, paused);
        if let Some(config) = chassis {
            progress(BuildProgress::PreparingChassis);
            simulation = simulation.with_spawner(ChassisSpawner { config, terrain });
        }
        simulation.prediction_scene = crate::prediction::PredictionScene::from_cad(cad, options);
        Ok(simulation)
    }

    /// Wrap a field with empty pacing, input and shot state. No player is added
    /// and no prediction scene is advertised.
    pub fn new(field: Field, paused: bool) -> Self {
        Self {
            password: String::new(),
            bots: Default::default(),
            input_epoch: 0,
            input_streams: BTreeMap::new(),
            shot_results: BTreeMap::new(),
            last_shot_ids: BTreeMap::new(),
            pending_shots: Vec::new(),
            completed_shots: VecDeque::new(),
            prediction_scene: None,
            field,
            paused,
            carry_ns: 0,
            spawner: None,
            weapon: WeaponConfig::default(),
            weapon_limits: Default::default(),
            pilot_weapons: BTreeMap::new(),
            robots: BTreeMap::new(),
            fired_ns: BTreeMap::new(),
            pilot_spawns: Default::default(),
            fire_records: VecDeque::new(),
        }
    }
    fn spawn_bot(&mut self, team: Team, spin_rad_s: f64) -> Result<(), String> {
        if !spin_rad_s.is_finite() || spin_rad_s.abs() > 20. {
            return Err("bot spin must be finite and within +/-20 rad/s".into());
        }
        if self.bots.len() >= 32 {
            return Err("at most 32 training bots".into());
        }
        let id = self.spawn_chassis(team)?;
        self.field
            .command_chassis(
                id,
                rm_simulator_world::ChassisCommand {
                    yaw_rate_rad_s: spin_rad_s,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
        self.bots.insert(id);
        Ok(())
    }

    /// Let players spawn chassis.
    /// Require this password for every remote seat, including spectators.
    pub fn with_password(mut self, password: String) -> Self {
        self.password = password;
        self
    }

    /// Let players spawn chassis. Without a spawner, `spawn_chassis` fails and
    /// everybody spectates.
    pub fn with_spawner(mut self, spawner: ChassisSpawner) -> Self {
        self.spawner = Some(spawner);
        self
    }
    /// Set host starting settings and match the caps to their rate and speed.
    /// Call `with_weapon_limits` afterward to allow higher values. Fails when
    /// the configuration is invalid.
    pub fn with_weapon(mut self, weapon: WeaponConfig) -> Result<Self, String> {
        self.weapon = weapon.validate().map_err(str::to_string)?;
        self.weapon_limits = crate::protocol::WeaponLimits {
            max_speed_m_s: weapon.shot.speed_m_s,
            min_interval_ns: weapon.interval_ns,
        };
        Ok(self)
    }
    /// Set independent host caps, refusing limits that exclude the defaults.
    ///
    /// ```
    /// use rm_simulator_server::{simulation::Simulation, protocol::WeaponLimits};
    /// use rm_simulator_world::{Field, FieldConfig};
    /// let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false)
    ///     .with_weapon_limits(WeaponLimits::default()).unwrap();
    /// assert_eq!(simulation.weapon_limits().max_speed_m_s, 30.0);
    /// ```
    pub fn with_weapon_limits(
        mut self,
        limits: crate::protocol::WeaponLimits,
    ) -> Result<Self, String> {
        limits
            .admit(self.weapon.shot.caliber, self.weapon)
            .map_err(str::to_string)?;
        self.weapon_limits = limits;
        Ok(self)
    }
    /// Caps offered to every pilot independently of their starting settings.
    pub fn weapon_limits(&self) -> crate::protocol::WeaponLimits {
        self.weapon_limits
    }
    /// The scene clients need to predict, or None when the field was not built
    /// from a verifiable CAD package.
    pub fn prediction_scene(&self) -> Option<crate::prediction::PredictionScene> {
        self.prediction_scene.clone()
    }
    /// Host starting weapon settings.
    pub fn weapon(&self) -> WeaponConfig {
        self.weapon
    }
    /// The live field.
    pub fn field(&self) -> &Field {
        &self.field
    }
    /// Tests only: mutate the field directly. Prediction is disabled because the
    /// host can no longer promise clients an identical world.
    #[cfg(test)]
    pub(crate) fn test_field_mut(&mut self) -> &mut Field {
        self.prediction_scene = None;
        &mut self.field
    }
    /// Whether the host is paused.
    pub fn paused(&self) -> bool {
        self.paused
    }
    /// The current input epoch, bumped on every pause transition.
    pub fn input_epoch(&self) -> u64 {
        self.input_epoch
    }
    /// Set the pause flag. A change bumps the input epoch, commands every held
    /// input to neutral, cancels queued shots and discards un-ticked host time,
    /// so no input or shot crosses the pause.
    pub fn set_paused(&mut self, paused: bool) {
        if paused != self.paused {
            self.input_epoch = self
                .input_epoch
                .checked_add(1)
                .expect("input epoch exhausted");
            for (id, stream) in &mut self.input_streams {
                if let Some(neutral) = stream.cancel() {
                    let _ = self.field.command_chassis(*id, neutral);
                }
            }
            self.cancel_pending_shots(None, "shot cancelled by pause or epoch change");
            self.input_streams.clear();
            self.carry_ns = 0;
        }
        self.paused = paused;
    }
    /// The chassis configuration a chassis gets when no robot is named for
    /// it, when the field offers chassis at all.
    pub fn spawner_config(&self) -> Option<&ChassisConfig> {
        self.spawner.as_ref().map(|spawner| &spawner.config)
    }
    /// Give a player of `team` a chassis of the spawner's default
    /// configuration in the team's first spawn slot that no chassis is
    /// standing in. It is no particular robot, so its gun fires the host's
    /// default caliber; training bots take this path.
    pub fn spawn_chassis(&mut self, team: Team) -> Result<u32, String> {
        let config = self.spawner_config().cloned();
        self.spawn_in_slot(team, config, RobotKind::Infantry, None)
    }
    /// Give a pilot of `team` the chassis of `robot` in the team's first spawn
    /// slot that no chassis is standing in. The robot fixes the chassis
    /// preset, the class the referee records and the caliber its gun fires.
    pub fn spawn_robot(&mut self, team: Team, robot: Robot) -> Result<u32, String> {
        self.spawn_in_slot(
            team,
            Some(robot.chassis_config()),
            robot.kind(),
            Some(robot),
        )
    }
    fn spawn_in_slot(
        &mut self,
        team: Team,
        config: Option<ChassisConfig>,
        kind: RobotKind,
        robot: Option<Robot>,
    ) -> Result<u32, String> {
        let spawner = self
            .spawner
            .as_ref()
            .ok_or("this field offers no chassis")?;
        let config = config.unwrap_or_else(|| spawner.config.clone());
        let clear = SPAWN_SLOT_SPACING_M / 2.0;
        let placement = (0..)
            .map(|slot| spawner.slot_with(config.clone(), kind, team, slot))
            .find(|placement| {
                let [x, y, _] = placement.spawn.translation_m;
                self.field
                    .chassis_positions_m()
                    .all(|[px, py, _]| (px - x).hypot(py - y) > clear)
            })
            .expect("spawn slots go on forever");
        self.place(placement, robot)
    }
    /// Place a local pilot at a requested FLU position and heading in the
    /// spawner's default configuration, using the same ground lookup as
    /// joining network pilots. Like `spawn_chassis` it is no particular robot.
    pub fn spawn_chassis_at(
        &mut self,
        team: Team,
        spawn_m: [f64; 3],
        yaw_deg: f64,
    ) -> Result<u32, String> {
        let spawner = self
            .spawner
            .as_ref()
            .ok_or("this field offers no chassis")?;
        let placement = spawner.at(team, spawn_m, yaw_deg);
        self.place(placement, None)
    }
    /// Place a local pilot's `robot` at a requested FLU position and heading,
    /// using the same chassis preset and ground lookup as a joining network
    /// pilot who chose it.
    pub fn spawn_robot_at(
        &mut self,
        team: Team,
        robot: Robot,
        spawn_m: [f64; 3],
        yaw_deg: f64,
    ) -> Result<u32, String> {
        let spawner = self
            .spawner
            .as_ref()
            .ok_or("this field offers no chassis")?;
        let placement =
            spawner.at_with(robot.chassis_config(), robot.kind(), team, spawn_m, yaw_deg);
        self.place(placement, Some(robot))
    }
    fn place(
        &mut self,
        placement: rm_simulator_world::ChassisPlacement,
        robot: Option<Robot>,
    ) -> Result<u32, String> {
        let id = self
            .field
            .add_chassis(&placement)
            .map_err(|e| e.to_string())?;
        self.pilot_spawns.insert(id, placement.spawn);
        if let Some(robot) = robot {
            self.robots.insert(id, robot);
        }
        Ok(id)
    }
    /// The robot a chassis was spawned as; `None` for a bot or a chassis
    /// spawned by configuration alone.
    pub fn robot(&self, chassis: u32) -> Option<Robot> {
        self.robots.get(&chassis).copied()
    }
    /// The caliber a chassis' gun fires: its robot's, or the host default
    /// weapon's for a chassis that is no particular robot.
    pub fn caliber(&self, chassis: u32) -> rm_simulator_world::Caliber {
        self.robot(chassis)
            .map_or(self.weapon.shot.caliber, Robot::caliber)
    }
    /// The weapon a chassis fires with: the pilot's admitted settings, else
    /// the host defaults, always at the chassis' own caliber.
    pub fn weapon_for(&self, chassis: u32) -> WeaponConfig {
        let mut weapon = self
            .pilot_weapons
            .get(&chassis)
            .copied()
            .unwrap_or(self.weapon);
        weapon.shot.caliber = self.caliber(chassis);
        weapon
    }

    /// Remove a chassis with its input stream, pending shots, results, bot mark
    /// and spawn record. A later spawn takes a fresh id.
    pub fn remove_chassis(&mut self, id: u32) -> Result<(), String> {
        self.input_streams.remove(&id);
        self.cancel_pending_shots(Some(id), "shooter disconnected");
        self.shot_results.remove(&id);
        self.last_shot_ids.remove(&id);
        self.bots.remove(&id);
        self.pilot_spawns.remove(&id);
        self.fired_ns.remove(&id);
        self.pilot_weapons.remove(&id);
        self.robots.remove(&id);
        self.field.remove_chassis(id).map_err(|e| e.to_string())
    }
    /// A copy of the accepted shot journal, oldest first, at most 256 records.
    pub fn fire_records(&self) -> Vec<FireRecord> {
        self.fire_records.iter().cloned().collect()
    }
    /// The whole field snapshot.
    pub fn snapshot(&self) -> FieldSnapshot {
        self.field.snapshot()
    }

    /// The publication state: field, bots, epoch, pause flag and recent shot
    /// results. `snapshot_id` is left at zero for the host to stamp.
    pub fn state(&self) -> SimulationState {
        SimulationState {
            bots: self.bots.iter().copied().collect(),
            snapshot_id: 0,
            input_epoch: self.input_epoch,
            shot_results: self
                .shot_results
                .values()
                .flat_map(|history| history.iter().rev().take(32).cloned())
                .collect(),
            paused: self.paused,
            field: self.snapshot(),
        }
    }
    /// Turn `delta_ns` of host time into ticks (capped at [`MAX_ADVANCE_NS`])
    /// and step them unless paused. Returns the ticks stepped.
    pub fn advance(&mut self, delta_ns: u64) -> Result<u64, FieldError> {
        self.advance_observed(delta_ns, &mut |_| {})
    }
    /// Advance host time while reporting contacts before their snapshot history expires.
    pub(crate) fn advance_observed(
        &mut self,
        delta_ns: u64,
        observer: &mut dyn FnMut(&rm_simulator_world::ArmorHit),
    ) -> Result<u64, FieldError> {
        if self.paused {
            return Ok(0);
        }
        self.carry_ns += delta_ns.min(MAX_ADVANCE_NS);
        let ticks = self.carry_ns / tick_ns();
        self.carry_ns -= ticks * tick_ns();
        if ticks > 0 {
            self.step_observed(ticks, observer)?;
        }
        Ok(ticks)
    }
    /// Step exactly `ticks`, paused or not.
    pub fn step(&mut self, ticks: u64) -> Result<(), FieldError> {
        self.step_observed(ticks, &mut |_| {})
    }
    /// Keep input and shot scheduling on the ordinary ticks while streaming contacts.
    fn step_observed(
        &mut self,
        ticks: u64,
        observer: &mut dyn FnMut(&rm_simulator_world::ArmorHit),
    ) -> Result<(), FieldError> {
        if self.input_streams.is_empty() && self.pending_shots.is_empty() {
            return self.field.step_with_hits(ticks, observer);
        }
        for _ in 0..ticks {
            let now = self.field.time_ns();
            self.input_streams.retain(|id, stream| {
                let valid = stream.belongs_to(self.field.chassis_revision(*id))
                    && self.field.chassis_defeated(*id) == Some(false);
                if !valid && let Some(neutral) = stream.cancel() {
                    let _ = self.field.command_chassis(*id, neutral);
                }
                valid
            });
            for (id, stream) in &mut self.input_streams {
                if let Some(neutral) = stream.advance(now) {
                    let _ = self.field.command_chassis(*id, neutral);
                }
            }
            self.execute_pending_shots();
            self.field.step_with_hits(1, observer)?;
        }
        Ok(())
    }
    /// Pilot recovery is requested explicitly; respawning changes health only.
    fn recover_pilot(&mut self, id: u32, reset: bool) -> Result<(), String> {
        let spawn = *self
            .pilot_spawns
            .get(&id)
            .ok_or("no pilot with that chassis")?;
        if !reset && self.field.chassis_defeated(id) != Some(true) {
            return Err("robot is not defeated".into());
        }
        // A no-referee practice robot can still be repositioned through debug.
        if self.field.referee().is_some() {
            self.field
                .referee_command(rm_simulator_world::RefereeCommand::ReviveRobot { robot: id })
                .map_err(|e| e.to_string())?;
        }
        if let Some(mut stream) = self.input_streams.remove(&id)
            && let Some(neutral) = stream.cancel()
        {
            let _ = self.field.command_chassis(id, neutral);
        }
        if reset {
            self.field
                .place_chassis(id, spawn)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// The recorded result of one shot, while the shooter's last 128 results
    /// still hold it.
    pub fn shot_result(&self, shooter: u32, shot_id: u64) -> Option<&crate::protocol::ShotResult> {
        self.shot_results
            .get(&shooter)?
            .iter()
            .find(|result| result.shot_id == shot_id)
    }
    fn validate_shot(
        &self,
        shooter: u32,
        shot_id: u64,
        input: crate::input_stream::InputFrame,
    ) -> Result<(), String> {
        if shot_id == 0
            || self
                .last_shot_ids
                .get(&shooter)
                .is_some_and(|last| shot_id <= last.saturating_sub(128))
        {
            return Err("shot id is stale".into());
        }
        let revision = self
            .field
            .chassis_revision(shooter)
            .ok_or("no chassis with that id")?;
        let now = self.field.time_ns();
        if self.paused
            || self.field.chassis_defeated(shooter) != Some(false)
            || input.input_epoch != self.input_epoch
            || input.placement_revision != revision
            || !input.command.is_finite()
            || input.sequence == 0
            || now.saturating_sub(input.sampled_time_ns) > 250_000_000
            || input.sampled_time_ns > now.saturating_add(200_000_000)
        {
            return Err("shot expired or belongs to another life".into());
        }
        Ok(())
    }
    fn remember_shot(
        &mut self,
        shooter: u32,
        shot_id: u64,
        result: Result<u64, String>,
        notify: bool,
    ) {
        self.last_shot_ids
            .entry(shooter)
            .and_modify(|last| *last = (*last).max(shot_id))
            .or_insert(shot_id);
        let launch_speed_m_s = result
            .as_ref()
            .ok()
            .and_then(|id| {
                self.fire_records
                    .iter()
                    .find(|record| record.projectile_id == *id)
            })
            .map(|record| record.launch_speed_m_s);
        let result = crate::protocol::ShotResult {
            launch_speed_m_s,
            executed_time_ns: result.as_ref().ok().map(|_| self.field.time_ns()),
            shooter,
            shot_id,
            result,
        };
        let history = self.shot_results.entry(shooter).or_default();
        if history.len() == 128 {
            history.pop_front();
        }
        history.push_back(result.clone());
        if notify {
            // At most 32 pending shots per live chassis can complete in one advance.
            if self
                .completed_shots
                .iter()
                .filter(|r| r.shooter == shooter)
                .count()
                >= 128
                && let Some(index) = self
                    .completed_shots
                    .iter()
                    .position(|r| r.shooter == shooter)
            {
                self.completed_shots.remove(index);
            }
            self.completed_shots.push_back(result);
        }
    }
    /// Remove and return the shot results completed since the last call, in
    /// completion order.
    pub(crate) fn take_completed_shots(&mut self) -> VecDeque<crate::protocol::ShotResult> {
        std::mem::take(&mut self.completed_shots)
    }
    /// The intended simulation time in ns of a still-queued shot with that id
    /// and shooter.
    pub(crate) fn scheduled_shot(&self, shooter: u32, shot_id: u64) -> Option<u64> {
        self.pending_shots.iter().find_map(|c| match c {
            Command::FireAimed {
                shooter: owner,
                shot_id: id,
                input,
                ..
            } if *owner == shooter && *id == shot_id => Some(input.sampled_time_ns),
            _ => None,
        })
    }
    fn cancel_pending_shots(&mut self, shooter: Option<u32>, reason: &str) {
        let pending = std::mem::take(&mut self.pending_shots);
        for command in pending {
            if let Command::FireAimed {
                shooter: owner,
                shot_id,
                ..
            } = command
            {
                if shooter.is_none_or(|id| id == owner) {
                    self.remember_shot(owner, shot_id, Err(reason.into()), true);
                } else {
                    self.pending_shots.push(command);
                }
            }
        }
    }
    fn execute_pending_shots(&mut self) {
        let mut index = 0;
        while index < self.pending_shots.len() {
            let Command::FireAimed {
                shooter,
                shot_id,
                input,
                timing,
            } = self.pending_shots[index]
            else {
                unreachable!()
            };
            let result = match self.validate_shot(shooter, shot_id, input) {
                Err(reason) => Err(reason),
                Ok(()) if input.sampled_time_ns <= self.field.time_ns() => {
                    self.fire_aimed(shooter, shot_id, input, timing)
                }
                Ok(()) => {
                    index += 1;
                    continue;
                }
            };
            self.pending_shots.remove(index);
            self.remember_shot(shooter, shot_id, result, true);
        }
    }
    fn fire_aimed(
        &mut self,
        shooter: u32,
        shot_id: u64,
        input: crate::input_stream::InputFrame,
        timing: Option<crate::protocol::FireTiming>,
    ) -> Result<u64, String> {
        self.validate_shot(shooter, shot_id, input)?;
        let state = self
            .field
            .snapshot()
            .chassis
            .into_iter()
            .find(|c| c.id == shooter)
            .ok_or("no chassis with that id")?;
        // Only the shot's exact aim is applied; held movement and its lease are untouched.
        let aim = rm_simulator_world::ChassisCommand {
            aim_yaw_rad: input.command.aim_yaw_rad,
            aim_pitch_rad: input.command.aim_pitch_rad,
            ..state.command
        };
        self.field
            .command_chassis(shooter, aim)
            .map_err(|e| e.to_string())?;
        let result = self.fire_now(shooter, timing, input.sampled_time_ns);
        let _ = self.field.command_chassis(shooter, state.command);
        let projectile_id = result?;
        self.last_shot_ids
            .entry(shooter)
            .and_modify(|last| *last = (*last).max(shot_id))
            .or_insert(shot_id);
        Ok(projectile_id)
    }
    /// Fire the shooter's gun at the current tick. The gun's rate is enforced
    /// on `intended_ns`, the pilot's timeline: no two shots may be intended
    /// within one interval of each other. A scheduled shot that arrived late,
    /// or behind a newer one, still fires, but it neither pushes the cooldown
    /// onto the correctly spaced shot behind it nor forms a burst.
    fn fire_now(
        &mut self,
        shooter: u32,
        timing: Option<crate::protocol::FireTiming>,
        intended_ns: u64,
    ) -> Result<u64, String> {
        let weapon = self.weapon_for(shooter);
        let fired = self.fired_ns.entry(shooter).or_default();
        if fired
            .iter()
            .any(|&t| t.abs_diff(intended_ns) < weapon.interval_ns)
        {
            return Err("weapon is cooling down".into());
        }
        let muzzle = self
            .field
            .chassis_muzzle_pose(shooter)
            .ok_or("no chassis with that id")?;
        let muzzle = weapon.spread.apply(muzzle, shooter, intended_ns);
        let shot = weapon.sample_shot(shooter, intended_ns, self.weapon_limits.max_speed_m_s);
        let projectile_id = self
            .field
            .fire(muzzle, shot, Some(shooter))
            .map_err(|e| e.to_string())?;
        if self.fire_records.len() == 256 {
            self.fire_records.pop_front();
        }
        self.fire_records.push_back(FireRecord {
            launch_speed_m_s: shot.speed_m_s,
            shooter,
            projectile_id,
            accepted_time_ns: self.field.time_ns(),
            authoritative_muzzle_pose: muzzle,
            client_timing: timing,
        });
        let fired = self.fired_ns.entry(shooter).or_default();
        if fired.len() == 32 {
            fired.pop_front();
        }
        fired.push_back(intended_ns);
        Ok(projectile_id)
    }
    /// Apply a player's or operator's command at the current tick.
    pub fn apply(&mut self, command: &Command) -> Result<(), String> {
        self.apply_observed(command, &mut |_| {})
    }
    /// Apply a command, streaming contacts from explicit steps to the host.
    pub(crate) fn apply_observed(
        &mut self,
        command: &Command,
        observer: &mut dyn FnMut(&rm_simulator_world::ArmorHit),
    ) -> Result<(), String> {
        match command {
            Command::ConfigureWeapon { chassis, weapon } => {
                if self.field.chassis_muzzle_pose(*chassis).is_none() {
                    return Err("no chassis with that id".into());
                }
                let weapon = self
                    .weapon_limits
                    .admit(self.caliber(*chassis), *weapon)
                    .map_err(str::to_string)?;
                self.pilot_weapons.insert(*chassis, weapon);
                Ok(())
            }
            Command::FireAimed {
                shooter,
                shot_id,
                input,
                timing,
            } => {
                if let Some(previous) = self.shot_result(*shooter, *shot_id) {
                    return previous.result.as_ref().map(|_| ()).map_err(Clone::clone);
                }
                if let Some(previous) = self.pending_shots.iter().find(|c| matches!(c,
                    Command::FireAimed { shooter: owner, shot_id: id, .. } if owner == shooter && id == shot_id)) {
                    return if previous == command { Ok(()) } else { Err("shot id changed contents".into()) };
                }
                let admission = self.validate_shot(*shooter, *shot_id, *input);
                if let Err(reason) = admission {
                    self.remember_shot(*shooter, *shot_id, Err(reason.clone()), false);
                    return Err(reason);
                }
                if input.sampled_time_ns > self.field.time_ns() {
                    if self.pending_shots.iter().filter(|c| matches!(c, Command::FireAimed { shooter: owner, .. } if owner == shooter)).count() >= 32 {
                        let reason = "shot schedule full".to_string();
                        self.remember_shot(*shooter, *shot_id, Err(reason.clone()), false);
                        return Err(reason);
                    }
                    self.pending_shots.push(*command);
                    self.pending_shots.sort_by_key(|c| match c {
                        Command::FireAimed {
                            input,
                            shooter,
                            shot_id,
                            ..
                        } => (input.sampled_time_ns, *shooter, *shot_id),
                        _ => unreachable!(),
                    });
                    return Ok(());
                }
                let result = self.fire_aimed(*shooter, *shot_id, *input, *timing);
                self.remember_shot(*shooter, *shot_id, result.clone(), false);
                result.map(|_| ())
            }
            Command::PilotInput { chassis, frame } => {
                if frame.input_epoch != self.input_epoch {
                    return Err("input epoch expired".into());
                }
                let revision = self
                    .field
                    .snapshot()
                    .chassis
                    .into_iter()
                    .find(|c| c.id == *chassis)
                    .ok_or("no chassis with that id")?
                    .placement_revision;
                let now = self.field.time_ns();
                let stream = self.input_streams.entry(*chassis).or_default();
                if let Some(command) = stream
                    .receive(*frame, now, revision)
                    .map_err(str::to_string)?
                {
                    self.field
                        .command_chassis(*chassis, command)
                        .map_err(|e| e.to_string())?;
                }
                Ok(())
            }
            Command::PlaceChassis {
                chassis,
                position_m,
                yaw_deg,
            } => {
                if !position_m.iter().all(|v| v.is_finite()) || !yaw_deg.is_finite() {
                    return Err("placement must be finite".into());
                }
                let team = self
                    .field
                    .chassis_team(*chassis)
                    .ok_or("no chassis with that id")?;
                let spawner = self
                    .spawner
                    .as_ref()
                    .ok_or("this field offers no chassis")?;
                let placement = spawner.at(team, *position_m, *yaw_deg);
                self.input_streams.remove(chassis);
                self.field
                    .place_chassis(*chassis, placement.spawn)
                    .map_err(|e| e.to_string())
            }
            Command::Chassis { chassis, command } => self
                .field
                .command_chassis(*chassis, *command)
                .map_err(|e| e.to_string()),
            Command::SpawnBot { team, spin_rad_s } => self.spawn_bot(*team, *spin_rad_s),
            Command::RemoveBot { chassis } => {
                if !self.bots.contains(chassis) {
                    return Err("not a training bot".into());
                }
                self.remove_chassis(*chassis)
            }
            Command::ClearBots => {
                for id in self.bots.clone() {
                    self.remove_chassis(id)?;
                }
                Ok(())
            }
            Command::Respawn { chassis } => self.recover_pilot(*chassis, false),
            Command::ResetRobot { chassis } => self.recover_pilot(*chassis, true),
            Command::BuyAmmo { chassis, caliber } => self
                .field
                .buy_ammo(*chassis, *caliber)
                .map_err(|e| e.to_string()),
            Command::Fire { shooter, timing } => {
                let now = self.field.time_ns();
                self.fire_now(*shooter, *timing, now).map(|_| ())
            }
            Command::SpawnProjectile { muzzle, shot } => self
                .field
                .fire(*muzzle, *shot, None)
                .map(|_| ())
                .map_err(|e| e.to_string()),
            Command::Referee(command) => {
                self.field
                    .referee_command(*command)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            Command::Pause { paused } => {
                self.set_paused(*paused);
                Ok(())
            }
            Command::Step { ticks } => {
                if *ticks == 0 || *ticks > 60_000 {
                    return Err("step between 1 and 60000 ticks".into());
                }
                self.step_observed(*ticks, observer)
                    .map_err(|e| e.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{FieldConfig, MatchPhase, RefereeCommand, RefereeConfig};

    fn cad_without_files() -> CadAssets {
        let asset = |placements| crate::cad_assets::CadAsset {
            semantics: None,
            file: "missing-builder-test.glb".into(),
            placements,
            collision: None,
            source_tessellated_collision: false,
        };
        CadAssets {
            root: std::path::PathBuf::from("missing-builder-test-package"),
            floor_top_cad_m: crate::cad_assets::FLOOR_TOP_CAD_M,
            collision_solids: false,
            floor: asset(Vec::new()),
            arena_static: asset(Vec::new()),
            rune: asset(vec![rm_simulator_world::Pose::at([5.0, 0.0, 2.0])]),
            outpost: asset(vec![rm_simulator_world::Pose::at([3.0, 0.0, 0.0])]),
            base: asset(Vec::new()),
            tech_core: asset(Vec::new()),
            static_assets: Vec::new(),
        }
    }
    fn build_options() -> LayoutOptions {
        LayoutOptions {
            rune: Some(rm_simulator_world::RuneKind::Big),
            outpost_speed_rad_s: 1.0,
            terrain: false,
            referee: true,
            physics_rate_hz: 1000,
            projectile_policy: Default::default(),
        }
    }

    #[test]
    fn placement_preserves_identity_and_hp_and_clears_velocity() {
        let mut simulation = Simulation::from_cad(
            &cad_without_files(),
            &build_options(),
            Some(ChassisConfig::default()),
            true,
            |_| {},
        )
        .unwrap();
        let id = simulation.spawn_chassis(Team::Red).unwrap();
        simulation
            .apply(&Command::Chassis {
                chassis: id,
                command: rm_simulator_world::ChassisCommand {
                    forward_m_s: 2.0,
                    ..Default::default()
                },
            })
            .unwrap();
        simulation.step(100).unwrap();
        let robots = simulation.snapshot().referee.unwrap().robots;
        simulation
            .apply(&Command::PlaceChassis {
                chassis: id,
                position_m: [2.0, 3.0, 10.0],
                yaw_deg: 90.0,
            })
            .unwrap();
        let snapshot = simulation.snapshot();
        assert_eq!(snapshot.referee.unwrap().robots, robots);
        assert_eq!(snapshot.chassis.len(), 1);
        let chassis = &snapshot.chassis[0];
        assert_eq!(chassis.id, id);
        assert_eq!(
            chassis.pose.translation_m,
            [2.0, 3.0, chassis.config.rest_height_m() + 0.01]
        );
        assert_eq!(chassis.velocity_m_s, [0.0; 3]);
        assert_eq!(chassis.angular_velocity_rad_s, [0.0; 3]);
        assert!(chassis.wheels.iter().all(|w| w.contact.is_none()));
        assert!(
            simulation
                .apply(&Command::PlaceChassis {
                    chassis: id,
                    position_m: [f64::NAN, 0.0, 0.0],
                    yaw_deg: 0.0
                })
                .is_err()
        );
    }
    #[test]
    fn cad_construction_preserves_pause_and_rules_without_creating_a_pilot() {
        let mut events = Vec::new();
        let config = ChassisConfig::default();
        let mut simulation = Simulation::from_cad(
            &cad_without_files(),
            &build_options(),
            Some(config.clone()),
            true,
            |event| events.push(event),
        )
        .unwrap();
        // No terrain files exist: disabling collision must skip their loading.
        assert_eq!(
            events,
            [
                BuildProgress::BuildingPhysics,
                BuildProgress::PreparingChassis
            ]
        );
        assert!(simulation.paused());
        assert_eq!(simulation.advance(1_000_000).unwrap(), 0);
        let snapshot = simulation.snapshot();
        assert!(snapshot.chassis.is_empty());
        assert_eq!(snapshot.runes.len(), 2);
        assert!(
            snapshot
                .runes
                .iter()
                .all(|r| r.kind == rm_simulator_world::RuneKind::Big)
        );
        assert_eq!(snapshot.outposts.len(), 1);
        assert!(snapshot.referee.is_some());

        let local = simulation
            .spawn_chassis_at(Team::Blue, [2.0, 3.0, 10.0], 90.0)
            .unwrap();
        let peer = simulation.spawn_chassis(Team::Red).unwrap();
        let snapshot = simulation.snapshot();
        let chassis = snapshot.chassis.iter().find(|c| c.id == local).unwrap();
        assert_eq!(chassis.team, Team::Blue);
        assert_eq!(
            chassis.pose.translation_m,
            [2.0, 3.0, config.rest_height_m() + 0.01]
        );
        assert!((chassis.pose.rotation_wxyz[0] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert!((chassis.pose.rotation_wxyz[3] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert_eq!(chassis.config, config);
        assert!(
            snapshot
                .chassis
                .iter()
                .any(|c| c.id == peer && c.team == Team::Red)
        );
    }

    #[test]
    fn cad_construction_can_disable_chassis_runes_and_referee() {
        let mut options = build_options();
        options.rune = None;
        options.referee = false;
        let mut simulation =
            Simulation::from_cad(&cad_without_files(), &options, None, false, |_| {}).unwrap();
        assert!(!simulation.paused());
        assert!(simulation.spawner_config().is_none());
        assert!(simulation.spawn_chassis(Team::Red).is_err());
        assert!(
            simulation
                .spawn_chassis_at(Team::Red, [0.0; 3], 0.0)
                .is_err()
        );
        let snapshot = simulation.snapshot();
        assert!(snapshot.runes.is_empty());
        assert!(snapshot.referee.is_none());
        assert!(snapshot.chassis.is_empty());
    }

    #[test]
    fn cad_construction_propagates_terrain_and_physics_failures() {
        let mut options = build_options();
        options.terrain = true;
        let mut events = Vec::new();
        let result = Simulation::from_cad(&cad_without_files(), &options, None, false, |event| {
            events.push(event)
        });
        assert!(result.is_err());
        assert_eq!(events, [BuildProgress::ReadingTerrain]);
        options.terrain = false;
        options.outpost_speed_rad_s = f64::NAN;
        assert!(Simulation::from_cad(&cad_without_files(), &options, None, false, |_| {}).is_err());
    }

    fn simulation() -> Simulation {
        let config = FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..FieldConfig::default()
        };
        Simulation::new(Field::new(&config).unwrap(), false)
            .with_weapon(WeaponConfig {
                interval_ns: 100_000_000,
                speed_variation_m_s: 0.,
                spread: Default::default(),
                ..Default::default()
            })
            .unwrap()
    }
    fn pilot_simulation() -> (Simulation, u32) {
        let mut simulation = simulation().with_spawner(ChassisSpawner {
            config: ChassisConfig::default(),
            terrain: None,
        });
        let id = simulation.spawn_chassis(Team::Red).unwrap();
        (simulation, id)
    }

    #[test]
    fn weapon_updates_are_per_pilot_leave_flying_bullets_alone_and_keep_cooldown() {
        let (mut sim, shooter) = pilot_simulation();
        let other = sim.spawn_chassis(Team::Blue).unwrap();
        sim.apply(&Command::Fire {
            shooter,
            timing: None,
        })
        .unwrap();
        let before = sim.snapshot().projectiles;
        let mut weapon = sim.weapon();
        weapon.shot.speed_m_s = 10.;
        weapon.speed_variation_m_s = 1.;
        weapon.spread.angle_rad = 0.1;
        weapon.spread.distribution = crate::protocol::SpreadDistribution::Gaussian;
        sim.apply(&Command::ConfigureWeapon {
            chassis: shooter,
            weapon,
        })
        .unwrap();
        assert_eq!(sim.snapshot().projectiles, before);
        assert_eq!(
            sim.apply(&Command::Fire {
                shooter,
                timing: None
            })
            .unwrap_err(),
            "weapon is cooling down"
        );
        sim.apply(&Command::Fire {
            shooter: other,
            timing: None,
        })
        .unwrap();
        let other_bullet = sim.snapshot().projectiles.last().unwrap().clone();
        let speed = |v: [f64; 3]| v.iter().map(|v| v * v).sum::<f64>().sqrt();
        assert!((speed(other_bullet.velocity_m_s) - sim.weapon().shot.speed_m_s).abs() < 1e-9);
        sim.step(100).unwrap();
        let expected = weapon.spread.apply(
            sim.field().chassis_muzzle_pose(shooter).unwrap(),
            shooter,
            sim.field().time_ns(),
        );
        sim.apply(&Command::Fire {
            shooter,
            timing: None,
        })
        .unwrap();
        assert_eq!(
            sim.fire_records().last().unwrap().authoritative_muzzle_pose,
            expected
        );
        assert!(
            (speed(sim.snapshot().projectiles.last().unwrap().velocity_m_s)
                - weapon
                    .sample_shot(shooter, sim.field().time_ns(), sim.weapon().shot.speed_m_s)
                    .speed_m_s)
                .abs()
                < 1e-9
        );
        let invalid = WeaponConfig {
            interval_ns: 1,
            ..weapon
        };
        assert!(
            sim.apply(&Command::ConfigureWeapon {
                chassis: shooter,
                weapon: invalid
            })
            .is_err()
        );
        assert_eq!(sim.pilot_weapons[&shooter], weapon);
        sim.remove_chassis(shooter).unwrap();
        assert!(!sim.pilot_weapons.contains_key(&shooter));
    }

    #[test]
    fn a_hero_fires_42_mm_and_an_infantry_17_mm_whatever_the_host_default_says() {
        let mut sim = simulation().with_spawner(ChassisSpawner {
            config: ChassisConfig::default(),
            terrain: None,
        });
        let hero = sim.spawn_robot(Team::Red, Robot::Hero).unwrap();
        let infantry = sim.spawn_robot(Team::Blue, Robot::Infantry4).unwrap();
        assert_eq!(sim.robot(hero), Some(Robot::Hero));
        assert_eq!(sim.robot(infantry), Some(Robot::Infantry4));
        assert_eq!(sim.weapon().shot.caliber, rm_simulator_world::Caliber::Mm17);
        assert_eq!(sim.caliber(hero), rm_simulator_world::Caliber::Mm42);
        assert_eq!(sim.caliber(infantry), rm_simulator_world::Caliber::Mm17);
        assert_eq!(
            sim.weapon_for(hero).shot.caliber,
            rm_simulator_world::Caliber::Mm42
        );
        let placed = sim.field().chassis_config(hero).unwrap();
        assert!(placed.mecanum);
        assert!(!sim.field().chassis_config(infantry).unwrap().mecanum);
        let referee = sim.field().referee().unwrap();
        let kind = |id| referee.robots().iter().find(|r| r.id == id).unwrap().kind;
        assert_eq!(kind(hero), RobotKind::Hero);
        assert_eq!(kind(infantry), RobotKind::Infantry);

        let mut weapon = sim.weapon();
        weapon.shot = rm_simulator_world::Shot::at_limit(rm_simulator_world::Caliber::Mm17);
        assert_eq!(
            sim.apply(&Command::ConfigureWeapon {
                chassis: hero,
                weapon
            })
            .unwrap_err(),
            "caliber follows the robot"
        );
        weapon.shot = rm_simulator_world::Shot::at_limit(rm_simulator_world::Caliber::Mm42);
        sim.apply(&Command::ConfigureWeapon {
            chassis: hero,
            weapon,
        })
        .unwrap();
        assert_eq!(sim.weapon_for(hero), weapon);

        for shooter in [hero, infantry] {
            sim.apply(&Command::Fire {
                shooter,
                timing: None,
            })
            .unwrap();
        }
        let calibers: Vec<_> = sim
            .snapshot()
            .projectiles
            .iter()
            .map(|p| p.caliber)
            .collect();
        assert_eq!(
            calibers,
            [
                rm_simulator_world::Caliber::Mm42,
                rm_simulator_world::Caliber::Mm17
            ]
        );
        sim.remove_chassis(hero).unwrap();
        assert_eq!(sim.robot(hero), None);
        assert_eq!(sim.caliber(hero), rm_simulator_world::Caliber::Mm17);
    }

    #[test]
    fn scheduled_launch_uses_settings_current_at_execution() {
        let (mut sim, shooter) = pilot_simulation();
        let input = crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence: 1,
            sampled_time_ns: 16_000_000,
            duration_ticks: 16,
            placement_revision: 0,
            command: Default::default(),
        };
        sim.apply(&Command::FireAimed {
            shooter,
            shot_id: 1,
            input,
            timing: None,
        })
        .unwrap();
        assert_eq!(sim.snapshot().shots_fired, 0);
        let mut weapon = sim.weapon();
        weapon.shot.speed_m_s = 7.;
        sim.apply(&Command::ConfigureWeapon {
            chassis: shooter,
            weapon,
        })
        .unwrap();
        sim.step(17).unwrap();
        let v = sim.snapshot().projectiles[0].velocity_m_s;
        assert!((v.iter().map(|v| v * v).sum::<f64>().sqrt() - 7.).abs() < 0.02);
        assert_eq!(sim.snapshot().shots_fired, 1);
    }

    #[test]
    fn bots_spin_pause_and_remove_without_touching_pilots() {
        let (mut sim, pilot) = pilot_simulation();
        assert!(
            sim.apply(&Command::SpawnBot {
                team: Team::Blue,
                spin_rad_s: f64::NAN
            })
            .is_err()
        );
        sim.apply(&Command::SpawnBot {
            team: Team::Blue,
            spin_rad_s: 3.,
        })
        .unwrap();
        let bot = sim.state().bots[0];
        sim.step(1500).unwrap();
        let robot = sim
            .snapshot()
            .chassis
            .into_iter()
            .find(|c| c.id == bot)
            .unwrap();
        assert!(robot.angular_velocity_rad_s[2].abs() > 0.5);
        sim.set_paused(true);
        let before = sim.snapshot();
        assert_eq!(sim.advance(100_000_000).unwrap(), 0);
        assert_eq!(sim.snapshot(), before);
        assert!(sim.apply(&Command::RemoveBot { chassis: pilot }).is_err());
        sim.apply(&Command::ClearBots).unwrap();
        assert!(sim.state().bots.is_empty());
        assert_eq!(sim.snapshot().chassis.len(), 1);
        assert_eq!(sim.snapshot().chassis[0].id, pilot);
        sim.apply(&Command::SpawnBot {
            team: Team::Red,
            spin_rad_s: -3.,
        })
        .unwrap();
        assert!(sim.state().bots[0] > bot);
    }

    #[test]
    fn pilot_purchases_charge_team_gold_and_reject_atomically() {
        use rm_simulator_world::Caliber;
        let (mut sim, id) = pilot_simulation();
        let buy = |caliber| Command::BuyAmmo {
            chassis: id,
            caliber,
        };
        assert!(sim.apply(&buy(Caliber::Mm17)).is_err());
        sim.apply(&Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
        sim.step(6_001).unwrap();
        let before = sim.snapshot().referee.unwrap().gameplay;
        sim.apply(&buy(Caliber::Mm17)).unwrap();
        sim.apply(&buy(Caliber::Mm42)).unwrap();
        let after = sim.snapshot().referee.unwrap().gameplay;
        assert_eq!(after.gold[0], before.gold[0] - 11);
        assert_eq!(
            after.robots[0].allowance,
            [
                before.robots[0].allowance[0] + 1,
                before.robots[0].allowance[1] + 1
            ]
        );
        for _ in 0..after.gold[0] {
            sim.apply(&buy(Caliber::Mm17)).unwrap();
        }
        let empty = sim.snapshot();
        assert!(sim.apply(&buy(Caliber::Mm42)).is_err());
        assert_eq!(sim.snapshot(), empty);
        assert!(
            sim.apply(&Command::BuyAmmo {
                chassis: id + 1,
                caliber: Caliber::Mm17
            })
            .is_err()
        );
        assert_eq!(sim.snapshot(), empty);
    }

    #[test]
    fn debug_reset_returns_owned_robot_to_spawn_without_resetting_economy() {
        let (mut sim, id) = pilot_simulation();
        let before = sim.snapshot();
        sim.test_field_mut()
            .place_chassis(id, rm_simulator_world::Pose::at([50.0, 50.0, -2.0]))
            .unwrap();
        sim.apply(&Command::ResetRobot { chassis: id }).unwrap();
        let after = sim.snapshot();
        assert_eq!(after.chassis[0].pose, before.chassis[0].pose);
        assert_eq!(after.chassis[0].placement_revision, 2);
        assert_eq!(
            after.referee.unwrap().gameplay,
            before.referee.unwrap().gameplay
        );
        assert!(sim.apply(&Command::ResetRobot { chassis: id + 1 }).is_err());
    }

    #[test]
    fn pilot_respawn_restores_only_hp_in_place() {
        let (mut sim, id) = pilot_simulation();
        sim.apply(&Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
        sim.step(6_001).unwrap();
        sim.apply(&Command::BuyAmmo {
            chassis: id,
            caliber: rm_simulator_world::Caliber::Mm17,
        })
        .unwrap();
        sim.apply(&Command::PlaceChassis {
            chassis: id,
            position_m: [3.0, 4.0, 1.0],
            yaw_deg: 20.0,
        })
        .unwrap();
        sim.apply(&Command::Chassis {
            chassis: id,
            command: rm_simulator_world::ChassisCommand {
                forward_m_s: 1.0,
                aim_yaw_rad: 0.5,
                aim_pitch_rad: 0.2,
                ..Default::default()
            },
        })
        .unwrap();
        sim.step(10).unwrap();
        let mut before_damage = sim.snapshot();
        sim.apply(&Command::Referee(RefereeCommand::DamageRobot {
            robot: id,
            amount: u32::MAX,
        }))
        .unwrap();
        assert!(sim.snapshot().chassis[0].defeated);
        sim.apply(&Command::Respawn { chassis: id }).unwrap();
        assert!(sim.apply(&Command::Respawn { chassis: id }).is_err());
        let respawn = sim.snapshot();
        assert_eq!(respawn.time_ns, before_damage.time_ns);
        before_damage.chassis[0].placement_revision += 1;
        // Defeat brakes the gimbal motors, revival resumes from rest.
        before_damage.chassis[0].gimbal_velocity_rad_s = [0.0; 2];
        assert_eq!(respawn.chassis, before_damage.chassis);
        assert_eq!(
            respawn.chassis[0].placement_revision,
            before_damage.chassis[0].placement_revision
        );
        assert!(!respawn.chassis[0].defeated);
        let referee = respawn.referee.unwrap();
        assert_eq!(referee.robots[0].hp, referee.robots[0].max_hp);
        assert_eq!(referee.gameplay, before_damage.referee.unwrap().gameplay);
        sim.remove_chassis(id).unwrap();
        assert!(sim.pilot_spawns.is_empty());
    }

    #[test]
    fn lethal_projectile_waits_for_respawn_independent_of_step_partition() {
        use rm_simulator_world::{Caliber, Pose, Shot};
        let (mut whole, id) = pilot_simulation();
        let (mut split, _) = pilot_simulation();
        let config = ChassisConfig::default();
        for sim in [&mut whole, &mut split] {
            sim.test_field_mut()
                .place_chassis(id, Pose::at([0.0, 0.0, config.rest_height_m()]))
                .unwrap();
            sim.step(300).unwrap();
            sim.apply(&Command::Referee(RefereeCommand::SetRobotHp {
                robot: id,
                hp: 1,
            }))
            .unwrap();
            sim.apply(&Command::SpawnProjectile {
                muzzle: Pose::at([-1.5, 0.0, config.rest_height_m() + config.armor_height_m]),
                shot: Shot::at_limit(Caliber::Mm17),
            })
            .unwrap();
        }
        whole.step(200).unwrap();
        for ticks in [1, 99, 100] {
            split.step(ticks).unwrap();
        }
        let state = whole.snapshot();
        assert_eq!(state, split.snapshot());
        assert!(state.hits.iter().any(|hit| hit.detected && hit.damage > 0));
        assert_eq!(state.chassis[0].placement_revision, 1);
        assert!(state.chassis[0].defeated);
        assert_eq!(state.referee.as_ref().unwrap().robots[0].hp, 0);
        whole.apply(&Command::Respawn { chassis: id }).unwrap();
        split.apply(&Command::Respawn { chassis: id }).unwrap();
        assert_eq!(whole.snapshot(), split.snapshot());
        assert!(!whole.snapshot().chassis[0].defeated);
    }

    #[test]
    fn host_time_becomes_whole_ticks_with_a_cap_and_pause() {
        let mut simulation = simulation();
        assert_eq!(simulation.advance(1_500_000).unwrap(), 1);
        assert_eq!(simulation.advance(1_500_000).unwrap(), 2);
        assert_eq!(simulation.field().tick(), 3);
        assert_eq!(simulation.advance(10_000_000_000).unwrap(), 250);
        simulation.apply(&Command::Pause { paused: true }).unwrap();
        assert_eq!(simulation.advance(1_000_000_000).unwrap(), 0);
        simulation.apply(&Command::Step { ticks: 16 }).unwrap();
        assert_eq!(simulation.field().tick(), 269);
        assert!(simulation.apply(&Command::Step { ticks: 0 }).is_err());
        simulation.apply(&Command::Pause { paused: false }).unwrap();
        assert_eq!(simulation.advance(999_999).unwrap(), 0);
        assert_eq!(simulation.advance(1).unwrap(), 1);
    }
    #[test]
    fn commands_reach_the_field_and_report_rejections() {
        let mut simulation = simulation();
        simulation
            .apply(&Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
        assert_eq!(
            simulation.snapshot().referee.unwrap().phase,
            MatchPhase::Countdown
        );
        let rejected = simulation
            .apply(&Command::Referee(RefereeCommand::StartMatch))
            .unwrap_err();
        assert!(rejected.contains("already in progress"), "{rejected}");
        // No chassis on this field, and no spawner to make one.
        assert!(
            simulation
                .apply(&Command::Chassis {
                    chassis: 0,
                    command: Default::default()
                })
                .is_err()
        );
        assert!(simulation.spawn_chassis(Team::Red).is_err());
        simulation
            .apply(&Command::SpawnProjectile {
                muzzle: rm_simulator_world::Pose::at([0.0, 0.0, 1.0]),
                shot: rm_simulator_world::Shot::at_limit(rm_simulator_world::Caliber::Mm17),
            })
            .unwrap();
        assert_eq!(simulation.snapshot().shots_fired, 1);
    }
    #[test]
    fn the_spawner_fills_team_slots_and_frees_them_on_removal() {
        let mut simulation = simulation().with_spawner(ChassisSpawner {
            config: ChassisConfig::default(),
            terrain: None,
        });
        let red = simulation.spawn_chassis(Team::Red).unwrap();
        let red2 = simulation.spawn_chassis(Team::Red).unwrap();
        let blue = simulation.spawn_chassis(Team::Blue).unwrap();
        let chassis = simulation.snapshot().chassis;
        assert_eq!(chassis.len(), 3);
        assert!(chassis[0].pose.translation_m[0] > 0.0 && chassis[2].pose.translation_m[0] < 0.0);
        // The second red chassis sits beside the first.
        assert!((chassis[1].pose.translation_m[1] - chassis[0].pose.translation_m[1]).abs() > 0.5);
        simulation
            .apply(&Command::Chassis {
                chassis: blue,
                command: Default::default(),
            })
            .unwrap();
        simulation.remove_chassis(red).unwrap();
        assert!(simulation.remove_chassis(red).is_err());
        // The freed slot is reused by the next red player; ids are not.
        let red3 = simulation.spawn_chassis(Team::Red).unwrap();
        assert!(red3 > red2);
        let chassis = simulation.snapshot().chassis;
        assert_eq!(chassis.len(), 3);
        assert!((chassis[2].pose.translation_m[1] - chassis[0].pose.translation_m[1]).abs() > 0.5);
    }

    #[test]
    fn fire_journal_is_bounded_and_rejections_do_not_create_shots() {
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false)
            .with_spawner(ChassisSpawner {
                config: ChassisConfig::default(),
                terrain: None,
            })
            .with_weapon(WeaponConfig {
                interval_ns: 1_000_000,
                ..Default::default()
            })
            .unwrap();
        let shooter = simulation.spawn_chassis(Team::Red).unwrap();
        for _ in 0..257 {
            simulation
                .apply(&Command::Fire {
                    shooter,
                    timing: None,
                })
                .unwrap();
            assert!(
                simulation
                    .apply(&Command::Fire {
                        shooter,
                        timing: None
                    })
                    .is_err()
            );
            simulation.step(1).unwrap();
        }
        let records = simulation.fire_records();
        assert_eq!(records.len(), 256);
        assert_eq!(records.first().unwrap().accepted_time_ns, 1_000_000);
        assert_eq!(records.last().unwrap().accepted_time_ns, 256_000_000);
    }

    #[test]
    fn pilot_fire_uses_the_authoritative_muzzle_and_cadence() {
        let weapon = WeaponConfig {
            shot: rm_simulator_world::Shot {
                caliber: rm_simulator_world::Caliber::Mm42,
                speed_m_s: 30.0,
            },
            interval_ns: 10_000_000,
            speed_variation_m_s: 0.,
            spread: Default::default(),
        };
        let mut simulation = simulation()
            .with_spawner(ChassisSpawner {
                config: ChassisConfig::default(),
                terrain: None,
            })
            .with_weapon(weapon)
            .unwrap();
        let chassis = simulation.spawn_chassis(Team::Red).unwrap();
        let other = simulation.spawn_chassis(Team::Blue).unwrap();
        simulation
            .apply(&Command::Chassis {
                chassis,
                command: rm_simulator_world::ChassisCommand {
                    aim_yaw_rad: 0.7,
                    aim_pitch_rad: 0.2,
                    ..Default::default()
                },
            })
            .unwrap();
        simulation.step(1).unwrap();
        let expected = simulation.field().chassis_muzzle_pose(chassis).unwrap();
        let reported = crate::protocol::FireTiming {
            client_elapsed_ns: 123,
            estimated_simulation_time_ns: u64::MAX,
            observed_snapshot_time_ns: Some(0),
            observed_chassis_pose: Some(rm_simulator_world::Pose::at([999.; 3])),
            observed_muzzle_pose: Some(rm_simulator_world::Pose::at([-999.; 3])),
        };
        simulation
            .apply(&Command::Fire {
                shooter: chassis,
                timing: Some(reported),
            })
            .unwrap();
        let records = simulation.fire_records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].client_timing, Some(reported));
        assert_eq!(records[0].authoritative_muzzle_pose, expected);
        assert_eq!(records[0].accepted_time_ns, 1_000_000);
        let snapshot = simulation.snapshot();
        assert_eq!(records[0].projectile_id, snapshot.projectiles[0].id);
        assert_eq!(snapshot.projectiles[0].position_m, expected.translation_m);
        assert_eq!(snapshot.projectiles[0].caliber, weapon.shot.caliber);
        let speed = snapshot.projectiles[0]
            .velocity_m_s
            .iter()
            .map(|v| v * v)
            .sum::<f64>()
            .sqrt();
        assert!((speed - weapon.shot.speed_m_s).abs() < 1e-9);
        assert!(
            simulation
                .apply(&Command::Fire {
                    shooter: chassis,
                    timing: None
                })
                .is_err()
        );
        simulation.set_paused(true);
        assert_eq!(simulation.advance(u64::MAX).unwrap(), 0);
        assert!(
            simulation
                .apply(&Command::Fire {
                    shooter: chassis,
                    timing: None
                })
                .is_err()
        );
        simulation
            .apply(&Command::Fire {
                shooter: other,
                timing: None,
            })
            .unwrap();
        assert_eq!(simulation.snapshot().shots_fired, 2);
        simulation.step(9).unwrap();
        assert!(
            simulation
                .apply(&Command::Fire {
                    shooter: chassis,
                    timing: None
                })
                .is_err()
        );
        simulation.step(1).unwrap();
        simulation
            .apply(&Command::Fire {
                shooter: chassis,
                timing: None,
            })
            .unwrap();
        assert_eq!(simulation.snapshot().shots_fired, 3);
    }

    #[test]
    fn rejected_fire_does_not_consume_a_cooldown() {
        let mut simulation = simulation().with_spawner(ChassisSpawner {
            config: ChassisConfig::default(),
            terrain: None,
        });
        let chassis = simulation.spawn_chassis(Team::Blue).unwrap();
        simulation
            .test_field_mut()
            .referee_command(RefereeCommand::SetRobotHp {
                robot: chassis,
                hp: 0,
            })
            .unwrap();
        assert!(
            simulation
                .apply(&Command::Fire {
                    shooter: chassis,
                    timing: None
                })
                .is_err()
        );
        simulation
            .apply(&Command::Referee(RefereeCommand::ReviveRobot {
                robot: chassis,
            }))
            .unwrap();
        simulation
            .apply(&Command::Fire {
                shooter: chassis,
                timing: None,
            })
            .unwrap();
        assert_eq!(simulation.snapshot().shots_fired, 1);
    }
    #[test]
    fn cooldown_rejection_is_terminal_and_retries_cannot_schedule_a_shot() {
        let (mut sim, shooter) = pilot_simulation();
        let input = crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence: 1,
            sampled_time_ns: 0,
            duration_ticks: 16,
            placement_revision: sim.field().chassis_revision(shooter).unwrap(),
            command: Default::default(),
        };
        let fire = |shot_id| Command::FireAimed {
            shooter,
            shot_id,
            input,
            timing: None,
        };
        sim.apply(&fire(1)).unwrap();
        assert_eq!(sim.apply(&fire(2)).unwrap_err(), "weapon is cooling down");
        assert!(sim.shot_result(shooter, 2).unwrap().result.is_err());
        assert_eq!(sim.snapshot().shots_fired, 1);
        sim.step(
            sim.weapon
                .interval_ns
                .div_ceil(rm_simulator_world::tick_ns()),
        )
        .unwrap();
        assert_eq!(sim.apply(&fire(2)).unwrap_err(), "weapon is cooling down");
        assert_eq!(sim.snapshot().shots_fired, 1);
        // A fresh intent spaced one interval behind the first on the pilot's
        // clock fires; one that reuses the first shot's time is still cooling.
        assert_eq!(sim.apply(&fire(3)).unwrap_err(), "weapon is cooling down");
        sim.apply(&Command::FireAimed {
            shooter,
            shot_id: 4,
            input: crate::input_stream::InputFrame {
                sequence: 2,
                sampled_time_ns: sim.weapon.interval_ns,
                ..input
            },
            timing: None,
        })
        .unwrap();
        assert_eq!(sim.snapshot().shots_fired, 2);
    }

    #[test]
    fn a_late_shot_does_not_reject_the_correctly_spaced_shot_behind_it() {
        let (mut sim, shooter) = pilot_simulation();
        let interval = sim.weapon.interval_ns;
        let placement_revision = sim.field().chassis_revision(shooter).unwrap();
        let frame = move |sequence, sampled_time_ns| crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence,
            sampled_time_ns,
            duration_ticks: 16,
            placement_revision,
            command: Default::default(),
        };
        // The first intent arrives 100 ms after its intended time and fires late.
        sim.step(100_000_000 / rm_simulator_world::tick_ns())
            .unwrap();
        sim.apply(&Command::FireAimed {
            shooter,
            shot_id: 1,
            input: frame(1, 0),
            timing: None,
        })
        .unwrap();
        // The second was spaced one interval behind the first on the pilot's
        // clock and arrives on time; it is also overdue, so it fires at once.
        sim.apply(&Command::FireAimed {
            shooter,
            shot_id: 2,
            input: frame(2, interval),
            timing: None,
        })
        .unwrap();
        assert_eq!(sim.snapshot().shots_fired, 2);
        // A third, overdue intent between the two on the pilot's clock is refused.
        assert_eq!(
            sim.apply(&Command::FireAimed {
                shooter,
                shot_id: 3,
                input: frame(3, interval / 2),
                timing: None,
            })
            .unwrap_err(),
            "weapon is cooling down"
        );
        assert_eq!(sim.snapshot().shots_fired, 2);
    }

    #[test]
    fn scheduled_fire_uses_the_same_tick_moving_muzzle_once() {
        let (mut sim, shooter) = pilot_simulation();
        sim.apply(&Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
        sim.step(6_001).unwrap();
        sim.apply(&Command::BuyAmmo {
            chassis: shooter,
            caliber: rm_simulator_world::Caliber::Mm17,
        })
        .unwrap();
        let frame = crate::input_stream::InputFrame {
            input_epoch: sim.input_epoch(),
            sequence: 1,
            sampled_time_ns: sim.field.time_ns() + 64_000_000,
            duration_ticks: 16,
            placement_revision: sim.field.chassis_revision(shooter).unwrap(),
            command: rm_simulator_world::ChassisCommand {
                forward_m_s: 2.,
                aim_yaw_rad: 0.4,
                ..Default::default()
            },
        };
        sim.apply(&Command::Chassis {
            chassis: shooter,
            command: frame.command,
        })
        .unwrap();
        let fire = Command::FireAimed {
            shooter,
            shot_id: 1,
            input: frame,
            timing: None,
        };
        let allowance = sim.snapshot().referee.unwrap().gameplay.robots[0].allowance;
        sim.apply(&fire).unwrap();
        sim.apply(&fire).unwrap();
        assert_eq!(
            sim.snapshot().referee.unwrap().gameplay.robots[0].allowance,
            allowance
        );
        assert_eq!(sim.pending_shots.len(), 1);
        assert_eq!(sim.snapshot().shots_fired, 0);
        let mut changed = frame;
        changed.command.aim_yaw_rad = 1.;
        assert!(
            sim.apply(&Command::FireAimed {
                shooter,
                shot_id: 1,
                input: changed,
                timing: None
            })
            .is_err()
        );
        sim.step(64).unwrap();
        assert!(sim.shot_result(shooter, 1).is_none());
        sim.apply(&Command::PilotInput {
            chassis: shooter,
            frame,
        })
        .unwrap();
        let expected = crate::prediction::muzzle_for(&sim.snapshot().chassis[0], frame.command);
        sim.step(1).unwrap();
        let actual = sim.fire_records()[0].authoritative_muzzle_pose;
        assert!(
            actual
                .translation_m
                .into_iter()
                .zip(expected.translation_m)
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        assert!(
            actual
                .rotation_wxyz
                .into_iter()
                .zip(expected.rotation_wxyz)
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        assert_eq!(
            sim.shot_result(shooter, 1).unwrap().executed_time_ns,
            Some(frame.sampled_time_ns)
        );
        sim.apply(&fire).unwrap();
        assert_eq!(sim.snapshot().shots_fired, 1);
        assert_eq!(sim.take_completed_shots().len(), 1);
        let after = sim.snapshot().referee.unwrap().gameplay.robots[0].allowance;
        assert_eq!(after, [allowance[0] - 1, allowance[1]]);
    }
    #[test]
    fn shot_queue_is_bounded_and_cancels_across_pause_placement_and_defeat() {
        for change in 0..3 {
            let (mut sim, shooter) = pilot_simulation();
            let input = crate::input_stream::InputFrame {
                input_epoch: sim.input_epoch(),
                sequence: 1,
                sampled_time_ns: 100_000_000,
                duration_ticks: 16,
                placement_revision: sim.field.chassis_revision(shooter).unwrap(),
                command: Default::default(),
            };
            for shot_id in 1..=32 {
                sim.apply(&Command::FireAimed {
                    shooter,
                    shot_id,
                    input,
                    timing: None,
                })
                .unwrap();
            }
            assert_eq!(
                sim.apply(&Command::FireAimed {
                    shooter,
                    shot_id: 33,
                    input,
                    timing: None
                })
                .unwrap_err(),
                "shot schedule full"
            );
            match change {
                0 => {
                    sim.set_paused(true);
                    sim.set_paused(false);
                }
                1 => {
                    sim.apply(&Command::PlaceChassis {
                        chassis: shooter,
                        position_m: [0., 0., 1.],
                        yaw_deg: 0.,
                    })
                    .unwrap();
                }
                _ => {
                    sim.apply(&Command::Referee(
                        rm_simulator_world::RefereeCommand::SetRobotHp {
                            robot: shooter,
                            hp: 0,
                        },
                    ))
                    .unwrap();
                }
            }
            sim.step(150).unwrap();
            assert!(sim.pending_shots.is_empty());
            assert_eq!(sim.snapshot().shots_fired, 0);
            assert_eq!(sim.take_completed_shots().len(), 32);
            assert!(sim.shot_result(shooter, 1).unwrap().result.is_err());
        }
    }
    #[test]
    fn same_tick_queued_shots_do_not_form_a_burst_and_partitioning_is_stable() {
        fn run(parts: &[u64]) -> (Vec<FireRecord>, Vec<crate::protocol::ShotResult>) {
            let (mut sim, shooter) = pilot_simulation();
            for shot_id in (1..=3).rev() {
                let input = crate::input_stream::InputFrame {
                    input_epoch: sim.input_epoch(),
                    sequence: shot_id,
                    sampled_time_ns: 50_000_000,
                    duration_ticks: 16,
                    placement_revision: sim.field.chassis_revision(shooter).unwrap(),
                    command: Default::default(),
                };
                sim.apply(&Command::FireAimed {
                    shooter,
                    shot_id,
                    input,
                    timing: None,
                })
                .unwrap();
            }
            for part in parts {
                sim.step(*part).unwrap();
            }
            (sim.fire_records(), sim.take_completed_shots().into())
        }
        let a = run(&[100]);
        let b = run(&[25, 25, 1, 49]);
        assert_eq!(a, b);
        assert_eq!(a.0.len(), 1);
        assert_eq!(a.1.iter().filter(|r| r.result.is_err()).count(), 2);
    }

    #[test]
    fn reordered_fresh_shots_are_deduplicated_without_sequence_head_of_line_loss() {
        let (mut sim, shooter) = pilot_simulation();
        let frame = crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence: 1,
            sampled_time_ns: 0,
            duration_ticks: 16,
            placement_revision: sim.field().chassis_revision(shooter).unwrap(),
            command: Default::default(),
        };
        // Each shot has its own intended time; the newer one arrives first.
        let interval = sim.weapon.interval_ns;
        let command = move |shot_id| Command::FireAimed {
            shooter,
            shot_id,
            input: crate::input_stream::InputFrame {
                sequence: shot_id,
                sampled_time_ns: (shot_id - 1) * interval,
                ..frame
            },
            timing: None,
        };
        sim.apply(&command(2)).unwrap();
        sim.step(2 * interval / rm_simulator_world::tick_ns())
            .unwrap();
        assert_eq!(sim.snapshot().shots_fired, 1);
        sim.apply(&command(1)).unwrap();
        sim.apply(&command(1)).unwrap();
        sim.apply(&command(2)).unwrap();
        assert_eq!(sim.snapshot().shots_fired, 2);
    }

    #[test]
    fn outage_shots_expire_without_backlog_bursts_or_movement_refresh() {
        let (mut sim, shooter) = pilot_simulation();
        let mut input = crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence: 1,
            sampled_time_ns: 0,
            duration_ticks: 16,
            placement_revision: sim.field().chassis_revision(shooter).unwrap(),
            command: rm_simulator_world::ChassisCommand {
                forward_m_s: 2.,
                aim_yaw_rad: 0.5,
                ..Default::default()
            },
        };
        sim.step(300).unwrap();
        assert!(
            sim.apply(&Command::FireAimed {
                shooter,
                shot_id: 1,
                input,
                timing: None
            })
            .is_err()
        );
        assert_eq!(sim.snapshot().shots_fired, 0);
        input.sampled_time_ns = sim.field().time_ns();
        let fire = Command::FireAimed {
            shooter,
            shot_id: 2,
            input,
            timing: None,
        };
        sim.apply(&fire).unwrap();
        sim.apply(&fire).unwrap();
        assert_eq!(sim.snapshot().shots_fired, 1);
        assert_eq!(sim.snapshot().chassis[0].command.forward_m_s, 0.);
        assert!(sim.input_streams.is_empty());
        assert!(
            sim.apply(&Command::FireAimed {
                shooter,
                shot_id: 3,
                input,
                timing: None
            })
            .is_err()
        );
        assert_eq!(sim.snapshot().shots_fired, 1);
        assert!(!sim.snapshot().projectiles.is_empty());
    }

    #[test]
    fn revival_without_fresh_packets_neutralizes_the_previous_drive() {
        for referee in [false, true] {
            let (mut sim, chassis) = pilot_simulation();
            let frame = crate::input_stream::InputFrame {
                input_epoch: 0,
                sequence: 1,
                sampled_time_ns: 0,
                duration_ticks: 16,
                placement_revision: sim.field().chassis_revision(chassis).unwrap(),
                command: rm_simulator_world::ChassisCommand {
                    forward_m_s: 2.,
                    ..Default::default()
                },
            };
            sim.apply(&Command::PilotInput { chassis, frame }).unwrap();
            sim.apply(&Command::Referee(RefereeCommand::SetRobotHp {
                robot: chassis,
                hp: 0,
            }))
            .unwrap();
            sim.apply(&if referee {
                Command::Referee(RefereeCommand::ReviveRobot { robot: chassis })
            } else {
                Command::Respawn { chassis }
            })
            .unwrap();
            sim.step(300).unwrap();
            assert_eq!(sim.snapshot().chassis[0].command.forward_m_s, 0.);
        }
    }

    #[test]
    fn queued_and_delayed_inputs_and_shots_cannot_cross_pause_epochs() {
        let (mut sim, chassis) = pilot_simulation();
        let mut frame = crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence: 1,
            sampled_time_ns: 100_000_000,
            duration_ticks: 16,
            placement_revision: sim.field().chassis_revision(chassis).unwrap(),
            command: rm_simulator_world::ChassisCommand {
                forward_m_s: 2.,
                ..Default::default()
            },
        };
        sim.apply(&Command::PilotInput { chassis, frame }).unwrap();
        sim.set_paused(true);
        sim.set_paused(false);
        assert!(sim.apply(&Command::PilotInput { chassis, frame }).is_err());
        assert!(
            sim.apply(&Command::FireAimed {
                shooter: chassis,
                shot_id: 1,
                input: frame,
                timing: None
            })
            .is_err()
        );
        sim.step(150).unwrap();
        assert_eq!(sim.snapshot().chassis[0].command.forward_m_s, 0.);
        frame.input_epoch = sim.input_epoch();
        frame.sampled_time_ns = sim.field().time_ns();
        frame.sequence += 1;
        sim.apply(&Command::PilotInput { chassis, frame }).unwrap();
        assert_eq!(sim.snapshot().chassis[0].command.forward_m_s, 2.);
    }

    #[test]
    fn future_controls_cannot_cross_a_reset_or_revived_life() {
        let (mut sim, chassis) = pilot_simulation();
        let frame = crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence: 1,
            sampled_time_ns: 100_000_000,
            duration_ticks: 16,
            placement_revision: sim.field().chassis_revision(chassis).unwrap(),
            command: rm_simulator_world::ChassisCommand {
                forward_m_s: 2.,
                ..Default::default()
            },
        };
        sim.apply(&Command::PilotInput { chassis, frame }).unwrap();
        sim.apply(&Command::ResetRobot { chassis }).unwrap();
        sim.step(150).unwrap();
        assert_eq!(sim.snapshot().chassis[0].command.forward_m_s, 0.);
        assert!(sim.apply(&Command::PilotInput { chassis, frame }).is_err());
    }

    #[test]
    fn aimed_fire_uses_its_own_input_without_rewinding_newer_held_aim() {
        let (mut simulation, shooter) = pilot_simulation();
        let revision = simulation.snapshot().chassis[0].placement_revision;
        let input = |sequence, yaw| crate::input_stream::InputFrame {
            input_epoch: 0,
            sequence,
            sampled_time_ns: simulation.field().time_ns(),
            duration_ticks: 16,
            placement_revision: revision,
            command: rm_simulator_world::ChassisCommand {
                aim_yaw_rad: yaw,
                ..Default::default()
            },
        };
        let earlier = input(1, 0.25);
        let newer = input(2, 1.25);
        simulation
            .apply(&Command::PilotInput {
                chassis: shooter,
                frame: newer,
            })
            .unwrap();
        let fire = Command::FireAimed {
            shooter,
            shot_id: 1,
            input: earlier,
            timing: None,
        };
        simulation.apply(&fire).unwrap();
        let shot = simulation.fire_records()[0].clone();
        let expected =
            crate::prediction::muzzle_for(&simulation.snapshot().chassis[0], earlier.command);
        assert!(
            shot.authoritative_muzzle_pose
                .translation_m
                .into_iter()
                .zip(expected.translation_m)
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        assert!(
            shot.authoritative_muzzle_pose
                .rotation_wxyz
                .into_iter()
                .zip(expected.rotation_wxyz)
                .all(|(a, b)| (a - b).abs() < 1e-12)
        );
        assert_eq!(simulation.snapshot().chassis[0].command, newer.command);
        simulation.apply(&fire).unwrap();
        assert_eq!(simulation.fire_records().len(), 1);
        assert_eq!(simulation.snapshot().shots_fired, 1);
    }
}
