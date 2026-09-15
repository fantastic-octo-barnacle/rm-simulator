// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Snapshot wire encodings. A compact independent player checkpoint plus the
//! structural patch primitives the UDP baseline codec builds its deltas from.
//! No encoding here depends on an earlier transmitted frame.
use crate::{protocol::ServerMessage, simulation::SimulationState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io};

/// A bounded structural replacement tree. Array length/key-set changes replace
/// the whole container, so joins, removals and optional values remain explicit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Patch {
    /// Replace the value at this position outright.
    Set(Value),
    /// Change the listed object keys, leaving the others untouched. The key set
    /// must match the baseline exactly; a join or removal replaces the container.
    Map(BTreeMap<String, Patch>),
    /// Change the listed array indices, leaving the others untouched. The length
    /// must match the baseline exactly, so an element added or removed replaces
    /// the whole array.
    List(BTreeMap<usize, Patch>),
}
/// Smallest structural patch turning `before` into `after`, or `None` when they
/// are equal. A nested patch is returned only when it is smaller than replacing
/// the subtree, so a delta never costs more than the state it describes.
///
/// ```
/// use rm_simulator_server::snapshot_codec::{apply, difference};
/// use serde_json::json;
///
/// let before = json!({"tick": 1, "projectiles": [{"id": 3, "x": 0.0, "y": 1.0}]});
/// let after = json!({"tick": 2, "projectiles": [{"id": 3, "x": 0.0, "y": 1.5}]});
/// let patch = difference(&before, &after).expect("values differ");
/// let mut value = before.clone();
/// apply(&mut value, patch, 0).unwrap();
/// assert_eq!(value, after);
/// // Equal values need no patch at all.
/// assert!(difference(&after, &after).is_none());
/// ```
pub fn difference(before: &Value, after: &Value) -> Option<Patch> {
    if before == after {
        return None;
    }
    let nested = match (before, after) {
        (Value::Object(a), Value::Object(b)) if a.keys().eq(b.keys()) => Some(Patch::Map(
            b.iter()
                .filter_map(|(key, value)| {
                    difference(&a[key], value).map(|patch| (key.clone(), patch))
                })
                .collect(),
        )),
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => Some(Patch::List(
            b.iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    difference(&a[index], value).map(|patch| (index, patch))
                })
                .collect(),
        )),
        _ => None,
    };
    let replacement = Patch::Set(after.clone());
    // Never inflate a subtree just to retain a fine-grained patch.
    Some(match nested {
        Some(patch)
            if serde_json::to_vec(&patch).unwrap().len()
                < serde_json::to_vec(&replacement).unwrap().len() =>
        {
            patch
        }
        _ => replacement,
    })
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid snapshot delta or missing baseline",
    )
}
/// Applies one patch to a decoded baseline, in place. Errors when the patch no
/// longer matches the value's shape or nests deeper than 32 levels, both of
/// which mean the patch and baseline disagree.
pub fn apply(value: &mut Value, patch: Patch, depth: usize) -> io::Result<()> {
    if depth > 32 {
        return Err(invalid());
    }
    match patch {
        Patch::Set(next) => *value = next,
        Patch::Map(changes) => {
            let object = value.as_object_mut().ok_or_else(invalid)?;
            for (key, change) in changes {
                apply(object.get_mut(&key).ok_or_else(invalid)?, change, depth + 1)?;
            }
        }
        Patch::List(changes) => {
            let array = value.as_array_mut().ok_or_else(invalid)?;
            for (index, change) in changes {
                apply(array.get_mut(index).ok_or_else(invalid)?, change, depth + 1)?;
            }
        }
    }
    Ok(())
}

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
            full_bytes += miniz_oxide::deflate::compress_to_vec(&full, 1).len();
            compact_bytes += miniz_oxide::deflate::compress_to_vec(&compact, 1).len();
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
}
