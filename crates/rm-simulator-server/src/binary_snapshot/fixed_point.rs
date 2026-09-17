// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Motion precision for binary checkpoints; application choices, not rule constants.
//! Only dynamic chassis state and compact projectile position/velocity change.
//! Commands, configuration, clocks, ids, contacts, scoring and hidden rules stay
//! exact. Reconstructed quaternion components are normalized before physics use.
use crate::protocol::ServerMessage;

/// Fine fixed-point rounding for the compact player checkpoint, selected by the
/// field path while the checkpoint is serialized. Chassis dynamics round onto
/// their bitpack grids; projectile position and velocity round to 1 mm and
/// 1 mm/s, both on the 18-bit grid, and a 0.5 mm/s velocity error drifts a
/// four-second flight by about 2 mm, far below armor scale. A value outside a
/// grid's range keeps its exact bits instead of clamping.
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
    /// Every `f64` beneath rounds to `1 / scale` steps within a signed
    /// integer of `width` bits.
    Round {
        /// Steps per unit.
        scale: f64,
        /// Signed integer width the rounded value must fit.
        width: u32,
    },
    /// Every value beneath stays exact.
    Exact,
}
impl crate::binary_snapshot::bitpack::Rounding for Fine {
    fn field(self, key: &'static str) -> Self {
        let round = |scale, width| Fine::Round { scale, width };
        match (self, key) {
            (Fine::Root, "state") => Fine::State,
            (Fine::Root, "projectiles") => Fine::Projectiles,
            (Fine::State, "field") => Fine::Field,
            (Fine::Field, "chassis") => Fine::Chassis,
            (Fine::Chassis, "config" | "command") => Fine::Exact,
            (Fine::Chassis, "translation_m") => round(1000., 18),
            (Fine::Chassis, "velocity_m_s") => round(100., 16),
            (Fine::Chassis, "angular_velocity_rad_s" | "gimbal_velocity_rad_s") => round(1000., 18),
            (Fine::Chassis, "held_aim_rad" | "wheel_spin_rad") => round(10000., 24),
            (Fine::Chassis, "rotation_wxyz") => round(32767., 16),
            (Fine::Chassis, _) => Fine::Chassis,
            (Fine::Projectiles, "position_m" | "velocity_m_s") => round(1000., 18),
            (Fine::Projectiles, _) => Fine::Projectiles,
            (Fine::Round { .. }, _) => self,
            _ => Fine::Exact,
        }
    }
    fn round(self, value: f64) -> f64 {
        let Fine::Round { scale, width } = self else {
            return value;
        };
        let q = (value * scale).round();
        let bound = (1_u64 << (width - 1)) as f64;
        if !q.is_finite() || q < -bound || q >= bound {
            return value;
        }
        q / scale
    }
}

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
    use crate::binary_snapshot::bitpack::Rounding;

    #[test]
    fn rounding_has_half_step_error_and_never_clamps_extreme_values() {
        let rule = Fine::Root.field("projectiles").field("position_m");
        let values = [-200., -131.072, -1.23456, 0.0005, 131.0709, 200.];
        let rounded = values.map(|v| rule.round(v));
        assert_eq!(rounded, [-200., -131.072, -1.235, 0.001, 131.071, 200.]);
        let chassis = Fine::Root.field("state").field("field").field("chassis");
        assert_eq!(
            chassis.field("pose").field("translation_m").round(1.23456),
            1.235
        );
        assert_eq!(
            chassis.field("config").field("hub_m").round(1.23456),
            1.23456
        );
        assert_eq!(
            chassis.field("command").field("forward_m_s").round(1.23456),
            1.23456
        );
        assert_eq!(Fine::Root.field("state").field("paused"), Fine::Exact);
    }
}
