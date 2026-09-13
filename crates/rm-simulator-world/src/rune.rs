// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Deterministic Power Rune rules, independent of physics and rendering.
//!
//! The 2026 rules (section 5.5.2) require five correct illuminated-module
//! hits, each within 2.5 s; hitting another module or exceeding that interval
//! resets activation. A rune is created in its *training* configuration: it
//! starts Activating, picks the lowest unhit blade instead of a random one,
//! and restarts one second after completion. A referee switches it to the
//! match behaviour with [`Rune::set_auto_restart`], [`Rune::set_seed`],
//! [`Rune::activate`] and [`Rune::deactivate`]: it then rests Inactive (dark,
//! still rotating) until triggered, chooses targets from a seeded generator,
//! and holds Activated until the referee takes the buff away. The optional
//! 200 ms transition blackout is omitted.
use crate::Pose;
use serde::{Deserialize, Serialize};

/// Blades on the rune wheel; five targets must be hit (section 5.5.2.1).
pub const BLADE_COUNT: usize = 5;
/// Training collision disk and effective scoring disk; not the target orbit radius.
pub const PHYSICAL_TARGET_RADIUS_M: f64 = 0.154;
/// 300 mm effective detection disk of the rune (Figure 5-18).
pub const EFFECTIVE_TARGET_RADIUS_M: f64 = 0.150;
/// Nominal 700 mm orbit of the five targets. The measured CAD orbit is
/// [`CAD_TARGET_RADIUS_M`].
pub const TARGET_RADIUS_M: f64 = 0.7;
/// Measured 2026 CAD orbit; rule timing and target scoring diameter are unchanged.
pub const CAD_TARGET_RADIUS_M: f64 = 0.6985;
/// Section 5.5.2: Small Rune speed, and the Big Rune speed outside activation.
pub const ANGULAR_SPEED_RAD_S: f64 = std::f64::consts::PI / 3.0;
const ROTATION_PERIOD_NS: u64 = 6_000_000_000;
/// Section 5.5.2.1: an illuminated module must be hit within 2.5 s.
const HIT_WINDOW_NS: u64 = 2_500_000_000;
/// Training policy: restart this long after completion.
const ACTIVATED_HOLD_NS: u64 = 1_000_000_000;

/// Rune activation state. A referee-controlled rune rests Inactive until a
/// team spends an opportunity, then Activating, and holds Activated until the
/// referee takes the buff away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuneState {
    /// Dark and rotating; hits are ignored (section 5.5.2.1 "Inactive").
    Inactive,
    /// Targets are lit in turn and the rune accepts hits.
    Activating,
    /// Every blade is lit and the buff is held; further hits are ignored.
    Activated,
}

/// One rune frame at an authoritative tick: the held angle, which blades are
/// lit, how far activation got and where the state machine stands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuneSnapshot {
    /// Analytic activation motion and its simulation-time epoch, when sinusoidal.
    pub motion: Option<(BigRuneMotion, u64)>,
    /// Which mode the rune is in.
    pub kind: RuneKind,
    /// Which of the five blades are lit as targets right now.
    pub active_blades: [bool; BLADE_COUNT],
    /// Activation progress: blades hit on a Small Rune, groups completed on a Big Rune.
    pub completed_groups: u32,
    /// Angular speed at this instant; a Big Rune's varies during activation.
    pub angular_speed_rad_s: f64,
    /// Field time this frame was taken at.
    pub time_ns: u64,
    /// Face centre pose; local +x points into the wheel, away from the shooter.
    pub hub_pose: Pose,
    /// Orbit radius of the five targets, 0.7 m nominally.
    pub target_radius_m: f64,
    /// Visual counterclockwise phase, wrapped to [0, 2pi), continuous through resets.
    pub angle_rad: f64,
    /// Whether the rune is Inactive, Activating or Activated.
    pub state: RuneState,
    /// The lit blade a Small or Big Rune demands first, when one is lit.
    pub active_blade: Option<u32>,
    /// Blades already hit in the current attempt.
    pub activated: [bool; BLADE_COUNT],
    /// The five target face poses in blade order, +x the outward scoring normal.
    pub target_poses: [Pose; BLADE_COUNT],
    /// World time at which the current state began.
    pub state_since_ns: u64,
}

impl RuneSnapshot {
    /// Reconstruct motion only; unknown hits and state transitions remain authoritative.
    pub fn presentation_angle_rad(&self, time_ns: u64) -> f64 {
        let time_ns = time_ns.max(self.time_ns);
        let displacement = self.motion.map_or_else(
            || self.angular_speed_rad_s * (time_ns - self.time_ns) as f64 * 1e-9,
            |(motion, epoch)| {
                motion.displacement_rad(time_ns.saturating_sub(epoch) as f64 * 1e-9)
                    - motion.displacement_rad(self.time_ns.saturating_sub(epoch) as f64 * 1e-9)
            },
        );
        (self.angle_rad + displacement).rem_euclid(std::f64::consts::TAU)
    }
}

/// What a rune did with a hit, including the hits it refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitOutcome {
    /// A Big Rune target was hit: the first of the pair, or the optional bonus
    /// when `bonus` is set and it landed within one second of the first.
    GroupHit {
        /// Blade that was struck.
        blade: u32,
        /// Groups completed before this hit.
        group: u32,
        /// The pair's first target was already hit within the last second.
        bonus: bool,
    },
    /// A Small Rune target was hit; `next_blade` is now lit.
    Accepted {
        /// Blade that was struck.
        blade: u32,
        /// Blade the rune now demands.
        next_blade: u32,
    },
    /// The hit completed the rune, which now holds Activated.
    Activated {
        /// Blade that completed the sequence.
        blade: u32,
    },
    /// Resets progress, as required for hitting a non-illuminated armor module.
    WrongBlade {
        /// Blade that was struck.
        blade: u32,
        /// Blade that was lit instead.
        expected: u32,
    },
    /// No active target: the rune is Inactive or already Activated.
    Inactive,
}

/// Reasons a rune call is refused. A refused call leaves the rune unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RuneError {
    /// The hub pose is not finite, its quaternion is zero, or a decoded rune
    /// carries a non-finite radius or angle offset.
    #[error("rune hub pose must be finite with a nonzero quaternion")]
    Pose,
    /// The requested time is before the rune's current time.
    #[error("rune time cannot move backwards")]
    TimeReversal,
    /// The blade index is not below [`BLADE_COUNT`].
    #[error("rune blade index must be in 0..5")]
    Blade,
    /// Big Rune amplitude or frequency is outside the 2026 rule bounds, or a
    /// decoded Big Rune has a non-finite epoch.
    #[error("big rune amplitude or frequency is outside the 2026 rule bounds")]
    Motion,
}

