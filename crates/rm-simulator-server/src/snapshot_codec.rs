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
/// Independent UDP player checkpoint. Physics restore values remain f64;
/// ball position and velocity carry `f32` precision, well below 0.1 mm on this
/// field, unless a value does not survive `f32`. No earlier packet or
/// acknowledgement is needed to decode it.
///
/// Nothing a decoder can derive travels. The field clock is `tick` times
/// `tick_ns()`; the rune, outpost and referee views are rebuilt from the rule
/// state in the restore (see [`RulesWire`]); wheel hubs and tyre targets are
/// rebuilt from the pose, configuration and command (see [`ChassisWire`]).
/// Every absolute timestamp rides as a [`StampCodes`] tick code, with any
/// sub-tick remainders listed once in `sub_tick_ns`, in visit order.
///
/// Field names never reach the wire; they select the fixed-point rounding in
/// [`fixed_point::Fine`].
#[derive(Serialize, Deserialize)]
struct PlayerSnapshot {
    state: StateWire,
    projectiles: Vec<ProjectileWire>,
    /// Nonzero remainders of the stamps whose code marks one, in the order
    /// [`PlayerSnapshot::for_each_stamp`] visits them.
    sub_tick_ns: Vec<u32>,
}
/// [`SimulationState`] with its field in wire form; shot results carry their
/// execution time as a stamp code.
#[derive(Serialize, Deserialize)]
struct StateWire {
    bots: Vec<u32>,
    snapshot_id: u64,
    input_epoch: u64,
    shot_results: Vec<crate::protocol::ShotResult>,
    paused: bool,
    field: FieldWire,
}
/// [`rm_simulator_world::FieldSnapshot`] without its clock, derived views or
/// projectiles (hoisted into [`ProjectileWire`]). Hit times are stamp codes.
#[derive(Serialize, Deserialize)]
struct FieldWire {
    bases: Vec<rm_simulator_world::BaseSnapshot>,
    tick: u64,
    chassis: Vec<ChassisWire>,
    hits: Vec<rm_simulator_world::ArmorHit>,
    shots_fired: u64,
    hits_detected: u64,
    rules: RulesWire,
}
/// Where the rule state and its views travel.
#[derive(Serialize, Deserialize)]
enum RulesWire {
    /// The usual case, one gamma bit: the field clock is `tick * tick_ns()`
    /// and every rune, outpost and referee view equals what the restore's own
    /// `snapshot` gives, so only the restore travels, with its stamps as tick
    /// codes. A Big Rune is the `Rune::Big` variant with its whole state:
    /// motion amplitude and frequency and epoch angle as exact `f64`, its
    /// epoch, stage, state, first-hit and current times as stamp codes, lit
    /// pair, hits, completed groups, restart policy and seeded stream.
    Derived(Box<rm_simulator_world::FieldRestore>),
    /// Anything else, such as a hand-built state with no restore or a view
    /// edited away from its rules: every part travels as it is, absolute
    /// times included, and only the rune target poses and outpost armor poses
    /// are rebuilt from their angles.
    Explicit(Box<ExplicitRules>),
}
/// The [`RulesWire::Explicit`] fallback: the views and the optional restore
/// exactly as the state holds them.
#[derive(Serialize, Deserialize)]
struct ExplicitRules {
    time_ns: u64,
    runes: Vec<rm_simulator_world::RuneSnapshot>,
    outposts: Vec<rm_simulator_world::OutpostSnapshot>,
    referee: Option<rm_simulator_world::RefereeSnapshot>,
    restore: Option<rm_simulator_world::FieldRestore>,
}
/// [`rm_simulator_world::ChassisSnapshot`] with each wheel reduced to its spin.
/// Contacts are diagnostics; hubs and tyre targets are rebuilt with
/// [`rm_simulator_world::ChassisSnapshot::derive_wheel_kinematics`] after the
/// pose is normalized.
#[derive(Serialize, Deserialize)]
struct ChassisWire {
    placement_revision: u64,
    id: u32,
    team: rm_simulator_world::Team,
    config: rm_simulator_world::ChassisConfig,
    pose: rm_simulator_world::Pose,
    turret: rm_simulator_world::Pose,
    velocity_m_s: [f64; 3],
    angular_velocity_rad_s: [f64; 3],
    command: rm_simulator_world::ChassisCommand,
    held_aim_rad: [f64; 2],
    gimbal_velocity_rad_s: [f64; 2],
    wheel_spin_rad: Vec<f64>,
    defeated: bool,
}
/// Stable identity and shooter, caliber, a position and velocity vector, then
/// the retirement bookkeeping. Spin is not carried: it moves no ball a client
/// can see. The launch, first-contact and dwell times are stamp codes, so a
/// predicting client retires the same ball on the same tick as the host does.
#[derive(Serialize, Deserialize)]
struct ProjectileWire {
    id: u64,
    caliber: CaliberBit,
    launched_ns: u64,
    position_m: [f64; 3],
    velocity_m_s: [f64; 3],
    shooter: Option<u32>,
    first_contact_ns: Option<u64>,
    dwell_since_ns: Option<u64>,
}

