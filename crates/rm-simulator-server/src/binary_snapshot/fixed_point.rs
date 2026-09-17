// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Motion precision for binary checkpoints; application choices, not rule constants.
//! Only chassis motion and compact projectile position/velocity change.
//! Commands, configuration, clocks, ids, contacts, scoring and hidden rules stay
//! exact. Reconstructed quaternion components are normalized before physics use.
use crate::binary_snapshot::bitpack::{Fixed, Grid, Rounding};
use crate::protocol::ServerMessage;

/// Fine fixed-point rounding for the compact player checkpoint, selected by the
/// field path while the checkpoint is serialized and again while it is decoded,
/// so the wire never names a grid. Chassis dynamics round onto their bitpack
/// grids and chassis rotations travel smallest-three; projectile position and
/// velocity round to 1 mm and 1 mm/s, both on the 18-bit grid, and a 0.5 mm/s
/// velocity error drifts a four-second flight by about 2 mm, far below armor
/// scale. A value outside a grid's range keeps its exact bits instead of
/// clamping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fine {
    /// The `PlayerSnapshot` itself.
    Root,
    /// Inside the chassis motion array.
    Chassis,
    /// Inside the projectile array.
    Projectiles,
    /// Every `f64` beneath rounds onto this grid.
    Round(Grid),
    /// A chassis `rotation_wxyz`, packed smallest-three
    /// ([`Fixed::Rotation`]).
    Rotation,
    /// Every value beneath stays exact.
    Exact,
}
impl Rounding for Fine {
    fn field(self, key: &'static str) -> Self {
        let round = |scale, width, k| Fine::Round(Grid { scale, width, k });
        match (self, key) {
            (Fine::Root, "motion") => Fine::Chassis,
            (Fine::Root, "projectiles") => Fine::Projectiles,
            (Fine::Chassis, "translation_m") => round(TRANSLATION_SCALE, 18, K_TRANSLATION),
            (Fine::Chassis, "velocity_m_s") => round(VELOCITY_SCALE, 16, K_VELOCITY),
            (Fine::Chassis, "angular_velocity_rad_s" | "gimbal_velocity_rad_s") => {
                round(RATE_SCALE, 18, K_RATE)
            }
            (Fine::Chassis, "held_aim_rad" | "wheel_spin_rad") => round(ANGLE_SCALE, 24, K_ANGLE),
            (Fine::Chassis, "rotation_wxyz") => Fine::Rotation,
            (Fine::Chassis, _) => Fine::Chassis,
            (Fine::Projectiles, "position_m" | "velocity_m_s") => {
                round(PROJECTILE_SCALE, 18, K_PROJECTILE)
            }
            (Fine::Projectiles, _) => Fine::Projectiles,
            (Fine::Round(_), _) => self,
            _ => Fine::Exact,
        }
    }
    fn fixed(self) -> Fixed {
        match self {
            Fine::Round(grid) => Fixed::Grid(grid),
            Fine::Rotation => Fixed::Rotation,
            _ => Fixed::Exact,
        }
    }
}
/// Chassis and turret translation steps per metre (millimetres).
pub const TRANSLATION_SCALE: f64 = 1000.;
/// Chassis linear velocity steps per metre per second (cm/s).
pub const VELOCITY_SCALE: f64 = 100.;
/// Chassis angular and gimbal rate steps per radian per second (mrad/s).
pub const RATE_SCALE: f64 = 1000.;
/// Held aim and wheel spin steps per radian (0.1 mrad).
pub const ANGLE_SCALE: f64 = 10000.;
/// Projectile position and velocity steps per metre and per metre per second.
pub const PROJECTILE_SCALE: f64 = 1000.;
// Exponential-Golomb orders for delta step residuals against a dead-reckoned
// baseline (protocol 42); application choices, not rule constants. Each was
// swept one at a time on raw deltas in `network_bandwidth` and on the
// remote-cadence bandwidth probe; the two disagree (short 16 ms leads favour
// smaller orders), and these orders are within 2% of the best in both.
/// Chassis translation, millimetre steps.
const K_TRANSLATION: u32 = 6;
/// Chassis linear velocity, centimetre-per-second steps.
const K_VELOCITY: u32 = 4;
/// Chassis and gimbal rates, milliradian-per-second steps.
const K_RATE: u32 = 7;
/// Held aim and unbounded wheel spin, 0.1 mrad steps.
const K_ANGLE: u32 = 11;
/// Projectile position and velocity, millimetre steps.
const K_PROJECTILE: u32 = 7;

/// Normalize quantized rotations at the physics adapter boundary. Wire values
/// stay on their grid so the binary codec and retained baselines remain exact.
/// Rejects nonfinite, zero or grossly nonunit quaternion norms.
pub fn normalize(message: &mut ServerMessage) -> std::io::Result<()> {
    if let ServerMessage::Snapshot(state) = message {
        for chassis in &mut state.field.chassis {
            for pose in [&mut chassis.pose, &mut chassis.turret] {
                normalize_pose(pose)?;
            }
        }
    }
    Ok(())
}

/// Normalize one quantized rotation in place, refusing nonfinite, zero or
/// grossly nonunit norms, as [`normalize`] does for a whole message.
pub fn normalize_pose(pose: &mut rm_simulator_world::Pose) -> std::io::Result<()> {
    let norm = pose.rotation_wxyz.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !norm.is_finite() || norm < 0.5 || norm > 1.5 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid quantized quaternion",
        ));
    }
    for v in &mut pose.rotation_wxyz {
        *v /= norm;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_has_half_step_error_and_never_clamps_extreme_values() {
        let round = |rule: Fine, v: f64| match rule.fixed() {
            Fixed::Grid(grid) => grid.round(v),
            _ => v,
        };
        let rule = Fine::Root.field("projectiles").field("position_m");
        let values = [-200., -131.072, -1.23456, 0.0005, 131.0709, 200.];
        let rounded = values.map(|v| round(rule, v));
        assert_eq!(rounded, [-200., -131.072, -1.235, 0.001, 131.071, 200.]);
        let chassis = Fine::Root.field("motion");
        assert_eq!(
            round(chassis.field("pose").field("translation_m"), 1.23456),
            1.235
        );
        // Configuration and command live in the slow chassis records.
        let records = Fine::Root.field("chassis");
        assert_eq!(
            round(records.field("config").field("hub_m"), 1.23456),
            1.23456
        );
        assert_eq!(
            round(records.field("command").field("forward_m_s"), 1.23456),
            1.23456
        );
        assert_eq!(Fine::Root.field("header").field("paused"), Fine::Exact);
        assert_eq!(
            chassis.field("turret").field("rotation_wxyz").fixed(),
            Fixed::Rotation
        );
    }
}
