// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Prescribed motion and fitted armor geometry. Callers decide activation and stops.
//! Rune pose helpers and fitted outpost geometry originated in rm-vision-sim
//! and were adapted in rm-simulator-world. See NOTICE.md for provenance.
use crate::{Pose, Team};
use serde::{Deserialize, Serialize};

/// One prismatic scenery part whose position the caller resolves each tick.
/// The server binds these to the CAD joints `base.shield.*.slide`,
/// `dart-station.gate.slide` and `base.dart_target.slide`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    /// Base shield plates, one per team.
    Base,
    /// Dart station gates, one per team.
    DartDoor,
    /// The dart rail target, shared by both teams.
    DartTarget,
}

/// Resolved positions supplied by the caller; physics never consults a referee.
///
/// ```
/// use rm_simulator_physics::{
///     motion::{Mechanism, MechanismState},
///     Team,
/// };
///
/// let state = MechanismState {
///     base_open: [true, false],
///     dart_door_open: [true, true],
///     dart_target_fraction: 0.25,
/// };
/// assert_eq!(state.fraction(Mechanism::Base, Team::Red), 1.0);
/// assert_eq!(state.fraction(Mechanism::Base, Team::Blue), 0.0);
/// assert_eq!(state.fraction(Mechanism::DartTarget, Team::Blue), 0.25);
///
/// // Defaults: base plates shut, dart gates open, target at its near stop.
/// let default = MechanismState::default();
/// assert_eq!(default.base_open, [false; 2]);
/// assert_eq!(default.dart_door_open, [true; 2]);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MechanismState {
    /// Base shield plates, red then blue: `true` is open, which is the far end
    /// of the captured travel.
    pub base_open: [bool; 2],
    /// Dart station gates, red then blue: `true` is open.
    pub dart_door_open: [bool; 2],
    /// Dart rail target between its stops: 0 at the near stop, 1 at the far one.
    pub dart_target_fraction: f64,
}
impl MechanismState {
    /// Interpolation fraction for one mechanism: a boolean state becomes 1.0
    /// when open and 0.0 when shut, and the dart target returns its own
    /// fraction unchanged.
    pub fn fraction(&self, mechanism: Mechanism, team: Team) -> f64 {
        match mechanism {
            Mechanism::Base => f64::from(self.base_open[team.index()]),
            Mechanism::DartDoor => f64::from(self.dart_door_open[team.index()]),
            Mechanism::DartTarget => self.dart_target_fraction,
        }
    }
}
impl Default for MechanismState {
    fn default() -> Self {
        Self {
            base_open: [false; 2],
            dart_door_open: [true; 2],
            dart_target_fraction: 0.0,
        }
    }
}
/// Four-second inspection sweep matching rm-map-tools' illustrative preview.
/// An application motion setting, not a rulebook target-mode speed.
pub const DART_TARGET_PERIOD_NS: u64 = 4_000_000_000;
/// Dart rail position at `time_ns`: a cosine sweep from 0 to 1 and back over
/// [`DART_TARGET_PERIOD_NS`]. Repeats forever and ignores the team.
///
/// ```
/// use rm_simulator_physics::motion::{dart_target_fraction, DART_TARGET_PERIOD_NS};
///
/// assert_eq!(dart_target_fraction(0), 0.0);
/// let far = dart_target_fraction(DART_TARGET_PERIOD_NS / 2);
/// assert!((far - 1.0).abs() < 1e-12);
/// assert_eq!(dart_target_fraction(DART_TARGET_PERIOD_NS), 0.0);
/// ```
pub fn dart_target_fraction(time_ns: u64) -> f64 {
    let phase = (time_ns % DART_TARGET_PERIOD_NS) as f64 / DART_TARGET_PERIOD_NS as f64;
    0.5 - 0.5 * (phase * std::f64::consts::TAU).cos()
}

/// Analytic rotor motion; stopping and resuming are caller decisions.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RotorMotion {
    /// Rotor pivot relative to the tower base, in the CAD frame where x is
    /// forward, y up and z right.
    pub pivot_cad_m: [f64; 3],
    /// Tower base pose: the base on the floor, rotated so the tower's local
    /// forward matches the CAD +x.
    pub origin: Pose,
    /// Constant rotation rate about world up, in radians per second.
    pub speed_rad_s: f64,
    /// Keep the existing serialized checkpoint field name.
    #[serde(rename = "destroyed_ns")]
    pub stopped_at_ns: Option<u64>,
}
impl RotorMotion {
    /// Rotor angle at `time_ns`, wrapped to one turn. Holds the angle reached
    /// at `stopped_at_ns` instead of advancing past it.
    pub fn angle_at(&self, time_ns: u64) -> f64 {
        let time_ns = self.stopped_at_ns.map_or(time_ns, |stop| time_ns.min(stop));
        (time_ns as f64 * 1e-9 * self.speed_rad_s).rem_euclid(std::f64::consts::TAU)
    }
    /// The three armor scoring face poses at `time_ns`, in face order.
    pub fn armor_poses(&self, time_ns: u64) -> [Pose; 3] {
        outpost::armor_poses_at(self.origin, self.pivot_cad_m, self.angle_at(time_ns))
    }
}

