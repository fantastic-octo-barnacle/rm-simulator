// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Offline stage-1 section delivery experiment, enabled by `section-topics`.
//!
//! Splits the existing player representation without further quantization. Every
//! capture sends all nine sections at the caller's publication cadence, using
//! the existing acknowledged-baseline algorithm separately for each section.
//! A manifest names exact value revisions captured together. Only complete
//! checkpoints leave the assembler; it never combines independently newest values.
//!
//! This is not a negotiated live wire protocol. [`delivery`] adds the stage-2
//! pacing, aggregation and repair experiment. Early presentation is still deferred.
//! Reliable confirmations and Pongs continue through the unmodified live codec.
//! All compressed experiment payloads use [`crate::compression::selected`], just
//! like the whole-snapshot control. Decoding detects either codec and retains
//! the existing decompressed byte limits; DEFLATE remains the default.
//!
//! ```
//! use rm_simulator_server::{section_topics::{Decoder, Encoder}, simulation::Simulation};
//! use rm_simulator_world::{Field, FieldConfig};
//!
//! let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false);
//! let mut state = simulation.state();
//! state.snapshot_id = 1;
//! let mut sender = Encoder::default();
//! let mut receiver = Decoder::default();
//! let frames = sender.capture(&state).unwrap();
//! let mut complete = None;
//! // Sections may arrive before their manifest.
//! for frame in frames.iter().rev() {
//!     let (snapshot, _feedback) = receiver.receive(frame).unwrap();
//!     complete = snapshot.or(complete);
//! }
//! assert_eq!(complete.unwrap().field.tick, state.field.tick);
//! ```
pub mod delivery;

use crate::{protocol::ServerMessage, simulation::SimulationState, snapshot_codec, udp_snapshot};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io};

const MAGIC: &[u8; 4] = b"RMT1";
const ACK_MAGIC: &[u8; 4] = b"RMTA";
const TOPICS: usize = 9;
const MAX_PENDING: usize = 4;
const MAX_REVISIONS: usize = 8;
// Serialized payload budget per topic, separate from the baseline decoder's
// two 1 MiB baselines. This bounds retained data, not allocator overhead.
const CACHE_BYTES: usize = crate::protocol::MAX_LINE_BYTES;

/// Section identity within this experimental wire version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Topic {
    /// Envelope identity, tick/time, counters, input acknowledgements and metadata.
    Metadata,
    /// Complete chassis records, including motion, aim and life identities.
    Chassis,
    /// Existing compact projectile records, or full records on codec fallback.
    Projectiles,
    /// Hidden rule and identity state required by `Field::restore`.
    Restore,
    /// Visible referee state, including its clock.
    Referee,
    /// Visible rune state at the capture tick.
    Runes,
    /// Visible outpost state at the capture tick.
    Outposts,
    /// Base geometry, HP, shield and protection state.
    Bases,
    /// Snapshot hit history; live reliable hit delivery is unaffected.
    Hits,
}
impl Topic {
    /// All topics in manifest revision order.
    pub const ALL: [Self; TOPICS] = [
        Self::Metadata,
        Self::Chassis,
        Self::Projectiles,
        Self::Restore,
        Self::Referee,
        Self::Runes,
        Self::Outposts,
        Self::Bases,
        Self::Hits,
    ];
    /// Stable label used in probe reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Chassis => "chassis",
            Self::Projectiles => "projectiles",
            Self::Restore => "restore",
            Self::Referee => "referee",
            Self::Runes => "runes",
            Self::Outposts => "outposts",
            Self::Bases => "bases",
            Self::Hits => "hits",
        }
    }
}

