// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Projectile flight and raw armor contacts in a Rapier world that integrates
//! every explicit tick in fixed substeps of at most [`SUBSTEP_MAX_NS`]. Chassis
//! and projectiles share the same solver; target endpoint poses and mechanism
//! states come from the caller.
//!
//! Caliber helpers retain the simulator's nominal physical and rule lookup
//! values for compatibility. Stepping does not apply detection intervals,
//! damage, buffs or activation. The world crate owns those decisions.
use crate::{
    Pose, Team,
    chassis::{Chassis, ChassisCommand, ChassisConfig, ChassisSnapshot},
    tick_ns,
};
use rapier3d_f64::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::geometry::{MovingGeometry, StaticGeometry};

fn outside_projectile_bounds(bounds: Option<[[f64; 3]; 2]>, point: Vector, radius_m: f64) -> bool {
    bounds.is_some_and(|[low, high]| {
        (0..2)
            .any(|axis| point[axis] - radius_m <= low[axis] || point[axis] + radius_m >= high[axis])
    })
}

/// Downward acceleration the solver applies to every projectile, in metres per
/// second squared.
pub const GRAVITY_M_S2: f64 = 9.81;
/// Sea-level air and a smooth sphere; the rules only say speed decays.
const AIR_DENSITY_KG_M3: f64 = 1.2;
const DRAG_COEFFICIENT: f64 = 0.47;
/// Shore 90A plastic on plastic; assumed, not measured.
pub(crate) const RESTITUTION: f64 = 0.45;
/// Coulomb friction coefficient on every collider this library creates.
pub(crate) const FRICTION: f64 = 0.4;
/// Flight time after which a projectile is discarded, even while still rolling.
pub const MAX_FLIGHT_NS: u64 = 4_000_000_000;
/// Most projectiles one world keeps in flight. A further launch drops the
/// oldest.
pub const MAX_PROJECTILES: usize = 64;
/// Dwell window a resting ball must stay slow for before low-speed retirement
/// removes it, in nanoseconds. Only used when a policy asks for retirement.
pub const RETIRE_DWELL_NS: u64 = 50_000_000;
/// Residual world speed at or below which a ball resting on stationary scenery
/// is retired, in metres per second. A 42 mm ball at this speed carries 89 mJ,
/// 4% of the 2.2 J it needs to reach the Table 5-1 detection speed, so a ball
/// retired here cannot register a hit from where it lies. Chosen from a
/// sustained-fire measurement on the CAD field: it cuts the mean live-ball
/// count by 34% without changing aggregate scoring.
pub const RETIRE_SPEED_M_S: f64 = 2.0;
/// Longest integration slice of a world tick, in nanoseconds. A 17 mm shot at
/// the 25 m/s limit crosses further in one 128 Hz tick than the thin armour
/// housings are deep, and Rapier CCD does not recover the hit, so the ballistic
/// world never integrates more than this at once whatever the tick length, and
/// a tick no longer than this is a single slice.
const SUBSTEP_MAX_NS: u64 = 1_000_000;

/// Solver runs per world tick: the tick split into [`SUBSTEP_MAX_NS`] slices,
/// so a coarse tick still integrates ballistics at the fine step the contact
/// margins were tuned at.
fn substep_count() -> usize {
    usize::try_from(tick_ns().div_ceil(SUBSTEP_MAX_NS))
        .unwrap_or(1)
        .max(1)
}

/// How long a ball stays in the world: its own contact restitution, the hard
/// flight limit and the optional low-speed retirement rule.
///
/// The default keeps restitution 0.45 and the four-second flight limit, and
/// retires a ball that has stayed below [`RETIRE_SPEED_M_S`] on stationary
/// scenery for [`RETIRE_DWELL_NS`]. [`ProjectilePolicy::without_retirement`]
/// restores the earlier behaviour of holding every ball to its flight limit.
/// This is a simulation simplification knob, not a rulebook quantity.
///
/// Restitution applies to the projectile collider with
/// `CoefficientCombineRule::Min`. Rapier resolves a pair with the higher-priority
/// rule of the two colliders (`GeometricMean > ClampedSum > Max > Multiply >
/// Min > Average`), so a projectile against default-rule scenery or a kinematic
/// armor housing uses `min(policy, 0.45)`, and against a chassis, whose
/// colliders already ask for `Min` at 0.05, it stays 0.05. Scenery and chassis
/// pairs are untouched by this field.
///
/// ```
/// use rm_simulator_physics::projectile::ProjectilePolicy;
///
/// let default = ProjectilePolicy::default();
/// assert_eq!(default.restitution, 0.45);
/// assert_eq!(default.retire_speed_m_s, Some(2.0));
/// assert_eq!(default.without_retirement().retire_speed_m_s, None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectilePolicy {
    /// Contact restitution carried by every projectile collider, combined with
    /// `Min` as described above. Dimensionless, in `[0, 1]`.
    pub restitution: f64,
    /// Residual world speed at or below which a ball resting on stationary
    /// scenery may be retired, in metres per second. `None` keeps every ball
    /// until the flight limit, the bounds or the cap remove it.
    pub retire_speed_m_s: Option<f64>,
    /// How long the ball must stay slow and in contact before retirement, in
    /// nanoseconds. Separation or renewed motion resets the window.
    pub retire_dwell_ns: u64,
    /// Hard flight limit, in nanoseconds, after which a ball is discarded even
    /// while it is still rolling.
    pub max_flight_ns: u64,
}
impl Default for ProjectilePolicy {
    fn default() -> Self {
        Self {
            restitution: RESTITUTION,
            retire_speed_m_s: Some(RETIRE_SPEED_M_S),
            retire_dwell_ns: RETIRE_DWELL_NS,
            max_flight_ns: MAX_FLIGHT_NS,
        }
    }
}
impl ProjectilePolicy {
    /// The same policy with low-speed retirement switched off, so a ball is
    /// held until the flight limit, the arena bounds or the cap removes it.
    /// This is the rollback of the retirement change, exposed as
    /// `--no-projectile-retirement` on the app and the server binary.
    pub fn without_retirement(self) -> Self {
        Self {
            retire_speed_m_s: None,
            ..self
        }
    }
    /// Whether the policy is valid: a restitution in `[0, 1]`, a finite
    /// non-negative retirement speed and a non-zero flight limit.
    pub fn is_valid(&self) -> bool {
        self.restitution.is_finite()
            && (0.0..=1.0).contains(&self.restitution)
            && self
                .retire_speed_m_s
                .is_none_or(|speed| speed.is_finite() && speed >= 0.0)
            && self.max_flight_ns > 0
    }
}

/// Why a ball left the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemovalReason {
    /// The hard flight limit of [`ProjectilePolicy::max_flight_ns`] elapsed.
    Expired,
    /// The ball left the modelled volume: below the catch floor, beyond 200 m
    /// of the origin, or at a non-finite position.
    OutOfWorld,
    /// The ball crossed the arena's projectile bounds and was absorbed.
    OutOfBounds,
    /// A launch beyond [`MAX_PROJECTILES`] dropped the oldest ball.
    Capped,
    /// Low-speed post-contact retirement removed a spent ball.
    Retired,
}

/// One projectile removal, for lifetime and workload accounting. Instrumentation
/// only: nothing in stepping or scoring reads it, and it is not part of a
/// snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Removal {
    /// Identity of the ball, as returned by [`WorldPhysics::fire`].
    pub projectile: u64,
    /// Simulation time at which it was removed, in nanoseconds.
    pub time_ns: u64,
    /// Why it was removed.
    pub reason: RemovalReason,
    /// How long it lived, in nanoseconds.
    pub lifetime_ns: u64,
    /// Time between its first contact with anything and its removal, in
    /// nanoseconds; `None` when it never touched anything.
    pub since_first_contact_ns: Option<u64>,
    /// World speed at removal, in metres per second.
    pub speed_m_s: f64,
}
/// Most removals one world remembers before dropping the oldest.
const REMOVAL_MEMORY: usize = 4096;
/// Strikes are reported for this long after they happen.
pub const HIT_MEMORY_NS: u64 = 1_000_000_000;
/// Outpost middle armor effective detection area (Figure 5-16: the hatched
/// rectangle inset 5 mm from the 111 mm span, 16 + 2 + 58 + 2 + 16 mm tall)
/// and the fitted housing behind it.
pub const OUTPOST_TARGET_HALF_M: [f64; 2] = [
    crate::motion::outpost::DETECTION_WIDTH_M / 2.,
    crate::motion::outpost::DETECTION_HEIGHT_M / 2.,
];
/// Half extents of the small armor housing behind the face: the outpost's
/// middle armor and the chassis modules share it (Figure 5-16 shows the
/// same small module).
pub const SMALL_ARMOR_HOUSING_HALF_M: [f64; 3] = [0.0095, 0.0705, 0.0675];
/// Power Rune target: 308 mm physical disk, 300 mm effective detection disk.
pub const RUNE_PHYSICAL_RADIUS_M: f64 = 0.154;
/// Effective detection disk radius, in metres (300 mm diameter, Figure 5-18).
pub const RUNE_EFFECTIVE_RADIUS_M: f64 = 0.150;
/// Half extents of the rune target housing, in metres: 5 mm through the disk,
/// which makes it 10 mm thick, and the physical disk radius in both face
/// directions.
pub const RUNE_HOUSING_HALF_M: [f64; 3] = [0.005, RUNE_PHYSICAL_RADIUS_M, RUNE_PHYSICAL_RADIUS_M];