/// Rewrites absolute nanosecond timestamps as small tick codes and back.
///
/// Code 0 is the clock's current time, so a value that tracks the clock
/// (a rune's or the referee's own time, the economy's round time) costs one
/// byte and never changes between frames. Any other stamp `t` splits into
/// whole ticks `t / tick_ns()` and a remainder; the code is
/// `1 + 2 * ticks + (remainder != 0)`, and a nonzero remainder is appended to
/// the checkpoint's sub-tick list. Rule stamps land on the 128 Hz grid except
/// where the rules add a nanosecond (a hit window "exactly at 2.5 s is valid")
/// or a referee skip moves the round clock by an arbitrary amount, so the
/// remainder keeps every value exact. Whole ticks since the clock's zero are
/// used rather than ages against the checkpoint tick: an unchanged stamp then
/// has the same code in every frame and costs one bit in a delta.
struct StampCodes<'a> {
    now_ns: u64,
    sub_tick_ns: &'a mut Vec<u32>,
    /// Next remainder to read; decoding only.
    next: usize,
    error: bool,
}
impl<'a> StampCodes<'a> {
    fn new(now_ns: u64, sub_tick_ns: &'a mut Vec<u32>) -> Self {
        Self {
            now_ns,
            sub_tick_ns,
            next: 0,
            error: false,
        }
    }
    fn encode(&mut self, stamp: &mut u64) {
        let tick = rm_simulator_world::tick_ns();
        if *stamp == self.now_ns {
            *stamp = 0;
            return;
        }
        let remainder = *stamp % tick;
        if remainder != 0 {
            self.sub_tick_ns
                .push(u32::try_from(remainder).expect("a tick is shorter than 4 s"));
        }
        *stamp = 1 + 2 * (*stamp / tick) + u64::from(remainder != 0);
    }
    fn decode(&mut self, stamp: &mut u64) {
        if *stamp == 0 {
            *stamp = self.now_ns;
            return;
        }
        let code = *stamp - 1;
        let remainder = if code & 1 == 1 {
            let Some(&remainder) = self.sub_tick_ns.get(self.next) else {
                self.error = true;
                return;
            };
            self.next += 1;
            u64::from(remainder)
        } else {
            0
        };
        match (code >> 1)
            .checked_mul(rm_simulator_world::tick_ns())
            .and_then(|ns| ns.checked_add(remainder))
        {
            Some(ns) => *stamp = ns,
            None => self.error = true,
        }
    }
}

impl PlayerSnapshot {
    /// Visit every field-clock stamp in the checkpoint: shot results, hits,
    /// projectiles, then a derived restore's runes, outposts, referee and
    /// detection times. The referee's round-clock stamps follow separately.
    /// The visit order is the wire order of `sub_tick_ns`.
    fn for_each_field_stamp(&mut self, visit: &mut dyn FnMut(&mut u64)) {
        for result in &mut self.state.shot_results {
            if let Some(time) = &mut result.executed_time_ns {
                visit(time);
            }
        }
        for hit in &mut self.state.field.hits {
            visit(&mut hit.time_ns);
        }
        for ball in &mut self.projectiles {
            visit(&mut ball.launched_ns);
            if let Some(time) = &mut ball.first_contact_ns {
                visit(time);
            }
            if let Some(time) = &mut ball.dwell_since_ns {
                visit(time);
            }
        }
        if let RulesWire::Derived(restore) = &mut self.state.field.rules {
            for rune in &mut restore.runes {
                rune.for_each_stamp_mut(visit);
            }
            for outpost in &mut restore.outposts {
                outpost.for_each_stamp_mut(visit);
            }
            if let Some(referee) = &mut restore.referee {
                referee.for_each_stamp_mut(rm_simulator_world::StampClock::Field, visit);
            }
            for (_, time) in &mut restore.last_detection_ns {
                visit(time);
            }
        }
    }
    fn referee_mut(&mut self) -> Option<&mut rm_simulator_world::Referee> {
        match &mut self.state.field.rules {
            RulesWire::Derived(restore) => restore.referee.as_mut(),
            RulesWire::Explicit(_) => None,
        }
    }

