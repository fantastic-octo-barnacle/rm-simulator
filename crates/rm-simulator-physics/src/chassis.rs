// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! A four-wheel omni chassis driven on ray-cast wheels inside the field's rapier
//! world. The body is one dynamic cuboid with a second cuboid for the turret
//! that reaches down to the body top (so low roofs stop the chassis and
//! nothing slips between the two). Each wheel is a ray from its hub along
//! the body's -z that carries a spring/damper suspension and an ideal omni tyre:
//! a motor-limited friction force along the rolling direction and a small roller
//! resistance along the axle, so the four wheels together move the body
//! holonomically. Ground is whatever the ray hits: the floor plane, the field
//! CAD (bumps, ramps, tunnels) or the tower proxies. Four small armor
//! modules ride the body sides as extra colliders, so projectiles strike
//! them where the body is at the moment of contact; a defeated robot's
//! drive is cut and its aim frozen.
//!
//! The rule manual constrains chassis power and envelope, not the mechanism;
//! the wheel, motor and suspension figures below are typical of a RoboMaster
//! infantry with 153 mm omni wheels and M3508 drives and are assumed, not
//! measured.
use crate::{
    Pose, Team,
    projectile::{FRICTION, SMALL_ARMOR_HOUSING_HALF_M},
};
use rapier3d_f64::prelude::*;
use serde::{Deserialize, Serialize};

/// Low robot contact rebound. Assumed, not a measured material property.
const ROBOT_RESTITUTION: f64 = 0.05;
/// Body collider friction against walls and the ground.
const BODY_FRICTION: f64 = 0.3;
/// Damping applied by rapier to the body velocities each step; keeps a
/// chassis that has lost wheel contact from spinning for ever.
const LINEAR_DAMPING: f64 = 0.05;
const ANGULAR_DAMPING: f64 = 0.5;
/// Surfaces steeper than this against the body's up axis are walls, not ground.
const MIN_GROUND_COSINE: f64 = 0.3;
/// Fraction of the mass carried by the turret collider (gimbal and gun).
const TURRET_MASS_SHARE: f64 = 0.15;
/// Armor modules per chassis: front, left, back and right, in that order.
pub const ARMOR_COUNT: usize = 4;
/// Gun muzzle offset from the stabilised turret pivot along its +x view line.
/// This 0.35 m barrel length is an application-level infantry geometry assumption.
pub const MUZZLE_FORWARD_M: f64 = 0.35;

/// Assumed actuator and suspension tuning, not rulebook limits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChassisDynamics {
    /// Gimbal speed-loop time constant in seconds: the commanded rate is the
    /// aim error divided by this.
    pub gimbal_response_s: f64,
    /// Cap on the gimbal rate, in radians per second.
    pub gimbal_max_speed_rad_s: f64,
    /// Cap on how fast the gimbal rate may change, in radians per second squared.
    pub gimbal_max_acceleration_rad_s2: f64,
    /// Shared drivetrain power budget in watts. The four drives scale down
    /// together once mechanical output plus copper loss exceeds it.
    pub drive_power_w: f64,
    /// Copper loss at stall force, per motor; braking does not return power.
    pub motor_stall_loss_w: f64,
    /// Maximum ordinary spring travel, followed by a progressive bump stop.
    pub suspension_travel_m: f64,
    /// Extra spring rate once compression passes `suspension_travel_m`, in
    /// newtons per metre.
    pub bump_stop_stiffness_n_m: f64,
}
impl Default for ChassisDynamics {
    fn default() -> Self {
        Self {
            gimbal_response_s: 0.025,
            gimbal_max_speed_rad_s: 12.0,
            gimbal_max_acceleration_rad_s2: 240.0,
            drive_power_w: 120.0,
            motor_stall_loss_w: 25.0,
            suspension_travel_m: 0.04,
            bump_stop_stiffness_n_m: 80_000.0,
        }
    }
}