/// Projectile caliber. Detection speed, damage and cadence are keyed to these
/// two sizes (Table 4-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Caliber {
    /// 17 mm, the Infantry, Drone and Sentry projectile.
    Mm17,
    /// 42 mm, the Hero projectile outside deployment mode.
    Mm42,
}
impl Caliber {
    /// Table 4-1 nominal projectile size.
    pub fn diameter_m(self) -> f64 {
        match self {
            Self::Mm17 => 0.0168,
            Self::Mm42 => 0.0425,
        }
    }
    /// Table 4-1 nominal projectile weight.
    pub fn mass_kg(self) -> f64 {
        match self {
            Self::Mm17 => 0.0032,
            Self::Mm42 => 0.0445,
        }
    }
    /// Quadratic drag acceleration is `-k * |velocity| * velocity`.
    /// Uses the same assumed air density and sphere drag as world stepping.
    pub fn drag_factor_per_m(self) -> f64 {
        let area = std::f64::consts::PI * (self.diameter_m() / 2.).powi(2);
        0.5 * AIR_DENSITY_KG_M3 * DRAG_COEFFICIENT * area / self.mass_kg()
    }
    /// Initial launching speed limit: Infantry, Drone and Sentry 17 mm (25 m/s);
    /// Hero 42 mm outside deployment mode (12 m/s).
    pub fn launch_speed_limit_m_s(self) -> f64 {
        match self {
            Self::Mm17 => 25.0,
            Self::Mm42 => 12.0,
        }
    }
    /// Table 5-1: normal contact speed above which robot, base and outpost armor
    /// detects the projectile. The Power Rune detects only 17 mm.
    pub fn armor_detection_speed_m_s(self) -> f64 {
        match self {
            Self::Mm17 => 12.0,
            Self::Mm42 => 10.0,
        }
    }
    /// Section 5.1.1 minimum detection interval per armor module.
    pub fn detection_interval_ns(self) -> u64 {
        match self {
            Self::Mm17 => 50_000_000,
            Self::Mm42 => 200_000_000,
        }
    }
    /// Table 5-2 outpost middle armor damage without buffs.
    pub fn outpost_damage(self) -> u32 {
        match self {
            Self::Mm17 => 20,
            Self::Mm42 => 200,
        }
    }
    /// Table 5-2 robot small armor damage without buffs (10 HP per 17 mm
    /// and 100 HP per 42 mm strike; taken from memory of the table, not
    /// re-read from the manual).
    pub fn robot_damage(self) -> u32 {
        match self {
            Self::Mm17 => 10,
            Self::Mm42 => 100,
        }
    }
}

/// One launch request. Speed is the referee's initial launching speed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shot {
    /// Projectile size and mass class.
    pub caliber: Caliber,
    /// Launch speed along the muzzle's +x, in metres per second.
    /// [`WorldPhysics::fire`] rejects anything outside `(0, 40]`.
    pub speed_m_s: f64,
}
impl Shot {
    /// The caliber at its rule limit.
    ///
    /// ```
    /// use rm_simulator_physics::{Caliber, Shot};
    ///
    /// assert_eq!(Shot::at_limit(Caliber::Mm17).speed_m_s, 25.0);
    /// assert_eq!(Shot::at_limit(Caliber::Mm42).speed_m_s, 12.0);
    /// ```
    pub fn at_limit(caliber: Caliber) -> Self {
        Self {
            caliber,
            speed_m_s: caliber.launch_speed_limit_m_s(),
        }
    }
}

/// Identity of one scoring face, as reported by [`Contact::target`]. Callers
/// give each face its own pose; the world layer applies detection and damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ArmorTarget {
    /// A base armor plate.
    Base {
        /// Index of the base in the field's base list.
        base: u32,
        /// Index of the plate within that base.
        plate: u32,
    },
    /// An outpost armor module on the rotating tower.
    Outpost {
        /// Index of the outpost in the field's outpost list.
        outpost: u32,
        /// Index of the face, which follows the rotor angle.
        face: u32,
    },
    /// A Power Rune target disk.
    Rune {
        /// Index of the rune in the field's rune list.
        rune: u32,
        /// Index of the blade, 0 to 4.
        blade: u32,
    },
    /// A chassis armor module: `plate` indexes `ChassisConfig::armor_faces`.
    Chassis {
        /// Id assigned by [`WorldPhysics::add_chassis`].
        chassis: u32,
        /// Index of the module: front, left, back, right.
        plate: u32,
    },
}

/// One ball in flight, complete enough to rebuild it after a restart.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectileSnapshot {
    /// Identity assigned at launch, unique among live projectiles.
    pub id: u64,
    /// Size and mass class of the ball.
    pub caliber: Caliber,
    /// Simulation time of launch, in nanoseconds.
    pub launched_ns: u64,
    /// Ball centre in world FLU metres.
    pub position_m: [f64; 3],
    /// Linear velocity in metres per second.
    pub velocity_m_s: [f64; 3],
    /// Angular velocity in radians per second.
    pub angular_velocity_rad_s: [f64; 3],
    /// The chassis credited with the shot, when a pilot fired it, so a
    /// restored field scores the ball for the same robot the host does.
    pub shooter: Option<u32>,
    /// Simulation time of the ball's first contact with anything, in
    /// nanoseconds, or `None` while it has never touched. Diagnostic only.
    pub first_contact_ns: Option<u64>,
    /// Start of the open low-speed retirement dwell window, in nanoseconds, or
    /// `None` when no window is open. Carried so a restored field retires the
    /// same ball on the same tick as the field it was captured from.
    pub dwell_since_ns: Option<u64>,
}

/// One armor scoring face: origin at the face centre, +x the outward normal,
/// +y along the width and +z along the height.
///
/// ```
/// use rm_simulator_physics::{motion::outpost::rotate, ArmorTarget, Pose, TargetFace};
///
/// // A face turned by pi looks back down -x, so it is shot from +x.
/// let face = TargetFace {
///     target: ArmorTarget::Outpost { outpost: 0, face: 0 },
///     pose: Pose::yawed([2.0, 0.0, 1.0], std::f64::consts::PI),
/// };
/// let width_axis = rotate(face.pose.rotation_wxyz, [0.0, 1.0, 0.0]);
/// let height_axis = rotate(face.pose.rotation_wxyz, [0.0, 0.0, 1.0]);
/// assert!(width_axis[1] < -0.99); // +y of the face runs along world -y
/// assert!(height_axis[2] > 0.99); // +z of the face stays world up
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetFace {
    /// Which scoring face this pose belongs to.
    pub target: ArmorTarget,
    /// Face centre pose: +x outward normal, +y width, +z height.
    pub pose: Pose,
}

/// Scoring target identities and their poses at both ends of one physics tick.
///
/// Construction fixes the identity order. Updating a frame may change poses but
/// cannot silently add, remove or reorder a target.
///
/// ```
/// use rm_simulator_physics::{ArmorTarget, Pose, TargetFace, TargetFrames};
///
/// let target = ArmorTarget::Rune { rune: 0, blade: 0 };
/// let mut frames = TargetFrames::new(vec![TargetFace {
///     target,
///     pose: Pose::at([3.0, 0.0, 1.0]),
/// }]);
///
/// // One tick of a plate sliding 5 mm along -x: start pose then end pose.
/// frames.begin(|faces| faces[0].pose.translation_m[0] -= 0.005)?;
/// frames.update(|faces| faces[0].pose.translation_m[0] -= 0.005)?;
/// assert_eq!(frames.start()[0].target, target);
///
/// // Swapping two faces is rejected, because contact order must stay stable.
/// let mut two = TargetFrames::new(vec![
///     TargetFace { target, pose: Pose::at([1.0, 0.0, 1.0]) },
///     TargetFace {
///         target: ArmorTarget::Rune { rune: 0, blade: 1 },
///         pose: Pose::at([2.0, 0.0, 1.0]),
///     },
/// ]);
/// assert!(two.update(|faces| faces.swap(0, 1)).is_err());
/// # Ok::<(), &'static str>(())
/// ```
pub struct TargetFrames {
    identities: Vec<ArmorTarget>,
    start: Vec<TargetFace>,
    end: Vec<TargetFace>,
}
impl TargetFrames {
    /// Fix the identity order from `faces`, which also become the start and end
    /// poses of the first tick.
    pub fn new(faces: Vec<TargetFace>) -> Self {
        Self {
            identities: faces.iter().map(|face| face.target).collect(),
            start: faces.clone(),
            end: faces,
        }
    }
    /// Write the start-of-tick poses through `write`. Errors with
    /// `"target face count changed"` or `"target face order changed"`, leaving
    /// the rejected poses in place.
    pub fn begin(&mut self, write: impl FnOnce(&mut Vec<TargetFace>)) -> Result<(), &'static str> {
        write(&mut self.start);
        self.validate(&self.start)
    }
    /// Write the end-of-tick poses through `write`. Applies the same identity
    /// check as [`Self::begin`].
    pub fn update(&mut self, write: impl FnOnce(&mut Vec<TargetFace>)) -> Result<(), &'static str> {
        write(&mut self.end);
        self.validate(&self.end)
    }
    fn validate(&self, faces: &[TargetFace]) -> Result<(), &'static str> {
        if faces.len() != self.identities.len() {
            return Err("target face count changed");
        }
        if self
            .identities
            .iter()
            .zip(faces)
            .any(|(identity, face)| *identity != face.target)
        {
            return Err("target face order changed");
        }
        Ok(())
    }
    fn motions(&self) -> impl Iterator<Item = (&TargetFace, &TargetFace)> {
        self.start.iter().zip(&self.end)
    }
    fn motion(&self, index: usize) -> (&TargetFace, &TargetFace) {
        (&self.start[index], &self.end[index])
    }
    /// Target poses at the start of the current tick.
    pub fn start(&self) -> &[TargetFace] {
        &self.start
    }
}

/// A raw contact between a projectile and an armor collider during one tick.
/// Each ball is reported at most once per armor collider while it keeps
/// touching it, so a resting or bouncing ball does not repeat.
#[derive(Clone, Debug, PartialEq)]
pub struct Contact {
    /// Identity of the ball, as returned by [`WorldPhysics::fire`].
    pub projectile: u64,
    /// Chassis credited with the shot, when a pilot fired it.
    pub shooter: Option<u32>,
    /// Size and mass class of the ball.
    pub caliber: Caliber,
    /// The scoring face that was touched.
    pub target: ArmorTarget,
    /// Contact point in world FLU metres.
    pub position_m: [f64; 3],
    /// Contact point in the target face frame at the end of the tick.
    /// [`scoring_offset`] decides whether it lies on the scoring area.
    pub local_m: [f64; 3],
    /// Closing speed along the face normal, in metres per second. Negative
    /// when the ball is moving away from the face.
    pub normal_speed_m_s: f64,
}