/// Analytic sinusoidal speed profile, independent of activation policy.
/// Section 5.5.2 defines the Big Rune speed as `a*sin(w*t) + 2.090 - a` rad/s.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SinusoidalMotion {
    /// Swing amplitude `a` about the 2.090 rad/s peak, in radians per second.
    /// The rule range is 0.780 to 1.045.
    pub amplitude_rad_s: f64,
    /// Angular frequency `w` of the swing, in radians per second. The rule
    /// range is 1.884 to 2.000.
    pub frequency_rad_s: f64,
}
impl SinusoidalMotion {
    /// Angular speed at `elapsed_s` since activation. Starts at
    /// `2.090 - amplitude_rad_s` and peaks at 2.090 rad/s.
    ///
    /// ```
    /// use rm_simulator_physics::motion::SinusoidalMotion;
    ///
    /// let motion = SinusoidalMotion { amplitude_rad_s: 0.9, frequency_rad_s: 1.94 };
    /// let start = motion.speed_rad_s(0.0);
    /// assert!((start - (2.090 - 0.9)).abs() < 1e-12);
    /// // The peak sits a quarter of a swing period in.
    /// let peak = motion.speed_rad_s(std::f64::consts::FRAC_PI_2 / motion.frequency_rad_s);
    /// assert!((peak - 2.090).abs() < 1e-12);
    /// ```
    pub fn speed_rad_s(self, elapsed_s: f64) -> f64 {
        self.amplitude_rad_s * (self.frequency_rad_s * elapsed_s).sin() + 2.090
            - self.amplitude_rad_s
    }
    /// Angle turned since `elapsed_s = 0`, the integral of
    /// [`Self::speed_rad_s`].
    pub fn displacement_rad(self, elapsed_s: f64) -> f64 {
        self.amplitude_rad_s / self.frequency_rad_s
            * (1.0 - (self.frequency_rad_s * elapsed_s).cos())
            + (2.090 - self.amplitude_rad_s) * elapsed_s
    }
}

