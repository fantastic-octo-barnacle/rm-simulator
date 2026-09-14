// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Stage-2 experimental delivery: one byte budget, aggregated fragments, section
//! receipts and bounded exact-revision repair. All deadlines use caller time.
//!
//! This remains an offline protocol behind `section-topics`. Publication and
//! physics cadences are unchanged, and no partial state reaches prediction.
//! Reliable controls have an ordered queue; periodic and repair topics have
//! separate replaceable queues. Every queue receives round-robin byte service.
//!
//! ```
//! use rm_simulator_server::{section_topics::delivery::{Sender, Receiver}, simulation::Simulation};
//! use rm_simulator_world::{Field, FieldConfig};
//! use std::time::Duration;
//! let mut state = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false).state();
//! state.snapshot_id = 1;
//! let mut sender = Sender::new(512 * 1024);
//! let mut receiver = Receiver::new(10 * 1024);
//! sender.publish(&state, Duration::ZERO).unwrap();
//! let mut complete = None;
//! for ms in 1..100 {
//!     let now = Duration::from_millis(ms);
//!     while let Some(packet) = sender.next(now).unwrap() {
//!         for message in receiver.receive(&packet.bytes, now).unwrap() {
//!             if let rm_simulator_server::protocol::ServerMessage::Snapshot(state) = message {
//!                 complete = Some(state);
//!             }
//!         }
//!     }
//!     if let Some(feedback) = receiver.feedback(now).unwrap() {
//!         sender.feedback(&feedback.bytes, now).unwrap();
//!     }
//! }
//! assert_eq!(complete.unwrap().snapshot_id, 1);
//! ```
use super::{Cache, Decoder, Encoder, Frame, Manifest, Section, TOPICS, Topic, frame, invalid};
use crate::{pacing::Datagram, protocol::ServerMessage, simulation::SimulationState, udp_snapshot};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    time::Duration,
};

mod completion;
pub mod presentation;
mod scheduling;

const DATA: &[u8; 4] = b"RMS2";
const RELIABLE: &[u8; 4] = b"RMR2";
const FEEDBACK: &[u8; 4] = b"RMF2";
const CONTROL: &[u8; 4] = b"RMC2";
const MTU: usize = 1024;
const CHUNK: usize = 1000;
const HEADER: usize = 19; // channel, frame id, full length, offset, piece length
const CLASSES: usize = TOPICS * 2 + 5;
const REPAIR_MANIFEST: usize = TOPICS * 2 + 1;
const CONTROL_CLASS: usize = CLASSES - 1;
const WINDOW: usize = 32;
const QUEUE_LIMIT: usize = 2 << 20;
const FRAME_LIMIT: usize = crate::protocol::MAX_LINE_BYTES;
const REPAIR_PERIOD: Duration = Duration::from_millis(96);
const PART_LIFETIME: Duration = Duration::from_secs(1);