struct Projectile {
    id: u64,
    shooter: Option<u32>,
    caliber: Caliber,
    launched_ns: u64,
    body: RigidBodyHandle,
    collider: ColliderHandle,
    /// Armor colliders touched at the end of the previous tick.
    touching: Vec<ColliderHandle>,
    /// Simulation time of the first contact with anything, if it happened.
    first_contact_ns: Option<u64>,
    /// Start of the current slow-and-touching-scenery window, if one is open.
    dwell_since_ns: Option<u64>,
}
struct TargetBody {
    target: ArmorTarget,
    body: RigidBodyHandle,
}
#[derive(Clone, Copy)]
enum Scorer {
    Face(usize),
    Armor { chassis: u32, plate: u32 },
}
/// How a struck plate was moving during the tick.
#[derive(Clone, Copy)]
enum PlateMotion {
    Kinematic { face_prev: Pose3 },
    Body(RigidBodyHandle),
}

/// The field's single Rapier scene and its dynamic physics participants.
///
/// One world holds the static collision geometry, the target faces it can
/// score on, the projectiles in flight and every chassis. Stepping is
/// deterministic for a fixed tick schedule.
pub struct WorldPhysics {
    /// Convex XY slab outside which projectiles are absorbed, in world FLU
    /// metres. `None` means no perimeter absorption. A hit beyond the slab
    /// scores nothing, which also catches a fast shot that crosses a whole
    /// wall in one tick.
    pub projectile_bounds_m: Option<[[f64; 3]; 2]>,
    /// Restitution, flight limit and low-speed retirement rule applied to
    /// projectiles created from now on. Changing it does not alter balls
    /// already in flight.
    policy: ProjectilePolicy,
    /// Recent removals, oldest first, bounded to [`REMOVAL_MEMORY`].
    removals: std::collections::VecDeque<Removal>,
    world: PhysicsWorld,
    projectiles: Vec<Projectile>,
    /// Velocities before integration, in the current projectile order.
    velocities_before: Vec<Vector>,
    chassis: Vec<Chassis>,
    next_chassis_id: u32,
    targets: Vec<TargetBody>,
    mechanisms: Vec<(
        ColliderHandle,
        crate::motion::Mechanism,
        crate::Team,
        [[f64; 3]; 2],
    )>,
    /// Every scoring collider: a face index into `targets`, or a chassis
    /// armor module.
    target_of: HashMap<ColliderHandle, Scorer>,
    /// Field time the target bodies currently stand at, if any.
    synced_ns: Option<u64>,
    /// The broad-phase tree has been filled at least once.
    primed: bool,
    next_id: u64,
    launched: u64,
}

/// Convert an FLU array in metres to a Rapier vector.
pub(crate) fn vector(v: [f64; 3]) -> Vector {
    Vector::new(v[0], v[1], v[2])
}
fn pose(p: Pose) -> Pose3 {
    let [w, x, y, z] = p.rotation_wxyz;
    Pose3::from_parts(
        vector(p.translation_m),
        Rotation::from_xyzw(x, y, z, w).normalize(),
    )
}
fn finite(v: [f64; 3]) -> bool {
    v.iter().all(|x| x.is_finite())
}
fn pose_is_valid(p: Pose) -> bool {
    let norm = p.rotation_wxyz.iter().map(|v| v * v).sum::<f64>();
    finite(p.translation_m) && norm.is_finite() && (norm - 1.).abs() < 1e-6
}

