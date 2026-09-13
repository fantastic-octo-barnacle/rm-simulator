// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Physical exports and the live simulation's serialized hit report.
use crate::rune::HitOutcome;
pub use rm_simulator_physics::projectile::*;
use serde::{Deserialize, Serialize};
/// Why an armor module did not register a contact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    /// Normal speed at or below the Table 5-1 threshold.
    NormalSpeed,
    /// Within the module's minimum detection interval of its previous strike.
    DetectionInterval,
    /// A rune ring the referee has switched off after earlier Big Rune
    /// activations (section 5.5.2: only rings 4..10, then 7..10, detect).
    DisabledRing,
    /// Contact on the housing, edge or back rather than the scoring face.
    OutsideTarget,
    /// 42 mm projectiles are not detected by the Power Rune.
    Caliber,
}

/// One armor contact the field evaluated at a tick, whether the module
/// registered it or not. Detected hits carry damage; rejected ones name the
/// condition that failed. The field keeps the last second of them.
///
/// ```rust
/// use rm_simulator_world::{
///     BaseConfig, Caliber, Field, FieldConfig, Pose, Rejection, Shot, Team,
/// };
///
/// let config = BaseConfig {
///     team: Team::Red,
///     plates: std::array::from_fn(|i| Pose::at([0.0, i as f64 * 0.5, 1.0])),
///     dart_offsets_m: [[0.0; 3]; 2],
/// };
/// let mut field = Field::new(&FieldConfig {
///     bases: vec![config],
///     runes: vec![],
///     outposts: vec![],
///     ..FieldConfig::default()
/// })
/// .unwrap();
/// // A 5 m/s 17 mm round is below the Table 5-1 12 m/s normal-speed threshold.
/// field
///     .fire(
///         Pose::yawed([0.05, 0.0, 1.002], std::f64::consts::PI),
///         Shot {
///             caliber: Caliber::Mm17,
///             speed_m_s: 5.0,
///         },
///         None,
///     )
///     .unwrap();
/// field.step(100).unwrap();
/// let hit = field.snapshot().hits.into_iter().next().unwrap();
/// assert!(!hit.detected);
/// assert_eq!(hit.rejection, Some(Rejection::NormalSpeed));
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArmorHit {
    /// Field time the contact was scored at, in the 1 ms tick clock.
    pub time_ns: u64,
    /// Identity of the ball, as returned by `Field::fire`.
    pub projectile: u64,
    /// The chassis that fired, when a pilot did.
    pub shooter: Option<u32>,
    /// Projectile caliber that made the contact.
    pub caliber: Caliber,
    /// Which armor module was touched.
    pub target: ArmorTarget,
    /// Contact point in world FLU metres.
    pub position_m: [f64; 3],
    /// Signed width/height offset from the scoring face centre.
    pub local_offset_m: [f64; 2],
    /// Closing speed along the plate's outward normal, positive when approaching.
    pub normal_speed_m_s: f64,
    /// The module registered the strike; `rejection` explains a `false`.
    pub detected: bool,
    /// Why the module ignored the contact; `None` for a detected strike.
    pub rejection: Option<Rejection>,
    /// Rule outcome for detected rune strikes.
    pub rune_outcome: Option<HitOutcome>,
    /// HP removed from an outpost or a robot, including the centre bonus.
    pub damage: u32,
}

const CENTER_BONUS_HALF_M: f64 = 0.005;
/// Section 5.5.1: strikes inside the 10 mm centre square earn a 150% attack buff.
pub(crate) fn outpost_damage(caliber: Caliber, offset_m: [f64; 2]) -> u32 {
    let base = caliber.outpost_damage();
    if offset_m[0].abs() <= CENTER_BONUS_HALF_M && offset_m[1].abs() <= CENTER_BONUS_HALF_M {
        base * 3 / 2
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibers_follow_the_rule_manual_tables() {
        assert_eq!(Caliber::Mm17.launch_speed_limit_m_s(), 25.);
        assert_eq!(Caliber::Mm42.launch_speed_limit_m_s(), 12.);
        assert_eq!(Caliber::Mm17.armor_detection_speed_m_s(), 12.);
        assert_eq!(Caliber::Mm42.armor_detection_speed_m_s(), 10.);
        assert_eq!(Caliber::Mm17.detection_interval_ns(), 50_000_000);
        assert_eq!(Caliber::Mm42.detection_interval_ns(), 200_000_000);
        assert_eq!(outpost_damage(Caliber::Mm17, [0.03, 0.]), 20);
        assert_eq!(outpost_damage(Caliber::Mm42, [0., 0.02]), 200);
        assert_eq!(outpost_damage(Caliber::Mm17, [0.004, -0.004]), 30);
        assert_eq!(outpost_damage(Caliber::Mm42, [0., 0.]), 300);
        assert_eq!(Shot::at_limit(Caliber::Mm42).speed_m_s, 12.);
    }
}