/// Physical description of the chassis. All lengths are in the body frame:
/// origin at the body centre, +x forward, +y left, +z up.
///
/// Two presets ship: [`ChassisConfig::default`] is the omni Infantry and
/// [`ChassisConfig::hero`] the mecanum Hero. Both are assumed values, not
/// rulebook dimensions.
///
/// ```
/// use rm_simulator_physics::chassis::ChassisConfig;
///
/// let infantry = ChassisConfig::default();
/// infantry.validate()?;
/// assert_eq!(infantry.mass_kg, 15.0);
/// assert!(!infantry.mecanum);
/// // Hub drop, wheel radius and rest suspension add up to the body height.
/// assert!((infantry.rest_height_m() - 0.1565).abs() < 1e-9);
///
/// let hero = ChassisConfig::hero();
/// hero.validate()?;
/// assert_eq!(hero.mass_kg, 25.0);
/// assert!(hero.mecanum);
/// assert!(hero.wheel_radius_m > infantry.wheel_radius_m);
/// # Ok::<(), &'static str>(())
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChassisConfig {
    /// Actuator and suspension tuning.
    #[serde(default)]
    pub dynamics: ChassisDynamics,
    /// Drive layout; omitted in older snapshots means the original omni infantry.
    #[serde(default)]
    pub mecanum: bool,
    /// Total chassis mass in kilograms, armour modules excluded.
    pub mass_kg: f64,
    /// Body collider half extents; the body covers the wheels' footprint so
    /// walls stop the chassis before a wheel would enter them.
    pub body_half_m: [f64; 3],
    /// Turret centre in the body frame (the gun pivot) and half extents. The
    /// collider is a column of this footprint from the body top to the turret
    /// top, standing in for the gimbal neck.
    pub turret_center_m: [f64; 3],
    /// Half extents of that turret column, in metres.
    pub turret_half_m: [f64; 3],
    /// Wheel hubs sit this far below the body centre.
    pub hub_drop_m: f64,
    /// Wheel hub positions in the body's horizontal plane. Each wheel rolls
    /// along the counter-clockwise tangent about the body centre and its
    /// rollers run along the radial axle.
    pub wheel_hubs_m: Vec<[f64; 2]>,
    /// Wheel radius in metres; it sets both the suspension reach and the
    /// rolling speed the spin integrates.
    pub wheel_radius_m: f64,
    /// Axle width of one wheel, in metres.
    pub wheel_width_m: f64,
    /// Gap between the wheel bottom and the ground with the suspension fully
    /// extended. Compression beyond it is resisted by the same spring. The
    /// static sag (weight over stiffness) is the margin a wheel keeps before
    /// it lifts off on twisted ground, so a chassis that is too stiff rocks on
    /// three wheels; the default is near critically damped so bumps and ramps
    /// reach the body as a quick jolt rather than being soaked up.
    pub suspension_rest_m: f64,
    /// Spring rate of one wheel suspension, in newtons per metre.
    pub suspension_stiffness_n_m: f64,
    /// Damper rate of one wheel suspension, in newtons per metre per second.
    pub suspension_damping_n_s_m: f64,
    /// Tyre force one drive can produce at standstill.
    pub wheel_stall_force_n: f64,
    /// Wheel surface speed at which the drive torque reaches zero.
    pub wheel_no_load_speed_m_s: f64,
    /// Tyre force per unit of slip speed before the friction or motor limit.
    pub slip_stiffness_n_s_m: f64,
    /// Coulomb friction coefficient along the rolling direction.
    pub drive_friction: f64,
    /// Coulomb friction coefficient along the axle: the rollers' resistance.
    pub roller_friction: f64,
    /// One small armor module on each side of the body (front, left, back,
    /// right): its scoring face stands this far outside the body side, its
    /// centre this high above the body centre, and it leans back by this
    /// angle so the outward normal tilts up. Real plates lean about 15
    /// degrees; the figure is assumed, the manual gives no single value.
    pub armor_standoff_m: f64,
    /// Armour centre height above the body centre, in metres.
    pub armor_height_m: f64,
    /// Lean angle of the plate in radians; the outward normal tilts up by this.
    pub armor_tilt_rad: f64,
}
impl Default for ChassisConfig {
    /// A typical infantry: 15 kg, 0.52 m square body, 153 mm omni wheels on a
    /// 0.4 m square at 45 degrees, M3508-class drives (about 3 N m at the
    /// wheel, 3.8 m/s free running). Assumed values, see the module notes.
    fn default() -> Self {
        Self {
            dynamics: ChassisDynamics::default(),
            mecanum: false,
            mass_kg: 15.0,
            body_half_m: [0.26, 0.26, 0.05],
            turret_center_m: [0.0, 0.0, 0.2],
            turret_half_m: [0.06, 0.06, 0.06],
            hub_drop_m: 0.05,
            wheel_hubs_m: vec![[0.2, 0.2], [-0.2, 0.2], [-0.2, -0.2], [0.2, -0.2]],
            wheel_radius_m: 0.0765,
            wheel_width_m: 0.04,
            suspension_rest_m: 0.03,
            suspension_stiffness_n_m: 10_000.0,
            suspension_damping_n_s_m: 350.0,
            wheel_stall_force_n: 40.0,
            wheel_no_load_speed_m_s: 3.8,
            slip_stiffness_n_s_m: 150.0,
            drive_friction: 0.7,
            roller_friction: 0.03,
            armor_standoff_m: 0.02,
            armor_height_m: 0.015,
            armor_tilt_rad: 15_f64.to_radians(),
        }
    }
}
impl ChassisConfig {
    /// Assumed Hero prototype with 203 mm mecanum wheels. These are design
    /// choices, not rulebook dimensions; HP and weapon limits remain configured separately.
    pub fn hero() -> Self {
        Self {
            dynamics: ChassisDynamics {
                drive_power_w: 160.0,
                ..Default::default()
            },
            mecanum: true,
            mass_kg: 25.0,
            body_half_m: [0.33, 0.28, 0.06],
            turret_center_m: [0.0, 0.0, 0.25],
            turret_half_m: [0.09, 0.09, 0.075],
            wheel_hubs_m: vec![[0.22, 0.22], [-0.22, 0.22], [-0.22, -0.22], [0.22, -0.22]],
            wheel_radius_m: 0.1015,
            wheel_width_m: 0.065,
            wheel_stall_force_n: 60.0,
            suspension_damping_n_s_m: 450.0,
            ..Self::default()
        }
    }

    /// Height of the body centre above flat ground with the springs unloaded.
    pub fn rest_height_m(&self) -> f64 {
        self.hub_drop_m + self.wheel_radius_m + self.suspension_rest_m
    }
    /// Body-frame pose of each armor scoring face (+x the outward normal,
    /// +y along the width, +z along the height): front, left, back, right.
    ///
    /// ```
    /// use rm_simulator_physics::{chassis::ChassisConfig, motion::outpost::rotate};
    ///
    /// let faces = ChassisConfig::default().armor_faces();
    /// assert_eq!(faces.len(), 4);
    ///
    /// let normals: Vec<[f64; 3]> = faces
    ///     .iter()
    ///     .map(|face| rotate(face.rotation_wxyz, [1.0, 0.0, 0.0]))
    ///     .collect();
    /// // Front points along +x, left +y, back -x, right -y.
    /// assert!(normals[0][0] > 0.96);
    /// assert!(normals[1][1] > 0.96);
    /// assert!(normals[2][0] < -0.96);
    /// assert!(normals[3][1] < -0.96);
    /// // Every plate leans back 15 degrees, so each normal tilts up.
    /// assert!(normals.iter().all(|normal| normal[2] > 0.2));
    /// ```
    pub fn armor_faces(&self) -> [Pose; ARMOR_COUNT] {
        let [hx, hy, _] = self.body_half_m;
        std::array::from_fn(|plate| {
            // Exact axis directions, so a plate never picks up a rounding yaw.
            let (cos, sin) = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)][plate];
            let reach = if plate.is_multiple_of(2) { hx } else { hy } + self.armor_standoff_m;
            let yaw = plate as f64 * std::f64::consts::FRAC_PI_2;
            let rotation =
                Rotation::from_rotation_z(yaw) * Rotation::from_rotation_y(-self.armor_tilt_rad);
            Pose {
                translation_m: [reach * cos, reach * sin, self.armor_height_m],
                rotation_wxyz: [rotation.w, rotation.x, rotation.y, rotation.z],
            }
        })
    }
    /// Validate dimensions and physical parameters before constructing a chassis.
    pub fn validate(&self) -> Result<(), &'static str> {
        let positive = [
            self.dynamics.gimbal_response_s,
            self.dynamics.gimbal_max_speed_rad_s,
            self.dynamics.gimbal_max_acceleration_rad_s2,
            self.dynamics.drive_power_w,
            self.dynamics.motor_stall_loss_w,
            self.dynamics.suspension_travel_m,
            self.dynamics.bump_stop_stiffness_n_m,
            self.mass_kg,
            self.hub_drop_m,
            self.wheel_radius_m,
            self.wheel_width_m,
            self.suspension_rest_m,
            self.suspension_stiffness_n_m,
            self.suspension_damping_n_s_m,
            self.wheel_stall_force_n,
            self.wheel_no_load_speed_m_s,
            self.slip_stiffness_n_s_m,
            self.drive_friction,
        ];
        if positive.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("chassis quantities must be finite and positive");
        }
        if !self.roller_friction.is_finite() || self.roller_friction < 0.0 {
            return Err("roller friction must be finite and non-negative");
        }
        if self.body_half_m.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("body half extents must be finite and positive");
        }
        if self
            .turret_half_m
            .iter()
            .any(|v| !v.is_finite() || *v <= 0.0)
            || self.turret_center_m.iter().any(|v| !v.is_finite())
        {
            return Err("turret extents must be finite and positive");
        }
        if self.turret_center_m[2] + self.turret_half_m[2] <= self.body_half_m[2] {
            return Err("turret top must be above the body top");
        }
        if self.wheel_hubs_m.is_empty() {
            return Err("chassis needs at least one wheel");
        }
        if self
            .wheel_hubs_m
            .iter()
            .any(|[x, y]| !x.is_finite() || !y.is_finite() || x.hypot(*y) < 1e-3)
        {
            return Err("wheel hubs must be finite and off the body centre");
        }
        if !self.armor_standoff_m.is_finite()
            || self.armor_standoff_m < 0.0
            || !self.armor_height_m.is_finite()
            || !self.armor_tilt_rad.is_finite()
            || self.armor_tilt_rad.abs() >= std::f64::consts::FRAC_PI_2
        {
            return Err(
                "armor standoff must be finite and non-negative, its height finite and its tilt under a quarter turn",
            );
        }
        Ok(())
    }
}