impl WorldPhysics {
    /// A world with a flat floor at `floor_height_m`. Target bodies are created
    /// for every face; their poses are supplied at each step.
    ///
    /// ```
    /// use rm_simulator_physics::{Caliber, Pose, Shot, tick_ns, TargetFrames, WorldPhysics};
    ///
    /// let mut physics = WorldPhysics::new(&[], 0.0);
    /// let frames = TargetFrames::new(Vec::new());
    ///
    /// // A slow ball falls, rolls and settles one ball radius above the floor.
    /// physics.fire(
    ///     0,
    ///     Pose::at([0.0, 0.0, 1.0]),
    ///     Shot { caliber: Caliber::Mm42, speed_m_s: 3.0 },
    ///     None,
    /// )?;
    /// assert!(!physics.is_idle());
    /// for tick in 0..2_000_000_000 / tick_ns() {
    ///     physics.step(tick * tick_ns(), &frames)?;
    /// }
    ///
    /// let resting = physics.snapshot();
    /// let radius_m = Caliber::Mm42.diameter_m() / 2.0;
    /// let resting_z_m = resting[0].position_m[2];
    /// assert!((resting_z_m - radius_m).abs() < 0.01, "{resting_z_m}");
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn new(faces: &[TargetFace], floor_height_m: f64) -> Self {
        let mut world = PhysicsWorld::new();
        world.gravity = Vector::new(0., 0., -GRAVITY_M_S2);
        let substep_s = tick_ns() as f64 * 1e-9 / substep_count() as f64;
        world.integration_parameters.dt = substep_s;
        world.integration_parameters.min_ccd_dt = substep_s / 100.;
        world.integration_parameters.max_ccd_substeps = 4;
        world.insert_collider(
            ColliderBuilder::new(SharedShape::halfspace(Vector::Z))
                .translation(Vector::new(0., 0., floor_height_m))
                .friction(FRICTION)
                .restitution(RESTITUTION),
            None,
        );
        let mut ballistics = Self {
            world,
            projectile_bounds_m: None,
            policy: ProjectilePolicy::default(),
            removals: std::collections::VecDeque::new(),
            projectiles: Vec::new(),
            velocities_before: Vec::new(),
            chassis: Vec::new(),
            next_chassis_id: 0,
            targets: Vec::new(),
            mechanisms: Vec::new(),
            target_of: HashMap::new(),
            synced_ns: None,
            primed: false,
            next_id: 0,
            launched: 0,
        };
        for face in faces {
            ballistics.add_target(*face);
        }
        ballistics
    }
    fn add_target(&mut self, face: TargetFace) {
        let housing_half_m = match face.target {
            ArmorTarget::Outpost { .. } | ArmorTarget::Base { .. } => SMALL_ARMOR_HOUSING_HALF_M,
            ArmorTarget::Rune { .. } => RUNE_HOUSING_HALF_M,
            ArmorTarget::Chassis { .. } => SMALL_ARMOR_HOUSING_HALF_M,
        };
        let body = self.world.insert_body(
            RigidBodyBuilder::kinematic_position_based()
                .pose(pose(face.pose))
                .can_sleep(false),
        );
        let [x, y, z] = housing_half_m;
        let collider = self.world.insert_collider(
            ColliderBuilder::cuboid(x, y, z)
                .translation(Vector::new(-x, 0., 0.))
                .friction(FRICTION)
                .restitution(RESTITUTION),
            Some(body),
        );
        self.target_of
            .insert(collider, Scorer::Face(self.targets.len()));
        self.targets.push(TargetBody {
            target: face.target,
            body,
        });
    }
    /// A fixed box; used for tower bodies that projectiles must not pass through.
    pub fn add_static_box(&mut self, center: Pose, size_m: [f64; 3]) {
        self.world.insert_collider(
            ColliderBuilder::cuboid(size_m[0] / 2., size_m[1] / 2., size_m[2] / 2.)
                .position(pose(center))
                .friction(FRICTION)
                .restitution(RESTITUTION),
            None,
        );
    }
    /// Add a fixed triangle mesh that stops chassis but not projectiles. The
    /// collider joins group 3 with a filter that rejects group 2, the
    /// projectile group; [`Self::projectile_bounds_m`] absorbs shots that
    /// leave the arena instead.
    pub fn add_boundary_mesh(
        &mut self,
        vertices_m: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> Result<(), &'static str> {
        let handle = self.add_static_mesh(vertices_m, triangles)?;
        self.world.colliders[handle].set_collision_groups(InteractionGroups::new(
            Group::GROUP_3,
            Group::ALL & !Group::GROUP_2,
            InteractionTestMode::And,
        ));
        Ok(())
    }
    /// A fixed triangle mesh in world FLU metres, for example the field CAD.
    pub fn add_static_mesh(
        &mut self,
        vertices_m: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> Result<ColliderHandle, &'static str> {
        if !vertices_m.iter().all(|v| finite(*v)) {
            return Err("mesh vertices must be finite");
        }
        let count = vertices_m.len() as u32;
        if triangles.iter().flatten().any(|index| *index >= count) {
            return Err("mesh triangle index out of range");
        }
        let vertices = vertices_m.into_iter().map(vector).collect();
        // Contacts stay two-sided (no internal-edge fixing, which drops
        // back-face contacts), so a CAD part with reversed winding still
        // carries wheels and stops bodies.
        let collider = ColliderBuilder::trimesh_with_flags(
            vertices,
            triangles,
            TriMeshFlags::MERGE_DUPLICATE_VERTICES | TriMeshFlags::DELETE_DEGENERATE_TRIANGLES,
        )
        .map_err(|_| "mesh must hold at least one triangle")?
        .friction(FRICTION)
        .restitution(RESTITUTION);
        Ok(self.world.insert_collider(collider, None))
    }
    /// Add one articulated part as a fixed mesh that
    /// [`Self::sync_mechanisms`] translates. `offsets_m` are the part's world
    /// translations in metres at mechanism fraction 0 and 1, and the vertices
    /// describe the part in its untranslated frame.
    pub fn add_mechanism_mesh(
        &mut self,
        vertices: Vec<[f64; 3]>,
        triangles: Vec<[u32; 3]>,
        mechanism: crate::motion::Mechanism,
        team: crate::Team,
        offsets_m: [[f64; 3]; 2],
    ) -> Result<(), &'static str> {
        if !offsets_m.iter().all(|v| finite(*v)) {
            return Err("nonfinite mechanism offset");
        }
        let handle = self.add_static_mesh(vertices, triangles)?;
        self.mechanisms.push((handle, mechanism, team, offsets_m));
        Ok(())
    }
    /// Whether resolving articulated scenery is necessary for this world.
    #[inline]
    pub fn has_mechanisms(&self) -> bool {
        !self.mechanisms.is_empty()
    }
    /// Only a moved mechanism is written back: marking an unchanged CAD
    /// collider dirty every millisecond rebuilds broad-phase work during replay.
    pub fn sync_mechanisms(&mut self, state: &crate::motion::MechanismState) {
        for (handle, mechanism, team, offsets) in &self.mechanisms {
            let fraction = state.fraction(*mechanism, *team);
            let position = vector(std::array::from_fn(|i| {
                offsets[0][i] + (offsets[1][i] - offsets[0][i]) * fraction
            }));
            let collider = &mut self.world.colliders[*handle];
            if collider.translation() != position {
                collider.set_translation(position);
            }
        }
    }
    /// Remove every half-space collider, which turns off the flat catch floor.
    /// A loaded CAD layout calls this once it has its own ground and boundary.
    pub fn remove_catch_floor(&mut self) {
        let handles: Vec<_> = self
            .world
            .all_colliders()
            .filter(|(_, c)| matches!(c.shape().as_typed_shape(), TypedShape::HalfSpace(_)))
            .map(|(h, _)| h)
            .collect();
        for handle in handles {
            self.world.remove_collider(handle);
        }
    }
    /// Retain fixed shapes without copying their triangle buffers.
    /// `include_mechanisms` keeps the articulated parts too, at the transforms
    /// they currently hold.
    pub fn static_geometry_snapshot(&self, include_mechanisms: bool) -> StaticGeometry {
        let colliders: Vec<_> = self
            .world
            .all_colliders()
            .filter(|(handle, collider)| {
                collider.parent().is_none()
                    && (include_mechanisms || !self.mechanisms.iter().any(|(h, ..)| h == handle))
                    && matches!(
                        collider.shape().as_typed_shape(),
                        TypedShape::TriMesh(_) | TypedShape::Cuboid(_)
                    )
            })
            .collect();
        StaticGeometry {
            bounds: Default::default(),
            projectile_bounds_m: self.projectile_bounds_m,
            no_catch_floor: !self
                .world
                .all_colliders()
                .any(|(_, c)| matches!(c.shape().as_typed_shape(), TypedShape::HalfSpace(_))),
            collision_groups: colliders
                .iter()
                .map(|(_, c)| c.collision_groups())
                .collect(),
            shapes: colliders
                .iter()
                .map(|(_, c)| (*c.position(), c.shared_shape().clone()))
                .collect(),
            mechanisms: colliders
                .iter()
                .enumerate()
                .filter_map(|(index, (handle, _))| {
                    self.mechanisms
                        .iter()
                        .find(|(h, ..)| h == handle)
                        .map(|(_, kind, team, offsets)| (index, *kind, *team, *offsets))
                })
                .collect(),
        }
    }
    /// Capture body-attached shapes at their current physics poses: chassis
    /// body, turret and armor colliders plus any articulated part, but no
    /// static mesh.
    pub fn dynamic_geometry_snapshot(&self) -> StaticGeometry {
        StaticGeometry {
            bounds: Default::default(),
            projectile_bounds_m: self.projectile_bounds_m,
            collision_groups: Vec::new(),
            mechanisms: Vec::new(),
            no_catch_floor: !self
                .world
                .all_colliders()
                .any(|(_, c)| matches!(c.shape().as_typed_shape(), TypedShape::HalfSpace(_))),
            shapes: self
                .world
                .all_colliders()
                .filter(|(handle, collider)| {
                    collider.is_enabled()
                        && (collider.parent().is_some()
                            || self.mechanisms.iter().any(|(h, ..)| h == handle))
                })
                .map(|(_, collider)| (*collider.position(), collider.shared_shape().clone()))
                .collect(),
        }
    }
    /// Nothing moves on its own: no projectile in flight and no chassis.
    pub fn is_idle(&self) -> bool {
        self.projectiles.is_empty() && self.chassis.is_empty()
    }
    /// Place a driven chassis in the world and return its id.
    ///
    /// ```
    /// use rm_simulator_physics::{
    ///     chassis::{ChassisCommand, ChassisConfig},
    ///     Pose, TargetFrames, Team, WorldPhysics, tick_ns,
    /// };
    ///
    /// let config = ChassisConfig::default();
    /// let mut physics = WorldPhysics::new(&[], 0.0);
    /// let id = physics.add_chassis(
    ///     Team::Red,
    ///     config.clone(),
    ///     // Spawn a little high; the wheel rays find the floor on the first tick.
    ///     Pose::at([0.0, 0.0, config.rest_height_m() + 0.05]),
    /// )?;
    ///
    /// let frames = TargetFrames::new(Vec::new());
    /// for tick in 0..300 {
    ///     physics.step(tick * tick_ns(), &frames)?;
    /// }
    ///
    /// // Each spring carries a quarter of the weight, so the body settles a few
    /// // millimetres below its unloaded height with all four wheels down.
    /// let settled = physics.chassis_snapshot(id).unwrap();
    /// assert!(settled.pose.translation_m[2] < config.rest_height_m());
    /// assert!(settled.wheels.iter().all(|wheel| wheel.contact.is_some()));
    ///
    /// // Drive forward at 1 m/s for two seconds.
    /// physics.command_chassis(
    ///     id,
    ///     ChassisCommand { forward_m_s: 1.0, ..Default::default() },
    /// )?;
    /// for tick in 300..2_300 {
    ///     physics.step(tick * tick_ns(), &frames)?;
    /// }
    /// let driven = physics.chassis_snapshot(id).unwrap();
    /// assert!(driven.pose.translation_m[0] > 1.5, "{:?}", driven.pose);
    /// assert!((driven.velocity_m_s[0] - 1.0).abs() < 0.05);
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn add_chassis(
        &mut self,
        team: Team,
        config: ChassisConfig,
        spawn: Pose,
    ) -> Result<u32, &'static str> {
        let id = self.next_chassis_id;
        let chassis = Chassis::new(&mut self.world, id, team, config, spawn)?;
        self.next_chassis_id += 1;
        for (plate, &collider) in chassis.armor_colliders().iter().enumerate() {
            self.target_of.insert(
                collider,
                Scorer::Armor {
                    chassis: id,
                    plate: plate as u32,
                },
            );
        }
        self.chassis.push(chassis);
        Ok(id)
    }
    /// Take a chassis out of the world; its id is never reused.
    pub fn remove_chassis(&mut self, id: u32) -> Result<(), &'static str> {
        let index = self
            .chassis
            .iter()
            .position(|chassis| chassis.id() == id)
            .ok_or("no chassis with that id")?;
        let chassis = self.chassis.remove(index);
        for collider in chassis.armor_colliders() {
            self.target_of.remove(collider);
        }
        chassis.remove(&mut self.world);
        Ok(())
    }
    /// Move an existing chassis to `spawn` and clear its body and wheel
    /// velocity. The id, armor and defeat state survive; a live robot re-aims
    /// to the spawn yaw.
    pub fn place_chassis(&mut self, id: u32, spawn: Pose) -> Result<(), &'static str> {
        let chassis = self
            .chassis
            .iter_mut()
            .find(|c| c.id() == id)
            .ok_or("no chassis with that id")?;
        chassis.place(&mut self.world, spawn)
    }
    fn chassis_ref(&self, id: u32) -> Option<&Chassis> {
        self.chassis.iter().find(|chassis| chassis.id() == id)
    }
    /// Cut or restore a chassis' drive; ignored for ids no longer present.
    pub fn set_chassis_defeated(&mut self, id: u32, defeated: bool) {
        if let Ok(chassis) = self.chassis_mut(id)
            && chassis.is_defeated() != defeated
        {
            chassis.set_defeated(defeated);
        }
    }
    /// Whether the chassis is defeated, or `None` when no chassis has that id.
    pub fn chassis_defeated(&self, id: u32) -> Option<bool> {
        self.chassis_ref(id).map(Chassis::is_defeated)
    }
    fn chassis_mut(&mut self, id: u32) -> Result<&mut Chassis, &'static str> {
        self.chassis
            .iter_mut()
            .find(|chassis| chassis.id() == id)
            .ok_or("no chassis with that id")
    }
    /// Set the drive and aim the chassis uses on the next tick. Errors when no
    /// chassis has that id or any component is not finite.
    pub fn command_chassis(
        &mut self,
        id: u32,
        command: ChassisCommand,
    ) -> Result<(), &'static str> {
        self.chassis_mut(id)?.set_command(command)
    }
    /// The configuration the chassis was built with, or `None` when no chassis
    /// has that id.
    pub fn chassis_config(&self, id: u32) -> Option<&ChassisConfig> {
        self.chassis_ref(id).map(Chassis::config)
    }
    /// The team the chassis plays for, or `None` when no chassis has that id.
    pub fn chassis_team(&self, id: u32) -> Option<Team> {
        self.chassis_ref(id).map(Chassis::team)
    }
    /// Body centres of every chassis in id order, in world FLU metres.
    pub fn chassis_positions_m(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.chassis
            .iter()
            .map(|chassis| self.world.bodies[chassis.body()].translation().to_array())
    }
    /// World pose of the gun muzzle, `MUZZLE_FORWARD_M` ahead of the turret
    /// pivot. `None` when no chassis has that id.
    pub fn chassis_muzzle_pose(&self, id: u32) -> Option<Pose> {
        self.chassis_ref(id)
            .map(|chassis| chassis.muzzle_pose(&self.world))
    }
    /// One chassis' state without walking the rules, for a replay that reads
    /// its own robot every tick and nothing else.
    pub fn chassis_snapshot(&self, id: u32) -> Option<ChassisSnapshot> {
        self.chassis_ref(id).map(|c| c.snapshot(&self.world))
    }
    /// Every chassis snapshot, in id order.
    pub fn chassis_snapshots(&self) -> Vec<ChassisSnapshot> {
        self.chassis
            .iter()
            .map(|chassis| chassis.snapshot(&self.world))
            .collect()
    }
    /// Total launches since the world was created. [`Self::restore_identity`]
    /// sets it again for a restored field.
    pub fn launched(&self) -> u64 {
        self.launched
    }
    /// Launch along the muzzle's +x at `time_ns`, credited to `shooter`'s
    /// chassis when given. The oldest shot makes room when full.
    ///
    /// Errors when the muzzle pose is not finite or is not a unit rotation,
    /// when `shooter` names no live chassis or a defeated one, or when the
    /// speed leaves `(0, 40]`.
    ///
    /// ```
    /// use rm_simulator_physics::{Caliber, Pose, Shot, TargetFrames, WorldPhysics};
    ///
    /// let mut physics = WorldPhysics::new(&[], 0.0);
    /// let id = physics.fire(
    ///     0,
    ///     // Yawed a quarter turn left, so the muzzle's +x runs along +y.
    ///     Pose::yawed([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2),
    ///     Shot::at_limit(Caliber::Mm17),
    ///     None,
    /// )?;
    /// assert_eq!(id, 0);
    /// assert_eq!(physics.launched(), 1);
    ///
    /// let frames = TargetFrames::new(Vec::new());
    /// physics.step(0, &frames)?;
    /// let velocity_m_s = physics.snapshot()[0].velocity_m_s;
    /// assert!(velocity_m_s[1] > 24.0, "{velocity_m_s:?}");
    ///
    /// // A launch needs a unit rotation and a speed strictly above zero.
    /// let bad = physics.fire(
    ///     0,
    ///     Pose::default(),
    ///     Shot { caliber: Caliber::Mm17, speed_m_s: 0.0 },
    ///     None,
    /// );
    /// assert!(bad.is_err());
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn fire(
        &mut self,
        time_ns: u64,
        muzzle: Pose,
        shot: Shot,
        shooter: Option<u32>,
    ) -> Result<u64, &'static str> {
        if !pose_is_valid(muzzle) {
            return Err("muzzle pose must be finite with a unit rotation");
        }
        if let Some(id) = shooter {
            match self.chassis_defeated(id) {
                None => return Err("no chassis with that id"),
                Some(true) => return Err("a defeated robot cannot fire"),
                Some(false) => {}
            }
        }
        if !shot.speed_m_s.is_finite() || shot.speed_m_s <= 0. || shot.speed_m_s > 40. {
            return Err("launch speed must be in (0, 40] m/s");
        }
        if self.projectiles.len() >= MAX_PROJECTILES {
            let oldest = self.projectiles.remove(0);
            self.record_removal(&oldest, time_ns, RemovalReason::Capped);
            self.world.remove_body(oldest.body);
        }
        let muzzle = pose(muzzle);
        let velocity = muzzle.transform_vector(Vector::new(shot.speed_m_s, 0., 0.));
        let radius = shot.caliber.diameter_m() / 2.;
        let body = self.world.insert_body(
            RigidBodyBuilder::dynamic()
                .translation(muzzle.translation)
                .linvel(velocity)
                .ccd_enabled(true)
                .can_sleep(false),
        );
        let collider = self.world.insert_collider(
            ColliderBuilder::ball(radius)
                .collision_groups(InteractionGroups::new(
                    Group::GROUP_2,
                    Group::ALL,
                    InteractionTestMode::And,
                ))
                .mass(shot.caliber.mass_kg())
                .friction(FRICTION)
                .restitution(self.policy.restitution)
                .restitution_combine_rule(CoefficientCombineRule::Min),
            Some(body),
        );
        let id = self.next_id;
        self.next_id += 1;
        self.launched += 1;
        self.projectiles.push(Projectile {
            id,
            shooter,
            caliber: shot.caliber,
            launched_ns: time_ns,
            body,
            collider,
            touching: Vec::new(),
            first_contact_ns: None,
            dwell_since_ns: None,
        });
        Ok(id)
    }
    /// The lifetime policy new projectiles are created with.
    pub fn projectile_policy(&self) -> ProjectilePolicy {
        self.policy
    }
    /// Choose the lifetime policy for projectiles created from now on. Balls
    /// already in flight keep the restitution they were built with, so a caller
    /// that wants one policy for a whole run sets it before the first launch.
    ///
    /// ```
    /// use rm_simulator_physics::{projectile::ProjectilePolicy, WorldPhysics};
    ///
    /// let mut physics = WorldPhysics::new(&[], 0.0);
    /// let policy = ProjectilePolicy {
    ///     restitution: 0.15,
    ///     retire_speed_m_s: Some(1.0),
    ///     ..ProjectilePolicy::default()
    /// };
    /// physics.set_projectile_policy(policy)?;
    /// assert_eq!(physics.projectile_policy(), policy);
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn set_projectile_policy(&mut self, policy: ProjectilePolicy) -> Result<(), &'static str> {
        if !policy.is_valid() {
            return Err("invalid projectile policy");
        }
        self.policy = policy;
        Ok(())
    }
    /// Take the recent removal records, oldest first, leaving none behind.
    /// Instrumentation for lifetime studies; stepping never reads them.
    pub fn take_removals(&mut self) -> Vec<Removal> {
        self.removals.drain(..).collect()
    }
    fn record_removal(&mut self, projectile: &Projectile, time_ns: u64, reason: RemovalReason) {
        let body = &self.world.bodies[projectile.body];
        if self.removals.len() == REMOVAL_MEMORY {
            self.removals.pop_front();
        }
        self.removals.push_back(Removal {
            projectile: projectile.id,
            time_ns,
            reason,
            lifetime_ns: time_ns.saturating_sub(projectile.launched_ns),
            since_first_contact_ns: projectile
                .first_contact_ns
                .map(|first| time_ns.saturating_sub(first)),
            speed_m_s: body.linvel().length(),
        });
    }
    /// Placement revision of one chassis, or `None` when no chassis has that
    /// id. It changes when the chassis is placed or revived, so a client never
    /// interpolates across robot lives.
    pub fn chassis_revision(&self, id: u32) -> Option<u64> {
        self.chassis_ref(id).map(|c| c.revision())
    }
    /// Insert a ball at a captured state, keeping its identity and age.
    /// Its contact cache starts empty: a ball already resting against armor
    /// registers that touch again on the first restored tick.
    pub fn restore_projectile(&mut self, state: &ProjectileSnapshot) -> Result<(), &'static str> {
        if !state
            .position_m
            .into_iter()
            .chain(state.velocity_m_s)
            .chain(state.angular_velocity_rad_s)
            .all(f64::is_finite)
        {
            return Err("invalid projectile checkpoint");
        }
        if self.projectiles.len() >= MAX_PROJECTILES {
            return Err("too many restored projectiles");
        }
        let radius = state.caliber.diameter_m() / 2.;
        let body = self.world.insert_body(
            RigidBodyBuilder::dynamic()
                .translation(vector(state.position_m))
                .linvel(vector(state.velocity_m_s))
                .angvel(vector(state.angular_velocity_rad_s))
                .ccd_enabled(true)
                .can_sleep(false),
        );
        let collider = self.world.insert_collider(
            ColliderBuilder::ball(radius)
                .collision_groups(InteractionGroups::new(
                    Group::GROUP_2,
                    Group::ALL,
                    InteractionTestMode::And,
                ))
                .mass(state.caliber.mass_kg())
                .friction(FRICTION)
                .restitution(self.policy.restitution)
                .restitution_combine_rule(CoefficientCombineRule::Min),
            Some(body),
        );
        self.projectiles.push(Projectile {
            id: state.id,
            shooter: state.shooter,
            caliber: state.caliber,
            launched_ns: state.launched_ns,
            body,
            collider,
            touching: Vec::new(),
            first_contact_ns: state.first_contact_ns,
            dwell_since_ns: state.dwell_since_ns,
        });
        Ok(())
    }
    /// Rebuild a chassis under its recorded id, with its armor scorers.
    pub fn restore_chassis(&mut self, state: &ChassisSnapshot) -> Result<(), &'static str> {
        if self.chassis.iter().any(|c| c.id() == state.id) {
            return Err("chassis id already restored");
        }
        let mut chassis = Chassis::new(
            &mut self.world,
            state.id,
            state.team,
            state.config.clone(),
            state.pose,
        )?;
        chassis.restore_prediction(&mut self.world, state)?;
        for (plate, &collider) in chassis.armor_colliders().iter().enumerate() {
            self.target_of.insert(
                collider,
                Scorer::Armor {
                    chassis: state.id,
                    plate: plate as u32,
                },
            );
        }
        self.chassis.push(chassis);
        self.next_chassis_id = self.next_chassis_id.max(state.id.saturating_add(1));
        Ok(())
    }
    /// Put an existing chassis back on a captured state. Prediction only: the
    /// authoritative field never rewinds a body this way.
    pub fn reset_chassis(&mut self, state: &ChassisSnapshot) -> Result<(), &'static str> {
        let chassis = self
            .chassis
            .iter_mut()
            .find(|c| c.id() == state.id)
            .ok_or("no chassis with that id")?;
        if chassis.config() != &state.config {
            return Err("chassis configuration changed");
        }
        chassis.restore_prediction(&mut self.world, state)
    }
    /// Insert shared collision shapes captured from another field, keeping
    /// their mechanism roles so `sync_mechanisms` still moves them.
    pub fn insert_geometry(&mut self, geometry: &StaticGeometry) {
        self.projectile_bounds_m = geometry.projectile_bounds_m;
        if geometry.no_catch_floor {
            self.remove_catch_floor();
        }
        let mut handles = Vec::with_capacity(geometry.shapes.len());
        for (index, (position, shape)) in geometry.shapes.iter().enumerate() {
            handles.push(
                self.world.insert_collider(
                    ColliderBuilder::new(shape.clone())
                        .collision_groups(
                            geometry
                                .collision_groups
                                .get(index)
                                .copied()
                                .unwrap_or_default(),
                        )
                        .position(*position)
                        .friction(FRICTION)
                        .restitution(RESTITUTION),
                    None,
                ),
            );
        }
        for (index, kind, team, offsets) in &geometry.mechanisms {
            self.mechanisms
                .push((handles[*index], *kind, *team, *offsets));
        }
    }
    /// The next id [`Self::add_chassis`] will hand out. Ids are never reused,
    /// even after a chassis leaves.
    pub fn next_chassis_id(&self) -> u32 {
        self.next_chassis_id
    }
    /// Continue the host's identity counters, so restored ids are never reissued.
    pub fn restore_identity(&mut self, next_chassis_id: u32, launched: u64) {
        self.next_chassis_id = self.next_chassis_id.max(next_chassis_id);
        self.next_id = self.next_id.max(launched);
        self.launched = launched;
    }
    /// Wheel rays read the broad-phase tree, which the pipeline fills while
    /// stepping. A world that has never stepped has an empty one, so the first
    /// tick would find no ground under any wheel; seed it with what is there.
    fn prime_queries(&mut self) {
        let parameters = self.world.integration_parameters;
        let bounds: Vec<_> = self
            .world
            .colliders
            .iter()
            .filter(|(_, collider)| collider.is_enabled())
            .map(|(handle, collider)| (handle, collider.compute_aabb()))
            .filter(|(_, aabb)| aabb.mins.is_finite() && aabb.maxs.is_finite())
            .collect();
        for (handle, aabb) in bounds {
            self.world.broad_phase.set_aabb(&parameters, handle, aabb);
        }
    }
    /// Declare the target bodies already standing at `time_ns`, so the first
    /// restored tick moves them instead of teleporting them into place.
    pub fn mark_synced(&mut self, time_ns: u64) {
        self.synced_ns = Some(time_ns);
    }
    /// Advance one tick from `prev_ns`, taking the target face poses at the
    /// start and end of the tick from `frames`.
    ///
    /// The solver runs [`SUBSTEP_MAX_NS`]-bounded slices, interpolating the
    /// target poses across the tick, so a coarse tick still carries fast shots
    /// past no armour between solver runs. Returns every contact first seen
    /// this tick, oldest projectile first. Applying detection intervals,
    /// damage and buffs is the caller's business. Errors when the face count or
    /// identity order no longer matches the targets the world was built with.
    ///
    /// ```
    /// use rm_simulator_physics::{
    ///     ArmorTarget, Caliber, Pose, Shot, TargetFace, TargetFrames, WorldPhysics, tick_ns,
    /// };
    ///
    /// let face = TargetFace {
    ///     target: ArmorTarget::Outpost { outpost: 0, face: 0 },
    ///     pose: Pose::yawed([1.5, 0.0, 1.0], std::f64::consts::PI),
    /// };
    /// let mut physics = WorldPhysics::new(&[face], 0.0);
    /// let frames = TargetFrames::new(vec![face]);
    ///
    /// // Two shots one tick apart, side by side so neither can strike the other.
    /// let first = physics.fire(
    ///     0, Pose::at([0.0, 0.0, 1.025]), Shot::at_limit(Caliber::Mm17), None)?;
    /// let second = physics.fire(
    ///     tick_ns(), Pose::at([0.0, 0.04, 1.025]), Shot::at_limit(Caliber::Mm17), None)?;
    ///
    /// let mut seen = Vec::new();
    /// for tick in 0..200 {
    ///     for contact in physics.step(tick * tick_ns(), &frames)? {
    ///         assert_eq!(contact.target, face.target);
    ///         assert!(contact.normal_speed_m_s > 20.0, "{contact:?}");
    ///         seen.push(contact.projectile);
    ///     }
    /// }
    /// assert_eq!(seen, vec![first, second]);
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn step(
        &mut self,
        prev_ns: u64,
        frames: &TargetFrames,
    ) -> Result<Vec<Contact>, &'static str> {
        let next_ns = prev_ns + tick_ns();
        if frames.start.len() != self.targets.len() {
            return Err("target face count changed");
        }
        if !self.primed {
            self.prime_queries();
            self.primed = true;
        }
        // Discard spent shots before they cost physics time.
        let mut index = 0;
        while index < self.projectiles.len() {
            let projectile = &self.projectiles[index];
            let position = self.world.bodies[projectile.body].translation();
            let reason =
                if next_ns.saturating_sub(projectile.launched_ns) > self.policy.max_flight_ns {
                    Some(RemovalReason::Expired)
                } else if position.z < -1. || position.length() > 200. || !position.is_finite() {
                    Some(RemovalReason::OutOfWorld)
                } else if outside_projectile_bounds(
                    self.projectile_bounds_m,
                    position,
                    projectile.caliber.diameter_m() / 2.0,
                ) {
                    Some(RemovalReason::OutOfBounds)
                } else {
                    None
                };
            if let Some(reason) = reason {
                let projectile = self.projectiles.remove(index);
                self.record_removal(&projectile, next_ns, reason);
                self.world.remove_body(projectile.body);
            } else {
                index += 1;
            }
        }
        if self.is_idle() {
            self.synced_ns = None;
            return Ok(Vec::new());
        }
        let teleport = self.synced_ns != Some(prev_ns);
        for (target, (start, end)) in self.targets.iter().zip(frames.motions()) {
            if start.target != target.target || end.target != target.target {
                return Err("target face order changed");
            }
            if !pose_is_valid(start.pose) || !pose_is_valid(end.pose) {
                return Err("target face pose must be finite with a unit rotation");
            }
        }
        let policy = self.policy;
        let substeps = substep_count();
        let substep_s = tick_ns() as f64 * 1e-9 / substeps as f64;
        let mut contacts = Vec::new();
        for substep in 0..substeps {
            let frac_prev = substep as f64 / substeps as f64;
            let frac_next = (substep + 1) as f64 / substeps as f64;
            for (target, (start, end)) in self.targets.iter().zip(frames.motions()) {
                let body = &mut self.world.bodies[target.body];
                if teleport && substep == 0 {
                    body.set_position(pose(start.pose), true);
                }
                body.set_next_kinematic_position(pose(start.pose).lerp(&pose(end.pose), frac_next));
            }
            self.velocities_before.clear();
            for projectile in &self.projectiles {
                let body = &mut self.world.bodies[projectile.body];
                let velocity = body.linvel();
                let area = std::f64::consts::PI * (projectile.caliber.diameter_m() / 2.).powi(2);
                let drag = -0.5 * AIR_DENSITY_KG_M3 * DRAG_COEFFICIENT * area * velocity.length();
                body.reset_forces(false);
                body.add_force(velocity * drag, false);
                self.velocities_before.push(velocity);
            }
            for chassis in &mut self.chassis {
                chassis.apply_forces(&mut self.world, substep_s);
            }
            self.world.step();
            contacts
                .extend(self.capture_contacts(frames, frac_prev, frac_next, substep_s, next_ns));
            // Absorb perimeter shots the slice they cross the wall, so a fast
            // shot cannot reach past the perimeter and bounce back inside. The
            // convex XY volume also catches one that traverses an entire wall
            // in a single slice.
            let mut index = 0;
            while index < self.projectiles.len() {
                let shot = &self.projectiles[index];
                let position = self.world.bodies[shot.body].translation();
                let reason = if outside_projectile_bounds(
                    self.projectile_bounds_m,
                    position,
                    shot.caliber.diameter_m() / 2.0,
                ) {
                    Some(RemovalReason::OutOfBounds)
                } else if policy.retire_speed_m_s.is_some()
                    && shot.dwell_since_ns.is_some_and(|since| {
                        next_ns.saturating_sub(since) >= policy.retire_dwell_ns
                    })
                {
                    Some(RemovalReason::Retired)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    let shot = self.projectiles.remove(index);
                    self.record_removal(&shot, next_ns, reason);
                    self.world.remove_body(shot.body);
                } else {
                    index += 1;
                }
            }
        }
        self.synced_ns = Some(next_ns);
        Ok(contacts)
    }
    /// Contacts first seen by the substep that just stepped, which carried the
    /// target motions between `frac_prev` and `frac_next` of the tick and
    /// integrated for `dt_s` seconds. Latches and dwell windows stamp
    /// `now_ns`, the end of the containing tick.
    fn capture_contacts(
        &mut self,
        frames: &TargetFrames,
        frac_prev: f64,
        frac_next: f64,
        dt_s: f64,
        now_ns: u64,
    ) -> Vec<Contact> {
        let policy = self.policy;
        let mut contacts = Vec::new();
        for (projectile, velocity) in self
            .projectiles
            .iter_mut()
            .zip(self.velocities_before.iter().copied())
        {
            let mut touching = Vec::new();
            // Any touch at all starts the ball's post-contact history; only a
            // fixed body counts as the stationary scenery retirement needs.
            let mut touched_anything = false;
            let mut touching_scenery = false;
            for pair in self.world.contact_pairs_with(projectile.collider) {
                if !pair.has_any_active_contact() {
                    continue;
                }
                let other = if pair.collider1 == projectile.collider {
                    pair.collider2
                } else {
                    pair.collider1
                };
                touched_anything = true;
                // Mechanism parts are parentless colliders too, but they move;
                // a ball resting on one is not on stationary scenery.
                touching_scenery |= match self.world.colliders[other].parent() {
                    Some(body) => self.world.bodies[body].is_fixed(),
                    None => !self.mechanisms.iter().any(|(handle, ..)| *handle == other),
                };
                let Some(&scorer) = self.target_of.get(&other) else {
                    continue;
                };
                touching.push(other);
                if projectile.touching.contains(&other) {
                    continue;
                }
                // The scoring face now, and how the plate moves: a kinematic
                // face by where it was a tick ago, a chassis by its body.
                let (target_of_scorer, face_next, motion) = match scorer {
                    Scorer::Face(index) => {
                        let (start, end) = frames.motion(index);
                        let from = pose(start.pose);
                        let to = pose(end.pose);
                        (
                            self.targets[index].target,
                            from.lerp(&to, frac_next),
                            PlateMotion::Kinematic {
                                face_prev: from.lerp(&to, frac_prev),
                            },
                        )
                    }
                    Scorer::Armor { chassis, plate } => {
                        let Some(owner) = self.chassis.iter().find(|c| c.id() == chassis) else {
                            continue;
                        };
                        (
                            ArmorTarget::Chassis { chassis, plate },
                            owner.armor_pose(&self.world, plate as usize),
                            PlateMotion::Body(owner.body()),
                        )
                    }
                };
                let world_point = pair
                    .find_deepest_contact()
                    .map(|(_, contact)| {
                        let (collider, local) = if pair.collider1 == projectile.collider {
                            (pair.collider2, contact.local_p2)
                        } else {
                            (pair.collider1, contact.local_p1)
                        };
                        self.world.colliders[collider]
                            .position()
                            .transform_point(local)
                    })
                    .unwrap_or_else(|| {
                        // Project the ball centre onto the housing front.
                        let centre = self.world.bodies[projectile.body].translation();
                        let mut local = face_next.inverse_transform_point(centre);
                        local.x = local.x.min(0.);
                        face_next.transform_point(local)
                    });
                // A shot is already absorbed before an out-of-arena armor contact.
                if outside_projectile_bounds(self.projectile_bounds_m, world_point, 0.0) {
                    continue;
                }
                let local = face_next.inverse_transform_point(world_point);
                let plate_velocity = match motion {
                    PlateMotion::Kinematic { face_prev } => {
                        (face_next.transform_point(local) - face_prev.transform_point(local)) / dt_s
                    }
                    PlateMotion::Body(body) => {
                        self.world.bodies[body].velocity_at_point(world_point)
                    }
                };
                let normal = face_next.transform_vector(Vector::X);
                let normal_speed_m_s = -(velocity - plate_velocity).dot(normal);
                contacts.push(Contact {
                    projectile: projectile.id,
                    shooter: projectile.shooter,
                    caliber: projectile.caliber,
                    target: target_of_scorer,
                    position_m: world_point.to_array(),
                    local_m: local.to_array(),
                    normal_speed_m_s,
                });
            }
            projectile.touching = touching;
            if touched_anything && projectile.first_contact_ns.is_none() {
                projectile.first_contact_ns = Some(now_ns);
            }
            // A spent ball has touched stationary scenery, still touches it and
            // stays below the residual speed for the whole dwell window. Any
            // separation or renewed motion reopens the window.
            let speed_m_s = self.world.bodies[projectile.body].linvel().length();
            let slow = policy
                .retire_speed_m_s
                .is_some_and(|limit| speed_m_s <= limit);
            if touching_scenery && slow {
                projectile.dwell_since_ns.get_or_insert(now_ns);
            } else {
                projectile.dwell_since_ns = None;
            }
        }
        contacts
    }
    /// Every projectile in flight, oldest first.
    pub fn snapshot(&self) -> Vec<ProjectileSnapshot> {
        self.projectiles
            .iter()
            .map(|projectile| {
                let body = &self.world.bodies[projectile.body];
                ProjectileSnapshot {
                    id: projectile.id,
                    caliber: projectile.caliber,
                    launched_ns: projectile.launched_ns,
                    position_m: body.translation().to_array(),
                    velocity_m_s: body.linvel().to_array(),
                    angular_velocity_rad_s: body.angvel().to_array(),
                    shooter: projectile.shooter,
                    first_contact_ns: projectile.first_contact_ns,
                    dwell_since_ns: projectile.dwell_since_ns,
                }
            })
            .collect()
    }
}

