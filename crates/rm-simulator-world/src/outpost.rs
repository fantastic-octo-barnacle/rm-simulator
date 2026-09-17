// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Prescribed outpost rotation and shared target/guard geometry. No renderer or scheduler.
//! Geometry was fitted from the extracted RMUC2026 CAD in `rm-vision-sim`.
use crate::{ArmorSnapshot, Pose};
use serde::{Deserialize, Serialize};

use rm_simulator_physics::motion::RotorMotion;
pub use rm_simulator_physics::motion::outpost::*;
/// Section 5.5.1 outpost HP.
pub const INITIAL_HP: u32 = 1500;

/// One outpost frame at an authoritative tick: the rotor angle and the three
/// armor poses it implies. HP and destruction travel with it so a client can
/// freeze a destroyed tower without recomputing anything.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutpostSnapshot {
    /// Configured motion and fitted pivot for client pose reconstruction.
    pub speed_rad_s: f64,
    /// CAD-space rotor pivot in metres; the fitted pivot when the asset did
    /// not supply a verified one.
    pub pivot_cad_m: [f64; 3],
    /// Field time this frame was taken at.
    pub time_ns: u64,
    /// Tower base pose; see `Outpost::new`.
    pub origin: Pose,
    /// Rotor angle at `time_ns`, radians counter-clockwise about the tower's up axis.
    pub angle_rad: f64,
    /// The three armor modules in face order, front first.
    pub armors: Vec<ArmorSnapshot>,
    /// Remaining outpost HP, at most [`INITIAL_HP`].
    pub hp: u32,
    /// HP reached zero; the middle armor stopped where it was.
    pub destroyed: bool,
}
impl OutpostSnapshot {
    /// Reconstruct armor geometry from the transmitted rotor pose, including
    /// a destroyed outpost frozen between ticks. HP is never recomputed.
    pub fn rebuild_armors(&mut self) {
        self.armors = armor_poses_at(self.origin, self.pivot_cad_m, self.angle_rad)
            .into_iter()
            .enumerate()
            .map(|(id, pose)| ArmorSnapshot {
                id: id as u32,
                pose,
            })
            .collect();
    }
    /// Presentation only: retain authoritative destruction and HP.
    pub fn presentation_at(&self, time_ns: u64) -> Self {
        if self.destroyed || time_ns <= self.time_ns {
            return self.clone();
        }
        let rotor = Outpost {
            motion: RotorMotion {
                pivot_cad_m: self.pivot_cad_m,
                origin: self.origin,
                speed_rad_s: self.speed_rad_s,
                stopped_at_ns: None,
            },
            hp: self.hp,
        };
        rotor.snapshot(time_ns)
    }
}

