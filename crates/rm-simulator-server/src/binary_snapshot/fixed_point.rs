// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Motion precision for binary checkpoints; application choices, not rule constants.
//! Only dynamic chassis state and compact projectile position/velocity change.
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
    /// Inside `state`, the `SimulationState`.
    State,
    /// Inside `state.field`.
    Field,
    /// Inside a chassis, outside its configuration and command.
    Chassis,
    /// Inside the hoisted projectile array.
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
            (Fine::Root, "state") => Fine::State,
            (Fine::Root, "projectiles") => Fine::Projectiles,
            (Fine::State, "field") => Fine::Field,
            (Fine::Field, "chassis") => Fine::Chassis,
            (Fine::Chassis, "config" | "command") => Fine::Exact,
            (Fine::Chassis, "translation_m") => round(1000., 18, K_TRANSLATION),
            (Fine::Chassis, "velocity_m_s") => round(100., 16, K_VELOCITY),
            (Fine::Chassis, "angular_velocity_rad_s" | "gimbal_velocity_rad_s") => {
                round(1000., 18, K_RATE)
            }
            (Fine::Chassis, "held_aim_rad" | "wheel_spin_rad") => round(10000., 24, K_ANGLE),
            (Fine::Chassis, "rotation_wxyz") => Fine::Rotation,
            (Fine::Chassis, _) => Fine::Chassis,
            (Fine::Projectiles, "position_m" | "velocity_m_s") => round(1000., 18, K_PROJECTILE),
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
// Exponential-Golomb orders for delta step differences, each the smallest
// measured over the dictionary training workloads (deltas against 32-frame
// retained baselines); application choices, not rule constants.
/// Chassis translation, millimetre steps.
const K_TRANSLATION: u32 = 8;
/// Chassis linear velocity, centimetre-per-second steps.
const K_VELOCITY: u32 = 6;
/// Chassis and gimbal rates, milliradian-per-second steps.
const K_RATE: u32 = 7;
/// Held aim and unbounded wheel spin, 0.1 mrad steps.
const K_ANGLE: u32 = 12;
/// Projectile position and velocity, millimetre steps.
const K_PROJECTILE: u32 = 10;

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
        let chassis = Fine::Root.field("state").field("field").field("chassis");
        assert_eq!(
            round(chassis.field("pose").field("translation_m"), 1.23456),
            1.235
        );
        assert_eq!(
            round(chassis.field("config").field("hub_m"), 1.23456),
            1.23456
        );
        assert_eq!(
            round(chassis.field("command").field("forward_m_s"), 1.23456),
            1.23456
        );
        assert_eq!(Fine::Root.field("state").field("paused"), Fine::Exact);
        assert_eq!(
            chassis.field("turret").field("rotation_wxyz").fixed(),
            Fixed::Rotation
        );
    }
}