/// Whether a contact lies on the scoring face, and the scoring offset when it does.
///
/// The contact must sit near the face plane and inside the module's detection
/// area: the 101 x 94 mm rectangle of Figure 5-16 for an outpost, base or
/// chassis face, or the rune's 150 mm effective disk. The returned offset is
/// the contact's face-frame `[y, z]` in metres. Detection speed, intervals and
/// damage stay with the caller.
///
/// ```
/// use rm_simulator_physics::{projectile::scoring_offset, ArmorTarget};
///
/// let outpost = ArmorTarget::Outpost { outpost: 0, face: 1 };
/// // A front strike inside the detection rectangle returns its face offset.
/// assert_eq!(scoring_offset(outpost, [0.001, 0.05, 0.04]), Some([0.05, 0.04]));
/// // Outside the rectangle, or a strike on the housing back, does not score.
/// assert_eq!(scoring_offset(outpost, [0.0, 0.06, 0.0]), None);
/// assert_eq!(scoring_offset(outpost, [-0.019, 0.0, 0.0]), None);
///
/// // The rune scores anywhere inside its effective disk.
/// let rune = ArmorTarget::Rune { rune: 0, blade: 2 };
/// assert_eq!(scoring_offset(rune, [0.002, 0.1, 0.1]), Some([0.1, 0.1]));
/// assert_eq!(scoring_offset(rune, [0.002, 0.152, 0.0]), None);
/// ```
pub fn scoring_offset(target: ArmorTarget, local_m: [f64; 3]) -> Option<[f64; 2]> {
    let [x, y, z] = local_m;
    // Housing sits behind the face; a front strike reports x near zero.
    if !(-0.004..=0.012).contains(&x) {
        return None;
    }
    let inside = match target {
        ArmorTarget::Outpost { .. } | ArmorTarget::Chassis { .. } | ArmorTarget::Base { .. } => {
            y.abs() <= OUTPOST_TARGET_HALF_M[0] && z.abs() <= OUTPOST_TARGET_HALF_M[1]
        }
        ArmorTarget::Rune { .. } => y.hypot(z) <= RUNE_EFFECTIVE_RADIUS_M,
    };
    inside.then_some([y, z])
}
#[cfg(test)]
mod tests {
    use super::*;
    fn face(target: ArmorTarget, x: f64, yaw_rad: f64) -> TargetFace {
        TargetFace {
            target,
            pose: Pose::yawed([x, 0., 1.], yaw_rad),
        }
    }
    #[test]
    fn cached_clearance_matches_exhaustive_queries_after_motion() {
        let mut physics = WorldPhysics::new(&[], 0.);
        for x in 0..50 {
            physics.add_static_box(Pose::yawed([x as f64, 1., 1.], 0.4), [0.4, 0.6, 1.]);
        }
        let geometry = physics.static_geometry_snapshot(true);
        for geometry in [
            geometry.clone(),
            geometry.interpolate(&geometry, 0.4).unwrap(),
            geometry.fixed_only(),
        ] {
            for x in -10..510 {
                let point = [x as f64 / 10., 0.7, 1.];
                let center = Pose3::from_translation(vector(point));
                let ball = Ball::new(0.15);
                let exhaustive = geometry.shapes.iter().all(|(pose, shape)| {
                    rapier3d_f64::parry::query::intersection_test(
                        &center,
                        &ball,
                        pose,
                        shape.as_ref(),
                    )
                    .is_ok_and(|hit| !hit)
                });
                assert_eq!(geometry.sphere_clear(point, 0.15), exhaustive, "{point:?}");
            }
        }
    }