    fn from_state(state: &SimulationState) -> Self {
        let field = &state.field;
        let now_ns = field.tick.checked_mul(rm_simulator_world::tick_ns());
        let rules = match &field.restore {
            Some(restore)
                if now_ns == Some(field.time_ns)
                    && field.runes.len() == restore.runes.len()
                    && field
                        .runes
                        .iter()
                        .zip(&restore.runes)
                        .all(|(view, rune)| *view == rune.snapshot())
                    && field.outposts.len() == restore.outposts.len()
                    && field
                        .outposts
                        .iter()
                        .zip(&restore.outposts)
                        .all(|(view, outpost)| *view == outpost.snapshot(field.time_ns))
                    && field.referee == restore.referee.as_ref().map(|r| r.snapshot()) =>
            {
                RulesWire::Derived(Box::new(restore.clone()))
            }
            _ => {
                let mut runes = field.runes.clone();
                for rune in &mut runes {
                    rune.target_poses = [rm_simulator_world::Pose::at([0.; 3]); 5];
                }
                let mut outposts = field.outposts.clone();
                for outpost in &mut outposts {
                    outpost.armors.clear();
                }
                RulesWire::Explicit(Box::new(ExplicitRules {
                    time_ns: field.time_ns,
                    runes,
                    outposts,
                    referee: field.referee.clone(),
                    restore: field.restore.clone(),
                }))
            }
        };
        let chassis = field
            .chassis
            .iter()
            .map(|chassis| ChassisWire {
                placement_revision: chassis.placement_revision,
                id: chassis.id,
                team: chassis.team,
                config: chassis.config.clone(),
                pose: chassis.pose,
                turret: chassis.turret,
                velocity_m_s: chassis.velocity_m_s,
                angular_velocity_rad_s: chassis.angular_velocity_rad_s,
                command: chassis.command,
                held_aim_rad: chassis.held_aim_rad,
                gimbal_velocity_rad_s: chassis.gimbal_velocity_rad_s,
                wheel_spin_rad: chassis.wheels.iter().map(|wheel| wheel.spin_rad).collect(),
                defeated: chassis.defeated,
            })
            .collect();
        let projectiles = field
            .projectiles
            .iter()
            .map(|ball| ProjectileWire {
                id: ball.id,
                caliber: CaliberBit::from(ball.caliber),
                launched_ns: ball.launched_ns,
                position_m: ball.position_m.map(single_precision),
                velocity_m_s: ball.velocity_m_s.map(single_precision),
                shooter: ball.shooter,
                first_contact_ns: ball.first_contact_ns,
                dwell_since_ns: ball.dwell_since_ns,
            })
            .collect();
        let mut snapshot = Self {
            state: StateWire {
                bots: state.bots.clone(),
                snapshot_id: state.snapshot_id,
                input_epoch: state.input_epoch,
                shot_results: state.shot_results.clone(),
                paused: state.paused,
                field: FieldWire {
                    bases: field.bases.clone(),
                    tick: field.tick,
                    chassis,
                    // Failed-contact diagnostics only fed the removed impact
                    // markers. Keep genuine registered hits, damage and rune
                    // outcomes for armor feedback.
                    hits: field
                        .hits
                        .iter()
                        .filter(|hit| hit.detected)
                        .cloned()
                        .collect(),
                    shots_fired: field.shots_fired,
                    hits_detected: field.hits_detected,
                    rules,
                },
            },
            projectiles,
            sub_tick_ns: Vec::new(),
        };
        // An `Explicit` field keeps its clock in `time_ns`; the stamps outside
        // the rules still code against it.
        let mut sub_tick_ns = Vec::new();
        let round_now_ns = snapshot.referee_mut().map(|r| r.match_time_ns());
        let mut codes = StampCodes::new(field.time_ns, &mut sub_tick_ns);
        snapshot.for_each_field_stamp(&mut |stamp| codes.encode(stamp));
        if let (Some(round_now_ns), Some(referee)) = (round_now_ns, snapshot.referee_mut()) {
            let mut codes = StampCodes::new(round_now_ns, &mut sub_tick_ns);
            referee.for_each_stamp_mut(rm_simulator_world::StampClock::Round, &mut |stamp| {
                codes.encode(stamp)
            });
        }
        snapshot.sub_tick_ns = sub_tick_ns;
        snapshot
    }

