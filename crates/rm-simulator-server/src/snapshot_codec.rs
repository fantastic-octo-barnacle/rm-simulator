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
///
/// Timestamps ride as checkpoint-relative ages in nanoseconds — launch age
/// against the checkpoint time, contact age against the launch, dwell age
/// against the contact (or the launch when no contact is recorded) — instead
/// of absolute simulation times. A four-second ball keeps every age under
/// 2^33, where an absolute time costs a 9–10 byte varint. Reconstruction is
/// exact whenever launch precedes contact precedes dwell; `saturating`
/// arithmetic only clips states that ordering already rules out.
#[derive(Serialize, Deserialize)]
struct ProjectileWire(
    u64,
    CaliberBit,
    u64,
    [f32; 3],
    [f32; 3],
    Option<u32>,
    Option<u64>,
    Option<u64>,
);
/// Projectile caliber as one bit on the compact checkpoint: `false` is
/// 17 mm, `true` is 42 mm (rulebook section 1.4 projectile sizes). A third
/// caliber would need a protocol bump; the bit order is fixed on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaliberBit(bool);
impl Serialize for CaliberBit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(self.0)
    }
}
impl<'de> Deserialize<'de> for CaliberBit {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(bool::deserialize(deserializer)?))
    }
}
impl From<rm_simulator_world::Caliber> for CaliberBit {
    fn from(caliber: rm_simulator_world::Caliber) -> Self {
        Self(matches!(caliber, rm_simulator_world::Caliber::Mm42))
    }
}
impl From<CaliberBit> for rm_simulator_world::Caliber {
    fn from(bit: CaliberBit) -> Self {
        if bit.0 { Self::Mm42 } else { Self::Mm17 }
    }
}
/// Two-variant rule enums ride the compact checkpoint as single bits and the
/// rune/chassis kinds as two-bit indexes, scoped by field name so display
/// text (refusal reasons) and unrelated booleans pass through untouched.
/// Unknown strings stay strings, so a new variant costs bytes but still
/// decodes. Control and confirmation JSON keeps the string forms.
///
/// Namespaces share the `kind`/`stage` keys between robot and rune enums,
/// whose serialized names are disjoint; rune states and match phases take
/// their own indexes under `state` and `phase`.
fn compact_name(key: &str, name: &str) -> Option<u64> {
    match (key, name) {
        ("team", "Red") | ("caliber", "Mm17") | ("kind", "Infantry") => Some(0),
        ("team", "Blue") | ("caliber", "Mm42") | ("kind", "Hero") => Some(1),
        ("kind" | "stage", "Small") => Some(2),
        ("kind" | "stage", "Big") => Some(3),
        ("state", "Inactive") => Some(4),
        ("state", "Activating") => Some(5),
        ("state", "Activated") => Some(6),
        ("phase", "Idle") => Some(7),
        ("phase", "Countdown") => Some(8),
        ("phase", "Running") => Some(9),
        ("phase", "Finished") => Some(10),
        _ => None,
    }
}
/// Inverse of [`compact_name`]: only the indexes the encoder writes map back,
/// everything else passes through to the typed deserializer.
fn expand_name(key: &str, index: u64) -> Option<&'static str> {
    match (key, index) {
        ("team", 0) => Some("Red"),
        ("team", 1) => Some("Blue"),
        ("caliber", 0) => Some("Mm17"),
        ("caliber", 1) => Some("Mm42"),
        ("kind", 0) => Some("Infantry"),
        ("kind", 1) => Some("Hero"),
        ("kind" | "stage", 2) => Some("Small"),
        ("kind" | "stage", 3) => Some("Big"),
        ("state", 4) => Some("Inactive"),
        ("state", 5) => Some("Activating"),
        ("state", 6) => Some("Activated"),
        ("phase", 7) => Some("Idle"),
        ("phase", 8) => Some("Countdown"),
        ("phase", 9) => Some("Running"),
        ("phase", 10) => Some("Finished"),
        _ => None,
    }
}
/// Team and rune-kind arrays (referee config) carry the enums as bare array
/// elements, so the parent key scopes them the same way.
fn compact_element(key: &str, name: &str) -> Option<u64> {
    match (key, name) {
        ("outpost_teams" | "rune_teams", "Red") => Some(0),
        ("outpost_teams" | "rune_teams", "Blue") => Some(1),
        ("training_kinds", "Small") => Some(2),
        ("training_kinds", "Big") => Some(3),
        _ => None,
    }
}
/// Inverse of [`compact_element`].
fn expand_element(key: &str, index: u64) -> Option<&'static str> {
    match (key, index) {
        ("outpost_teams" | "rune_teams", 0) => Some("Red"),
        ("outpost_teams" | "rune_teams", 1) => Some("Blue"),
        ("training_kinds", 2) => Some("Small"),
        ("training_kinds", 3) => Some("Big"),
        _ => None,
    }
}
fn compact_child(array_key: Option<&str>, value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                if let Value::String(name) = &*field
                    && let Some(index) = compact_name(key, name)
                {
                    *field = Value::from(index);
                } else {
                    compact_child(Some(key), field);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                if let Value::String(name) = &*item
                    && let Some(key) = array_key
                    && let Some(index) = compact_element(key, name)
                {
                    *item = Value::from(index);
                } else {
                    compact_child(None, item);
                }
            }
        }
        _ => {}
    }
}
/// Map the compact checkpoint's rule enums to small indexes before bitpacking
/// (teams, calibers, robot/rune kinds, rune stages/states and match phases).
/// The packed checkpoint codec and the dictionary trainer share this so
/// training corpora match live bytes. Unknown strings pass through, so a new
/// variant costs bytes but still decodes; [`expand_checkpoint_enums`]
/// reverses the mapping on decode.
pub fn compact_checkpoint_enums(value: &mut Value) {
    compact_child(None, value);
}
fn expand_child(array_key: Option<&str>, value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                if let Value::Number(index) = &*field
                    && let Some(index) = index.as_u64()
                    && let Some(name) = expand_name(key, index)
                {
                    *field = Value::from(name);
                } else {
                    expand_child(Some(key), field);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                if let Value::Number(index) = &*item
                    && let Some(index) = index.as_u64()
                    && let Some(key) = array_key
                    && let Some(name) = expand_element(key, index)
                {
                    *item = Value::from(name);
                } else {
                    expand_child(None, item);
                }
            }
        }
        _ => {}
    }
}
/// Inverse of [`compact_checkpoint_enums`]: only the indexes the encoder
/// writes map back, everything else passes through to the typed deserializer.
pub fn expand_checkpoint_enums(value: &mut Value) {
    expand_child(None, value);
}
impl PlayerSnapshot {
    fn from_state(state: &SimulationState) -> Self {
        let mut state = state.clone();
        let time_ns = state.field.time_ns;
        let projectiles = std::mem::take(&mut state.field.projectiles)
            .into_iter()
            .map(|ball| {
                let launched_age_ns = time_ns.saturating_sub(ball.launched_ns);
                let first_contact_age_ns = ball
                    .first_contact_ns
                    .map(|contact_ns| contact_ns.saturating_sub(ball.launched_ns));
                let dwell_base_ns = ball.first_contact_ns.unwrap_or(ball.launched_ns);
                let dwell_age_ns = ball
                    .dwell_since_ns
                    .map(|dwell_ns| dwell_ns.saturating_sub(dwell_base_ns));
                ProjectileWire(
                    ball.id,
                    CaliberBit::from(ball.caliber),
                    launched_age_ns,
                    ball.position_m.map(|x| x as f32),
                    ball.velocity_m_s.map(|x| x as f32),
                    ball.shooter,
                    first_contact_age_ns,
                    dwell_age_ns,
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
        let time_ns = self.state.field.time_ns;
        self.state.field.projectiles = self
            .projectiles
            .into_iter()
            .map(
                |ProjectileWire(
                    id,
                    caliber_bit,
                    launched_age_ns,
                    position,
                    velocity,
                    shooter,
                    first_contact_age_ns,
                    dwell_age_ns,
                )| {
                    let launched_ns = time_ns.saturating_sub(launched_age_ns);
                    let first_contact_ns =
                        first_contact_age_ns.map(|age_ns| launched_ns.saturating_add(age_ns));
                    let dwell_base_ns = first_contact_ns.unwrap_or(launched_ns);
                    let dwell_since_ns =
                        dwell_age_ns.map(|age_ns| dwell_base_ns.saturating_add(age_ns));
                    rm_simulator_world::ProjectileSnapshot {
                        id,
                        caliber: rm_simulator_world::Caliber::from(caliber_bit),
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
            let mut envelope = serde_json::to_value(&PlayerEnvelope {
                snapshot: PlayerSnapshot::from_state(state),
            })
            .expect("finite protocol message");
            compact_checkpoint_enums(&mut envelope);
            serde_json::to_vec(&envelope)
        }
        _ => serde_json::to_vec(message),
    }
    .expect("finite protocol message")
}
/// Decodes either a compact checkpoint or an ordinary message. Errors on
/// malformed bytes rather than returning a partial state, because a decoder
/// that drops rule state gives up restoring, not drawing.
pub fn decode_player_message(bytes: &[u8]) -> io::Result<ServerMessage> {
    let mut value: Value =
        serde_json::from_slice(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if value.get("CompactSnapshot").is_some() {
        expand_checkpoint_enums(&mut value);
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
    fn compacted_enums_round_trip_and_leave_other_values_alone() {
        let mut value = serde_json::json!({
            "team": "Blue",
            "caliber": "Mm42",
            "kind": "Small",
            "stage": "Big",
            "state": "Activating",
            "phase": "Countdown",
            "paused": true,
            "hp": 10, // small ints under other keys stay numbers
            "reason": "Blue", // display text under another key stays a string
            "future": {"team": "Green"}, // unknown variants stay strings
            "outpost_teams": ["Red", "Blue"],
            "training_kinds": ["Small"],
            "chassis": [{"team": "Red", "kind": "Hero"}],
        });
        let original = value.clone();
        compact_checkpoint_enums(&mut value);
        assert_eq!(value["team"], serde_json::json!(1));
        assert_eq!(value["caliber"], serde_json::json!(1));
        assert_eq!(value["kind"], serde_json::json!(2));
        assert_eq!(value["stage"], serde_json::json!(3));
        assert_eq!(value["state"], serde_json::json!(5));
        assert_eq!(value["phase"], serde_json::json!(8));
        assert_eq!(value["paused"], serde_json::json!(true));
        assert_eq!(value["hp"], serde_json::json!(10));
        assert_eq!(value["reason"], serde_json::json!("Blue"));
        assert_eq!(value["future"]["team"], serde_json::json!("Green"));
        assert_eq!(value["outpost_teams"], serde_json::json!([0, 1]));
        assert_eq!(value["training_kinds"], serde_json::json!([2]));
        expand_checkpoint_enums(&mut value);
        assert_eq!(value, original);
    }

    #[test]
    fn projectile_timestamp_ages_reconstruct_exactly() {
        use rm_simulator_world::{Caliber, ProjectileSnapshot};
        fn ball(
            id: u64,
            caliber: Caliber,
            launched_ns: u64,
            first_contact_ns: Option<u64>,
            dwell_since_ns: Option<u64>,
        ) -> ProjectileSnapshot {
            ProjectileSnapshot {
                id,
                caliber,
                launched_ns,
                position_m: [1., 2., 3.],
                velocity_m_s: [25., 0., 0.],
                angular_velocity_rad_s: [0.; 3],
                shooter: Some(7),
                first_contact_ns,
                dwell_since_ns,
            }
        }
        let mut state = simulation();
        state.field.time_ns = 90_000_000_000;
        state.field.projectiles = vec![
            ball(1, Caliber::Mm17, 89_000_000_000, None, None),
            ball(2, Caliber::Mm42, 88_000_000_000, Some(88_500_000_000), None),
            ball(
                3,
                Caliber::Mm17,
                87_000_000_000,
                Some(87_100_000_000),
                Some(87_150_000_000),
            ),
        ];
        let message = ServerMessage::Snapshot(Box::new(state.clone()));
        let ServerMessage::Snapshot(actual) =
            decode_player_message(&encode_player_message(&message)).unwrap()
        else {
            panic!("a snapshot must decode to a snapshot");
        };
        assert_eq!(actual.field.projectiles, state.field.projectiles);
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