    #[test]
    fn sight_segments_stop_at_walls_and_reject_invalid_coordinates() {
        let geometry = StaticGeometry {
            shapes: vec![(Pose3::IDENTITY, SharedShape::cuboid(0.5, 0.5, 0.5))],
            ..Default::default()
        };
        assert!(!geometry.segment_clear([-2., 0., 0.], [2., 0., 0.]));
        assert!(geometry.segment_clear([-2., 0., 0.], [-1., 0., 0.]));
        assert!(geometry.segment_clear([-2., 1., 0.], [2., 1., 0.]));
        assert!(!geometry.segment_clear([f64::NAN, 0., 0.], [2., 0., 0.]));
    }

    #[test]
    fn moving_clearance_invalidates_cached_bounds() {
        let mut geometry = StaticGeometry {
            shapes: vec![(Pose3::IDENTITY, SharedShape::cuboid(0.5, 0.5, 0.5))],
            mechanisms: vec![(
                0,
                crate::motion::Mechanism::DartDoor,
                Team::Red,
                [[0.; 3], [10., 0., 0.]],
            )],
            ..Default::default()
        };
        assert!(!geometry.sphere_clear([0.; 3], 0.1));
        let moved = geometry.at_mechanisms(&crate::motion::MechanismState::default());
        assert!(moved.sphere_clear([0.; 3], 0.1));
        assert!(!moved.sphere_clear([10., 0., 0.], 0.1));
        geometry = geometry.interpolate(&moved, 0.5).unwrap();
        assert!(!geometry.sphere_clear([5., 0., 0.], 0.1));
        assert!(geometry.sphere_clear([10., 0., 0.], 0.1));
    }