/// Deterministic xorshift64* stream for target selection. Seeded per rune.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// A uniform index below `n` (n > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Pick one blade among `candidates` (true entries): random from the stream,
/// or the lowest index for the training policy.
fn pick_blade(candidates: &[bool; BLADE_COUNT], rng: Option<&mut Rng>) -> Option<u32> {
    let options: Vec<u32> = (0..BLADE_COUNT as u32)
        .filter(|blade| candidates[*blade as usize])
        .collect();
    if options.is_empty() {
        return None;
    }
    match rng {
        Some(rng) => Some(options[rng.below(options.len())]),
        None => Some(options[0]),
    }
}

/// A Small Rune: constant π/3 rad/s, one lit target at a time, five targets in
/// sequence (section 5.5.2.1). Its own clock is the field clock; the rune never
/// reads host time.
///
/// ```rust
/// use rm_simulator_world::{Pose, RuneState};
/// use rm_simulator_world::rune::SmallRune;
///
/// // Training policy: blade 0 is lit first, one hit each, five hits activate it.
/// let mut rune = SmallRune::new(Pose::at([6.0, 0.0, 1.6])).unwrap();
/// for blade in 0..5 {
///     rune.hit(u64::from(blade) * 100_000_000, blade).unwrap();
/// }
/// let snapshot = rune.snapshot();
/// assert_eq!(snapshot.state, RuneState::Activated);
/// assert_eq!(snapshot.activated, [true; 5]);
/// assert_eq!(snapshot.completed_groups, 5);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SmallRune {
    hub_pose: Pose,
    target_radius_m: f64,
    time_ns: u64,
    stage_started_ns: u64,
    state_since_ns: u64,
    state: RuneState,
    active_blade: Option<u32>,
    activated: [bool; BLADE_COUNT],
    /// Restart activation one second after completion (training policy).
    auto_restart: bool,
    /// Phase at time zero, so a rune converted from another kind keeps its angle.
    angle_offset_rad: f64,
    rng: Option<Rng>,
}

impl Default for SmallRune {
    fn default() -> Self {
        Self::new(Pose {
            translation_m: [5.0, 0.0, 1.6],
            ..Pose::default()
        })
        .expect("constant valid rune pose")
    }
}

impl SmallRune {
    /// A Small Rune at `hub` in the training policy: Activating with blade 0
    /// lit and restarting one second after completion. The hub rotation is
    /// normalised; a non-finite or zero pose is refused with [`RuneError::Pose`].
    pub fn new(mut hub: Pose) -> Result<Self, RuneError> {
        let norm_squared = hub.rotation_wxyz.iter().map(|v| v * v).sum::<f64>();
        if !hub
            .translation_m
            .iter()
            .chain(&hub.rotation_wxyz)
            .all(|v| v.is_finite())
            || !norm_squared.is_finite()
            || norm_squared < 1e-24
        {
            return Err(RuneError::Pose);
        }
        let norm = norm_squared.sqrt();
        for value in &mut hub.rotation_wxyz {
            *value /= norm;
        }
        Ok(Self {
            hub_pose: hub,
            target_radius_m: TARGET_RADIUS_M,
            time_ns: 0,
            stage_started_ns: 0,
            state_since_ns: 0,
            state: RuneState::Activating,
            active_blade: Some(0),
            activated: [false; BLADE_COUNT],
            auto_restart: true,
            angle_offset_rad: 0.0,
            rng: None,
        })
    }

    /// Measured 2026 CAD orbit; rule timing and target scoring diameter are unchanged.
    pub fn from_cad(hub: Pose) -> Result<Self, RuneError> {
        let mut rune = Self::new(hub)?;
        rune.target_radius_m = CAD_TARGET_RADIUS_M;
        Ok(rune)
    }

    /// Whether the rune is Inactive, Activating or Activated.
    pub fn state(&self) -> RuneState {
        self.state
    }

    /// Advance the rule state to an absolute simulation timestamp; never uses host time.
    /// Skipped intervals produce the same result as advancing nanosecond by nanosecond.
    pub fn advance_to(&mut self, time_ns: u64) -> Result<(), RuneError> {
        if time_ns < self.time_ns {
            return Err(RuneError::TimeReversal);
        }
        match self.state {
            RuneState::Inactive => {
                self.time_ns = time_ns;
                return Ok(());
            }
            RuneState::Activated => {
                if !self.auto_restart || time_ns - self.stage_started_ns < ACTIVATED_HOLD_NS {
                    self.time_ns = time_ns;
                    return Ok(());
                }
                self.reset_at(self.stage_started_ns + ACTIVATED_HOLD_NS);
            }
            RuneState::Activating => {}
        }
        // A hit exactly at 2.5 s is valid; failure first occurs one nanosecond later.
        let reset_period_ns = HIT_WINDOW_NS + 1;
        let periods = (time_ns - self.stage_started_ns) / reset_period_ns;
        // One reset per elapsed window, so a seeded stream draws the same
        // targets whether the clock jumps or steps.
        for _ in 0..periods {
            self.reset_at(self.stage_started_ns + reset_period_ns);
        }
        self.time_ns = time_ns;
        Ok(())
    }

    /// Score an externally established armor intersection. This does no projectile collision math.
    /// Invalid blade indices and backwards timestamps leave the world unchanged.
    pub fn hit(&mut self, time_ns: u64, blade: u32) -> Result<HitOutcome, RuneError> {
        if blade as usize >= BLADE_COUNT {
            return Err(RuneError::Blade);
        }
        self.advance_to(time_ns)?;
        let Some(expected) = self.active_blade else {
            return Ok(HitOutcome::Inactive);
        };
        if blade != expected {
            self.reset_at(time_ns);
            return Ok(HitOutcome::WrongBlade { blade, expected });
        }
        self.activated[blade as usize] = true;
        self.stage_started_ns = time_ns;
        self.active_blade = pick_blade(&self.activated.map(|hit| !hit), self.rng.as_mut());
        match self.active_blade {
            Some(next_blade) => Ok(HitOutcome::Accepted { blade, next_blade }),
            None => {
                self.state = RuneState::Activated;
                self.state_since_ns = time_ns;
                Ok(HitOutcome::Activated { blade })
            }
        }
    }

    /// Referee trigger: an Inactive rune starts Activating with fresh progress.
    /// Any other state is left alone.
    pub fn activate(&mut self, time_ns: u64) -> Result<(), RuneError> {
        self.advance_to(time_ns)?;
        if self.state == RuneState::Inactive {
            self.reset_at(time_ns);
        }
        Ok(())
    }