/// Desired body-frame velocity and gun aim. The wheels are commanded from
/// the velocity by the omni inverse kinematics; the motors decide how much
/// of it they reach. The aim is where the stabilised gun pivot points in the
/// world; the gimbal is assumed to hold it exactly, so the turret pose in
/// the snapshot follows it at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChassisCommand {
    /// Desired body forward speed, in metres per second.
    pub forward_m_s: f64,
    /// Desired body left speed, in metres per second.
    pub left_m_s: f64,
    /// Counter-clockwise about the body's up axis.
    pub yaw_rate_rad_s: f64,
    /// Gun heading, counter-clockwise about world up from +x.
    #[serde(default)]
    pub aim_yaw_rad: f64,
    /// Gun elevation above the horizon.
    #[serde(default)]
    pub aim_pitch_rad: f64,
}
impl ChassisCommand {
    /// Validate numeric input before admitting it to a prediction history.
    pub fn is_finite(&self) -> bool {
        [
            self.forward_m_s,
            self.left_m_s,
            self.yaw_rate_rad_s,
            self.aim_yaw_rad,
            self.aim_pitch_rad,
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

/// Ground contact of one wheel at the end of a tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WheelContact {
    /// Contact point in world FLU metres.
    pub point_m: [f64; 3],
    /// Ground normal, turned to face the body's up axis.
    pub normal: [f64; 3],
    /// Suspension force pressing the wheel onto the ground.
    pub load_n: f64,
    /// Commanded minus actual surface speed along the rolling direction.
    pub slip_m_s: f64,
}
/// One wheel's state at the end of a tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WheelSnapshot {
    /// Wheel hub centre in world FLU metres.
    pub hub_m: [f64; 3],
    /// Accumulated wheel rotation about its axle, wrapped to one turn.
    pub spin_rad: f64,
    /// Surface speed the inverse kinematics asked of this wheel.
    pub target_m_s: f64,
    /// Ground contact this tick, or `None` when the ray found no ground.
    pub contact: Option<WheelContact>,
}
/// One chassis' observable state, complete enough to draw it or to rebuild a
/// prediction at a checkpoint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChassisSnapshot {
    /// Changes on placement or revival, so clients never interpolate across robot lives.
    pub placement_revision: u64,
    /// Field-assigned identity, stable for the chassis' life.
    pub id: u32,
    /// Team the chassis plays for.
    pub team: Team,
    /// Its dimensions, so a viewer can draw any chassis it sees.
    pub config: ChassisConfig,
    /// Body centre pose in world FLU.
    pub pose: Pose,
    /// Gun pivot pose: the configured turret centre carried by the body,
    /// facing the actual motor aim with +x along the barrel (world-stabilised,
    /// so the body's roll and pitch do not reach it).
    pub turret: Pose,
    /// Body linear velocity in metres per second.
    pub velocity_m_s: [f64; 3],
    /// Body angular velocity in radians per second.
    pub angular_velocity_rad_s: [f64; 3],
    /// Drive and aim the pilot last requested.
    pub command: ChassisCommand,
    /// Actual stabilized gimbal heading/elevation, preserved while defeated.
    pub held_aim_rad: [f64; 2],
    /// Actual motor rates, needed to continue acceleration-limited replay.
    #[serde(default)]
    pub gimbal_velocity_rad_s: [f64; 2],
    /// Wheel states in `ChassisConfig::wheel_hubs_m` order.
    pub wheels: Vec<WheelSnapshot>,
    /// The referee has this robot at zero HP: the drive is cut and the aim
    /// holds where it was.
    pub defeated: bool,
}

/// Fixed body-frame geometry of one wheel.
struct WheelGeometry {
    hub_m: Vector,
    /// Unit rolling direction in the body's horizontal plane.
    roll: Vector,
    /// Free roller direction in the contact plane. Radial for omni wheels.
    axle: Vector,
    /// Signed contact speed per unit yaw rate, from the hub cross rolling direction.
    lever_m: f64,
}
#[derive(Clone, Copy)]
struct WheelState {
    spin_rad: f64,
    target_m_s: f64,
    contact: Option<WheelContact>,
}

/// One chassis inside a physics world: its Rapier bodies and colliders, the
/// wheel geometry resolved from its configuration, and the drive and gimbal
/// state carried between ticks.
pub(crate) struct Chassis {
    placement_revision: u64,
    id: u32,
    team: Team,
    config: ChassisConfig,
    body: RigidBodyHandle,
    /// Armor housing colliders on the body, in `armor_faces` order.
    armor: Vec<ColliderHandle>,
    geometry: Vec<WheelGeometry>,
    wheels: Vec<WheelState>,
    /// Reused each tick; collect forces before mutably borrowing the body.
    forces: Vec<(Vector, Vector)>,
    drive_forces: Vec<(Vector, Vector)>,
    command: ChassisCommand,
    /// Actual motor heading and elevation; defeat freezes them.
    aim_rad: [f64; 2],
    gimbal_velocity_rad_s: [f64; 2],
    defeated: bool,
}

