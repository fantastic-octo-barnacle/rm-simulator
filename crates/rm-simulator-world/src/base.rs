// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Live base armor. Geometry is fitted from the verified CAD by the server.
use crate::{Caliber, Pose, Team};
use serde::{Deserialize, Serialize};

/// Section 5.5.1: base HP and the separate initial virtual shield.
pub const INITIAL_HP: u32 = rm_simulator_gameplay::BASE_HP;
/// Section 5.5.1: the shield is spent before HP and starts at 150.
pub const INITIAL_SHIELD_HP: u32 = rm_simulator_gameplay::BASE_SHIELD_HP;

/// Fitted scoring geometry for one live base: the seven plate poses and the
/// dart rail's two stops. The server reads both off the verified CAD package.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseConfig {
    /// Team defending this base.
    pub team: Team,
    /// Three lower plates, upper front, two other upper plates, then dart detector.
    /// +x is the outward scoring normal, +y width, +z height.
    pub plates: [Pose; 7],
    /// World translations at the dart rail's two stops, relative to its rest pose.
    pub dart_offsets_m: [[f64; 3]; 2],
}
impl BaseConfig {
    /// Reject non-finite plates, a rotation that is not unit length, or dart
    /// travel outside 1 m. The server calls this before a config reaches a field.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.plates.iter().any(|pose| {
            !pose
                .translation_m
                .iter()
                .chain(&pose.rotation_wxyz)
                .all(|v| v.is_finite())
                || (pose.rotation_wxyz.iter().map(|v| v * v).sum::<f64>() - 1.).abs() > 1e-6
        }) || self
            .dart_offsets_m
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1.)
        {
            return Err("invalid base scoring pose or dart travel");
        }
        Ok(())
    }
}
/// One base's fitted geometry with its current HP and shield. A snapshot is
/// taken once when the field is built and then updated in place by damage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseSnapshot {
    /// Plates and dart travel this state was fitted with.
    pub config: BaseConfig,
    /// Remaining HP; zero disables every plate and ends a running match.
    pub hp: u32,
    /// Remaining virtual shield, spent before HP.
    pub shield_hp: u32,
}
impl BaseSnapshot {
    /// A base at [`INITIAL_HP`] and [`INITIAL_SHIELD_HP`].
    pub fn new(config: BaseConfig) -> Self {
        Self {
            config,
            hp: INITIAL_HP,
            shield_hp: INITIAL_SHIELD_HP,
        }
    }
    /// Scoring face pose of `plate` (0 to 6) at `time_ns`. Plate 6 is the dart
    /// detector, whose translation slides between the two rail stops.
    pub fn pose(&self, plate: usize, time_ns: u64) -> Pose {
        let mut pose = self.config.plates[plate];
        if plate == 6 {
            let fraction = crate::referee::dart_target_fraction(time_ns);
            for (i, point) in pose.translation_m.iter_mut().enumerate() {
                *point += self.config.dart_offsets_m[0][i] * (1. - fraction)
                    + self.config.dart_offsets_m[1][i] * fraction;
            }
        }
        pose
    }
    /// Refill HP and shield for a new match or a reset.
    pub fn reset(&mut self) {
        self.hp = INITIAL_HP;
        self.shield_hp = INITIAL_SHIELD_HP;
    }
    /// Returns total damage absorbed by HP and shield. Shield is spent first.
    pub fn damage(&mut self, amount: u32) -> u32 {
        if self.hp == 0 {
            return 0;
        }
        let shield = amount.min(self.shield_hp);
        self.shield_hp -= shield;
        let hp = (amount - shield).min(self.hp);
        self.hp -= hp;
        shield + hp
    }
}
/// Table 5-2, checked in the local V2.2.0 manual: 17 mm does 5 HP to
/// upper front and 20 to the other five; 42 mm does 200. Section 5.5.1
/// gives the 10 mm centre square 150% damage. The seventh (dart) plate
/// accepting 20/200 projectile damage is a training override, without a bonus.
///
/// ```rust
/// use rm_simulator_world::base::damage;
/// use rm_simulator_world::Caliber;
///
/// // Outside the 10 mm centre square, the Table 5-2 values apply.
/// assert_eq!(damage(Caliber::Mm17, 0, [0.02, 0.0]), 20);
/// assert_eq!(damage(Caliber::Mm42, 0, [0.02, 0.0]), 200);
/// // Plate 3 is the upper front, worth 5 HP to a 17 mm round.
/// assert_eq!(damage(Caliber::Mm17, 3, [0.02, 0.0]), 5);
/// // A centre hit on any plate but the dart detector earns the 150 % bonus.
/// assert_eq!(damage(Caliber::Mm17, 0, [0.0, 0.0]), 30);
/// assert_eq!(damage(Caliber::Mm17, 6, [0.0, 0.0]), 20);
/// ```
pub fn damage(caliber: Caliber, plate: u32, offset_m: [f64; 2]) -> u32 {
    let amount: u32 = match caliber {
        Caliber::Mm17 if plate == 3 => 5,
        Caliber::Mm17 => 20,
        Caliber::Mm42 => 200,
    };
    if plate != 6 && offset_m.iter().all(|v| v.abs() <= 0.005) {
        (amount * 3).div_ceil(2)
    } else {
        amount
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArmorTarget, Field, FieldConfig, RefereeCommand, RefereeConfig, Shot};
    fn field() -> Field {
        Field::new(&FieldConfig {
            bases: vec![BaseConfig {
                team: Team::Red,
                plates: std::array::from_fn(|i| Pose::at([0., i as f64 * 0.5, 1.])),
                dart_offsets_m: [[0.; 3]; 2],
            }],
            runes: vec![],
            outposts: vec![],
            referee: Some(RefereeConfig::alternating(0, 0)),
            ..FieldConfig::default()
        })
        .unwrap()
    }
    fn shoot(field: &mut Field, plate: usize) {
        let center = field.snapshot().bases[0]
            .pose(plate, field.time_ns())
            .translation_m;
        field
            .fire(
                Pose::yawed(
                    [center[0] + 0.5, center[1], center[2] + 0.002],
                    std::f64::consts::PI,
                ),
                Shot::at_limit(Caliber::Mm17),
                None,
            )
            .unwrap();
        field.step(100).unwrap();
    }
    #[test]
    fn seven_plates_score_projectiles_and_share_hp_with_shield_first() {
        let mut f = field();
        for plate in 0..7 {
            shoot(&mut f, plate);
            assert!(f.snapshot().hits.iter().any(|h| h.detected
                && h.target
                    == ArmorTarget::Base {
                        base: 0,
                        plate: plate as u32
                    }));
        }
        let b = f.snapshot().bases.remove(0);
        // Five ordinary centre hits (30), upper front (8), dart override (20).
        assert_eq!((b.hp, b.shield_hp), (4972, 0));
        let snapshot = f.snapshot();
        let mut restored = Field::restore(&snapshot, &f.static_geometry_snapshot(), 0.).unwrap();
        assert_eq!(restored.snapshot().bases, snapshot.bases);
        shoot(&mut restored, 6);
        assert_eq!(restored.snapshot().bases[0].hp, 4952);
        restored
            .referee_command(RefereeCommand::StartMatch)
            .unwrap();
        restored
            .referee_command(RefereeCommand::ResetMatch)
            .unwrap();
        assert_eq!(
            (
                restored.snapshot().bases[0].hp,
                restored.snapshot().bases[0].shield_hp
            ),
            (5000, 150)
        );
    }
    #[test]
    fn destruction_finishes_running_match_and_operator_can_restore_hp() {
        let mut f = field();
        f.referee_command(RefereeCommand::StartMatch).unwrap();
        f.step(5000).unwrap();
        f.referee_command(RefereeCommand::SetBaseHp {
            team: Team::Red,
            hp: 10,
            shield_hp: 0,
        })
        .unwrap();
        shoot(&mut f, 6);
        assert_eq!(f.snapshot().bases[0].hp, 0);
        assert_eq!(
            f.snapshot().referee.unwrap().phase,
            crate::MatchPhase::Finished
        );
        assert!(f.armor_disabled(ArmorTarget::Base { base: 0, plate: 6 }));
    }
    #[test]
    fn outpost_protects_base_during_match_but_not_idle_practice() {
        let config = field().snapshot().bases.remove(0).config;
        let mut f = Field::new(&FieldConfig {
            bases: vec![config],
            runes: vec![],
            outposts: vec![crate::OutpostConfig {
                origin: Pose::at([10., 10., 0.]),
                speed_rad_s: 0.,
                pivot_cad_m: None,
            }],
            referee: Some(RefereeConfig::owned(vec![], vec![Team::Red])),
            ..FieldConfig::default()
        })
        .unwrap();
        shoot(&mut f, 0);
        assert!(f.snapshot().bases[0].shield_hp < INITIAL_SHIELD_HP);
        f.referee_command(RefereeCommand::StartMatch).unwrap();
        f.step(5000).unwrap();
        shoot(&mut f, 0);
        assert_eq!(f.snapshot().bases[0].shield_hp, INITIAL_SHIELD_HP);
        f.referee_command(RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 })
            .unwrap();
        shoot(&mut f, 0);
        assert!(f.snapshot().bases[0].shield_hp < INITIAL_SHIELD_HP);
    }
    #[test]
    fn dart_scoring_pose_follows_the_same_rail_fraction() {
        let mut b = field().snapshot().bases.remove(0);
        b.config.dart_offsets_m = [[0., -0.28, 0.], [0., 0.28, 0.]];
        assert_eq!(b.pose(0, 0), b.pose(0, 2_000_000_000));
        assert!(
            (b.pose(6, 2_000_000_000).translation_m[1] - b.pose(6, 0).translation_m[1] - 0.56)
                .abs()
                < 1e-9
        );
    }
}
