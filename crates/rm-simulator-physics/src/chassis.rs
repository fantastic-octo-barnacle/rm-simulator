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

/// Horizontal acceleration a flying chassis may use to reach its commanded
/// velocity, in metres per second squared. Assumed, not a rulebook figure.
const FLIGHT_ACCELERATION_M_S2: f64 = 4.0;
/// Velocity-tracking gain of the flight controller, per second.
const FLIGHT_VELOCITY_GAIN_PER_S: f64 = 5.0;
/// Stiffness of a taut tether per kilogram of drone, in newtons per metre per
/// kilogram. A 30 rad/s pull-back keeps a full-speed overshoot to about 0.13 m.
/// Assumed; the rulebook specifies the tether's length, not its elasticity.
const TETHER_STIFFNESS_PER_KG: f64 = 900.0;
/// Near-critical damping of a taut tether per kilogram, in newtons per metre
/// per second per kilogram. It resists only motion that stretches the tether.
const TETHER_DAMPING_PER_KG: f64 = 60.0;
/// Cap on the taut tether's pull-back acceleration, in metres per second
/// squared, so a chassis placed far outside its reach returns smoothly.
const TETHER_MAX_PULL_M_S2: f64 = 20.0;

/// The Aerial Safety Rope a Drone flies on (rule manual V2.1.0 section 4.5,
/// Flight Zone). A retractable tether box rides a wire rope and cannot pass
/// the rope's Snap Ring; a soft tether of `length_m` runs from the box to the
/// drone's hook. The drone may therefore go anywhere within `length_m` of the
/// rope span from `rope_start_m` to `rope_end_m`, and nowhere else.
///
/// The hook is taken at the body centre. The tether's constant elastic force
/// (under 5 N static, section 4.5) is not modelled: the drone feels nothing
/// until the tether is taut. The box's inability to follow many turns in one
/// direction is not modelled either.
///
/// ```
/// use rm_simulator_physics::chassis::Tether;
///
/// let tether = Tether {
///     rope_start_m: [14.0, -5.8, 3.6],
///     rope_end_m: [0.0, -5.8, 3.6],
///     length_m: 2.4,
/// };
/// tether.validate()?;
/// // Directly below the rope and 1.6 m under it: 0.8 m of tether to spare.
/// assert!((tether.slack_m([7.0, -5.8, 2.0]) - 0.8).abs() < 1e-12);
/// // Past the Snap Ring the box stays at the rope's end.
/// assert_eq!(tether.anchor_m([-3.0, -5.8, 2.0]), [0.0, -5.8, 3.6]);
/// assert!(tether.slack_m([-3.0, -5.8, 2.0]) < 0.0);
/// # Ok::<(), &'static str>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tether {
    /// World FLU position of the near end of the span the tether box can
    /// travel, in metres: the end above the team's own Landing Pad.
    pub rope_start_m: [f64; 3],
    /// World FLU position of the Snap Ring, in metres; the box cannot pass it.
    pub rope_end_m: [f64; 3],
    /// Tether length from box to hook, in metres; 2.4 m in section 4.5.
    pub length_m: f64,
}
impl Tether {
    /// Where the tether box sits for a hook at `point_m`: the closest point of
    /// the rope span, in world FLU metres.
    pub fn anchor_m(&self, point_m: [f64; 3]) -> [f64; 3] {
        let start = vector(self.rope_start_m);
        let span = vector(self.rope_end_m) - start;
        let along = if span.length_squared() > 0.0 {
            ((vector(point_m) - start).dot(span) / span.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (start + span * along).to_array()
    }
    /// Tether left before it is taut with the hook at `point_m`, in metres:
    /// positive inside the reach, negative by how far the hook is beyond it.
    pub fn slack_m(&self, point_m: [f64; 3]) -> f64 {
        self.length_m - (vector(point_m) - vector(self.anchor_m(point_m))).length()
    }
    /// Check that every coordinate is finite and the length positive.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .rope_start_m
            .iter()
            .chain(&self.rope_end_m)
            .all(|v| v.is_finite())
            && self.length_m.is_finite()
            && self.length_m > 0.0
        {
            Ok(())
        } else {
            Err("tether rope must be finite and its length positive")
        }
    }
}

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
/// Presets cover omni Infantry/Sentry, mecanum Hero/Engineer and two-wheel
/// Balance Infantry. All use assumed values, not rulebook dimensions.
///
/// ```
/// use rm_simulator_physics::chassis::ChassisConfig;
///
/// let infantry = ChassisConfig::default();
/// infantry.validate()?;
/// assert_eq!(infantry.mass_kg, 22.0);
/// assert!(!infantry.mecanum);
/// // Hub drop, wheel radius and rest suspension add up to the body height.
/// assert!((infantry.rest_height_m() - 0.2065).abs() < 1e-9);
///
/// let hero = ChassisConfig::hero();
/// hero.validate()?;
/// assert_eq!(hero.mass_kg, 30.0);
/// assert!(hero.mecanum);
/// assert!(hero.wheel_radius_m > infantry.wheel_radius_m);
/// # Ok::<(), &'static str>(())
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChassisConfig {
    /// Actuator and suspension tuning.
    pub dynamics: ChassisDynamics,
    /// Drive layout: `true` is mecanum rollers, `false` ideal omni wheels.
    pub mecanum: bool,
    /// Simplified ground-contact pitch controller for the two-wheel prototype.
    /// Reduced-model LQR body assistance is not a full wheel-leg dynamics model.
    #[serde(default)]
    pub balance_assist: bool,
    /// Fixed-altitude horizontal flight. Gravity and vertical motion are disabled.
    #[serde(default)]
    pub planar_flight: bool,
    /// Aerial Safety Rope limiting where a flying chassis may go, set by the
    /// field layout when the drone is placed. `None` flies unrestrained.
    #[serde(default)]
    pub tether: Option<Tether>,
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
    /// two or three wheels. The default uses compliant, near-critically damped
    /// springs so the edge-centered wheels retain traction on ramp entries.
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
    /// A typical infantry: 22 kg (competition robots weigh about 22 to 32 kg),
    /// 0.52 m square body, 153 mm omni wheels at the
    /// four edge midpoints (245 mm from centre), prototype drives (about 4.6 N m at the
    /// wheel, 3.8 m/s free running). Assumed values, see the module notes.
    fn default() -> Self {
        Self {
            dynamics: ChassisDynamics {
                suspension_travel_m: 0.12,
                ..Default::default()
            },
            mecanum: false,
            balance_assist: false,
            planar_flight: false,
            tether: None,
            mass_kg: 22.0,
            body_half_m: [0.26, 0.26, 0.05],
            turret_center_m: [0.0, 0.0, 0.2],
            turret_half_m: [0.06, 0.06, 0.06],
            hub_drop_m: 0.05,
            wheel_hubs_m: vec![[0.245, 0.0], [0.0, 0.245], [-0.245, 0.0], [0.0, -0.245]],
            wheel_radius_m: 0.0765,
            wheel_width_m: 0.04,
            suspension_rest_m: 0.08,
            // Softer independent suspension keeps the driven side wheels loaded
            // while the front edge wheel climbs a ramp.
            suspension_stiffness_n_m: 1_000.0,
            // About 0.9 of critical for a quarter of 22 kg on 1 kN/m.
            suspension_damping_n_s_m: 135.0,
            // Only the two side wheels propel a straight run in this layout.
            wheel_stall_force_n: 60.0,
            wheel_no_load_speed_m_s: 3.8,
            slip_stiffness_n_s_m: 150.0,
            drive_friction: 1.0,
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
            mass_kg: 30.0,
            body_half_m: [0.33, 0.28, 0.06],
            turret_center_m: [0.0, 0.0, 0.25],
            turret_half_m: [0.09, 0.09, 0.075],
            wheel_hubs_m: vec![[0.22, 0.22], [-0.22, 0.22], [-0.22, -0.22], [0.22, -0.22]],
            wheel_radius_m: 0.1015,
            wheel_width_m: 0.065,
            wheel_stall_force_n: 60.0,
            drive_friction: 0.7,
            suspension_rest_m: 0.03,
            suspension_stiffness_n_m: 10_000.0,
            // About 0.9 of critical for a quarter of 30 kg on 10 kN/m.
            suspension_damping_n_s_m: 500.0,
            ..Self::default()
        }
    }

    /// Approximate two-wheel infantry with passive leg suspension. Dimensions and controller
    /// gains are prototype assumptions, not rulebook constants. It drives
    /// forward/backward and turns; it cannot strafe. Jump uses an equivalent
    /// leg push impulse; joint-level leg dynamics are not simulated.
    ///
    /// ```
    /// use rm_simulator_physics::chassis::ChassisConfig;
    /// let config = ChassisConfig::balance();
    /// config.validate()?;
    /// assert_eq!(config.wheel_hubs_m.len(), 2);
    /// assert!(config.balance_assist);
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn balance() -> Self {
        Self {
            balance_assist: true,
            dynamics: ChassisDynamics::default(),
            suspension_rest_m: 0.03,
            body_half_m: [0.18, 0.24, 0.09],
            hub_drop_m: 0.18,
            wheel_hubs_m: vec![[0.0, 0.24], [0.0, -0.24]],
            wheel_radius_m: 0.10,
            wheel_width_m: 0.045,
            suspension_stiffness_n_m: 20_000.0,
            suspension_damping_n_s_m: 840.0,
            roller_friction: 0.7,
            drive_friction: 0.7,
            wheel_stall_force_n: 80.0,
            ..Self::default()
        }
    }
    /// Approximate engineer platform carrying a fixed arm. The envelope,
    /// mass and drive tuning are design assumptions; arm dynamics are absent.
    ///
    /// ```
    /// use rm_simulator_physics::chassis::ChassisConfig;
    /// let config = ChassisConfig::engineer();
    /// config.validate()?;
    /// assert!(config.mecanum);
    /// # Ok::<(), &'static str>(())
    /// ```
    pub fn engineer() -> Self {
        Self {
            body_half_m: [0.36, 0.29, 0.08],
            wheel_hubs_m: vec![
                [0.24, 0.255],
                [-0.24, 0.255],
                [-0.24, -0.255],
                [0.24, -0.255],
            ],
            turret_center_m: [-0.12, 0.0, 0.36],
            turret_half_m: [0.13, 0.13, 0.1],
            ..Self::hero()
        }
    }

    /// Guarded quadcopter prototype constrained to the horizontal plane it is
    /// placed in; `rest_height_m` offers 1.6 m above the ground for a bare
    /// placement, and the field layout sets its own flight height and tether.
    /// It flies in its body frame and yaws on command, independent of the
    /// gimbal aim. No rotor aerodynamics or altitude controls.
    ///
    /// The underslung gimbal hangs 0.20 m ahead of the body centre, so a
    /// forward shot's muzzle clears the 0.8 m frame at every pitch the flight
    /// controls allow; a centred gimbal put upward shots into the frame.
    pub fn drone() -> Self {
        Self {
            planar_flight: true,
            mass_kg: 5.0,
            body_half_m: [0.40, 0.40, 0.065],
            turret_center_m: [0.20, 0.0, -0.14],
            turret_half_m: [0.035; 3],
            wheel_hubs_m: Vec::new(),
            ..Self::default()
        }
    }
    /// Height of the body centre above flat ground with the springs unloaded.
    pub fn rest_height_m(&self) -> f64 {
        if self.planar_flight {
            return 1.6;
        }
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
        if self.planar_flight && (self.balance_assist || !self.wheel_hubs_m.is_empty()) {
            return Err("planar flight cannot have wheels or balance assist");
        }
        if let Some(tether) = &self.tether {
            if !self.planar_flight {
                return Err("only a flying chassis has a tether");
            }
            tether.validate()?;
        }
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
        if self.turret_center_m[2] + self.turret_half_m[2] <= self.body_half_m[2]
            && !(self.planar_flight
                && self.turret_center_m[2] - self.turret_half_m[2] < -self.body_half_m[2])
        {
            return Err("turret top must be above the body top");
        }
        if self.wheel_hubs_m.is_empty() && !self.planar_flight {
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Held jump request. Balance chassis launch once per press while supported;
    /// other chassis ignore it. Release before requesting another jump.
    #[serde(default)]
    pub jump: bool,
    /// Stabilization strength in percent, 0 (unassisted) through 100 (full LQR).
    /// Ignored by other chassis. Carried in input and restorable snapshots.
    #[serde(default = "full_balance_control")]
    pub balance_control: u8,
}
fn full_balance_control() -> u8 {
    100
}
impl Default for ChassisCommand {
    fn default() -> Self {
        Self {
            forward_m_s: 0.,
            left_m_s: 0.,
            yaw_rate_rad_s: 0.,
            aim_yaw_rad: 0.,
            aim_pitch_rad: 0.,
            jump: false,
            balance_control: 100,
        }
    }
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
            && self.balance_control <= 100
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Previous physics slice jump state, retained to prevent replayed launches.
    #[serde(default)]
    pub jump_held: bool,
    /// Actual stabilized gimbal heading/elevation, preserved while defeated.
    pub held_aim_rad: [f64; 2],
    /// Actual motor rates, needed to continue acceleration-limited replay.
    pub gimbal_velocity_rad_s: [f64; 2],
    /// Wheel states in `ChassisConfig::wheel_hubs_m` order.
    pub wheels: Vec<WheelSnapshot>,
    /// The referee has this robot at zero HP: the drive is cut and the aim
    /// holds where it was.
    pub defeated: bool,
}

/// Body-frame wheel geometry in `ChassisConfig::wheel_hubs_m` order.
fn wheel_geometry(config: &ChassisConfig) -> Vec<WheelGeometry> {
    config
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
        .collect()
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
    jump_held: bool,
    balance_gains: [f64; 2],
    /// Actual motor heading and elevation; defeat freezes them.
    aim_rad: [f64; 2],
    gimbal_velocity_rad_s: [f64; 2],
    defeated: bool,
    /// Colliders the armor housings touched after the previous substep, so a
    /// resting contact registers one collision rather than one per substep.
    pub(crate) armor_touching: Vec<ColliderHandle>,
}

fn vector(v: [f64; 3]) -> Vector {
    Vector::new(v[0], v[1], v[2])
}
fn rapier_pose(p: Pose) -> Pose3 {
    let pose = exact_rapier_pose(p);
    Pose3::from_parts(pose.translation, pose.rotation.normalize())
}
/// The pose bit for bit. Rapier integrates rotations without renormalising,
/// so a live body sits an ulp or two off unit length; a restore must keep that
/// or its replay drifts from the host's.
fn exact_rapier_pose(p: Pose) -> Pose3 {
    let [w, x, y, z] = p.rotation_wxyz;
    Pose3::from_parts(vector(p.translation_m), Rotation::from_xyzw(x, y, z, w))
}
fn pose_is_valid(p: Pose) -> bool {
    let norm = p.rotation_wxyz.iter().map(|v| v * v).sum::<f64>();
    p.translation_m.iter().all(|v| v.is_finite()) && norm.is_finite() && (norm - 1.).abs() < 1e-6
}

impl ChassisSnapshot {
    /// Recompute the wheel values that follow from the rest of the snapshot:
    /// each hub centre from `pose` and the configured hub layout, and each
    /// tyre target from the inverse kinematics of `command` (zero while
    /// `defeated`, whose drive is cut). A codec that carries only wheel spin
    /// calls this after decoding. The hub matches the host within the pose's
    /// own precision. The target matches whenever the command has not changed
    /// since the last tick; a restored chassis recomputes both before using
    /// them, so neither affects stepping. Contacts are left alone. Wheels
    /// beyond the configured hubs are left unchanged.
    ///
    /// ```rust
    /// use rm_simulator_physics::chassis::{ChassisCommand, ChassisConfig, ChassisSnapshot};
    /// use rm_simulator_physics::{Pose, Team};
    ///
    /// let config = ChassisConfig::default();
    /// let wheels = config.wheel_hubs_m.len();
    /// let mut snapshot = ChassisSnapshot {
    ///     placement_revision: 0,
    ///     id: 0,
    ///     team: Team::Red,
    ///     config,
    ///     pose: Pose::at([1.0, 2.0, 0.3]),
    ///     turret: Pose::at([1.0, 2.0, 0.5]),
    ///     velocity_m_s: [0.0; 3],
    ///     angular_velocity_rad_s: [0.0; 3],
    ///     jump_held: false,
    ///     command: ChassisCommand { forward_m_s: 1.0, ..Default::default() },
    ///     held_aim_rad: [0.0; 2],
    ///     gimbal_velocity_rad_s: [0.0; 2],
    ///     wheels: vec![Default::default(); wheels],
    ///     defeated: false,
    /// };
    /// snapshot.derive_wheel_kinematics();
    /// let hub = snapshot.config.wheel_hubs_m[0];
    /// assert!((snapshot.wheels[0].hub_m[0] - (1.0 + hub[0])).abs() < 1e-12);
    /// assert_eq!(snapshot.wheels[0].target_m_s, 0.0); // Front wheel rolls sideways.
    /// assert_eq!(snapshot.wheels[1].target_m_s, -1.0); // Side wheel drives forward.
    /// ```
    pub fn derive_wheel_kinematics(&mut self) {
        let pose = rapier_pose(self.pose);
        let command = if self.defeated {
            ChassisCommand::default()
        } else {
            self.command
        };
        for (wheel, geometry) in self.wheels.iter_mut().zip(wheel_geometry(&self.config)) {
            wheel.hub_m = pose.transform_point(geometry.hub_m).to_array();
            wheel.target_m_s = command.forward_m_s * geometry.roll.x
                + command.left_m_s * geometry.roll.y
                + command.yaw_rate_rad_s * geometry.lever_m;
        }
    }
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
                .gravity_scale(if config.planar_flight { 0.0 } else { 1.0 })
                .enabled_translations(true, true, !config.planar_flight)
                .enabled_rotations(!config.planar_flight, !config.planar_flight, true)
                .pose(rapier_pose(spawn))
                .linear_damping(LINEAR_DAMPING)
                .angular_damping(ANGULAR_DAMPING)
                .can_sleep(false),
        );
        // The turret carries a share of the mass so the centre of mass sits
        // above the body centre, as on a real robot. A flying body keeps all
        // of it centred instead: its roll and pitch locks hold only while
        // the inertia stays axis-aligned, and its gimbal hangs off-centre.
        let turret_share = if config.planar_flight {
            0.0
        } else {
            TURRET_MASS_SHARE
        };
        let [hx, hy, hz] = config.body_half_m;
        world.insert_collider(
            ColliderBuilder::cuboid(hx, hy, hz)
                .mass(config.mass_kg * (1.0 - turret_share))
                .friction(BODY_FRICTION)
                .restitution(ROBOT_RESTITUTION)
                .restitution_combine_rule(CoefficientCombineRule::Min),
            Some(body),
        );
        let [tx, ty, tz] = config.turret_half_m;
        let [cx, cy, cz] = config.turret_center_m;
        let turret_top = cz + tz;
        let column_bottom = if config.planar_flight {
            (cz - tz).min(hz)
        } else {
            hz
        };
        let column_top = turret_top.max(hz);
        let column_half = (column_top - column_bottom) * 0.5;
        world.insert_collider(
            ColliderBuilder::cuboid(tx, ty, column_half)
                .translation(Vector::new(cx, cy, column_bottom + column_half))
                .mass(config.mass_kg * turret_share)
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
        let geometry = wheel_geometry(&config);
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
            balance_gains: balance_lqr_gains(&config),
            config,
            body,
            armor,
            forces: Vec::with_capacity(geometry.len() * 2),
            drive_forces: Vec::with_capacity(geometry.len()),
            geometry,
            wheels,
            jump_held: false,
            command: ChassisCommand {
                aim_yaw_rad: yaw_of(spawn),
                ..Default::default()
            },
            aim_rad: [yaw_of(spawn), 0.0],
            gimbal_velocity_rad_s: [0.0; 2],
            defeated: false,
            armor_touching: Vec::new(),
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
            self.jump_held = false;
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
        self.jump_held = false;
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
        world.bodies[self.body].set_position(exact_rapier_pose(state.pose), true);
        world
            .bodies
            .propagate_modified_body_positions_to_colliders(&mut world.colliders);
        self.placement_revision = state.placement_revision;
        self.set_command(state.command)?;
        self.defeated = state.defeated;
        self.jump_held = state.jump_held;
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
        if cfg.planar_flight {
            let mut wish = pose.rotation * Vector::new(command.forward_m_s, command.left_m_s, 0.0);
            wish.z = 0.0;
            let mut pull = Vector::ZERO;
            if let Some(tether) = &cfg.tether {
                let hook = pose.translation;
                let anchor = vector(tether.anchor_m(hook.to_array()));
                let outward = Vector::new(hook.x - anchor.x, hook.y - anchor.y, 0.0);
                if outward.length() > 1e-9 {
                    let outward = outward.normalize();
                    let slack = tether.slack_m(hook.to_array());
                    // Brake so the drone stops where the tether goes taut; once
                    // it is past, fly back in at up to 1 m/s.
                    let allowed = if slack > 0.0 {
                        (2.0 * FLIGHT_ACCELERATION_M_S2 * slack).sqrt()
                    } else {
                        -(FLIGHT_VELOCITY_GAIN_PER_S * -slack).min(1.0)
                    };
                    let out = wish.dot(outward);
                    if out > allowed {
                        wish -= outward * (out - allowed);
                    }
                    if slack < 0.0 {
                        let stretching = linvel.dot(outward).max(0.0);
                        pull = -outward
                            * (TETHER_STIFFNESS_PER_KG * -slack
                                + TETHER_DAMPING_PER_KG * stretching)
                                .min(TETHER_MAX_PULL_M_S2);
                    }
                }
            }
            let acceleration = ((wish - linvel) * FLIGHT_VELOCITY_GAIN_PER_S)
                .clamp_length_max(FLIGHT_ACCELERATION_M_S2)
                + pull;
            let body = &mut world.bodies[self.body];
            body.reset_forces(false);
            body.reset_torques(false);
            body.add_force(
                Vector::new(acceleration.x, acceleration.y, 0.0) * cfg.mass_kg,
                true,
            );
            body.add_torque(
                Vector::Z * ((command.yaw_rate_rad_s - angvel.z) * 3.0).clamp(-8.0, 8.0),
                true,
            );
            self.jump_held = self.command.jump;
            return;
        }
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
        let supported = self
            .wheels
            .iter()
            .filter(|wheel| wheel.contact.is_some())
            .count();
        if cfg.balance_assist && supported > 0 {
            // Reduced inverted-pendulum LQR, with a small speed-error lean
            // setpoint. The motor/leg plant remains an approximation; this is
            // an assisted body torque, not a firmware wheel-leg controller.
            let lateral = pose.rotation * Vector::Y;
            let forward = pose.rotation * Vector::X;
            let pitch_error = up.cross(Vector::Z).dot(lateral).clamp(-1.0, 1.0).asin();
            let rate = angvel.dot(lateral);
            let desired_lean =
                ((command.forward_m_s - linvel.dot(forward)) * 0.045).clamp(-0.10, 0.10);
            let strength = f64::from(command.balance_control) / 100.0;
            let [kp, kd] = self.balance_gains;
            let torque = (kp * (pitch_error + scale * desired_lean) - kd * rate)
                .clamp(-180.0, 180.0)
                * strength;
            body.add_torque(lateral * torque, true);
            // A 2.4 m/s equivalent leg push gives about 0.29 m ballistic rise.
            // Both wheels must support an upright body. Holding the button,
            // replaying a checkpoint or pressing again in flight cannot stack it.
            if command.jump
                && !self.jump_held
                && supported == cfg.wheel_hubs_m.len()
                && up.dot(Vector::Z) > 0.85
                && linvel.z < 0.5
                && !self.defeated
            {
                body.apply_impulse(Vector::Z * cfg.mass_kg * 2.4, true);
            }
        }
        self.jump_held = self.command.jump;
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
            jump_held: self.jump_held,
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
// Closed-form continuous LQR for x=[pitch, pitch_rate], A=[[0,1],[mgh/I,0]],
// B=[0,1/I], Q=diag(1000,60), R=0.01. These cost weights and the fitted
// centre-of-mass height are prototype choices. Solve once per chassis creation.
fn balance_lqr_gains(config: &ChassisConfig) -> [f64; 2] {
    let h = config.hub_drop_m + TURRET_MASS_SHARE * config.turret_center_m[2];
    let inertia = config.mass_kg
        * ((config.body_half_m[0].powi(2) + config.body_half_m[2].powi(2)) / 3.0 + h * h);
    let gravity = config.mass_kg * 9.81 * h;
    let kp = gravity + (gravity * gravity + 1000.0 / 0.01).sqrt();
    let kd = (2.0 * inertia * kp + 60.0 / 0.01).sqrt();
    [kp, kd]
}

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
        projectile::{Caliber, Shot, TargetFrames, WorldPhysics},
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
        let sag = config.mass_kg * 9.81 / 4.0 / config.suspension_stiffness_n_m;
        assert!((state.pose.translation_m[2] - (config.rest_height_m() - sag)).abs() < 0.002);
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
    fn drone_stays_in_its_plane_and_restores_its_drive() {
        let config = ChassisConfig::drone();
        config.validate().unwrap();
        let mut physics = chassis_world(config, Pose::at([0.0, 0.0, 1.6]));
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 1.0,
                    left_m_s: 0.5,
                    yaw_rate_rad_s: 0.2,
                    jump: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let state = run(&mut physics, 128);
        assert!(state.pose.translation_m[0] > 0.6);
        assert!(state.pose.translation_m[1] > 0.3);
        assert!((state.pose.translation_m[2] - 1.6).abs() < 1e-10);
        assert!(state.wheels.is_empty());
        assert!(state.pose.rotation_wxyz[1].abs() < 1e-10);
        assert!(state.pose.rotation_wxyz[2].abs() < 1e-10);
        let mut restored = WorldPhysics::new(&[], 0.0);
        restored.restore_chassis(&state).unwrap();
        let original = run(&mut physics, 128);
        let replayed = run(&mut restored, 128);
        for (a, b) in original
            .pose
            .translation_m
            .iter()
            .zip(replayed.pose.translation_m)
        {
            assert!((a - b).abs() < 1e-8);
        }
    }

    #[test]
    fn drone_shots_clear_its_own_frame_at_every_forward_pitch() {
        let config = ChassisConfig::drone();
        let mut physics = chassis_world(config, Pose::at([0.0, 0.0, 2.0]));
        let frames = TargetFrames::new(Vec::new());
        let mut time_ns = 0;
        // The flight controls allow -1.2 rad down to 0.79 rad up.
        for step in 0..=20 {
            let pitch = -1.2 + (0.79 + 1.2) * f64::from(step) / 20.0;
            physics
                .command_chassis(
                    0,
                    ChassisCommand {
                        aim_pitch_rad: pitch,
                        ..Default::default()
                    },
                )
                .unwrap();
            for _ in 0..128 {
                time_ns += tick_ns();
                physics.step(time_ns, &frames).unwrap();
            }
            let muzzle = physics.chassis_muzzle_pose(0).unwrap();
            let id = physics
                .fire(time_ns, muzzle, Shot::at_limit(Caliber::Mm17), Some(0))
                .unwrap();
            for _ in 0..8 {
                time_ns += tick_ns();
                physics.step(time_ns, &frames).unwrap();
            }
            let ball = physics.snapshot().into_iter().find(|b| b.id == id).unwrap();
            assert_eq!(
                ball.first_contact_ns, None,
                "pitch {pitch:.2} rad hit the drone"
            );
        }
    }

    #[test]
    fn tethered_drone_stops_at_its_reach_and_is_pulled_back_inside() {
        let tether = Tether {
            rope_start_m: [14.0, -5.8, 3.6],
            rope_end_m: [0.0, -5.8, 3.6],
            length_m: 2.4,
        };
        let config = ChassisConfig {
            tether: Some(tether),
            ..ChassisConfig::drone()
        };
        config.validate().unwrap();
        // Facing -x from above the pad, full speed towards the Snap Ring and
        // beyond it: the drone must brake to a stop within the tether's reach.
        let spawn = Pose::yawed([10.0, -5.8, 2.0], std::f64::consts::PI);
        let mut physics = chassis_world(config.clone(), spawn);
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 4.0,
                    aim_yaw_rad: std::f64::consts::PI,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut deepest = f64::INFINITY;
        let frames = TargetFrames::new(Vec::new());
        for tick in 0..128 * 8 {
            physics.step(tick * tick_ns(), &frames).unwrap();
            let hook = physics.chassis_snapshots()[0].pose.translation_m;
            deepest = deepest.min(tether.slack_m(hook));
        }
        let state = physics.chassis_snapshots().remove(0);
        // Beyond the ring by the tether's horizontal reach at 1.6 m below the rope.
        let reach = (2.4_f64.powi(2) - 1.6_f64.powi(2)).sqrt();
        assert!((state.pose.translation_m[0] + reach).abs() < 0.05);
        assert!(state.velocity_m_s[0].abs() < 0.05);
        assert!(deepest > -0.05, "overshot the tether by {deepest} m");

        // A drone placed well outside its reach is pulled back without the
        // pilot's help, and the pilot cannot hold it out.
        let mut outside = chassis_world(config, Pose::at([7.0, -1.0, 2.0]));
        outside
            .command_chassis(
                0,
                ChassisCommand {
                    left_m_s: 3.0,
                    ..Default::default()
                },
            )
            .unwrap();
        let state = run(&mut outside, 128 * 6);
        assert!(tether.slack_m(state.pose.translation_m) > -0.05);
        assert!((state.pose.translation_m[2] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn balance_stabilization_can_be_disabled_and_restored() {
        let pose = Pose {
            translation_m: [0.0, 0.0, 0.31],
            rotation_wxyz: [0.05_f64.cos(), 0.0, 0.05_f64.sin(), 0.0],
        };
        let mut assisted = chassis_world(ChassisConfig::balance(), pose);
        let mut unassisted = chassis_world(ChassisConfig::balance(), pose);
        unassisted
            .command_chassis(
                0,
                ChassisCommand {
                    balance_control: 0,
                    ..Default::default()
                },
            )
            .unwrap();
        let stable = run(&mut assisted, 128);
        let fallen = run(&mut unassisted, 128);
        assert!(stable.pose.rotation_wxyz[2].abs() < 0.025, "{stable:?}");
        assert!(fallen.pose.rotation_wxyz[2].abs() > 0.10, "{fallen:?}");
        assert!(
            unassisted
                .command_chassis(
                    0,
                    ChassisCommand {
                        balance_control: 101,
                        ..Default::default()
                    }
                )
                .is_err()
        );
        let mut restored = WorldPhysics::new(&[], 0.0);
        restored.restore_chassis(&fallen).unwrap();
        assert_eq!(restored.chassis_snapshots()[0].command.balance_control, 0);
    }

    #[test]
    fn balance_recovers_pitch_drives_and_jumps_once_per_press() {
        let config = ChassisConfig::balance();
        config.validate().unwrap();
        let mut spawn = Pose::at([0., 0., config.rest_height_m()]);
        let angle: f64 = 0.10;
        spawn.rotation_wxyz = [(angle / 2.).cos(), 0., (angle / 2.).sin(), 0.];
        let mut physics = chassis_world(config, spawn);
        let settled = run(&mut physics, 256);
        assert!(settled.pose.rotation_wxyz[2].abs() < 0.03, "{settled:?}");
        assert!(settled.pose.translation_m[2] > 0.20);
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    forward_m_s: 1.0,
                    ..Default::default()
                },
            )
            .unwrap();
        let driven = run(&mut physics, 128);
        assert!(
            driven.pose.translation_m[0] > settled.pose.translation_m[0] + 0.35,
            "{driven:?}"
        );
        physics
            .command_chassis(0, ChassisCommand::default())
            .unwrap();
        let grounded = run(&mut physics, 256);
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    jump: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let airborne = run(&mut physics, 22);
        assert!(
            airborne.pose.translation_m[2] > grounded.pose.translation_m[2] + 0.12,
            "{airborne:?}"
        );
        assert!(airborne.jump_held);
        let mut restored = WorldPhysics::new(&[], 0.0);
        restored.restore_chassis(&airborne).unwrap();
        let replayed = run(&mut restored, 256);
        assert!((replayed.pose.translation_m[2] - grounded.pose.translation_m[2]).abs() < 0.03);

        let landed = run(&mut physics, 256);
        assert!(
            (landed.pose.translation_m[2] - grounded.pose.translation_m[2]).abs() < 0.03,
            "held jump repeated: {landed:?}"
        );
        physics
            .command_chassis(0, ChassisCommand::default())
            .unwrap();
        run(&mut physics, 1);
        physics
            .command_chassis(
                0,
                ChassisCommand {
                    jump: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(
            run(&mut physics, 22).pose.translation_m[2] > grounded.pose.translation_m[2] + 0.12
        );
    }

    #[test]
    fn jump_cannot_launch_omni_or_defeated_balance() {
        for (config, defeated) in [
            (ChassisConfig::default(), false),
            (ChassisConfig::balance(), true),
        ] {
            let mut physics =
                chassis_world(config.clone(), Pose::at([0., 0., config.rest_height_m()]));
            let before = run(&mut physics, 128);
            physics.set_chassis_defeated(0, defeated);
            physics
                .command_chassis(
                    0,
                    ChassisCommand {
                        jump: true,
                        ..Default::default()
                    },
                )
                .unwrap();
            let after = run(&mut physics, 22);
            assert!(after.pose.translation_m[2] < before.pose.translation_m[2] + 0.02);
        }
    }

    #[test]
    fn default_configuration_is_valid_and_bad_ones_are_rejected() {
        let config = ChassisConfig::default();
        assert!(config.validate().is_ok());
        assert!((config.rest_height_m() - 0.2065).abs() < 1e-9);
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
        // Each spring carries a quarter of the weight.
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
        // The two side-midpoint wheels roll along the forward axis, so their
        // no-load speed directly caps straight-line body speed.
        let top = config.wheel_no_load_speed_m_s;
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