    #[test]
    fn perimeter_absorbs_fast_and_high_shots_and_survives_restore() {
        let mut physics = WorldPhysics::new(&[], 0.);
        physics.projectile_bounds_m = Some([[-1., -1., 0.], [1., 1., 2.]]);
        physics
            .add_boundary_mesh(
                vec![[1., -1., 0.], [1., 1., 0.], [1., -1., 5.], [1., 1., 5.]],
                vec![[0, 1, 2], [1, 3, 2]],
            )
            .unwrap();
        let geometry = physics.static_geometry_snapshot(true);
        let mut restored = WorldPhysics::new(&[], 0.);
        restored.insert_geometry(&geometry);
        assert_eq!(
            geometry.collision_groups,
            restored.static_geometry_snapshot(true).collision_groups
        );
        let frames = TargetFrames::new(Vec::new());
        for world in [&mut physics, &mut restored] {
            world
                .fire(
                    0,
                    Pose::at([0.98, 0., 4.]),
                    Shot {
                        caliber: Caliber::Mm17,
                        speed_m_s: 40.,
                    },
                    None,
                )
                .unwrap();
            world.step(0, &frames).unwrap();
            assert!(world.projectiles.is_empty());
        }
    }

    #[test]
    fn armor_beyond_perimeter_cannot_score() {
        let target = face(
            ArmorTarget::Outpost {
                outpost: 0,
                face: 0,
            },
            1.03,
            std::f64::consts::PI,
        );
        let mut physics = WorldPhysics::new(&[target], 0.);
        physics.projectile_bounds_m = Some([[-1., -1., 0.], [1., 1., 2.]]);
        physics
            .fire(
                0,
                Pose::at([0.98, 0., 1.]),
                Shot {
                    caliber: Caliber::Mm17,
                    speed_m_s: 40.,
                },
                None,
            )
            .unwrap();
        assert!(
            physics
                .step(0, &TargetFrames::new(vec![target]))
                .unwrap()
                .is_empty()
        );
        assert!(physics.projectiles.is_empty());
    }

