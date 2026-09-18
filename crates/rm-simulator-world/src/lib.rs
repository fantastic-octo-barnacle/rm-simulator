// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Deterministic RoboMaster field objects on an explicit 1 ms clock.
//! No renderer, host clock, transport or background threads. Projectiles fly in
//! the shared `rm-simulator-physics` world. This facade coordinates its contacts and
//! motion with live rules, scoring, referee decisions and complete restore.
#![deny(missing_docs)]

pub mod base;
pub use base::{BaseConfig, BaseSnapshot};
pub mod chassis;
pub mod outpost;
pub mod projectile;
pub mod referee;
pub mod rune;
mod scoring;
pub mod zones;
pub use chassis::{ChassisCommand, ChassisConfig, ChassisSnapshot, WheelContact, WheelSnapshot};
pub use outpost::{Outpost, OutpostSnapshot};
pub use projectile::{
    ArmorHit, ArmorTarget, Caliber, ProjectileSnapshot, Rejection, SMALL_ARMOR_HOUSING_HALF_M,
    Shot, StaticGeometry,
};
use projectile::{Contact, HIT_MEMORY_NS, TargetFace, TargetFrames, WorldPhysics};
pub use referee::{
    BuffState, MatchPhase, Referee, RefereeCommand, RefereeConfig, RefereeEvent, RefereeSnapshot,
    RobotConfig, RobotKind, RobotSnapshot, RuneStage, StampClock, Team, TeamSnapshot, TimedEvent,
};
use rune::{BigRune, RuneError, SmallRune};
pub use rune::{BigRuneMotion, HitOutcome, Rune, RuneKind, RuneSnapshot, RuneState};
use serde::{Deserialize, Serialize};

/// The match engine the referee runs, so callers can read
/// [`RefereeSnapshot::game`] without depending on it directly.
pub use rm_simulator_gameplay as gameplay;
pub use rm_simulator_physics::{Pose, tick_ns};

/// One armor module pose on a rotating mechanism, keyed by face index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ArmorSnapshot {
    /// Face index in the mechanism's own order.
    pub id: u32,
    /// Face centre in world FLU metres; +x is the outward scoring normal.
    pub pose: Pose,
}

/// One Power Rune in the layout: which mode it starts in and where its hub sits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuneConfig {
    /// Rune mode. The referee's three-minute stage change converts every rune
    /// to Big (section 5.5.2).
    pub kind: RuneKind,
    /// Face centre; local +x points into the wheel, away from the shooter.
    /// The scoring front faces local -x; local +z is up.
    pub hub_pose: Pose,
    /// Use the measured 698.5 mm CAD target orbit instead of the nominal 700 mm.
    pub cad_orbit: bool,
}
impl Default for RuneConfig {
    fn default() -> Self {
        Self {
            kind: RuneKind::Small,
            hub_pose: Pose::at([6.0, 0.0, 1.6]),
            cad_orbit: false,
        }
    }
}

/// One outpost in the layout: where the tower base stands, how fast it turns
/// and, when the asset supplied one, its verified rotor pivot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutpostConfig {
    /// Optional verified pivot in CAD glTF metres; legacy configurations use the fitted pivot.
    #[serde(default)]
    pub pivot_cad_m: Option<[f64; 3]>,
    /// Tower base pose: translation is the base on the floor, rotation maps the
    /// tower's local FLU frame (forward = CAD +x) into the world.
    pub origin: Pose,
    /// Rotor speed counter-clockwise about the tower's up axis. Section 5.5.1
    /// gives 0.8π rad/s; the constructor rejects magnitudes above 10 rad/s.
    pub speed_rad_s: f64,
}

/// A driven chassis, whose team it plays for and where it starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChassisPlacement {
    /// Preset dimensions, suspension, armor geometry and drivetrain budget.
    pub config: ChassisConfig,
    /// Body centre pose; the wheels find the ground on the first tick.
    pub spawn: Pose,
    /// Team whose referee record, base and buffs this chassis belongs to.
    pub team: Team,
    /// Robot class the referee records for it; a placement that names none
    /// is an infantry, which is what older serialized layouts hold.
    #[serde(default)]
    pub kind: RobotKind,
    /// Section 5.4.2 performance type the robot selected; `None` takes the
    /// class default (a long-range Hero, an HP-focused cooling-focused Infantry).
    #[serde(default)]
    pub performance: Option<rm_simulator_gameplay::Performance>,
}

/// Static field layout. Everything here is placed once; motion comes from the clock.
///
/// ```rust
/// use rm_simulator_world::FieldConfig;
///
/// // The default layout is one Small Rune and an outpost on either side.
/// let config = FieldConfig::default();
/// assert_eq!(config.runes.len(), 1);
/// assert_eq!(config.outposts.len(), 2);
/// assert_eq!(config.outposts[0].origin.translation_m, [4.0, -3.0, 0.0]);
/// assert_eq!(config.floor_height_m, 0.0);
/// assert!(config.bases.is_empty());
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldConfig {
    /// Live bases. `ArmorTarget::Base` indexes this list; empty means no base
    /// is simulated and none can be destroyed.
    #[serde(default)]
    pub bases: Vec<BaseConfig>,
    /// Power Runes in referee index order, so `RefereeConfig::rune_teams`
    /// must name an owner for each.
    pub runes: Vec<RuneConfig>,
    /// Outposts in referee index order, so `RefereeConfig::outpost_teams`
    /// must name an owner for each.
    pub outposts: Vec<OutpostConfig>,
    /// Height of the flat floor plane. Zero is the playing floor; a field
    /// that supplies its own ground mesh lowers it to a catch surface.
    pub floor_height_m: f64,
    /// Chassis present from the start, in id order; more can join and
    /// leave with [`Field::add_chassis`] and [`Field::remove_chassis`].
    #[serde(default)]
    pub chassis: Vec<ChassisPlacement>,
    /// Optional match referee. Without one, runes use training policy and
    /// base/outpost damage still applies, but no referee tracks robot HP or a match.
    #[serde(default)]
    pub referee: Option<RefereeConfig>,
    /// Ball restitution, hard flight limit and low-speed retirement rule. The
    /// default is the simulator's control behaviour. A host and every client
    /// predicting for it must use the same value, so it travels in the
    /// snapshot's [`FieldRestore`] as well.
    #[serde(default)]
    pub projectile_policy: projectile::ProjectilePolicy,
    /// Buff point footprints a running match reports robot contacts for;
    /// empty reports none. [`zones::rmuc_2026`] is the full field's set.
    #[serde(default)]
    pub zones: Vec<zones::ZoneArea>,
}

impl Default for FieldConfig {
    fn default() -> Self {
        Self {
            zones: Vec::new(),
            bases: Vec::new(),
            floor_height_m: 0.0,
            chassis: Vec::new(),
            referee: None,
            projectile_policy: projectile::ProjectilePolicy::default(),
            runes: vec![RuneConfig::default()],
            outposts: [-1.0, 1.0]
                .into_iter()
                .map(|side| OutpostConfig {
                    pivot_cad_m: None,
                    origin: Pose::at([4.0, side * 3.0, 0.0]),
                    speed_rad_s: outpost::DEFAULT_SPEED_RAD_S,
                })
                .collect(),
        }
    }
}

