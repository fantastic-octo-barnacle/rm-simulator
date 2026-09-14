// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Deterministic physical RoboMaster field building blocks, without match rules,
//! networking, CAD loading, Bevy or host time. Callers supply target endpoint
//! poses and resolved mechanism state, step once per explicit `tick_ns` tick, then
//! consume contacts in returned order. See the `moving_armor` example.
//!
//! ```
//! use rm_simulator_physics::{
//!     ArmorTarget, Caliber, Pose, Shot, TargetFace, TargetFrames, WorldPhysics, tick_ns,
//! };
//!
//! let face = TargetFace {
//!     target: ArmorTarget::Outpost { outpost: 0, face: 0 },
//!     // +x is the outward normal, so a half turn faces the shooter at the origin.
//!     pose: Pose::yawed([1.5, 0.0, 1.0], std::f64::consts::PI),
//! };
//! let mut physics = WorldPhysics::new(&[face], 0.0);
//! let frames = TargetFrames::new(vec![face]);
//! physics.fire(0, Pose::at([0.0, 0.0, 1.0]), Shot::at_limit(Caliber::Mm17), None)?;
//!
//! let mut contacts = 0;
//! for tick in 0..200 {
//!     contacts += physics.step(tick * tick_ns(), &frames)?.len();
//! }
//! // The 1.5 m flight takes about 60 ticks; each ball touches the housing once.
//! assert_eq!(contacts, 1);
//! # Ok::<(), &'static str>(())
//! ```
#![deny(missing_docs)]
pub mod chassis;
pub mod geometry;
pub mod motion;
pub mod projectile;
pub use projectile::{
    ArmorTarget, Caliber, Contact, Shot, StaticGeometry, TargetFace, TargetFrames, WorldPhysics,
};
use serde::{Deserialize, Serialize};

/// Nanoseconds in one simulation tick at the shipped 1 kHz rate. Every time
/// argument in this crate counts ticks of [`tick_ns`] length, and physics
/// integrates at that fixed step; this is the default and the rollback value.
pub const DEFAULT_TICK_NS: u64 = 1_000_000;

/// Process-wide tick length in nanoseconds, frozen on first read.
static TICK_NS_CELL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// Nanoseconds in one simulation tick. Every time argument in this crate counts
/// ticks of this length, and physics integrates at this fixed step.
///
/// This is [`DEFAULT_TICK_NS`] unless [`set_tick_ns`] or the `RM_SIM_TICK_NS`
/// environment variable chose another rate before the first read. It is frozen
/// on first read so that no two parts of one process disagree, and so that a
/// snapshot's tick count always means the same span of time. Experiment 1
/// (shared physics rate) is the only reason this is not a constant.
///
/// ```
/// use rm_simulator_physics::{DEFAULT_TICK_NS, tick_ns};
///
/// // Unset by default: the shipped 1 kHz control path.
/// assert_eq!(tick_ns(), DEFAULT_TICK_NS);
/// assert_eq!(1_000_000_000 % tick_ns(), 0);
/// ```
pub fn tick_ns() -> u64 {
    *TICK_NS_CELL.get_or_init(|| {
        std::env::var("RM_SIM_TICK_NS")
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .filter(|ns| valid_tick_ns(*ns))
            .unwrap_or(DEFAULT_TICK_NS)
    })
}

/// Whether `ns` is a usable tick length: a whole nanosecond count from 100 us
/// to 20 ms that divides one second exactly, so a rule deadline expressed in
/// seconds still lands on a tick boundary.
///
/// ```
/// use rm_simulator_physics::valid_tick_ns;
///
/// assert!(valid_tick_ns(7_812_500)); // exactly 128 Hz
/// assert!(!valid_tick_ns(3_000_000)); // 333.3 Hz does not divide a second
/// ```
pub fn valid_tick_ns(ns: u64) -> bool {
    (100_000..=20_000_000).contains(&ns) && 1_000_000_000 % ns == 0
}