/// One compressed application frame, before ordinary RMG1 fragmentation.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Encoded bytes; stage 1 sends one frame per manifest or section.
    pub bytes: Vec<u8>,
    /// Retirement messages use the reliable lane; periodic data does not.
    pub reliable: bool,
}
impl Frame {
    /// Applies production RMG1 framing for byte/fragment accounting or a harness.
    /// `revision` is a connection-wide frame identity, not a section revision.
    pub fn packets(&self, revision: u64) -> io::Result<Vec<Vec<u8>>> {
        crate::udp_codec::packets(revision, self.reliable, &self.bytes)
    }
    /// Section carried by this frame, or `None` for a manifest/other codec.
    pub fn topic(&self) -> Option<Topic> {
        if !self.bytes.starts_with(MAGIC) {
            return None;
        }
        self.bytes.get(4).and_then(|tag| {
            tag.checked_sub(1)
                .and_then(|index| Topic::ALL.get(index as usize).copied())
        })
    }
}
fn frame(topic: Option<Topic>, bytes: Vec<u8>, reliable: bool) -> Frame {
    let mut framed = Vec::with_capacity(bytes.len() + 5);
    framed.extend_from_slice(MAGIC);
    framed.push(topic.map_or(0, |topic| topic as u8 + 1));
    framed.extend(bytes);
    Frame {
        bytes: framed,
        reliable,
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Manifest {
    epoch: u64,
    checkpoint: u64,
    tick: u64,
    time_ns: u64,
    revisions: [u64; TOPICS],
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Section {
    epoch: u64,
    topic: Topic,
    revision: u64,
    value: Value,
}

// Leave null placeholders in the metadata skeleton. Unknown future metadata
// survives; each known topic must exist, so a schema mismatch fails closed.
fn pointer(metadata: &Value, topic: Topic) -> io::Result<String> {
    let compact = metadata.get("CompactSnapshot").is_some();
    let root = if compact {
        "/CompactSnapshot/state"
    } else {
        "/Snapshot"
    };
    if metadata.pointer(root).is_none() {
        return Err(invalid("not a player checkpoint"));
    }
    if topic == Topic::Projectiles && compact {
        Ok("/CompactSnapshot/projectiles".into())
    } else {
        Ok(format!("{root}/field/{}", topic.name()))
    }
}
fn split(state: &SimulationState) -> io::Result<[Value; TOPICS]> {
    let raw =
        snapshot_codec::encode_player_message(&ServerMessage::Snapshot(Box::new(state.clone())));
    let mut metadata: Value = serde_json::from_slice(&raw)?;
    let mut sections = std::array::from_fn(|_| Value::Null);
    for topic in Topic::ALL.into_iter().skip(1) {
        let path = pointer(&metadata, topic)?;
        sections[topic as usize] = metadata
            .pointer_mut(&path)
            .ok_or_else(|| invalid("missing checkpoint section"))?
            .take();
    }
    sections[0] = metadata;
    Ok(sections)
}
fn assemble(values: [&Value; TOPICS], manifest: &Manifest) -> io::Result<SimulationState> {
    let mut metadata = values[0].clone();
    for topic in Topic::ALL.into_iter().skip(1) {
        let path = pointer(&metadata, topic)?;
        let slot = metadata
            .pointer_mut(&path)
            .ok_or_else(|| invalid("missing section placeholder"))?;
        if !slot.is_null() {
            return Err(invalid("section placeholder is occupied"));
        }
        *slot = values[topic as usize].clone();
    }
    let bytes = serde_json::to_vec(&metadata)?;
    if bytes.len() > crate::protocol::MAX_LINE_BYTES {
        return Err(invalid("assembled checkpoint exceeds byte cap"));
    }
    let ServerMessage::Snapshot(state) = snapshot_codec::decode_player_message(&bytes)? else {
        return Err(invalid("assembled message is not a snapshot"));
    };
    if state.input_epoch != manifest.epoch
        || state.snapshot_id != manifest.checkpoint
        || state.field.tick != manifest.tick
        || state.field.time_ns != manifest.time_ns
        || state.field.restore.is_none()
    {
        return Err(invalid("checkpoint manifest/state mismatch"));
    }
    Ok(*state)
}

/// Per-peer experimental encoder. Captures once, then emits every section.
#[derive(Default)]
pub struct Encoder {
    epoch: Option<u64>,
    checkpoint: u64,
    codecs: [udp_snapshot::Encoder; TOPICS],
    sections: [Option<Section>; TOPICS],
}
impl Encoder {
    /// Splits a published complete state. Identities must increase within an epoch;
    /// unchanged exact section values reuse their revision, never an inferred pose.
    pub fn capture(&mut self, state: &SimulationState) -> io::Result<Vec<Frame>> {
        self.capture_with(state, None)
    }
    // Stage 2 suppresses only values explicitly received in this epoch. The
    // stage-1 path always passes None and remains the same-cadence control.
    fn capture_with(
        &mut self,
        state: &SimulationState,
        received: Option<&[u64; TOPICS]>,
    ) -> io::Result<Vec<Frame>> {
        if state.snapshot_id == 0
            || state.field.restore.is_none()
            || state.field.tick.checked_mul(rm_simulator_world::tick_ns())
                != Some(state.field.time_ns)
            || self.epoch.is_some_and(|epoch| state.input_epoch < epoch)
            || (self.epoch == Some(state.input_epoch) && state.snapshot_id <= self.checkpoint)
        {
            return Err(invalid(
                "invalid capture identity or incomplete restore state",
            ));
        }
        let values = split(state)?;
        if self.epoch != Some(state.input_epoch) {
            self.epoch = Some(state.input_epoch);
            self.sections = Default::default();
        }
        self.checkpoint = state.snapshot_id;
        let mut frames = Vec::with_capacity(TOPICS + 1);
        for (topic, value) in Topic::ALL.into_iter().zip(values) {
            let index = topic as usize;
            let previous = &self.sections[index];
            if previous.as_ref().is_none_or(|old| old.value != value) {
                let revision = previous
                    .as_ref()
                    .map_or(Some(1), |old| old.revision.checked_add(1))
                    .ok_or_else(|| invalid("section revision exhausted"))?;
                self.sections[index] = Some(Section {
                    epoch: state.input_epoch,
                    topic,
                    revision,
                    value,
                });
            }
            if received.is_some_and(|revisions| {
                revisions[index] == self.sections[index].as_ref().unwrap().revision
            }) {
                continue;
            }
            let bytes = serde_json::to_vec(self.sections[index].as_ref().unwrap())?;
            frames.push(frame(
                Some(topic),
                self.codecs[index].snapshot(state.input_epoch, &bytes)?,
                false,
            ));
            if let Some(retire) = self.codecs[index].resend_retire() {
                frames.push(frame(Some(topic), udp_snapshot::encode(retire), true));
            }
        }
        let manifest = Manifest {
            epoch: state.input_epoch,
            checkpoint: state.snapshot_id,
            tick: state.field.tick,
            time_ns: state.field.time_ns,
            revisions: std::array::from_fn(|i| self.sections[i].as_ref().unwrap().revision),
        };
        frames.insert(
            0,
            frame(
                None,
                crate::compression::compress(&serde_json::to_vec(&manifest)?),
                false,
            ),
        );
        Ok(frames)
    }
    /// Applies topic-tagged baseline feedback; returns reliable retirement work.
    /// Checkpoint dependency repair is deliberately not implemented in stage 1.
    pub fn feedback(&mut self, bytes: &[u8]) -> io::Result<Option<Frame>> {
        if !bytes.starts_with(ACK_MAGIC) || bytes.len() < 6 {
            return Err(invalid("invalid topic feedback"));
        }
        let topic = *Topic::ALL
            .get(bytes[4] as usize)
            .ok_or_else(|| invalid("unknown feedback topic"))?;
        let feedback = udp_snapshot::Feedback::decode(&bytes[5..])?;
        Ok(self.codecs[topic as usize]
            .feedback(feedback)
            .map(|wire| frame(Some(topic), udp_snapshot::encode(wire), true)))
    }
}

#[derive(Default)]
struct Cache {
    values: BTreeMap<u64, (Value, usize)>,
    bytes: usize,
}
impl Cache {
    fn insert(
        &mut self,
        section: Section,
        limit: usize,
        pinned: Option<u64>,
    ) -> io::Result<Vec<u64>> {
        if let Some((old, _)) = self.values.get(&section.revision) {
            if old != &section.value {
                return Err(invalid("section revision changed value"));
            }
            return Ok(Vec::new());
        }
        let size = serde_json::to_vec(&section.value)?.len();
        if size > CACHE_BYTES {
            return Err(invalid("section exceeds cache byte cap"));
        }
        self.bytes += size;
        self.values.insert(section.revision, (section.value, size));
        let mut evicted = Vec::new();
        while self.values.len() > limit || self.bytes > CACHE_BYTES {
            let latest = *self.values.last_key_value().unwrap().0;
            let revision = self
                .values
                .keys()
                .copied()
                .find(|id| Some(*id) != pinned && *id != latest);
            let Some(revision) = revision else {
                let (_, size) = self.values.remove(&section.revision).unwrap();
                self.bytes -= size;
                return Err(invalid("pinned section values exceed cache cap"));
            };
            let (_, size) = self.values.remove(&revision).unwrap();
            self.bytes -= size;
            evicted.push(revision);
        }
        Ok(evicted)
    }
}

/// Complete-checkpoint assembler with separate baseline and revision caches.
///
/// Holds at most four manifests and eight values / 4 MiB serialized payload per
/// topic, plus the baseline codec's two 1 MiB baselines per topic. Eviction abandons
/// dependent manifests; it never substitutes another revision. These are generous
/// prototype bounds, not a production memory budget or a recovery guarantee.
#[derive(Default)]
pub struct Decoder {
    // Stage 2 protects one repair target from ordinary revision/manifest churn.
    pinned: Option<Manifest>,
    // Zero selects the stage-1 limits; the paced experiment uses a larger,
    // still bounded window to accommodate delayed delivery and repair.
    window: usize,
    epoch: Option<u64>,
    completed: u64,
    codecs: [udp_snapshot::Decoder; TOPICS],
    caches: [Cache; TOPICS],
    pending: BTreeMap<u64, Manifest>,
}
impl Decoder {
    fn epoch(&mut self, epoch: u64) -> bool {
        if self.epoch.is_some_and(|current| epoch < current) {
            return false;
        }
        if self.epoch != Some(epoch) {
            *self = Self {
                epoch: Some(epoch),
                window: self.window,
                ..Default::default()
            };
        }
        true
    }
    /// Decodes one complete application frame. Only atomically assembled newer
    /// snapshots are returned. Feedback is an unfragmented upstream datagram.
    pub fn receive(
        &mut self,
        frame: &Frame,
    ) -> io::Result<(Option<SimulationState>, Option<Vec<u8>>)> {
        if !frame.bytes.starts_with(MAGIC)
            || frame.bytes.len() < 6
            || frame.bytes.len() > crate::protocol::MAX_LINE_BYTES
        {
            return Err(invalid("invalid section frame"));
        }
        let raw =
            crate::compression::decompress(&frame.bytes[5..], crate::protocol::MAX_LINE_BYTES)
                .map_err(|_| invalid("invalid compressed section frame"))?;
        let mut answer = None;
        if frame.bytes[4] == 0 {
            let manifest: Manifest = serde_json::from_slice(&raw)?;
            if manifest.checkpoint == 0
                || manifest.revisions.contains(&0)
                || manifest.tick.checked_mul(rm_simulator_world::tick_ns())
                    != Some(manifest.time_ns)
            {
                return Err(invalid("invalid checkpoint manifest"));
            }
            if !self.epoch(manifest.epoch) || manifest.checkpoint <= self.completed {
                return Ok((None, None));
            }
            if self
                .pending
                .get(&manifest.checkpoint)
                .is_some_and(|old| old != &manifest)
            {
                return Err(invalid("checkpoint identity changed manifest"));
            }
            self.pending.insert(manifest.checkpoint, manifest);
            while self.pending.len() > self.window.max(MAX_PENDING) {
                let oldest = *self
                    .pending
                    .keys()
                    .find(|id| self.pinned.as_ref().is_none_or(|p| p.checkpoint != **id))
                    .unwrap();
                self.pending.remove(&oldest);
            }
        } else {
            let topic = frame
                .topic()
                .ok_or_else(|| invalid("unknown section topic"))?;
            let wire = udp_snapshot::parse(&raw)?
                .ok_or_else(|| invalid("missing section baseline envelope"))?;
            if !frame.reliable && matches!(wire, udp_snapshot::Wire::Retire { .. }) {
                return Err(invalid("unreliable section baseline retirement"));
            }
            let epoch = match &wire {
                udp_snapshot::Wire::Independent { epoch, .. }
                | udp_snapshot::Wire::Full { epoch, .. }
                | udp_snapshot::Wire::Delta { epoch, .. }
                | udp_snapshot::Wire::Retire { epoch, .. } => *epoch,
            };
            if !self.epoch(epoch) {
                return Ok((None, None));
            }
            let (section, feedback) =
                self.codecs[topic as usize].receive_with(wire, |bytes, epoch| {
                    let section: Section = serde_json::from_slice(bytes)?;
                    if section.epoch != epoch || section.topic != topic || section.revision == 0 {
                        return Err(invalid("invalid section identity"));
                    }
                    Ok(section)
                })?;
            if let Some(feedback) = feedback {
                let mut bytes = ACK_MAGIC.to_vec();
                bytes.push(topic as u8);
                bytes.extend(feedback.encode());
                answer = Some(bytes);
            }
            if let Some(section) = section {
                let evicted = self.caches[topic as usize].insert(
                    section,
                    self.window.max(MAX_REVISIONS),
                    self.pinned.as_ref().map(|p| p.revisions[topic as usize]),
                )?;
                self.pending
                    .retain(|_, manifest| !evicted.contains(&manifest.revisions[topic as usize]));
            }
        }
        // Prefer the newest complete capture, not the newest capture with some data.
        let ready = self.pending.iter().rev().find_map(|(&id, manifest)| {
            manifest
                .revisions
                .iter()
                .enumerate()
                .all(|(i, revision)| self.caches[i].values.contains_key(revision))
                .then_some(id)
        });
        let complete = if let Some(id) = ready {
            let manifest = &self.pending[&id];
            let values = std::array::from_fn(|i| &self.caches[i].values[&manifest.revisions[i]].0);
            let state = assemble(values, manifest)?;
            self.completed = id;
            if self.pinned.as_ref().is_some_and(|p| p.checkpoint <= id) {
                self.pinned = None;
            }
            self.pending.retain(|&pending, _| pending > id);
            Some(state)
        } else {
            None
        };
        Ok((complete, answer))
    }
    /// Number of incomplete manifests, bounded by four.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    /// Retained section payload bytes, excluding JSON allocator and baseline overhead.
    pub fn cached_bytes(&self) -> usize {
        self.caches.iter().map(|cache| cache.bytes).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::Simulation;
    use rm_simulator_world::{
        Caliber, ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, RefereeConfig, Shot,
        Team,
    };
    use std::collections::VecDeque;

    fn source() -> (Field, rm_simulator_world::projectile::StaticGeometry) {
        let mut config = FieldConfig {
            referee: Some(RefereeConfig::alternating(2, 2)),
            ..Default::default()
        };
        config.runes.push(config.runes[0]);
        config.chassis.push(ChassisPlacement {
            team: Team::Red,
            config: ChassisConfig::default(),
            spawn: Pose::at([0., 0., 0.3]),
        });
        let field = Field::new(&config).unwrap();
        let geometry = field.static_geometry_snapshot();
        (field, geometry)
    }
    fn state(field: &Field, id: u64, epoch: u64) -> SimulationState {
        SimulationState {
            bots: vec![],
            snapshot_id: id,
            input_epoch: epoch,
            shot_results: vec![],
            paused: false,
            field: field.snapshot(),
        }
    }
    fn expected(state: &SimulationState) -> SimulationState {
        let ServerMessage::Snapshot(state) =
            snapshot_codec::decode_player_message(&snapshot_codec::encode_player_message(
                &ServerMessage::Snapshot(Box::new(state.clone())),
            ))
            .unwrap()
        else {
            panic!("snapshot")
        };
        *state
    }
    fn deliver(
        sender: &mut Encoder,
        receiver: &mut Decoder,
        frames: Vec<Frame>,
    ) -> Vec<SimulationState> {
        let mut queue: VecDeque<_> = frames.into();
        let mut states = Vec::new();
        while let Some(frame) = queue.pop_front() {
            let (state, feedback) = receiver.receive(&frame).unwrap();
            states.extend(state);
            if let Some(feedback) = feedback
                && let Some(retire) = sender.feedback(&feedback).unwrap()
            {
                queue.push_back(retire);
            }
        }
        states
    }

    #[test]
    fn reversed_sections_reconstruct_exact_player_state_and_restore_replay() {
        let (mut field, geometry) = source();
        let mut sender = Encoder::default();
        let mut receiver = Decoder::default();
        for id in 1..=70 {
            if id % 3 == 0 {
                field
                    .fire(Pose::at([0., 0., 2.]), Shot::at_limit(Caliber::Mm17), None)
                    .unwrap();
            }
            if id == 20 {
                field.remove_chassis(0).unwrap();
            }
            if id == 40 {
                field
                    .add_chassis(&ChassisPlacement {
                        config: ChassisConfig::default(),
                        spawn: Pose::at([1., 0., 0.3]),
                        team: Team::Blue,
                    })
                    .unwrap();
            }
            field.step(32).unwrap();
            let state = state(&field, id, u64::from(id >= 50));
            let mut frames = sender.capture(&state).unwrap();
            frames.reverse();
            let states = deliver(&mut sender, &mut receiver, frames);
            assert_eq!(states, vec![expected(&state)]);
            let mut restored = Field::restore(&states[0].field, &geometry, 0.).unwrap();
            let mut control = Field::restore(&expected(&state).field, &geometry, 0.).unwrap();
            restored.step(8).unwrap();
            control.step(8).unwrap();
            assert_eq!(restored.snapshot(), control.snapshot());
            assert!(receiver.codecs.iter().all(|codec| codec.pinned() <= 2));
        }
    }

    #[test]
    fn missing_sections_never_mix_ticks_and_delayed_data_completes_exact_capture() {
        let (mut field, _) = source();
        field
            .fire(Pose::at([0., 0., 2.]), Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        let mut sender = Encoder::default();
        let mut receiver = Decoder::default();
        let first = sender.capture(&state(&field, 1, 0)).unwrap();
        assert_eq!(deliver(&mut sender, &mut receiver, first).len(), 1);
        let mut delayed = Vec::new();
        let mut last = None;
        for id in 2..=3 {
            field.step(32).unwrap();
            let source = state(&field, id, 0);
            let frames = sender.capture(&source).unwrap();
            let (missing, available): (Vec<_>, Vec<_>) = frames
                .into_iter()
                .partition(|frame| frame.topic() == Some(Topic::Projectiles));
            delayed.extend(missing);
            assert!(deliver(&mut sender, &mut receiver, available).is_empty());
            last = Some(source);
        }
        // Newest missing revision arrives first. Older revision must not regress it.
        delayed.reverse();
        assert_eq!(
            deliver(&mut sender, &mut receiver, delayed),
            vec![expected(&last.unwrap())]
        );
        assert_eq!(receiver.pending(), 0);
    }

    #[test]
    fn epoch_change_discards_incomplete_captures_and_rejects_old_delivery() {
        let (mut field, _) = source();
        let mut sender = Encoder::default();
        let mut receiver = Decoder::default();
        let old = sender.capture(&state(&field, 1, 0)).unwrap();
        receiver.receive(&old[0]).unwrap();
        field.step(32).unwrap();
        let mut paused = state(&field, 2, 1);
        paused.paused = true;
        let frames = sender.capture(&paused).unwrap();
        assert_eq!(
            deliver(&mut sender, &mut receiver, frames),
            vec![expected(&paused)]
        );
        assert!(deliver(&mut sender, &mut receiver, old).is_empty());
        paused.snapshot_id = 3;
        let frames = sender.capture(&paused).unwrap();
        assert_eq!(
            deliver(&mut sender, &mut receiver, frames),
            vec![expected(&paused)]
        );
    }

    #[test]
    fn permanently_missing_topic_bounds_caches_without_partial_promotion() {
        let (mut field, _) = source();
        let mut sender = Encoder::default();
        let mut receiver = Decoder::default();
        for id in 1..=100 {
            field.step(32).unwrap();
            let frames = sender
                .capture(&state(&field, id, 0))
                .unwrap()
                .into_iter()
                .filter(|frame| frame.topic() != Some(Topic::Restore))
                .collect();
            assert!(deliver(&mut sender, &mut receiver, frames).is_empty());
            assert!(receiver.pending() <= MAX_PENDING);
            assert!(receiver.cached_bytes() <= TOPICS * CACHE_BYTES);
            assert!(
                receiver
                    .caches
                    .iter()
                    .all(|cache| cache.values.len() <= MAX_REVISIONS)
            );
        }
        field.step(32).unwrap();
        let source = state(&field, 101, 0);
        let frames = sender.capture(&source).unwrap();
        assert_eq!(
            deliver(&mut sender, &mut receiver, frames),
            vec![expected(&source)]
        );
    }

    #[test]
    fn manifest_and_topic_identity_mismatches_are_rejected() {
        let (field, _) = source();
        let frames = Encoder::default().capture(&state(&field, 1, 0)).unwrap();
        let mut receiver = Decoder::default();
        receiver.receive(&frames[0]).unwrap();
        let raw =
            crate::compression::decompress(&frames[0].bytes[5..], crate::protocol::MAX_LINE_BYTES)
                .unwrap();
        let mut manifest: Manifest = serde_json::from_slice(&raw).unwrap();
        manifest.revisions[0] += 1;
        let changed = frame(
            None,
            crate::compression::compress(&serde_json::to_vec(&manifest).unwrap()),
            false,
        );
        assert!(receiver.receive(&changed).is_err());
        let mut wrong_topic = frames[1].clone();
        wrong_topic.bytes[4] = Topic::Chassis as u8 + 1;
        assert!(receiver.receive(&wrong_topic).is_err());
        let mut mismatch = frames;
        manifest.revisions[0] -= 1;
        manifest.tick += 1;
        manifest.time_ns += rm_simulator_world::tick_ns();
        mismatch[0] = frame(
            None,
            crate::compression::compress(&serde_json::to_vec(&manifest).unwrap()),
            false,
        );
        let mut receiver = Decoder::default();
        assert!(
            mismatch
                .iter()
                .try_for_each(|frame| receiver.receive(frame).map(|_| ()))
                .is_err()
        );
    }

    #[test]
    fn unchanged_revisions_survive_baseline_retirement() {
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true);
        simulation.set_paused(true);
        let mut state = simulation.state();
        let mut sender = Encoder::default();
        let mut receiver = Decoder::default();
        let mut retirements = 0;
        for id in 1..=100 {
            state.snapshot_id = id;
            let frames = sender.capture(&state).unwrap();
            for frame in frames {
                let (_, feedback) = receiver.receive(&frame).unwrap();
                if let Some(feedback) = feedback
                    && let Some(retire) = sender.feedback(&feedback).unwrap()
                {
                    retirements += 1;
                    let (_, ack) = receiver.receive(&retire).unwrap();
                    sender.feedback(&ack.unwrap()).unwrap();
                }
            }
            assert_eq!(receiver.completed, id);
            assert_eq!(
                sender.sections[Topic::Restore as usize]
                    .as_ref()
                    .unwrap()
                    .revision,
                1
            );
            assert!(
                receiver.caches[Topic::Restore as usize]
                    .values
                    .contains_key(&1)
            );
        }
        assert!(retirements >= TOPICS * 2);
    }

    #[test]
    fn full_precision_fallback_is_preserved() {
        let (mut field, _) = source();
        field
            .fire(Pose::at([0., 0., 2.]), Shot::at_limit(Caliber::Mm17), None)
            .unwrap();
        let mut source = state(&field, 1, 0);
        source.field.projectiles[0].position_m[0] = 1e60;
        let mut sender = Encoder::default();
        let mut receiver = Decoder::default();
        let frames = sender.capture(&source).unwrap();
        assert_eq!(deliver(&mut sender, &mut receiver, frames), vec![source]);
    }
}