/// Everything a `Field` can refuse: an invalid layout, a launch the referee
/// blocks, a physics step that cannot resolve a target frame, or a restore
/// whose snapshot disagrees with the geometry on hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FieldError {
    /// A rune pose or motion parameter failed validation (section 5.5.2 bounds).
    #[error("invalid rune configuration: {0}")]
    Rune(#[from] RuneError),
    /// An outpost origin, pivot or speed failed validation.
    #[error("invalid outpost configuration: {0}")]
    Outpost(&'static str),
    /// The shot could not be launched, for example at a defeated shooter.
    #[error("invalid shot: {0}")]
    Shot(&'static str),
    /// An unknown chassis id or a rejected chassis placement.
    #[error("invalid chassis: {0}")]
    Chassis(&'static str),
    /// A collision mesh or projectile bound was rejected.
    #[error("invalid collision mesh: {0}")]
    Mesh(&'static str),
    /// The physics step could not resolve its scoring target frames.
    #[error("projectile stepping failed: {0}")]
    Ballistics(&'static str),
    /// `step` would carry the tick counter past the range of the nanosecond clock.
    #[error("field tick overflow")]
    TickOverflow,
    /// The referee refused the command, or the field has no referee.
    #[error("referee: {0}")]
    Referee(&'static str),
    /// The snapshot cannot rebuild a field: rule state, floor height or HP is wrong.
    #[error("cannot restore the field: {0}")]
    Restore(&'static str),
}

/// The rules' own bookkeeping, beyond what an observer needs: rune activation
/// progress and its seeded stream, outpost rotor stops, the referee's schedule
/// and configuration, and the physics identity counters. [`Field::restore`]
/// needs it; a snapshot that lost it can still be drawn.
///
/// What it deliberately cannot carry is solver state. Rapier's contact
/// manifolds, warm starts and each ball's touched-armor set are rebuilt from
/// the restored bodies, so a restored field's first ticks differ from the
/// original's by the solver's warm-start residual, and a ball already resting
/// against armor registers that touch once more.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldRestore {
    /// Every rune's activation state, held angle and seeded target stream.
    pub runes: Vec<Rune>,
    /// Every outpost's rotor stop and HP.
    pub outposts: Vec<Outpost>,
    /// The referee's schedule, teams, robots and buffs, when the field has one.
    pub referee: Option<Referee>,
    /// Next id [`Field::add_chassis`] would hand out; ids are never reused.
    pub next_chassis_id: u32,
    /// Per-module time of the last detected strike, oldest kept, in target
    /// order, so the detection intervals of section 5.1.1 survive a restore.
    pub last_detection_ns: Vec<(ArmorTarget, u64)>,
    /// The projectile restitution, flight limit and retirement rule the field
    /// runs, so a restored field spends and retires balls exactly as the field
    /// it was captured from does.
    pub projectile_policy: projectile::ProjectilePolicy,
}

/// One authoritative tick of the whole field, as an observer or a peer
/// receives it. `Field::restore` rebuilds a field from one, so it carries the
/// rules' hidden state as well as what a viewer draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSnapshot {
    /// Every base's fitted geometry, HP and shield.
    pub bases: Vec<BaseSnapshot>,
    /// Ticks elapsed since the field was built.
    pub tick: u64,
    /// `tick` times `tick_ns()`, the field's own clock in nanoseconds.
    pub time_ns: u64,
    /// One frame per rune, in referee index order.
    pub runes: Vec<RuneSnapshot>,
    /// One frame per outpost, in referee index order.
    pub outposts: Vec<OutpostSnapshot>,
    /// Balls still in flight at this tick.
    pub projectiles: Vec<ProjectileSnapshot>,
    /// Every chassis on the field, in id order.
    pub chassis: Vec<ChassisSnapshot>,
    /// Armor contacts from the last second, oldest first.
    pub hits: Vec<ArmorHit>,
    /// Balls launched since the field was built.
    pub shots_fired: u64,
    /// Contacts that passed every detection condition, rejected ones excluded.
    pub hits_detected: u64,
    /// The match state, when the field has a referee.
    pub referee: Option<RefereeSnapshot>,
    /// Hidden rule state, for [`Field::restore`]. Present on every snapshot a
    /// field takes; a decoder that drops it gives up restoring, not drawing.
    pub restore: Option<FieldRestore>,
}

/// The complete field: one physics world holding projectiles and chassis, with
/// the runes, outposts, bases and optional referee on top. Reading a snapshot
/// never advances time.
///
/// ```rust
/// use rm_simulator_world::{Field, FieldConfig, tick_ns};
///
/// let mut field = Field::new(&FieldConfig::default()).unwrap();
/// assert_eq!(field.tick(), 0);
/// assert_eq!(field.time_ns(), 0);
/// // Time moves only through explicit ticks of tick_ns() nanoseconds.
/// field.step(1).unwrap();
/// assert_eq!(field.tick(), 1);
/// assert_eq!(field.time_ns(), tick_ns());
/// ```
pub struct Field {
    bases: Vec<BaseSnapshot>,
    floor_height_m: f64,
    tick: u64,
    runes: Vec<Rune>,
    outposts: Vec<Outpost>,
    physics: WorldPhysics,
    target_frames: TargetFrames,
    hits: Vec<ArmorHit>,
    hits_detected: u64,
    last_detection_ns: std::collections::HashMap<ArmorTarget, u64>,
    referee: Option<Referee>,
    zones: Vec<zones::ZoneArea>,
}
impl Field {
    /// Build a field from a layout, validating every rune, outpost and base.
    /// Chassis listed in the config join immediately, in order, so their ids
    /// match their index. The clock starts at tick zero.
    ///
    /// ```rust
    /// use rm_simulator_world::{Field, FieldConfig, Pose, RuneConfig, RuneKind};
    ///
    /// let config = FieldConfig {
    ///     runes: vec![RuneConfig {
    ///         kind: RuneKind::Big,
    ///         hub_pose: Pose::at([6.0, 0.0, 1.6]),
    ///         cad_orbit: false,
    ///     }],
    ///     outposts: vec![],
    ///     ..FieldConfig::default()
    /// };
    /// let field = Field::new(&config).unwrap();
    /// assert_eq!(field.runes().len(), 1);
    /// assert_eq!(field.runes()[0].kind(), RuneKind::Big);
    /// assert!(field.referee().is_none());
    /// ```
    pub fn new(config: &FieldConfig) -> Result<Self, FieldError> {
        for base in &config.bases {
            base.validate().map_err(FieldError::Mesh)?;
        }
        let runes = config
            .runes
            .iter()
            .map(|rune| {
                Ok(match rune.kind {
                    RuneKind::Small if rune.cad_orbit => {
                        Rune::Small(SmallRune::from_cad(rune.hub_pose)?)
                    }
                    RuneKind::Small => Rune::Small(SmallRune::new(rune.hub_pose)?),
                    RuneKind::Big => Rune::Big(BigRune::new(
                        rune.hub_pose,
                        rune.cad_orbit,
                        BigRuneMotion::default(),
                    )?),
                })
            })
            .collect::<Result<Vec<_>, RuneError>>()?;
        let outposts = config
            .outposts
            .iter()
            .map(|outpost| {
                Outpost::with_pivot(
                    outpost.origin,
                    outpost.speed_rad_s,
                    outpost.pivot_cad_m.unwrap_or(outpost::PIVOT_CAD_M),
                )
                .map_err(FieldError::Outpost)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let referee = config
            .referee
            .as_ref()
            .map(|referee| {
                Referee::new(
                    referee.clone(),
                    runes.iter().map(Rune::kind).collect(),
                    outposts.len(),
                )
            })
            .transpose()
            .map_err(FieldError::Referee)?;
        if referee.is_some()
            && Team::BOTH
                .iter()
                .any(|team| config.bases.iter().filter(|b| b.team == *team).count() > 1)
        {
            return Err(FieldError::Referee("a team can have at most one base"));
        }
        let bases: Vec<_> = config
            .bases
            .iter()
            .cloned()
            .map(BaseSnapshot::new)
            .collect();
        let mut faces = Vec::new();
        Self::write_target_faces(&runes, &outposts, &bases, 0, &mut faces);
        let mut field = Self {
            bases,
            floor_height_m: config.floor_height_m,
            tick: 0,
            runes,
            outposts,
            physics: {
                let mut physics = WorldPhysics::new(&faces, config.floor_height_m);
                physics
                    .set_projectile_policy(config.projectile_policy)
                    .map_err(FieldError::Shot)?;
                physics
            },
            target_frames: TargetFrames::new(faces),
            hits: Vec::new(),
            hits_detected: 0,
            last_detection_ns: std::collections::HashMap::new(),
            referee,
            zones: config.zones.clone(),
        };
        for outpost in &field.outposts {
            let (pose, size) = outpost.body_proxy();
            field.physics.add_static_box(pose, size);
        }
        for chassis in &config.chassis {
            field.add_chassis(chassis)?;
        }
        Ok(field)
    }
    /// Refill scoring faces in their fixed construction order, keeping capacity.
    fn write_target_faces(
        runes: &[Rune],
        outposts: &[Outpost],
        bases: &[BaseSnapshot],
        time_ns: u64,
        faces: &mut Vec<TargetFace>,
    ) {
        faces.clear();
        for (base, state) in bases.iter().enumerate() {
            for plate in 0..7 {
                faces.push(TargetFace {
                    target: ArmorTarget::Base {
                        base: base as u32,
                        plate: plate as u32,
                    },
                    pose: state.pose(plate, time_ns),
                });
            }
        }
        for (index, outpost) in outposts.iter().enumerate() {
            for (face, pose) in outpost.armor_poses(time_ns).into_iter().enumerate() {
                faces.push(TargetFace {
                    target: ArmorTarget::Outpost {
                        outpost: index as u32,
                        face: face as u32,
                    },
                    pose,
                });
            }
        }
        for (index, rune) in runes.iter().enumerate() {
            for (blade, pose) in rune.target_poses().into_iter().enumerate() {
                faces.push(TargetFace {
                    target: ArmorTarget::Rune {
                        rune: index as u32,
                        blade: blade as u32,
                    },
                    pose: rune::scoring_pose(pose),
                });
            }
        }
    }
    /// Add a fixed collision mesh (world FLU metres), for example the field CAD.
    pub fn add_static_mesh(
        &mut self,
        vertices_m: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> Result<(), FieldError> {
        self.physics
            .add_static_mesh(vertices_m, triangles)
            .map(|_| ())
            .map_err(FieldError::Mesh)
    }
    /// Register an operator-driven mesh at its exported rest pose, with world
    /// FLU translations for the closed and open endpoints. No inferred rules.
    pub fn add_mechanism_mesh(
        &mut self,
        vertices_m: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
        mechanism: referee::Mechanism,
        team: Team,
        offsets_m: [[f64; 3]; 2],
    ) -> Result<(), FieldError> {
        self.physics
            .add_mechanism_mesh(vertices_m, triangles, mechanism, team, offsets_m)
            .map_err(FieldError::Mesh)?;
        self.sync_mechanisms();
        Ok(())
    }
    /// The fixed collision geometry the physics actually holds (meshes
    /// and boxes; not the catch floor), as one triangle soup in
    /// world FLU metres, for debugging views.
    pub fn static_geometry(&self) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        self.static_geometry_snapshot().into_triangles()
    }
    /// Robot containment mesh ignored by projectile collision queries.
    /// Pair with `set_projectile_bounds` to discard shots at the perimeter.
    pub fn add_boundary_mesh(
        &mut self,
        vertices_m: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> Result<(), FieldError> {
        self.physics
            .add_boundary_mesh(vertices_m, triangles)
            .map_err(FieldError::Mesh)
    }
    /// Absorb projectiles touching or leaving the arena's XY perimeter at any height.
    /// This is a simulator setting. Robot collision walls remain ordinary geometry.
    pub fn set_projectile_bounds(&mut self, bounds_m: [[f64; 3]; 2]) -> Result<(), FieldError> {
        if !bounds_m.iter().flatten().all(|v| v.is_finite())
            || (0..2).any(|i| bounds_m[0][i] >= bounds_m[1][i])
        {
            return Err(FieldError::Mesh("invalid projectile bounds"));
        }
        self.physics.projectile_bounds_m = Some(bounds_m);
        Ok(())
    }
    /// CAD-backed arenas use their own floor mesh and perimeter walls.
    pub fn remove_catch_floor(&mut self) {
        self.physics.remove_catch_floor();
    }
    /// Configured catch-plane height; CAD arenas may have removed that plane.
    pub fn floor_height_m(&self) -> f64 {
        self.floor_height_m
    }
    /// Capture shared shapes without copying mesh buffers or advancing time.
    pub fn static_geometry_snapshot(&self) -> StaticGeometry {
        self.physics.static_geometry_snapshot(true)
    }
    /// Current chassis, armor and projectile collider shapes in world coordinates.
    /// Fixed debug geometry excludes articulated equipment captured each frame.
    pub fn fixed_geometry_snapshot(&self) -> StaticGeometry {
        self.physics.static_geometry_snapshot(false)
    }
    /// Chassis, armor, projectile and mechanism collider shapes at the current
    /// tick, in world FLU metres. Fixed terrain is excluded.
    pub fn dynamic_geometry_snapshot(&self) -> StaticGeometry {
        self.physics.dynamic_geometry_snapshot()
    }

    /// Launch a projectile along the muzzle's +x at the current tick,
    /// credited to the chassis `shooter` when a pilot fires. With a referee a
    /// defeated, weakened or overheated robot cannot fire, a robot fires only
    /// its own caliber, and a running round charges heat, allowance and launch
    /// experience. Returns the ball's identity.
    ///
    /// ```rust
    /// use rm_simulator_world::{BaseConfig, Caliber, Field, FieldConfig, Pose, Shot, Team};
    ///
    /// # let base = BaseConfig {
    /// #     team: Team::Red,
    /// #     plates: std::array::from_fn(|i| Pose::at([0.0, i as f64 * 0.5, 1.0])),
    /// #     dart_offsets_m: [[0.0; 3]; 2],
    /// # };
    /// # let mut field = Field::new(&FieldConfig {
    /// #     bases: vec![base],
    /// #     runes: vec![],
    /// #     outposts: vec![],
    /// #     ..FieldConfig::default()
    /// # })
    /// # .unwrap();
    /// // The muzzle faces the base plate 0.5 m ahead of it; +x is the barrel.
    /// let ball = field
    ///     .fire(
    ///         Pose::yawed([0.5, 0.0, 1.002], std::f64::consts::PI),
    ///         Shot::at_limit(Caliber::Mm17),
    ///         None,
    ///     )
    ///     .unwrap();
    /// field.step(100).unwrap();
    ///
    /// let hit = field
    ///     .snapshot()
    ///     .hits
    ///     .into_iter()
    ///     .find(|hit| hit.projectile == ball)
    ///     .unwrap();
    /// assert!(hit.detected);
    /// // 20 HP for a non-frontal plate, doubled to 150 % inside the centre square.
    /// assert_eq!(hit.damage, 30);
    /// ```
    pub fn fire(
        &mut self,
        muzzle: Pose,
        shot: Shot,
        shooter: Option<u32>,
    ) -> Result<u64, FieldError> {
        if let Some(referee) = &self.referee {
            referee
                .check_launch(shooter, shot.caliber)
                .map_err(FieldError::Referee)?;
        }
        let id = self
            .physics
            .fire(self.time_ns(), muzzle, shot, shooter)
            .map_err(FieldError::Shot)?;
        if let Some(referee) = &mut self.referee {
            referee.record_launch(shooter, shot.caliber);
        }
        Ok(id)
    }
    /// Immediate pilot resupply with team gold during a running round: one
    /// Table 5-6 exchange unit, ten 17 mm rounds or one 42 mm round.
    pub fn buy_ammo(&mut self, chassis: u32, caliber: Caliber) -> Result<(), FieldError> {
        self.referee_command(RefereeCommand::BuyAmmo {
            robot: chassis,
            caliber,
            amount: match caliber {
                Caliber::Mm17 => 10,
                Caliber::Mm42 => 1,
            },
        })
    }

    /// Put a chassis on the field and return its id; ids count up and are
    /// never reused. The referee, when there is one, opens a robot record
    /// under the same id.
    ///
    /// ```rust
    /// use rm_simulator_world::{
    ///     ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, RobotKind, Team,
    /// };
    ///
    /// let mut field = Field::new(&FieldConfig {
    ///     runes: vec![],
    ///     outposts: vec![],
    ///     ..FieldConfig::default()
    /// })
    /// .unwrap();
    /// let placement = |x_m| ChassisPlacement {
    ///     config: ChassisConfig::default(),
    ///     spawn: Pose::at([x_m, 0.0, ChassisConfig::default().rest_height_m()]),
    ///     team: Team::Blue,
    ///     kind: RobotKind::Infantry,
    ///     performance: None,
    /// };
    /// assert_eq!(field.add_chassis(&placement(0.0)).unwrap(), 0);
    /// assert_eq!(field.add_chassis(&placement(1.0)).unwrap(), 1);
    /// field.remove_chassis(0).unwrap();
    /// // A freed id is never handed out again.
    /// assert_eq!(field.add_chassis(&placement(2.0)).unwrap(), 2);
    /// assert_eq!(field.chassis_team(2), Some(Team::Blue));
    /// ```
    pub fn add_chassis(&mut self, placement: &ChassisPlacement) -> Result<u32, FieldError> {
        let id = self
            .physics
            .add_chassis(placement.team, placement.config.clone(), placement.spawn)
            .map_err(FieldError::Chassis)?;
        if let Some(referee) = &mut self.referee {
            referee
                .add_robot(id, placement.team, placement.kind, placement.performance)
                .map_err(FieldError::Referee)?;
        }
        Ok(id)
    }
    /// Take a chassis off the field, with its robot record.
    pub fn remove_chassis(&mut self, id: u32) -> Result<(), FieldError> {
        self.physics
            .remove_chassis(id)
            .map_err(FieldError::Chassis)?;
        if let Some(referee) = &mut self.referee {
            referee.remove_robot(id).map_err(FieldError::Referee)?;
        }
        Ok(())
    }
    /// Reposition an existing chassis for inspection without changing its HP or id.
    pub fn place_chassis(&mut self, id: u32, spawn: Pose) -> Result<(), FieldError> {
        self.physics
            .place_chassis(id, spawn)
            .map_err(FieldError::Chassis)
    }
    /// Placement revision of a chassis id; it changes on placement or revival,
    /// so a client never interpolates across robot lives. `None` for an unknown id.
    pub fn chassis_revision(&self, id: u32) -> Option<u64> {
        self.physics.chassis_revision(id)
    }
    /// Whether the referee has the chassis' robot at zero HP; `None` for an
    /// unknown id. A defeated robot cannot drive, aim or fire and absorbs no damage.
    pub fn chassis_defeated(&self, id: u32) -> Option<bool> {
        self.physics.chassis_defeated(id)
    }
    /// Mirror the gameplay engine's base and outpost HP onto the physical
    /// bases and outposts at `time_ns`. Without a referee the physical objects
    /// are their own authority.
    fn sync_structures(&mut self, time_ns: u64) {
        let Some(referee) = &self.referee else {
            return;
        };
        for base in &mut self.bases {
            (base.hp, base.shield_hp) = referee.base_hp(base.config.team);
        }
        for (index, outpost) in self.outposts.iter_mut().enumerate() {
            if let Some(hp) = referee.outpost_hp(index)
                && hp != outpost.hp()
            {
                // The game bounds outpost HP to 1500, which `set_hp` accepts.
                let _ = outpost.set_hp(time_ns, hp);
            }
        }
    }
    /// Cut the drive of every chassis whose robot the referee has defeated
    /// and restore the revived ones.
    fn sync_defeats(&mut self) {
        let Some(referee) = &self.referee else {
            return;
        };
        for robot in referee.robots() {
            self.physics.set_chassis_defeated(robot.id, !robot.alive());
        }
    }
    /// Set a chassis' desired body velocity; it holds until replaced. The
    /// omni inverse kinematics derives wheel speeds from it, and the motors
    /// decide how much of the wish the drivetrain budget allows.
    ///
    /// ```rust
    /// use rm_simulator_world::{
    ///     ChassisCommand, ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, RobotKind,
    ///     Team,
    /// };
    ///
    /// let config = FieldConfig {
    ///     chassis: vec![ChassisPlacement {
    ///         config: ChassisConfig::default(),
    ///         spawn: Pose::at([0.0, 0.0, ChassisConfig::default().rest_height_m()]),
    ///         team: Team::Red,
    ///         kind: RobotKind::Infantry,
    ///         performance: None,
    ///     }],
    ///     runes: vec![],
    ///     outposts: vec![],
    ///     ..FieldConfig::default()
    /// };
    /// let mut field = Field::new(&config).unwrap();
    /// field
    ///     .command_chassis(
    ///         0,
    ///         ChassisCommand {
    ///             forward_m_s: 2.0,
    ///             ..ChassisCommand::default()
    ///         },
    ///     )
    ///     .unwrap();
    /// field.step(1_000).unwrap();
    /// let moved = field.chassis_positions_m().next().unwrap();
    /// assert!(moved[0] > 0.5, "{moved:?}");
    /// ```
    pub fn command_chassis(&mut self, id: u32, command: ChassisCommand) -> Result<(), FieldError> {
        self.physics
            .command_chassis(id, command)
            .map_err(FieldError::Chassis)
    }
    /// The preset a chassis id was placed with; `None` for an unknown id.
    pub fn chassis_config(&self, id: u32) -> Option<&ChassisConfig> {
        self.physics.chassis_config(id)
    }
    /// The team a chassis id plays for; `None` for an unknown id.
    pub fn chassis_team(&self, id: u32) -> Option<Team> {
        self.physics.chassis_team(id)
    }
    /// Authoritative muzzle pose for a chassis' held turret aim.
    /// The muzzle is 0.35 m along the turret's +x barrel line.
    pub fn chassis_muzzle_pose(&self, id: u32) -> Option<Pose> {
        self.physics.chassis_muzzle_pose(id)
    }
    /// The ball restitution, flight limit and retirement rule this field runs.
    pub fn projectile_policy(&self) -> projectile::ProjectilePolicy {
        self.physics.projectile_policy()
    }
    /// Take the recent projectile removal records, oldest first. Lifetime
    /// instrumentation; no rule reads them.
    pub fn take_projectile_removals(&mut self) -> Vec<projectile::Removal> {
        self.physics.take_removals()
    }
    /// Every ball in flight, without building a whole field snapshot; a
    /// provisional-shot replay reads its own balls and nothing else.
    pub fn projectile_snapshots(&self) -> Vec<ProjectileSnapshot> {
        self.physics.snapshot()
    }
    /// One chassis' snapshot on its own, without building a whole field
    /// snapshot; a replay reads its own robot after every tick.
    pub fn chassis_snapshot(&self, id: u32) -> Option<ChassisSnapshot> {
        self.physics.chassis_snapshot(id)
    }
    /// Current chassis centres in world FLU metres, in id order, without snapshots.
    pub fn chassis_positions_m(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.physics.chassis_positions_m()
    }
    /// The field's outposts in referee index order.
    ///
    /// ```rust
    /// use rm_simulator_world::{Field, FieldConfig, outpost::INITIAL_HP};
    ///
    /// let field = Field::new(&FieldConfig::default()).unwrap();
    /// assert_eq!(field.outposts().len(), 2);
    /// assert_eq!(field.outposts()[0].hp(), INITIAL_HP);
    /// ```
    pub fn outposts(&self) -> &[Outpost] {
        &self.outposts
    }
    /// Ticks elapsed since the field was built.
    pub fn tick(&self) -> u64 {
        self.tick
    }
    /// `tick` times `tick_ns()`: the field's own clock, never the host's.
    pub fn time_ns(&self) -> u64 {
        self.tick * tick_ns()
    }
    /// The field's runes in referee index order.
    ///
    /// ```rust
    /// use rm_simulator_world::{Field, FieldConfig, RuneState};
    ///
    /// let field = Field::new(&FieldConfig::default()).unwrap();
    /// assert_eq!(field.runes().len(), 1);
    /// // Training policy: a fresh rune is already Activating with blade 0 lit.
    /// assert_eq!(field.runes()[0].state(), RuneState::Activating);
    /// assert_eq!(field.runes()[0].snapshot().active_blade, Some(0));
    /// ```
    pub fn runes(&self) -> &[Rune] {
        &self.runes
    }
    /// Mutable access to rune `index` for a caller that establishes hits
    /// itself; the field advances the rune again on the next tick. `None`
    /// when the index is out of range.
    pub fn rune_mut(&mut self, index: usize) -> Option<&mut Rune> {
        self.runes.get_mut(index)
    }
    /// The match referee, when the field was configured with one.
    pub fn referee(&self) -> Option<&Referee> {
        self.referee.as_ref()
    }
    /// Forward an operator command to the referee, stamped with the field's
    /// own clock. Base and outpost HP overrides are applied to the physical
    /// objects here rather than inside the referee; `StartMatch` and
    /// `ResetMatch` also restore every base, outpost and robot.
    ///
    /// ```rust
    /// use rm_simulator_world::{
    ///     Field, FieldConfig, MatchPhase, RefereeCommand, RefereeConfig, tick_ns,
    /// };
    ///
    /// let config = FieldConfig {
    ///     referee: Some(RefereeConfig::alternating(1, 2)),
    ///     ..FieldConfig::default()
    /// };
    /// let mut field = Field::new(&config).unwrap();
    /// // StartMatch runs the section 6.5 five-second countdown first.
    /// field.referee_command(RefereeCommand::StartMatch).unwrap();
    /// field.step(5_000_000_000 / tick_ns()).unwrap();
    /// assert_eq!(field.referee().unwrap().phase(), MatchPhase::Running);
    /// assert_eq!(field.referee().unwrap().snapshot().match_time_ns, 0);
    /// // The round clock advances with the field's ticks of tick_ns() each.
    /// field.step(1_000_000_000 / tick_ns()).unwrap();
    /// assert_eq!(
    ///     field.referee().unwrap().snapshot().match_time_ns,
    ///     1_000_000_000
    /// );
    /// ```
    pub fn referee_command(&mut self, command: RefereeCommand) -> Result<(), FieldError> {
        let now_ns = self.time_ns();
        let referee = self
            .referee
            .as_mut()
            .ok_or(FieldError::Referee("the field has no referee"))?;
        referee
            .command(command, now_ns, &mut self.runes)
            .map_err(FieldError::Referee)?;
        match command {
            RefereeCommand::StartMatch => {
                let start_ns = now_ns.saturating_add(referee.config().countdown_ns);
                for outpost in &mut self.outposts {
                    outpost.start_match(start_ns);
                }
            }
            RefereeCommand::ResetMatch => {
                for outpost in &mut self.outposts {
                    outpost.reset_training();
                }
            }
            _ => {}
        }
        self.sync_structures(now_ns);
        self.sync_mechanisms();
        self.sync_defeats();
        Ok(())
    }
    /// Advance exactly `ticks` ticks of `tick_ns()` nanoseconds each. Stepping
    /// in pieces equals one large step.
    /// Projectiles and chassis are integrated tick by tick. With no active
    /// bodies and no referee, rune state advances directly to the target time.
    ///
    /// ```rust
    /// use rm_simulator_world::{Field, FieldConfig, tick_ns};
    ///
    /// let mut whole = Field::new(&FieldConfig::default()).unwrap();
    /// let mut split = Field::new(&FieldConfig::default()).unwrap();
    /// whole.step(1_000).unwrap();
    /// for ticks in [1, 249, 333, 417] {
    ///     split.step(ticks).unwrap();
    /// }
    /// assert_eq!(whole.tick(), 1_000);
    /// assert_eq!(whole.time_ns(), 1_000 * tick_ns());
    /// // Partitioning the ticks does not change the result.
    /// assert_eq!(whole.snapshot().runes, split.snapshot().runes);
    /// ```
    pub fn step(&mut self, ticks: u64) -> Result<(), FieldError> {
        self.step_with_hits(ticks, &mut |_| {})
    }

    /// Step `ticks` ticks of `tick_ns()` nanoseconds each, reporting each
    /// scored contact in order.
    /// The observer runs after scoring and before snapshot retention can remove
    /// the contact. It sees only new contacts, never restored history, and must
    /// not block. No event queue is retained by the field.
    ///
    /// ```
    /// use rm_simulator_world::{Field, FieldConfig};
    /// let mut field = Field::new(&FieldConfig::default()).unwrap();
    /// let mut contacts = Vec::new();
    /// field.step_with_hits(2_000, &mut |hit| contacts.push(hit.clone())).unwrap();
    /// assert_eq!(field.tick(), 2_000);
    /// assert!(contacts.is_empty());
    /// ```
    pub fn step_with_hits(
        &mut self,
        ticks: u64,
        observer: &mut dyn FnMut(&ArmorHit),
    ) -> Result<(), FieldError> {
        let end = self
            .tick
            .checked_add(ticks)
            .filter(|tick| tick.checked_mul(tick_ns()).is_some())
            .ok_or(FieldError::TickOverflow)?;
        while self.tick < end {
            let idle = self.physics.is_idle();
            if idle && self.referee.is_none() {
                for rune in &mut self.runes {
                    rune.advance_to(end * tick_ns())?;
                }
                self.tick = end;
                self.sync_mechanisms();
                break;
            }
            let prev_ns = self.time_ns();
            if !idle {
                // Rule commands and scored hits may change a target at this same
                // timestamp, so derive the start frame from authoritative rules.
                self.target_frames
                    .begin(|faces| {
                        Self::write_target_faces(
                            &self.runes,
                            &self.outposts,
                            &self.bases,
                            prev_ns,
                            faces,
                        )
                    })
                    .map_err(FieldError::Ballistics)?;
            }
            self.tick += 1;
            let next_ns = self.time_ns();
            for rune in &mut self.runes {
                rune.advance_to(next_ns)?;
            }
            if let Some(referee) = &mut self.referee {
                // The referee's own rune changes are applied at this tick too.
                referee.tick(next_ns, &mut self.runes)?;
                for rune in &mut self.runes {
                    rune.advance_to(next_ns)?;
                }
                // Section 5.5.1: the middle armor stops for the round at 3:00,
                // and a team's once the other team's base armor expands.
                if referee.phase() == MatchPhase::Running {
                    let teams = &referee.game().snapshot().teams;
                    let late = referee.match_time_ns() >= referee::OUTPOST_ROTOR_STOP_NS;
                    for team in Team::BOTH {
                        let expanded = teams[team.other().index()].base_armor_expanded;
                        if (late || expanded)
                            && let Some(index) = referee.outpost_of(team)
                        {
                            self.outposts[index].stop(next_ns);
                        }
                    }
                }
                self.sync_structures(next_ns);
                self.sync_defeats();
            }
            self.sync_mechanisms();
            if idle {
                continue;
            }
            self.target_frames
                .update(|faces| {
                    Self::write_target_faces(
                        &self.runes,
                        &self.outposts,
                        &self.bases,
                        next_ns,
                        faces,
                    )
                })
                .map_err(FieldError::Ballistics)?;
            let contacts = self
                .physics
                .step(prev_ns, &self.target_frames)
                .map_err(FieldError::Ballistics)?;
            for contact in contacts {
                if self.armor_disabled(contact.target) {
                    continue;
                }
                let hit = self.score(next_ns, contact)?;
                if let Some(referee) = &mut self.referee {
                    referee.observe_hit(&hit);
                }
                observer(&hit);
                self.hits.push(hit);
            }
            self.apply_collisions(next_ns);
            self.observe_zones();
            self.sync_structures(next_ns);
            self.sync_mechanisms();
            self.sync_defeats();
            self.hits
                .retain(|hit| next_ns.saturating_sub(hit.time_ns) < HIT_MEMORY_NS);
        }
        Ok(())
    }
    /// Table 5-2 collision damage for chassis armor modules that struck
    /// something during a running round, at most once per module per 17 mm
    /// detection interval.
    fn apply_collisions(&mut self, time_ns: u64) {
        let collisions = self.physics.take_armor_collisions();
        let Some(referee) = &mut self.referee else {
            return;
        };
        if referee.phase() != MatchPhase::Running {
            return;
        }
        for collision in collisions {
            let target = ArmorTarget::Chassis {
                chassis: collision.chassis,
                plate: collision.plate,
            };
            if self.physics.chassis_defeated(collision.chassis) != Some(false) {
                continue;
            }
            if let Some(last) = self.last_detection_ns.get(&target)
                && time_ns.saturating_sub(*last) < Caliber::Mm17.detection_interval_ns()
            {
                continue;
            }
            self.last_detection_ns.insert(target, time_ns);
            referee.damage(
                rm_simulator_gameplay::Target::Robot(collision.chassis),
                referee::COLLISION_DAMAGE_HP,
                rm_simulator_gameplay::DamageKind::Collision,
            );
        }
    }
    /// Report each robot's contact with every buff point footprint, so a
    /// running round applies the section 5.5.3 occupation effects.
    fn observe_zones(&mut self) {
        let Some(referee) = &mut self.referee else {
            return;
        };
        if self.zones.is_empty() || referee.phase() != MatchPhase::Running {
            return;
        }
        let robots: Vec<u32> = referee.robots().map(|r| r.id).collect();
        for id in robots {
            let Some(position) = self.physics.chassis_position_m(id) else {
                continue;
            };
            for area in &self.zones {
                referee.observe_zone(id, area.zone(), area.contains(position));
            }
        }
    }
    /// Buff point footprints this field reports contacts for.
    pub fn zones(&self) -> &[zones::ZoneArea] {
        &self.zones
    }
    /// Replace the buff point footprints, for a field rebuilt by
    /// [`Field::restore`], which carries none.
    pub fn set_zones(&mut self, zones: Vec<zones::ZoneArea>) {
        self.zones = zones;
    }
    /// Resolve match state only when articulated scenery actually needs it.
    fn sync_mechanisms(&mut self) {
        if self.physics.has_mechanisms() {
            let state = referee::mechanism_state(self.referee.as_ref(), self.time_ns());
            self.physics.sync_mechanisms(&state);
        }
    }
    /// Disabled armor remains physical but does not register hits or flashes.
    /// A base or outpost is disabled once its HP reaches zero, and a chassis
    /// module once the referee has defeated its robot. Rune targets are never
    /// disabled here; the referee decides which rings still detect.
    ///
    /// ```rust
    /// use rm_simulator_world::{
    ///     ArmorTarget, Field, FieldConfig, RefereeCommand, RefereeConfig,
    /// };
    ///
    /// let config = FieldConfig {
    ///     referee: Some(RefereeConfig::alternating(1, 2)),
    ///     ..FieldConfig::default()
    /// };
    /// let mut field = Field::new(&config).unwrap();
    /// let face = ArmorTarget::Outpost { outpost: 0, face: 0 };
    /// assert!(!field.armor_disabled(face));
    /// field
    ///     .referee_command(RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 })
    ///     .unwrap();
    /// assert!(field.armor_disabled(face));
    /// ```
    pub fn armor_disabled(&self, target: ArmorTarget) -> bool {
        match target {
            ArmorTarget::Base { base, .. } => {
                self.bases.get(base as usize).is_none_or(|b| b.hp == 0)
            }
            ArmorTarget::Outpost { outpost, .. } => self
                .outposts
                .get(outpost as usize)
                .is_none_or(|o| o.hp() == 0),
            ArmorTarget::Chassis { chassis, .. } => {
                self.physics.chassis_defeated(chassis).unwrap_or(true)
            }
            ArmorTarget::Rune { .. } => false,
        }
    }
    /// A frame of everything an observer needs at the current tick: bases,
    /// runes, outposts, projectiles, chassis, recent hits and the referee.
    /// Taking one never advances the clock, and it always carries the hidden
    /// rule state a `restore` needs.
    ///
    /// ```rust
    /// use rm_simulator_world::{Field, FieldConfig, RuneState, tick_ns};
    ///
    /// let mut field = Field::new(&FieldConfig::default()).unwrap();
    /// field.step(250).unwrap();
    /// let snapshot = field.snapshot();
    /// assert_eq!(snapshot.tick, 250);
    /// assert_eq!(snapshot.time_ns, 250 * tick_ns());
    /// assert_eq!(snapshot.runes[0].state, RuneState::Activating);
    /// assert!(snapshot.restore.is_some());
    /// ```
    pub fn snapshot(&self) -> FieldSnapshot {
        let time_ns = self.time_ns();
        FieldSnapshot {
            bases: self.bases.clone(),
            tick: self.tick,
            time_ns,
            runes: self.runes.iter().map(Rune::snapshot).collect(),
            outposts: self
                .outposts
                .iter()
                .map(|outpost| outpost.snapshot(time_ns))
                .collect(),
            projectiles: self.physics.snapshot(),
            chassis: self.physics.chassis_snapshots(),
            hits: self.hits.clone(),
            shots_fired: self.physics.launched(),
            hits_detected: self.hits_detected,
            referee: self.referee.as_ref().map(Referee::snapshot),
            restore: Some(FieldRestore {
                runes: self.runes.clone(),
                outposts: self.outposts.clone(),
                referee: self.referee.clone(),
                next_chassis_id: self.physics.next_chassis_id(),
                projectile_policy: self.physics.projectile_policy(),
                last_detection_ns: {
                    // A hash map has no order; sort so equal fields snapshot equal.
                    let mut detections: Vec<_> = self
                        .last_detection_ns
                        .iter()
                        .map(|(target, time_ns)| (*target, *time_ns))
                        .collect();
                    detections.sort_unstable();
                    detections
                },
            }),
        }
    }
    /// Rebuild the field the snapshot was taken from, sharing `geometry`'s
    /// collision shapes instead of reloading any CAD, and standing at the
    /// snapshot's own tick. Stepping the result applies the same rules as the
    /// original: this is one field, not a reduced presentation world.
    ///
    /// What is restored: every rune, outpost and the referee exactly (from
    /// [`FieldRestore`]); every chassis with its pose, velocities, held aim,
    /// command, defeat and wheel spin; every projectile with its identity,
    /// age, position, velocity and spin; the armor detection intervals, the
    /// recent hits and the shot and detection counters.
    ///
    /// What cannot be restored is solver state, as [`FieldRestore`] records:
    /// contact manifolds and warm starts are rebuilt, and a projectile's set
    /// of already-touched armor colliders starts empty. Wheel contacts and
    /// tyre targets are recomputed on the first tick, so they need no
    /// checkpoint.
    ///
    /// ```rust
    /// use rm_simulator_world::{Field, FieldConfig};
    ///
    /// let mut field = Field::new(&FieldConfig::default()).unwrap();
    /// field.step(500).unwrap();
    /// let checkpoint = field.snapshot();
    /// let geometry = field.static_geometry_snapshot();
    ///
    /// let mut restored = Field::restore(&checkpoint, &geometry, 0.0).unwrap();
    /// assert_eq!(restored.tick(), checkpoint.tick);
    /// // The restored field keeps stepping under the same rules as the original.
    /// field.step(10).unwrap();
    /// restored.step(10).unwrap();
    /// assert_eq!(restored.snapshot().runes, field.snapshot().runes);
    /// ```
    pub fn restore(
        snapshot: &FieldSnapshot,
        geometry: &StaticGeometry,
        floor_height_m: f64,
    ) -> Result<Self, FieldError> {
        let rules = snapshot
            .restore
            .as_ref()
            .ok_or(FieldError::Restore("the snapshot carries no rule state"))?;
        if !floor_height_m.is_finite() {
            return Err(FieldError::Restore("the floor height must be finite"));
        }
        if snapshot.tick.checked_mul(tick_ns()) != Some(snapshot.time_ns) {
            return Err(FieldError::Restore("the snapshot tick and time disagree"));
        }
        for base in &snapshot.bases {
            base.config.validate().map_err(FieldError::Restore)?;
            if base.hp > base::INITIAL_HP || base.shield_hp > base::INITIAL_HP {
                return Err(FieldError::Restore("base HP or shield out of range"));
            }
        }
        let time_ns = snapshot.time_ns;
        let runes = rules.runes.clone();
        let outposts = rules.outposts.clone();
        for rune in &runes {
            rune.validate().map_err(FieldError::Rune)?;
        }
        for outpost in &outposts {
            outpost.validate().map_err(FieldError::Outpost)?;
        }
        let mut faces = Vec::new();
        Self::write_target_faces(&runes, &outposts, &snapshot.bases, time_ns, &mut faces);
        let mut physics = WorldPhysics::new(&faces, floor_height_m);
        physics
            .set_projectile_policy(rules.projectile_policy)
            .map_err(FieldError::Restore)?;
        physics.insert_geometry(geometry);
        let mut field = Self {
            bases: snapshot.bases.clone(),
            floor_height_m,
            tick: snapshot.tick,
            runes,
            outposts,
            physics,
            target_frames: TargetFrames::new(faces),
            hits: snapshot.hits.clone(),
            hits_detected: snapshot.hits_detected,
            last_detection_ns: rules.last_detection_ns.iter().copied().collect(),
            referee: rules.referee.clone(),
            zones: Vec::new(),
        };
        for chassis in &snapshot.chassis {
            field
                .physics
                .restore_chassis(chassis)
                .map_err(FieldError::Chassis)?;
        }
        for projectile in &snapshot.projectiles {
            field
                .physics
                .restore_projectile(projectile)
                .map_err(FieldError::Shot)?;
        }
        field
            .physics
            .restore_identity(rules.next_chassis_id, snapshot.shots_fired);
        field.physics.mark_synced(time_ns);
        field.sync_mechanisms();
        field.sync_defeats();
        Ok(field)
    }
    /// Put one chassis back on a captured state, for a client replaying its
    /// own robot against estimated peers. The authoritative field never does
    /// this; nothing else about the field moves.
    pub fn reset_chassis(&mut self, state: &ChassisSnapshot) -> Result<(), FieldError> {
        self.physics
            .reset_chassis(state)
            .map_err(FieldError::Chassis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// World-time durations as tick counts, so every stepping rhythm here
    /// keeps its meaning whatever the fixed tick length is. Rounded up: these
    /// are settling and boundary waits, and a truncated duration advances less
    /// than the world time it names. A test that needs an exact world time
    /// steps an explicit tick count instead, the way the partition tests do.
    fn ticks(ns: u64) -> u64 {
        ns.div_ceil(tick_ns())
    }
    /// A referee'd field with two pilots, a rune and both outposts, driven
    /// and shot at long enough for every rule to have moved.
    fn busy_field(shoot: bool) -> (Field, u32) {
        let spawn = ChassisConfig::default().rest_height_m();
        let config = FieldConfig {
            chassis: vec![
                ChassisPlacement {
                    config: ChassisConfig::default(),
                    team: Team::Red,
                    kind: RobotKind::Infantry,
                    // A heat limit the eight quick shots below stay under.
                    performance: Some(rm_simulator_gameplay::Performance::Fixed(
                        rm_simulator_gameplay::Stats {
                            max_hp: 200,
                            chassis_power_w: 60,
                            heat_limit: 1_000,
                            cooling_per_s: 20,
                        },
                    )),
                    spawn: Pose::at([0.0, 0.0, spawn]),
                },
                ChassisPlacement {
                    config: ChassisConfig::default(),
                    team: Team::Blue,
                    kind: RobotKind::Infantry,
                    performance: None,
                    spawn: Pose::at([1.5, 0.6, spawn]),
                },
            ],
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..FieldConfig::default()
        };
        let mut field = Field::new(&config).unwrap();
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        field.step(ticks(referee::COUNTDOWN_NS)).unwrap();
        let drive = ChassisCommand {
            forward_m_s: 1.2,
            yaw_rate_rad_s: 0.4,
            aim_yaw_rad: 0.3,
            ..Default::default()
        };
        field.command_chassis(0, drive).unwrap();
        for round in 0..8 {
            field.step(ticks(60_000_000)).unwrap();
            if !shoot {
                continue;
            }
            let muzzle = field.chassis_muzzle_pose(0).unwrap();
            field
                .fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(0))
                .unwrap();
            // Aim one shot straight at the rune so its rules advance too.
            let target = field.snapshot().runes[0].target_poses[round % 5];
            let toward = Pose {
                translation_m: [
                    target.translation_m[0] - 1.2,
                    target.translation_m[1],
                    target.translation_m[2],
                ],
                ..Pose::default()
            };
            field
                .fire(toward, Shot::at_limit(Caliber::Mm17), None)
                .unwrap();
        }
        field.step(ticks(30_000_000)).unwrap();
        (field, 0)
    }
    fn chassis_gap(a: &FieldSnapshot, b: &FieldSnapshot) -> f64 {
        a.chassis
            .iter()
            .zip(&b.chassis)
            .flat_map(|(a, b)| {
                assert_eq!(a.id, b.id);
                a.pose
                    .translation_m
                    .into_iter()
                    .zip(b.pose.translation_m)
                    .map(|(a, b)| (a - b).abs())
            })
            .fold(0.0_f64, f64::max)
    }
    fn projectile_gap(a: &FieldSnapshot, b: &FieldSnapshot) -> f64 {
        assert_eq!(
            a.projectiles.iter().map(|p| p.id).collect::<Vec<_>>(),
            b.projectiles.iter().map(|p| p.id).collect::<Vec<_>>(),
            "a restored field must keep every ball's identity"
        );
        a.projectiles
            .iter()
            .zip(&b.projectiles)
            .flat_map(|(a, b)| {
                a.position_m
                    .into_iter()
                    .zip(b.position_m)
                    .map(|(a, b)| (a - b).abs())
            })
            .fold(0.0_f64, f64::max)
    }
    /// Replaying a restored field is exact while nothing rests in a contact:
    /// the chassis stands on wheel ray casts, not on the contact solver.

    #[test]
    fn a_restored_field_without_contacts_replays_exactly() {
        let (mut original, pilot) = busy_field(false);
        let geometry = original.static_geometry_snapshot();
        let floor = original.floor_height_m();
        let mut restored = Field::restore(&original.snapshot(), &geometry, floor).unwrap();
        for round in 0..10 {
            let command = ChassisCommand {
                forward_m_s: if round % 3 == 0 { 1.5 } else { -0.5 },
                left_m_s: 0.4,
                yaw_rate_rad_s: 0.6,
                aim_yaw_rad: 0.2 * round as f64,
                aim_pitch_rad: 0.05,
            };
            original.command_chassis(pilot, command).unwrap();
            restored.command_chassis(pilot, command).unwrap();
            original.step(ticks(20_000_000)).unwrap();
            restored.step(ticks(20_000_000)).unwrap();
            assert_eq!(original.snapshot(), restored.snapshot());
        }
    }
    #[test]
    fn a_restored_field_keeps_stepping_like_the_one_it_came_from() {
        let (mut original, pilot) = busy_field(true);
        let geometry = original.static_geometry_snapshot();
        let floor = original.floor_height_m();
        let checkpoint = original.snapshot();
        let mut restored = Field::restore(&checkpoint, &geometry, floor).unwrap();
        // Restoring is a pure read of the checkpoint, in both directions.
        assert_eq!(restored.snapshot(), checkpoint);
        assert_eq!(
            Field::restore(&restored.snapshot(), &geometry, floor)
                .unwrap()
                .snapshot(),
            checkpoint,
            "restore(snapshot(restore(s))) must stand exactly where s did"
        );
        let mut worst_chassis = 0.0_f64;
        let mut worst_projectile = 0.0_f64;
        for round in 0..10 {
            let command = ChassisCommand {
                forward_m_s: if round % 3 == 0 { 1.5 } else { -0.5 },
                left_m_s: 0.4,
                yaw_rate_rad_s: 0.6,
                aim_yaw_rad: 0.2 * round as f64,
                aim_pitch_rad: 0.05,
            };
            original.command_chassis(pilot, command).unwrap();
            restored.command_chassis(pilot, command).unwrap();
            original.step(ticks(20_000_000)).unwrap();
            restored.step(ticks(20_000_000)).unwrap();
            let (a, b) = (original.snapshot(), restored.snapshot());
            worst_chassis = worst_chassis.max(chassis_gap(&a, &b));
            worst_projectile = worst_projectile.max(projectile_gap(&a, &b));
            assert_eq!(a.referee, b.referee, "referee state diverged");
            assert_eq!(a.runes, b.runes, "rune state diverged");
            assert_eq!(a.outposts, b.outposts, "outpost state diverged");
            assert_eq!(a.tick, b.tick);
            assert_eq!(a.shots_fired, b.shots_fired);
            assert_eq!(a.hits_detected, b.hits_detected);
        }
        eprintln!(
            "restore over 200 ticks: worst chassis {worst_chassis:.3e} m, worst ball {worst_projectile:.3e} m"
        );
        // Measured over these 200 ticks: the chassis agree exactly (0 m) and
        // the balls to 1.6e-17 m, one ulp of a metre, picked up where a spent
        // ball rolls on a rebuilt contact manifold. The bounds below leave
        // room for that rounding and for nothing else: a state the restore
        // forgot moves a driven chassis by millimetres within a few ticks,
        // which is what the pre-priming version of this test measured.
        assert!(
            worst_projectile < 1e-12,
            "projectile disagreement {worst_projectile} m"
        );
        assert!(
            worst_chassis < 1e-9,
            "chassis disagreement {worst_chassis} m"
        );
    }
    #[test]
    fn restoring_rejects_a_snapshot_it_cannot_rebuild() {
        let (field, _) = busy_field(true);
        let mut snapshot = field.snapshot();
        snapshot.restore = None;
        assert!(matches!(
            Field::restore(&snapshot, &StaticGeometry::default(), 0.0),
            Err(FieldError::Restore(_))
        ));
        let snapshot = field.snapshot();
        assert!(matches!(
            Field::restore(&snapshot, &StaticGeometry::default(), f64::NAN),
            Err(FieldError::Restore(_))
        ));
        let mut broken = field.snapshot();
        broken.time_ns += 1;
        assert!(matches!(
            Field::restore(&broken, &StaticGeometry::default(), 0.0),
            Err(FieldError::Restore(_))
        ));
    }
    /// Partition invariance stated in world time rather than tick counts, so it
    /// holds whatever the fixed `tick_ns` length is.
    #[test]
    fn field_partition_invariance_holds_at_the_fixed_tick() {
        let config = FieldConfig::default();
        let mut whole = Field::new(&config).unwrap();
        let mut split = Field::new(&config).unwrap();
        let muzzle = Pose::yawed([0.0, 0.0, 1.0], 0.6);
        for field in [&mut whole, &mut split] {
            field.step(ticks(500_000_000)).unwrap();
            field
                .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
                .unwrap();
        }
        // Split by uneven tick counts derived from the total, so the parts
        // still sum to it at the fixed tick.
        let total = ticks(1_500_000_000);
        whole.step(total).unwrap();
        let parts = [1, total / 7, total / 3];
        let rest = total - parts.iter().sum::<u64>();
        for part in parts.into_iter().chain([rest]) {
            split.step(part).unwrap();
        }
        assert_eq!(whole.tick(), split.tick());
        assert_eq!(whole.snapshot(), split.snapshot());
    }
    #[test]
    fn default_field_steps_deterministically_in_any_partition() {
        let config = FieldConfig::default();
        let mut whole = Field::new(&config).unwrap();
        let mut split = Field::new(&config).unwrap();
        whole.step(2_600).unwrap();
        for ticks in [1, 999, 1_000, 600] {
            split.step(ticks).unwrap();
        }
        assert_eq!(whole.snapshot(), split.snapshot());
        let snapshot = whole.snapshot();
        assert_eq!(snapshot.tick, 2_600);
        assert_eq!(snapshot.time_ns, 2_600 * tick_ns());
        assert_eq!(snapshot.outposts.len(), 2);
        assert!(snapshot.runes[0].angle_rad > 0.0);
        // Reading a snapshot does not advance time.
        assert_eq!(whole.snapshot(), snapshot);
    }
    #[test]
    fn big_rune_and_empty_field_are_valid_and_invalid_input_is_rejected() {
        let big = FieldConfig {
            runes: vec![
                RuneConfig {
                    kind: RuneKind::Big,
                    cad_orbit: true,
                    ..RuneConfig::default()
                },
                RuneConfig {
                    cad_orbit: true,
                    ..RuneConfig::default()
                },
            ],
            outposts: Vec::new(),
            ..FieldConfig::default()
        };
        let mut field = Field::new(&big).unwrap();
        field.step(10).unwrap();
        let snapshot = field.snapshot();
        assert_eq!(snapshot.runes[0].kind, rune::RuneKind::Big);
        assert_eq!(snapshot.runes[1].kind, rune::RuneKind::Small);
        assert!(
            snapshot
                .runes
                .iter()
                .all(|rune| (rune.target_radius_m - 0.6985).abs() < 1e-12)
        );
        assert!(snapshot.outposts.is_empty());
        assert!(field.rune_mut(1).is_some() && field.rune_mut(2).is_none());
        let empty = Field::new(&FieldConfig {
            runes: Vec::new(),
            outposts: Vec::new(),
            ..FieldConfig::default()
        })
        .unwrap();
        assert!(empty.snapshot().runes.is_empty());
        assert!(matches!(
            Field::new(&FieldConfig {
                outposts: vec![OutpostConfig {
                    pivot_cad_m: None,
                    origin: Pose::at([f64::NAN, 0.0, 0.0]),
                    speed_rad_s: 1.0
                }],
                ..FieldConfig::default()
            }),
            Err(FieldError::Outpost(_))
        ));
        assert!(matches!(
            Field::new(&FieldConfig {
                runes: vec![RuneConfig {
                    hub_pose: Pose {
                        rotation_wxyz: [0.0; 4],
                        ..Pose::default()
                    },
                    ..RuneConfig::default()
                }],
                ..FieldConfig::default()
            }),
            Err(FieldError::Rune(_))
        ));
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        assert_eq!(field.step(u64::MAX), Err(FieldError::TickOverflow));
        assert_eq!(field.tick(), 0);
    }
    /// A stationary outpost whose face 0 (facing -x, pitched 15 degrees down)
    /// stands 1.5 m ahead of a shooter at the given height.
    fn stationary_outpost() -> (FieldConfig, Pose) {
        let config = FieldConfig {
            runes: Vec::new(),
            outposts: vec![OutpostConfig {
                pivot_cad_m: None,
                origin: Pose::at([4.0, 0.0, 0.0]),
                speed_rad_s: 0.0,
            }],
            ..FieldConfig::default()
        };
        let field = Field::new(&config).unwrap();
        let face = field.snapshot().outposts[0].armors[0].pose;
        let muzzle = Pose::at([
            face.translation_m[0] - 1.5,
            face.translation_m[1],
            face.translation_m[2] + 0.005,
        ]);
        (config, muzzle)
    }
    #[test]
    fn disabled_outpost_armor_does_not_register_hits_and_revives() {
        let (mut config, muzzle) = stationary_outpost();
        config.referee = Some(RefereeConfig::alternating(0, 1));
        let mut field = Field::new(&config).unwrap();
        field
            .referee_command(RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 })
            .unwrap();
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(100).unwrap();
        assert!(field.snapshot().hits.is_empty());
        assert_eq!(field.snapshot().hits_detected, 0);
        // The projectile bounced off the retained armor housing.
        assert!(field.snapshot().projectiles[0].velocity_m_s[0] < 0.);
        field
            .referee_command(RefereeCommand::SetOutpostHp {
                outpost: 0,
                hp: 1500,
            })
            .unwrap();
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(100).unwrap();
        assert_eq!(field.snapshot().hits_detected, 1);
    }
    #[test]
    fn cached_moving_collider_mesh_reconstructs_live_physics() {
        let mut field = Field::new(&FieldConfig {
            runes: Vec::new(),
            outposts: Vec::new(),
            ..Default::default()
        })
        .unwrap();
        field
            .add_mechanism_mesh(
                vec![[20., 0., 1.], [20., 1., 1.], [20., 0., 2.]],
                vec![[0, 1, 2]],
                referee::Mechanism::DartTarget,
                Team::Red,
                [[0., -0.28, 0.], [0., 0.28, 0.]],
            )
            .unwrap();
        let mut parts = field.static_geometry_snapshot().moving_parts();
        assert_eq!(parts.len(), 1);
        let part = parts.pop().unwrap();
        let (vertices, indices) = part.geometry.into_triangles();
        for ticks in [0, 16, 250, 1000, 2000] {
            field.step(ticks).unwrap();
            let fraction = referee::dart_target_fraction(field.time_ns());
            let translation: [f64; 3] = std::array::from_fn(|i| {
                part.translations_m[0][i]
                    + (part.translations_m[1][i] - part.translations_m[0][i]) * fraction
            });
            let moved: Vec<[f64; 3]> = vertices
                .iter()
                .map(|v| std::array::from_fn(|i| v[i] + translation[i]))
                .collect();
            let actual = field.dynamic_geometry_snapshot().into_triangles();
            assert_eq!(actual, (moved, indices.clone()));
        }
    }

    #[test]
    fn dart_target_collision_sweeps_and_is_tick_partition_independent() {
        for referee in [None, Some(RefereeConfig::alternating(1, 2))] {
            let config = FieldConfig {
                referee,
                ..Default::default()
            };
            let make = || {
                let mut field = Field::new(&config).unwrap();
                field
                    .add_mechanism_mesh(
                        vec![[20., 0., 1.], [20., 1., 1.], [20., 0., 2.]],
                        vec![[0, 1, 2]],
                        referee::Mechanism::DartTarget,
                        Team::Red,
                        [[0., -0.28, 0.], [0., 0.28, 0.]],
                    )
                    .unwrap();
                field
            };
            let mut bulk = make();
            let mut incremental = make();
            let initial = bulk.static_geometry();
            assert!(initial.0.contains(&[20., -0.28, 1.]));
            bulk.step(ticks(2_000_000_000)).unwrap();
            for _ in 0..ticks(2_000_000_000) {
                incremental.step(1).unwrap();
            }
            assert_eq!(bulk.static_geometry(), incremental.static_geometry());
            assert!(bulk.static_geometry().0.contains(&[20., 0.28, 1.]));
            bulk.step(ticks(2_000_000_000)).unwrap();
            assert_eq!(bulk.static_geometry(), initial);
            bulk.step(0).unwrap();
            assert_eq!(bulk.static_geometry(), initial);
        }
    }
    #[test]
    fn mechanism_override_moves_collision_over_its_travel_and_resets() {
        let config = FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..Default::default()
        };
        let mut field = Field::new(&config).unwrap();
        let vertices = vec![[20., 0., 1.], [20., 1., 1.], [20., 0., 2.]];
        field
            .add_mechanism_mesh(
                vertices,
                vec![[0, 1, 2]],
                referee::Mechanism::Base,
                Team::Blue,
                [[0.; 3], [0., 0., 3.]],
            )
            .unwrap();
        let closed = field.static_geometry();
        field
            .referee_command(RefereeCommand::SetBaseOpen {
                team: Team::Red,
                open: true,
            })
            .unwrap();
        assert_eq!(field.static_geometry(), closed);
        field
            .referee_command(RefereeCommand::SetBaseOpen {
                team: Team::Blue,
                open: true,
            })
            .unwrap();
        // The armor travels; halfway through it is halfway up.
        field
            .step(ticks(rm_simulator_physics::motion::BASE_TRAVEL_NS / 2))
            .unwrap();
        assert!(field.static_geometry().0.contains(&[20., 0., 2.5]));
        field
            .step(ticks(rm_simulator_physics::motion::BASE_TRAVEL_NS / 2))
            .unwrap();
        let opened = field.static_geometry();
        assert_ne!(opened, closed);
        assert!(opened.0.contains(&[20., 0., 4.]));
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        assert_eq!(field.static_geometry(), closed);
        assert_eq!(field.snapshot().referee.unwrap().base_open, [false; 2]);
    }
    #[test]
    fn outpost_strike_is_detected_once_per_interval_and_costs_hp() {
        let (config, muzzle) = stationary_outpost();
        let mut field = Field::new(&config).unwrap();
        let shot = Shot::at_limit(Caliber::Mm17);
        let first = field.fire(muzzle, shot, None).unwrap();
        field.step(ticks(10_000_000)).unwrap();
        let mut second_muzzle = muzzle;
        second_muzzle.translation_m[1] += 0.03;
        let second = field.fire(second_muzzle, shot, None).unwrap();
        field.step(ticks(300_000_000)).unwrap();
        let snapshot = field.snapshot();
        assert_eq!(snapshot.shots_fired, 2);
        assert_eq!(snapshot.hits_detected, 1);
        assert_eq!(snapshot.hits.len(), 2, "{:?}", snapshot.hits);
        let hit = &snapshot.hits[0];
        assert_eq!(hit.projectile, first);
        assert!(hit.detected);
        assert_eq!(
            hit.target,
            ArmorTarget::Outpost {
                outpost: 0,
                face: 0
            }
        );
        assert!(hit.normal_speed_m_s > 22.0, "{hit:?}");
        assert_eq!(hit.damage, 20);
        assert!(hit.local_offset_m[0].abs() < 0.01, "{hit:?}");
        let rejected = &snapshot.hits[1];
        assert_eq!(rejected.projectile, second);
        assert!(!rejected.detected);
        assert_eq!(rejected.rejection, Some(Rejection::DetectionInterval));
        assert_eq!(rejected.damage, 0);
        assert_eq!(snapshot.outposts[0].hp, outpost::INITIAL_HP - 20);
        // Hits age out of the snapshot after a second.
        field.step(ticks(1_000_000_000)).unwrap();
        assert!(field.snapshot().hits.is_empty());
        assert_eq!(field.snapshot().hits_detected, 1);
    }
    #[test]
    fn referee_runs_the_match_on_the_field_clock_and_shields_buffed_outposts() {
        let (mut config, muzzle) = stationary_outpost();
        config.runes = vec![RuneConfig::default()];
        config.referee = Some(RefereeConfig::alternating(1, 1));
        let mut field = Field::new(&config).unwrap();
        assert_eq!(field.referee().unwrap().phase(), MatchPhase::Idle);
        assert_eq!(field.runes()[0].state(), RuneState::Activating);
        assert_eq!(
            field.referee_command(RefereeCommand::ActivateRune { team: Team::Red }),
            Err(FieldError::Referee("the round is not running"))
        );
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        assert_eq!(field.runes()[0].state(), RuneState::Inactive);
        // The same match, stepped whole and in pieces, agrees.
        let mut split = Field::new(&config).unwrap();
        split.referee_command(RefereeCommand::StartMatch).unwrap();
        let total = ticks(6_000_000_000);
        field.step(total).unwrap();
        let parts = [1, total / 2, total / 4];
        for part in parts {
            split.step(part).unwrap();
        }
        split.step(total - parts.iter().sum::<u64>()).unwrap();
        assert_eq!(field.snapshot(), split.snapshot());
        let referee = field.snapshot().referee.unwrap();
        assert_eq!(referee.phase, MatchPhase::Running);
        assert_eq!(referee.match_time_ns, 1_000_000_000);
        assert_eq!(referee.teams[0].rune_opportunities, 1);
        // Red activates by hitting each lit blade; the referee then shields red's outpost.
        field
            .referee_command(RefereeCommand::ActivateRune { team: Team::Red })
            .unwrap();
        for _ in 0..5 {
            field.step(ticks(100_000_000)).unwrap();
            let time_ns = field.time_ns();
            let blade = field.runes()[0].snapshot().active_blade.unwrap();
            field.rune_mut(0).unwrap().hit(time_ns, blade).unwrap();
        }
        field.step(1).unwrap();
        assert_eq!(field.runes()[0].state(), RuneState::Activated);
        let buff = field.snapshot().referee.unwrap().teams[0].buff.unwrap();
        assert_eq!(buff.defense_pct, 25);
        let shot = Shot::at_limit(Caliber::Mm17);
        field.fire(muzzle, shot, None).unwrap();
        field.step(ticks(300_000_000)).unwrap();
        let snapshot = field.snapshot();
        let hit = snapshot.hits.iter().find(|hit| hit.detected).unwrap();
        assert_eq!(hit.damage, 15);
        assert_eq!(snapshot.outposts[0].hp, outpost::INITIAL_HP - 15);
        // Reset hands the rune back to training.
        field.referee_command(RefereeCommand::ResetMatch).unwrap();
        assert_eq!(field.runes()[0].state(), RuneState::Activating);
        assert_eq!(field.snapshot().referee.unwrap().phase, MatchPhase::Idle);
        // A referee needs an owner for every rune and outpost.
        let mut bad = config.clone();
        bad.referee = Some(RefereeConfig::alternating(2, 1));
        assert!(matches!(Field::new(&bad), Err(FieldError::Referee(_))));
    }
    #[test]
    fn slow_and_heavy_shots_follow_their_own_thresholds() {
        let (config, muzzle) = stationary_outpost();
        let mut field = Field::new(&config).unwrap();
        // A slow ball drops about 9 cm over 1.5 m; lift the muzzle to keep it on the face.
        let mut lifted = muzzle;
        lifted.translation_m[2] += 0.09;
        field
            .fire(
                lifted,
                Shot {
                    caliber: Caliber::Mm17,
                    speed_m_s: 11.0,
                },
                None,
            )
            .unwrap();
        field.step(ticks(400_000_000)).unwrap();
        let hits = field.snapshot().hits;
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(!hits[0].detected);
        assert_eq!(hits[0].rejection, Some(Rejection::NormalSpeed));
        let mut field = Field::new(&config).unwrap();
        // Aim off the 10 mm centre square so the plain Table 5-2 damage applies.
        let mut raised = muzzle;
        raised.translation_m[1] += 0.03;
        raised.translation_m[2] += 0.06;
        field
            .fire(raised, Shot::at_limit(Caliber::Mm42), None)
            .unwrap();
        field.step(ticks(400_000_000)).unwrap();
        let snapshot = field.snapshot();
        assert_eq!(snapshot.hits.len(), 1, "{:?}", snapshot.hits);
        assert!(snapshot.hits[0].detected, "{:?}", snapshot.hits);
        assert_eq!(snapshot.hits[0].damage, 200);
        assert_eq!(snapshot.outposts[0].hp, outpost::INITIAL_HP - 200);
        assert!(matches!(
            field.fire(
                muzzle,
                Shot {
                    caliber: Caliber::Mm17,
                    speed_m_s: f64::NAN
                },
                None,
            ),
            Err(FieldError::Shot(_))
        ));
    }
    #[test]
    fn shots_follow_rune_conversion_and_idle_resume() {
        let mut field = Field::new(&FieldConfig {
            outposts: Vec::new(),
            ..FieldConfig::default()
        })
        .unwrap();
        // Reuse the same physics scene across expired shots, idle jumps and
        // both rune conversions. Old target poses or velocities must not leak.
        for (kind, speed_m_s) in [
            (RuneKind::Small, 25.0),
            (RuneKind::Big, 20.0),
            (RuneKind::Small, 25.0),
        ] {
            let now_ns = field.time_ns();
            let rune = field.rune_mut(0).unwrap();
            rune.convert(kind, now_ns).unwrap();
            rune.activate(now_ns).unwrap();
            field.step(1_237).unwrap();
            let rune = &field.snapshot().runes[0];
            let blade = rune.active_blade.unwrap();
            let [x, y, z] = rune.target_poses[blade as usize].translation_m;
            let projectile = field
                .fire(
                    Pose::at([x - 0.1, y, z]),
                    Shot {
                        caliber: Caliber::Mm17,
                        speed_m_s,
                    },
                    None,
                )
                .unwrap();
            field.step(10).unwrap();
            let snapshot = field.snapshot();
            let hit = snapshot
                .hits
                .iter()
                .find(|hit| hit.projectile == projectile)
                .expect("the projectile must hit the visible rune target");
            assert_eq!(hit.target, ArmorTarget::Rune { rune: 0, blade });
            assert!(hit.detected, "{hit:?}");
            assert!((hit.normal_speed_m_s - speed_m_s).abs() < 0.2, "{hit:?}");
            field
                .step(projectile::MAX_FLIGHT_NS / tick_ns() + 1)
                .unwrap();
            assert!(field.snapshot().projectiles.is_empty());
        }
    }
    #[test]
    fn active_physics_refreshes_targets_changed_at_the_current_time() {
        let chassis = ChassisConfig::default();
        let mut field = Field::new(&FieldConfig {
            outposts: Vec::new(),
            chassis: vec![ChassisPlacement {
                config: chassis.clone(),
                spawn: Pose::at([-10.0, -5.0, chassis.rest_height_m()]),
                team: Team::Red,
                kind: RobotKind::Infantry,
                performance: None,
            }],
            ..FieldConfig::default()
        })
        .unwrap();
        field.step(10).unwrap();
        let now_ns = field.time_ns();
        field
            .rune_mut(0)
            .unwrap()
            .convert(RuneKind::Big, now_ns)
            .unwrap();
        let mut expected = Vec::new();
        Field::write_target_faces(
            &field.runes,
            &field.outposts,
            &field.bases,
            now_ns,
            &mut expected,
        );

        field.step(1).unwrap();
        assert_eq!(field.target_frames.start(), expected);
    }
    #[test]
    fn rune_target_scores_from_its_visible_front_and_rejects_42mm() {
        let config = FieldConfig {
            runes: vec![RuneConfig::default()],
            outposts: Vec::new(),
            ..FieldConfig::default()
        };
        let mut field = Field::new(&config).unwrap();
        // The visible front is the hub's -x side; blade 0 starts straight up.
        let target = field.snapshot().runes[0].target_poses[0].translation_m;
        assert_eq!(target, [6.0, 0.0, 2.3]);
        let muzzle = Pose::at([target[0] - 1.5, target[1], target[2] + 0.02]);
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(ticks(200_000_000)).unwrap();
        let snapshot = field.snapshot();
        assert_eq!(snapshot.hits.len(), 1, "{:?}", snapshot.hits);
        let hit = &snapshot.hits[0];
        assert!(hit.detected, "{hit:?}");
        assert_eq!(hit.target, ArmorTarget::Rune { rune: 0, blade: 0 });
        assert_eq!(
            hit.rune_outcome,
            Some(HitOutcome::Accepted {
                blade: 0,
                next_blade: 1
            })
        );
        assert!(hit.local_offset_m[0].abs() < 0.1 && hit.local_offset_m[1].abs() < 0.1);
        assert_eq!(snapshot.runes[0].active_blade, Some(1));
        assert!(snapshot.runes[0].activated[0]);
        // A 42 mm projectile bounces off the same face without being detected.
        let mut field = Field::new(&config).unwrap();
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm42), None)
            .unwrap();
        field.step(ticks(400_000_000)).unwrap();
        let hits = field.snapshot().hits;
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(!hits[0].detected);
        assert_eq!(hits[0].rejection, Some(Rejection::Caliber));
        // Shooting the wheel from behind touches the housing back: no detection.
        let mut field = Field::new(&config).unwrap();
        let behind = Pose::yawed(
            [target[0] + 1.5, target[1], target[2] + 0.02],
            std::f64::consts::PI,
        );
        field
            .fire(behind, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(ticks(400_000_000)).unwrap();
        let hits = field.snapshot().hits;
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].rejection, Some(Rejection::OutsideTarget));
    }
    #[test]
    fn projectiles_keep_stepping_deterministic_in_any_partition() {
        let config = FieldConfig::default();
        let mut whole = Field::new(&config).unwrap();
        let mut split = Field::new(&config).unwrap();
        let muzzle = Pose::yawed([0.0, 0.0, 1.0], 0.6);
        let shot = Shot::at_limit(Caliber::Mm17);
        for field in [&mut whole, &mut split] {
            field.step(ticks(500_000_000)).unwrap();
            field.fire(muzzle, shot, None).unwrap();
        }
        let total = ticks(700_000_000);
        whole.step(total).unwrap();
        let parts = [1, total / 4, total / 4];
        for part in parts {
            split.step(part).unwrap();
        }
        split.step(total - parts.iter().sum::<u64>()).unwrap();
        let a = whole.snapshot();
        let b = split.snapshot();
        assert_eq!(a, b);
        assert_eq!(a.projectiles.len(), 1);
        assert!(a.projectiles[0].position_m[0] > 5.0, "{:?}", a.projectiles);
        // The world keeps ticking after the shot is spent.
        whole.step(ticks(5_000_000_000)).unwrap();
        assert!(whole.snapshot().projectiles.is_empty());
        assert_eq!(
            whole.tick(),
            ticks(500_000_000) + ticks(700_000_000) + ticks(5_000_000_000)
        );
    }

    /// A field with a chassis and a 15 degree ramp mesh ahead of it.
    fn ramp_field() -> FieldConfig {
        FieldConfig {
            bases: vec![],
            runes: Vec::new(),
            outposts: Vec::new(),
            floor_height_m: 0.0,
            referee: None,
            projectile_policy: Default::default(),
            zones: Vec::new(),
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                spawn: Pose::at([0.0, 0.0, ChassisConfig::default().rest_height_m()]),
                team: Team::Red,
                kind: RobotKind::Infantry,
                performance: None,
            }],
        }
    }
    fn add_ramp(field: &mut Field) {
        // Rises 15 degrees from x = 1 to x = 3, then a 2 m plateau.
        let top = 2.0 * 15_f64.to_radians().tan();
        field
            .add_static_mesh(
                vec![
                    [1.0, -1.0, 0.0],
                    [1.0, 1.0, 0.0],
                    [3.0, -1.0, top],
                    [3.0, 1.0, top],
                    [5.0, -1.0, top],
                    [5.0, 1.0, top],
                    [5.0, -1.0, 0.0],
                    [5.0, 1.0, 0.0],
                ],
                vec![
                    [0, 2, 1],
                    [1, 2, 3],
                    [2, 4, 3],
                    [3, 4, 5],
                    [4, 6, 5],
                    [5, 6, 7],
                ],
            )
            .unwrap();
    }
    #[test]
    fn mecanum_chassis_steps_deterministically_in_any_partition() {
        let mut config = ramp_field();
        config.chassis[0].config = ChassisConfig::hero();
        config.chassis[0].spawn.translation_m[2] = ChassisConfig::hero().rest_height_m();
        let mut whole = Field::new(&config).unwrap();
        let mut split = Field::new(&config).unwrap();
        for field in [&mut whole, &mut split] {
            field
                .command_chassis(
                    0,
                    ChassisCommand {
                        forward_m_s: 1.0,
                        left_m_s: 0.5,
                        yaw_rate_rad_s: 0.3,
                        ..ChassisCommand::default()
                    },
                )
                .unwrap();
        }
        whole.step(1000).unwrap();
        for ticks in [1, 249, 333, 417] {
            split.step(ticks).unwrap();
        }
        assert_eq!(whole.snapshot(), split.snapshot());
    }
    #[test]
    fn rotating_consumes_drive_capacity_while_climbing() {
        let climb = |spin: f64| {
            let mut field = Field::new(&ramp_field()).unwrap();
            add_ramp(&mut field);
            field.step(ticks(300_000_000)).unwrap();
            for _ in 0..250 {
                let yaw = chassis::yaw_of(field.snapshot().chassis[0].pose);
                // Hold the same world-space uphill wish while the body rotates.
                field
                    .command_chassis(
                        0,
                        ChassisCommand {
                            forward_m_s: 1.5 * yaw.cos(),
                            left_m_s: -1.5 * yaw.sin(),
                            yaw_rate_rad_s: spin,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                field.step(ticks(10_000_000)).unwrap();
            }
            field.snapshot().chassis[0].pose.translation_m[0]
        };
        let straight = climb(0.);
        let rotating = climb(6.);
        assert!(straight > 2.3, "straight={straight}");
        assert!(
            rotating < straight - 0.2,
            "straight={straight}, rotating={rotating}"
        );
    }

    #[test]
    fn chassis_climbs_a_ramp_and_steps_deterministically_in_any_partition() {
        let config = ramp_field();
        let mut whole = Field::new(&config).unwrap();
        let mut split = Field::new(&config).unwrap();
        for field in [&mut whole, &mut split] {
            add_ramp(field);
            field.step(ticks(300_000_000)).unwrap();
            field
                .command_chassis(
                    0,
                    ChassisCommand {
                        forward_m_s: 1.5,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let total = ticks(3_500_000_000);
        whole.step(total).unwrap();
        let parts = [1, total / 4, total / 3];
        for part in parts {
            split.step(part).unwrap();
        }
        split.step(total - parts.iter().sum::<u64>()).unwrap();
        assert_eq!(whole.snapshot(), split.snapshot());
        let chassis = whole.snapshot().chassis.remove(0);
        let top = 2.0 * 15_f64.to_radians().tan();
        let [x, _, z] = chassis.pose.translation_m;
        assert!(x > 3.2 && x < 5.0, "{chassis:?}");
        assert!(
            (z - (top + ChassisConfig::default().rest_height_m())).abs() < 0.02,
            "{chassis:?}"
        );
        assert!(chassis.wheels.iter().all(|w| w.contact.is_some()));
        assert_eq!(whole.tick(), ticks(300_000_000) + total);
        // A field without a chassis or projectiles still jumps straight ahead.
        let mut plain = Field::new(&FieldConfig::default()).unwrap();
        assert!(plain.snapshot().chassis.is_empty());
        assert!(matches!(
            plain.command_chassis(0, ChassisCommand::default()),
            Err(FieldError::Chassis(_))
        ));
        plain.step(10).unwrap();
    }
    /// Chassis join and leave a running field; ids count up and are never
    /// reused, and each one answers only to its own id.
    #[test]
    fn chassis_join_and_leave_with_stable_ids() {
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        let placement = |x: f64, team: Team| ChassisPlacement {
            config: ChassisConfig::default(),
            spawn: Pose::at([x, 0.0, ChassisConfig::default().rest_height_m()]),
            team,
            kind: RobotKind::Infantry,
            performance: None,
        };
        let red = field.add_chassis(&placement(-2.0, Team::Red)).unwrap();
        let blue = field.add_chassis(&placement(-4.0, Team::Blue)).unwrap();
        assert_eq!((red, blue), (0, 1));
        assert_eq!(field.chassis_team(blue), Some(Team::Blue));
        field.step(300).unwrap();
        field
            .command_chassis(
                blue,
                ChassisCommand {
                    forward_m_s: 1.0,
                    ..Default::default()
                },
            )
            .unwrap();
        field.step(1_000).unwrap();
        let snapshot = field.snapshot();
        assert_eq!(
            snapshot
                .chassis
                .iter()
                .map(|c| (c.id, c.team))
                .collect::<Vec<_>>(),
            vec![(red, Team::Red), (blue, Team::Blue)]
        );
        // Only the commanded chassis moved.
        assert!(snapshot.chassis[0].pose.translation_m[0].abs() - 2.0 < 0.05);
        assert!(snapshot.chassis[1].pose.translation_m[0] > -3.5);
        field.remove_chassis(red).unwrap();
        assert!(matches!(
            field.remove_chassis(red),
            Err(FieldError::Chassis(_))
        ));
        assert!(matches!(
            field.command_chassis(red, ChassisCommand::default()),
            Err(FieldError::Chassis(_))
        ));
        let next = field.add_chassis(&placement(-2.0, Team::Red)).unwrap();
        assert_eq!(next, 2);
        field.step(100).unwrap();
        let ids: Vec<u32> = field.snapshot().chassis.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![blue, next]);
    }
    /// Strikes on a chassis armor module damage its referee robot; at zero
    /// HP the drive is cut and the gun silent until the referee revives it.
    #[test]
    fn chassis_armor_takes_damage_and_a_defeated_robot_stops() {
        let config = FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..Default::default()
        };
        let mut field = Field::new(&config).unwrap();
        let chassis = ChassisConfig::default();
        let placement = |x: f64, team: Team| ChassisPlacement {
            config: chassis.clone(),
            spawn: Pose::at([x, 0.0, chassis.rest_height_m()]),
            team,
            kind: RobotKind::Infantry,
            performance: None,
        };
        let red = field.add_chassis(&placement(0.0, Team::Red)).unwrap();
        let blue = field.add_chassis(&placement(-2.0, Team::Blue)).unwrap();
        let robots = field.snapshot().referee.unwrap().robots;
        assert_eq!(robots.len(), 2);
        assert_eq!(
            (robots[0].id, robots[0].team, robots[0].hp),
            (red, Team::Red, 200)
        );
        assert_eq!((robots[1].id, robots[1].team), (blue, Team::Blue));
        field.step(300).unwrap();
        // Blue shoots red's back plate from 1.2 m behind it; a ball at the
        // 17 mm limit drops under a centimetre on the way.
        let muzzle = Pose::at([-1.5, 0.0, chassis.rest_height_m() + chassis.armor_height_m]);
        assert!(matches!(
            field.fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(9)),
            Err(FieldError::Shot(_))
        ));
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(blue))
            .unwrap();
        field.step(ticks(200_000_000)).unwrap();
        let snapshot = field.snapshot();
        let hit = snapshot
            .hits
            .iter()
            .find(|hit| matches!(hit.target, ArmorTarget::Chassis { .. }))
            .expect("the ball struck the armor");
        assert_eq!(
            hit.target,
            ArmorTarget::Chassis {
                chassis: red,
                plate: 2
            }
        );
        assert!(hit.detected, "{hit:?}");
        assert_eq!((hit.shooter, hit.damage), (Some(blue), 20));
        assert_eq!(snapshot.referee.unwrap().robots[0].hp, 180);
        assert!(!snapshot.chassis[0].defeated);
        // Wear it down; each shot waits out the 50 ms detection interval.
        // Inelastic spent balls can intercept later rounds; allow a bounded
        // number of extra attempts rather than assuming every shot scores.
        for _ in 0..29 {
            if field.snapshot().chassis[0].defeated {
                break;
            }
            field
                .fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(blue))
                .unwrap();
            field.step(ticks(200_000_000)).unwrap();
        }
        let snapshot = field.snapshot();
        assert_eq!(
            snapshot.referee.unwrap().robots[0].hp,
            0,
            "{:?}",
            snapshot.hits
        );
        assert!(snapshot.chassis[0].defeated);
        assert!(matches!(
            field.fire(muzzle, Shot::at_limit(Caliber::Mm17), Some(red)),
            Err(FieldError::Referee(_))
        ));
        let drive = ChassisCommand {
            forward_m_s: 1.0,
            aim_yaw_rad: 1.0,
            ..Default::default()
        };
        field.command_chassis(red, drive).unwrap();
        field.step(1_000).unwrap();
        let stopped = field.snapshot().chassis[0].clone();
        assert!(stopped.pose.translation_m[0].abs() < 0.1, "{stopped:?}");
        assert!(chassis::yaw_of(stopped.turret).abs() < 1e-6);
        field
            .referee_command(RefereeCommand::ReviveRobot { robot: red })
            .unwrap();
        field.step(1_000).unwrap();
        let revived = field.snapshot().chassis[0].clone();
        assert!(!revived.defeated);
        assert!(revived.pose.translation_m[0] > 0.3, "{revived:?}");
        assert!((chassis::yaw_of(revived.turret) - 1.0).abs() < 1e-6);
        // Leaving takes the robot record along.
        field.remove_chassis(red).unwrap();
        assert_eq!(field.snapshot().referee.unwrap().robots.len(), 1);
    }
    /// A roof the body fits under but the turret does not stops the chassis.
    #[test]
    fn chassis_turret_stops_under_a_low_roof() {
        let config = ramp_field();
        let chassis = config.chassis[0].config.clone();
        let mut field = Field::new(&config).unwrap();
        let roof_z = chassis.rest_height_m() + chassis.body_half_m[2] + 0.05;
        assert!(
            roof_z
                < chassis.rest_height_m() + chassis.turret_center_m[2] + chassis.turret_half_m[2]
        );
        field
            .add_static_mesh(
                vec![
                    [1.0, -1.0, roof_z],
                    [1.0, 1.0, roof_z],
                    [3.0, -1.0, roof_z],
                    [3.0, 1.0, roof_z],
                ],
                vec![[0, 1, 2], [1, 3, 2]],
            )
            .unwrap();
        field.step(300).unwrap();
        field
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 1.0,
                    ..ChassisCommand::default()
                },
            )
            .unwrap();
        field.step(3_000).unwrap();
        let snapshot = field.snapshot().chassis.remove(0);
        let x = snapshot.pose.translation_m[0];
        // The turret's front face reaches the roof edge and no further.
        let turret_front = chassis.turret_center_m[0] + chassis.turret_half_m[0];
        assert!(x + turret_front < 1.05 && x + turret_front > 0.8, "{x}");
        assert!(
            snapshot.velocity_m_s[0].abs() < 0.2,
            "{:?}",
            snapshot.velocity_m_s
        );
    }
    /// A closed 1 m box mesh centred at `center`.
    fn box_mesh(center: [f64; 3], half: f64) -> (Vec<[f64; 3]>, Vec<[u32; 3]>) {
        let [cx, cy, cz] = center;
        let mut vertices = Vec::new();
        for i in 0..8 {
            let sx = if i & 1 == 0 { -half } else { half };
            let sy = if i & 2 == 0 { -half } else { half };
            let sz = if i & 4 == 0 { -half } else { half };
            vertices.push([cx + sx, cy + sy, cz + sz]);
        }
        let triangles = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 4],
            [1, 5, 4],
            [2, 6, 3],
            [3, 6, 7],
            [0, 4, 2],
            [2, 4, 6],
            [1, 3, 5],
            [3, 7, 5],
        ];
        (vertices, triangles)
    }
    #[test]
    fn dynamic_geometry_tracks_bodies_and_excludes_terrain() {
        let mut field = Field::new(&FieldConfig {
            runes: Vec::new(),
            outposts: Vec::new(),
            ..Default::default()
        })
        .unwrap();
        assert!(
            field
                .dynamic_geometry_snapshot()
                .into_triangles()
                .0
                .is_empty()
        );
        let id = field
            .add_chassis(&ChassisPlacement {
                team: Team::Red,
                kind: RobotKind::Infantry,
                performance: None,
                config: Default::default(),
                spawn: Pose::at([0., 0., 2.]),
            })
            .unwrap();
        let before = field.dynamic_geometry_snapshot().into_triangles();
        assert!(!before.1.is_empty());
        field.step(100).unwrap();
        assert_ne!(
            before.0,
            field.dynamic_geometry_snapshot().into_triangles().0
        );
        field.remove_chassis(id).unwrap();
        assert!(
            field
                .dynamic_geometry_snapshot()
                .into_triangles()
                .0
                .is_empty()
        );
        field
            .fire(Pose::at([0., 0., 2.]), Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        let captured = field.dynamic_geometry_snapshot();
        field.step(10).unwrap();
        let projectile = captured.into_triangles();
        assert!(!projectile.1.is_empty());
        assert_ne!(
            projectile.0,
            field.dynamic_geometry_snapshot().into_triangles().0
        );
        assert!(field.static_geometry().0.is_empty());
    }

    #[test]
    fn fixed_geometry_snapshot_survives_field_changes_and_drop() {
        let mut field = Field::new(&ramp_field()).unwrap();
        add_ramp(&mut field);
        let expected = field.static_geometry();
        let captured = field.static_geometry_snapshot();
        let (vertices, triangles) = box_mesh([1.5, 0.0, 0.5], 0.5);
        field.add_static_mesh(vertices, triangles).unwrap();
        assert!(field.static_geometry().1.len() > expected.1.len());
        drop(field);
        let actual = std::thread::spawn(move || captured.into_triangles())
            .join()
            .unwrap();
        assert_eq!(actual, expected);
    }

    /// A triangle mesh block ahead of the chassis stops it and bounces
    /// projectiles; the physics geometry export shows it and the meshes.
    #[test]
    fn triangle_mesh_blocks_chassis_and_projectiles() {
        let config = ramp_field();
        let mut field = Field::new(&config).unwrap();
        assert_eq!(field.static_geometry().1.len(), 0);
        add_ramp(&mut field);
        assert_eq!(field.static_geometry().1.len(), 6);
        let (vertices, triangles) = box_mesh([1.5, 0.0, 0.5], 0.5);
        let count = triangles.len();
        field.add_static_mesh(vertices, triangles).unwrap();
        assert_eq!(field.static_geometry().1.len(), 6 + count);
        // A projectile fired at the block comes back.
        let muzzle = Pose::at([0.0, 0.0, 0.5]);
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(120).unwrap();
        let projectile = &field.snapshot().projectiles[0];
        assert!(projectile.position_m[0] < 1.0, "{projectile:?}");
        assert!(projectile.velocity_m_s[0] < 0.0, "{projectile:?}");
        // The chassis drives into the block and stops at its face.
        field
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 1.0,
                    ..ChassisCommand::default()
                },
            )
            .unwrap();
        field.step(3_000).unwrap();
        let chassis = field.snapshot().chassis.remove(0);
        let x = chassis.pose.translation_m[0];
        let front = config.chassis[0].config.body_half_m[0];
        assert!(x + front < 1.05 && x + front > 0.8, "{x}");
        assert!(
            chassis.velocity_m_s[0].abs() < 0.2,
            "{:?}",
            chassis.velocity_m_s
        );
    }
    #[test]
    fn chassis_stops_against_a_wall_and_rejects_bad_placements() {
        let config = ramp_field();
        let mut field = Field::new(&config).unwrap();
        // A wall across the path at x = 1.5 whose winding faces away from the
        // chassis, and a back-facing deck under it: mesh contacts are two-sided.
        field
            .add_static_mesh(
                vec![
                    [1.5, -1.0, 0.0],
                    [1.5, 1.0, 0.0],
                    [1.5, -1.0, 1.0],
                    [1.5, 1.0, 1.0],
                ],
                vec![[0, 1, 2], [1, 3, 2]],
            )
            .unwrap();
        field
            .add_static_mesh(
                vec![
                    [-1.0, -1.0, 0.03],
                    [-1.0, 1.0, 0.03],
                    [1.5, -1.0, 0.03],
                    [1.5, 1.0, 0.03],
                ],
                vec![[0, 1, 2], [1, 3, 2]],
            )
            .unwrap();
        field.step(300).unwrap();
        let resting = field.snapshot().chassis.remove(0);
        assert!(
            resting.wheels.iter().all(|w| w
                .contact
                .is_some_and(|c| (c.point_m[2] - 0.03).abs() < 1e-6)),
            "{resting:?}"
        );
        field
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 2.0,
                    ..Default::default()
                },
            )
            .unwrap();
        field.step(3_000).unwrap();
        let chassis = field.snapshot().chassis.remove(0);
        let front = chassis.pose.translation_m[0] + ChassisConfig::default().body_half_m[0];
        assert!(front < 1.52 && front > 1.3, "{chassis:?}");
        assert!(chassis.velocity_m_s[0].abs() < 0.2, "{chassis:?}");
        assert!(matches!(
            Field::new(&FieldConfig {
                chassis: vec![ChassisPlacement {
                    config: ChassisConfig {
                        mass_kg: -1.0,
                        ..ChassisConfig::default()
                    },
                    spawn: Pose::default(),
                    team: Team::Red,
                    kind: RobotKind::Infantry,
                    performance: None,
                }],
                ..FieldConfig::default()
            }),
            Err(FieldError::Chassis(_))
        ));
    }

    /// A field whose red team has activated the Big Rune once, so its rune
    /// only detects rings 4 to 10.
    fn field_after_one_big_activation(config: &FieldConfig) -> Field {
        let mut field = Field::new(config).unwrap();
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        field.step(ticks(5_000_000_000)).unwrap();
        field
            .referee_command(RefereeCommand::SkipTo {
                match_time_ns: 180_000_000_000,
            })
            .unwrap();
        field.step(1).unwrap();
        assert_eq!(field.runes()[0].kind(), RuneKind::Big);
        field
            .referee_command(RefereeCommand::ActivateRune { team: Team::Red })
            .unwrap();
        for _ in 0..12 {
            field.step(ticks(100_000_000)).unwrap();
            let time_ns = field.time_ns();
            let Some(blade) = field.runes()[0].snapshot().active_blade else {
                break;
            };
            let outcome = field.rune_mut(0).unwrap().hit(time_ns, blade).unwrap();
            if matches!(outcome, HitOutcome::GroupHit { .. }) {
                field.step(ticks(1_001_000_000)).unwrap();
            }
        }
        field.step(1).unwrap();
        assert_eq!(field.runes()[0].state(), RuneState::Activated);
        assert_eq!(field.referee().unwrap().rune_min_ring(0), 4);
        field
    }
    #[test]
    fn projectiles_on_a_disabled_ring_are_rejected() {
        let config = FieldConfig {
            runes: vec![RuneConfig::default()],
            outposts: Vec::new(),
            referee: Some(RefereeConfig::alternating(1, 0)),
            ..FieldConfig::default()
        };
        // Aim where blade 0 will be when the ball arrives (1.5 m at 25 m/s).
        let flight_ns = 60_000_000;
        let mut probe = field_after_one_big_activation(&config);
        probe.step(ticks(flight_ns)).unwrap();
        let target = probe.snapshot().runes[0].target_poses[0].translation_m;
        let shoot = |offset_y_m: f64| {
            let mut field = field_after_one_big_activation(&config);
            let muzzle = Pose::at([target[0] - 1.5, target[1] + offset_y_m, target[2] + 0.02]);
            field
                .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
                .unwrap();
            field.step(ticks(flight_ns) + ticks(100_000_000)).unwrap();
            let snapshot = field.snapshot();
            snapshot
                .hits
                .iter()
                .find(|hit| matches!(hit.target, ArmorTarget::Rune { rune: 0, .. }))
                .cloned()
                .unwrap_or_else(|| panic!("no rune hit: {:?}", snapshot.hits))
        };
        let outer = shoot(0.13);
        assert!(referee::ring_of(outer.local_offset_m) < 4, "{outer:?}");
        assert_eq!(outer.rejection, Some(Rejection::DisabledRing));
        assert!(!outer.detected);
        let centre = shoot(0.0);
        assert!(referee::ring_of(centre.local_offset_m) >= 4, "{centre:?}");
        assert!(centre.detected, "{centre:?}");
    }
    fn referee_events(field: &Field) -> Vec<RefereeEvent> {
        field
            .snapshot()
            .referee
            .unwrap()
            .events
            .into_iter()
            .map(|timed| timed.event)
            .collect()
    }
    /// A defeated robot respawns where it stands, weakened until its own
    /// outpost zone clears it; the rotor stops at 3:00 and a destroyed base
    /// ends the round with a result.
    #[test]
    fn a_match_respawns_in_place_stops_the_rotor_and_ends_on_a_base() {
        let (mut config, _) = stationary_outpost();
        config.outposts[0].speed_rad_s = 0.8;
        config.referee = Some(RefereeConfig::alternating(0, 1));
        config.zones = vec![zones::ZoneArea {
            kind: rm_simulator_gameplay::ZoneKind::Outpost,
            owner: Team::Red,
            pad: 0,
            floor_m: 0.0,
            polygon_m: vec![[3.0, 0.5], [5.0, 0.5], [5.0, 1.5], [3.0, 1.5]],
        }];
        let mut field = Field::new(&config).unwrap();
        let red = field
            .add_chassis(&ChassisPlacement {
                config: ChassisConfig::default(),
                spawn: Pose::at([4.0, 1.0, 1.0]),
                team: Team::Red,
                kind: RobotKind::Infantry,
                performance: None,
            })
            .unwrap();
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        // The match rotor rests through the countdown.
        let resting = field.snapshot().outposts[0].angle_rad;
        field.step(ticks(referee::COUNTDOWN_NS)).unwrap();
        assert_eq!(field.snapshot().outposts[0].angle_rad, resting);
        field.step(ticks(1_000_000_000)).unwrap();
        let before = field.snapshot().chassis[0].pose.translation_m;
        field
            .referee_command(RefereeCommand::SetRobotHp { robot: red, hp: 0 })
            .unwrap();
        assert!(field.snapshot().chassis[0].defeated);
        // Section 5.2.2: 10 s plus a tenth of the elapsed round.
        field.step(ticks(10_200_000_000)).unwrap();
        let events = referee_events(&field);
        assert!(events.contains(&RefereeEvent::RobotRespawned { robot: red }));
        assert!(events.contains(&RefereeEvent::WeaknessCleared { robot: red }));
        let snapshot = field.snapshot();
        let robot = &snapshot.referee.as_ref().unwrap().robots[0];
        assert_eq!((robot.hp, robot.weakened), (20, false));
        assert!(!snapshot.chassis[0].defeated);
        let after = snapshot.chassis[0].pose.translation_m;
        assert!((after[0] - before[0]).hypot(after[1] - before[1]) < 0.05);
        // Section 5.5.1: the living rotor stops and homes at 3:00.
        field
            .referee_command(RefereeCommand::SkipTo {
                match_time_ns: referee::OUTPOST_ROTOR_STOP_NS,
            })
            .unwrap();
        field.step(1).unwrap();
        let outpost = &field.snapshot().outposts[0];
        assert!(
            outpost.stopped_ns.is_some() && outpost.homing,
            "{outpost:?}"
        );
        field
            .referee_command(RefereeCommand::SetBaseHp {
                team: Team::Blue,
                hp: 0,
                shield_hp: 0,
            })
            .unwrap();
        field.step(1).unwrap();
        assert_eq!(field.referee().unwrap().phase(), MatchPhase::Finished);
        assert!(referee_events(&field).iter().any(|event| matches!(
            event,
            RefereeEvent::RoundResult(rm_simulator_gameplay::RoundResult::Decided {
                winner: Some(rm_simulator_gameplay::Team::Red),
                ..
            })
        )));
    }
    /// Section 5.5.3.9: 20 s on the opponent's Fortress after 3:00, with
    /// its outpost destroyed, expands that team's Base Protective Armor and
    /// opens the base mechanism.
    #[test]
    fn fortress_capture_opens_the_owners_base() {
        let (mut config, _) = stationary_outpost();
        config.referee = Some(RefereeConfig::alternating(0, 1));
        config.zones = zones::rmuc_2026();
        let mut field = Field::new(&config).unwrap();
        let fortress = config
            .zones
            .iter()
            .find(|z| z.kind == rm_simulator_gameplay::ZoneKind::Fortress && z.owner == Team::Red)
            .unwrap();
        let n = fortress.polygon_m.len() as f64;
        let x = fortress.polygon_m.iter().map(|p| p[0]).sum::<f64>() / n;
        let y = fortress.polygon_m.iter().map(|p| p[1]).sum::<f64>() / n;
        field
            .add_chassis(&ChassisPlacement {
                config: ChassisConfig::default(),
                spawn: Pose::at([x, y, ChassisConfig::default().rest_height_m()]),
                team: Team::Blue,
                kind: RobotKind::Infantry,
                performance: None,
            })
            .unwrap();
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        field.step(ticks(referee::COUNTDOWN_NS)).unwrap();
        field
            .referee_command(RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 })
            .unwrap();
        field
            .referee_command(RefereeCommand::SkipTo {
                match_time_ns: referee::OUTPOST_ROTOR_STOP_NS,
            })
            .unwrap();
        field.step(ticks(19_000_000_000)).unwrap();
        assert_eq!(field.snapshot().referee.unwrap().base_open, [false; 2]);
        field.step(ticks(1_100_000_000)).unwrap();
        assert_eq!(field.snapshot().referee.unwrap().base_open, [true, false]);
        assert!(
            referee_events(&field).contains(&RefereeEvent::BaseArmorExpanded { team: Team::Red })
        );
    }
    /// Driving armor into a wall during a round costs collision HP; idle
    /// practice and a slow approach do not.
    #[test]
    fn armor_collisions_cost_hp_only_in_a_running_round() {
        let run = |start: bool| {
            let mut field = Field::new(&FieldConfig {
                runes: Vec::new(),
                outposts: Vec::new(),
                referee: Some(RefereeConfig::alternating(0, 0)),
                ..FieldConfig::default()
            })
            .unwrap();
            field
                .add_chassis(&ChassisPlacement {
                    config: ChassisConfig::default(),
                    spawn: Pose::at([0.0, 0.0, 1.0]),
                    team: Team::Red,
                    kind: RobotKind::Infantry,
                    performance: None,
                })
                .unwrap();
            field
                .add_static_mesh(
                    vec![
                        [4.0, -2.0, 0.0],
                        [4.0, 2.0, 0.0],
                        [4.0, -2.0, 1.0],
                        [4.0, 2.0, 1.0],
                    ],
                    vec![[0, 1, 2], [1, 3, 2]],
                )
                .unwrap();
            if start {
                field.referee_command(RefereeCommand::StartMatch).unwrap();
            }
            field.step(ticks(referee::COUNTDOWN_NS)).unwrap();
            field
                .command_chassis(
                    0,
                    ChassisCommand {
                        forward_m_s: 3.0,
                        ..Default::default()
                    },
                )
                .unwrap();
            field.step(ticks(4_000_000_000)).unwrap();
            field.snapshot().referee.unwrap().robots[0].hp
        };
        let hp = run(true);
        assert!(hp < 200 && hp % referee::COLLISION_DAMAGE_HP == 0, "{hp}");
        assert_eq!(run(false), 200);
    }
    #[test]
    fn launches_follow_the_gameplay_rules_and_roster() {
        let mut field = Field::new(&FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..Default::default()
        })
        .unwrap();
        let placement = ChassisPlacement {
            config: ChassisConfig::default(),
            spawn: Pose::at([0.0, 0.0, 1.0]),
            team: Team::Red,
            kind: RobotKind::Infantry,
            performance: None,
        };
        let id = field.add_chassis(&placement).unwrap();
        let shot = Shot::at_limit(Caliber::Mm17);
        let muzzle = Pose::at([0.0, 0.0, 2.0]);
        let game = |field: &Field| field.snapshot().referee.unwrap().game;
        // Idle practice fires freely without accounting, but only the
        // robot's own caliber.
        field.fire(muzzle, shot, Some(id)).unwrap();
        assert_eq!(game(&field).robots[0].shots_launched, [0; 2]);
        assert_eq!(
            field.fire(muzzle, Shot::at_limit(Caliber::Mm42), Some(id)),
            Err(FieldError::Referee(
                "the robot has no launcher for that caliber"
            ))
        );
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        assert_eq!(
            field.fire(muzzle, shot, Some(id)),
            Err(FieldError::Referee("the round is not running"))
        );
        field.step(ticks(6_100_000_000)).unwrap();
        assert_eq!(field.referee().unwrap().phase(), MatchPhase::Running);
        // A failed launch changes nothing.
        let before = game(&field);
        let nan = Shot {
            speed_m_s: f64::NAN,
            ..shot
        };
        assert!(field.fire(muzzle, nan, Some(id)).is_err());
        assert_eq!(game(&field).robots, before.robots);
        // Table 5-14 cooling-focused launcher: a 40 heat limit, 10 per shot.
        for _ in 0..5 {
            field.fire(muzzle, shot, Some(id)).unwrap();
        }
        let robot = game(&field).robots.remove(0);
        assert_eq!(robot.shots_launched, [5, 0]);
        assert!(robot.overheated);
        assert_eq!(
            field.fire(muzzle, shot, Some(id)),
            Err(FieldError::Referee("the barrel is overheated"))
        );
        // Income arrived at 0:01, so ten rounds can be bought anywhere.
        field.buy_ammo(id, Caliber::Mm17).unwrap();
        assert_eq!(game(&field).robots[0].allowance, [10, 0]);
        field
            .referee_command(RefereeCommand::SetRobotHp { robot: id, hp: 0 })
            .unwrap();
        assert_eq!(
            field.fire(muzzle, shot, Some(id)),
            Err(FieldError::Referee("the robot is defeated"))
        );
        assert_eq!(field.chassis_defeated(id), Some(true));
        let other = field
            .add_chassis(&ChassisPlacement {
                spawn: Pose::at([3.0, 0.0, 1.0]),
                ..placement
            })
            .unwrap();
        field
            .referee_command(RefereeCommand::SetPolicy(rm_simulator_gameplay::Policy {
                enforce_allowance: true,
                exchange_requires_zone: false,
            }))
            .unwrap();
        assert_eq!(
            field.fire(muzzle, shot, Some(other)),
            Err(FieldError::Referee("no projectile allowance"))
        );
        field.remove_chassis(other).unwrap();
        field.referee_command(RefereeCommand::ResetMatch).unwrap();
        let reset = game(&field);
        assert_eq!(reset.robots.len(), 1);
        assert_eq!(reset.robots[0].shots_launched, [0; 2]);
        assert!(reset.robots[0].alive());
        assert!(reset.policy.enforce_allowance);
        assert_eq!(field.chassis_defeated(id), Some(false));
    }
    #[test]
    fn equipment_edits_affect_physics_defense_and_reset() {
        let (mut config, muzzle) = stationary_outpost();
        config.runes = FieldConfig::default().runes;
        config.referee = Some(RefereeConfig::alternating(1, 1));
        let mut field = Field::new(&config).unwrap();
        assert!(
            field
                .referee_command(RefereeCommand::SetOutpostHp { outpost: 8, hp: 1 })
                .is_err()
        );
        assert!(
            field
                .referee_command(RefereeCommand::SetOutpostHp {
                    outpost: 0,
                    hp: 1501
                })
                .is_err()
        );
        field
            .referee_command(RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 })
            .unwrap();
        assert!(field.snapshot().outposts[0].destroyed);
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        field.step(5000).unwrap();
        assert_eq!(field.snapshot().outposts[0].hp, 1500);
        field
            .referee_command(RefereeCommand::SetOutpostHp {
                outpost: 0,
                hp: 100,
            })
            .unwrap();
        field
            .referee_command(RefereeCommand::SetRuneBuff {
                team: Team::Red,
                defense_pct: 50,
                attack_pct: 100,
                cooling_multiplier: 1,
                duration_ns: 1_000_000_000,
            })
            .unwrap();
        field
            .fire(muzzle, Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        field.step(300).unwrap();
        assert_eq!(field.snapshot().outposts[0].hp, 90);
        field.step(700).unwrap();
        assert!(field.snapshot().referee.unwrap().teams[0].buff.is_none());
        field.referee_command(RefereeCommand::ResetMatch).unwrap();
        assert_eq!(field.snapshot().outposts[0].hp, 1500);
    }
}