    /// Referee reset: the rune goes dark and ignores hits until activated.
    pub fn deactivate(&mut self, time_ns: u64) -> Result<(), RuneError> {
        self.advance_to(time_ns)?;
        self.stage_started_ns = time_ns;
        self.state_since_ns = time_ns;
        self.state = RuneState::Inactive;
        self.active_blade = None;
        self.activated = [false; BLADE_COUNT];
        Ok(())
    }

    /// Restart activation one second after completion (the training default),
    /// or hold Activated until a referee changes it.
    pub fn set_auto_restart(&mut self, auto_restart: bool) {
        self.auto_restart = auto_restart;
    }

    /// Choose targets from a seeded stream instead of the lowest unhit blade.
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = Some(Rng::new(seed));
    }
    /// Back to the training policy: the lowest unhit blade.
    pub fn clear_seed(&mut self) {
        self.rng = None;
    }

    fn angle_at(&self, time_ns: u64) -> f64 {
        (self.angle_offset_rad + (time_ns % ROTATION_PERIOD_NS) as f64 * 1e-9 * ANGULAR_SPEED_RAD_S)
            .rem_euclid(std::f64::consts::TAU)
    }

    /// The rune's frame at its current time: held angle, lit targets and progress.
    pub fn snapshot(&self) -> RuneSnapshot {
        let angle_rad = self.angle_at(self.time_ns);
        let target_poses = target_poses(self.hub_pose, self.target_radius_m, angle_rad);
        RuneSnapshot {
            motion: None,
            kind: RuneKind::Small,
            active_blades: std::array::from_fn(|id| self.active_blade == Some(id as u32)),
            completed_groups: self.activated.iter().filter(|x| **x).count() as u32,
            angular_speed_rad_s: ANGULAR_SPEED_RAD_S,
            time_ns: self.time_ns,
            hub_pose: self.hub_pose,
            target_radius_m: self.target_radius_m,
            angle_rad,
            state: self.state,
            active_blade: self.active_blade,
            activated: self.activated,
            target_poses,
            state_since_ns: self.state_since_ns,
        }
    }

    fn reset_at(&mut self, time_ns: u64) {
        self.stage_started_ns = time_ns;
        if self.state != RuneState::Activating {
            self.state_since_ns = time_ns;
        }
        self.state = RuneState::Activating;
        self.activated = [false; BLADE_COUNT];
        self.active_blade = pick_blade(&[true; BLADE_COUNT], self.rng.as_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cad_targets_match_measured_orbit_through_rotation() {
        let mut rune = SmallRune::from_cad(Pose::default()).unwrap();
        for time_ns in [0, 1_500_000_000, 3_000_000_000, 6_000_000_000] {
            rune.advance_to(time_ns).unwrap();
            for target in rune.snapshot().target_poses {
                let p = target.translation_m;
                assert!(p[0].abs() < 1e-12);
                assert!((p[1].hypot(p[2]) - 0.6985).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn geometry_rotation_and_advance_partition_are_deterministic() {
        let mut world = SmallRune::default();
        let initial = world.snapshot();
        assert_eq!(initial.target_poses[0].translation_m, [5.0, 0.0, 2.3]);
        for blade in initial.target_poses {
            let delta: [f64; 3] =
                std::array::from_fn(|i| blade.translation_m[i] - initial.hub_pose.translation_m[i]);
            assert!((delta.iter().map(|v| v * v).sum::<f64>().sqrt() - 0.7).abs() < 1e-12);
        }
        world.advance_to(1_500_000_000).unwrap();
        let quarter = world.snapshot();
        assert!((quarter.target_poses[0].translation_m[1] - 0.7).abs() < 1e-12);
        assert!((quarter.target_poses[0].translation_m[2] - 1.6).abs() < 1e-12);
        assert!((quarter.angle_rad - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        let mut split = SmallRune::default();
        for time in [1_000_000_000, 2_500_000_001, 3_000_000_000, 8_000_000_000] {
            split.advance_to(time).unwrap();
        }
        world.advance_to(8_000_000_000).unwrap();
        assert_eq!(world, split);
        assert_eq!(world.snapshot(), world.snapshot());
    }
    #[test]
    fn all_five_hits_hold_activated_then_restart_without_stopping_rotation() {
        let mut world = SmallRune::default();
        for blade in 0..4 {
            assert!(matches!(
                world.hit(u64::from(blade) * 100_000_000, blade).unwrap(),
                HitOutcome::Accepted { .. }
            ));
        }
        assert_eq!(
            world.hit(400_000_000, 4).unwrap(),
            HitOutcome::Activated { blade: 4 }
        );
        assert_eq!(world.snapshot().activated, [true; 5]);
        assert_eq!(world.snapshot().state_since_ns, 400_000_000);
        assert_eq!(world.hit(500_000_000, 0).unwrap(), HitOutcome::Inactive);
        world.advance_to(1_399_999_999).unwrap();
        assert_eq!(world.snapshot().state, RuneState::Activated);
        world.advance_to(1_400_000_000).unwrap();
        assert_eq!(world.snapshot().state, RuneState::Activating);
        assert_eq!(world.snapshot().active_blade, Some(0));
        assert_eq!(world.snapshot().activated, [false; 5]);
        assert!(world.snapshot().angle_rad > 0.0);
    }
    #[test]
    fn wrong_blade_and_expired_window_reset_progress() {
        let mut world = SmallRune::default();
        world.hit(0, 0).unwrap();
        assert_eq!(
            world.hit(1, 4).unwrap(),
            HitOutcome::WrongBlade {
                blade: 4,
                expected: 1
            }
        );
        assert_eq!(world.snapshot().activated, [false; 5]);
        world.hit(1, 0).unwrap();
        world.advance_to(2_500_000_001).unwrap();
        assert_eq!(world.snapshot().active_blade, Some(1));
        world.advance_to(2_500_000_002).unwrap();
        assert_eq!(world.snapshot().active_blade, Some(0));
        assert_eq!(world.snapshot().activated, [false; 5]);
    }
    #[test]
    fn invalid_calls_do_not_mutate_and_hub_rotation_composes() {
        let mut world = SmallRune::default();
        world.advance_to(1).unwrap();
        let before = world.clone();
        assert_eq!(world.advance_to(0), Err(RuneError::TimeReversal));
        assert_eq!(world.hit(20, 5), Err(RuneError::Blade));
        assert_eq!(world.hit(0, 0), Err(RuneError::TimeReversal));
        assert_eq!(world, before);
        assert!(
            SmallRune::new(Pose {
                rotation_wxyz: [0.0; 4],
                ..Pose::default()
            })
            .is_err()
        );
        assert!(
            SmallRune::new(Pose {
                translation_m: [f64::NAN, 0.0, 0.0],
                ..Pose::default()
            })
            .is_err()
        );
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let hub = Pose {
            translation_m: [1.0, 2.0, 3.0],
            rotation_wxyz: [half, 0.0, half, 0.0],
        };
        let rotated = SmallRune::new(hub).unwrap().snapshot();
        assert!((rotated.target_poses[0].translation_m[0] - 1.7).abs() < 1e-12);
        assert!((rotated.target_poses[0].translation_m[2] - 3.0).abs() < 1e-12);
    }
    /// Referee behaviour: dark until activated, holds Activated, and a
    /// seeded stream picks the targets the same way every run.
    #[test]
    fn referee_controlled_rune_rests_inactive_and_holds_activation() {
        let mut rune = SmallRune::default();
        rune.set_auto_restart(false);
        rune.set_seed(7);
        rune.deactivate(0).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Inactive);
        assert_eq!(rune.snapshot().active_blades, [false; 5]);
        rune.advance_to(30_500_000_000).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Inactive);
        assert_eq!(rune.hit(30_500_000_000, 0).unwrap(), HitOutcome::Inactive);
        assert!(rune.snapshot().angle_rad > 0.0);
        rune.activate(31_000_000_000).unwrap();
        let mut twin = rune.clone();
        assert_eq!(rune.snapshot().state, RuneState::Activating);
        assert_eq!(rune.snapshot().state_since_ns, 31_000_000_000);
        let mut t = 31_000_000_000;
        let mut order = Vec::new();
        for _ in 0..5 {
            let blade = rune.snapshot().active_blade.unwrap();
            order.push(blade);
            t += 100_000_000;
            rune.hit(t, blade).unwrap();
        }
        assert_eq!(rune.snapshot().state, RuneState::Activated);
        // Random, not simply ascending, and repeatable.
        assert_ne!(order, vec![0, 1, 2, 3, 4]);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
        let mut t2 = 31_000_000_000;
        for expected in &order {
            assert_eq!(twin.snapshot().active_blade, Some(*expected));
            t2 += 100_000_000;
            twin.hit(t2, *expected).unwrap();
        }
        assert_eq!(rune, twin);
        // Holds until the referee takes it away; activate is a no-op meanwhile.
        rune.advance_to(t + 60_000_000_000).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Activated);
        rune.activate(t + 60_000_000_000).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Activated);
        rune.deactivate(t + 61_000_000_000).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Inactive);
        assert_eq!(rune.snapshot().activated, [false; 5]);
    }
}

/// Convert the rune's inward +x target axis to the outward scoring-face +x.
pub use rm_simulator_physics::motion::rune::{scoring_pose, target_poses};

/// Which Power Rune mode a rune runs. The referee converts every rune to Big
/// at the three-minute stage change (section 5.5.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuneKind {
    /// Constant π/3 rad/s, one lit target at a time.
    Small,
    /// Sinusoidal speed during activation, five pairs of lit targets.
    Big,
}

/// Caller-selected, repeatable parameters within the 2026 rule bounds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BigRuneMotion {
    amplitude_rad_s: f64,
    frequency_rad_s: f64,
}
impl Default for BigRuneMotion {
    fn default() -> Self {
        Self {
            amplitude_rad_s: 0.9,
            frequency_rad_s: 1.94,
        }
    }
}
impl BigRuneMotion {
    /// Checked sinusoid parameters: `amplitude_rad_s` in 0.780..=1.045 and
    /// `frequency_rad_s` in 1.884..=2.000 (section 5.5.2.1). Anything else is
    /// refused with [`RuneError::Motion`].
    ///
    /// ```rust
    /// use rm_simulator_world::rune::BigRuneMotion;
    ///
    /// let motion = BigRuneMotion::new(0.9, 1.94).unwrap();
    /// // At the start of an activation the sinusoid contributes nothing, so
    /// // the speed is 2.090 - a.
    /// assert!((motion.speed_rad_s(0.0) - (2.090 - 0.9)).abs() < 1e-12);
    /// assert!(BigRuneMotion::new(1.2, 1.94).is_err());
    /// ```
    pub fn new(amplitude_rad_s: f64, frequency_rad_s: f64) -> Result<Self, RuneError> {
        if !(0.780..=1.045).contains(&amplitude_rad_s)
            || !(1.884..=2.000).contains(&frequency_rad_s)
        {
            return Err(RuneError::Motion);
        }
        Ok(Self {
            amplitude_rad_s,
            frequency_rad_s,
        })
    }
    /// Parameters drawn from the rule ranges by a seeded stream (section
    /// 5.5.2: a and ω are "any value within their respective ranges").
    fn drawn(rng: &mut Rng) -> Self {
        let unit = |rng: &mut Rng| (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
        Self {
            amplitude_rad_s: 0.780 + unit(rng) * (1.045 - 0.780),
            frequency_rad_s: 1.884 + unit(rng) * (2.000 - 1.884),
        }
    }
    /// Angular speed `elapsed_s` into an activation, in rad/s:
    /// `a*sin(omega*t) + 2.090 - a`.
    pub fn speed_rad_s(self, elapsed_s: f64) -> f64 {
        self.profile().speed_rad_s(elapsed_s)
    }
    /// Angle turned `elapsed_s` into an activation, in radians; the integral
    /// of [`BigRuneMotion::speed_rad_s`].
    pub fn displacement_rad(self, elapsed_s: f64) -> f64 {
        self.profile().displacement_rad(elapsed_s)
    }
    fn profile(self) -> rm_simulator_physics::motion::SinusoidalMotion {
        rm_simulator_physics::motion::SinusoidalMotion {
            amplitude_rad_s: self.amplitude_rad_s,
            frequency_rad_s: self.frequency_rad_s,
        }
    }
}

/// Five groups of two lit targets. The first hit is required, the second is optional.
/// Training policy waits the entire one-second bonus window before selecting a new pair.
/// Selection is deterministic (the lowest pair, or a seeded stream); the
/// blackout is omitted.
///
/// ```rust
/// use rm_simulator_world::rune::{BigRune, BigRuneMotion};
/// use rm_simulator_world::{HitOutcome, Pose, RuneState};
///
/// let mut rune =
///     BigRune::new(Pose::at([6.0, 0.0, 1.6]), false, BigRuneMotion::default()).unwrap();
/// // The training policy lights the lowest pair first.
/// assert_eq!(rune.snapshot().active_blades, [true, false, true, false, false]);
/// assert_eq!(
///     rune.hit(0, 0).unwrap(),
///     HitOutcome::GroupHit { blade: 0, group: 0, bonus: false }
/// );
/// // The pair's second target within one second is the optional bonus.
/// assert_eq!(
///     rune.hit(500_000_000, 2).unwrap(),
///     HitOutcome::GroupHit { blade: 2, group: 0, bonus: true }
/// );
/// // The group completes one second after its first hit.
/// rune.advance_to(1_000_000_001).unwrap();
/// assert_eq!(rune.snapshot().completed_groups, 1);
/// assert_eq!(rune.snapshot().state, RuneState::Activating);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BigRune {
    geometry: SmallRune,
    motion: BigRuneMotion,
    time_ns: u64,
    epoch_ns: u64,
    epoch_angle_rad: f64,
    stage_started_ns: u64,
    state_since_ns: u64,
    first_hit_ns: Option<u64>,
    state: RuneState,
    completed_groups: u32,
    active: [bool; BLADE_COUNT],
    hit: [bool; BLADE_COUNT],
    auto_restart: bool,
    rng: Option<Rng>,
}
impl BigRune {
    /// A Big Rune at `hub`, on the measured CAD orbit when `cad` is set, with
    /// the given speed function. It starts in the training policy: Activating,
    /// the lowest pair lit, restarting one second after completion.
    pub fn new(hub: Pose, cad: bool, motion: BigRuneMotion) -> Result<Self, RuneError> {
        let geometry = if cad {
            SmallRune::from_cad(hub)?
        } else {
            SmallRune::new(hub)?
        };
        let mut rune = Self {
            geometry,
            motion,
            time_ns: 0,
            epoch_ns: 0,
            epoch_angle_rad: 0.,
            stage_started_ns: 0,
            state_since_ns: 0,
            first_hit_ns: None,
            state: RuneState::Activating,
            completed_groups: 0,
            active: [false; 5],
            hit: [false; 5],
            auto_restart: true,
            rng: None,
        };
        rune.select_pair(0);
        Ok(rune)
    }
    /// Whether the rune is Inactive, Activating or Activated.
    pub fn state(&self) -> RuneState {
        self.state
    }
    fn angle_at(&self, time_ns: u64) -> f64 {
        let elapsed_s = (time_ns - self.epoch_ns) as f64 * 1e-9;
        (self.epoch_angle_rad
            + if self.state == RuneState::Activating {
                self.motion.displacement_rad(elapsed_s)
            } else {
                elapsed_s * ANGULAR_SPEED_RAD_S
            })
        .rem_euclid(std::f64::consts::TAU)
    }
    fn change_motion(&mut self, time_ns: u64, state: RuneState) {
        self.epoch_angle_rad = self.angle_at(time_ns);
        self.epoch_ns = time_ns;
        if self.state != state {
            self.state_since_ns = time_ns;
        }
        self.state = state;
    }
    fn select_pair(&mut self, time_ns: u64) {
        self.active = [false; 5];
        self.hit = [false; 5];
        match self.rng.as_mut() {
            Some(rng) => {
                let first = rng.below(5);
                let second = (first + 1 + rng.below(4)) % 5;
                self.active[first] = true;
                self.active[second] = true;
            }
            None => {
                self.active[(self.completed_groups as usize * 2) % 5] = true;
                self.active[(self.completed_groups as usize * 2 + 2) % 5] = true;
            }
        }
        self.first_hit_ns = None;
        self.stage_started_ns = time_ns;
    }
    fn reset_progress(&mut self, time_ns: u64) {
        self.completed_groups = 0;
        self.select_pair(time_ns);
    }
    /// Advance the rule state to an absolute simulation timestamp. Skipped
    /// intervals complete the same groups and draw the same seeded pairs as
    /// advancing nanosecond by nanosecond.
    pub fn advance_to(&mut self, time_ns: u64) -> Result<(), RuneError> {
        if time_ns < self.time_ns {
            return Err(RuneError::TimeReversal);
        }
        if self.state == RuneState::Inactive {
            self.time_ns = time_ns;
            return Ok(());
        }
        if let Some(first) = self.first_hit_ns {
            // A bonus hit exactly at one second still belongs to this group.
            let end = first.saturating_add(1_000_000_001);
            if time_ns >= end {
                self.completed_groups += 1;
                self.first_hit_ns = None;
                if self.completed_groups == 5 {
                    self.change_motion(end, RuneState::Activated);
                    self.stage_started_ns = end;
                    self.active = [false; 5];
                    self.hit = [true; 5];
                } else {
                    self.select_pair(end);
                }
            }
        }
        if self.state == RuneState::Activated && self.auto_restart {
            let end = self.stage_started_ns.saturating_add(ACTIVATED_HOLD_NS);
            if time_ns >= end {
                self.change_motion(end, RuneState::Activating);
                self.reset_progress(end);
            }
        }
        if self.state == RuneState::Activating && self.first_hit_ns.is_none() {
            let periods = (time_ns - self.stage_started_ns) / (HIT_WINDOW_NS + 1);
            for _ in 0..periods {
                self.reset_progress(self.stage_started_ns + HIT_WINDOW_NS + 1);
            }
        }
        self.time_ns = time_ns;
        Ok(())
    }
    /// Score a hit on `blade` at `time_ns`. A lit target is accepted; anything
    /// else resets the attempt and reports the blade that was expected.
    pub fn hit(&mut self, time_ns: u64, blade: u32) -> Result<HitOutcome, RuneError> {
        if blade >= 5 {
            return Err(RuneError::Blade);
        }
        self.advance_to(time_ns)?;
        if self.state != RuneState::Activating {
            return Ok(HitOutcome::Inactive);
        }
        if !self.active[blade as usize] {
            let expected = self.active.iter().position(|x| *x).unwrap_or(0) as u32;
            self.reset_progress(time_ns);
            return Ok(HitOutcome::WrongBlade { blade, expected });
        }
        let bonus = self.first_hit_ns.is_some();
        self.first_hit_ns.get_or_insert(time_ns);
        self.active[blade as usize] = false;
        self.hit[blade as usize] = true;
        Ok(HitOutcome::GroupHit {
            blade,
            group: self.completed_groups,
            bonus,
        })
    }
    /// Referee trigger: an Inactive rune starts Activating with fresh
    /// progress, and the speed function restarts at t = 0 with parameters
    /// drawn from the seeded stream (section 5.5.2).
    pub fn activate(&mut self, time_ns: u64) -> Result<(), RuneError> {
        self.advance_to(time_ns)?;
        if self.state == RuneState::Inactive {
            if let Some(rng) = self.rng.as_mut() {
                self.motion = BigRuneMotion::drawn(rng);
            }
            self.change_motion(time_ns, RuneState::Activating);
            self.reset_progress(time_ns);
        }
        Ok(())
    }
    /// Referee reset: dark, constant speed, hits ignored.
    pub fn deactivate(&mut self, time_ns: u64) -> Result<(), RuneError> {
        self.advance_to(time_ns)?;
        self.change_motion(time_ns, RuneState::Inactive);
        self.stage_started_ns = time_ns;
        self.first_hit_ns = None;
        self.completed_groups = 0;
        self.active = [false; 5];
        self.hit = [false; 5];
        Ok(())
    }
    /// Restart activation one second after completion (the training default),
    /// or hold Activated until a referee changes it.
    pub fn set_auto_restart(&mut self, auto_restart: bool) {
        self.auto_restart = auto_restart;
    }
    /// Choose target pairs and draw the speed parameters from a seeded stream
    /// instead of the training policy.
    pub fn set_seed(&mut self, seed: u64) {
        self.rng = Some(Rng::new(seed));
    }
    /// Back to the training policy: the lowest pair and the default motion.
    pub fn clear_seed(&mut self) {
        self.rng = None;
    }
    /// The rune's frame at its current time, with the analytic activation
    /// speed attached while it is Activating.
    pub fn snapshot(&self) -> RuneSnapshot {
        let angle_rad = self.angle_at(self.time_ns);
        let mut snapshot = self.geometry.snapshot();
        snapshot.kind = RuneKind::Big;
        snapshot.motion =
            (self.state == RuneState::Activating).then_some((self.motion, self.epoch_ns));
        snapshot.time_ns = self.time_ns;
        snapshot.angle_rad = angle_rad;
        snapshot.angular_speed_rad_s = if self.state == RuneState::Activating {
            self.motion
                .speed_rad_s((self.time_ns - self.epoch_ns) as f64 * 1e-9)
        } else {
            ANGULAR_SPEED_RAD_S
        };
        snapshot.state = self.state;
        snapshot.state_since_ns = self.state_since_ns;
        snapshot.active_blades = self.active;
        snapshot.active_blade = self.active.iter().position(|x| *x).map(|x| x as u32);
        snapshot.activated = self.hit;
        snapshot.completed_groups = self.completed_groups;
        snapshot.target_poses =
            target_poses(snapshot.hub_pose, snapshot.target_radius_m, angle_rad);
        snapshot
    }
}