fn vector(v: [f64; 3]) -> Vector {
    Vector::new(v[0], v[1], v[2])
}
fn rapier_pose(p: Pose) -> Pose3 {
    let [w, x, y, z] = p.rotation_wxyz;
    Pose3::from_parts(
        vector(p.translation_m),
        Rotation::from_xyzw(x, y, z, w).normalize(),
    )
}
fn pose_is_valid(p: Pose) -> bool {
    let norm = p.rotation_wxyz.iter().map(|v| v * v).sum::<f64>();
    p.translation_m.iter().all(|v| v.is_finite()) && norm.is_finite() && (norm - 1.).abs() < 1e-6
}

impl Chassis {
    /// Insert the body into `world` at `spawn`. The wheels find the ground on
    /// the first step, so spawning a little above it is fine.
    pub(crate) fn new(
        world: &mut PhysicsWorld,
        id: u32,
        team: Team,
        config: ChassisConfig,
        spawn: Pose,
    ) -> Result<Self, &'static str> {
        config.validate()?;
        if !pose_is_valid(spawn) {
            return Err("chassis spawn pose must be finite with a unit rotation");
        }
        let body = world.insert_body(
            RigidBodyBuilder::dynamic()
                .pose(rapier_pose(spawn))
                .linear_damping(LINEAR_DAMPING)
                .angular_damping(ANGULAR_DAMPING)
                .can_sleep(false),
        );
        // The turret carries a share of the mass so the centre of mass sits
        // above the body centre, as on a real robot.
        let [hx, hy, hz] = config.body_half_m;
        world.insert_collider(
            ColliderBuilder::cuboid(hx, hy, hz)
                .mass(config.mass_kg * (1.0 - TURRET_MASS_SHARE))
                .friction(BODY_FRICTION)
                .restitution(ROBOT_RESTITUTION)
                .restitution_combine_rule(CoefficientCombineRule::Min),
            Some(body),
        );
        let [tx, ty, tz] = config.turret_half_m;
        let [cx, cy, cz] = config.turret_center_m;
        let turret_top = cz + tz;
        let column_half = (turret_top - hz) * 0.5;
        world.insert_collider(
            ColliderBuilder::cuboid(tx, ty, column_half)
                .translation(Vector::new(cx, cy, hz + column_half))
                .mass(config.mass_kg * TURRET_MASS_SHARE)
                .friction(BODY_FRICTION)
                .restitution(ROBOT_RESTITUTION)
                .restitution_combine_rule(CoefficientCombineRule::Min),
            Some(body),
        );
        // Armor housings ride the body without adding to its mass.
        let [ax, ay, az] = SMALL_ARMOR_HOUSING_HALF_M;
        let armor = config
            .armor_faces()
            .iter()
            .map(|face| {
                let local =
                    rapier_pose(*face) * Pose3::from_translation(Vector::new(-ax, 0.0, 0.0));
                world.insert_collider(
                    ColliderBuilder::cuboid(ax, ay, az)
                        .position(local)
                        .density(0.0)
                        .friction(FRICTION)
                        .restitution(ROBOT_RESTITUTION)
                        .restitution_combine_rule(CoefficientCombineRule::Min),
                    Some(body),
                )
            })
            .collect();
        let geometry = config
            .wheel_hubs_m
            .iter()
            .map(|&[x, y]| {
                let lever_m = x.hypot(y);
                // Mecanum contact force is perpendicular to the free roller,
                // at 45 degrees to the wheel plane, with mirrored handedness.
                let roll = if config.mecanum {
                    Vector::new(1.0, -(x * y).signum(), 0.0).normalize()
                } else {
                    Vector::new(-y / lever_m, x / lever_m, 0.0)
                };
                WheelGeometry {
                    hub_m: Vector::new(x, y, -config.hub_drop_m),
                    roll,
                    axle: Vector::new(roll.y, -roll.x, 0.0),
                    lever_m: x * roll.y - y * roll.x,
                }
            })
            .collect::<Vec<_>>();
        let wheels = vec![
            WheelState {
                spin_rad: 0.0,
                target_m_s: 0.0,
                contact: None,
            };
            geometry.len()
        ];
        Ok(Self {
            placement_revision: 0,
            id,
            team,
            config,
            body,
            armor,
            forces: Vec::with_capacity(geometry.len() * 2),
            drive_forces: Vec::with_capacity(geometry.len()),
            geometry,
            wheels,
            command: ChassisCommand {
                aim_yaw_rad: yaw_of(spawn),
                ..Default::default()
            },
            aim_rad: [yaw_of(spawn), 0.0],
            gimbal_velocity_rad_s: [0.0; 2],
            defeated: false,
        })
    }
    /// Field-assigned identity of this chassis.
    pub(crate) fn id(&self) -> u32 {
        self.id
    }
    /// Handle of the dynamic body that carries the chassis.
    pub(crate) fn body(&self) -> RigidBodyHandle {
        self.body
    }
    /// The four armor housing colliders, in `ChassisConfig::armor_faces` order.
    pub(crate) fn armor_colliders(&self) -> &[ColliderHandle] {
        &self.armor
    }
    /// World pose of armor scoring face `plate` right now.
    pub(crate) fn armor_pose(&self, world: &PhysicsWorld, plate: usize) -> Pose3 {
        *world.bodies[self.body].position() * rapier_pose(self.config.armor_faces()[plate])
    }
    /// Authoritative held turret pose, including the aim frozen by defeat.
    pub(crate) fn turret_pose(&self, world: &PhysicsWorld) -> Pose {
        let body = &world.bodies[self.body];
        Pose {
            translation_m: body
                .position()
                .transform_point(vector(self.config.turret_center_m))
                .to_array(),
            rotation_wxyz: aim_rotation(self.aim_rad[0], self.aim_rad[1]),
        }
    }
    /// World pose of the gun muzzle: the turret pose carried `MUZZLE_FORWARD_M`
    /// along the barrel's +x, which is the launch direction `fire` uses.
    pub(crate) fn muzzle_pose(&self, world: &PhysicsWorld) -> Pose {
        let turret = self.turret_pose(world);
        let transform = rapier_pose(turret);
        Pose {
            translation_m: transform
                .transform_point(Vector::new(MUZZLE_FORWARD_M, 0.0, 0.0))
                .to_array(),
            rotation_wxyz: turret.rotation_wxyz,
        }
    }
    /// Placement revision, bumped by `place` and by revival.
    pub(crate) fn revision(&self) -> u64 {
        self.placement_revision
    }
    /// Whether the referee has this robot at zero HP.
    pub(crate) fn is_defeated(&self) -> bool {
        self.defeated
    }
    /// Cut or restore the drive. A revived robot picks up its pilot's
    /// current aim again.
    pub(crate) fn set_defeated(&mut self, defeated: bool) {
        if self.defeated && !defeated {
            self.placement_revision = self
                .placement_revision
                .checked_add(1)
                .expect("chassis revision overflow");
        }
        self.defeated = defeated;
        if defeated {
            self.gimbal_velocity_rad_s = [0.0; 2];
        }
    }
    /// Team this chassis plays for.
    pub(crate) fn team(&self) -> Team {
        self.team
    }
    /// Configuration the chassis was built with.
    pub(crate) fn config(&self) -> &ChassisConfig {
        &self.config
    }
    /// Take the body and its colliders out of `world`.
    pub(crate) fn remove(self, world: &mut PhysicsWorld) {
        world.remove_body(self.body);
    }
    /// Reposition for local inspection, preserving identity and defeat state.
    pub(crate) fn place(
        &mut self,
        world: &mut PhysicsWorld,
        spawn: Pose,
    ) -> Result<(), &'static str> {
        if !pose_is_valid(spawn) {
            return Err("chassis spawn pose must be finite with a unit rotation");
        }
        self.placement_revision = self
            .placement_revision
            .checked_add(1)
            .ok_or("placement revision exhausted")?;
        let body = &mut world.bodies[self.body];
        body.set_position(rapier_pose(spawn), true);
        body.set_linvel(Vector::ZERO, true);
        body.set_angvel(Vector::ZERO, true);
        body.reset_forces(true);
        body.reset_torques(true);
        self.command = ChassisCommand {
            aim_yaw_rad: yaw_of(spawn),
            ..Default::default()
        };
        self.gimbal_velocity_rad_s = [0.0; 2];
        if !self.defeated {
            self.aim_rad = [yaw_of(spawn), 0.0];
        }
        for wheel in &mut self.wheels {
            wheel.contact = None;
            wheel.target_m_s = 0.0;
        }
        world
            .bodies
            .propagate_modified_body_positions_to_colliders(&mut world.colliders);
        Ok(())
    }
    /// Store a command already checked by the caller; rejects non-finite input.
    pub(crate) fn set_command(&mut self, command: ChassisCommand) -> Result<(), &'static str> {
        if !command.is_finite() {
            return Err("chassis command must be finite");
        }
        self.command = command;

        Ok(())
    }
    /// Restore observable rigid-body and wheel state for presentation replay.
    /// Contact solver caches are deliberately rebuilt, not copied from the host.
    pub(crate) fn restore_prediction(
        &mut self,
        world: &mut PhysicsWorld,
        state: &ChassisSnapshot,
    ) -> Result<(), &'static str> {
        if !state
            .velocity_m_s
            .into_iter()
            .chain(state.angular_velocity_rad_s)
            .all(f64::is_finite)
            || state
                .held_aim_rad
                .iter()
                .chain(&state.gimbal_velocity_rad_s)
                .any(|v| !v.is_finite())
            || state.wheels.iter().any(|w| !w.spin_rad.is_finite())
        {
            return Err("non-finite prediction state");
        }
        self.place(world, state.pose)?;
        self.placement_revision = state.placement_revision;
        self.set_command(state.command)?;
        self.defeated = state.defeated;
        self.aim_rad = state.held_aim_rad;
        self.gimbal_velocity_rad_s = state.gimbal_velocity_rad_s;
        let body = &mut world.bodies[self.body];
        body.set_linvel(Vector::from_array(state.velocity_m_s), true);
        body.set_angvel(Vector::from_array(state.angular_velocity_rad_s), true);
        for (wheel, saved) in self.wheels.iter_mut().zip(&state.wheels) {
            wheel.spin_rad = saved.spin_rad;
            // Both are outputs of the previous tick, recomputed before they
            // are used again; carrying them makes a restore snapshot equal.
            wheel.target_m_s = saved.target_m_s;
            wheel.contact = saved.contact;
        }
        Ok(())
    }
    /// Cast the wheel rays and load the body with this slice's suspension and
    /// tyre forces, integrating the gimbal and wheel spin by `dt_s`. Call once
    /// per solver slice before the world steps; forces are replaced, not
    /// accumulated, so repeated calls over one tick are safe.
    pub(crate) fn apply_forces(&mut self, world: &mut PhysicsWorld, dt_s: f64) {
        self.step_gimbal(dt_s);
        let cfg = &self.config;
        let (pose, linvel, angvel, com) = {
            let body = &world.bodies[self.body];
            (
                *body.position(),
                body.linvel(),
                body.angvel(),
                body.center_of_mass(),
            )
        };
        let up = pose.rotation * Vector::Z;
        // A defeated robot's drive is cut: the wheels brake to a stop.
        let command = if self.defeated {
            ChassisCommand::default()
        } else {
            self.command
        };
        let reach = cfg.wheel_radius_m + cfg.suspension_rest_m;
        let filter = QueryFilter::default().exclude_rigid_body(self.body);
        self.forces.clear();
        self.drive_forces.clear();
        let mut mechanical_w = 0.0;
        let mut copper_w = 0.0;
        for (wheel, geometry) in self.wheels.iter_mut().zip(&self.geometry) {
            let hub = pose.transform_point(geometry.hub_m);
            let axle = pose.rotation * geometry.axle;
            let target = command.forward_m_s * geometry.roll.x
                + command.left_m_s * geometry.roll.y
                + command.yaw_rate_rad_s * geometry.lever_m;
            wheel.target_m_s = target;
            let ray = Ray::new(hub, -up);
            let ground = world
                .cast_ray_and_get_normal(&ray, reach, true, filter)
                .and_then(|(_, hit)| {
                    let normal = if hit.normal.dot(up) < 0.0 {
                        -hit.normal
                    } else {
                        hit.normal
                    };
                    (normal.dot(up) > MIN_GROUND_COSINE && normal.is_finite())
                        .then_some((hit.time_of_impact, normal))
                });
            let Some((distance, normal)) = ground else {
                wheel.spin_rad = wrap_angle(
                    wheel.spin_rad
                        + target / cfg.wheel_radius_m
                            * dt_s
                            * if cfg.mecanum {
                                std::f64::consts::SQRT_2
                            } else {
                                1.0
                            },
                );
                wheel.contact = None;
                continue;
            };
            let compression = reach - distance;
            let hub_velocity = linvel + angvel.cross(hub - com);
            let compression_rate = -hub_velocity.dot(up);
            let overtravel = (compression - cfg.dynamics.suspension_travel_m).max(0.0);
            let bump_stop = cfg.dynamics.bump_stop_stiffness_n_m
                * overtravel
                * (1.0 + overtravel / cfg.dynamics.suspension_travel_m);
            let load = (bump_stop
                + cfg.suspension_stiffness_n_m * compression
                + cfg.suspension_damping_n_s_m * compression_rate)
                .max(0.0);
            let point = ray.point_at(distance);
            let contact_velocity = linvel + angvel.cross(point - com);
            let roll_dir = normal.cross(axle).normalize();
            let roller_dir = (axle - normal * axle.dot(normal)).normalize();
            if !roll_dir.is_finite() || !roller_dir.is_finite() {
                wheel.contact = None;
                continue;
            }
            let roll_speed = contact_velocity.dot(roll_dir);
            let roller_speed = contact_velocity.dot(roller_dir);
            let slip = target - roll_speed;
            // Torque falls linearly with speed when driving the wheel faster in
            // its direction of travel; braking and reversing get the stall force.
            let available = if slip * roll_speed > 0.0 {
                cfg.wheel_stall_force_n
                    * (1.0 - roll_speed.abs() / cfg.wheel_no_load_speed_m_s).clamp(0.0, 1.0)
            } else {
                cfg.wheel_stall_force_n
            };
            let grip = cfg.drive_friction * load;
            let drive = (cfg.slip_stiffness_n_s_m * slip)
                .clamp(-available, available)
                .clamp(-grip, grip);
            let roller_grip = cfg.roller_friction * load;
            let roller =
                (-cfg.slip_stiffness_n_s_m * roller_speed).clamp(-roller_grip, roller_grip);
            mechanical_w += (drive * roll_speed).max(0.0);
            copper_w += cfg.dynamics.motor_stall_loss_w * (drive / cfg.wheel_stall_force_n).powi(2);
            self.drive_forces.push((roll_dir * drive, point));
            self.forces.push((roller_dir * roller, point));
            self.forces.push((up * load, hub));
            wheel.spin_rad = wrap_angle(
                wheel.spin_rad
                    + roll_speed / cfg.wheel_radius_m
                        * dt_s
                        * if cfg.mecanum {
                            std::f64::consts::SQRT_2
                        } else {
                            1.0
                        },
            );
            wheel.contact = Some(WheelContact {
                point_m: point.to_array(),
                normal: normal.to_array(),
                load_n: load,
                slip_m_s: slip,
            });
        }
        let body = &mut world.bodies[self.body];
        // Forces and torques are separate accumulators; both must be cleared.
        body.reset_forces(false);
        body.reset_torques(false);
        let scale = drive_power_scale(mechanical_w, copper_w, cfg.dynamics.drive_power_w);
        for (force, point) in self.drive_forces.drain(..) {
            body.add_force_at_point(force * scale, point, true);
        }
        for (force, point) in self.forces.drain(..) {
            body.add_force_at_point(force, point, true);
        }
    }
    fn step_gimbal(&mut self, dt_s: f64) {
        if self.defeated {
            return;
        }
        let cfg = &self.config.dynamics;
        let targets = [
            wrap_angle(self.command.aim_yaw_rad),
            self.command.aim_pitch_rad.clamp(-1.4, 1.4),
        ];
        for (axis, target) in targets.into_iter().enumerate() {
            let error = if axis == 0 {
                wrap_angle(target - self.aim_rad[axis])
            } else {
                target - self.aim_rad[axis]
            };
            let speed = (error / cfg.gimbal_response_s)
                .clamp(-cfg.gimbal_max_speed_rad_s, cfg.gimbal_max_speed_rad_s);
            let rate = &mut self.gimbal_velocity_rad_s[axis];
            *rate += (speed - *rate).clamp(
                -cfg.gimbal_max_acceleration_rad_s2 * dt_s,
                cfg.gimbal_max_acceleration_rad_s2 * dt_s,
            );
            self.aim_rad[axis] += *rate * dt_s;
            if axis == 0 {
                self.aim_rad[axis] = wrap_angle(self.aim_rad[axis]);
            } else {
                self.aim_rad[axis] = self.aim_rad[axis].clamp(-1.4, 1.4);
            }
        }
    }
    /// Read the body, wheel and gimbal state into a serializable snapshot.
    pub(crate) fn snapshot(&self, world: &PhysicsWorld) -> ChassisSnapshot {
        let body = &world.bodies[self.body];
        let rotation = body.rotation();
        let pose = *body.position();
        let turret = self.turret_pose(world);
        ChassisSnapshot {
            placement_revision: self.placement_revision,
            id: self.id,
            team: self.team,
            config: self.config.clone(),
            pose: Pose {
                translation_m: body.translation().to_array(),
                rotation_wxyz: [rotation.w, rotation.x, rotation.y, rotation.z],
            },
            turret,
            velocity_m_s: body.linvel().to_array(),
            angular_velocity_rad_s: body.angvel().to_array(),
            command: self.command,
            held_aim_rad: self.aim_rad,
            gimbal_velocity_rad_s: self.gimbal_velocity_rad_s,
            wheels: self
                .wheels
                .iter()
                .zip(&self.geometry)
                .map(|(wheel, geometry)| WheelSnapshot {
                    hub_m: pose.transform_point(geometry.hub_m).to_array(),
                    spin_rad: wheel.spin_rad,
                    target_m_s: wheel.target_m_s,
                    contact: wheel.contact,
                })
                .collect(),
            defeated: self.defeated,
        }
    }
}