/// Application-byte counters for a stage-2 sender. Native UDP/GNS overhead and
/// carrier retransmission bytes are not known to this codec.
#[derive(Clone, Default, Debug, Serialize)]
pub struct Stats {
    /// Complete publications encoded after backpressure coalescing.
    pub captured_checkpoints: u64,
    /// Unsent successor states replaced before encoding.
    pub coalesced_checkpoints: u64,
    /// Bytes released under the connection's token bucket, aggregation included.
    pub sent_bytes: u64,
    /// Datagrams released under that bucket.
    pub sent_packets: u64,
    /// Section deliveries omitted because the exact revision was acknowledged.
    pub suppressed_sections: u64,
    /// Exact independent section repairs queued after missing-dependency feedback.
    pub repair_sections: u64,
    /// Requested checkpoints/revisions outside the bounded repair history.
    pub repair_misses: u64,
    /// Fragment bytes served per topic, including its repair work, excluding the
    /// shared four-byte datagram header. Ordered baseline control is separate.
    pub topic_service_bytes: [u64; TOPICS],
}
struct Budget {
    rate: f64,
    tokens: f64,
    last: Duration,
}
impl Budget {
    fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1024) as f64,
            tokens: 0.,
            last: Duration::ZERO,
        }
    }
    fn advance(&mut self, now: Duration) -> io::Result<()> {
        if now < self.last {
            return Err(invalid("delivery clock moved backwards"));
        }
        self.tokens = (self.tokens + (now - self.last).as_secs_f64() * self.rate)
            .min((self.rate * 0.032).max(MTU as f64));
        self.last = now;
        Ok(())
    }
}
struct Transfer {
    checkpoint: Option<u64>,
    id: u64,
    bytes: Vec<u8>,
    offset: usize,
}
#[derive(Default)]
struct Slot {
    active: Option<Transfer>,
    pending: Option<Transfer>,
}
impl Slot {
    fn offer(&mut self, transfer: Transfer) {
        if self.active.as_ref().is_some_and(|active| active.offset > 0) {
            self.pending = Some(transfer);
        } else {
            self.active = Some(transfer);
            self.pending = None;
        }
    }
    fn bytes(&self) -> usize {
        [&self.active, &self.pending]
            .into_iter()
            .flatten()
            .map(|t| t.bytes.len())
            .sum()
    }
}
struct Queue {
    completion: Option<completion::Completion>,
    drr: Option<scheduling::Drr>,
    #[cfg(test)]
    trace: Option<trials::tuning::Trace>,
    slots: [Slot; CONTROL_CLASS],
    control: VecDeque<Transfer>,
    next_id: u64,
    cursor: usize,
    budget: Budget,
    stats: Stats,
}
impl Queue {
    fn new(rate: u32) -> Self {
        Self {
            drr: None,
            completion: None,
            #[cfg(test)]
            trace: None,
            slots: std::array::from_fn(|_| Slot::default()),
            control: VecDeque::new(),
            next_id: 0,
            cursor: 0,
            budget: Budget::new(rate),
            stats: Stats::default(),
        }
    }
    fn bytes(&self) -> usize {
        self.slots.iter().map(Slot::bytes).sum::<usize>()
            + self.control.iter().map(|t| t.bytes.len()).sum::<usize>()
    }
    fn offer(&mut self, class: usize, bytes: Vec<u8>) -> io::Result<()> {
        self.offer_checkpoint(class, bytes, None)
    }
    fn offer_checkpoint(
        &mut self,
        class: usize,
        bytes: Vec<u8>,
        checkpoint: Option<u64>,
    ) -> io::Result<()> {
        #[cfg(test)]
        if let Some(trace) = &self.trace {
            trace.borrow_mut().offer(class, bytes.len());
        }
        if class != CONTROL_CLASS
            && [&self.slots[class].active, &self.slots[class].pending]
                .into_iter()
                .flatten()
                .any(|t| t.bytes == bytes)
        {
            #[cfg(test)]
            if let Some(trace) = &self.trace {
                trace.borrow_mut().suppressed(class, bytes.len());
            }
            return Ok(());
        }
        if bytes.is_empty() || bytes.len() > FRAME_LIMIT || self.bytes() + bytes.len() > QUEUE_LIMIT
        {
            return Err(invalid("section delivery queue byte cap"));
        }
        if class == CONTROL_CLASS && self.control.len() >= 64 {
            return Err(invalid("section control queue count cap"));
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| invalid("delivery frame id exhausted"))?;
        #[cfg(test)]
        if let Some(trace) = &self.trace {
            let replaced = if class == CONTROL_CLASS {
                None
            } else if self.slots[class]
                .active
                .as_ref()
                .is_some_and(|t| t.offset > 0)
            {
                self.slots[class].pending.as_ref()
            } else {
                self.slots[class].active.as_ref()
            };
            trace
                .borrow_mut()
                .enqueue(class, self.next_id, bytes.len(), replaced.map(|t| t.id));
        }
        let transfer = Transfer {
            checkpoint,
            id: self.next_id,
            bytes,
            offset: 0,
        };
        if class == CONTROL_CLASS {
            self.control.push_back(transfer);
        } else {
            self.slots[class].offer(transfer);
        }
        Ok(())
    }
    fn front(&self, class: usize) -> Option<&Transfer> {
        if class == CONTROL_CLASS {
            self.control.front()
        } else {
            self.slots[class].active.as_ref()
        }
    }
    fn consume(&mut self, class: usize, output: &mut Vec<u8>) -> usize {
        let transfer = if class == CONTROL_CLASS {
            self.control.front_mut().unwrap()
        } else {
            self.slots[class].active.as_mut().unwrap()
        };
        let length = (transfer.bytes.len() - transfer.offset).min(CHUNK);
        output.push(class as u8);
        output.extend(transfer.id.to_le_bytes());
        output.extend((transfer.bytes.len() as u32).to_le_bytes());
        output.extend((transfer.offset as u32).to_le_bytes());
        output.extend((length as u16).to_le_bytes());
        output.extend(&transfer.bytes[transfer.offset..transfer.offset + length]);
        #[cfg(test)]
        if let Some(trace) = &self.trace {
            trace.borrow_mut().sent(
                class,
                transfer.id,
                length,
                transfer.offset + length == transfer.bytes.len(),
            );
        }
        transfer.offset += length;
        if transfer.offset == transfer.bytes.len() {
            if class == CONTROL_CLASS {
                self.control.pop_front();
            } else {
                self.slots[class].active = self.slots[class].pending.take();
            }
        }
        length + HEADER
    }
    fn next(&mut self, now: Duration) -> io::Result<Option<Datagram>> {
        self.budget.advance(now)?;
        if self.drr.is_some() {
            return self.next_weighted();
        }
        let mut bytes = Vec::with_capacity(MTU);
        let mut reliable = false;
        for _ in 0..CLASSES * 2 {
            let class = self.cursor;
            let Some(transfer) = self.front(class) else {
                self.cursor = (class + 1) % CLASSES;
                continue;
            };
            let cost = HEADER + (transfer.bytes.len() - transfer.offset).min(CHUNK);
            if bytes.is_empty() {
                if (cost + 4) as f64 > self.budget.tokens {
                    break;
                }
                reliable = class == CONTROL_CLASS;
                bytes.extend(if reliable { RELIABLE } else { DATA });
            } else if reliable != (class == CONTROL_CLASS) {
                break;
            }
            if bytes.len() + cost > MTU || (bytes.len() + cost) as f64 > self.budget.tokens {
                break;
            }
            let served = self.consume(class, &mut bytes);
            if (1..REPAIR_MANIFEST).contains(&class) {
                self.stats.topic_service_bytes[(class - 1) % TOPICS] += served as u64;
            }
            self.cursor = (class + 1) % CLASSES;
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        self.budget.tokens -= bytes.len() as f64;
        self.stats.sent_bytes += bytes.len() as u64;
        self.stats.sent_packets += 1;
        #[cfg(test)]
        if let Some(trace) = &self.trace {
            trace.borrow_mut().packet(bytes.len());
        }
        Ok(Some(Datagram { bytes, reliable }))
    }
}

#[derive(Serialize, Deserialize)]
struct Feedback {
    epoch: u64,
    have: [u64; TOPICS],
    completed: u64,
    need: Option<(u64, [bool; TOPICS])>,
    baseline: Vec<Vec<u8>>,
}

struct Repair {
    manifest: Manifest,
    values: [Option<serde_json::Value>; TOPICS],
}

/// Stage-2 host leg. Retains at most 32 captured manifests and 32 revisions /
/// 4 MiB serialized values per topic for repair. Data queues retain an active
/// transfer and one replaceable successor per topic/class, under a 2 MiB cap.
/// A successor snapshot and one selected repair each have a separate 4 MiB
/// serialized-state cap. Every operation uses caller-supplied monotonic time.
pub struct Sender {
    presentation_offered: Option<(u64, u64, Duration)>,
    pending_state: Option<SimulationState>,
    offered: Option<(u64, u64)>,
    repair: Option<Repair>,
    encoder: Encoder,
    history: [Cache; TOPICS],
    manifests: BTreeMap<u64, Manifest>,
    received: [u64; TOPICS],
    completed: u64,
    queue: Queue,
    published: Duration,
    last_repair: Option<Duration>,
    last_manifest: Duration,
}
impl Sender {
    /// Creates one connection with a downstream application-byte budget per second.
    pub fn new(bytes_per_s: u32) -> Self {
        Self {
            presentation_offered: None,
            pending_state: None,
            offered: None,
            repair: None,
            encoder: Encoder::default(),
            history: std::array::from_fn(|_| Cache::default()),
            manifests: BTreeMap::new(),
            received: [0; TOPICS],
            completed: 0,
            queue: Queue::new(bytes_per_s),
            published: Duration::ZERO,
            last_repair: None,
            last_manifest: Duration::ZERO,
        }
    }
    /// Offers a complete state at the unchanged publication cadence. While a
    /// publication is being sent, only the newest successor is retained, so
    /// congestion cannot independently replace every section needed to assemble
    /// a checkpoint. This waits for queue service, never for delivery ACKs.
    pub fn publish(&mut self, state: &SimulationState, now: Duration) -> io::Result<()> {
        if self
            .offered
            .is_some_and(|old| (state.input_epoch, state.snapshot_id) <= old)
            || state.snapshot_id == 0
            || state.field.restore.is_none()
            || state.field.tick.checked_mul(rm_simulator_world::tick_ns())
                != Some(state.field.time_ns)
            || now < self.published
        {
            return Err(invalid("invalid offered checkpoint identity or clock"));
        }
        if serde_json::to_vec(state)?.len() > FRAME_LIMIT {
            return Err(invalid("pending checkpoint exceeds byte cap"));
        }
        self.offered = Some((state.input_epoch, state.snapshot_id));
        if self.encoder.epoch != Some(state.input_epoch) || self.periodic_empty() {
            self.queue.stats.coalesced_checkpoints += u64::from(self.pending_state.is_some());
            self.pending_state = None;
            self.capture(state, now)
        } else {
            self.queue.stats.coalesced_checkpoints += u64::from(self.pending_state.is_some());
            self.pending_state = Some(state.clone());
            self.published = now;
            Ok(())
        }
    }
    fn periodic_empty(&self) -> bool {
        self.queue.slots[..=TOPICS]
            .iter()
            .all(|slot| slot.active.is_none() && slot.pending.is_none())
    }
    fn capture(&mut self, state: &SimulationState, now: Duration) -> io::Result<()> {
        if now < self.published {
            return Err(invalid("publication clock moved backwards"));
        }
        if self
            .encoder
            .epoch
            .is_some_and(|epoch| state.input_epoch < epoch)
        {
            return Err(invalid("publication epoch moved backwards"));
        }
        if self.encoder.epoch != Some(state.input_epoch) {
            self.received = [0; TOPICS];
            self.completed = 0;
            self.history = std::array::from_fn(|_| Cache::default());
            self.manifests.clear();
            self.queue.slots = std::array::from_fn(|_| Slot::default());
            if self.queue.completion.is_some() {
                self.queue.completion = Some(completion::Completion::default());
            }
            self.last_repair = None;
            self.repair = None;
        }
        #[cfg(test)]
        if let Some(trace) = &self.queue.trace {
            trace
                .borrow_mut()
                .capture(state.snapshot_id, state.field.time_ns);
        }
        let frames = self.encoder.capture_with(state, Some(&self.received))?;
        self.queue.stats.captured_checkpoints += 1;
        self.published = now;
        self.last_manifest = now;
        let manifest = Manifest {
            epoch: state.input_epoch,
            checkpoint: state.snapshot_id,
            tick: state.field.tick,
            time_ns: state.field.time_ns,
            revisions: std::array::from_fn(|i| self.encoder.sections[i].as_ref().unwrap().revision),
        };
        self.manifests.insert(manifest.checkpoint, manifest);
        while self.manifests.len() > WINDOW {
            self.manifests.pop_first();
        }
        for i in 0..TOPICS {
            self.history[i].insert(
                self.encoder.sections[i].as_ref().unwrap().clone(),
                WINDOW,
                None,
            )?;
            self.queue.stats.suppressed_sections +=
                u64::from(self.received[i] == self.encoder.sections[i].as_ref().unwrap().revision);
        }
        for frame in frames {
            self.enqueue(frame, false)?;
        }
        Ok(())
    }
    fn enqueue(&mut self, frame: Frame, repair: bool) -> io::Result<()> {
        let class = if frame.reliable {
            CONTROL_CLASS
        } else {
            frame
                .topic()
                .map_or(if repair { REPAIR_MANIFEST } else { 0 }, |topic| {
                    1 + topic as usize + if repair { TOPICS } else { 0 }
                })
        };
        let checkpoint = if class == CONTROL_CLASS {
            None
        } else if repair {
            self.repair.as_ref().map(|r| r.manifest.checkpoint)
        } else {
            Some(self.encoder.checkpoint)
        };
        self.queue.offer_checkpoint(class, frame.bytes, checkpoint)
    }
    fn manifest(&mut self, manifest: &Manifest) -> io::Result<()> {
        self.enqueue(
            frame(
                None,
                crate::compression::compress(&serde_json::to_vec(manifest)?),
                false,
            ),
            false,
        )
    }
    /// Enqueues reliable application controls in order. Snapshot confirmations
    /// retain full JSON precision and cannot be replaced by a periodic snapshot.
    pub fn control(&mut self, message: &ServerMessage) -> io::Result<()> {
        let mut bytes = CONTROL.to_vec();
        bytes.extend(crate::compression::compress(&serde_json::to_vec(message)?));
        self.queue.offer(CONTROL_CLASS, bytes)
    }
    /// Handles a bounded receipt/repair datagram. Missing dependencies are served
    /// as exact independent section values, never newest-value substitutions.
    pub fn feedback(&mut self, bytes: &[u8], now: Duration) -> io::Result<()> {
        if !bytes.starts_with(FEEDBACK) || bytes.len() > MTU {
            return Err(invalid("invalid delivery feedback"));
        }
        let raw = crate::compression::decompress(&bytes[4..], 16 * 1024)
            .map_err(|_| invalid("invalid compressed feedback"))?;
        let feedback: Feedback = serde_json::from_slice(&raw)?;
        if self.encoder.epoch != Some(feedback.epoch) {
            return Ok(());
        }
        if feedback.baseline.len() > 32 {
            return Err(invalid("too many baseline answers"));
        }
        for bytes in feedback.baseline {
            if let Some(retire) = self.encoder.feedback(&bytes)? {
                self.enqueue(retire, false)?;
            }
        }
        for i in 0..TOPICS {
            if feedback.have[i] <= self.encoder.sections[i].as_ref().unwrap().revision {
                self.received[i] = self.received[i].max(feedback.have[i]);
            }
        }
        self.completed = self
            .completed
            .max(feedback.completed.min(self.encoder.checkpoint));
        if let Some(policy) = &mut self.queue.completion {
            let requested = feedback.need.as_ref().map(|(id, _)| *id).filter(|id| {
                self.manifests.contains_key(id)
                    || self
                        .repair
                        .as_ref()
                        .is_some_and(|r| r.manifest.checkpoint == *id)
            });
            policy.feedback(self.completed, requested);
        }
        if self
            .repair
            .as_ref()
            .is_some_and(|r| r.manifest.checkpoint <= self.completed)
        {
            self.repair = None;
        }
        if let Some((id, missing)) = feedback.need {
            if self
                .last_repair
                .is_some_and(|last| now.saturating_sub(last) < REPAIR_PERIOD)
            {
                return Ok(());
            }
            self.last_repair = Some(now);
            let Some(manifest) = self
                .repair
                .as_ref()
                .filter(|r| r.manifest.checkpoint == id)
                .map(|r| r.manifest.clone())
                .or_else(|| self.manifests.get(&id).cloned())
            else {
                self.queue.stats.repair_misses += 1;
                if let Some(manifest) = self.manifests.last_key_value().map(|(_, m)| m.clone()) {
                    self.manifest(&manifest)?;
                }
                return Ok(());
            };
            if self
                .repair
                .as_ref()
                .is_none_or(|r| r.manifest.checkpoint != id)
            {
                let values = std::array::from_fn(|i| {
                    self.history[i]
                        .values
                        .get(&manifest.revisions[i])
                        .map(|(value, _)| value.clone())
                });
                if serde_json::to_vec(&values)?.len() > FRAME_LIMIT {
                    return Err(invalid("pinned repair exceeds byte cap"));
                }
                self.repair = Some(Repair {
                    manifest: manifest.clone(),
                    values,
                });
            }
            // Cache pressure may also have evicted the dependent manifest.
            self.enqueue(
                frame(
                    None,
                    crate::compression::compress(&serde_json::to_vec(&manifest)?),
                    false,
                ),
                true,
            )?;
            for (i, missing) in missing.into_iter().enumerate() {
                if !missing {
                    continue;
                }
                if let Some(value) = self.repair.as_ref().unwrap().values[i].as_ref() {
                    let section = Section {
                        epoch: manifest.epoch,
                        topic: Topic::ALL[i],
                        revision: manifest.revisions[i],
                        value: value.clone(),
                    };
                    let wire = udp_snapshot::Wire::Independent {
                        epoch: manifest.epoch,
                        state: serde_json::to_value(section)?,
                    };
                    self.enqueue(
                        frame(Some(Topic::ALL[i]), udp_snapshot::encode(wire), false),
                        true,
                    )?;
                    self.queue.stats.repair_sections += 1;
                } else {
                    self.queue.stats.repair_misses += 1;
                }
            }
        }
        Ok(())
    }
    /// Releases an aggregated datagram when the shared token bucket allows it.
    /// A quiet final manifest is retried until its complete checkpoint is received.
    pub fn next(&mut self, now: Duration) -> io::Result<Option<Datagram>> {
        if self.periodic_empty()
            && let Some(state) = self.pending_state.take()
        {
            self.capture(&state, now)?;
        }
        if now.saturating_sub(self.last_manifest) >= REPAIR_PERIOD
            && let Some(manifest) = self
                .manifests
                .last_key_value()
                .map(|(_, manifest)| manifest.clone())
            && manifest.checkpoint > self.completed
        {
            self.manifest(&manifest)?;
            self.last_manifest = now;
        }
        self.queue.next(now)
    }
    /// Connection lifetime application-byte and repair counters.
    pub fn stats(&self) -> &Stats {
        &self.queue.stats
    }
    /// Encoded payload bytes retained by the bounded outgoing queues.
    pub fn queued_bytes(&self) -> usize {
        self.queue.bytes()
    }
    /// Serialized exact section values retained for repair, excluding baseline
    /// and allocator overhead. The cap is 4 MiB per topic.
    pub fn history_bytes(&self) -> usize {
        self.history.iter().map(|cache| cache.bytes).sum()
    }
}

struct Partial {
    id: u64,
    bytes: Vec<u8>,
    received: Vec<bool>,
    remaining: usize,
    started: Duration,
}
struct Reassembly {
    #[cfg(test)]
    trace: Option<trials::tuning::Trace>,
    pending: [Option<Partial>; CLASSES],
    completed: [u64; CLASSES],
}
impl Default for Reassembly {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            trace: None,
            pending: std::array::from_fn(|_| None),
            completed: [0; CLASSES],
        }
    }
}
impl Reassembly {
    fn bytes(&self) -> usize {
        self.pending.iter().flatten().map(|p| p.bytes.len()).sum()
    }
    fn receive(&mut self, packet: &[u8], now: Duration) -> io::Result<Vec<(usize, Frame)>> {
        if packet.len() < 4 + HEADER
            || packet.len() > MTU
            || (!packet.starts_with(DATA) && !packet.starts_with(RELIABLE))
        {
            return Err(invalid("invalid aggregate datagram"));
        }
        let reliable = packet.starts_with(RELIABLE);
        // The class index is used only by the test-only observer.
        #[cfg_attr(not(test), allow(clippy::unused_enumerate_index))]
        for (_class, partial) in self.pending[..CONTROL_CLASS].iter_mut().enumerate() {
            if partial
                .as_ref()
                .is_some_and(|p| now.saturating_sub(p.started) >= PART_LIFETIME)
            {
                #[cfg(test)]
                if let Some(trace) = &self.trace {
                    trace
                        .borrow_mut()
                        .discarded(_class, partial.as_ref().unwrap());
                }
                *partial = None;
            }
        }
        let mut cursor = 4;
        let mut frames = Vec::new();
        while cursor < packet.len() {
            let header = packet
                .get(cursor..cursor + HEADER)
                .ok_or_else(|| invalid("truncated fragment header"))?;
            let class = header[0] as usize;
            let id = u64::from_le_bytes(header[1..9].try_into().unwrap());
            let total = u32::from_le_bytes(header[9..13].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(header[13..17].try_into().unwrap()) as usize;
            let length = u16::from_le_bytes(header[17..19].try_into().unwrap()) as usize;
            cursor += HEADER;
            let bytes = packet
                .get(cursor..cursor + length)
                .ok_or_else(|| invalid("truncated fragment payload"))?;
            cursor += length;
            if class >= CLASSES
                || reliable != (class == CONTROL_CLASS)
                || id == 0
                || total == 0
                || total > FRAME_LIMIT
                || offset >= total
                || !offset.is_multiple_of(CHUNK)
                || length != (total - offset).min(CHUNK)
            {
                return Err(invalid("invalid aggregate fragment identity or size"));
            }
            #[cfg(test)]
            if let Some(trace) = &self.trace {
                trace.borrow_mut().receipt(id);
            }
            if id <= self.completed[class]
                || self.pending[class].as_ref().is_some_and(|p| p.id > id)
            {
                continue;
            }
            if self.pending[class].as_ref().is_none_or(|p| p.id != id) {
                if reliable && self.pending[class].is_some() {
                    return Err(invalid("reliable frame order changed"));
                }
                #[cfg(test)]
                if let (Some(trace), Some(partial)) = (&self.trace, &self.pending[class]) {
                    trace.borrow_mut().discarded(class, partial);
                }
                self.pending[class] = None;
                while self.bytes() + total > FRAME_LIMIT {
                    let oldest = self.pending[..CONTROL_CLASS]
                        .iter()
                        .enumerate()
                        .filter_map(|(i, p)| p.as_ref().map(|p| (i, p.started)))
                        .min_by_key(|(_, at)| *at);
                    let Some((oldest, _)) = oldest else {
                        return Err(invalid("reliable reassembly byte cap"));
                    };
                    #[cfg(test)]
                    if let Some(trace) = &self.trace {
                        trace
                            .borrow_mut()
                            .discarded(oldest, self.pending[oldest].as_ref().unwrap());
                    }
                    self.pending[oldest] = None;
                }
                self.pending[class] = Some(Partial {
                    id,
                    bytes: vec![0; total],
                    received: vec![false; total.div_ceil(CHUNK)],
                    remaining: total.div_ceil(CHUNK),
                    started: now,
                });
            }
            let partial = self.pending[class].as_mut().unwrap();
            if partial.bytes.len() != total {
                return Err(invalid("fragment frame size changed"));
            }
            let index = offset / CHUNK;
            if partial.received[index] && partial.bytes[offset..offset + length] != *bytes {
                return Err(invalid("duplicate fragment changed value"));
            }
            if !partial.received[index] {
                partial.bytes[offset..offset + length].copy_from_slice(bytes);
                partial.received[index] = true;
                partial.remaining -= 1;
            }
            if partial.remaining == 0 {
                let partial = self.pending[class].take().unwrap();
                self.completed[class] = id;
                #[cfg(test)]
                if let Some(trace) = &self.trace {
                    trace.borrow_mut().assembled(class, id, partial.started);
                }
                frames.push((
                    class,
                    Frame {
                        bytes: partial.bytes,
                        reliable,
                    },
                ));
            }
        }
        Ok(frames)
    }
}

/// Stage-2 client leg. Reassembles one frame per topic/repair class (4 MiB total),
/// assembles at most 32 pending manifests, and paces aggregate upstream feedback.
/// Available section timestamps are diagnostics only, never partial physics input.
pub struct Receiver {
    presentation: presentation::View,
    pinned_since: Duration,
    decoder: Decoder,
    parts: Reassembly,
    manifests: BTreeMap<u64, (Manifest, Duration)>,
    answers: VecDeque<Vec<u8>>,
    available: [Option<u64>; TOPICS],
    budget: Budget,
    last_feedback: Option<Duration>,
    dirty: bool,
    feedback_bytes: u64,
    feedback_packets: u64,
}
impl Receiver {
    /// Creates a client with an upstream application-byte budget per second.
    pub fn new(bytes_per_s: u32) -> Self {
        Self {
            presentation: presentation::View::default(),
            pinned_since: Duration::ZERO,
            decoder: Decoder {
                window: WINDOW,
                ..Default::default()
            },
            parts: Reassembly::default(),
            manifests: BTreeMap::new(),
            answers: VecDeque::new(),
            available: [None; TOPICS],
            budget: Budget::new(bytes_per_s),
            last_feedback: None,
            dirty: false,
            feedback_bytes: 0,
            feedback_packets: 0,
        }
    }
    /// Decodes datagrams into complete checkpoints or ordered reliable messages.
    /// A permanently missing topic cannot block decoding of other section values.
    pub fn receive(&mut self, packet: &[u8], now: Duration) -> io::Result<Vec<ServerMessage>> {
        let mut messages = Vec::new();
        for (class, frame) in self.parts.receive(packet, now)? {
            if class == presentation::CHASSIS || class == presentation::PROJECTILES {
                #[cfg(test)]
                let old_capture = if class == presentation::CHASSIS {
                    self.presentation.chassis.as_ref().map(|p| p.capture)
                } else {
                    self.presentation.projectiles.as_ref().map(|p| p.capture)
                };
                self.presentation.receive(class, &frame.bytes)?;
                #[cfg(test)]
                if let Some(trace) = &self.parts.trace {
                    let new_capture = if class == presentation::CHASSIS {
                        self.presentation.chassis.as_ref().map(|p| p.capture)
                    } else {
                        self.presentation.projectiles.as_ref().map(|p| p.capture)
                    };
                    if new_capture != old_capture {
                        trace.borrow_mut().useful_pose(class, frame.bytes.len());
                    }
                }
                continue;
            }
            if frame.bytes.starts_with(CONTROL) {
                if !frame.reliable {
                    return Err(invalid("unreliable application control"));
                }
                let raw = crate::compression::decompress(&frame.bytes[4..], FRAME_LIMIT)
                    .map_err(|_| invalid("invalid control frame"))?;
                messages.push(serde_json::from_slice(&raw)?);
                continue;
            }
            if !frame.reliable
                && class != frame.topic().map_or(0, |topic| 1 + topic as usize)
                && class
                    != frame
                        .topic()
                        .map_or(REPAIR_MANIFEST, |topic| 1 + TOPICS + topic as usize)
            {
                return Err(invalid("section delivered on wrong topic"));
            }
            let old_epoch = self.decoder.epoch;
            let (state, feedback) = self.decoder.receive(&frame)?;
            #[cfg(test)]
            if let Some(trace) = &self.parts.trace {
                trace
                    .borrow_mut()
                    .decoded(class, frame.bytes.len(), &self.decoder);
            }
            if old_epoch != self.decoder.epoch {
                self.manifests.clear();
                self.answers.clear();
                self.available = [None; TOPICS];
            }
            if let Some(feedback) = feedback
                && !self.answers.contains(&feedback)
            {
                if self.answers.len() >= 32 {
                    self.answers.pop_front();
                }
                self.answers.push_back(feedback);
            }
            if frame.topic().is_none() && frame.bytes.starts_with(super::MAGIC) {
                let raw = crate::compression::decompress(&frame.bytes[5..], FRAME_LIMIT)
                    .map_err(|_| invalid("invalid manifest frame"))?;
                let manifest: Manifest = serde_json::from_slice(&raw)?;
                if self.decoder.epoch == Some(manifest.epoch) {
                    #[cfg(test)]
                    if let Some(trace) = &self.parts.trace {
                        trace.borrow_mut().manifest(manifest.checkpoint);
                    }
                    self.manifests
                        .entry(manifest.checkpoint)
                        .or_insert((manifest, now));
                    while self.manifests.len() > WINDOW {
                        self.manifests.pop_first();
                    }
                }
            }
            for (manifest, _) in self.manifests.values() {
                for (i, revision) in manifest.revisions.iter().enumerate() {
                    if self.decoder.caches[i].values.contains_key(revision) {
                        self.available[i] =
                            Some(self.available[i].unwrap_or(0).max(manifest.time_ns));
                    }
                }
            }
            if let Some(state) = state {
                #[cfg(test)]
                if let Some(trace) = &self.parts.trace {
                    trace.borrow_mut().checkpoint(state.snapshot_id, class);
                }
                self.presentation.checkpoint(&state);
                messages.push(ServerMessage::Snapshot(Box::new(state)));
            }
            self.dirty = true;
        }
        Ok(messages)
    }
    /// Emits coalesced section receipts, baseline answers and at most one aged
    /// missing-manifest request. Repairs are retried every 96 ms; the token bucket
    /// includes all feedback bytes. Lost receipts cannot authorize suppression.
    pub fn feedback(&mut self, now: Duration) -> io::Result<Option<Datagram>> {
        self.budget.advance(now)?;
        let Some(epoch) = self.decoder.epoch else {
            return Ok(None);
        };
        if self.last_feedback.is_some_and(|last| {
            now.saturating_sub(last)
                < if self.dirty {
                    Duration::from_millis(16)
                } else {
                    REPAIR_PERIOD
                }
        }) {
            return Ok(None);
        }
        if self.decoder.pinned.is_some()
            && now.saturating_sub(self.pinned_since) >= Duration::from_secs(2)
        {
            self.decoder.pinned = None;
        }
        if self.decoder.pinned.is_none() {
            let selected = self
                .manifests
                .iter()
                .rev()
                .find_map(|(&id, (manifest, first))| {
                    if id <= self.decoder.completed || now.saturating_sub(*first) < REPAIR_PERIOD {
                        return None;
                    }
                    let missing: [bool; TOPICS] = std::array::from_fn(|i| {
                        !self.decoder.caches[i]
                            .values
                            .contains_key(&manifest.revisions[i])
                    });
                    missing.contains(&true).then_some(manifest.clone())
                });
            if let Some(manifest) = selected {
                self.pinned_since = now;
                self.decoder
                    .pending
                    .insert(manifest.checkpoint, manifest.clone());
                self.decoder.pinned = Some(manifest);
                while self.decoder.pending.len() > WINDOW {
                    let oldest = *self
                        .decoder
                        .pending
                        .keys()
                        .find(|id| self.decoder.pinned.as_ref().unwrap().checkpoint != **id)
                        .unwrap();
                    self.decoder.pending.remove(&oldest);
                }
            }
        }
        let need = self.decoder.pinned.as_ref().map(|manifest| {
            (
                manifest.checkpoint,
                std::array::from_fn(|i| {
                    !self.decoder.caches[i]
                        .values
                        .contains_key(&manifest.revisions[i])
                }),
            )
        });
        let feedback = Feedback {
            epoch,
            completed: self.decoder.completed,
            have: std::array::from_fn(|i| {
                self.decoder.caches[i]
                    .values
                    .last_key_value()
                    .map_or(0, |(&id, _)| id)
            }),
            need,
            baseline: self.answers.iter().cloned().collect(),
        };
        let mut bytes = FEEDBACK.to_vec();
        bytes.extend(crate::compression::compress(&serde_json::to_vec(
            &feedback,
        )?));
        if bytes.len() > MTU {
            return Err(invalid("feedback exceeds datagram cap"));
        }
        if bytes.len() as f64 > self.budget.tokens {
            return Ok(None);
        }
        self.budget.tokens -= bytes.len() as f64;
        self.answers.clear();
        self.last_feedback = Some(now);
        self.dirty = false;
        self.feedback_bytes += bytes.len() as u64;
        self.feedback_packets += 1;
        Ok(Some(Datagram {
            bytes,
            reliable: false,
        }))
    }
    /// Newest capture time in nanoseconds for which this topic's exact value was
    /// available, even if another topic prevented a complete checkpoint.
    pub fn available_time_ns(&self, topic: Topic) -> Option<u64> {
        self.available[topic as usize]
    }
    /// Upstream application bytes/datagrams emitted over this connection.
    pub fn feedback_counts(&self) -> (u64, u64) {
        (self.feedback_bytes, self.feedback_packets)
    }
    /// Serialized section cache bytes, excluding baselines and allocator overhead.
    pub fn cached_bytes(&self) -> usize {
        self.decoder.cached_bytes()
    }
    /// Bytes held in partially reassembled frames, capped at 4 MiB total.
    pub fn partial_bytes(&self) -> usize {
        self.parts.bytes()
    }
    /// Incomplete checkpoint manifests retained, capped at 32.
    pub fn pending(&self) -> usize {
        self.decoder.pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        scripted_link::{Impairment, Link},
        simulation::Simulation,
        snapshot_codec,
    };
    use rm_simulator_world::{ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, Team};
    use std::time::Instant;