/// Common runner interface; each mode owns its activation state machine. The
/// field stores one of these per entry in `FieldConfig::runes`, and the referee
/// converts them at the three-minute stage change.
///
/// ```rust
/// use rm_simulator_world::rune::SmallRune;
/// use rm_simulator_world::{Pose, Rune, RuneKind, RuneState};
///
/// let mut rune = Rune::Small(SmallRune::new(Pose::at([6.0, 0.0, 1.6])).unwrap());
/// rune.advance_to(2_000_000_000).unwrap();
/// let before = rune.snapshot();
/// rune.convert(RuneKind::Big, 2_000_000_000).unwrap();
/// let after = rune.snapshot();
/// assert_eq!(rune.kind(), RuneKind::Big);
/// // The new mode keeps the hub, orbit and angle, and waits for the referee.
/// assert_eq!(after.state, RuneState::Inactive);
/// assert!((after.angle_rad - before.angle_rad).abs() < 1e-9);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Rune {
    /// A Small Rune: five single targets in sequence.
    Small(SmallRune),
    /// A Big Rune: five pairs of targets, the second optionally a bonus.
    Big(BigRune),
}
impl Rune {
    /// Current scoring target poses without constructing an observer snapshot.
    pub(crate) fn target_poses(&self) -> [Pose; BLADE_COUNT] {
        match self {
            Self::Small(r) => target_poses(r.hub_pose, r.target_radius_m, r.angle_at(r.time_ns)),
            Self::Big(r) => target_poses(
                r.geometry.hub_pose,
                r.geometry.target_radius_m,
                r.angle_at(r.time_ns),
            ),
        }
    }
    /// Which mode this rune is in.
    pub fn kind(&self) -> RuneKind {
        match self {
            Self::Small(_) => RuneKind::Small,
            Self::Big(_) => RuneKind::Big,
        }
    }
    /// Re-check the invariants the constructors enforce. A rune that came
    /// back from a decoder never went through them.
    pub fn validate(&self) -> Result<(), RuneError> {
        let geometry = match self {
            Self::Small(r) => r,
            Self::Big(r) => &r.geometry,
        };
        let norm_squared = geometry
            .hub_pose
            .rotation_wxyz
            .iter()
            .map(|v| v * v)
            .sum::<f64>();
        if !geometry
            .hub_pose
            .translation_m
            .iter()
            .chain(&geometry.hub_pose.rotation_wxyz)
            .chain([&geometry.target_radius_m, &geometry.angle_offset_rad])
            .all(|v| v.is_finite())
            || (norm_squared - 1.0).abs() > 1e-6
            || geometry.target_radius_m <= 0.0
        {
            return Err(RuneError::Pose);
        }
        if let Self::Big(r) = self
            && (!r.epoch_angle_rad.is_finite()
                || !r.motion.amplitude_rad_s.is_finite()
                || !r.motion.frequency_rad_s.is_finite()
                || r.time_ns < r.epoch_ns)
        {
            return Err(RuneError::Motion);
        }
        Ok(())
    }
    /// Whether the rune is Inactive, Activating or Activated.
    pub fn state(&self) -> RuneState {
        match self {
            Self::Small(r) => r.state,
            Self::Big(r) => r.state,
        }
    }
    /// The rune's frame at its current time, in either mode.
    pub fn snapshot(&self) -> RuneSnapshot {
        match self {
            Self::Small(r) => r.snapshot(),
            Self::Big(r) => r.snapshot(),
        }
    }
    /// Advance the rune to an absolute simulation timestamp. Either mode gives
    /// the same result whether the clock jumps or steps.
    pub fn advance_to(&mut self, time_ns: u64) -> Result<(), RuneError> {
        match self {
            Self::Small(r) => r.advance_to(time_ns),
            Self::Big(r) => r.advance_to(time_ns),
        }
    }
    /// Score a hit on `blade` at `time_ns` and report what the rune did with it.
    pub fn hit(&mut self, time_ns: u64, blade: u32) -> Result<HitOutcome, RuneError> {
        match self {
            Self::Small(r) => r.hit(time_ns, blade),
            Self::Big(r) => r.hit(time_ns, blade),
        }
    }
    /// Referee trigger: an Inactive rune starts Activating with fresh progress.
    /// A rune that is already Activating or Activated is left alone.
    pub fn activate(&mut self, time_ns: u64) -> Result<(), RuneError> {
        match self {
            Self::Small(r) => r.activate(time_ns),
            Self::Big(r) => r.activate(time_ns),
        }
    }
    /// Referee reset: the rune goes dark and ignores hits until activated.
    pub fn deactivate(&mut self, time_ns: u64) -> Result<(), RuneError> {
        match self {
            Self::Small(r) => r.deactivate(time_ns),
            Self::Big(r) => r.deactivate(time_ns),
        }
    }
    /// Restart activation one second after completion, or hold Activated until
    /// a referee changes it.
    pub fn set_auto_restart(&mut self, auto_restart: bool) {
        match self {
            Self::Small(r) => r.set_auto_restart(auto_restart),
            Self::Big(r) => r.set_auto_restart(auto_restart),
        }
    }
    /// Choose targets, and for a Big Rune the speed parameters, from a seeded
    /// stream so every peer sees the same sequence.
    pub fn set_seed(&mut self, seed: u64) {
        match self {
            Self::Small(r) => r.set_seed(seed),
            Self::Big(r) => r.set_seed(seed),
        }
    }
    /// Back to the training policy: the lowest unhit blade or pair.
    pub fn clear_seed(&mut self) {
        match self {
            Self::Small(r) => r.clear_seed(),
            Self::Big(r) => r.clear_seed(),
        }
    }
    /// Replace the rune with one of another kind at the same hub, orbit and
    /// current angle, Inactive and referee controlled (the three-minute
    /// Small-to-Big stage change of section 5.5.2). Same kind: no change.
    pub fn convert(&mut self, kind: RuneKind, time_ns: u64) -> Result<(), RuneError> {
        if self.kind() == kind {
            return Ok(());
        }
        self.advance_to(time_ns)?;
        let snapshot = self.snapshot();
        let (auto_restart, rng) = match self {
            Self::Small(r) => (r.auto_restart, r.rng),
            Self::Big(r) => (r.auto_restart, r.rng),
        };
        let mut geometry = SmallRune::new(snapshot.hub_pose)?;
        geometry.target_radius_m = snapshot.target_radius_m;
        geometry.time_ns = time_ns;
        geometry.auto_restart = auto_restart;
        geometry.rng = rng;
        *self = match kind {
            RuneKind::Small => {
                geometry.angle_offset_rad = snapshot.angle_rad
                    - (time_ns % ROTATION_PERIOD_NS) as f64 * 1e-9 * ANGULAR_SPEED_RAD_S;
                geometry.deactivate(time_ns)?;
                Self::Small(geometry)
            }
            RuneKind::Big => {
                let mut big = BigRune {
                    geometry,
                    motion: BigRuneMotion::default(),
                    time_ns,
                    epoch_ns: time_ns,
                    epoch_angle_rad: snapshot.angle_rad,
                    stage_started_ns: time_ns,
                    state_since_ns: time_ns,
                    first_hit_ns: None,
                    state: RuneState::Inactive,
                    completed_groups: 0,
                    active: [false; 5],
                    hit: [false; 5],
                    auto_restart,
                    rng,
                };
                big.select_pair(time_ns);
                big.active = [false; 5];
                Self::Big(big)
            }
        };
        Ok(())
    }
}