    #[test]
    fn target_frames_reject_identity_reordering() {
        let a = face(ArmorTarget::Rune { rune: 0, blade: 0 }, 1.0, 0.0);
        let b = face(ArmorTarget::Rune { rune: 0, blade: 1 }, 2.0, 0.0);
        let mut stable = TargetFrames::new(vec![a, b]);
        let allocations = (stable.start.as_ptr(), stable.end.as_ptr());
        for x in 3..100 {
            stable
                .update(|faces| {
                    faces.clear();
                    faces.extend([face(a.target, x as f64, 0.0), b]);
                })
                .unwrap();
        }
        assert_eq!(allocations, (stable.start.as_ptr(), stable.end.as_ptr()));

        let mut frames = TargetFrames::new(vec![a, b]);
        assert_eq!(
            frames.update(|faces| faces.swap(0, 1)),
            Err("target face order changed")
        );
        let mut frames = TargetFrames::new(vec![a, b]);
        assert_eq!(
            frames.begin(|faces| {
                faces.pop();
            }),
            Err("target face count changed")
        );
    }
    /// Ticks in `n` milliseconds of world time at the configured rate, so the
    /// tests below state their durations rather than 1 kHz tick counts.
    fn ms(n: u64) -> u64 {
        n * 1_000_000 / tick_ns()
    }
    /// Step `ticks` from `start`; returns the contacts and the tick of the first one.
    fn run(
        ballistics: &mut WorldPhysics,
        faces: &[TargetFace],
        start: u64,
        ticks: u64,
    ) -> (Vec<Contact>, u64) {
        let mut contacts = Vec::new();
        let frames = TargetFrames::new(faces.to_vec());
        let mut first = None;
        for tick in start..start + ticks {
            let found = ballistics.step(tick * tick_ns(), &frames).unwrap();
            if !found.is_empty() && first.is_none() {
                first = Some(tick + 1);
            }
            contacts.extend(found);
        }
        (contacts, first.unwrap_or(0))
    }

    #[test]
    fn scoring_face_accepts_front_strikes_inside_the_target_only() {
        let outpost = ArmorTarget::Outpost {
            outpost: 0,
            face: 1,
        };
        assert_eq!(
            scoring_offset(outpost, [0.001, 0.05, -0.02]),
            Some([0.05, -0.02])
        );
        // The whole hatched face scores, not only the light bar band.
        assert_eq!(
            scoring_offset(outpost, [0., 0.05, 0.046]),
            Some([0.05, 0.046])
        );
        assert_eq!(scoring_offset(outpost, [0., 0.052, 0.]), None);
        assert_eq!(scoring_offset(outpost, [0., 0., 0.048]), None);
        assert_eq!(scoring_offset(outpost, [-0.019, 0., 0.]), None);
        let rune = ArmorTarget::Rune { rune: 1, blade: 3 };
        assert_eq!(scoring_offset(rune, [0.002, 0.1, 0.1]), Some([0.1, 0.1]));
        assert_eq!(scoring_offset(rune, [0.002, 0.152, 0.]), None);
        assert_eq!(scoring_offset(rune, [0.03, 0., 0.]), None);
    }
    #[test]
    fn straight_shot_strikes_the_facing_plate_with_the_launch_speed() {
        let target = ArmorTarget::Outpost {
            outpost: 0,
            face: 0,
        };
        let faces = [face(target, 1.5, std::f64::consts::PI)];
        let mut ballistics = WorldPhysics::new(&faces, 0.0);
        let id = ballistics
            .fire(
                0,
                Pose::at([0., 0., 1.]),
                Shot::at_limit(Caliber::Mm17),
                None,
            )
            .unwrap();
        let (contacts, tick) = run(&mut ballistics, &faces, 0, ms(200));
        assert_eq!(contacts.len(), 1, "{contacts:?}");
        let contact = &contacts[0];
        assert_eq!(contact.projectile, id);
        assert_eq!(contact.target, target);
        // 1.5 m at 25 m/s with drag takes a little over 60 ms; the contact
        // tick ends within one tick of that at any offered rate.
        let contact_ns = tick * tick_ns();
        assert!(
            (58_000_000..75_000_000 + tick_ns()).contains(&contact_ns),
            "{contact_ns} ns"
        );
        assert!(
            contact.normal_speed_m_s > 23. && contact.normal_speed_m_s < 25.,
            "{contact:?}"
        );
        assert!(contact.local_m[0].abs() < 0.004, "{:?}", contact.local_m);
        // Gravity drops the ball under two centimetres; y stays centred.
        assert!(contact.local_m[1].abs() < 0.002, "{:?}", contact.local_m);
        assert!(
            contact.local_m[2] < -0.01 && contact.local_m[2] > -0.025,
            "{:?}",
            contact.local_m
        );
        assert!(scoring_offset(target, contact.local_m).is_some());
        // The ball bounces away and is not reported twice.
        let (later, _) = run(&mut ballistics, &faces, ms(200), ms(100));
        assert!(later.is_empty());
        let snapshot = ballistics.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot[0].position_m[0] < 1.4, "{:?}", snapshot[0]);
    }
    #[test]
    fn back_and_side_strikes_are_contacts_but_not_scoring() {
        let target = ArmorTarget::Outpost {
            outpost: 0,
            face: 0,
        };
        // Plate faces away from the shooter: the ball hits the housing back.
        let faces = [face(target, 3., 0.)];
        let mut ballistics = WorldPhysics::new(&faces, 0.0);
        ballistics
            .fire(
                0,
                Pose::at([0., 0., 1.]),
                Shot::at_limit(Caliber::Mm17),
                None,
            )
            .unwrap();
        let (contacts, _) = run(&mut ballistics, &faces, 0, ms(300));
        assert_eq!(contacts.len(), 1, "{contacts:?}");
        assert!(contacts[0].local_m[0] < -0.015, "{contacts:?}");
        assert!(contacts[0].normal_speed_m_s < 0.);
        assert!(scoring_offset(target, contacts[0].local_m).is_none());
    }
    #[test]
    fn moving_plate_adds_its_own_closing_speed() {
        let target = ArmorTarget::Rune { rune: 0, blade: 0 };
        let start = face(target, 4., std::f64::consts::PI);
        let mut ballistics = WorldPhysics::new(&[start], 0.0);
        ballistics
            .fire(
                0,
                Pose::at([0., 0., 1.]),
                Shot::at_limit(Caliber::Mm17),
                None,
            )
            .unwrap();
        let mut contacts = Vec::new();
        let mut frames = TargetFrames::new(vec![start]);
        let tick_s = tick_ns() as f64 * 1e-9;
        for tick in 0..400_000_000 / tick_ns() {
            // The plate advances toward the shooter at 5 m/s.
            let at = |t: u64| {
                let mut f = start;
                f.pose.translation_m[0] -= 5. * t as f64 * tick_s;
                f
            };
            frames.begin(|faces| faces[0] = at(tick)).unwrap();
            frames.update(|faces| faces[0] = at(tick + 1)).unwrap();
            contacts.extend(ballistics.step(tick * tick_ns(), &frames).unwrap());
        }
        assert_eq!(contacts.len(), 1, "{contacts:?}");
        assert!(contacts[0].normal_speed_m_s > 26., "{contacts:?}");
    }
    #[test]
    fn slow_shots_fall_to_the_floor_and_are_discarded_after_their_flight_time() {
        let mut ballistics = WorldPhysics::new(&[], 0.0);
        ballistics
            .fire(
                0,
                Pose::at([0., 0., 1.]),
                Shot {
                    caliber: Caliber::Mm42,
                    speed_m_s: 3.,
                },
                None,
            )
            .unwrap();
        assert!(!ballistics.is_idle());
        run(&mut ballistics, &[], 0, ms(2_000));
        let snapshot = ballistics.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert!(snapshot[0].position_m[2] > 0.02 && snapshot[0].position_m[2] < 0.03);
        assert!(snapshot[0].position_m[0] > 0.5);
        run(&mut ballistics, &[], ms(2_000), ms(2_100));
        assert!(ballistics.is_idle());
        assert_eq!(ballistics.launched(), 1);
    }
    #[test]
    fn invalid_launches_and_meshes_are_rejected() {
        let mut ballistics = WorldPhysics::new(&[], 0.0);
        assert!(
            ballistics
                .fire(
                    0,
                    Pose::at([f64::NAN, 0., 0.]),
                    Shot::at_limit(Caliber::Mm17),
                    None,
                )
                .is_err()
        );
        assert!(
            ballistics
                .fire(
                    0,
                    Pose::default(),
                    Shot {
                        caliber: Caliber::Mm17,
                        speed_m_s: 0.
                    },
                    None,
                )
                .is_err()
        );
        assert!(
            ballistics
                .add_static_mesh(
                    vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                    vec![[0, 1, 3]]
                )
                .is_err()
        );
        assert!(ballistics.add_static_mesh(Vec::new(), Vec::new()).is_err());
        assert!(
            ballistics
                .add_static_mesh(
                    vec![[0., -1., 0.5], [3., -1., 0.5], [0., 1., 0.5], [3., 1., 0.5]],
                    vec![[0, 1, 2], [1, 3, 2]]
                )
                .is_ok()
        );
        // A ball dropped onto the mesh rests on it instead of the floor. Low
        // speed retirement would remove it once it settles, so this check runs
        // on the policy that keeps every ball to its flight limit.
        ballistics
            .set_projectile_policy(ProjectilePolicy::default().without_retirement())
            .unwrap();
        ballistics
            .fire(
                0,
                Pose::yawed([1.5, 0., 1.5], 0.),
                Shot {
                    caliber: Caliber::Mm17,
                    speed_m_s: 0.1,
                },
                None,
            )
            .unwrap();
        run(&mut ballistics, &[], 0, ms(1_500));
        let snapshot = ballistics.snapshot();
        assert!(snapshot[0].position_m[2] > 0.5, "{:?}", snapshot[0]);
    }
    #[test]
    fn projectile_cap_drops_the_oldest_shot() {
        let mut ballistics = WorldPhysics::new(&[], 0.0);
        for _ in 0..=MAX_PROJECTILES {
            ballistics
                .fire(
                    0,
                    Pose::at([0., 0., 1.]),
                    Shot::at_limit(Caliber::Mm17),
                    None,
                )
                .unwrap();
        }
        let snapshot = ballistics.snapshot();
        assert_eq!(snapshot.len(), MAX_PROJECTILES);
        assert_eq!(snapshot[0].id, 1);
    }
}