/// Gun rotation as wxyz: yaw about world up, then elevation, which is a
/// negative rotation about the gun's own left (+y) axis.
fn aim_rotation(yaw_rad: f64, pitch_rad: f64) -> [f64; 4] {
    let (sy, cy) = (yaw_rad / 2.0).sin_cos();
    let (sp, cp) = (-pitch_rad / 2.0).sin_cos();
    // (cy, 0, 0, sy) * (cp, 0, sp, 0)
    [cy * cp, -sy * sp, cy * sp, cp * sy]
}

fn wrap_angle(angle: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    (angle + PI).rem_euclid(TAU) - PI
}

/// Yaw of a pose about world up, counter-clockwise from +x.
pub fn yaw_of(pose: Pose) -> f64 {
    let [w, x, y, z] = pose.rotation_wxyz;
    (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z))
}

/// Uniform motor allocation: mechanical output scales linearly and copper loss quadratically.
fn drive_power_scale(mechanical_w: f64, copper_w: f64, budget_w: f64) -> f64 {
    if mechanical_w + copper_w <= budget_w {
        return 1.0;
    }
    if copper_w <= f64::EPSILON {
        return (budget_w / mechanical_w).min(1.0);
    }
    (2.0 * budget_w
        / (mechanical_w + (mechanical_w * mechanical_w + 4.0 * copper_w * budget_w).sqrt()))
    .min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        projectile::{TargetFrames, WorldPhysics},
        tick_ns,
    };
    fn chassis_world(config: ChassisConfig, spawn: Pose) -> WorldPhysics {
        let mut ballistics = WorldPhysics::new(&[], 0.0);
        ballistics.add_chassis(Team::Red, config, spawn).unwrap();
        ballistics
    }

    #[test]
    fn gimbal_motor_is_tick_driven_rate_limited_and_restorable() {
        let config = ChassisConfig::default();
        let mut physics = chassis_world(config.clone(), Pose::at([0., 0., config.rest_height_m()]));
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    aim_yaw_rad: 1.,
                    aim_pitch_rad: 0.4,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(physics.chassis_snapshots()[0].held_aim_rad, [0.; 2]);
        let frames = TargetFrames::new(Vec::new());
        let mut previous_rate = [0.; 2];
        let tick_s = tick_ns() as f64 * 1e-9;
        for tick in 0..200 {
            physics.step(tick * tick_ns(), &frames).unwrap();
            let motor = physics.chassis_snapshots()[0].gimbal_velocity_rad_s;
            for axis in 0..2 {
                assert!(motor[axis].abs() <= config.dynamics.gimbal_max_speed_rad_s + 1e-10);
                assert!(
                    (motor[axis] - previous_rate[axis]).abs()
                        <= config.dynamics.gimbal_max_acceleration_rad_s2 * tick_s + 1e-10
                );
            }
            previous_rate = motor;
        }
        let saved = physics.chassis_snapshots().remove(0);
        let mut restored = chassis_world(config, saved.pose);
        restored.reset_chassis(&saved).unwrap();
        for tick in 200..500 {
            physics.step(tick * tick_ns(), &frames).unwrap();
            restored.step(tick * tick_ns(), &frames).unwrap();
            assert_eq!(
                physics.chassis_snapshots()[0].held_aim_rad,
                restored.chassis_snapshots()[0].held_aim_rad
            );
            assert_eq!(
                physics.chassis_snapshots()[0].gimbal_velocity_rad_s,
                restored.chassis_snapshots()[0].gimbal_velocity_rad_s
            );
        }
        assert!((physics.chassis_snapshots()[0].held_aim_rad[0] - 1.).abs() < 0.01);
        physics.set_chassis_defeated(0, true);
        let frozen = physics.chassis_snapshots()[0].held_aim_rad;
        physics.step(500 * tick_ns(), &frames).unwrap();
        assert_eq!(physics.chassis_snapshots()[0].held_aim_rad, frozen);
        assert_eq!(
            physics.chassis_snapshots()[0].gimbal_velocity_rad_s,
            [0.; 2]
        );
    }

    #[test]
    fn gimbal_takes_short_yaw_path_across_wrap() {
        let config = ChassisConfig::default();
        let mut physics = chassis_world(
            config.clone(),
            Pose::yawed([0., 0., config.rest_height_m()], std::f64::consts::PI - 0.1),
        );
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    aim_yaw_rad: -std::f64::consts::PI + 0.1,
                    ..Default::default()
                },
            )
            .unwrap();
        let state = run(&mut physics, 1);
        assert!(state.gimbal_velocity_rad_s[0] > 0.);
        let state = run(&mut physics, 1000);
        assert!(wrap_angle(state.held_aim_rad[0] - (-std::f64::consts::PI + 0.1)).abs() < 1e-6);
    }

    #[test]
    fn landing_settles_without_gaining_bounce_height() {
        let config = ChassisConfig::default();
        let mut physics = chassis_world(config.clone(), Pose::at([0., 0., 0.6]));
        let frames = TargetFrames::new(Vec::new());
        let mut highest = 0.6_f64;
        for tick in 0..3000 {
            physics.step(tick * tick_ns(), &frames).unwrap();
            highest = highest.max(physics.chassis_snapshots()[0].pose.translation_m[2]);
        }
        let state = physics.chassis_snapshots().remove(0);
        assert!(highest <= 0.601, "landing gained height: {highest}");
        assert!((state.pose.translation_m[2] - config.rest_height_m()).abs() < 0.02);
        assert!(state.velocity_m_s.iter().all(|v| v.abs() < 0.01));
        assert!(state.wheels.iter().all(|w| w.contact.is_some()));
    }

    #[test]
    fn shared_power_budget_charges_stall_losses_and_has_no_regeneration_credit() {
        for mechanical in [0., 40., 200.] {
            for copper in [0., 60., 400.] {
                let scale = drive_power_scale(mechanical, copper, 80.);
                assert!((0.0..=1.0).contains(&scale));
                assert!(scale * mechanical + scale * scale * copper <= 80. + 1e-10);
            }
        }
        assert!(drive_power_scale(0., 100., 80.) < 1.);
    }

    #[test]
    fn muzzle_uses_rotated_body_pivot_and_freezes_held_aim_when_defeated() {
        let config = ChassisConfig::default();
        let spawn = Pose::yawed(
            [2.0, 3.0, config.rest_height_m()],
            std::f64::consts::FRAC_PI_2,
        );
        let mut physics = chassis_world(config.clone(), spawn);
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    aim_yaw_rad: std::f64::consts::FRAC_PI_2,
                    aim_pitch_rad: 0.0,
                    ..Default::default()
                },
            )
            .unwrap();
        let aimed = physics.chassis_muzzle_pose(0).unwrap();
        let expected_pivot = rapier_pose(spawn).transform_point(vector(config.turret_center_m));
        assert!((aimed.translation_m[0] - expected_pivot.x).abs() < 1e-12);
        assert!((aimed.translation_m[1] - (expected_pivot.y + MUZZLE_FORWARD_M)).abs() < 1e-12);
        assert!((aimed.translation_m[2] - expected_pivot.z).abs() < 1e-12);

        physics.set_chassis_defeated(0, true);
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    aim_yaw_rad: 0.0,
                    aim_pitch_rad: 0.4,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(physics.chassis_muzzle_pose(0), Some(aimed));
        assert_eq!(physics.chassis_muzzle_pose(99), None);
    }
    fn run(ballistics: &mut WorldPhysics, ticks: u64) -> ChassisSnapshot {
        let frames = TargetFrames::new(Vec::new());
        for tick in 0..ticks {
            ballistics.step(tick * tick_ns(), &frames).unwrap();
        }
        ballistics.chassis_snapshots().remove(0)
    }
    #[test]
    fn default_configuration_is_valid_and_bad_ones_are_rejected() {
        let config = ChassisConfig::default();
        assert!(config.validate().is_ok());
        assert!((config.rest_height_m() - 0.1565).abs() < 1e-9);
        assert!(
            ChassisConfig {
                mass_kg: 0.0,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ChassisConfig {
                wheel_hubs_m: vec![[0.0, 0.0]],
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            ChassisConfig {
                roller_friction: -0.1,
                ..config.clone()
            }
            .validate()
            .is_err()
        );
        let mut world = PhysicsWorld::new();
        assert!(
            Chassis::new(
                &mut world,
                0,
                Team::Red,
                config,
                Pose {
                    rotation_wxyz: [0.0; 4],
                    ..Pose::default()
                }
            )
            .is_err()
        );
        assert!((yaw_of(Pose::yawed([0.0; 3], 0.7)) - 0.7).abs() < 1e-12);
        assert!((wrap_angle(7.0) - (7.0 - std::f64::consts::TAU)).abs() < 1e-12);
    }
    #[test]
    fn chassis_settles_on_the_floor_with_all_wheels_loaded() {
        let config = ChassisConfig::default();
        let mut ballistics = chassis_world(
            config.clone(),
            Pose::at([0.0, 0.0, config.rest_height_m() + 0.05]),
        );
        let snapshot = run(&mut ballistics, 1_500);
        let z = snapshot.pose.translation_m[2];
        // Each spring carries a quarter of the weight: about 4.6 mm of sag.
        let sag = config.mass_kg * 9.81 / 4.0 / config.suspension_stiffness_n_m;
        assert!((z - (config.rest_height_m() - sag)).abs() < 0.001, "{z}");
        assert!(snapshot.velocity_m_s.iter().all(|v| v.abs() < 1e-3));
        assert_eq!(snapshot.wheels.len(), 4);
        for wheel in &snapshot.wheels {
            let contact = wheel.contact.expect("wheel on the ground");
            assert!((contact.load_n - config.mass_kg * 9.81 / 4.0).abs() < 1.0);
            assert!(contact.point_m[2].abs() < 1e-6);
            assert!((contact.normal[2] - 1.0).abs() < 1e-9);
        }
        assert!((yaw_of(snapshot.pose)).abs() < 1e-6);
    }
    #[test]
    fn forward_left_and_yaw_commands_move_the_body_holonomically() {
        for config in [ChassisConfig::default(), ChassisConfig::hero()] {
            let spawn = Pose::at([0.0, 0.0, config.rest_height_m()]);
            for (command, check) in [
                (
                    ChassisCommand {
                        forward_m_s: 1.0,
                        ..Default::default()
                    },
                    (|s: &ChassisSnapshot| {
                        s.pose.translation_m[0] > 1.5
                            && s.pose.translation_m[1].abs() < 0.05
                            && (s.velocity_m_s[0] - 1.0).abs() < 0.05
                            && yaw_of(s.pose).abs() < 0.05
                    }) as fn(&ChassisSnapshot) -> bool,
                ),
                (
                    ChassisCommand {
                        left_m_s: 1.0,
                        ..Default::default()
                    },
                    |s| {
                        s.pose.translation_m[1] > 1.5
                            && s.pose.translation_m[0].abs() < 0.05
                            && yaw_of(s.pose).abs() < 0.05
                    },
                ),
                (
                    ChassisCommand {
                        yaw_rate_rad_s: 1.0,
                        ..Default::default()
                    },
                    |s| {
                        s.pose.translation_m[0].abs() < 0.05
                            && s.pose.translation_m[1].abs() < 0.05
                            && (s.angular_velocity_rad_s[2] - 1.0).abs() < 0.05
                    },
                ),
            ] {
                let mut ballistics = chassis_world(config.clone(), spawn);
                run(&mut ballistics, 300);
                ballistics.command_chassis(0, command).unwrap();
                let snapshot = run(&mut ballistics, 2_000);
                assert!(check(&snapshot), "{command:?}: {snapshot:?}");
                assert_eq!(snapshot.command, command);
                // Wheels spin with the ground speed they see.
                assert!(snapshot.wheels.iter().any(|w| w.spin_rad != 0.0));
            }
        }
    }
    #[test]
    fn the_turret_rides_the_body_and_faces_the_commanded_aim() {
        let config = ChassisConfig::default();
        let mut ballistics =
            chassis_world(config.clone(), Pose::at([0.0, 0.0, config.rest_height_m()]));
        let rest = run(&mut ballistics, 300);
        let [x, y, z] = rest.turret.translation_m;
        assert!(x.abs() < 1e-6 && y.abs() < 1e-6, "{x} {y}");
        assert!((z - rest.pose.translation_m[2] - config.turret_center_m[2]).abs() < 1e-9);
        assert_eq!(rest.turret.rotation_wxyz, [1.0, 0.0, 0.0, 0.0]);
        // Aim left and up: the barrel (+x) points along +y, raised.
        ballistics
            .command_chassis(
                0,
                ChassisCommand {
                    aim_yaw_rad: std::f64::consts::FRAC_PI_2,
                    aim_pitch_rad: 0.5,
                    ..Default::default()
                },
            )
            .unwrap();
        let aimed = run(&mut ballistics, 2_000);
        let barrel = rapier_pose(aimed.turret).rotation * Vector::X;
        assert!(barrel.x.abs() < 1e-9, "{barrel}");
        assert!((barrel.y - 0.5f64.cos()).abs() < 1e-9, "{barrel}");
        assert!((barrel.z - 0.5f64.sin()).abs() < 1e-9, "{barrel}");
        assert!(
            ballistics
                .command_chassis(
                    0,
                    ChassisCommand {
                        aim_pitch_rad: f64::INFINITY,
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
    #[test]
    fn motor_limit_caps_the_top_speed() {
        let config = ChassisConfig::default();
        let mut ballistics =
            chassis_world(config.clone(), Pose::at([0.0, 0.0, config.rest_height_m()]));
        run(&mut ballistics, 300);
        ballistics
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 10.0,
                    ..Default::default()
                },
            )
            .unwrap();
        let snapshot = run(&mut ballistics, 4_000);
        let speed = snapshot.velocity_m_s[0];
        // Wheels at 45 degrees see 1/sqrt(2) of the body speed, so straight-line
        // running tops out at sqrt(2) times the wheel's no-load speed.
        let top = config.wheel_no_load_speed_m_s * std::f64::consts::SQRT_2;
        assert!(speed > 0.8 * top && speed < top, "{speed}");
        assert!(
            ballistics
                .command_chassis(
                    0,
                    ChassisCommand {
                        forward_m_s: f64::NAN,
                        ..Default::default()
                    }
                )
                .is_err()
        );
    }
}