#[cfg(test)]
mod big_tests {
    use super::*;
    fn big() -> BigRune {
        BigRune::new(Pose::default(), true, BigRuneMotion::default()).unwrap()
    }
    #[test]
    fn sinusoid_integral_matches_speed_and_parameter_bounds() {
        for a in [0.780, 0.9, 1.045] {
            for w in [1.884, 1.94, 2.0] {
                let motion = BigRuneMotion::new(a, w).unwrap();
                for t in [0.1, 1., 2., 6.] {
                    let derivative = (motion.displacement_rad(t + 1e-5)
                        - motion.displacement_rad(t - 1e-5))
                        / 2e-5;
                    assert!((derivative - motion.speed_rad_s(t)).abs() < 1e-8);
                }
            }
        }
        assert_eq!(BigRuneMotion::new(f64::NAN, 1.94), Err(RuneError::Motion));
        assert_eq!(BigRuneMotion::new(0.7, 1.94), Err(RuneError::Motion));
        assert_eq!(BigRuneMotion::new(0.9, 2.1), Err(RuneError::Motion));
        let mut rng = Rng::new(3);
        for _ in 0..50 {
            let drawn = BigRuneMotion::drawn(&mut rng);
            assert!(BigRuneMotion::new(drawn.amplitude_rad_s, drawn.frequency_rad_s).is_ok());
        }
    }
    #[test]
    fn bonus_is_optional_and_one_second_boundary_is_inclusive() {
        let mut rune = big();
        assert_eq!(
            rune.snapshot().active_blades,
            [true, false, true, false, false]
        );
        assert_eq!(
            rune.hit(2_500_000_000, 0).unwrap(),
            HitOutcome::GroupHit {
                blade: 0,
                group: 0,
                bonus: false
            }
        );
        assert_eq!(
            rune.hit(3_500_000_000, 2).unwrap(),
            HitOutcome::GroupHit {
                blade: 2,
                group: 0,
                bonus: true
            }
        );
        assert_eq!(rune.snapshot().completed_groups, 0);
        rune.advance_to(3_500_000_001).unwrap();
        assert_eq!(rune.snapshot().completed_groups, 1);
        assert_eq!(
            rune.snapshot().active_blades,
            [false, false, true, false, true]
        );
        rune.hit(3_600_000_000, 4).unwrap();
        rune.advance_to(4_600_000_001).unwrap();
        assert_eq!(rune.snapshot().completed_groups, 2);
    }
    #[test]
    fn five_groups_activate_then_restart_with_continuous_angle() {
        let mut rune = big();
        for group in 0..5 {
            let t = group as u64 * 1_100_000_000;
            rune.advance_to(t).unwrap();
            let blade = rune.snapshot().active_blade.unwrap();
            rune.hit(t, blade).unwrap();
            rune.advance_to(t + 1_000_000_001).unwrap();
        }
        assert_eq!(rune.snapshot().state, RuneState::Activated);
        assert_eq!(rune.snapshot().state_since_ns, 5_400_000_001);
        assert_eq!(rune.snapshot().completed_groups, 5);
        assert_eq!(rune.snapshot().active_blades, [false; 5]);
        assert_eq!(rune.snapshot().angular_speed_rad_s, ANGULAR_SPEED_RAD_S);
        let angle = rune.snapshot().angle_rad;
        rune.advance_to(6_400_000_001).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Activating);
        assert!(
            (rune.snapshot().angle_rad
                - (angle + ANGULAR_SPEED_RAD_S).rem_euclid(std::f64::consts::TAU))
            .abs()
                < 1e-9
        );
        assert_eq!(rune.snapshot().completed_groups, 0);
    }
    #[test]
    fn timeout_wrong_target_and_partitioned_advance() {
        let mut rune = big();
        rune.hit(0, 0).unwrap();
        rune.advance_to(1_000_000_001).unwrap();
        let mut split = rune.clone();
        for t in [1_500_000_000, 3_500_000_001, 3_500_000_002, 10_000_000_000] {
            split.advance_to(t).unwrap();
        }
        rune.advance_to(10_000_000_000).unwrap();
        assert_eq!(rune, split);
        assert_eq!(rune.snapshot().completed_groups, 0);
        assert!(matches!(
            rune.hit(10_000_000_001, 1).unwrap(),
            HitOutcome::WrongBlade { .. }
        ));
        let before = rune.clone();
        assert_eq!(rune.hit(20_000_000_000, 5), Err(RuneError::Blade));
        assert_eq!(rune.advance_to(0), Err(RuneError::TimeReversal));
        assert_eq!(rune, before);
    }
    /// Referee behaviour: Inactive spins at the constant speed, activation
    /// restarts the speed function, seeded pairs are distinct and
    /// repeatable, and Activated holds.
    #[test]
    fn referee_controlled_big_rune_activates_from_rest_and_holds() {
        let mut rune = big();
        rune.set_auto_restart(false);
        rune.set_seed(11);
        rune.deactivate(0).unwrap();
        rune.advance_to(3_000_000_000).unwrap();
        let resting = rune.snapshot();
        assert_eq!(resting.state, RuneState::Inactive);
        assert_eq!(resting.angular_speed_rad_s, ANGULAR_SPEED_RAD_S);
        assert!((resting.angle_rad - 3.0 * ANGULAR_SPEED_RAD_S).abs() < 1e-9);
        assert_eq!(rune.hit(3_000_000_000, 0).unwrap(), HitOutcome::Inactive);
        rune.activate(3_000_000_000).unwrap();
        let twin = rune.clone();
        assert_eq!(rune.snapshot().state, RuneState::Activating);
        // t = 0 of the speed function: speed is b = 2.090 - a.
        let speed = rune.snapshot().angular_speed_rad_s;
        assert!((1.045..=1.31).contains(&speed), "{speed}");
        let mut t = 3_000_000_000;
        for group in 0..5 {
            let active = rune.snapshot().active_blades;
            assert_eq!(active.iter().filter(|x| **x).count(), 2, "{active:?}");
            let blade = rune.snapshot().active_blade.unwrap();
            t += 200_000_000;
            assert_eq!(
                rune.hit(t, blade).unwrap(),
                HitOutcome::GroupHit {
                    blade,
                    group,
                    bonus: false
                }
            );
            t += 1_000_000_001;
            rune.advance_to(t).unwrap();
        }
        assert_eq!(rune.snapshot().state, RuneState::Activated);
        rune.advance_to(t + 90_000_000_000).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Activated);
        assert_eq!(rune.snapshot().angular_speed_rad_s, ANGULAR_SPEED_RAD_S);
        // Without hits the seeded stream picks the same pairs however the
        // clock is partitioned.
        let mut whole = twin.clone();
        whole.advance_to(t + 90_000_000_000).unwrap();
        let mut pieces = twin.clone();
        let mut at = pieces.snapshot().time_ns;
        while at < t + 90_000_000_000 {
            at = (at + 777_777_777).min(t + 90_000_000_000);
            pieces.advance_to(at).unwrap();
        }
        assert_eq!(whole.snapshot(), pieces.snapshot());
        rune.deactivate(t + 90_000_000_000).unwrap();
        assert_eq!(rune.snapshot().state, RuneState::Inactive);
        assert_eq!(rune.snapshot().completed_groups, 0);
    }
    /// The stage change keeps the hub, orbit and angle and yields an
    /// Inactive rune of the other kind.
    #[test]
    fn conversion_between_kinds_keeps_the_angle_and_rests_inactive() {
        let mut rune = Rune::Small(SmallRune::from_cad(Pose::at([1.0, 2.0, 3.0])).unwrap());
        rune.set_seed(5);
        rune.set_auto_restart(false);
        rune.advance_to(4_000_000_000).unwrap();
        let before = rune.snapshot();
        rune.convert(RuneKind::Big, 4_000_000_000).unwrap();
        let after = rune.snapshot();
        assert_eq!(rune.kind(), RuneKind::Big);
        assert_eq!(after.state, RuneState::Inactive);
        assert_eq!(after.hub_pose, before.hub_pose);
        assert_eq!(after.target_radius_m, before.target_radius_m);
        assert!((after.angle_rad - before.angle_rad).abs() < 1e-9);
        assert_eq!(after.active_blades, [false; 5]);
        rune.advance_to(5_000_000_000).unwrap();
        let later = rune.snapshot();
        assert!(
            (later.angle_rad
                - (before.angle_rad + ANGULAR_SPEED_RAD_S).rem_euclid(std::f64::consts::TAU))
            .abs()
                < 1e-9
        );
        rune.activate(5_000_000_000).unwrap();
        assert_eq!(
            rune.snapshot().active_blades.iter().filter(|x| **x).count(),
            2
        );
        // And back, at an angle a fresh Small rune could not represent.
        rune.convert(RuneKind::Small, 5_500_000_000).unwrap();
        let small = rune.snapshot();
        assert_eq!(rune.kind(), RuneKind::Small);
        assert_eq!(small.state, RuneState::Inactive);
        rune.advance_to(6_000_000_000).unwrap();
        assert!(
            (rune.snapshot().angle_rad
                - (small.angle_rad + 0.5 * ANGULAR_SPEED_RAD_S).rem_euclid(std::f64::consts::TAU))
            .abs()
                < 1e-9
        );
        rune.activate(6_000_000_000).unwrap();
        assert_eq!(rune.state(), RuneState::Activating);
        assert_eq!(
            rune.snapshot().active_blades.iter().filter(|x| **x).count(),
            1
        );
        // Same kind is a no-op.
        let same = rune.clone();
        rune.convert(RuneKind::Small, 6_000_000_000).unwrap();
        assert_eq!(rune, same);
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn analytic_reconstruction_matches_small_and_big_rule_motion() {
        let mut small = SmallRune::default();
        let mut big = BigRune::new(Pose::default(), false, BigRuneMotion::default()).unwrap();
        small.advance_to(123_000_000).unwrap();
        big.advance_to(123_000_000).unwrap();
        let checkpoints = [small.snapshot(), big.snapshot()];
        for time in [124_000_000, 151_000_000, 323_000_000, 623_000_000] {
            small.advance_to(time).unwrap();
            big.advance_to(time).unwrap();
            for (checkpoint, actual) in checkpoints.iter().zip([small.snapshot(), big.snapshot()]) {
                assert!((checkpoint.presentation_angle_rad(time) - actual.angle_rad).abs() < 1e-12);
            }
        }
        big.deactivate(623_000_000).unwrap();
        let resting = big.snapshot();
        assert!(resting.motion.is_none());
        big.advance_to(923_000_000).unwrap();
        assert!(
            (resting.presentation_angle_rad(923_000_000) - big.snapshot().angle_rad).abs() < 1e-12
        );
    }
}