/// Choose the tick length before anything reads it. Returns the frozen value,
/// which is `ns` only when this call won the race and `ns` was valid. Intended
/// for experiment harnesses, once, before any [`WorldPhysics`] exists.
///
/// ```
/// use rm_simulator_physics::{set_tick_ns, tick_ns};
///
/// // An invalid rate never replaces the frozen default.
/// assert_eq!(set_tick_ns(3), tick_ns());
/// ```
pub fn set_tick_ns(ns: u64) -> u64 {
    if !valid_tick_ns(ns) {
        return tick_ns();
    }
    *TICK_NS_CELL.get_or_init(|| ns)
}

/// A rigid pose in forward/left/up coordinates, metres and a wxyz quaternion.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Pose {
    /// Translation in metres, forward/left/up.
    pub translation_m: [f64; 3],
    /// Rotation as a unit wxyz quaternion.
    pub rotation_wxyz: [f64; 4],
}
impl Default for Pose {
    fn default() -> Self {
        Self {
            translation_m: [0.0; 3],
            rotation_wxyz: [1.0, 0.0, 0.0, 0.0],
        }
    }
}
impl Pose {
    /// An unrotated pose at `translation_m`.
    ///
    /// ```
    /// use rm_simulator_physics::{chassis::yaw_of, Pose};
    ///
    /// let mut pose = Pose::at([1.0, -2.0, 0.5]);
    /// assert_eq!(pose.rotation_wxyz, [1.0, 0.0, 0.0, 0.0]);
    /// assert_eq!(yaw_of(pose), 0.0);
    ///
    /// // Yaw turns about world up, counter-clockwise from +x.
    /// pose = Pose::yawed(pose.translation_m, std::f64::consts::FRAC_PI_2);
    /// assert!((yaw_of(pose) - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    /// ```
    pub fn at(translation_m: [f64; 3]) -> Self {
        Self {
            translation_m,
            ..Self::default()
        }
    }
    /// A pose rotated about world up (yaw) only.
    ///
    /// ```
    /// use rm_simulator_physics::{motion::outpost::rotate, Pose};
    ///
    /// // Yaw is counter-clockwise about world up, so a quarter turn sends
    /// // forward (+x) to left (+y).
    /// let pose = Pose::yawed([0.0; 3], std::f64::consts::FRAC_PI_2);
    /// let forward = rotate(pose.rotation_wxyz, [1.0, 0.0, 0.0]);
    /// assert!(forward[0].abs() < 1e-12);
    /// assert!((forward[1] - 1.0).abs() < 1e-12);
    /// ```
    pub fn yawed(translation_m: [f64; 3], yaw_rad: f64) -> Self {
        let (s, c) = (yaw_rad / 2.0).sin_cos();
        Self {
            translation_m,
            rotation_wxyz: [c, 0.0, 0.0, s],
        }
    }
}

/// One of the two alliances. [`Team::index`] is the slot a team fills in the
/// fixed `[_; 2]` arrays that mechanism and geometry state use.
///
/// ```
/// use rm_simulator_physics::Team;
///
/// assert_eq!(Team::Red.index(), 0);
/// assert_eq!(Team::Blue.index(), 1);
/// assert_eq!(Team::Red.other(), Team::Blue);
/// assert_eq!(Team::BOTH.map(Team::name), ["red", "blue"]);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Team {
    /// The red alliance.
    Red,
    /// The blue alliance.
    Blue,
}
impl Team {
    /// Both teams, red then blue.
    pub const BOTH: [Team; 2] = [Team::Red, Team::Blue];
    /// Array slot for this team: red 0, blue 1.
    pub fn index(self) -> usize {
        match self {
            Team::Red => 0,
            Team::Blue => 1,
        }
    }
    /// The opposing team.
    pub fn other(self) -> Team {
        match self {
            Team::Red => Team::Blue,
            Team::Blue => Team::Red,
        }
    }
    /// Lower-case name, `"red"` or `"blue"`.
    pub fn name(self) -> &'static str {
        match self {
            Team::Red => "red",
            Team::Blue => "blue",
        }
    }
}
