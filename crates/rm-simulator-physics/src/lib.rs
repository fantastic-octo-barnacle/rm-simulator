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

/// Nanoseconds in one simulation tick: 7.8125 ms, the fixed 128 Hz rate. Every
/// time argument in this crate counts ticks of this length, and physics
/// integrates at that fixed step. [`tick_ns`] always returns this value.
pub const DEFAULT_TICK_NS: u64 = 7_812_500;

/// Nanoseconds in one simulation tick, fixed at 128 Hz (7.8125 ms) for every
/// build, host and client. Every time argument in this crate counts ticks of
/// this length, and physics integrates at this fixed step.
///
/// The tick is not selectable: there is no rate option, no environment
/// override and no per-match rate, so a snapshot's tick count always means
/// 7.8125 ms per tick.
///
/// ```
/// use rm_simulator_physics::{DEFAULT_TICK_NS, tick_ns};
///
/// assert_eq!(tick_ns(), 7_812_500);
/// assert_eq!(tick_ns(), DEFAULT_TICK_NS);
/// assert_eq!(1_000_000_000 % tick_ns(), 0);
/// assert_eq!(1_000_000_000 / tick_ns(), 128);
/// ```
pub const fn tick_ns() -> u64 {
    DEFAULT_TICK_NS
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