    /// The simulation state this checkpoint carries. With `normalize`,
    /// quantized chassis rotations are normalized first (see
    /// [`fixed_point::normalize`]), so the rebuilt wheel hubs follow the pose
    /// physics will use.
    fn into_state(mut self, normalize: bool) -> io::Result<SimulationState> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
        let mut sub_tick_ns = std::mem::take(&mut self.sub_tick_ns);
        let time_ns = match &self.state.field.rules {
            RulesWire::Derived(_) => self
                .state
                .field
                .tick
                .checked_mul(rm_simulator_world::tick_ns())
                .ok_or_else(|| invalid("checkpoint tick overflows the clock"))?,
            RulesWire::Explicit(rules) => rules.time_ns,
        };
        let mut codes = StampCodes::new(time_ns, &mut sub_tick_ns);
        self.for_each_field_stamp(&mut |stamp| codes.decode(stamp));
        let (mut next, mut error) = (codes.next, codes.error);
        if let Some(referee) = self.referee_mut() {
            let mut codes = StampCodes::new(referee.match_time_ns(), &mut sub_tick_ns);
            codes.next = next;
            referee.for_each_stamp_mut(rm_simulator_world::StampClock::Round, &mut |stamp| {
                codes.decode(stamp)
            });
            (next, error) = (codes.next, error || codes.error);
        }
        if error || next != sub_tick_ns.len() {
            return Err(invalid(
                "checkpoint timestamps do not match their remainders",
            ));
        }
        let PlayerSnapshot {
            state, projectiles, ..
        } = self;
        let field = state.field;
        let (runes, outposts, referee, restore) = match field.rules {
            RulesWire::Derived(restore) => (
                restore
                    .runes
                    .iter()
                    .map(rm_simulator_world::Rune::snapshot)
                    .collect(),
                restore
                    .outposts
                    .iter()
                    .map(|outpost| outpost.snapshot(time_ns))
                    .collect(),
                restore
                    .referee
                    .as_ref()
                    .map(rm_simulator_world::Referee::snapshot),
                Some(*restore),
            ),
            RulesWire::Explicit(rules) => {
                let ExplicitRules {
                    mut runes,
                    mut outposts,
                    referee,
                    restore,
                    ..
                } = *rules;
                for rune in &mut runes {
                    rune.target_poses = rm_simulator_world::rune::target_poses(
                        rune.hub_pose,
                        rune.target_radius_m,
                        rune.angle_rad,
                    );
                }
                for outpost in &mut outposts {
                    outpost.rebuild_armors();
                }
                (runes, outposts, referee, restore)
            }
        };
        let mut chassis = Vec::with_capacity(field.chassis.len());
        for wire in field.chassis {
            if wire.wheel_spin_rad.len() != wire.config.wheel_hubs_m.len() {
                return Err(invalid("checkpoint wheel count does not match the chassis"));
            }
            let mut snapshot = rm_simulator_world::ChassisSnapshot {
                placement_revision: wire.placement_revision,
                id: wire.id,
                team: wire.team,
                config: wire.config,
                pose: wire.pose,
                turret: wire.turret,
                velocity_m_s: wire.velocity_m_s,
                angular_velocity_rad_s: wire.angular_velocity_rad_s,
                command: wire.command,
                held_aim_rad: wire.held_aim_rad,
                gimbal_velocity_rad_s: wire.gimbal_velocity_rad_s,
                wheels: wire
                    .wheel_spin_rad
                    .into_iter()
                    .map(|spin_rad| rm_simulator_world::WheelSnapshot {
                        spin_rad,
                        ..Default::default()
                    })
                    .collect(),
                defeated: wire.defeated,
            };
            if normalize {
                for pose in [&mut snapshot.pose, &mut snapshot.turret] {
                    fixed_point::normalize_pose(pose)?;
                }
            }
            snapshot.derive_wheel_kinematics();
            chassis.push(snapshot);
        }
        let projectiles = projectiles
            .into_iter()
            .map(|ball| rm_simulator_world::ProjectileSnapshot {
                id: ball.id,
                caliber: rm_simulator_world::Caliber::from(ball.caliber),
                launched_ns: ball.launched_ns,
                position_m: ball.position_m,
                velocity_m_s: ball.velocity_m_s,
                angular_velocity_rad_s: [0.; 3],
                shooter: ball.shooter,
                first_contact_ns: ball.first_contact_ns,
                dwell_since_ns: ball.dwell_since_ns,
            })
            .collect();
        Ok(SimulationState {
            bots: state.bots,
            snapshot_id: state.snapshot_id,
            input_epoch: state.input_epoch,
            shot_results: state.shot_results,
            paused: state.paused,
            field: rm_simulator_world::FieldSnapshot {
                bases: field.bases,
                tick: field.tick,
                time_ns,
                runes,
                outposts,
                projectiles,
                chassis,
                hits: field.hits,
                shots_fired: field.shots_fired,
                hits_detected: field.hits_detected,
                referee,
                restore,
            },
        })
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
        Wire::Snapshot(snapshot) => ServerMessage::Snapshot(Box::new(snapshot.into_state(false)?)),
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
    let (snapshot, _) =
        bitpack::decode_with::<PlayerSnapshot, _>(bytes, baseline, epoch, fixed_point::Fine::Root)?;
    let node = keep_node
        .then(|| bitpack::to_node_with(&snapshot, fixed_point::Fine::Root))
        .transpose()?;
    if snapshot.state.input_epoch != epoch {
        return Err(io::Error::other("invalid baseline state/epoch"));
    }
    let message = ServerMessage::Snapshot(Box::new(snapshot.into_state(true)?));
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
    /// Steps `field` through 40 published frames, checking each checkpoint
    /// decodes exactly through the independent codec and through the
    /// acknowledged delta lane, then restores the last decoded checkpoint and
    /// replays `replay_ticks` against the host.
    fn assert_rules_round_trip(field: &mut Field, replay_ticks: u64) {
        use crate::udp_snapshot::{Decoder, Encoder, parse};
        let geometry = field.static_geometry_snapshot();
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let mut last = None;
        for frame in 0..40 {
            field.step(1).unwrap();
            let mut state = simulation();
            state.snapshot_id = frame + 1;
            state.field = field.snapshot();
            assert!(
                matches!(
                    PlayerSnapshot::from_state(&state).state.field.rules,
                    RulesWire::Derived(_)
                ),
                "a field's own snapshot must travel as its restore alone"
            );
            let message = ServerMessage::Snapshot(Box::new(state.clone()));
            assert_eq!(
                decode_player_message(&encode_player_message(&message)).unwrap(),
                message
            );
            let bytes = encoder.snapshot(0, &state).unwrap();
            let wire = parse(&crate::compression::decompress(&bytes, 4 << 20).unwrap())
                .unwrap()
                .unwrap();
            let (received, feedback) = decoder.receive(wire).unwrap();
            let Some(ServerMessage::Snapshot(received)) = received else {
                panic!("every frame must deliver a snapshot");
            };
            assert_eq!(received.field, state.field);
            if let Some(feedback) = feedback
                && let Some(retire) = encoder.feedback(feedback)
                && let (_, Some(retired)) = decoder.receive(retire).unwrap()
            {
                encoder.feedback(retired);
            }
            last = Some(received.field);
        }
        assert!(
            encoder.deltas > 0,
            "the delta lane must have been exercised"
        );
        let mut restored = Field::restore(&last.unwrap(), &geometry, 0.).unwrap();
        field.step(replay_ticks).unwrap();
        restored.step(replay_ticks).unwrap();
        let (host, replay) = (field.snapshot(), restored.snapshot());
        assert_eq!(replay.runes, host.runes);
        assert_eq!(replay.referee, host.referee);
        assert_eq!(replay.restore, host.restore);
    }