/// Fitted outpost geometry and its analytic rotor motion.
///
/// ```
/// use rm_simulator_physics::{motion::outpost, Pose};
///
/// let origin = Pose::at([4.0, 3.0, 0.0]);
/// let rest = outpost::armor_poses_at(origin, outpost::PIVOT_CAD_M, 0.0);
/// let quarter = outpost::armor_poses_at(
///     origin,
///     outpost::PIVOT_CAD_M,
///     std::f64::consts::FRAC_PI_2,
/// );
///
/// // Section 5.5.1: the middle armor turns at 0.8 pi rad/s.
/// assert!((outpost::DEFAULT_SPEED_RAD_S - 0.8 * std::f64::consts::PI).abs() < 1e-12);
/// // A quarter turn carries all three faces to new centres.
/// for (turned, parked) in quarter.iter().zip(&rest) {
///     assert_ne!(turned.translation_m, parked.translation_m);
/// }
/// ```
pub mod outpost {
    use super::*;
    /// Section 5.5.1: the middle armor reaches 0.8π rad/s and holds it.
    pub const DEFAULT_SPEED_RAD_S: f64 = 0.8 * std::f64::consts::PI;
    /// Rotor pivot relative to the tower base. CAD frame: x forward, y up, z right.
    pub const PIVOT_CAD_M: [f64; 3] = [-0.0094378835, 1.14, -0.0010455168];
    // Front housing centers fitted in the extracted rotor's local frame.
    /// The three armor module centres, fitted in the extracted rotor's local
    /// frame (x forward, y up, z right) and read off the CAD assembly.
    pub const FRONT_CAD_M: [[f64; 3]; 3] = [
        [-0.27643089, -0.13731677, 0.0],
        [0.13823941, 0.06881135, 0.23943769],
        [0.13851113, -0.03343800, -0.23990831],
    ];
    /// Light bar band of the armor module (the 135 x 55 mm optical target).
    pub const TARGET_WIDTH_M: f64 = 0.135;
    /// Height of that light bar band, in metres (55 mm).
    pub const TARGET_HEIGHT_M: f64 = 0.055;
    /// Figure 5-16 effective detection area: 111 mm span inset 5 mm each side by
    /// 16 + 2 + 58 + 2 + 16 mm, centred on the face.
    pub const DETECTION_WIDTH_M: f64 = 0.101;
    /// Height of that detection rectangle, in metres (94 mm).
    pub const DETECTION_HEIGHT_M: f64 = 0.094;
    /// Centre-to-centre distance between the module's two light bars, in metres
    /// (130 mm). Fitted to the CAD module; the manual gives no such figure.
    pub const LIGHT_SPAN_M: f64 = 0.130;
    /// Tip-to-tip height of one light bar, in metres (56 mm).
    pub const LIGHT_LENGTH_M: f64 = 0.056;
    /// Width of the module's artwork panel, in metres (128 mm). A visual fit to
    /// the CAD face, not a rulebook dimension.
    pub const FACE_WIDTH_M: f64 = 0.128;
    /// Height of that artwork panel, in metres (113 mm).
    pub const FACE_HEIGHT_M: f64 = 0.113;
    /// Simplified fitted housing. x is outward normal, y width, z height.
    /// Rails project 1 mm ahead of the scoring plane; backing prevents rear hits.
    pub const HOUSING_BOXES: [([f64; 3], [f64; 3]); 5] = [
        ([-0.010, 0., 0.], [0.018, 0.141, 0.135]),
        ([0., -0.0695, 0.], [0.002, 0.002, 0.135]),
        ([0., 0.0695, 0.], [0.002, 0.002, 0.135]),
        ([0., 0., -0.062], [0.002, 0.137, 0.011]),
        ([0., 0., 0.062], [0.002, 0.137, 0.011]),
    ];
    /// Conservative tower box relative to the base, excluding scored armor.
    pub const BODY_CENTER_M: [f64; 3] = [PIVOT_CAD_M[0], -PIVOT_CAD_M[2], 0.70];
    /// Full size of that tower box, in metres: 0.30 by 0.34 by 1.40. Pass it to
    /// [`WorldPhysics::add_static_box`](crate::WorldPhysics::add_static_box),
    /// which halves it into collider extents.
    pub const BODY_SIZE_M: [f64; 3] = [0.30, 0.34, 1.40];

    /// World poses of the three armor scoring faces at rotor angle
    /// `angle_rad`, in face order. Each face carries the fitted module centre
    /// and points its +x outward, 1 mm behind the measured housing face. The
    /// caller supplies activation, speed and stops.
    pub fn armor_poses_at(origin: Pose, pivot_cad_m: [f64; 3], angle_rad: f64) -> [Pose; 3] {
        let (s, c) = angle_rad.sin_cos();
        std::array::from_fn(|id| {
            let [x, y, z] = FRONT_CAD_M[id];
            let yaw = angle_rad + std::f64::consts::PI + id as f64 * std::f64::consts::TAU / 3.;
            let (sy, cy) = (yaw / 2.).sin_cos();
            let (sp, cp) = (15_f64.to_radians() / 2.).sin_cos();
            // Tower-local yaw then pitch, carried into the world by the base pose.
            let rotation_wxyz =
                multiply(origin.rotation_wxyz, [cy * cp, -sy * sp, cy * sp, sy * cp]);
            // CAD rotor frame (x forward, y up, z right) to tower FLU, spun about up.
            let center = offset_pose(
                origin,
                [
                    pivot_cad_m[0] + c * x + s * z,
                    -pivot_cad_m[2] + s * x - c * z,
                    pivot_cad_m[1] + y,
                ],
            )
            .translation_m;
            // Scoring/optical face sits 1 mm behind the measured outer housing face.
            let normal = rotate(rotation_wxyz, [1., 0., 0.]);
            Pose {
                translation_m: std::array::from_fn(|i| center[i] - 0.001 * normal[i]),
                rotation_wxyz,
            }
        })
    }
    /// Translate `pose` by `offset` in metres, measured in the pose's own frame.
    pub fn offset_pose(pose: Pose, offset: [f64; 3]) -> Pose {
        let d = rotate(pose.rotation_wxyz, offset);
        Pose {
            translation_m: std::array::from_fn(|i| pose.translation_m[i] + d[i]),
            ..pose
        }
    }
    /// Hamilton product `a * b` of wxyz quaternions: apply `b`, then `a`.
    fn multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
        let [aw, ax, ay, az] = a;
        let [bw, bx, by, bz] = b;
        [
            aw * bw - ax * bx - ay * by - az * bz,
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
        ]
    }
    /// Rotate the point `p` by the unit wxyz quaternion `q`.
    pub fn rotate(q: [f64; 4], p: [f64; 3]) -> [f64; 3] {
        let [w, x, y, z] = q;
        let t = [
            2. * (y * p[2] - z * p[1]),
            2. * (z * p[0] - x * p[2]),
            2. * (x * p[1] - y * p[0]),
        ];
        [
            p[0] + w * t[0] + y * t[2] - z * t[1],
            p[1] + w * t[1] + z * t[0] - x * t[2],
            p[2] + w * t[2] + x * t[1] - y * t[0],
        ]
    }
}