    fn state(id: u64) -> SimulationState {
        let config = FieldConfig {
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                spawn: Pose::at([0., 0., 0.3]),
                team: Team::Red,
            }],
            ..Default::default()
        };
        let mut simulation = Simulation::new(Field::new(&config).unwrap(), false);
        simulation.step(id * 32).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = id;
        state
    }
    fn compact(state: &SimulationState) -> ServerMessage {
        snapshot_codec::decode_player_message(&snapshot_codec::encode_player_message(
            &ServerMessage::Snapshot(Box::new(state.clone())),
        ))
        .unwrap()
    }
    // Logical-topic fault injection, not physical packet loss: aggregation means
    // dropping a whole datagram could also drop an unrelated topic's fragment.
    // Physical loss/duplication/reordering is exercised separately by Link.
    fn without_topic(packet: &[u8], topic: Topic) -> Vec<u8> {
        if !packet.starts_with(DATA) {
            return packet.to_vec();
        }
        let mut kept = DATA.to_vec();
        let mut cursor = 4;
        while cursor < packet.len() {
            let class = packet[cursor] as usize;
            let length =
                u16::from_le_bytes(packet[cursor + 17..cursor + 19].try_into().unwrap()) as usize;
            if class != 1 + topic as usize && class != 1 + TOPICS + topic as usize {
                kept.extend_from_slice(&packet[cursor..cursor + HEADER + length]);
            }
            cursor += HEADER + length;
        }
        kept
    }
    fn pump(
        sender: &mut Sender,
        receiver: &mut Receiver,
        link: &mut Link,
        epoch: Instant,
        ms: u64,
        drop: Option<Topic>,
    ) -> Vec<ServerMessage> {
        let now = Duration::from_millis(ms);
        while let Some(mut packet) = sender.next(now).unwrap() {
            assert!(packet.bytes.len() <= MTU);
            if let Some(topic) = drop {
                packet.bytes = without_topic(&packet.bytes, topic);
            }
            if packet.bytes.len() > 4 {
                link.send_to_client(epoch + now, packet);
            }
        }
        let mut messages = Vec::new();
        for packet in link.take_to_client(epoch + now) {
            messages.extend(receiver.receive(&packet, now).unwrap());
        }
        if let Some(feedback) = receiver.feedback(now).unwrap() {
            link.send_to_host(epoch + now, feedback);
        }
        for feedback in link.take_to_host(epoch + now) {
            sender.feedback(&feedback, now).unwrap();
        }
        assert!(sender.queued_bytes() <= QUEUE_LIMIT);
        assert!(sender.history_bytes() <= TOPICS * super::super::CACHE_BYTES);
        assert!(receiver.partial_bytes() <= FRAME_LIMIT);
        assert!(receiver.pending() <= WINDOW);
        messages
    }
    #[test]
    fn all_topics_reassemble_under_loss_reordering_and_blackout() {
        for seed in 1..=5 {
            let epoch = Instant::now();
            let rules = Impairment {
                loss: 0.1,
                duplicate: 0.2,
                reorder_every: 7,
                reorder_hold: 2,
                delay: Duration::from_millis(50),
                jitter: Duration::from_millis(10),
                recovery: Duration::from_millis(100),
                blackout: Some((Duration::ZERO, Duration::from_millis(200))),
                ..Default::default()
            };
            let mut link = Link::new(epoch, seed, rules.clone(), rules);
            let mut sender = Sender::new(40 * 1024);
            let mut receiver = Receiver::new(10 * 1024);
            let source = state(1);
            sender.publish(&source, Duration::ZERO).unwrap();
            let mut snapshots = Vec::new();
            for ms in 1..=2500 {
                snapshots.extend(pump(&mut sender, &mut receiver, &mut link, epoch, ms, None));
            }
            assert_eq!(snapshots, vec![compact(&source)], "seed {seed}");
            assert!(sender.stats().repair_sections > 0);
            assert!(sender.stats().sent_bytes <= 40 * 1024 * 2500 / 1000);
            assert!(receiver.feedback_counts().0 <= 10 * 1024 * 2500 / 1000);
        }
    }
    #[test]
    fn permanently_missing_topic_does_not_starve_chassis_or_grow_memory() {
        for topic in Topic::ALL {
            let epoch = Instant::now();
            let mut link = Link::new(epoch, 1, Impairment::default(), Impairment::default());
            let mut sender = Sender::new(512 * 1024);
            let mut receiver = Receiver::new(10 * 1024);
            for ms in 1..=2400 {
                if ms % 32 == 0 {
                    sender
                        .publish(&state(ms / 32), Duration::from_millis(ms))
                        .unwrap();
                }
                let messages = pump(
                    &mut sender,
                    &mut receiver,
                    &mut link,
                    epoch,
                    ms,
                    Some(topic),
                );
                assert!(
                    messages.is_empty(),
                    "a missing {topic:?} must prevent promotion"
                );
            }
            if topic != Topic::Chassis && topic != Topic::Metadata {
                // Co-aggregation may lose metadata with the selected topic. The
                // chassis itself must still receive service on its own class.
                assert!(sender.stats().topic_service_bytes[Topic::Chassis as usize] > 1000);
            }
            if topic == Topic::Projectiles {
                assert!(receiver.available_time_ns(Topic::Chassis).unwrap() > 2_000_000_000);
            }
            assert!(sender.stats().repair_sections <= 2400 / 96 * TOPICS as u64);
        }
    }
    #[test]
    fn exact_receipts_suppress_only_unchanged_values_in_the_same_epoch() {
        let epoch = Instant::now();
        let mut link = Link::new(epoch, 2, Impairment::default(), Impairment::default());
        let mut sender = Sender::new(512 * 1024);
        let mut receiver = Receiver::new(10 * 1024);
        let mut source = state(1);
        sender.publish(&source, Duration::ZERO).unwrap();
        for ms in 1..=100 {
            pump(&mut sender, &mut receiver, &mut link, epoch, ms, None);
        }
        source.snapshot_id = 2;
        sender.publish(&source, Duration::from_millis(100)).unwrap();
        assert_eq!(sender.stats().suppressed_sections, 8);
        let mut messages = Vec::new();
        for ms in 101..=200 {
            messages.extend(pump(&mut sender, &mut receiver, &mut link, epoch, ms, None));
        }
        assert_eq!(messages, vec![compact(&source)]);
        source.snapshot_id = 3;
        source.input_epoch += 1;
        source.paused = true;
        sender.publish(&source, Duration::from_millis(200)).unwrap();
        assert_eq!(
            sender.stats().suppressed_sections,
            8,
            "new epoch must resend all topics"
        );
        let mut messages = Vec::new();
        for ms in 201..=300 {
            messages.extend(pump(&mut sender, &mut receiver, &mut link, epoch, ms, None));
        }
        assert_eq!(messages, vec![compact(&source)]);
    }
    #[test]
    fn suppressed_revision_eviction_is_repaired_even_after_sender_received_it() {
        let epoch = Instant::now();
        let mut link = Link::new(epoch, 2, Impairment::default(), Impairment::default());
        let mut sender = Sender::new(512 * 1024);
        let mut receiver = Receiver::new(10 * 1024);
        let mut source = state(1);
        sender.publish(&source, Duration::ZERO).unwrap();
        for ms in 1..=100 {
            pump(&mut sender, &mut receiver, &mut link, epoch, ms, None);
        }
        // Exercise explicit cache loss after an acknowledged receipt. A manifest
        // request must repair this revision despite normal delivery suppression.
        receiver.decoder.caches[Topic::Restore as usize] = Cache::default();
        source.snapshot_id = 2;
        sender.publish(&source, Duration::from_millis(100)).unwrap();
        let mut messages = Vec::new();
        for ms in 101..=1000 {
            messages.extend(pump(&mut sender, &mut receiver, &mut link, epoch, ms, None));
        }
        assert_eq!(messages, vec![compact(&source)]);
        assert!(sender.stats().repair_sections > 0);
    }
    #[test]
    fn fragmented_confirmations_precede_pong_despite_periodic_replacement() {
        let epoch = Instant::now();
        let mut link = Link::new(
            epoch,
            4,
            Impairment::default(),
            Impairment {
                loss: 0.1,
                recovery: Duration::from_millis(100),
                ..Default::default()
            },
        );
        let mut sender = Sender::new(40 * 1024);
        let mut receiver = Receiver::new(10 * 1024);
        let confirmation = ServerMessage::Snapshot(Box::new(state(1000)));
        sender.control(&confirmation).unwrap();
        sender.control(&ServerMessage::Pong { nonce: 7 }).unwrap();
        let mut controls = Vec::new();
        for ms in 1..=1500 {
            if ms % 32 == 0 {
                sender
                    .publish(&state(ms / 32), Duration::from_millis(ms))
                    .unwrap();
            }
            for message in pump(&mut sender, &mut receiver, &mut link, epoch, ms, None) {
                if matches!(&message, ServerMessage::Pong { .. }) || message == confirmation {
                    controls.push(message);
                }
            }
        }
        assert_eq!(
            controls,
            vec![confirmation, ServerMessage::Pong { nonce: 7 }]
        );
    }
    #[test]
    fn malformed_and_conflicting_aggregate_fragments_are_rejected() {
        let mut queue = Queue::new(512 * 1024);
        queue.offer(1, vec![5; 2001]).unwrap();
        let packet = queue
            .next(Duration::from_millis(32))
            .unwrap()
            .unwrap()
            .bytes;
        let mut parts = Reassembly::default();
        assert!(parts.receive(&packet, Duration::ZERO).unwrap().is_empty());
        let mut changed = packet.clone();
        *changed.last_mut().unwrap() ^= 1;
        assert!(parts.receive(&changed, Duration::ZERO).is_err());
        assert!(
            Reassembly::default()
                .receive(&packet[..packet.len() - 1], Duration::ZERO)
                .is_err()
        );
        let mut wrong_lane = packet;
        wrong_lane[..4].copy_from_slice(RELIABLE);
        assert!(
            Reassembly::default()
                .receive(&wrong_lane, Duration::ZERO)
                .is_err()
        );
    }

    #[test]
    fn damage_revive_placement_and_join_removal_remain_coherent_under_loss() {
        use rm_simulator_world::{RefereeCommand, RefereeConfig};
        let config = FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                spawn: Pose::at([0., 0., 0.3]),
                team: Team::Red,
            }],
            ..Default::default()
        };
        let mut field = Field::new(&config).unwrap();
        let geometry = field.static_geometry_snapshot();
        let mut states = Vec::new();
        for id in 1..=5 {
            match id {
                2 => {
                    field
                        .referee_command(RefereeCommand::DamageRobot {
                            robot: 0,
                            amount: 200,
                        })
                        .unwrap();
                    field
                        .referee_command(RefereeCommand::SetOutpostHp { outpost: 0, hp: 0 })
                        .unwrap();
                }
                3 => {
                    field
                        .referee_command(RefereeCommand::ReviveRobot { robot: 0 })
                        .unwrap();
                    field.place_chassis(0, Pose::at([2., 0., 0.3])).unwrap();
                }
                4 => {
                    field.remove_chassis(0).unwrap();
                }
                5 => {
                    assert_eq!(field.add_chassis(&config.chassis[0]).unwrap(), 1);
                }
                _ => {}
            }
            field.step(32).unwrap();
            states.push(SimulationState {
                bots: vec![],
                snapshot_id: id,
                input_epoch: 0,
                shot_results: vec![],
                paused: false,
                field: field.snapshot(),
            });
        }
        let epoch = Instant::now();
        let rules = Impairment {
            loss: 0.05,
            delay: Duration::from_millis(20),
            reorder_every: 9,
            reorder_hold: 2,
            recovery: Duration::from_millis(100),
            ..Default::default()
        };
        let mut link = Link::new(epoch, 19, rules.clone(), rules);
        let mut sender = Sender::new(40 * 1024);
        let mut receiver = Receiver::new(10 * 1024);
        let mut latest = 0;
        for ms in 1..=2500 {
            if ms <= 160 && ms % 32 == 0 {
                sender
                    .publish(&states[ms as usize / 32 - 1], Duration::from_millis(ms))
                    .unwrap();
            }
            for message in pump(&mut sender, &mut receiver, &mut link, epoch, ms, None) {
                let ServerMessage::Snapshot(state) = &message else {
                    panic!("snapshot expected")
                };
                assert!(state.snapshot_id > latest);
                latest = state.snapshot_id;
                assert_eq!(message, compact(&states[latest as usize - 1]));
                Field::restore(&state.field, &geometry, 0.)
                    .unwrap()
                    .step(8)
                    .unwrap();
            }
        }
        assert_eq!(latest, 5, "final lifecycle checkpoint must recover");
    }

    #[test]
    fn one_impaired_peer_does_not_delay_a_healthy_peer() {
        let epoch = Instant::now();
        let mut healthy_link = Link::new(epoch, 1, Impairment::default(), Impairment::default());
        let mut slow_link = Link::new(
            epoch,
            1,
            Impairment::default(),
            Impairment {
                blackout: Some((Duration::ZERO, Duration::from_secs(2))),
                ..Default::default()
            },
        );
        let mut healthy = Sender::new(512 * 1024);
        let mut slow = Sender::new(40 * 1024);
        let mut healthy_client = Receiver::new(10 * 1024);
        let mut slow_client = Receiver::new(10 * 1024);
        let mut delivered = 0;
        for ms in 1..=1600 {
            if ms % 32 == 0 {
                let source = state(ms / 32);
                healthy.publish(&source, Duration::from_millis(ms)).unwrap();
                slow.publish(&source, Duration::from_millis(ms)).unwrap();
            }
            delivered += pump(
                &mut healthy,
                &mut healthy_client,
                &mut healthy_link,
                epoch,
                ms,
                None,
            )
            .len();
            assert!(pump(&mut slow, &mut slow_client, &mut slow_link, epoch, ms, None).is_empty());
        }
        assert!(delivered >= 49);
    }
}

#[cfg(test)]
mod trials;