    /// Hits the first lit blade of rune `index` at the field's current time.
    fn hit_lit_blade(field: &mut Field, index: usize) {
        use rm_simulator_world::HitOutcome;
        let now = field.time_ns();
        let blade = field.snapshot().runes[index]
            .active_blades
            .iter()
            .position(|lit| *lit)
            .expect("an activating Big Rune lights a pair") as u32;
        let outcome = field.rune_mut(index).unwrap().hit(now, blade).unwrap();
        assert!(matches!(outcome, HitOutcome::GroupHit { bonus: false, .. }));
    }

    #[test]
    fn a_big_rune_with_a_pending_bonus_window_restores_and_predicts_exactly() {
        use rm_simulator_world::{RuneKind, RuneState};
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        field.step(64).unwrap();
        let now = field.time_ns();
        let rune = field.rune_mut(0).unwrap();
        rune.set_seed(0x5eed);
        rune.convert(RuneKind::Big, now).unwrap();
        // A seeded activation draws its sinusoid, so the motion is not the
        // default and must travel exactly.
        rune.activate(now).unwrap();
        // Idle through one 2.5 s window: the reset lands the stage start one
        // nanosecond off the tick grid.
        field.step(330).unwrap();
        hit_lit_blade(&mut field, 0);
        let state = SimulationState {
            field: field.snapshot(),
            ..simulation()
        };
        let rune = &state.field.runes[0];
        assert_eq!(
            (rune.kind, rune.state),
            (RuneKind::Big, RuneState::Activating)
        );
        assert!(rune.motion.is_some());
        assert_eq!(rune.completed_groups, 0);
        assert!(rune.activated.contains(&true));
        assert!(
            !PlayerSnapshot::from_state(&state).sub_tick_ns.is_empty(),
            "the off-grid stage start must ride as a remainder"
        );
        // 40 frames stay inside the one-second second-hit window; the replay
        // then crosses the group's completion and later window resets.
        assert_rules_round_trip(&mut field, 700);
    }

