// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! UDP deltas reference decoded, pinned baselines; never the last transmitted frame.
//! The decoder pins at most two baselines, so a new one may only be proposed once
//! the previous one is retired. `Retire` is reliable but its `Retired` answer is
//! not, so an unanswered retirement is resent on a bounded schedule rather than
//! abandoned: dropping it locally would propose a third baseline the decoder must
//! refuse.
use crate::binary_snapshot::bitpack::{self, Node};
use crate::protocol::ServerMessage;
use crate::simulation::SimulationState;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};
/// First four bytes of every baseline or owner-configuration feedback payload,
/// so a sender can tell feedback from snapshot traffic on the same lane.
pub const ACK_MAGIC: &[u8; 4] = b"RMA2";
/// First four bytes of every [`Wire::Retire`] request.
pub const RETIRE_MAGIC: &[u8; 4] = b"RMR1";
const BASE_LIMIT: usize = 1024 * 1024;
/// One application frame on the delta lane, always tagged by the state epoch.
#[derive(Clone, Debug)]
pub enum Wire {
    /// Packed full or delta checkpoint. Header fields are inspected before
    /// looking up a baseline; the entire body is validated before delivery.
    Packed {
        /// Input generation; older generations cannot repopulate the cache.
        epoch: u64,
        /// Zero for independent frames, otherwise the proposed or referenced id.
        id: u64,
        /// True when the frame requires the named pinned baseline.
        delta: bool,
        /// Complete inflated RMB1 frame, including the checked header.
        bytes: Vec<u8>,
    },
    /// Asks the decoder to drop baseline `id` so the encoder can propose a new
    /// one. The decoder pins at most two baselines, so retirement is what keeps
    /// the rotation moving.
    Retire {
        /// Input epoch the baseline belongs to.
        epoch: u64,
        /// Id of the baseline to drop.
        id: u64,
    },
}
/// The decoder's answer about one baseline, or the owner codec's answer about
/// one owner configuration. `Stored` and `Retired` confirm a state transition;
/// `Missing` tells the encoder a delta referenced a baseline the decoder never
/// pinned, so the next frame must be independent again; `ConfigStored` and
/// `ConfigMissing` run the owner-configuration acknowledgement handshake.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Feedback {
    /// The candidate baseline was pinned under `id`.
    Stored {
        /// Input epoch the baseline belongs to.
        epoch: u64,
        /// Id of the pinned baseline.
        id: u64,
    },
    /// Baseline `id` is gone and will not be referenced again.
    Retired {
        /// Input epoch the baseline belongs to.
        epoch: u64,
        /// Id of the dropped baseline.
        id: u64,
    },
    /// A delta referenced baseline `id`, which was not pinned.
    Missing {
        /// Input epoch the delta claimed.
        epoch: u64,
        /// Id of the baseline that was needed.
        id: u64,
    },
    /// The client stored the owner configuration named by `revision`, so the
    /// host may now reference it from an anchor.
    ConfigStored {
        /// Wire value of the acknowledged configuration revision.
        revision: u64,
    },
    /// An anchor named a configuration `revision` the client does not hold. The
    /// host should resend it; the client dropped that anchor rather than
    /// applying a reference it could not decode.
    ConfigMissing {
        /// Wire value of the configuration revision the client needs.
        revision: u64,
    },
}
impl Feedback {
    /// Encodes the feedback as [`ACK_MAGIC`] followed by its binary form, which keeps it
    /// distinguishable from a snapshot frame on the same unreliable lane.
    ///
    /// ```
    /// use rm_simulator_server::udp_snapshot::{ACK_MAGIC, Feedback};
    ///
    /// let stored = Feedback::Stored { epoch: 4, id: 2 };
    /// let bytes = stored.encode();
    /// assert!(bytes.starts_with(ACK_MAGIC));
    /// assert_eq!(Feedback::decode(&bytes).unwrap(), stored);
    /// // A payload that is not baseline feedback is refused, not guessed at.
    /// assert!(Feedback::decode(b"RMG1nonsense").is_err());
    /// ```
    pub fn encode(self) -> Vec<u8> {
        let mut bytes = ACK_MAGIC.to_vec();
        bytes.extend(bitpack::to_bytes(&self).expect("feedback is positional"));
        bytes
    }
    /// Decodes [`Feedback::encode`] output. Errors on a missing magic prefix or
    /// a payload longer than 256 bytes.
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 256 || !bytes.starts_with(ACK_MAGIC) {
            return Err(io::Error::other("invalid baseline feedback"));
        }
        bitpack::from_bytes(&bytes[4..])
    }
}
/// The uncompressed bytes for one [`Wire`] frame: a packed checkpoint is its
/// own frame and a retirement is [`RETIRE_MAGIC`] with the epoch and id.
/// [`encode`] compresses exactly these bytes, so the dictionary trainer sees
/// the framing the wire does instead of re-deriving it.
pub fn envelope_bytes(wire: &Wire) -> Vec<u8> {
    match wire {
        Wire::Packed { bytes, .. } => bytes.clone(),
        Wire::Retire { epoch, id } => {
            let mut bytes = RETIRE_MAGIC.to_vec();
            bytes.extend(bitpack::to_bytes(&(epoch, id)).expect("retire is positional"));
            bytes
        }
    }
}
/// Frames a [`Wire`] and compresses it. Only the reliable [`Wire::Retire`]
/// request travels this way; checkpoints carry their own packed framing.
/// `raw = true` writes the envelope behind [`crate::compression::RAW_MAGIC`] instead of ZSTD, which
/// is what the loopback transport uses so a local session runs the same frames
/// without paying for compression.
pub fn encode_with(raw: bool, wire: Wire) -> Vec<u8> {
    crate::compression::encode(raw, &envelope_bytes(&wire))
}
/// Reads an inflated packed checkpoint or a `Retire` request. `Ok(None)` means
/// the frame is neither, so a caller can leave other message kinds on the same
/// stream alone instead of failing on them.
///
/// ```
/// use rm_simulator_server::udp_snapshot::{Wire, encode_with, parse};
/// use rm_simulator_server::compression::decompress;
///
/// let frame = decompress(&encode_with(true, Wire::Retire { epoch: 7, id: 2 }), 64).unwrap();
/// let wire = parse(&frame).unwrap().unwrap();
/// assert!(matches!(wire, Wire::Retire { epoch: 7, id: 2 }));
/// // A message of another kind is not an error here.
/// assert!(parse(b"RMM1").unwrap().is_none());
/// ```
pub fn parse(bytes: &[u8]) -> io::Result<Option<Wire>> {
    if bytes.starts_with(bitpack::MAGIC) {
        let (delta, epoch, id) = bitpack::header(bytes)?;
        return Ok(Some(Wire::Packed {
            epoch,
            id,
            delta,
            bytes: bytes.to_vec(),
        }));
    }
    if let Some(body) = bytes.strip_prefix(RETIRE_MAGIC) {
        let (epoch, id) = bitpack::from_bytes::<(u64, u64)>(body)?;
        return Ok(Some(Wire::Retire { epoch, id }));
    }
    Ok(None)
}
/// Server-side delta encoder for one peer.
///
/// A new baseline is proposed only while no proposal and no retirement are
/// outstanding, so the decoder never has to pin a third baseline. Every frame
/// also carries an independent encoding of the same state, and the encoder falls
/// back to it whenever the delta would be larger, which keeps a single lost
/// baseline from stalling delivery. A delta is coded against its baseline
/// dead-reckoned to the frame's tick
/// ([`crate::snapshot_codec::predict_baseline`]), so the decoder predicts the
/// same tree from its pinned copy before reading the body. States are packed as fine fixed point and
/// compressed with the embedded checkpoint dictionary; in raw mode the packed
/// frame is emitted unchanged, so a loopback peer runs the same baseline
/// rotation without the dictionary.
pub struct Encoder {
    epoch: Option<u64>,
    next_id: u64,
    active: Option<(u64, Node)>,
    pending: Option<(u64, Node)>,
    retiring: Option<(u64, Node)>,
    /// Frames since the outstanding `Retire` was last sent.
    retire_age: u64,
    count: u64,
    recover: bool,
    /// Emit packed frames unchanged instead of dictionary-compressing them.
    raw: bool,
    compressor: crate::binary_snapshot::Compressor,
    /// The active baseline dead-reckoned to the latest frame.
    prediction: crate::snapshot_codec::Prediction,
    /// Bytes the same states would have taken as independent frames.
    pub full_bytes: u64,
    /// Bytes actually produced, deltas and full frames together.
    pub sent_bytes: u64,
    /// Frames emitted as a packed delta.
    pub deltas: u64,
    /// Uncompressed size of the latest independent packed checkpoint.
    pub last_packed_bytes: usize,
}
/// The packed fine fixed-point encoder with its trained dictionary, which is the
/// network default.
impl Default for Encoder {
    fn default() -> Self {
        Self::with_raw(false)
    }
}
/// Resend an unanswered `Retire` after this many frames; at the 32 ms broadcast
/// period that is roughly half a second between attempts.
const RETIRE_RESEND_FRAMES: u64 = 16;
/// Encoded frames before a new baseline may be proposed. Application choice,
/// not a rule constant: with dead-reckoned baselines (protocol 42) the
/// remote-cadence probe measured 12 smallest over 8/12/16/24/32 once the
/// extra `Retire`, `Stored` and `Retired` traffic is counted, and the scripted
/// lossy links put 8, 12 and 16 within 0.1% of each other.
const ROTATION_FRAMES: u64 = 12;
impl Encoder {
    /// The packed fine fixed-point encoder that emits its `RMB1` frames
    /// unchanged, for the loopback transport.
    pub fn new_raw() -> Self {
        Self::with_raw(true)
    }
    /// The packed fine fixed-point encoder with or without dictionary
    /// compression.
    fn with_raw(raw: bool) -> Self {
        Self {
            epoch: None,
            next_id: 0,
            active: None,
            pending: None,
            retiring: None,
            retire_age: 0,
            count: 0,
            recover: false,
            raw,
            compressor: crate::binary_snapshot::Compressor::new(),
            prediction: Default::default(),
            full_bytes: 0,
            sent_bytes: 0,
            deltas: 0,
            last_packed_bytes: 0,
        }
    }
    /// Packed frames the checkpoint compressor returned uncompressed because
    /// dictionary compression would not have shrunk them. Both framings stay
    /// decodable, so this only measures the gate, never a fallback path.
    pub fn raw_fallbacks(&self) -> u64 {
        self.compressor.raw_fallbacks()
    }
    /// Deltas below
    /// [`crate::binary_snapshot::DELTA_COMPRESSION_MIN_BYTES`] sent packed
    /// without a compression attempt, because none that small shrinks.
    pub fn skipped_deltas(&self) -> u64 {
        self.compressor.skipped_deltas()
    }
    /// The `Retire` to resend, if its `Retired` answer has not arrived in time.
    /// Callers send it on the same reliable lane as the original.
    pub fn resend_retire(&mut self) -> Option<Wire> {
        let epoch = self.epoch?;
        let (id, _) = self.retiring.as_ref()?;
        if self.retire_age < RETIRE_RESEND_FRAMES {
            return None;
        }
        self.retire_age = 0;
        Some(Wire::Retire { epoch, id: *id })
    }
    /// Encodes one frame from a host state's independent player checkpoint, framed for the
    /// wire. `epoch` selects the state generation: a change discards every
    /// baseline and restarts the rotation. Binary traversal and frame-size
    /// violations return an error before transmission.
    ///
    /// ```
    /// use rm_simulator_server::compression::decompress;
    /// use rm_simulator_server::simulation::Simulation;
    /// use rm_simulator_server::udp_snapshot::{Decoder, Encoder, Wire, parse};
    /// use rm_simulator_world::{Field, FieldConfig};
    ///
    /// let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
    /// let mut state = simulation.state();
    /// // The decoder rejects a frame whose state epoch does not match the wire.
    /// state.input_epoch = 4;
    /// let inflate = |bytes: &[u8]| {
    ///     parse(&decompress(bytes, 4 << 20).unwrap()).unwrap().unwrap()
    /// };
    /// let mut encoder = Encoder::default();
    /// let mut decoder = Decoder::default();
    /// // The first frame proposes a baseline, since none is pinned yet.
    /// let first = inflate(&encoder.snapshot(4, &state).unwrap());
    /// assert!(matches!(first, Wire::Packed { id: 1, delta: false, .. }));
    /// let (message, feedback) = decoder.receive(first).unwrap();
    /// assert!(message.is_some());
    /// assert_eq!(decoder.pinned(), 1);
    /// // The answer promotes the candidate, so the next frame is a delta.
    /// assert!(encoder.feedback(feedback.unwrap()).is_none());
    /// let second = inflate(&encoder.snapshot(4, &state).unwrap());
    /// assert!(matches!(second, Wire::Packed { id: 1, delta: true, .. }));
    /// assert!(encoder.deltas > 0);
    /// ```
    pub fn snapshot(&mut self, epoch: u64, state: &SimulationState) -> io::Result<Vec<u8>> {
        let tick = state.field.tick;
        let state = crate::snapshot_codec::checkpoint_node(state)?;
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.active = None;
            self.pending = None;
            self.retiring = None;
            self.retire_age = 0;
            self.count = 0;
            self.recover = false;
        }
        self.count += 1;
        if self.retiring.is_some() {
            self.retire_age += 1;
        }
        let packed = bitpack::encode(&state, None, epoch, 0)?;
        self.last_packed_bytes = packed.len();
        let oversize = packed.len() > BASE_LIMIT;
        let independent = self.frame(packed);
        self.full_bytes += independent.len() as u64;
        if oversize {
            self.sent_bytes += independent.len() as u64;
            return Ok(independent);
        }
        if self.pending.is_none()
            && self.retiring.is_none()
            && (self.active.is_none() || self.count >= ROTATION_FRAMES)
        {
            self.next_id = self
                .next_id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("baseline id exhausted"))?;
            self.pending = Some((self.next_id, state.clone()));
            self.count = 0;
        }
        let encoded = if self.recover
            && let Some((id, baseline)) = self.active.as_ref()
        {
            self.recover = false;
            let packed = bitpack::encode(baseline, None, epoch, *id)?;
            self.frame(packed)
        } else if let Some((id, baseline)) = &self.pending
            && self.count.is_multiple_of(8)
        {
            let packed = bitpack::encode(baseline, None, epoch, *id)?;
            self.frame(packed)
        } else if let Some((id, baseline)) = &self.active {
            let packed = crate::snapshot_codec::encode_checkpoint(
                &state,
                tick,
                Some((baseline, *id)),
                epoch,
                &mut self.prediction,
            )?;
            let delta = if self.raw {
                packed
            } else {
                self.compressor.compress_delta(&packed)
            };
            if delta.len() < independent.len() {
                self.deltas += 1;
                delta
            } else {
                independent
            }
        } else {
            independent
        };
        self.sent_bytes += encoded.len() as u64;
        Ok(encoded)
    }
    /// A packed frame as it goes on the wire: unchanged for a raw encoder,
    /// otherwise through the checkpoint dictionary's no-bloat gate.
    fn frame(&mut self, packed: Vec<u8>) -> Vec<u8> {
        if self.raw {
            packed
        } else {
            self.compressor.compress(&packed)
        }
    }
    /// Applies one decoder answer. Returns a [`Wire::Retire`] to send when the
    /// stored baseline replaced an older active one, which is the only moment a
    /// baseline may be retired. Answers for other epochs or other ids are
    /// ignored, so a delayed duplicate cannot disturb the current rotation.
    pub fn feedback(&mut self, feedback: Feedback) -> Option<Wire> {
        match feedback {
            Feedback::Stored { epoch, id }
                if Some(epoch) == self.epoch
                    && self.pending.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                self.retiring = self.active.take();
                self.active = self.pending.take();
                self.retire_age = 0;
                self.count = 0;
                self.retiring
                    .as_ref()
                    .map(|(id, _)| Wire::Retire { epoch, id: *id })
            }
            Feedback::Retired { epoch, id }
                if Some(epoch) == self.epoch
                    && self.retiring.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                self.retiring = None;
                self.retire_age = 0;
                None
            }
            Feedback::Missing { epoch, id }
                if Some(epoch) == self.epoch
                    && self.active.as_ref().is_some_and(|(p, _)| *p == id) =>
            {
                self.recover = true;
                None
            }
            _ => None,
        }
    }
}
/// Client-side delta decoder for one connection.
///
/// Pins at most two baselines, the active one and the candidate replacing it.
/// A third full frame is refused rather than evicting a baseline a delta might
/// still reference, which is why the encoder retires before proposing.
#[derive(Default)]
pub struct Decoder {
    epoch: Option<u64>,
    baselines: BTreeMap<u64, Node>,
    retired_through: u64,
    /// The referenced baseline dead-reckoned to the latest delta.
    prediction: crate::snapshot_codec::Prediction,
    /// Deltas refused because their baseline was not pinned.
    pub missing: u64,
    /// Deltas applied to a pinned baseline.
    pub deltas: u64,
}
impl Decoder {
    /// Baselines held for future deltas. The wire contract caps this at two:
    /// a third can only be proposed once the encoder retires one.
    pub fn pinned(&self) -> usize {
        self.baselines.len()
    }
    /// Applies one frame, returning the decoded message when it produced one and
    /// the feedback the encoder needs. A delta for an unpinned baseline yields
    /// `Feedback::Missing` and no message rather than a partially applied state.
    pub fn receive(&mut self, wire: Wire) -> io::Result<(Option<ServerMessage>, Option<Feedback>)> {
        let epoch = match &wire {
            Wire::Packed { epoch, .. } | Wire::Retire { epoch, .. } => *epoch,
        };
        if self.epoch.is_some_and(|old| epoch < old) {
            return Ok((None, None));
        }
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.baselines.clear();
            self.retired_through = 0;
        }
        match wire {
            Wire::Retire { id, .. } => {
                self.baselines.retain(|base, _| *base > id);
                self.retired_through = self.retired_through.max(id);
                Ok((None, Some(Feedback::Retired { epoch, id })))
            }
            Wire::Packed {
                id, delta, bytes, ..
            } => {
                // The public Wire can also be constructed directly; don't trust
                // its inspected header in place of validating the actual bytes.
                if crate::binary_snapshot::bitpack::header(&bytes)? != (delta, epoch, id) {
                    return Err(io::Error::other("binary snapshot header mismatch"));
                }
                // A full frame at or below the retired watermark can neither
                // repopulate the cache nor be applied.
                if !delta && id > 0 && id <= self.retired_through {
                    return Ok((None, None));
                }
                let base = if delta {
                    let Some(base) = self.baselines.get(&id) else {
                        self.missing += 1;
                        return Ok((None, Some(Feedback::Missing { epoch, id })));
                    };
                    Some((base, id))
                } else {
                    None
                };
                let pin = !delta && id > 0;
                if pin {
                    if self.baselines.len() >= 2 && !self.baselines.contains_key(&id) {
                        return Err(io::Error::other("baseline cache overflow"));
                    }
                    if bytes.len() > BASE_LIMIT {
                        return Err(io::Error::other("baseline exceeds byte cap"));
                    }
                }
                // Cache wire-grid values; only the delivered state is normalized,
                // or later deltas would use a different baseline.
                let (message, node) = crate::snapshot_codec::decode_checkpoint_with(
                    &bytes,
                    base,
                    epoch,
                    pin,
                    &mut self.prediction,
                )?;
                if let Some(node) = node {
                    if self.baselines.get(&id).is_some_and(|old| old != &node) {
                        return Err(io::Error::other("baseline identity changed"));
                    }
                    self.baselines.insert(id, node);
                    return Ok((Some(message), Some(Feedback::Stored { epoch, id })));
                }
                if delta {
                    self.deltas += 1;
                }
                Ok((Some(message), None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::Simulation;
    use rm_simulator_world::{Field, FieldConfig};

    #[test]
    fn binary_chassis_and_projectiles_round_trip_across_baselines_and_epochs() {
        use crate::{layout::ChassisSpawner, protocol::Command};
        use rm_simulator_world::{ChassisCommand, ChassisConfig, Team};
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false)
            .with_spawner(ChassisSpawner {
                config: ChassisConfig::default(),
                terrain: None,
            });
        let chassis = simulation.spawn_chassis(Team::Red).unwrap();
        simulation
            .apply(&Command::Chassis {
                chassis,
                command: ChassisCommand {
                    forward_m_s: 1.23456789,
                    yaw_rate_rad_s: 0.31234567,
                    ..Default::default()
                },
            })
            .unwrap();
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let mut old = None;
        for frame in 0..160 {
            simulation.step(32).unwrap();
            if frame % 8 == 0 {
                simulation
                    .apply(&Command::Fire { shooter: chassis })
                    .unwrap();
            }
            let mut state = simulation.state();
            let epoch = frame / 64;
            state.snapshot_id = frame + 1;
            state.input_epoch = epoch;
            // The expected delivery is the quantized checkpoint decoded on its
            // own, which also leaves the hidden restore state exact.
            let independent = bitpack::encode(
                &crate::snapshot_codec::checkpoint_node(&state).unwrap(),
                None,
                epoch,
                0,
            )
            .unwrap();
            let (expected, _) =
                crate::snapshot_codec::decode_checkpoint(&independent, None, epoch, false).unwrap();
            let ServerMessage::Snapshot(quantized) = &expected else {
                unreachable!()
            };
            assert_eq!(quantized.field.restore, state.field.restore);
            let source = state;
            let bytes = encoder.snapshot(epoch, &source).unwrap();
            // The no-bloat gate may return a small delta as a bare packed
            // frame instead of dictionary-compressing it; both framings
            // decode through the same path the `wire` helper exercises.
            assert!(
                bytes.starts_with(crate::binary_snapshot::MAGIC)
                    || bytes.starts_with(crate::binary_snapshot::bitpack::MAGIC)
            );
            let (actual, feedback) = decoder.receive(wire(&bytes)).unwrap();
            assert_eq!(actual.as_ref(), Some(&expected));
            let ServerMessage::Snapshot(actual) = actual.unwrap() else {
                unreachable!()
            };
            Field::restore(
                &actual.field,
                &simulation.field().static_geometry_snapshot(),
                0.,
            )
            .unwrap()
            .step(32)
            .unwrap();
            if let Some(feedback) = feedback
                && let Some(retire) = encoder.feedback(feedback)
            {
                let (_, feedback) = decoder.receive(retire).unwrap();
                encoder.feedback(feedback.unwrap());
            }
            assert!(decoder.pinned() <= 2);
            if frame == 63 {
                old = Some(bytes);
            }
        }
        assert!(decoder.receive(wire(&old.unwrap())).unwrap().0.is_none());
        assert!(encoder.deltas > 100);
    }

    /// Dead-reckoned deltas deliver exactly the independent decode while
    /// chassis join and leave, projectiles spawn and retire, the tick lead
    /// varies and the epoch changes under a pinned baseline.
    #[test]
    fn predicted_deltas_survive_joins_leaves_projectiles_and_epochs() {
        use crate::{layout::ChassisSpawner, protocol::Command};
        use rm_simulator_world::{ChassisCommand, ChassisConfig, Team};
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false)
            .with_spawner(ChassisSpawner {
                config: ChassisConfig::default(),
                terrain: None,
            });
        let shooter = simulation.spawn_chassis(Team::Red).unwrap();
        let drive = ChassisCommand {
            forward_m_s: 1.5,
            yaw_rate_rad_s: 0.7,
            aim_yaw_rad: 0.3,
            ..Default::default()
        };
        simulation
            .apply(&Command::Chassis {
                chassis: shooter,
                command: drive,
            })
            .unwrap();
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let mut guest = None;
        for frame in 0..240_u64 {
            simulation.step(1 + frame % 5).unwrap();
            match frame % 60 {
                10 => guest = Some(simulation.spawn_chassis(Team::Blue).unwrap()),
                40 => simulation.remove_chassis(guest.take().unwrap()).unwrap(),
                _ => {}
            }
            if frame % 8 == 0 {
                simulation.apply(&Command::Fire { shooter }).unwrap();
            }
            let mut state = simulation.state();
            let epoch = frame / 100;
            state.snapshot_id = frame + 1;
            state.input_epoch = epoch;
            let independent = bitpack::encode(
                &crate::snapshot_codec::checkpoint_node(&state).unwrap(),
                None,
                epoch,
                0,
            )
            .unwrap();
            let (expected, _) =
                crate::snapshot_codec::decode_checkpoint(&independent, None, epoch, false).unwrap();
            let bytes = encoder.snapshot(epoch, &state).unwrap();
            let (actual, feedback) = decoder.receive(wire(&bytes)).unwrap();
            assert_eq!(actual.as_ref(), Some(&expected), "frame {frame}");
            if let Some(feedback) = feedback
                && let Some(retire) = encoder.feedback(feedback)
            {
                let (_, feedback) = decoder.receive(retire).unwrap();
                encoder.feedback(feedback.unwrap());
            }
            assert!(decoder.pinned() <= 2);
        }
        assert!(decoder.deltas > 150, "{} deltas", decoder.deltas);
    }

    #[test]
    fn binary_snapshot_propagates_traversal_errors() {
        let mut encoder = Encoder::default();
        let valid = source(0, 0);
        let mut state = valid.clone();
        state.bots = vec![0; 100_000];
        let error = encoder.snapshot(0, &state).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(encoder.sent_bytes, 0);
        assert!(encoder.pending.is_none());
        assert!(encoder.snapshot(0, &valid).is_ok());
    }

    #[test]
    fn malformed_binary_rotations_are_rejected_before_pinning_a_baseline() {
        let mut state = source(0, 0);
        let mut config = FieldConfig::default();
        config.chassis.push(rm_simulator_world::ChassisPlacement {
            team: rm_simulator_world::Team::Red,
            kind: rm_simulator_world::RobotKind::Infantry,
            performance: None,
            config: Default::default(),
            spawn: rm_simulator_world::Pose::at([0., 0., 0.2]),
        });
        state.field.chassis = Field::new(&config).unwrap().snapshot().chassis;
        state.field.chassis[0].pose.rotation_wxyz = [0.; 4];
        let node = crate::snapshot_codec::checkpoint_node(&state).unwrap();
        let bytes = bitpack::encode(&node, None, 0, 1).unwrap();
        let mut decoder = Decoder::default();
        assert!(decoder.receive(parse(&bytes).unwrap().unwrap()).is_err());
        assert_eq!(decoder.pinned(), 0);
    }
    fn source(tick: u64, epoch: u64) -> SimulationState {
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
        simulation.step(tick).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = tick + 1;
        state.input_epoch = epoch;
        state
    }
    fn wire(bytes: &[u8]) -> Wire {
        parse(&crate::compression::decompress(bytes, 4 << 20).unwrap())
            .unwrap()
            .unwrap()
    }
    fn decoded(message: Option<ServerMessage>) -> crate::simulation::SimulationState {
        let Some(ServerMessage::Snapshot(state)) = message else {
            panic!("missing snapshot")
        };
        *state
    }
    #[test]
    fn missing_ack_still_delivers_current_independent_state() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let mut newest = 0;
        for tick in 0..30 {
            let bytes = encoder.snapshot(0, &source(tick, 0)).unwrap();
            let (message, _) = decoder.receive(wire(&bytes)).unwrap(); // Drop every application ACK.
            newest = newest.max(decoded(message).field.tick);
        }
        assert_eq!(newest, 29);
        assert_eq!(decoder.baselines.len(), 1);
        assert_eq!(encoder.deltas, 0);
    }
    #[test]
    fn deltas_survive_loss_reordering_and_duplicates_without_chaining() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let (_, ack) = decoder
            .receive(wire(&encoder.snapshot(0, &source(0, 0)).unwrap()))
            .unwrap();
        encoder.feedback(ack.unwrap());
        let lost = encoder.snapshot(0, &source(1, 0)).unwrap();
        let next = encoder.snapshot(0, &source(2, 0)).unwrap();
        for bytes in [&next, &next, &lost] {
            let actual = decoded(decoder.receive(wire(bytes)).unwrap().0);
            let expected = decoded(Some(
                crate::snapshot_codec::decode_player_message(
                    &crate::snapshot_codec::encode_player_message(&ServerMessage::Snapshot(
                        Box::new(source(actual.field.tick, 0)),
                    )),
                )
                .unwrap(),
            ));
            assert_eq!(actual, expected);
        }
        assert_eq!(decoder.baselines.len(), 1);
        assert!(encoder.deltas > 0);
    }
    #[test]
    fn retirement_pins_two_baselines_until_ack_and_old_full_cannot_resurrect() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let old = encoder.snapshot(0, &source(0, 0)).unwrap();
        let (_, ack) = decoder.receive(wire(&old)).unwrap();
        encoder.feedback(ack.unwrap());
        let mut retirement = None;
        for tick in 1..40 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                retirement = Some(retire);
                break;
            }
        }
        assert!(retirement.is_some());
        assert_eq!(decoder.baselines.len(), 2);
        for tick in 50..100 {
            decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            assert!(encoder.pending.is_none());
        }
        let (_, ack) = decoder.receive(retirement.unwrap()).unwrap();
        assert_eq!(decoder.baselines.len(), 1);
        assert!(decoder.receive(wire(&old)).unwrap().0.is_none());
        encoder.feedback(ack.unwrap());
        assert!(encoder.retiring.is_none());
        decoder
            .receive(wire(&encoder.snapshot(0, &source(101, 0)).unwrap()))
            .unwrap();
        assert!(encoder.pending.is_some());
    }
    #[test]
    fn a_lost_retired_answer_is_resent_and_rotation_resumes() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let (_, ack) = decoder
            .receive(wire(&encoder.snapshot(0, &source(0, 0)).unwrap()))
            .unwrap();
        encoder.feedback(ack.unwrap());
        let mut retirement = None;
        for tick in 1..40 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                retirement = Some(retire);
                break;
            }
        }
        // The decoder retires the old baseline but its answer never arrives.
        let (_, retired) = decoder.receive(retirement.unwrap()).unwrap();
        assert!(matches!(retired, Some(Feedback::Retired { .. })));
        assert_eq!(decoder.baselines.len(), 1);
        // Without a retry the encoder would stay pinned for the connection's life.
        let mut resent = None;
        for tick in 100..100 + RETIRE_RESEND_FRAMES + 1 {
            decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            assert!(
                encoder.pending.is_none(),
                "rotation is blocked until retired"
            );
            if let Some(retire) = encoder.resend_retire() {
                resent = Some(retire);
                break;
            }
        }
        let (_, retired) = decoder.receive(resent.expect("retire resent")).unwrap();
        encoder.feedback(retired.unwrap());
        assert!(encoder.retiring.is_none());
        // A fresh baseline can be proposed and stored again.
        let mut rotated = false;
        for tick in 200..260 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack {
                encoder.feedback(ack);
                rotated = true;
                break;
            }
        }
        assert!(rotated, "a new baseline must be storable after retirement");
        assert!(decoder.baselines.len() <= 2);
    }
    #[test]
    fn missing_baseline_recovers_and_epoch_change_rejects_old_traffic() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        let first = encoder.snapshot(1, &source(0, 1)).unwrap();
        let (_, ack) = decoder.receive(wire(&first)).unwrap();
        encoder.feedback(ack.unwrap());
        decoder.baselines.clear();
        let delta = encoder.snapshot(1, &source(2, 1)).unwrap();
        let (message, missing) = decoder.receive(wire(&delta)).unwrap();
        assert!(message.is_none());
        encoder.feedback(missing.unwrap());
        let (_, ack) = decoder
            .receive(wire(&encoder.snapshot(1, &source(3, 1)).unwrap()))
            .unwrap();
        assert!(ack.is_some());
        let new_epoch = encoder.snapshot(2, &source(4, 2)).unwrap();
        decoder.receive(wire(&new_epoch)).unwrap();
        assert!(decoder.receive(wire(&delta)).unwrap().0.is_none());
        assert!(decoder.receive(wire(&first)).unwrap().0.is_none());
        assert_eq!(decoder.baselines.len(), 1);
    }
    #[test]
    fn raw_encoder_emits_packed_frames_and_keeps_the_baseline_rotation() {
        let mut encoder = Encoder::new_raw();
        let mut decoder = Decoder::default();
        for tick in 0..160 {
            let bytes = encoder.snapshot(0, &source(tick, 0)).unwrap();
            assert!(
                bytes.starts_with(crate::binary_snapshot::bitpack::MAGIC),
                "a raw checkpoint is the packed frame itself"
            );
            assert!(!bytes.starts_with(crate::binary_snapshot::MAGIC));
            let (_, ack) = decoder.receive(wire(&bytes)).unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                let (_, ack) = decoder.receive(retire).unwrap();
                encoder.feedback(ack.unwrap());
            }
            assert!(decoder.baselines.len() <= 2);
        }
        assert!(
            encoder.deltas > 100,
            "the raw rotation still selects deltas"
        );
        // The reliable Retire envelope uses the same raw framing.
        let retire = encode_with(true, Wire::Retire { epoch: 0, id: 1 });
        assert!(retire.starts_with(crate::compression::RAW_MAGIC));
        assert!(matches!(
            parse(&crate::compression::decompress(&retire, 1 << 20).unwrap())
                .unwrap()
                .unwrap(),
            Wire::Retire { epoch: 0, id: 1 }
        ));
    }

    #[test]
    fn sustained_updates_save_encoded_bytes_and_caches_stay_bounded() {
        let mut encoder = Encoder::default();
        let mut decoder = Decoder::default();
        for tick in 0..160 {
            let (_, ack) = decoder
                .receive(wire(&encoder.snapshot(0, &source(tick, 0)).unwrap()))
                .unwrap();
            if let Some(ack) = ack
                && let Some(retire) = encoder.feedback(ack)
            {
                let (_, ack) = decoder.receive(retire).unwrap();
                encoder.feedback(ack.unwrap());
            }
            assert!(decoder.baselines.len() <= 2);
        }
        eprintln!(
            "UDP independent/delta encoded bytes: {}/{} ({} deltas)",
            encoder.full_bytes, encoder.sent_bytes, encoder.deltas
        );
        // The dictionary makes an independent frame small enough to win the
        // size guard often, so the saving is real but not twofold on every
        // frame. Both halves of the scheme must still be exercised.
        assert!(encoder.sent_bytes < encoder.full_bytes);
        assert!(encoder.deltas > 100);
    }
}