/// Power Rune target poses and the scoring-face axis conversion.
///
/// ```
/// use rm_simulator_physics::{
///     motion::rune::{scoring_pose, target_poses, BLADE_COUNT},
///     Pose,
/// };
///
/// let hub = Pose::at([6.0, 0.0, 1.6]);
/// let radius_m = 0.35;
/// let targets = target_poses(hub, radius_m, 0.0);
/// assert_eq!(targets.len(), BLADE_COUNT);
///
/// // Every target sits on the orbit, and neighbours are one fifth of a turn
/// // apart, which is the chord of a regular pentagon.
/// let chord_m = 2.0 * radius_m * (std::f64::consts::PI / BLADE_COUNT as f64).sin();
/// for target in targets {
///     let distance_m = (0..3)
///         .map(|axis| (target.translation_m[axis] - hub.translation_m[axis]).powi(2))
///         .sum::<f64>()
///         .sqrt();
///     assert!((distance_m - radius_m).abs() < 1e-12);
///     // The scoring pose keeps the centre and turns the target axis to face out.
///     assert_eq!(scoring_pose(target).translation_m, target.translation_m);
/// }
/// for pair in targets.windows(2) {
///     let separation_m = (0..3)
///         .map(|axis| (pair[1].translation_m[axis] - pair[0].translation_m[axis]).powi(2))
///         .sum::<f64>()
///         .sqrt();
///     assert!((separation_m - chord_m).abs() < 1e-12);
/// }
/// ```
pub mod rune {
    use super::*;
    /// Targets per rune: five blades, one per arm. The Small Rune lights one at
    /// a time and the Big Rune lights groups of two (section 5.5.2.1).
    pub const BLADE_COUNT: usize = 5;
    /// Hamilton product `a * b` of wxyz quaternions: apply `b`, then `a`.
    pub(crate) fn multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
        let [w, x, y, z] = a;
        let [bw, bx, by, bz] = b;
        [
            w * bw - x * bx - y * by - z * bz,
            w * bx + x * bw + y * bz - z * by,
            w * by - x * bz + y * bw + z * bx,
            w * bz + x * by - y * bx + z * bw,
        ]
    }
    /// Rotate `point` by the unit wxyz quaternion `q`.
    pub(crate) fn rotate(q: [f64; 4], point: [f64; 3]) -> [f64; 3] {
        let p = multiply(q, [0.0, point[0], point[1], point[2]]);
        let result = multiply(p, [q[0], -q[1], -q[2], -q[3]]);
        [result[1], result[2], result[3]]
    }

    /// Turn a [`target_poses`] target axis into the outward scoring-face +x
    /// that [`TargetFace`](crate::TargetFace) expects. The centre is unchanged.
    pub fn scoring_pose(target: Pose) -> Pose {
        let [w, x, y, z] = target.rotation_wxyz;
        Pose {
            translation_m: target.translation_m,
            rotation_wxyz: [-z, y, -x, w],
        }
    }

    /// Target poses at a caller-selected rotor angle, also used by collider inspection.
    /// Blade 0 sits at `angle_rad` in the hub frame's yz plane and each later
    /// blade is one fifth of a turn further on.
    pub fn target_poses(hub: Pose, radius_m: f64, angle_rad: f64) -> [Pose; BLADE_COUNT] {
        std::array::from_fn(|blade| {
            let angle = angle_rad + blade as f64 * std::f64::consts::TAU / BLADE_COUNT as f64;
            let local = [0.0, radius_m * angle.sin(), radius_m * angle.cos()];
            let offset = rotate(hub.rotation_wxyz, local);
            let half = angle * 0.5;
            let spin = [half.cos(), -half.sin(), 0.0, 0.0];
            Pose {
                translation_m: std::array::from_fn(|axis| hub.translation_m[axis] + offset[axis]),
                rotation_wxyz: multiply(hub.rotation_wxyz, spin),
            }
        })
    }
}