/// A rotating outpost tower with its own HP. Rotation is a pure function of
/// time: the tower spins at its configured rate until a strike destroys it,
/// and then holds the angle it had. The three armor faces are fitted from the
/// extracted RMUC 2026 CAD and scored by the field.
///
/// ```rust
/// use rm_simulator_world::{Outpost, Pose};
///
/// // One revolution per six seconds, based on the floor at the origin.
/// let tower = Outpost::new(Pose::at([4.0, 3.0, 0.0]), std::f64::consts::PI / 3.0).unwrap();
/// let first = tower.snapshot(0);
/// let later = tower.snapshot(1_000_000_000);
/// assert!((later.angle_rad - first.angle_rad - std::f64::consts::PI / 3.0).abs() < 1e-9);
/// assert_eq!(first.armors.len(), 3);
/// assert_eq!(first.hp, rm_simulator_world::outpost::INITIAL_HP);
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "OutpostRecord", into = "OutpostRecord")]
pub struct Outpost {
    motion: RotorMotion,
    hp: u32,
}
/// The serialized outpost: the rotor fields inline beside HP, spelled out
/// rather than flattened so positional (non-self-describing) formats can carry
/// it. Field order and names keep the existing checkpoint shape.
#[derive(Serialize, Deserialize)]
struct OutpostRecord {
    pivot_cad_m: [f64; 3],
    origin: Pose,
    speed_rad_s: f64,
    destroyed_ns: Option<u64>,
    hp: u32,
}
impl From<OutpostRecord> for Outpost {
    fn from(record: OutpostRecord) -> Self {
        Self {
            motion: RotorMotion {
                pivot_cad_m: record.pivot_cad_m,
                origin: record.origin,
                speed_rad_s: record.speed_rad_s,
                stopped_at_ns: record.destroyed_ns,
            },
            hp: record.hp,
        }
    }
}
impl From<Outpost> for OutpostRecord {
    fn from(outpost: Outpost) -> Self {
        Self {
            pivot_cad_m: outpost.motion.pivot_cad_m,
            origin: outpost.motion.origin,
            speed_rad_s: outpost.motion.speed_rad_s,
            destroyed_ns: outpost.motion.stopped_at_ns,
            hp: outpost.hp,
        }
    }
}
impl Outpost {
    /// `origin` places the tower base on the floor in world FLU coordinates; its
    /// rotation maps the tower's local FLU frame (forward = CAD +x, up = CAD +y)
    /// into the world. The rotation must be unit length.
    pub fn new(origin: Pose, speed_rad_s: f64) -> Result<Self, &'static str> {
        Self::with_pivot(origin, speed_rad_s, PIVOT_CAD_M)
    }
    /// Use a verified asset-space pivot; detector offsets remain relative to it.
    pub fn with_pivot(
        origin: Pose,
        speed_rad_s: f64,
        pivot_cad_m: [f64; 3],
    ) -> Result<Self, &'static str> {
        if !pivot_cad_m.iter().all(|v| v.is_finite()) {
            return Err("nonfinite outpost pivot");
        }
        let norm = origin.rotation_wxyz.iter().map(|v| v * v).sum::<f64>();
        if !origin.translation_m.iter().all(|v| v.is_finite())
            || !norm.is_finite()
            || (norm - 1.).abs() > 1e-6
            || !speed_rad_s.is_finite()
            || speed_rad_s.abs() > 10.
        {
            return Err(
                "outpost origin must be finite with a unit rotation and |speed| must be <= 10 rad/s",
            );
        }
        Ok(Self {
            motion: RotorMotion {
                pivot_cad_m,
                origin,
                speed_rad_s,
                stopped_at_ns: None,
            },
            hp: INITIAL_HP,
        })
    }
    /// Tower base pose, unchanged by rotation.
    pub fn origin(&self) -> Pose {
        self.motion.origin
    }
    /// Re-check the constructor's invariants on a decoded rotor.
    pub fn validate(&self) -> Result<(), &'static str> {
        Self::with_pivot(
            self.motion.origin,
            self.motion.speed_rad_s,
            self.motion.pivot_cad_m,
        )?;
        if self.hp > INITIAL_HP || (self.hp == 0) != self.motion.stopped_at_ns.is_some() {
            return Err("outpost HP and destruction disagree");
        }
        Ok(())
    }
    /// Operator HP correction. Revival resumes the configured rotor motion.
    pub fn set_hp(&mut self, time_ns: u64, hp: u32) -> Result<(), &'static str> {
        if hp > INITIAL_HP {
            return Err("outpost HP must be at most 1500");
        }
        if hp == 0 {
            self.damage(time_ns, self.hp);
        } else {
            self.hp = hp;
            self.motion.stopped_at_ns = None;
        }
        Ok(())
    }
    /// Remaining HP; zero means the tower is destroyed and frozen.
    pub fn hp(&self) -> u32 {
        self.hp
    }
    /// Remove HP for a detected strike at `time_ns`; returns the HP actually lost.
    /// Rotation freezes when the outpost is destroyed (section 5.5.1).
    ///
    /// ```rust
    /// use rm_simulator_world::{Outpost, Pose, outpost::INITIAL_HP};
    ///
    /// let mut tower = Outpost::new(Pose::at([4.0, 3.0, 0.0]), std::f64::consts::PI / 3.0).unwrap();
    /// assert_eq!(tower.damage(100_000_000, 20), 20);
    /// assert_eq!(tower.hp(), INITIAL_HP - 20);
    /// // The finishing strike costs 1480 HP and stops the rotor where it stood.
    /// assert_eq!(tower.damage(500_000_000, u32::MAX), INITIAL_HP - 20);
    /// let stopped = tower.snapshot(500_000_000);
    /// assert!(stopped.destroyed);
    /// assert_eq!(stopped.angle_rad, tower.snapshot(9_000_000_000).angle_rad);
    /// // A destroyed outpost absorbs nothing further.
    /// assert_eq!(tower.damage(600_000_000, 20), 0);
    /// ```
    pub fn damage(&mut self, time_ns: u64, amount: u32) -> u32 {
        if self.motion.stopped_at_ns.is_some() {
            return 0;
        }
        let lost = amount.min(self.hp);
        self.hp -= lost;
        if self.hp == 0 {
            self.motion.stopped_at_ns = Some(time_ns);
        }
        lost
    }
    fn angle_at(&self, time_ns: u64) -> f64 {
        self.motion.angle_at(time_ns)
    }
    /// Conservative tower box, excluding scored armor. A temporary structural proxy.
    pub fn body_proxy(&self) -> (Pose, [f64; 3]) {
        (offset_pose(self.motion.origin, BODY_CENTER_M), BODY_SIZE_M)
    }
    /// Scoring face poses without allocating an observer snapshot.
    pub(crate) fn armor_poses(&self, time_ns: u64) -> [Pose; 3] {
        armor_poses_at(
            self.motion.origin,
            self.motion.pivot_cad_m,
            self.angle_at(time_ns),
        )
    }
    /// The tower's frame at `time_ns`: rotor angle, three armor poses, HP and
    /// destruction. Reading it never advances the field.
    pub fn snapshot(&self, time_ns: u64) -> OutpostSnapshot {
        let armors = self
            .armor_poses(time_ns)
            .into_iter()
            .enumerate()
            .map(|(id, pose)| ArmorSnapshot {
                id: id as u32,
                pose,
            })
            .collect();
        OutpostSnapshot {
            speed_rad_s: self.motion.speed_rad_s,
            pivot_cad_m: self.motion.pivot_cad_m,
            time_ns,
            origin: self.motion.origin,
            angle_rad: self.angle_at(time_ns),
            armors,
            hp: self.hp,
            destroyed: self.motion.stopped_at_ns.is_some(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exported_pivot_changes_scoring_positions_without_moving_the_tower() {
        let old = Outpost::new(Pose::default(), 2.).unwrap();
        let delta = [0.1, 0.2, -0.3];
        let pivot = std::array::from_fn(|i| PIVOT_CAD_M[i] + delta[i]);
        let new = Outpost::with_pivot(Pose::default(), 2., pivot).unwrap();
        for tick in [0, 300_000_000, 1_000_000_000] {
            let a = old.snapshot(tick);
            let b = new.snapshot(tick);
            assert_eq!(a.origin, b.origin);
            for (a, b) in a.armors.iter().zip(&b.armors) {
                let expected = [delta[0], -delta[2], delta[1]];
                for (i, offset) in expected.iter().enumerate() {
                    assert!(
                        (b.pose.translation_m[i] - a.pose.translation_m[i] - offset).abs() < 1e-12
                    );
                }
                assert_eq!(a.pose.rotation_wxyz, b.pose.rotation_wxyz);
            }
        }
        assert!(Outpost::with_pivot(Pose::default(), 2., [f64::NAN; 3]).is_err());
    }
    #[test]
    fn cadence_independent_and_heights_preserved() {
        let o = Outpost::new(Pose::at([3., 0., 0.]), std::f64::consts::TAU).unwrap();
        let a = o.snapshot(0);
        let b = o.snapshot(1_000_000_000);
        for (a, b) in a.armors.iter().zip(b.armors) {
            for i in 0..3 {
                assert!((a.pose.translation_m[i] - b.pose.translation_m[i]).abs() < 1e-12);
            }
        }
        assert!(a.armors[0].pose.translation_m[2] < a.armors[2].pose.translation_m[2]);
        assert!(a.armors[2].pose.translation_m[2] < a.armors[1].pose.translation_m[2]);
        let normal = rotate(a.armors[0].pose.rotation_wxyz, [1., 0., 0.]);
        assert!(normal[0] < -0.9 && normal[2] < -0.25);
    }
    #[test]
    fn origin_translates_armor_and_body_without_changing_rotation() {
        let base = Outpost::new(Pose::at([0., 0., 0.]), 1.)
            .unwrap()
            .snapshot(250_000_000);
        let moved = Outpost::new(Pose::at([2., -1.5, 0.3]), 1.)
            .unwrap()
            .snapshot(250_000_000);
        for (a, b) in base.armors.iter().zip(&moved.armors) {
            assert_eq!(a.pose.rotation_wxyz, b.pose.rotation_wxyz);
            let delta: [f64; 3] =
                std::array::from_fn(|i| b.pose.translation_m[i] - a.pose.translation_m[i]);
            assert!((delta[0] - 2.).abs() < 1e-12);
            assert!((delta[1] + 1.5).abs() < 1e-12);
            assert!((delta[2] - 0.3).abs() < 1e-12);
        }
        let (pose, size) = Outpost::new(Pose::at([2., -1.5, 0.3]), 1.)
            .unwrap()
            .body_proxy();
        assert_eq!(size, BODY_SIZE_M);
        assert!((pose.translation_m[2] - 1.0).abs() < 1e-12);
    }
    #[test]
    fn yaw_rotates_the_whole_tower_about_its_base() {
        let quarter = std::f64::consts::FRAC_PI_2;
        let base = Outpost::new(Pose::at([0., 0., 0.]), 1.)
            .unwrap()
            .snapshot(0);
        let turned = Outpost::new(Pose::yawed([0., 0., 0.], quarter), 1.)
            .unwrap()
            .snapshot(0);
        for (a, b) in base.armors.iter().zip(&turned.armors) {
            let [x, y, z] = a.pose.translation_m;
            let expected = [-y, x, z];
            for (actual, expected) in b.pose.translation_m.iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-12);
            }
            let normal = rotate(a.pose.rotation_wxyz, [1., 0., 0.]);
            let turned_normal = rotate(b.pose.rotation_wxyz, [1., 0., 0.]);
            assert!((turned_normal[0] + normal[1]).abs() < 1e-12);
            assert!((turned_normal[1] - normal[0]).abs() < 1e-12);
            assert!((turned_normal[2] - normal[2]).abs() < 1e-12);
        }
        let (pose, _) = Outpost::new(Pose::yawed([1., 0., 0.], quarter), 1.)
            .unwrap()
            .body_proxy();
        assert!((pose.translation_m[0] - (1. - BODY_CENTER_M[1])).abs() < 1e-12);
        assert!((pose.translation_m[1] - BODY_CENTER_M[0]).abs() < 1e-12);
    }
    #[test]
    fn damage_drains_hp_and_freezes_the_rotor_at_destruction() {
        let mut o = Outpost::new(Pose::at([3., 0., 0.]), 1.).unwrap();
        assert_eq!(o.snapshot(0).hp, INITIAL_HP);
        assert_eq!(o.damage(100_000_000, 20), 20);
        assert_eq!(o.hp(), INITIAL_HP - 20);
        assert!(!o.snapshot(100_000_000).destroyed);
        assert_eq!(o.damage(500_000_000, 10_000), INITIAL_HP - 20);
        assert_eq!(o.hp(), 0);
        assert_eq!(o.damage(600_000_000, 20), 0);
        let stopped = o.snapshot(500_000_000);
        assert!(stopped.destroyed);
        assert_eq!(stopped.angle_rad, o.snapshot(2_000_000_000).angle_rad);
        assert!((stopped.angle_rad - 0.5).abs() < 1e-12);
    }
    #[test]
    fn invalid_configuration_rejected() {
        assert!(Outpost::new(Pose::at([f64::NAN, 0., 0.]), 1.).is_err());
        assert!(Outpost::new(Pose::yawed([3., 0., 0.], f64::NAN), 1.).is_err());
        let unnormalized = Pose {
            translation_m: [3., 0., 0.],
            rotation_wxyz: [1., 0., 0., 0.5],
        };
        assert!(Outpost::new(unnormalized, 1.).is_err());
        assert!(Outpost::new(Pose::at([3., 0., 0.]), f64::INFINITY).is_err());
        assert!(Outpost::new(Pose::at([3., 0., 0.]), 11.).is_err());
    }
    #[test]
    fn leaning_base_tilts_armor_positions_and_normals_together() {
        // 90 degree roll about forward: tower up becomes world left.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let rolled = Pose {
            translation_m: [0., 0., 0.],
            rotation_wxyz: [h, h, 0., 0.],
        };
        let base = Outpost::new(Pose::default(), 1.).unwrap().snapshot(0);
        let tilted = Outpost::new(rolled, 1.).unwrap().snapshot(0);
        for (a, b) in base.armors.iter().zip(&tilted.armors) {
            let [x, y, z] = a.pose.translation_m;
            let expected = [x, -z, y];
            for (actual, expected) in b.pose.translation_m.iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-12);
            }
            let n = rotate(a.pose.rotation_wxyz, [1., 0., 0.]);
            let m = rotate(b.pose.rotation_wxyz, [1., 0., 0.]);
            let expected = [n[0], -n[2], n[1]];
            for (actual, expected) in m.iter().zip(expected) {
                assert!((actual - expected).abs() < 1e-12);
            }
        }
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn presentation_matches_fitted_rotor_and_freezes_on_destruction() {
        let mut outpost =
            Outpost::with_pivot(Pose::at([2., 3., 4.]), -2.0, [0.1, 0.8, 0.03]).unwrap();
        let checkpoint = outpost.snapshot(123_000_000);
        for time in [140_000_000, 300_000_000, 623_000_000] {
            assert_eq!(checkpoint.presentation_at(time), outpost.snapshot(time));
        }
        outpost.damage(623_000_000, INITIAL_HP);
        let stopped = outpost.snapshot(623_000_000);
        assert_eq!(stopped.presentation_at(999_000_000), stopped);
        assert_eq!(checkpoint.presentation_at(0), checkpoint);
    }
}
