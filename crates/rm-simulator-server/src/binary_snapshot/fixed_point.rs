// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Motion precision for binary checkpoints; application choices, not rule constants.
//! Only dynamic chassis state and compact projectile position/velocity change.
//! Commands, configuration, clocks, ids, contacts, scoring and hidden rules stay
//! exact. Reconstructed quaternion components are normalized before physics use.
use crate::protocol::ServerMessage;
use serde_json::Value;

/// Precision candidates; all leave configuration, commands and rule state exact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Quantization {
    /// Preserve all current compact-checkpoint values exactly.
    None,
    /// Quantize chassis dynamics; preserve current projectile values exactly.
    Chassis,
    /// Also round projectiles to 1 mm and 0.01 m/s steps.
    Coarse,
    /// Also round projectiles to 1 mm and 1 mm/s steps. Both fit the bitpack
    /// 18-bit grids (4+3+18 bits per component instead of 4+3+24/26), and a
    /// 0.5 mm/s velocity error drifts a four-second flight by about 2 mm,
    /// far below armor scale.
    Fine,
}

/// Maximum absolute component errors from the explicitly rounded fields.
#[derive(Default)]
pub struct Errors {
    /// Position and wheel-hub component error, metres.
    pub position_m: f64,
    /// Linear velocity and wheel target speed component error, metres/second.
    pub velocity_m_s: f64,
    /// Held aim and wheel spin error, radians.
    pub angle_rad: f64,
    /// Body and gimbal angular velocity component error, radians/second.
    pub angular_velocity_rad_s: f64,
    /// Dimensionless quaternion component error before normalization.
    pub quaternion_component: f64,
    /// Out-of-range components left at their original precision.
    pub escapes: usize,
}

fn round(value: &mut Value, scale: f64, width: u32, maximum: &mut f64, escapes: &mut usize) {
    if let Some(values) = value.as_array_mut() {
        for v in values {
            round(v, scale, width, maximum, escapes);
        }
    } else if let Some(n) = value.as_f64() {
        let q = (n * scale).round();
        let bound = (1_u64 << (width - 1)) as f64;
        if !q.is_finite() || q < -bound || q >= bound {
            *escapes += 1;
            return;
        }
        let quantized = q / scale;
        *maximum = maximum.max((n - quantized).abs());
        *value = Value::from(quantized);
    }
}

fn chassis(value: &mut Value, e: &mut Errors) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                match key.as_str() {
                    "config" | "command" => (),
                    "translation_m" | "hub_m" => {
                        round(value, 1000., 18, &mut e.position_m, &mut e.escapes)
                    }
                    "velocity_m_s" | "target_m_s" => {
                        round(value, 100., 16, &mut e.velocity_m_s, &mut e.escapes)
                    }
                    "angular_velocity_rad_s" | "gimbal_velocity_rad_s" => round(
                        value,
                        1000.,
                        18,
                        &mut e.angular_velocity_rad_s,
                        &mut e.escapes,
                    ),
                    "held_aim_rad" | "spin_rad" => {
                        round(value, 10000., 24, &mut e.angle_rad, &mut e.escapes)
                    }
                    "rotation_wxyz" => round(
                        value,
                        32767.,
                        16,
                        &mut e.quaternion_component,
                        &mut e.escapes,
                    ),
                    _ => chassis(value, e),
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                chassis(value, e);
            }
        }
        _ => (),
    }
}

/// Round only explicitly selected player-checkpoint fields, retaining f64 for
/// out-of-range values. The integer representation is emitted by `bitpack`.
pub fn checkpoint(value: &mut Value, errors: &mut Errors, mode: Quantization) {
    if mode == Quantization::None {
        return;
    }
    // Extreme projectiles make the compact encoder fall back to a full
    // Snapshot. Preserve that diagnostic state exactly instead of indexing a
    // missing compact envelope.
    let Some(chassis_value) = value.pointer_mut("/CompactSnapshot/state/field/chassis") else {
        return;
    };
    chassis(chassis_value, errors);
    if mode == Quantization::Chassis {
        return;
    }
    let (position_scale, position_bits, velocity_scale, velocity_bits) =
        if mode == Quantization::Fine {
            (1000., 18, 1000., 18)
        } else {
            (1000., 18, 100., 16)
        };
    let Some(projectiles) = value
        .pointer_mut("/CompactSnapshot/projectiles")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for projectile in projectiles {
        round(
            &mut projectile[3],
            position_scale,
            position_bits,
            &mut errors.position_m,
            &mut errors.escapes,
        );
        round(
            &mut projectile[4],
            velocity_scale,
            velocity_bits,
            &mut errors.velocity_m_s,
            &mut errors.escapes,
        );
    }
}

/// Normalize quantized rotations at the physics adapter boundary. Wire values
/// stay on their grid so the binary codec and retained baselines remain exact.
/// Rejects nonfinite, zero or grossly nonunit quaternion norms.
pub fn normalize(message: &mut ServerMessage) -> std::io::Result<()> {
    if let ServerMessage::Snapshot(state) = message {
        for chassis in &mut state.field.chassis {
            for pose in [&mut chassis.pose, &mut chassis.turret] {
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
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rounding_has_half_step_error_and_never_clamps_extreme_values() {
        let mut error = 0.;
        let mut escapes = 0;
        let mut values = json!([-200., -131.072, -1.23456, 0.0005, 131.0709, 200.]);
        round(&mut values, 1000., 18, &mut error, &mut escapes);
        assert!(error <= 0.0005);
        assert_eq!(escapes, 2);
        assert_eq!(
            values,
            json!([-200., -131.072, -1.235, 0.001, 131.071, 200.])
        );
        let mut chassis_value = json!({"pose": {"translation_m": [1.23456, 0., 0.]}, "config": {"hub_m": [1.23456]}, "command": {"forward_m_s": 1.23456}});
        chassis(&mut chassis_value, &mut Errors::default());
        assert_eq!(chassis_value["pose"]["translation_m"][0], 1.235);
        assert_eq!(chassis_value["config"]["hub_m"][0], 1.23456);
        assert_eq!(chassis_value["command"]["forward_m_s"], 1.23456);
    }
}
