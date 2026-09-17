// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Binary message encodings. Every server message a peer receives and every
//! client message a host receives is a magic-tagged value in the positional
//! [`bitpack`] format; a snapshot travels as the compact independent player
//! checkpoint. No encoding here depends on an earlier transmitted frame; the
//! periodic delta lane in [`crate::udp_snapshot`] builds on
//! [`checkpoint_node`] and [`decode_checkpoint`].
use crate::binary_snapshot::{bitpack, bitpack::Node, fixed_point};
use crate::protocol::{ClientMessage, ServerMessage};
use crate::simulation::SimulationState;
use serde::{Deserialize, Serialize};
use std::io;

/// First four bytes of every encoded [`ServerMessage`].
pub const SERVER_MAGIC: &[u8; 4] = b"RMM1";
/// First four bytes of every encoded [`ClientMessage`].
pub const CLIENT_MAGIC: &[u8; 4] = b"RMQ1";

/// Independent UDP player checkpoint. Physics restore values remain f64;
/// ball position and velocity carry `f32` precision, well below 0.1 mm on this
/// field, unless a value does not survive `f32`. No earlier packet or
/// acknowledgement is needed to decode it.
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
///
/// Field names never reach the wire; they select the fixed-point rounding in
/// [`fixed_point::Fine`].
#[derive(Serialize, Deserialize)]
struct ProjectileWire {
    id: u64,
    caliber: CaliberBit,
    launched_age_ns: u64,
    position_m: [f64; 3],
    velocity_m_s: [f64; 3],
    shooter: Option<u32>,
    first_contact_age_ns: Option<u64>,
    dwell_age_ns: Option<u64>,
}

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
                ProjectileWire {
                    id: ball.id,
                    caliber: CaliberBit::from(ball.caliber),
                    launched_age_ns,
                    position_m: ball.position_m.map(single_precision),
                    velocity_m_s: ball.velocity_m_s.map(single_precision),
                    shooter: ball.shooter,
                    first_contact_age_ns,
                    dwell_age_ns,
                }
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
                |ProjectileWire {
                     id,
                     caliber,
                     launched_age_ns,
                     position_m,
                     velocity_m_s,
                     shooter,
                     first_contact_age_ns,
                     dwell_age_ns,
                 }| {
                    let launched_ns = time_ns.saturating_sub(launched_age_ns);
                    let first_contact_ns =
                        first_contact_age_ns.map(|age_ns| launched_ns.saturating_add(age_ns));
                    let dwell_base_ns = first_contact_ns.unwrap_or(launched_ns);
                    let dwell_since_ns =
                        dwell_age_ns.map(|age_ns| dwell_base_ns.saturating_add(age_ns));
                    rm_simulator_world::ProjectileSnapshot {
                        id,
                        caliber: rm_simulator_world::Caliber::from(caliber),
                        launched_ns,
                        position_m,
                        velocity_m_s,
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
/// `x` at `f32` precision when `f32` holds it finitely, otherwise exact, so an
/// extreme diagnostic coordinate is never turned into infinity.
fn single_precision(x: f64) -> f64 {
    let single = x as f32;
    if single.is_finite() {
        f64::from(single)
    } else {
        x
    }
}
/// Server message wire form; the snapshot arm carries its compact view.
#[derive(Serialize)]
enum WireRef<'a> {
    Snapshot(Box<PlayerSnapshot>),
    Message(&'a ServerMessage),
}
/// Decoded [`WireRef`]; boxing keeps the arms comparable in size.
#[derive(Deserialize)]
enum Wire {
    Snapshot(Box<PlayerSnapshot>),
    Message(ServerMessage),
}

/// Encodes a server message for a peer: a snapshot becomes an independent
/// compact checkpoint that needs no earlier frame, and every other message its
/// positional binary form, both behind [`SERVER_MAGIC`].
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
    let wire = match message {
        ServerMessage::Snapshot(state) => {
            WireRef::Snapshot(Box::new(PlayerSnapshot::from_state(state)))
        }
        other => WireRef::Message(other),
    };
    let mut bytes = SERVER_MAGIC.to_vec();
    bytes.extend(bitpack::to_bytes(&wire).expect("protocol messages are positional"));
    bytes
}
/// Decodes [`encode_player_message`] output. Errors on malformed bytes rather
/// than returning a partial state, because a decoder that drops rule state
/// gives up restoring, not drawing.
pub fn decode_player_message(bytes: &[u8]) -> io::Result<ServerMessage> {
    let body = bytes
        .strip_prefix(SERVER_MAGIC)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a server message"))?;
    Ok(match bitpack::from_bytes::<Wire>(body)? {
        Wire::Snapshot(snapshot) => ServerMessage::Snapshot(Box::new(snapshot.into_state())),
        Wire::Message(message) => message,
    })
}

/// Encodes a client message behind [`CLIENT_MAGIC`].
///
/// ```
/// use rm_simulator_server::protocol::ClientMessage;
/// use rm_simulator_server::snapshot_codec::{decode_client_message, encode_client_message};
///
/// let ping = ClientMessage::Ping { nonce: 7 };
/// let bytes = encode_client_message(&ping);
/// assert_eq!(bytes.len(), 6);
/// assert_eq!(decode_client_message(&bytes).unwrap(), ping);
/// ```
pub fn encode_client_message(message: &ClientMessage) -> Vec<u8> {
    let mut bytes = CLIENT_MAGIC.to_vec();
    bytes.extend(bitpack::to_bytes(message).expect("protocol messages are positional"));
    bytes
}
/// Decodes [`encode_client_message`] output.
pub fn decode_client_message(bytes: &[u8]) -> io::Result<ClientMessage> {
    let body = bytes
        .strip_prefix(CLIENT_MAGIC)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a client message"))?;
    bitpack::from_bytes(body)
}

/// The fine fixed-point checkpoint tree for `state`: the tree the periodic
/// delta lane packs and pins as a baseline.
pub fn checkpoint_node(state: &SimulationState) -> io::Result<Node> {
    Ok(bitpack::to_node_with(
        &PlayerSnapshot::from_state(state),
        fixed_point::Fine::Root,
    )?)
}

/// Decodes one packed checkpoint frame against its pinned baseline, checks its
/// input epoch and normalizes its quantized rotations. With `keep_node` the
/// decoded wire-grid tree comes back too, for pinning as a baseline; the
/// delivered state is normalized, the tree is not, so later deltas use the
/// exact values the encoder holds.
pub fn decode_checkpoint(
    bytes: &[u8],
    baseline: Option<(&Node, u64)>,
    epoch: u64,
    keep_node: bool,
) -> io::Result<(ServerMessage, Option<Node>)> {
    let (snapshot, _) = bitpack::decode::<PlayerSnapshot>(bytes, baseline, epoch)?;
    let node = keep_node.then(|| bitpack::to_node(&snapshot)).transpose()?;
    if snapshot.state.input_epoch != epoch {
        return Err(io::Error::other("invalid baseline state/epoch"));
    }
    let mut message = ServerMessage::Snapshot(Box::new(snapshot.into_state()));
    fixed_point::normalize(&mut message)?;
    Ok((message, node))
}

#[cfg(test)]
mod tests {
    #[test]
    fn outpost_motion_extraction_preserves_legacy_checkpoint_shape() {
        // The JSON shape is the world crate's saved form; the binary form below
        // must decode the same outpost positionally.
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
        let bytes = crate::binary_snapshot::bitpack::to_bytes(&outpost).unwrap();
        assert_eq!(
            crate::binary_snapshot::bitpack::from_bytes::<Outpost>(&bytes).unwrap(),
            outpost
        );
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
    fn equivalent(a: &serde_json::Value, b: &serde_json::Value) {
        use serde_json::Value;
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
    fn extreme_diagnostic_coordinates_keep_full_precision() {
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
        let message = ServerMessage::Snapshot(Box::new(state.clone()));
        let decoded = decode_player_message(&encode_player_message(&message)).unwrap();
        // Spin is never carried; the out-of-range coordinate stays exact.
        state.field.projectiles[0].angular_velocity_rad_s = [0.; 3];
        assert_eq!(decoded, ServerMessage::Snapshot(Box::new(state)));
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
            // The JSON form is only a size reference here.
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
        // The periodic lane's quantized checkpoint, not the f32 confirmation.
        let bytes = crate::binary_snapshot::bitpack::encode(
            &checkpoint_node(&state).unwrap(),
            None,
            state.input_epoch,
            0,
        )
        .unwrap();
        let (ServerMessage::Snapshot(decoded), _) =
            decode_checkpoint(&bytes, None, state.input_epoch, false).unwrap()
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
