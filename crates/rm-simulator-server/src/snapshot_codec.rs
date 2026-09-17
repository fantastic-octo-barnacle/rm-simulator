// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Snapshot wire encodings. A compact independent player checkpoint, which the
//! packed checkpoint codec bitpacks and both confirmation and control paths
//! carry as JSON. No encoding here depends on an earlier transmitted frame.
use crate::{protocol::ServerMessage, simulation::SimulationState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;

/// Independent UDP player checkpoint. Physics restore values remain f64 except
/// ball position/velocity, whose f32 precision is well below 0.1 mm on this field.
/// No earlier packet or acknowledgement is needed to decode it.
#[derive(Serialize, Deserialize)]
struct PlayerSnapshot {
    state: SimulationState,
    projectiles: Vec<ProjectileWire>,
}
/// Stable identity, lifetime and shooter followed by a position and velocity
/// vector, then the retirement bookkeeping. Spin is not carried: it moves no
/// ball a client can see. The first-contact time and the open low-speed dwell
/// window are carried so a predicting client retires the same ball on the same
/// tick as the host does.
#[derive(Serialize, Deserialize)]
struct ProjectileWire(
    u64,
    rm_simulator_world::Caliber,
    u64,
    [f32; 3],
    [f32; 3],
    Option<u32>,
    Option<u64>,
    Option<u64>,
);
impl PlayerSnapshot {
    fn from_state(state: &SimulationState) -> Self {
        let mut state = state.clone();
        let projectiles = std::mem::take(&mut state.field.projectiles)
            .into_iter()
            .map(|ball| {
                ProjectileWire(
                    ball.id,
                    ball.caliber,
                    ball.launched_ns,
                    ball.position_m.map(|x| x as f32),
                    ball.velocity_m_s.map(|x| x as f32),
                    ball.shooter,
                    ball.first_contact_ns,
                    ball.dwell_since_ns,
                )
            })
            .collect();
        for rune in &mut state.field.runes {
            rune.target_poses = [rm_simulator_world::Pose::at([0.; 3]); 5];
        }
        for outpost in &mut state.field.outposts {
            outpost.armors.clear();
        }
        for chassis in &mut state.field.chassis {
            for wheel in &mut chassis.wheels {
                wheel.contact = None;
            }
        }
        // Failed-contact diagnostics only fed the removed impact markers. Keep
        // genuine registered hits, damage and rune outcomes for armor feedback.
        state.field.hits.retain(|hit| hit.detected);
        Self { state, projectiles }
    }
    fn into_state(mut self) -> SimulationState {
        self.state.field.projectiles = self
            .projectiles
            .into_iter()
            .map(
                |ProjectileWire(
                    id,
                    caliber,
                    launched_ns,
                    position,
                    velocity,
                    shooter,
                    first_contact_ns,
                    dwell_since_ns,
                )| {
                    rm_simulator_world::ProjectileSnapshot {
                        id,
                        caliber,
                        launched_ns,
                        position_m: position.map(f64::from),
                        velocity_m_s: velocity.map(f64::from),
                        angular_velocity_rad_s: [0.; 3],
                        shooter,
                        first_contact_ns,
                        dwell_since_ns,
                    }
                },
            )
            .collect();
        for rune in &mut self.state.field.runes {
            rune.target_poses = rm_simulator_world::rune::target_poses(
                rune.hub_pose,
                rune.target_radius_m,
                rune.angle_rad,
            );
        }
        for outpost in &mut self.state.field.outposts {
            outpost.rebuild_armors();
        }
        self.state
    }
}
#[derive(Serialize, Deserialize)]
struct PlayerEnvelope {
    #[serde(rename = "CompactSnapshot")]
    snapshot: PlayerSnapshot,
}
/// Encodes a server message for an unreliable peer: a snapshot becomes an
/// independent compact checkpoint that needs no earlier frame, and every other
/// message keeps its ordinary JSON form. A projectile position or velocity that
/// cannot survive f32 is the one case that falls back to the full snapshot.
///
/// ```
/// use rm_simulator_server::protocol::ServerMessage;
/// use rm_simulator_server::simulation::Simulation;
/// use rm_simulator_server::snapshot_codec::{decode_player_message, encode_player_message};
/// use rm_simulator_world::{Field, FieldConfig};
///
/// let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
/// let mut state = simulation.state();
/// state.snapshot_id = 9;
/// let tick = state.field.tick;
/// let message = ServerMessage::Snapshot(Box::new(state));
/// let compact = encode_player_message(&message);
/// let ServerMessage::Snapshot(restored) = decode_player_message(&compact).unwrap() else {
///     panic!("a snapshot must decode to a snapshot");
/// };
/// assert_eq!(restored.snapshot_id, 9);
/// assert_eq!(restored.field.tick, tick);
/// ```
pub fn encode_player_message(message: &ServerMessage) -> Vec<u8> {
    match message {
        ServerMessage::Snapshot(state)
            if state.field.projectiles.iter().all(|ball| {
                ball.position_m
                    .into_iter()
                    .chain(ball.velocity_m_s)
                    .all(|x| (x as f32).is_finite())
            }) =>
        {
            serde_json::to_vec(&PlayerEnvelope {
                snapshot: PlayerSnapshot::from_state(state),
            })
        }
        _ => serde_json::to_vec(message),
    }
    .expect("finite protocol message")
}
/// Decodes either a compact checkpoint or an ordinary message. Errors on
/// malformed bytes rather than returning a partial state, because a decoder
/// that drops rule state gives up restoring, not drawing.
pub fn decode_player_message(bytes: &[u8]) -> io::Result<ServerMessage> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if value.get("CompactSnapshot").is_some() {
        let envelope: PlayerEnvelope = serde_json::from_value(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(ServerMessage::Snapshot(Box::new(
            envelope.snapshot.into_state(),
        )))
    } else {
        serde_json::from_value(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn outpost_motion_extraction_preserves_legacy_checkpoint_shape() {
        use rm_simulator_world::{Outpost, Pose};
        let legacy = serde_json::json!({
            "pivot_cad_m": rm_simulator_world::outpost::PIVOT_CAD_M,
            "origin": Pose::at([4.0, 3.0, 0.0]),
            "speed_rad_s": 0.4,
            "hp": 0,
            "destroyed_ns": 123_000_000_u64
        });
        let mut outpost: Outpost = serde_json::from_value(legacy.clone()).unwrap();
        outpost.validate().unwrap();
        assert_eq!(serde_json::to_value(&outpost).unwrap(), legacy);
        assert_eq!(
            outpost.snapshot(123_000_000).armors,
            outpost.snapshot(900_000_000).armors
        );
        outpost.set_hp(900_000_000, 100).unwrap();
        assert_ne!(
            outpost.snapshot(900_000_000).armors,
            outpost.snapshot(901_000_000).armors
        );
    }

    use super::*;
    use rm_simulator_world::{Field, FieldConfig};
    fn simulation() -> SimulationState {
        SimulationState {
            bots: vec![],
            input_epoch: 0,
            shot_results: Vec::new(),
            snapshot_id: 0,
            paused: false,
            field: Field::new(&FieldConfig::default()).unwrap().snapshot(),
        }
    }
    fn equivalent(a: &Value, b: &Value) {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) if x.is_f64() || y.is_f64() => {
                let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                assert!((x - y).abs() < 1e-10 * x.abs().max(1.), "{x} != {y}");
            }
            (Value::Object(x), Value::Object(y)) => {
                assert!(x.keys().eq(y.keys()));
                for (key, value) in x {
                    equivalent(value, &y[key]);
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                assert_eq!(x.len(), y.len());
                for (a, b) in x.iter().zip(y) {
                    equivalent(a, b);
                }
            }
            _ => assert_eq!(a, b),
        }
    }
    #[test]
    fn extreme_diagnostic_coordinates_fall_back_to_full_precision() {
        let mut state = simulation();
        state
            .field
            .projectiles
            .push(rm_simulator_world::ProjectileSnapshot {
                id: 1,
                caliber: rm_simulator_world::Caliber::Mm17,
                launched_ns: 0,
                position_m: [1e60, 0., 0.],
                velocity_m_s: [1., 0., 0.],
                angular_velocity_rad_s: [2.; 3],
                shooter: None,
                first_contact_ns: None,
                dwell_since_ns: None,
            });
        let message = ServerMessage::Snapshot(Box::new(state));
        assert_eq!(
            decode_player_message(&encode_player_message(&message)).unwrap(),
            message
        );
    }

    #[test]
    fn independent_player_checkpoints_reduce_bytes_and_preserve_reconciliation() {
        use rm_simulator_world::{Caliber, ChassisConfig, ChassisPlacement, Pose, Shot, Team};
        let mut config = FieldConfig::default();
        config.chassis.push(ChassisPlacement {
            team: Team::Red,
            kind: rm_simulator_world::RobotKind::Infantry,
            config: ChassisConfig::default(),
            spawn: Pose::at([0., 0., 0.2]),
        });
        let mut field = Field::new(&config).unwrap();
        let mut full_bytes = 0;
        let mut compact_bytes = 0;
        for index in 0..80 {
            if index % 4 == 0 {
                field
                    .fire(Pose::at([0., 0., 5.]), Shot::at_limit(Caliber::Mm17), None)
                    .unwrap();
            }
            field.step(16).unwrap();
            let mut state = simulation();
            state.snapshot_id = index + 1;
            state.field = field.snapshot();
            // Frozen rotor poses must reconstruct without moving the destroyed target.
            if index % 2 == 0 {
                state.field.outposts[0].destroyed = true;
                state.field.outposts[0].hp = 0;
            }
            let message = ServerMessage::Snapshot(Box::new(state.clone()));
            let full = serde_json::to_vec(&message).unwrap();
            let compact = encode_player_message(&message);
            full_bytes += crate::compression::compress(&full).len();
            compact_bytes += crate::compression::compress(&compact).len();
            // Decode each packet in isolation; no earlier frame or delta chain.
            let ServerMessage::Snapshot(actual) = decode_player_message(&compact).unwrap() else {
                panic!("not a snapshot")
            };
            let mut expected = state.clone();
            expected.field.hits.retain(|hit| hit.detected);
            for chassis in &mut expected.field.chassis {
                for wheel in &mut chassis.wheels {
                    wheel.contact = None;
                }
            }
            for ball in &mut expected.field.projectiles {
                ball.angular_velocity_rad_s = [0.; 3];
                let position = ball.position_m.map(|x| f64::from(x as f32));
                assert!(
                    position
                        .into_iter()
                        .zip(ball.position_m)
                        .all(|(a, b)| (a - b).abs() < 0.0001)
                );
                ball.position_m = position;
                ball.velocity_m_s = ball.velocity_m_s.map(|x| f64::from(x as f32));
            }
            equivalent(
                &serde_json::to_value(expected).unwrap(),
                &serde_json::to_value(actual).unwrap(),
            );
            assert_eq!(
                message,
                ServerMessage::Snapshot(Box::new(state)),
                "encoding must not mutate host state"
            );
        }
        eprintln!("UDP full/compact compressed bytes: {full_bytes}/{compact_bytes}");
        assert!(
            compact_bytes * 4 < full_bytes * 3,
            "expect at least 25% less compressed traffic"
        );
    }

    #[test]
    fn coarsened_projectile_checkpoints_replay_within_centimetres() {
        use crate::protocol::Command;
        // Balls in flight, cut through the compact checkpoint, then replayed
        // from the restore against the exact state: the 1 mm / 1 mm/s
        // projectile quantization must not move any ball by armor scale.
        let (mut simulation, chassis) = crate::workload::simulation(1);
        let shooter = chassis[0];
        for frame in 0..40 {
            if frame % 4 == 0 {
                let _ = simulation.apply(&Command::Fire {
                    shooter,
                    timing: None,
                });
            }
            simulation.step(4).unwrap();
        }
        let state = simulation.state();
        assert!(
            !state.field.projectiles.is_empty(),
            "the workload must hold balls in flight"
        );
        let message = ServerMessage::Snapshot(Box::new(state.clone()));
        let ServerMessage::Snapshot(decoded) =
            decode_player_message(&encode_player_message(&message)).unwrap()
        else {
            panic!("a snapshot must decode to a snapshot");
        };
        let geometry = simulation.field().static_geometry_snapshot();
        let mut exact = Field::restore(&state.field, &geometry, 0.).unwrap();
        let mut replay = Field::restore(&decoded.field, &geometry, 0.).unwrap();
        exact.step(512).unwrap();
        replay.step(512).unwrap();
        let replayed = replay.snapshot().projectiles;
        for ball in exact.snapshot().projectiles {
            let other = replayed
                .iter()
                .find(|other| other.id == ball.id)
                .unwrap_or_else(|| panic!("ball {} must survive the replay", ball.id));
            let drift_m = ball
                .position_m
                .into_iter()
                .zip(other.position_m)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                drift_m < 0.05,
                "ball {} drifted {drift_m:.4} m over four seconds",
                ball.id
            );
        }
    }
}