    #[test]
    fn big_runes_after_the_three_minute_stage_change_restore_and_predict_exactly() {
        use rm_simulator_world::{
            RefereeCommand, RefereeConfig, RuneKind, RuneStage, RuneState, Team,
        };
        let mut config = FieldConfig {
            referee: Some(RefereeConfig::alternating(2, 2)),
            ..Default::default()
        };
        config.runes.push(config.runes[0]);
        let mut field = Field::new(&config).unwrap();
        field.referee_command(RefereeCommand::StartMatch).unwrap();
        field.step(641).unwrap();
        // An off-grid skip puts every round-clock stamp off the tick grid.
        field
            .referee_command(RefereeCommand::SkipTo {
                match_time_ns: 180_000_000_123,
            })
            .unwrap();
        field.step(2).unwrap();
        field
            .referee_command(RefereeCommand::ActivateRune { team: Team::Red })
            .unwrap();
        field.step(64).unwrap();
        let snapshot = field.snapshot();
        assert_eq!(snapshot.referee.as_ref().unwrap().stage, RuneStage::Big);
        assert!(snapshot.runes.iter().all(|rune| rune.kind == RuneKind::Big));
        let index = snapshot
            .runes
            .iter()
            .position(|rune| rune.state == RuneState::Activating)
            .expect("red's rune is activating");
        hit_lit_blade(&mut field, index);
        assert_rules_round_trip(&mut field, 700);
    }

    #[test]
    fn stamp_codes_are_exact_and_refuse_missing_remainders() {
        let tick = rm_simulator_world::tick_ns();
        let now_ns = 1_000 * tick;
        let stamps = [
            0,
            now_ns,
            now_ns + 1,
            7 * tick,
            7 * tick + tick - 1,
            u64::MAX,
        ];
        let mut sub_tick_ns = Vec::new();
        let mut codes = stamps;
        let mut encoder = StampCodes::new(now_ns, &mut sub_tick_ns);
        codes.iter_mut().for_each(|stamp| encoder.encode(stamp));
        assert_eq!(codes[1], 0, "the current time is code zero");
        assert_eq!(codes[3], 15, "an on-grid stamp is one plus twice its tick");
        assert_eq!(sub_tick_ns.len(), 3);
        let mut decoded = codes;
        let mut decoder = StampCodes::new(now_ns, &mut sub_tick_ns);
        decoded.iter_mut().for_each(|stamp| decoder.decode(stamp));
        assert!(!decoder.error && decoder.next == 3);
        assert_eq!(decoded, stamps);
        let mut none = Vec::new();
        let mut decoder = StampCodes::new(now_ns, &mut none);
        decoder.decode(&mut codes[2].clone());
        assert!(decoder.error);
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
