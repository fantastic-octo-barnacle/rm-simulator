// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The per-peer UDP codec, with no socket in it.
//!
//! One peer's whole wire behaviour lives here: the RMG1 fragment framing, the
//! RMI3 input batches, the RMC1 compressed commands, the RMO4 owner anchor, the
//! RMA1 baseline feedback, snapshot delta encoding and the byte pacer. The GNS
//! reactor in `gns_transport` is a thin wrapper that moves datagrams between a
//! socket and these structs; a test drives the same structs over a scripted
//! link. Every deadline is an explicit `now: Instant`, so neither side reads
//! the host clock on its own.
use crate::host::{HostHandle, Outbound, PeerRegistration};
use crate::lifecycle::{ConnectionStop, Stop};
use crate::net::QueuedCommand;
use crate::net::outbox;
use crate::pacing::{Datagram, Pacer};
use crate::protocol::{ClientMessage, Command, Role, ServerMessage, Welcome};
use crate::udp_snapshot::Feedback;
use rm_simulator_world::{ChassisConfig, Team};
use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

/// Application bytes carried by one RMG1 fragment. 1,000 bytes keeps every
/// datagram below the path MTU.
const CHUNK: usize = 1000;
/// Bytes of RMG1 header before each fragment: magic, lane byte, revision, total
/// application length and fragment index.
const HEADER: usize = 21;
/// Largest application frame any transport may carry, taken from the protocol's
/// line cap.
const MAX_WIRE: usize = crate::protocol::MAX_LINE_BYTES;
/// Unreliable partial frames older than this are abandoned. A reliable frame
/// waits for its retransmission instead.
const FRAME_LIFETIME: Duration = Duration::from_millis(250);
/// First four bytes of every RMG1 fragment.
const MAGIC: &[u8; 4] = b"RMG1";
/// First four bytes of a deflated command payload, which marks the unreliable
/// shot-retry lane.
const COMMAND_MAGIC: &[u8; 4] = b"RMC1";
/// First four bytes of a deflated compact pilot input batch. One shared batch
/// header carries the chassis, input epoch and placement revision, and every
/// frame carries relative sequence and sampled-time fields plus an exact
/// changed-value mask over the five command values.
const INPUT_BATCH_MAGIC: &[u8; 4] = b"RMI3";
/// First four bytes of the older fixed pilot input batch, which put all 80 bytes
/// of every frame on the wire. The decoder still accepts it.
const INPUT_BATCH_MAGIC_V2: &[u8; 4] = b"RMI2";
/// Bytes of one fixed input frame: chassis, input epoch, sequence, sampled time,
/// placement revision, duration and five f64 command values.
const INPUT_FRAME_BYTES: usize = 80;
/// Most frames one batch may carry.
const MAX_INPUT_FRAMES: usize = 12;
/// Bytes of the shared RMI3 batch header: chassis, input epoch and placement
/// revision, which every compact frame in the batch inherits.
const INPUT_HEADER_BYTES: usize = 20;
/// Per-frame tag for a frame that keeps the fixed 80-byte encoding.
const INPUT_FRAME_LEGACY: u8 = 0x00;
/// Per-frame tag for a frame that takes its identity from the shared header.
const INPUT_FRAME_COMPACT: u8 = 0x01;
/// Compact-frame flag: the sequence is an absolute u64, not a relative delta.
const INPUT_SEQUENCE_ABSOLUTE: u8 = 0x01;
/// Compact-frame flag: the sampled time is an absolute u64, not a relative delta.
const INPUT_TIME_ABSOLUTE: u8 = 0x02;
/// Largest inflated RMI3 body: the count, the shared header and twelve frames
/// that each fall back to the fixed encoding. It bounds a hostile peer's
/// allocation and proves the body still fits one datagram.
const INPUT_BATCH_BODY_LIMIT: usize =
    1 + INPUT_HEADER_BYTES + MAX_INPUT_FRAMES * (1 + INPUT_FRAME_BYTES);
/// Ceiling for inflating one compressed client command, so a hostile peer cannot
/// force a larger allocation.
const INPUT_BATCH_LIMIT: usize = 16 * 1024;
/// Host frames queued past this native backlog skip their periodic checkpoint.
pub(crate) const CONGESTED_PENDING_BYTES: u32 = 64 * 1024;
/// Owner configurations one client keeps, so a received anchor can resolve the
/// revision it names. Oldest first; a connection changes configuration rarely.
const CONFIG_CACHE: usize = 4;
/// Owner-configuration feedback one client queues before the oldest is dropped.
const CONFIG_FEEDBACK: usize = 8;
/// Resend an unacknowledged owner configuration after this many anchor
/// opportunities, mirroring the baseline `Retire` schedule. At the 32 ms
/// broadcast period that is about half a second between attempts.
const CONFIG_RESEND_FRAMES: u64 = 16;

/// Host-side owner-configuration handshake for one peer.
///
/// An anchor may name a revision only once the peer answered
/// [`Feedback::ConfigStored`] for it; until then the configuration rides the
/// reliable control lane and is resent on the same bounded schedule as a
/// baseline retirement. The identity is a pure function of the configuration,
/// so a changed configuration always starts a new handshake and can never be
/// silently reused.
#[derive(Default)]
struct OwnerConfigSender {
    /// The configuration the peer acknowledged, if any.
    acked: Option<crate::owner_stream::ConfigRevision>,
    /// The configuration currently being offered, with its framed bytes.
    pending: Option<(crate::owner_stream::ConfigRevision, Vec<u8>)>,
    /// Anchor opportunities since the pending frame was last sent.
    age: u64,
}
impl OwnerConfigSender {
    /// True when an anchor may safely name `revision`.
    fn acknowledged(&self, revision: crate::owner_stream::ConfigRevision) -> bool {
        self.acked == Some(revision) && self.pending.is_none()
    }
    /// The framed owner-configuration message for `config`.
    fn frame(config: &ChassisConfig) -> io::Result<Vec<u8>> {
        let message = ServerMessage::OwnerConfig(Box::new(crate::owner_stream::OwnerConfig::new(
            config.clone(),
        )));
        Ok(miniz_oxide::deflate::compress_to_vec(
            &crate::snapshot_codec::encode_player_message(&message),
            1,
        ))
    }
    /// Offers `config` and returns the frame to queue now, or `None` while the
    /// current frame is between bounded resends. A new revision always queues
    /// immediately and discards the superseded frame.
    fn offer(
        &mut self,
        revision: crate::owner_stream::ConfigRevision,
        config: &ChassisConfig,
    ) -> io::Result<Option<Vec<u8>>> {
        match &self.pending {
            Some((pending, _)) if *pending == revision => {
                self.age += 1;
                if self.age < CONFIG_RESEND_FRAMES {
                    return Ok(None);
                }
                self.age = 0;
                Ok(self.pending.as_ref().map(|(_, bytes)| bytes.clone()))
            }
            _ => {
                let bytes = Self::frame(config)?;
                self.pending = Some((revision, bytes.clone()));
                self.age = 0;
                Ok(Some(bytes))
            }
        }
    }
    /// Applies one peer answer, returning true when it belongs to the
    /// configuration handshake. An acknowledgement for a revision that is not
    /// pending is ignored, so a delayed answer cannot unlock a changed one.
    fn answer(&mut self, feedback: Feedback) -> bool {
        match feedback {
            Feedback::ConfigStored { revision } => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|(pending, _)| pending.raw() == revision)
                {
                    self.acked = Some(crate::owner_stream::ConfigRevision::from_raw(revision));
                    self.pending = None;
                    self.age = 0;
                }
                true
            }
            Feedback::ConfigMissing { revision } => {
                // Bring the next anchor opportunity forward to a resend.
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|(pending, _)| pending.raw() == revision)
                {
                    self.age = CONFIG_RESEND_FRAMES;
                }
                true
            }
            _ => false,
        }
    }
}

/// Wraps a displayable failure as `io::Error::other`, so codec errors keep
/// their message on the crate's io path.
pub(crate) fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
/// Keep the newest four samples plus older movement transitions, within one
/// 1,000-byte packet and the host's 250 ms useful-input window.
fn select_inputs(history: &VecDeque<Command>, limit: usize) -> VecDeque<Command> {
    let Some(Command::PilotInput {
        chassis,
        frame: newest,
    }) = history.back()
    else {
        return VecDeque::new();
    };
    let useful: Vec<_> = history.iter().filter(|c| matches!(c, Command::PilotInput { chassis: id, frame }
        if id == chassis && frame.input_epoch == newest.input_epoch && frame.placement_revision == newest.placement_revision
        && newest.sampled_time_ns.saturating_sub(frame.sampled_time_ns) <= crate::input_stream::INPUT_LEASE_NS)).copied().collect();
    let mut chosen: Vec<usize> = (useful.len().saturating_sub(4)..useful.len()).collect();
    for i in (1..useful.len().saturating_sub(4)).rev() {
        // Body-relative commands drift as the gimbal/chassis turns. Retain
        // direction/press/release changes, not every tiny steering correction.
        let direction = |v: f64| {
            if v.abs() < 0.1 {
                0
            } else if v > 0. {
                1
            } else {
                -1
            }
        };
        let movement = |c: Command| match c {
            Command::PilotInput { frame, .. } => (
                direction(frame.command.forward_m_s),
                direction(frame.command.left_m_s),
                direction(frame.command.yaw_rate_rad_s),
            ),
            _ => unreachable!(),
        };
        if chosen.len() < limit && movement(useful[i]) != movement(useful[i - 1]) {
            chosen.push(i);
        }
    }
    chosen.sort_unstable();
    chosen.into_iter().map(|i| useful[i]).collect()
}

/// The five command values in wire order.
fn command_values(command: &rm_simulator_world::ChassisCommand) -> [f64; 5] {
    [
        command.forward_m_s,
        command.left_m_s,
        command.yaw_rate_rad_s,
        command.aim_yaw_rad,
        command.aim_pitch_rad,
    ]
}
/// The five command values as raw bits. A change mask compares bits, so `-0.0`
/// is not mistaken for `0.0` and a NaN payload round-trips unchanged.
fn command_bits(command: &rm_simulator_world::ChassisCommand) -> [u64; 5] {
    command_values(command).map(f64::to_bits)
}
/// The fixed 80-byte frame encoding this codec used before the compact batch:
/// chassis, input epoch, sequence, sampled time, placement revision, duration
/// and five f64 command values. A compact batch keeps it as the exact fallback
/// for a frame whose identity is not the shared header's, and the decoder still
/// reads the older `RMI2` batch that used it for every frame.
fn fixed_frame(chassis: u32, frame: &crate::input_stream::InputFrame) -> [u8; INPUT_FRAME_BYTES] {
    let mut bytes = [0; INPUT_FRAME_BYTES];
    bytes[..4].copy_from_slice(&chassis.to_le_bytes());
    bytes[4..12].copy_from_slice(&frame.input_epoch.to_le_bytes());
    bytes[12..20].copy_from_slice(&frame.sequence.to_le_bytes());
    bytes[20..28].copy_from_slice(&frame.sampled_time_ns.to_le_bytes());
    bytes[28..36].copy_from_slice(&frame.placement_revision.to_le_bytes());
    bytes[36..40].copy_from_slice(&frame.duration_ticks.to_le_bytes());
    for (index, value) in command_values(&frame.command).into_iter().enumerate() {
        let start = 40 + index * 8;
        bytes[start..start + 8].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
/// Whether `payload` is a pilot input batch this codec can decode, in either the
/// compact `RMI3` or the older fixed `RMI2` framing.
fn is_input_batch(payload: &[u8]) -> bool {
    payload.starts_with(INPUT_BATCH_MAGIC) || payload.starts_with(INPUT_BATCH_MAGIC_V2)
}
/// Writes `value` as an unsigned LEB128 varint, which is exact for every u64.
fn write_varint(bytes: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        bytes.push(value as u8 | 0x80);
        value >>= 7;
    }
    bytes.push(value as u8);
}
/// Reads one unsigned LEB128 varint, refusing a truncated or overflowing one.
fn read_varint(bytes: &[u8], cursor: &mut usize) -> io::Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| io_error("truncated input batch"))?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err(io_error("input batch varint overflow"));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(io_error("input batch varint overflow"))
}
/// Reads one little-endian u64 at `cursor` and advances it.
fn read_u64(bytes: &[u8], cursor: &mut usize) -> io::Result<u64> {
    let slice = bytes
        .get(*cursor..*cursor + 8)
        .ok_or_else(|| io_error("truncated input batch"))?;
    *cursor += 8;
    Ok(u64::from_le_bytes(slice.try_into().unwrap()))
}
/// Decodes one fixed 80-byte frame. Refuses a non-finite command.
fn decode_fixed_frame(frame: &[u8]) -> io::Result<Command> {
    let u64_at = |i| u64::from_le_bytes(frame[i..i + 8].try_into().unwrap());
    let f64_at = |i| f64::from_le_bytes(frame[i..i + 8].try_into().unwrap());
    let command = rm_simulator_world::ChassisCommand {
        forward_m_s: f64_at(40),
        left_m_s: f64_at(48),
        yaw_rate_rad_s: f64_at(56),
        aim_yaw_rad: f64_at(64),
        aim_pitch_rad: f64_at(72),
    };
    if !command.is_finite() {
        return Err(io_error("non-finite input"));
    }
    Ok(Command::PilotInput {
        chassis: u32::from_le_bytes(frame[..4].try_into().unwrap()),
        frame: crate::input_stream::InputFrame {
            input_epoch: u64_at(4),
            sequence: u64_at(12),
            sampled_time_ns: u64_at(20),
            placement_revision: u64_at(28),
            duration_ticks: u32::from_le_bytes(frame[36..40].try_into().unwrap()),
            command,
        },
    })
}
/// Encodes an input batch as `RMI3` followed by a deflate stream: one count byte,
/// one header holding the values the newest frame shares (chassis, input epoch
/// and placement revision), then one entry per frame.
///
/// A compact frame carries a change mask over the five command values, a
/// relative sequence delta, a relative sampled-time delta and a duration
/// varint; only changed values ride at full f64 precision. A frame whose
/// identity is not the header's, or whose delta cannot be represented exactly,
/// keeps its fixed 80-byte encoding, so decoding returns the same `InputFrame`
/// the fixed encoding produces, bit for bit.
///
/// Refuses an empty batch, more than [`MAX_INPUT_FRAMES`] frames, any command
/// that is not pilot input and any packet that would need more than the
/// 1,000-byte single-datagram budget.
fn input_batch(inputs: &VecDeque<Command>) -> io::Result<Vec<u8>> {
    if inputs.is_empty() || inputs.len() > MAX_INPUT_FRAMES {
        return Err(io_error("invalid input count"));
    }
    let Some(Command::PilotInput {
        chassis,
        frame: newest,
    }) = inputs.back()
    else {
        return Err(io_error("non-pilot input"));
    };
    let (header_chassis, header_epoch, header_revision) =
        (*chassis, newest.input_epoch, newest.placement_revision);
    let mut bytes = vec![inputs.len() as u8];
    bytes.extend(header_chassis.to_le_bytes());
    bytes.extend(header_epoch.to_le_bytes());
    bytes.extend(header_revision.to_le_bytes());
    let mut previous_sequence = None;
    let mut previous_time = None;
    let mut previous_bits = None;
    for input in inputs {
        let Command::PilotInput { chassis, frame } = input else {
            return Err(io_error("non-pilot input"));
        };
        if *chassis == header_chassis
            && frame.input_epoch == header_epoch
            && frame.placement_revision == header_revision
        {
            let bits = command_bits(&frame.command);
            let mut mask = 0u8;
            for (index, bit) in bits.iter().enumerate() {
                if previous_bits.is_none_or(|before: [u64; 5]| before[index] != *bit) {
                    mask |= 1 << index;
                }
            }
            // Frame zero has no in-batch base, and a delta that would move
            // backwards is not representable, so those fields go absolute.
            let sequence = previous_sequence.and_then(|before| frame.sequence.checked_sub(before));
            let sampled_time =
                previous_time.and_then(|before| frame.sampled_time_ns.checked_sub(before));
            let mut flags = 0u8;
            if sequence.is_none() {
                flags |= INPUT_SEQUENCE_ABSOLUTE;
            }
            if sampled_time.is_none() {
                flags |= INPUT_TIME_ABSOLUTE;
            }
            bytes.push(INPUT_FRAME_COMPACT);
            bytes.push(mask);
            bytes.push(flags);
            match sequence {
                Some(delta) => write_varint(&mut bytes, delta),
                None => bytes.extend(frame.sequence.to_le_bytes()),
            }
            match sampled_time {
                Some(delta) => write_varint(&mut bytes, delta),
                None => bytes.extend(frame.sampled_time_ns.to_le_bytes()),
            }
            write_varint(&mut bytes, u64::from(frame.duration_ticks));
            for (index, bit) in bits.iter().enumerate() {
                if mask & (1 << index) != 0 {
                    bytes.extend(bit.to_le_bytes());
                }
            }
        } else {
            // Another life or epoch: keep the fixed encoding for this frame
            // rather than lose the values the shared header cannot carry.
            bytes.push(INPUT_FRAME_LEGACY);
            bytes.extend_from_slice(&fixed_frame(*chassis, frame));
        }
        previous_sequence = Some(frame.sequence);
        previous_time = Some(frame.sampled_time_ns);
        previous_bits = Some(command_bits(&frame.command));
    }
    let mut packet = INPUT_BATCH_MAGIC.to_vec();
    packet.extend(miniz_oxide::deflate::compress_to_vec(&bytes, 1));
    if packet.len() > CHUNK {
        return Err(io_error("input batch exceeds one datagram"));
    }
    Ok(packet)
}
/// Decodes a pilot input batch in encoded order, oldest first, in either the
/// compact `RMI3` or the older fixed `RMI2` framing. Refuses a missing magic, a
/// packet over the single-datagram budget, an inflated body over
/// [`INPUT_BATCH_BODY_LIMIT`], a count outside 1..=12, a reserved mask or flag
/// bit, a delta or command value with no in-batch base, a truncated frame, a
/// trailing byte and any non-finite command.
fn decode_inputs(packet: &[u8]) -> io::Result<Vec<Command>> {
    if packet.len() > CHUNK {
        return Err(io_error("input batch exceeds one datagram"));
    }
    if let Some(compressed) = packet.strip_prefix(INPUT_BATCH_MAGIC) {
        return decode_compact_inputs(compressed);
    }
    let compressed = packet
        .strip_prefix(INPUT_BATCH_MAGIC_V2)
        .ok_or_else(|| io_error("invalid input batch"))?;
    let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(
        compressed,
        1 + MAX_INPUT_FRAMES * INPUT_FRAME_BYTES,
    )
    .map_err(io_error)?;
    let count = bytes.first().copied().unwrap_or(0) as usize;
    if !(1..=MAX_INPUT_FRAMES).contains(&count) || bytes.len() != 1 + count * INPUT_FRAME_BYTES {
        return Err(io_error("invalid input batch length"));
    }
    bytes[1..]
        .as_chunks::<INPUT_FRAME_BYTES>()
        .0
        .iter()
        .map(|frame| decode_fixed_frame(frame))
        .collect()
}
/// Decodes the `RMI3` body: one count byte, the shared identity header, then one
/// entry per count. A compact frame inherits the header's identity and reuses the
/// previous frame's sequence, sampled time and unchanged command values; a legacy
/// frame carries all 80 bytes of its own.
fn decode_compact_inputs(compressed: &[u8]) -> io::Result<Vec<Command>> {
    let bytes =
        miniz_oxide::inflate::decompress_to_vec_with_limit(compressed, INPUT_BATCH_BODY_LIMIT)
            .map_err(io_error)?;
    let count = bytes.first().copied().unwrap_or(0) as usize;
    if !(1..=MAX_INPUT_FRAMES).contains(&count) {
        return Err(io_error("invalid input count"));
    }
    let mut cursor = 1;
    let header = bytes
        .get(cursor..cursor + INPUT_HEADER_BYTES)
        .ok_or_else(|| io_error("invalid input header"))?;
    cursor += INPUT_HEADER_BYTES;
    let header_chassis = u32::from_le_bytes(header[..4].try_into().unwrap());
    let header_epoch = u64::from_le_bytes(header[4..12].try_into().unwrap());
    let header_revision = u64::from_le_bytes(header[12..20].try_into().unwrap());
    let mut previous_sequence: Option<u64> = None;
    let mut previous_time: Option<u64> = None;
    let mut previous_bits: Option<[u64; 5]> = None;
    let mut inputs = Vec::with_capacity(count);
    for _ in 0..count {
        let tag = *bytes
            .get(cursor)
            .ok_or_else(|| io_error("truncated input batch"))?;
        cursor += 1;
        let (chassis, frame) = match tag {
            INPUT_FRAME_LEGACY => {
                let frame = bytes
                    .get(cursor..cursor + INPUT_FRAME_BYTES)
                    .ok_or_else(|| io_error("truncated input frame"))?;
                cursor += INPUT_FRAME_BYTES;
                let Command::PilotInput { chassis, frame } = decode_fixed_frame(frame)? else {
                    unreachable!()
                };
                (chassis, frame)
            }
            INPUT_FRAME_COMPACT => {
                let mask = *bytes
                    .get(cursor)
                    .ok_or_else(|| io_error("truncated input frame"))?;
                cursor += 1;
                let flags = *bytes
                    .get(cursor)
                    .ok_or_else(|| io_error("truncated input frame"))?;
                cursor += 1;
                if mask & !0x1f != 0 {
                    return Err(io_error("invalid input mask"));
                }
                if flags & !(INPUT_SEQUENCE_ABSOLUTE | INPUT_TIME_ABSOLUTE) != 0 {
                    return Err(io_error("invalid input flags"));
                }
                let sequence = if flags & INPUT_SEQUENCE_ABSOLUTE != 0 {
                    read_u64(&bytes, &mut cursor)?
                } else {
                    previous_sequence
                        .ok_or_else(|| io_error("missing input sequence base"))?
                        .checked_add(read_varint(&bytes, &mut cursor)?)
                        .ok_or_else(|| io_error("input sequence overflow"))?
                };
                let sampled_time_ns = if flags & INPUT_TIME_ABSOLUTE != 0 {
                    read_u64(&bytes, &mut cursor)?
                } else {
                    previous_time
                        .ok_or_else(|| io_error("missing input time base"))?
                        .checked_add(read_varint(&bytes, &mut cursor)?)
                        .ok_or_else(|| io_error("input time overflow"))?
                };
                let duration_ticks = u32::try_from(read_varint(&bytes, &mut cursor)?)
                    .map_err(|_| io_error("invalid input duration"))?;
                if previous_bits.is_none() && mask != 0x1f {
                    return Err(io_error("missing input command base"));
                }
                let mut bits = previous_bits.unwrap_or([0; 5]);
                for (index, bit) in bits.iter_mut().enumerate() {
                    if mask & (1 << index) != 0 {
                        *bit = read_u64(&bytes, &mut cursor)?;
                    }
                }
                let command = rm_simulator_world::ChassisCommand {
                    forward_m_s: f64::from_bits(bits[0]),
                    left_m_s: f64::from_bits(bits[1]),
                    yaw_rate_rad_s: f64::from_bits(bits[2]),
                    aim_yaw_rad: f64::from_bits(bits[3]),
                    aim_pitch_rad: f64::from_bits(bits[4]),
                };
                if !command.is_finite() {
                    return Err(io_error("non-finite input"));
                }
                (
                    header_chassis,
                    crate::input_stream::InputFrame {
                        input_epoch: header_epoch,
                        sequence,
                        sampled_time_ns,
                        duration_ticks,
                        placement_revision: header_revision,
                        command,
                    },
                )
            }
            _ => return Err(io_error("invalid input frame tag")),
        };
        previous_sequence = Some(frame.sequence);
        previous_time = Some(frame.sampled_time_ns);
        previous_bits = Some(command_bits(&frame.command));
        inputs.push(Command::PilotInput { chassis, frame });
    }
    if cursor != bytes.len() {
        return Err(io_error("invalid input batch length"));
    }
    Ok(inputs)
}
/// Every packet fits comfortably below the path MTU. GNS supplies encryption,
/// congestion control and reliability; this header only assembles application
/// frames and rejects stale state. Each connection has a fresh revision space.
pub(crate) fn packets(revision: u64, reliable: bool, bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    if bytes.is_empty() || bytes.len() > MAX_WIRE {
        return Err(io_error("frame size exceeds limit"));
    }
    Ok(bytes
        .chunks(CHUNK)
        .enumerate()
        .map(|(index, chunk)| {
            let mut packet = Vec::with_capacity(HEADER + chunk.len());
            packet.extend_from_slice(MAGIC);
            packet.push(u8::from(reliable));
            packet.extend_from_slice(&revision.to_le_bytes());
            packet.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            packet.extend_from_slice(&(index as u32).to_le_bytes());
            packet.extend_from_slice(chunk);
            packet
        })
        .collect())
}
/// One RMG1 application frame still missing fragments, with the time its first
/// fragment arrived for the lifetime check.
struct PartialFrame {
    revision: u64,
    reliable: bool,
    started: Instant,
    bytes: Vec<u8>,
    received: Vec<bool>,
    remaining: usize,
}
/// Reassembles RMG1 fragments for one peer and decodes the completed
/// application frame. Keeps the newest snapshot id it delivered and the
/// delivery counters the network overlay reports.
#[derive(Default)]
pub(crate) struct Frames {
    decoder: crate::udp_snapshot::Decoder,
    feedback: VecDeque<crate::udp_snapshot::Feedback>,
    pending: VecDeque<PartialFrame>,
    latest_snapshot: u64,
    incomplete_frames: u64,
    stale_updates: u64,
    complete_snapshots: u64,
}
impl Frames {
    /// Accepts one RMG1 fragment at `now`. Returns the decoded message once the
    /// frame is complete, `None` while fragments are missing, for a snapshot no
    /// newer than the last one delivered and for a baseline retirement that
    /// carries no message. Errors on a malformed header, an oversize frame,
    /// inconsistent fragments, a reliable reassembly overflow, a control message
    /// on the unreliable lane and any failed baseline or player-message decode.
    pub(crate) fn receive(
        &mut self,
        packet: &[u8],
        now: Instant,
    ) -> io::Result<Option<ServerMessage>> {
        if packet.len() < HEADER || &packet[..4] != MAGIC || packet[4] > 1 {
            return Err(io_error("invalid GNS frame header"));
        }
        let reliable = packet[4] == 1;
        let revision = u64::from_le_bytes(packet[5..13].try_into().unwrap());
        let length = u32::from_le_bytes(packet[13..17].try_into().unwrap()) as usize;
        let index = u32::from_le_bytes(packet[17..21].try_into().unwrap()) as usize;
        let count = length.div_ceil(CHUNK);
        if revision == 0
            || length == 0
            || length > MAX_WIRE
            || index >= count
            || packet.len() - HEADER != (length - index * CHUNK).min(CHUNK)
        {
            return Err(io_error("invalid GNS frame size"));
        }
        let previous = self.pending.len();
        self.pending
            .retain(|frame| frame.reliable || now.duration_since(frame.started) < FRAME_LIFETIME);
        self.incomplete_frames += (previous - self.pending.len()) as u64;
        let position =
            if let Some(position) = self.pending.iter().position(|f| f.revision == revision) {
                position
            } else {
                while self.pending.len() >= 4
                    || self.pending.iter().map(|f| f.bytes.len()).sum::<usize>() + length > MAX_WIRE
                {
                    if let Some(index) = self.pending.iter().position(|f| !f.reliable) {
                        self.pending.remove(index);
                        self.incomplete_frames += 1;
                    } else if reliable {
                        return Err(io_error("reliable reassembly overflow"));
                    } else {
                        return Ok(None);
                    }
                }
                self.pending.push_back(PartialFrame {
                    revision,
                    reliable,
                    started: now,
                    bytes: vec![0; length],
                    received: vec![false; count],
                    remaining: count,
                });
                self.pending.len() - 1
            };
        let frame = &mut self.pending[position];
        if frame.bytes.len() != length || frame.reliable != reliable {
            return Err(io_error("inconsistent GNS fragments"));
        }
        if !frame.received[index] {
            frame.bytes[index * CHUNK..index * CHUNK + packet.len() - HEADER]
                .copy_from_slice(&packet[HEADER..]);
            frame.received[index] = true;
            frame.remaining -= 1;
        }
        if frame.remaining != 0 {
            return Ok(None);
        }
        let frame = self.pending.remove(position).unwrap();
        let json = miniz_oxide::inflate::decompress_to_vec_with_limit(&frame.bytes, MAX_WIRE)
            .map_err(io_error)?;
        let message = if let Some(wire) = crate::udp_snapshot::parse(&json)? {
            if !reliable && matches!(wire, crate::udp_snapshot::Wire::Retire { .. }) {
                return Err(io_error("unreliable baseline retirement"));
            }
            let (message, feedback) = self.decoder.receive(wire)?;
            if let Some(feedback) = feedback {
                if self.feedback.len() >= 16 {
                    self.feedback.pop_front();
                }
                self.feedback.push_back(feedback);
            }
            let Some(message) = message else {
                return Ok(None);
            };
            message
        } else {
            crate::snapshot_codec::decode_player_message(&json)?
        };
        if !reliable && !matches!(message, ServerMessage::Snapshot(_)) {
            return Err(io_error("unreliable control message"));
        }
        if let ServerMessage::Snapshot(state) = &message {
            if state.snapshot_id <= self.latest_snapshot {
                self.stale_updates += 1;
                return Ok(None);
            }
            self.latest_snapshot = state.snapshot_id;
            self.complete_snapshots += 1;
        }
        Ok(Some(message))
    }
}

/// What one client datagram turned out to be, once framing, baseline feedback
/// and input batches have been handled.
pub(crate) enum PeerRequest {
    /// Baseline feedback, or a fragment that completed nothing. Nothing to do.
    Handled,
    /// One datagram's worth of pilot input frames, newest last.
    Inputs(Vec<Command>),
    /// A decoded client message that is not pilot input.
    Message(Box<ClientMessage>),
}

/// The host's half of one peer's wire: framing in, paced datagrams out.
/// Holds no socket and reads no clock; callers supply `now`.
pub(crate) struct PeerCodec {
    admitted: Instant,
    chassis: Option<u32>,
    joined: bool,
    revision: u64,
    pacer: Pacer,
    encoder: crate::udp_snapshot::Encoder,
    full_checkpoints: bool,
    encoding: crate::network_stats::EncodingStats,
    owner_config: OwnerConfigSender,
}
impl PeerCodec {
    /// Creates the host codec for a peer the carrier admitted at `admitted`.
    /// `rate_bytes_per_s` is the pacing budget and `full_checkpoints` disables
    /// baseline deltas, which reproduces a client without the delta scheme.
    pub(crate) fn new(admitted: Instant, rate_bytes_per_s: u32, full_checkpoints: bool) -> Self {
        Self {
            admitted,
            chassis: None,
            joined: false,
            revision: 0,
            pacer: Pacer::new(rate_bytes_per_s),
            encoder: crate::udp_snapshot::Encoder::default(),
            full_checkpoints,
            encoding: Default::default(),
            owner_config: OwnerConfigSender::default(),
        }
    }
    /// When the carrier accepted this peer; the pacer and the hello timeout both
    /// measure from here.
    pub(crate) fn admitted(&self) -> Instant {
        self.admitted
    }
    /// Record the seat this peer was given. The owner anchor names its chassis.
    pub(crate) fn joined(&mut self, chassis: Option<u32>) {
        self.joined = true;
        self.chassis = chassis;
    }
    /// Time since admission, which is the pacer's time base.
    fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.admitted)
    }
    /// Allocates the next RMG1 revision for one frame. Revisions order frames
    /// within a connection and never reset while it lives.
    fn next_revision(&mut self) -> io::Result<u64> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| io_error("revision overflow"))?;
        Ok(self.revision)
    }
    /// Frames `bytes` under a fresh revision and queues them on the reliable
    /// control class, preserving the application order the caller needs.
    fn queue_control(&mut self, now: Instant, bytes: &[u8]) -> io::Result<()> {
        let revision = self.next_revision()?;
        self.pacer
            .control(self.elapsed(now), packets(revision, true, bytes)?)
            .map_err(io_error)
    }
    /// Decode one datagram from this peer.
    pub(crate) fn receive(&mut self, payload: &[u8], now: Instant) -> io::Result<PeerRequest> {
        if payload.len() > 64 * 1024 {
            return Err(io_error("client message too large"));
        }
        if payload.starts_with(crate::udp_snapshot::ACK_MAGIC) {
            if !self.joined {
                return Err(io_error("expected hello"));
            }
            let feedback = crate::udp_snapshot::Feedback::decode(payload)?;
            // The owner-configuration handshake owns its two answers; every
            // other answer belongs to the baseline encoder.
            if !self.owner_config.answer(feedback)
                && let Some(retire) = self.encoder.feedback(feedback)
            {
                let bytes = crate::udp_snapshot::encode(retire);
                self.queue_control(now, &bytes)?;
            }
            return Ok(PeerRequest::Handled);
        }
        if is_input_batch(payload) {
            if !self.joined {
                return Err(io_error("expected hello"));
            }
            return Ok(PeerRequest::Inputs(decode_inputs(payload)?));
        }
        let payload = if let Some(bytes) = payload.strip_prefix(COMMAND_MAGIC) {
            miniz_oxide::inflate::decompress_to_vec_with_limit(bytes, INPUT_BATCH_LIMIT)
                .map_err(io_error)?
        } else {
            payload.to_vec()
        };
        Ok(PeerRequest::Message(Box::new(
            serde_json::from_slice(&payload).map_err(io_error)?,
        )))
    }
    /// Queue one host frame. `pending_bytes` is the carrier's own backlog; a
    /// congested carrier skips the world checkpoint but never the owner anchor.
    pub(crate) fn send(
        &mut self,
        frame: &Outbound,
        now: Instant,
        pending_bytes: u32,
    ) -> io::Result<()> {
        // The owner anchor is a single replaceable datagram that carries this
        // pilot's own chassis. It must not be dropped with the world checkpoint
        // a backlog skips below. An anchor may name a configuration only once
        // the peer acknowledged it; until then the configuration itself is the
        // reliable-lane payload and no anchor is emitted, because a reference
        // the peer cannot resolve is worse than a delayed correction.
        if frame.periodic
            && let Some(chassis) = self.chassis
            && let ServerMessage::Snapshot(state) = frame.message()
            && let Some(anchor) = crate::owner_stream::OwnerAnchor::from_state(state, chassis)
        {
            let revision = anchor.config_revision();
            if self.owner_config.acknowledged(revision) {
                // Oversize anchors never fragment. Full checkpoints remain the recovery path.
                if let Ok(bytes) = anchor.encode() {
                    self.encoding.owner_updates += 1;
                    self.encoding.owner_bytes += bytes.len() as u64;
                    self.pacer.owner(self.elapsed(now), bytes);
                }
            } else if let Some(bytes) = self.owner_config.offer(revision, &anchor.owner.config)? {
                self.queue_control(now, &bytes)?;
            }
        }
        if frame.periodic && pending_bytes > CONGESTED_PENDING_BYTES {
            self.encoding.skipped_world_updates += 1;
            return Ok(());
        }
        let compressed = if frame.periodic
            && !self.full_checkpoints
            && let ServerMessage::Snapshot(state) = frame.message()
        {
            let raw = crate::snapshot_codec::encode_player_message(frame.message());
            self.encoding.raw_world_bytes += raw.len() as u64;
            self.encoder.snapshot(state.input_epoch, &raw)?
        } else {
            frame.compressed().to_vec()
        };
        let revision = self.next_revision()?;
        let frames = packets(revision, !frame.periodic, &compressed)?;
        if frame.periodic {
            self.encoding.world_updates += 1;
            self.encoding.framed_world_bytes += frames.iter().map(|p| p.len() as u64).sum::<u64>();
            self.encoding.independent_bytes = self.encoder.full_bytes;
            self.encoding.selected_bytes = self.encoder.sent_bytes;
            self.pacer.world(self.elapsed(now), frames);
        } else {
            self.pacer
                .control(self.elapsed(now), frames)
                .map_err(io_error)?;
        }
        // A lost `Retired` answer would otherwise pin the retiring baseline
        // forever and stop new ones being proposed.
        if let Some(retire) = self.encoder.resend_retire() {
            let bytes = crate::udp_snapshot::encode(retire);
            self.queue_control(now, &bytes)?;
        }
        Ok(())
    }
    /// Next datagram the pacer allows at `now`, or `None` until its bytes have
    /// accrued.
    pub(crate) fn next(&mut self, now: Instant) -> io::Result<Option<Datagram>> {
        self.pacer.next(self.elapsed(now)).map_err(io_error)
    }
    /// Host encoding counters for this peer. Test-facing: the bandwidth probe
    /// reads them here because no production path reports them over the wire.
    #[cfg(test)]
    pub(crate) fn encoding_stats(&self) -> crate::network_stats::EncodingStats {
        self.encoding.clone()
    }
}

/// One peer's whole host-side leg: admission, the seat it was given, its bounded
/// outbox and the codec above. `gns_transport::serve` drives one of these per
/// connection; a test drives one over a scripted link. Dropping it releases the
/// seat, so a closed carrier never strands a chassis.
pub struct HostPeer {
    handle: HostHandle,
    codec: PeerCodec,
    stop: Stop,
    seat: Option<(u32, outbox::Receiver)>,
    welcome: Option<Welcome>,
    outgoing: Vec<Datagram>,
    last_delivery_report: Instant,
}
impl HostPeer {
    /// `admitted` is when the carrier accepted this peer; the hello timeout and
    /// the pacer both measure from it.
    pub fn new(handle: HostHandle, admitted: Instant, rate_bytes_per_s: u32) -> Self {
        Self {
            handle,
            codec: PeerCodec::new(
                admitted,
                rate_bytes_per_s,
                std::env::var_os("RM_NET_FULL_CHECKPOINTS").is_some(),
            ),
            stop: Stop::default(),
            seat: None,
            welcome: None,
            outgoing: Vec::new(),
            last_delivery_report: admitted,
        }
    }
    /// When the carrier accepted this peer.
    pub fn admitted(&self) -> Instant {
        self.codec.admitted()
    }
    /// The host's id for this peer's seat, or `None` before its hello.
    pub fn client_id(&self) -> Option<u32> {
        self.seat.as_ref().map(|(id, _)| *id)
    }
    /// The welcome the host returned for this peer, or `None` before its hello.
    pub fn welcome(&self) -> Option<&Welcome> {
        self.welcome.as_ref()
    }
    /// The host asked for this seat to close.
    pub fn closed(&self) -> bool {
        self.stop.wait(Duration::ZERO)
    }
    /// Decode one datagram from this peer and submit what it says to the host.
    pub fn deliver(&mut self, payload: &[u8], now: Instant) -> io::Result<()> {
        match self.codec.receive(payload, now)? {
            PeerRequest::Handled => Ok(()),
            PeerRequest::Inputs(inputs) => {
                let (id, _) = self
                    .seat
                    .as_ref()
                    .ok_or_else(|| io_error("expected hello"))?;
                self.handle.pilot_batch(*id, inputs).map_err(io_error)
            }
            PeerRequest::Message(message) => {
                let message = *message;
                if let Some((id, _)) = &self.seat {
                    return self.handle.message(*id, message).map_err(io_error);
                }
                let ClientMessage::Hello {
                    password,
                    protocol,
                    name,
                    team,
                    role,
                } = message
                else {
                    return Err(io_error("expected hello"));
                };
                if protocol != crate::protocol::PROTOCOL_VERSION {
                    return Err(io_error(crate::protocol::version_mismatch(
                        crate::protocol::PROTOCOL_VERSION,
                        protocol,
                    )));
                }
                self.join(name, team, role, password)
            }
        }
    }
    /// Registers the peer with the host and keeps the outbox the host writes its
    /// frames to for as long as the seat lives.
    fn join(
        &mut self,
        name: String,
        team: Option<Team>,
        role: Role,
        password: String,
    ) -> io::Result<()> {
        let (sender, receiver) = outbox::channel(crate::net::OUTBOX_CAPACITY);
        let welcome = self
            .handle
            .join(PeerRegistration {
                password,
                name,
                team,
                role,
                owner_spawn: None,
                outbox: sender,
                stream: ConnectionStop::Worker(self.stop.clone()),
            })
            .map_err(io_error)?;
        self.codec
            .joined(welcome.chassis.as_ref().map(|chassis| chassis.id));
        self.seat = Some((welcome.client_id, receiver));
        self.welcome = Some(welcome);
        Ok(())
    }
    /// Move up to `frames` queued host messages into the codec, then pace out
    /// up to `datagrams` packets. Limiting work per call keeps one busy peer
    /// from starving the rest of a reactor's peers.
    pub fn pump(
        &mut self,
        now: Instant,
        pending_bytes: u32,
        frames: usize,
        datagrams: usize,
    ) -> io::Result<()> {
        if let Some((_, receiver)) = &self.seat {
            for _ in 0..frames {
                let Some(frame) = receiver.try_recv() else {
                    break;
                };
                self.codec.send(&frame, now, pending_bytes)?;
            }
        }
        if self.seat.is_some()
            && now.saturating_duration_since(self.last_delivery_report) >= Duration::from_secs(1)
        {
            let mut stats = self.codec.pacer.stats(self.codec.elapsed(now));
            stats.encoding = Some(self.codec.encoding.clone());
            self.codec.send(
                &Outbound::new(ServerMessage::DeliveryStats(stats)),
                now,
                pending_bytes,
            )?;
            self.last_delivery_report = now;
        }
        for _ in 0..datagrams {
            let Some(packet) = self.codec.next(now)? else {
                break;
            };
            self.outgoing.push(packet);
        }
        Ok(())
    }
    /// Datagrams the carrier should transmit, oldest first.
    pub fn take_outgoing(&mut self) -> Vec<Datagram> {
        std::mem::take(&mut self.outgoing)
    }
}
impl Drop for HostPeer {
    fn drop(&mut self) {
        if let Some((id, _)) = &self.seat {
            let _ = self.handle.leave(*id);
        }
    }
}

/// What one host datagram turned out to be, once framing, reassembly and
/// baseline decoding have run.
pub(crate) enum ClientEvent {
    /// The host's answer to the hello, carrying this peer's seat.
    Welcome(Box<Welcome>),
    /// One decoded server message, snapshot or control.
    Message(ServerMessage),
    /// The newest owner anchor for the pilot's own chassis.
    Anchor(Box<crate::owner_stream::OwnerAnchor>),
}

/// The client's half of the same wire. Its `epoch` is the pacer's zero; the
/// session's clock supplies every `now`.
pub(crate) struct ClientCodec {
    epoch: Instant,
    welcomed: bool,
    latest_owner: u64,
    frames: Frames,
    pacer: Pacer,
    recent_inputs: VecDeque<Command>,
    history_limit: usize,
    /// Owner configurations this client holds, oldest first, so an anchor can
    /// resolve the revision it names.
    configs: VecDeque<(crate::owner_stream::ConfigRevision, ChassisConfig)>,
    /// Owner-configuration answers waiting for the next [`ClientCodec::acknowledge`].
    config_feedback: VecDeque<Feedback>,
}
impl ClientCodec {
    /// Creates the client codec with `epoch` as the pacer's zero.
    /// `rate_bytes_per_s` is the configured upstream budget and `history_limit`
    /// caps the redundant input frames in one batch.
    pub(crate) fn new(epoch: Instant, rate_bytes_per_s: u32, history_limit: usize) -> Self {
        Self {
            epoch,
            welcomed: false,
            latest_owner: 0,
            frames: Frames::default(),
            pacer: Pacer::new(rate_bytes_per_s),
            recent_inputs: VecDeque::new(),
            history_limit,
            configs: VecDeque::new(),
            config_feedback: VecDeque::new(),
        }
    }
    /// The opening datagram. It travels on the reliable lane, like every command.
    pub(crate) fn hello(name: &str, team: Option<Team>, role: Role) -> io::Result<Vec<u8>> {
        Self::hello_with_password(name, team, role, "")
    }
    /// The opening datagram with a password. It travels on the reliable lane,
    /// like every command.
    pub(crate) fn hello_with_password(
        name: &str,
        team: Option<Team>,
        role: Role,
        password: &str,
    ) -> io::Result<Vec<u8>> {
        serde_json::to_vec(&ClientMessage::Hello {
            password: password.into(),
            protocol: crate::protocol::PROTOCOL_VERSION,
            name: name.to_string(),
            team,
            role,
        })
        .map_err(io_error)
    }
    fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.epoch)
    }
    /// Accepts one server datagram at `now`. Returns `None` for an anchor that
    /// arrived before the welcome or is no newer than the last one, for an owner
    /// configuration the codec consumes itself, for an anchor that names a
    /// configuration this client does not hold, for an incomplete frame and for
    /// a frame that decoded to nothing. An unknown configuration reference is
    /// never an error: the anchor is dropped and the configuration requested
    /// again. Errors on an invalid welcome, a protocol mismatch and any framing
    /// or baseline failure.
    pub(crate) fn receive(
        &mut self,
        payload: &[u8],
        now: Instant,
    ) -> io::Result<Option<ClientEvent>> {
        if payload.starts_with(crate::owner_stream::MAGIC) {
            return self.receive_anchor(payload);
        }
        let Some(message) = self.frames.receive(payload, now)? else {
            return Ok(None);
        };
        match message {
            ServerMessage::Welcome(welcome) => {
                if self.welcomed || welcome.protocol != crate::protocol::PROTOCOL_VERSION {
                    return Err(io_error("invalid welcome"));
                }
                self.welcomed = true;
                Ok(Some(ClientEvent::Welcome(welcome)))
            }
            ServerMessage::OwnerConfig(frame) => {
                // The configuration is codec state, not application input: cache
                // it for anchor resolution and acknowledge it on the reliable
                // lane. It never reaches the app or fails the connection.
                frame.validate()?;
                self.remember_config(frame.revision, frame.config);
                Ok(None)
            }
            message => Ok(Some(ClientEvent::Message(message))),
        }
    }
    /// Resolves one anchor's configuration reference, or asks the host to resend
    /// the configuration when it is unknown.
    fn receive_anchor(&mut self, payload: &[u8]) -> io::Result<Option<ClientEvent>> {
        if !self.welcomed {
            return Ok(None);
        }
        let revision = crate::owner_stream::OwnerAnchor::config_revision_of(payload)?;
        let Some(config) = self.config(revision) else {
            // A reference that cannot decode is dropped, never applied. The
            // request is bounded: repeated anchors for one missing revision ask
            // once until the configuration arrives.
            self.request_config(revision);
            return Ok(None);
        };
        let anchor = crate::owner_stream::OwnerAnchor::decode(payload, &config)?;
        if anchor.snapshot_id <= self.latest_owner {
            return Ok(None);
        }
        self.latest_owner = anchor.snapshot_id;
        Ok(Some(ClientEvent::Anchor(Box::new(anchor))))
    }
    /// The configuration this client holds for `revision`, if any.
    fn config(&self, revision: crate::owner_stream::ConfigRevision) -> Option<ChassisConfig> {
        self.configs
            .iter()
            .rev()
            .find(|(held, _)| *held == revision)
            .map(|(_, config)| config.clone())
    }
    /// Caches one configuration and answers the host so anchors may name it.
    fn remember_config(
        &mut self,
        revision: crate::owner_stream::ConfigRevision,
        config: ChassisConfig,
    ) {
        self.configs.retain(|(held, _)| *held != revision);
        self.configs.push_back((revision, config));
        while self.configs.len() > CONFIG_CACHE {
            self.configs.pop_front();
        }
        self.config_feedback.retain(
            |feedback| !matches!(feedback, Feedback::ConfigMissing { revision: missing } if *missing == revision.raw()),
        );
        self.push_config_feedback(Feedback::ConfigStored {
            revision: revision.raw(),
        });
    }
    /// Asks the host to resend one configuration.
    fn request_config(&mut self, revision: crate::owner_stream::ConfigRevision) {
        self.push_config_feedback(Feedback::ConfigMissing {
            revision: revision.raw(),
        });
    }
    /// Queues one configuration answer, ignoring a duplicate and bounding the
    /// queue so a stream of unknown references cannot grow it without limit.
    fn push_config_feedback(&mut self, feedback: Feedback) {
        if self.config_feedback.contains(&feedback) {
            return;
        }
        while self.config_feedback.len() >= CONFIG_FEEDBACK {
            self.config_feedback.pop_front();
        }
        self.config_feedback.push_back(feedback);
    }
    /// Test seam: mark the connection welcomed without running a real hello, so
    /// a codec test can exercise anchor acceptance.
    #[cfg(test)]
    pub(crate) fn welcome_for_test(&mut self) {
        self.welcomed = true;
    }
    /// Answer every baseline the decoder stored, retired or found missing, and
    /// every owner-configuration answer the anchor handshake queued.
    pub(crate) fn acknowledge(&mut self, now: Instant) -> io::Result<()> {
        while let Some(feedback) = self.frames.feedback.pop_front() {
            self.pacer
                .control(self.elapsed(now), vec![feedback.encode()])
                .map_err(io_error)?;
        }
        while let Some(feedback) = self.config_feedback.pop_front() {
            self.pacer
                .control(self.elapsed(now), vec![feedback.encode()])
                .map_err(io_error)?;
        }
        Ok(())
    }
    /// Encode one queued command. Pilot input rides a replaceable batch of
    /// recent frames; an unconfirmed shot retries on the unreliable lane;
    /// everything else is a reliable control message.
    pub(crate) fn submit(&mut self, queued: QueuedCommand, now: Instant) -> io::Result<()> {
        if queued.confirmation.is_none()
            && let Some(command @ Command::PilotInput { .. }) = queued.command
        {
            self.recent_inputs.push_back(command);
            while self.recent_inputs.len() > 32 {
                self.recent_inputs.pop_front();
            }
            self.pacer.owner(
                self.elapsed(now),
                input_batch(&select_inputs(&self.recent_inputs, self.history_limit))?,
            );
            return Ok(());
        }
        if queued.confirmation.is_none()
            && let Some(command @ Command::FireAimed { .. }) = queued.command
        {
            let mut bytes = COMMAND_MAGIC.to_vec();
            bytes.extend(miniz_oxide::deflate::compress_to_vec(
                &serde_json::to_vec(&ClientMessage::Command(command)).map_err(io_error)?,
                1,
            ));
            return self
                .pacer
                .unreliable_control(self.elapsed(now), bytes)
                .map_err(io_error);
        }
        let messages = queued
            .command
            .map(ClientMessage::Command)
            .into_iter()
            .chain(
                queued
                    .time_probe
                    .map(|nonce| ClientMessage::TimeProbe { nonce }),
            )
            .chain(
                queued
                    .confirmation
                    .map(|nonce| ClientMessage::Ping { nonce }),
            );
        for message in messages {
            self.pacer
                .control(
                    self.elapsed(now),
                    vec![serde_json::to_vec(&message).map_err(io_error)?],
                )
                .map_err(io_error)?;
        }
        Ok(())
    }
    /// Next datagram the pacer allows at `now`, or `None` until its bytes have
    /// accrued.
    pub(crate) fn next(&mut self, now: Instant) -> io::Result<Option<Datagram>> {
        self.pacer.next(self.elapsed(now)).map_err(io_error)
    }
    /// Delivery counters for the network overlay, and for tests that assert the
    /// decoder never pins a third baseline.
    pub(crate) fn stats(&self) -> crate::network_stats::TransportStats {
        crate::network_stats::TransportStats {
            incomplete_frames: self.frames.incomplete_frames,
            stale_updates: self.frames.stale_updates,
            complete_snapshots: self.frames.complete_snapshots,
            application_queued_bytes: self.pacer.queue_bytes(),
            replaced_unsent: self.pacer.replaced,
            expired_unsent: self.pacer.expired,
            decoded_deltas: self.frames.decoder.deltas,
            missing_baselines: self.frames.decoder.missing,
            ..Default::default()
        }
    }
    /// Baselines the decoder still holds. The wire contract caps this at two, so
    /// an encoder may propose a third only after one is retired.
    pub(crate) fn pinned_baselines(&self) -> usize {
        self.frames.decoder.pinned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ServerMessage;
    use crate::simulation::Simulation;
    use rm_simulator_world::{ChassisCommand, Field, FieldConfig};

    fn snapshot(ticks: u64) -> ServerMessage {
        let mut simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true);
        simulation.step(ticks).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = ticks + 1;
        ServerMessage::Snapshot(Box::new(state))
    }

    fn encoded(revision: u64, reliable: bool, message: &ServerMessage) -> Vec<Vec<u8>> {
        packets(
            revision,
            reliable,
            &miniz_oxide::deflate::compress_to_vec(
                &crate::snapshot_codec::encode_player_message(message),
                1,
            ),
        )
        .unwrap()
    }

    /// One host state with a single chassis, a snapshot id and an input epoch.
    fn owner_state(ticks: u64, snapshot_id: u64, epoch: u64) -> crate::simulation::SimulationState {
        use rm_simulator_world::{ChassisPlacement, Pose, Team};
        let mut field = Field::new(&FieldConfig::default()).unwrap();
        field
            .add_chassis(&ChassisPlacement {
                team: Team::Red,
                spawn: Pose::at([0., 0., 0.3]),
                config: ChassisConfig::default(),
            })
            .unwrap();
        let mut simulation = Simulation::new(field, false);
        simulation.step(ticks).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = snapshot_id;
        state.input_epoch = epoch;
        state
    }

    /// A periodic host publication, which is what produces an owner anchor.
    fn periodic(state: &crate::simulation::SimulationState) -> Outbound {
        let mut frame = Outbound::new(ServerMessage::Snapshot(Box::new(state.clone())));
        frame.periodic = true;
        frame
    }

    /// Drain every datagram the host pacer allows at `now`.
    fn host_packets(host: &mut PeerCodec, now: Instant) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        while let Some(packet) = host.next(now).unwrap() {
            packets.push(packet.bytes);
        }
        packets
    }

    /// Drain every datagram the client pacer allows at `now`.
    fn client_packets(client: &mut ClientCodec, now: Instant) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        while let Some(packet) = client.next(now).unwrap() {
            packets.push(packet.bytes);
        }
        packets
    }

    /// A complete RMG1 frame's message, or `None` for a fragment or a peer that
    /// cannot be decoded. A configuration frame is small enough to be one.
    fn framed_message(packet: &[u8]) -> Option<ServerMessage> {
        if packet.len() < HEADER || &packet[..4] != b"RMG1" {
            return None;
        }
        let json =
            miniz_oxide::inflate::decompress_to_vec_with_limit(&packet[HEADER..], MAX_WIRE).ok()?;
        crate::snapshot_codec::decode_player_message(&json).ok()
    }

    #[test]
    fn owner_config_is_carried_once_then_anchors_reference_it() {
        use crate::owner_stream::{ConfigRevision, OwnerAnchor};
        let epoch = Instant::now();
        let state = owner_state(3, 3, 0);
        let chassis = state.field.chassis.first().unwrap().id;
        let config = state.field.chassis.first().unwrap().config.clone();
        let revision = ConfigRevision::of(&config);
        let mut host = PeerCodec::new(epoch, 1 << 20, false);
        host.joined(Some(chassis));
        let mut client = ClientCodec::new(epoch, 1 << 20, 12);
        client.welcome_for_test();

        // Before any acknowledgement, exactly the configuration is queued and no
        // anchor is: a reference the peer cannot resolve is never emitted.
        host.send(&periodic(&state), epoch, 0).unwrap();
        let first = host_packets(&mut host, epoch + Duration::from_millis(32));
        assert_eq!(
            first
                .iter()
                .filter(|packet| matches!(
                    framed_message(packet),
                    Some(ServerMessage::OwnerConfig(_))
                ))
                .count(),
            1,
            "the configuration travels once"
        );
        assert!(
            !first
                .iter()
                .any(|packet| packet.starts_with(crate::owner_stream::MAGIC)),
            "no anchor before the acknowledgement"
        );

        // The client consumes the configuration as codec state and acknowledges.
        for packet in &first {
            if let Some(event) = client.receive(packet, epoch).unwrap() {
                assert!(
                    !matches!(event, ClientEvent::Message(ServerMessage::OwnerConfig(_))),
                    "the configuration never reaches the app"
                );
            }
        }
        client.acknowledge(epoch).unwrap();
        let answers = client_packets(&mut client, epoch + Duration::from_millis(64));
        assert!(answers.iter().any(|packet| matches!(
            Feedback::decode(packet),
            Ok(Feedback::ConfigStored { revision: stored }) if stored == revision.raw()
        )));
        for packet in &answers {
            host.receive(packet, epoch).unwrap();
        }

        // A later publication carries an anchor that references the config.
        let later = owner_state(5, 4, 2);
        host.send(&periodic(&later), epoch + Duration::from_millis(64), 0)
            .unwrap();
        let second = host_packets(&mut host, epoch + Duration::from_millis(96));
        let anchor = second
            .iter()
            .find(|packet| packet.starts_with(crate::owner_stream::MAGIC))
            .expect("an acknowledged configuration releases anchors");
        assert_eq!(OwnerAnchor::config_revision_of(anchor).unwrap(), revision);
        let mut applied = None;
        for packet in &second {
            if let Some(ClientEvent::Anchor(anchor)) = client
                .receive(packet, epoch + Duration::from_millis(96))
                .unwrap()
            {
                applied = Some(*anchor);
            }
        }
        assert_eq!(applied.expect("the anchor applies").owner.config, config);
    }

    #[test]
    fn unknown_anchor_reference_defers_and_recovers() {
        use crate::owner_stream::{OwnerAnchor, OwnerConfig};
        let epoch = Instant::now();
        let state = owner_state(7, 7, 0);
        let chassis = state.field.chassis.first().unwrap().id;
        let config = state.field.chassis.first().unwrap().config.clone();
        let anchor = OwnerAnchor::from_state(&state, chassis).unwrap();
        let bytes = anchor.encode().unwrap();
        let revision = anchor.config_revision();

        let mut client = ClientCodec::new(epoch, 1 << 20, 12);
        client.welcome_for_test();
        // An unknown reference is dropped, not applied, and the connection lives.
        assert!(client.receive(&bytes, epoch).unwrap().is_none());
        client.acknowledge(epoch).unwrap();
        let requests = client_packets(&mut client, epoch + Duration::from_millis(32));
        assert!(requests.iter().any(|packet| matches!(
            Feedback::decode(packet),
            Ok(Feedback::ConfigMissing { revision: missing }) if missing == revision.raw()
        )));

        // The configuration arrives, is acknowledged, and resolves the anchor.
        let frame = ServerMessage::OwnerConfig(Box::new(OwnerConfig::new(config.clone())));
        for packet in encoded(1, true, &frame) {
            assert!(client.receive(&packet, epoch).unwrap().is_none());
        }
        client.acknowledge(epoch).unwrap();
        let stored = client_packets(&mut client, epoch + Duration::from_millis(64));
        assert!(stored.iter().any(|packet| matches!(
            Feedback::decode(packet),
            Ok(Feedback::ConfigStored { revision: saved }) if saved == revision.raw()
        )));
        let Some(ClientEvent::Anchor(applied)) = client
            .receive(&bytes, epoch + Duration::from_millis(64))
            .unwrap()
        else {
            panic!("the configuration resolves the anchor");
        };
        assert_eq!(applied.owner.config, config);
    }

    #[test]
    fn changed_owner_configuration_is_not_confused_with_the_old_one() {
        use crate::owner_stream::{ConfigRevision, OwnerAnchor};
        let epoch = Instant::now();
        let first = owner_state(1, 1, 0);
        let chassis = first.field.chassis.first().unwrap().id;
        let mut host = PeerCodec::new(epoch, 1 << 20, false);
        host.joined(Some(chassis));
        let mut client = ClientCodec::new(epoch, 1 << 20, 12);
        client.welcome_for_test();

        // Establish configuration A.
        host.send(&periodic(&first), epoch, 0).unwrap();
        for packet in host_packets(&mut host, epoch + Duration::from_millis(32)) {
            client.receive(&packet, epoch).unwrap();
        }
        client.acknowledge(epoch).unwrap();
        for packet in client_packets(&mut client, epoch + Duration::from_millis(64)) {
            host.receive(&packet, epoch).unwrap();
        }
        let old = host.owner_config.acked.expect("A is acknowledged");

        // The chassis is respawned with a changed configuration B.
        let mut changed = owner_state(3, 2, 0);
        changed.field.chassis[0].config.mass_kg += 5.;
        let new = ConfigRevision::of(&changed.field.chassis[0].config);
        assert_ne!(old, new, "changed values must not reuse an identity");
        host.send(&periodic(&changed), epoch + Duration::from_millis(64), 0)
            .unwrap();
        let offered = host_packets(&mut host, epoch + Duration::from_millis(96));
        assert!(
            !offered
                .iter()
                .any(|packet| packet.starts_with(crate::owner_stream::MAGIC)),
            "B is not referenced before it is acknowledged"
        );
        assert!(offered.iter().any(|packet| matches!(
            framed_message(packet),
            Some(ServerMessage::OwnerConfig(frame)) if frame.revision == new
        )));

        // A delayed acknowledgement for A must not unlock B.
        host.receive(
            &Feedback::ConfigStored {
                revision: old.raw(),
            }
            .encode(),
            epoch,
        )
        .unwrap();
        host.send(&periodic(&changed), epoch + Duration::from_millis(96), 0)
            .unwrap();
        assert!(
            !host_packets(&mut host, epoch + Duration::from_millis(128))
                .iter()
                .any(|packet| packet.starts_with(crate::owner_stream::MAGIC)),
            "a stale acknowledgement cannot unlock the changed configuration"
        );

        // Only B's own acknowledgement releases anchors.
        host.receive(
            &Feedback::ConfigStored {
                revision: new.raw(),
            }
            .encode(),
            epoch,
        )
        .unwrap();
        host.send(&periodic(&changed), epoch + Duration::from_millis(128), 0)
            .unwrap();
        let released = host_packets(&mut host, epoch + Duration::from_millis(160));
        let anchor = released
            .iter()
            .find(|packet| packet.starts_with(crate::owner_stream::MAGIC))
            .expect("B releases anchors once acknowledged");
        assert_eq!(OwnerAnchor::config_revision_of(anchor).unwrap(), new);
    }

    #[test]
    fn unacknowledged_config_is_resent_only_on_the_bounded_schedule() {
        let epoch = Instant::now();
        let state = owner_state(1, 1, 0);
        let chassis = state.field.chassis.first().unwrap().id;
        let mut host = PeerCodec::new(epoch, 1 << 20, false);
        host.joined(Some(chassis));
        let mut offers = 0;
        for step in 0..CONFIG_RESEND_FRAMES + 2 {
            let now = epoch + Duration::from_millis(32 * step);
            host.send(&periodic(&state), now, 0).unwrap();
            let packets = host_packets(&mut host, now + Duration::from_millis(32));
            assert!(
                !packets
                    .iter()
                    .any(|packet| packet.starts_with(crate::owner_stream::MAGIC)),
                "an unacknowledged configuration never releases anchors"
            );
            offers += packets
                .iter()
                .filter(|packet| {
                    matches!(framed_message(packet), Some(ServerMessage::OwnerConfig(_)))
                })
                .count();
        }
        assert_eq!(offers, 2, "one offer and exactly one bounded resend");
    }

    #[test]
    fn owner_config_recovers_after_lost_setup_and_acknowledgement() {
        let epoch = Instant::now();
        let state = owner_state(1, 1, 0);
        let chassis = state.field.chassis[0].id;
        let mut host = PeerCodec::new(epoch, 1 << 20, false);
        host.joined(Some(chassis));
        let mut client = ClientCodec::new(epoch, 1 << 20, 12);
        client.welcome_for_test();
        let mut configurations = 0;
        let mut anchors = 0;
        let mut checkpoints = 0;
        for step in 0..CONFIG_RESEND_FRAMES * 3 + 2 {
            let now = epoch + Duration::from_millis(32 * step);
            host.send(&periodic(&state), now, 0).unwrap();
            for packet in host_packets(&mut host, now) {
                if matches!(framed_message(&packet), Some(ServerMessage::OwnerConfig(_))) {
                    configurations += 1;
                    if configurations == 1 {
                        continue; // Lose initial setup; the host must retry.
                    }
                }
                match client.receive(&packet, now).unwrap() {
                    Some(ClientEvent::Anchor(_)) => anchors += 1,
                    Some(ClientEvent::Message(ServerMessage::Snapshot(_))) => checkpoints += 1,
                    _ => {}
                }
            }
            client.acknowledge(now).unwrap();
            for packet in client_packets(&mut client, now) {
                if configurations < 3
                    && matches!(Feedback::decode(&packet), Ok(Feedback::ConfigStored { .. }))
                {
                    continue; // Lose acknowledgements until another setup resend.
                }
                host.receive(&packet, now).unwrap();
            }
            if configurations < 3 {
                assert_eq!(anchors, 0, "no unresolved configuration references");
            }
        }
        assert_eq!(configurations, 3, "resends stop after acknowledgement");
        assert!(
            anchors > 0,
            "owner corrections recover after acknowledgement loss"
        );
        assert!(checkpoints > 0, "complete world context remains available");
    }

    #[test]
    fn a_configuration_reference_survives_an_input_epoch_reset() {
        use crate::owner_stream::{OwnerAnchor, OwnerConfig};
        let epoch = Instant::now();
        let state = owner_state(3, 3, 0);
        let chassis = state.field.chassis.first().unwrap().id;
        let config = state.field.chassis.first().unwrap().config.clone();
        let mut client = ClientCodec::new(epoch, 1 << 20, 12);
        client.welcome_for_test();
        // The client already holds the configuration from its join.
        let frame = ServerMessage::OwnerConfig(Box::new(OwnerConfig::new(config.clone())));
        for packet in encoded(1, true, &frame) {
            client.receive(&packet, epoch).unwrap();
        }
        // The host restarts its input timeline. The configuration identity is
        // content-derived, so an anchor under the new epoch still resolves.
        let reset = owner_state(5, 4, 9);
        let anchor = OwnerAnchor::from_state(&reset, chassis).unwrap();
        let Some(ClientEvent::Anchor(applied)) =
            client.receive(&anchor.encode().unwrap(), epoch).unwrap()
        else {
            panic!("an epoch reset must not discard the configuration");
        };
        assert_eq!(applied.input_epoch, 9);
        assert_eq!(applied.owner.config, config);
    }

    #[test]
    fn a_reconnecting_peer_reestablishes_the_configuration_before_anchors() {
        use crate::owner_stream::OwnerAnchor;
        let epoch = Instant::now();
        let state = owner_state(1, 1, 0);
        let chassis = state.field.chassis.first().unwrap().id;
        // A reconnect is a fresh codec pair: the host must carry the
        // configuration again and release no anchor until the new peer answers.
        let mut host = PeerCodec::new(epoch, 1 << 20, false);
        host.joined(Some(chassis));
        host.send(&periodic(&state), epoch, 0).unwrap();
        let packets = host_packets(&mut host, epoch + Duration::from_millis(32));
        assert!(
            packets.iter().any(|packet| matches!(
                framed_message(packet),
                Some(ServerMessage::OwnerConfig(_))
            ))
        );
        assert!(
            !packets
                .iter()
                .any(|packet| packet.starts_with(crate::owner_stream::MAGIC)),
            "a reconnecting peer gets no anchor before its acknowledgement"
        );
        // An anchor that races ahead of the configuration is dropped and
        // answered with a request, never applied and never fatal.
        let mut client = ClientCodec::new(epoch, 1 << 20, 12);
        client.welcome_for_test();
        let anchor = OwnerAnchor::from_state(&state, chassis).unwrap();
        assert!(
            client
                .receive(&anchor.encode().unwrap(), epoch)
                .unwrap()
                .is_none()
        );
        client.acknowledge(epoch).unwrap();
        let answers = client_packets(&mut client, epoch + Duration::from_millis(64));
        assert!(
            answers.iter().any(|packet| matches!(
                Feedback::decode(packet),
                Ok(Feedback::ConfigMissing { .. })
            ))
        );
    }

    #[test]
    fn redundant_history_retains_release_and_stays_below_one_datagram() {
        let mut history = VecDeque::new();
        for sequence in 1..=20 {
            history.push_back(Command::PilotInput {
                chassis: 1,
                frame: crate::input_stream::InputFrame {
                    input_epoch: 0,
                    sequence,
                    sampled_time_ns: sequence * 16_000_000,
                    duration_ticks: 16,
                    placement_revision: 0,
                    command: rm_simulator_world::ChassisCommand {
                        forward_m_s: if sequence < 8 { 2. } else { 0. },
                        aim_yaw_rad: sequence as f64 * 0.0123456789123456,
                        ..Default::default()
                    },
                },
            });
        }
        let selected = select_inputs(&history, 12);
        assert!(
            selected
                .iter()
                .any(|c| matches!(c, Command::PilotInput { frame, .. } if frame.sequence == 8))
        );
        assert_eq!(select_inputs(&history, 4).len(), 4);
        let mut stream = crate::input_stream::InputStream::default();
        for command in decode_inputs(&input_batch(&selected).unwrap()).unwrap() {
            if let Command::PilotInput { frame, .. } = command {
                stream.receive(frame, 320_000_000, 0).unwrap();
            }
        }
        assert_eq!(stream.latest(), 20);
        assert_eq!(stream.expire(570_000_000).unwrap().forward_m_s, 0.);
        let maximal: VecDeque<_> = history.iter().rev().take(12).copied().collect();
        assert!(input_batch(&maximal).unwrap().len() <= 1_000);
        let Command::PilotInput { frame, .. } = history.back_mut().unwrap() else {
            unreachable!()
        };
        frame.input_epoch = 1;
        assert_eq!(select_inputs(&history, 12).len(), 1);
    }

    #[test]
    fn compressed_input_history_preserves_samples_and_bounds_decoding() {
        let mut inputs = VecDeque::new();
        for sequence in 1..=4 {
            inputs.push_back(Command::PilotInput {
                chassis: 7,
                frame: crate::input_stream::InputFrame {
                    input_epoch: 2,
                    sequence,
                    sampled_time_ns: sequence * 16_000_000,
                    duration_ticks: 16,
                    placement_revision: 3,
                    command: rm_simulator_world::ChassisCommand {
                        forward_m_s: 1.5,
                        aim_yaw_rad: 0.7,
                        ..Default::default()
                    },
                },
            });
        }
        let packet = input_batch(&inputs).unwrap();
        let previous_bytes: usize = inputs
            .iter()
            .map(|input| {
                serde_json::to_vec(&ClientMessage::Command(*input))
                    .unwrap()
                    .len()
            })
            .sum();
        assert_eq!(
            decode_inputs(&packet).unwrap(),
            inputs.iter().copied().collect::<Vec<_>>()
        );
        eprintln!(
            "input history separate/batched bytes: {previous_bytes}/{}",
            packet.len()
        );
        assert!(packet.len() * 2 < previous_bytes);
        assert!(
            packet.len() < CHUNK,
            "normal batches should fit one datagram"
        );
        while inputs.len() <= 12 {
            inputs.push_back(*inputs.back().unwrap());
        }
        assert!(input_batch(&inputs).is_err());
        inputs.clear();
        inputs.push_back(Command::Pause { paused: true });
        assert!(input_batch(&inputs).is_err());
        let mut bomb = INPUT_BATCH_MAGIC.to_vec();
        bomb.extend(miniz_oxide::deflate::compress_to_vec(
            &vec![b' '; INPUT_BATCH_LIMIT + 1],
            1,
        ));
        assert!(decode_inputs(&bomb).is_err());
    }

    #[test]
    fn loss_reordering_and_duplicates_do_not_break_next_snapshot() {
        let now = Instant::now();
        let mut frames = Frames::default();
        let first = encoded(1, false, &snapshot(1));
        // Lose the end of one frame; receive the next in reverse order.
        for packet in first.iter().take(first.len() - 1) {
            assert!(frames.receive(packet, now).unwrap().is_none());
        }
        let next = encoded(2, false, &snapshot(2));
        let mut result = None;
        for packet in next.iter().rev() {
            result = frames.receive(packet, now).unwrap().or(result);
            assert!(frames.receive(packet, now).unwrap().is_none());
        }
        let Some(ServerMessage::Snapshot(state)) = result else {
            panic!("missing state")
        };
        assert_eq!(state.field.tick, 2);
        for packet in first {
            assert!(frames.receive(&packet, now).unwrap().is_none());
        }
        // A delayed reliable confirmation must not regress newer periodic state.
        for packet in encoded(1, true, &snapshot(1)) {
            assert!(frames.receive(&packet, now).unwrap().is_none());
        }
        let mut pong = None;
        for packet in encoded(3, true, &ServerMessage::Pong { nonce: 7 }) {
            pong = frames.receive(&packet, now).unwrap().or(pong);
        }
        assert!(matches!(pong, Some(ServerMessage::Pong { nonce: 7 })));
    }

    #[test]
    fn repeated_old_baseline_cannot_hide_a_newer_confirmation() {
        let now = Instant::now();
        let mut frames = Frames::default();
        let mut encoder = crate::udp_snapshot::Encoder::default();
        let bytes = crate::snapshot_codec::encode_player_message(&snapshot(1));
        let full = encoder.snapshot(0, &bytes).unwrap();
        // A retried bootstrap baseline has a newer transport ordinal but old host state.
        for packet in packets(100, false, &full).unwrap() {
            frames.receive(&packet, now).unwrap();
        }
        let mut confirmed = None;
        for packet in encoded(50, true, &snapshot(5)) {
            confirmed = frames.receive(&packet, now).unwrap().or(confirmed);
        }
        assert!(matches!(confirmed, Some(ServerMessage::Snapshot(s)) if s.field.tick == 5));
    }
    #[test]
    fn malformed_and_oversized_frames_are_rejected() {
        let mut frames = Frames::default();
        assert!(frames.receive(b"bad", Instant::now()).is_err());
        let bomb = miniz_oxide::deflate::compress_to_vec(&vec![0; MAX_WIRE + 1], 1);
        let mut failed = false;
        for packet in packets(1, true, &bomb).unwrap() {
            failed |= frames.receive(&packet, Instant::now()).is_err();
        }
        assert!(failed);
        let notice = encoded(2, false, &ServerMessage::Notice("bad lane".into()));
        assert!(frames.receive(&notice[0], Instant::now()).is_err());
    }

    /// A pilot input command at one sampled time, on the default life.
    fn pilot(sequence: u64, command: ChassisCommand) -> Command {
        pilot_at(1, 0, sequence, sequence * 16_000_000, command)
    }

    /// A pilot input command with an explicit chassis, life and sample time.
    fn pilot_at(
        chassis: u32,
        placement_revision: u64,
        sequence: u64,
        sampled_time_ns: u64,
        command: ChassisCommand,
    ) -> Command {
        Command::PilotInput {
            chassis,
            frame: crate::input_stream::InputFrame {
                input_epoch: 0,
                sequence,
                sampled_time_ns,
                duration_ticks: 16,
                placement_revision,
                command,
            },
        }
    }

    /// The fixed 80-byte encoding of every frame in a batch. It is the equality
    /// witness: f64 `==` cannot tell `-0.0` from `0.0`.
    fn fixed_batch(inputs: &[Command]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for input in inputs {
            let Command::PilotInput { chassis, frame } = input else {
                unreachable!()
            };
            bytes.extend_from_slice(&fixed_frame(*chassis, frame));
        }
        bytes
    }

    /// The inflated `RMI3` body behind one encoded batch.
    fn body_of(packet: &[u8]) -> Vec<u8> {
        miniz_oxide::inflate::decompress_to_vec_with_limit(
            packet.strip_prefix(INPUT_BATCH_MAGIC).unwrap(),
            INPUT_BATCH_BODY_LIMIT,
        )
        .unwrap()
    }

    /// Frames a hand-built body as an `RMI3` packet.
    fn reencoded(body: &[u8]) -> Vec<u8> {
        let mut packet = INPUT_BATCH_MAGIC.to_vec();
        packet.extend(miniz_oxide::deflate::compress_to_vec(body, 1));
        packet
    }

    /// Encodes, decodes and asserts the decoded batch reproduces the encoded
    /// frames bit for bit, within the single-datagram budget.
    fn assert_round_trip(inputs: &VecDeque<Command>) -> Vec<Command> {
        let packet = input_batch(inputs).unwrap();
        assert!(packet.len() <= CHUNK, "batch must fit one datagram");
        let decoded = decode_inputs(&packet).unwrap();
        assert_eq!(decoded.len(), inputs.len());
        assert_eq!(
            fixed_batch(&decoded),
            fixed_batch(&inputs.iter().copied().collect::<Vec<_>>())
        );
        decoded
    }

    /// A deterministic generator, so a failing round trip reproduces.
    struct Lcg(u64);
    impl Lcg {
        fn bits(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }
        /// A finite value in [1, 2) with a full random mantissa.
        fn value(&mut self) -> f64 {
            f64::from_bits((self.bits() & 0x800f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000)
        }
    }

    #[test]
    fn compact_input_batch_round_trips_a_long_randomized_reversal_sequence() {
        let mut random = Lcg(0x5eed_1234_abcd_0001);
        let mut history = VecDeque::new();
        let mut batches = 0;
        for sequence in 1..=400 {
            // Reverse the drive direction on almost every sample, with
            // full-precision aim and steering, so the change mask and both
            // relative deltas are exercised together.
            let sign = if random.bits() & 1 == 0 { 1. } else { -1. };
            history.push_back(pilot(
                sequence,
                ChassisCommand {
                    forward_m_s: sign * 2.5,
                    left_m_s: random.value() - 1.5,
                    yaw_rate_rad_s: -sign * random.value(),
                    aim_yaw_rad: random.value() * 6.0,
                    aim_pitch_rad: random.value() * 0.4 - 0.2,
                },
            ));
            while history.len() > 40 {
                history.pop_front();
            }
            if sequence % 5 == 0 {
                assert_round_trip(&select_inputs(&history, 12));
                batches += 1;
            }
        }
        assert!(batches >= 70);
        eprintln!("randomized reversal batches={batches}");
    }

    #[test]
    fn compact_input_batch_round_trips_an_aim_sweep() {
        let mut history = VecDeque::new();
        let mut batches = 0;
        for sequence in 1..=200 {
            // A fine sweep: neighbours differ in the low mantissa bits, so the
            // mask has to carry every aim value at full f64 precision.
            let aim = 0.000_976_562_5 * sequence as f64 + f64::from_bits(sequence);
            history.push_back(pilot(
                sequence,
                ChassisCommand {
                    aim_yaw_rad: aim,
                    aim_pitch_rad: -0.5 + aim * 0.25,
                    ..Default::default()
                },
            ));
            while history.len() > 32 {
                history.pop_front();
            }
            if sequence % 7 == 0 {
                assert_round_trip(&select_inputs(&history, 12));
                batches += 1;
            }
        }
        assert!(batches >= 28);
    }

    #[test]
    fn compact_input_batch_round_trips_movement_with_fire() {
        let mut history = VecDeque::new();
        let mut batches = 0;
        for sequence in 1..=120 {
            // Fire rides its own reliable-command lane; a stray Fire in the
            // input history must never enter the batch.
            history.push_back(Command::Fire {
                shooter: 1,
                timing: None,
            });
            history.push_back(pilot(
                sequence,
                ChassisCommand {
                    forward_m_s: if (sequence / 6) % 2 == 0 { 1.75 } else { -1.75 },
                    yaw_rate_rad_s: if sequence % 3 == 0 { 0.4 } else { -0.4 },
                    aim_yaw_rad: sequence as f64 * 0.013_37,
                    aim_pitch_rad: 0.05,
                    ..Default::default()
                },
            ));
            while history.len() > 24 {
                history.pop_front();
            }
            if sequence % 4 == 0 {
                let selected = select_inputs(&history, 12);
                assert!(
                    selected
                        .iter()
                        .all(|command| matches!(command, Command::PilotInput { .. }))
                );
                assert_round_trip(&selected);
                batches += 1;
            }
        }
        assert!(batches >= 30);
    }

    #[test]
    fn compact_input_batch_round_trips_a_lost_release_and_the_stop_lease() {
        let mut history = VecDeque::new();
        for sequence in 1..=20 {
            history.push_back(pilot(
                sequence,
                ChassisCommand {
                    forward_m_s: 2.0,
                    aim_yaw_rad: 0.25,
                    ..Default::default()
                },
            ));
        }
        // The release sample is lost, so the newest frames the selector can see
        // still drive. The batch must carry them exactly...
        let decoded = assert_round_trip(&select_inputs(&history, 12));
        assert!(decoded.iter().all(|command| matches!(command,
            Command::PilotInput { frame, .. } if frame.command.forward_m_s == 2.0)));
        // ...and the host must still stop the chassis one lease after the last
        // applied sample, preserving the last aim.
        let now_ns = 20 * 16_000_000;
        let mut stream = crate::input_stream::InputStream::default();
        for command in decoded {
            let Command::PilotInput { frame, .. } = command else {
                unreachable!()
            };
            stream.receive(frame, now_ns, 0).unwrap();
        }
        let stopped = stream
            .expire(now_ns + crate::input_stream::INPUT_LEASE_NS)
            .unwrap();
        assert_eq!(stopped.forward_m_s, 0.);
        assert_eq!(stopped.aim_yaw_rad, 0.25);
        assert!(
            stream
                .expire(now_ns + crate::input_stream::INPUT_LEASE_NS + 1)
                .is_none()
        );
        // A release that does arrive still round-trips and decodes to a zero drive.
        history.push_back(pilot(
            21,
            ChassisCommand {
                aim_yaw_rad: 0.25,
                ..Default::default()
            },
        ));
        let decoded = assert_round_trip(&select_inputs(&history, 12));
        assert!(decoded.iter().any(|command| matches!(command,
            Command::PilotInput { frame, .. }
                if frame.sequence == 21 && frame.command.forward_m_s == 0.0)));
    }

    #[test]
    fn compact_input_batch_preserves_negative_zero_and_full_precision() {
        let mut inputs = VecDeque::new();
        inputs.push_back(pilot(
            1,
            ChassisCommand {
                forward_m_s: 0.0,
                left_m_s: -0.0,
                aim_yaw_rad: f64::from_bits(0x3ff0_0000_0000_0001),
                ..Default::default()
            },
        ));
        // Only the sign of zero changes, which f64 `==` cannot see.
        inputs.push_back(pilot(
            2,
            ChassisCommand {
                forward_m_s: -0.0,
                left_m_s: 0.0,
                aim_yaw_rad: f64::from_bits(0x3ff0_0000_0000_0001),
                ..Default::default()
            },
        ));
        let decoded = assert_round_trip(&inputs);
        let Command::PilotInput { frame, .. } = &decoded[0] else {
            unreachable!()
        };
        assert_eq!(frame.command.forward_m_s.to_bits(), 0.0f64.to_bits());
        assert_eq!(frame.command.left_m_s.to_bits(), (-0.0f64).to_bits());
        let Command::PilotInput { frame, .. } = &decoded[1] else {
            unreachable!()
        };
        assert_eq!(frame.command.forward_m_s.to_bits(), (-0.0f64).to_bits());
        assert_eq!(frame.command.left_m_s.to_bits(), 0.0f64.to_bits());
    }

    #[test]
    fn compact_input_batch_falls_back_to_the_fixed_frame_for_another_life() {
        let mut inputs = VecDeque::new();
        for sequence in 1..=4 {
            inputs.push_back(pilot_at(
                1,
                if sequence == 2 { 9 } else { 0 },
                sequence,
                sequence * 16_000_000,
                ChassisCommand {
                    forward_m_s: 1.0,
                    ..Default::default()
                },
            ));
        }
        // The header comes from the newest frame, so the mismatched frame keeps
        // its fixed 80-byte encoding and still round-trips exactly.
        let packet = input_batch(&inputs).unwrap();
        assert_eq!(
            fixed_batch(&decode_inputs(&packet).unwrap()),
            fixed_batch(&inputs.iter().copied().collect::<Vec<_>>())
        );
    }

    #[test]
    fn compact_batch_carries_the_scripted_workload_not_just_its_bytes() {
        // An idle and a driving pilot produce batches of similar size, so the
        // decoded values, not the byte count, must show which one was sent.
        let idle: VecDeque<_> = (1..=8)
            .map(|sequence| pilot(sequence, ChassisCommand::default()))
            .collect();
        let drive: VecDeque<_> = (1..=8)
            .map(|sequence| {
                pilot(
                    sequence,
                    ChassisCommand {
                        forward_m_s: 2.0,
                        aim_yaw_rad: 0.5,
                        ..Default::default()
                    },
                )
            })
            .collect();
        let idle_packet = input_batch(&idle).unwrap();
        let drive_packet = input_batch(&drive).unwrap();
        let idle_decoded = decode_inputs(&idle_packet).unwrap();
        let drive_decoded = decode_inputs(&drive_packet).unwrap();
        assert!(idle_decoded.iter().all(|command| matches!(command,
            Command::PilotInput { frame, .. } if frame.command.forward_m_s == 0.0)));
        assert!(drive_decoded.iter().all(|command| matches!(command,
            Command::PilotInput { frame, .. }
                if frame.command.forward_m_s == 2.0 && frame.command.aim_yaw_rad == 0.5)));
        eprintln!(
            "workload idle_bytes={} drive_bytes={}",
            idle_packet.len(),
            drive_packet.len()
        );
    }

    #[test]
    fn older_fixed_rmi2_batches_still_decode() {
        let inputs: VecDeque<_> = (1..=3)
            .map(|sequence| {
                pilot(
                    sequence,
                    ChassisCommand {
                        forward_m_s: 1.5,
                        aim_yaw_rad: 0.7,
                        ..Default::default()
                    },
                )
            })
            .collect();
        let mut body = vec![inputs.len() as u8];
        for input in &inputs {
            let Command::PilotInput { chassis, frame } = input else {
                unreachable!()
            };
            body.extend_from_slice(&fixed_frame(*chassis, frame));
        }
        let mut packet = INPUT_BATCH_MAGIC_V2.to_vec();
        packet.extend(miniz_oxide::deflate::compress_to_vec(&body, 1));
        assert_eq!(
            fixed_batch(&decode_inputs(&packet).unwrap()),
            fixed_batch(&inputs.iter().copied().collect::<Vec<_>>())
        );
    }

    #[test]
    fn compact_input_batch_refuses_malformed_headers_masks_counts_and_oversized_batches() {
        // A malformed header: a count byte with no shared identity behind it.
        let mut short = INPUT_BATCH_MAGIC.to_vec();
        short.extend(miniz_oxide::deflate::compress_to_vec(&[1u8], 1));
        assert!(decode_inputs(&short).is_err());

        // A giant count and a zero count, which no encoder can produce.
        let mut giant = vec![13u8];
        giant.extend([0u8; INPUT_HEADER_BYTES]);
        assert!(decode_inputs(&reencoded(&giant)).is_err());
        let mut zero = vec![0u8];
        zero.extend([0u8; INPUT_HEADER_BYTES]);
        assert!(decode_inputs(&reencoded(&zero)).is_err());

        let inputs: VecDeque<_> = (1..=4)
            .map(|sequence| {
                pilot(
                    sequence,
                    ChassisCommand {
                        forward_m_s: 1.0,
                        ..Default::default()
                    },
                )
            })
            .collect();

        // A wrong mask: a bit outside the five command values.
        let mut body = body_of(&input_batch(&inputs).unwrap());
        body[1 + INPUT_HEADER_BYTES + 1] |= 0x20;
        assert!(decode_inputs(&reencoded(&body)).is_err());

        // A wrong flag: a reserved bit beside the two absolute markers.
        let mut body = body_of(&input_batch(&inputs).unwrap());
        body[1 + INPUT_HEADER_BYTES + 2] |= 0x80;
        assert!(decode_inputs(&reencoded(&body)).is_err());

        // A batch over the 1,000-byte single-datagram bound, both as a whole
        // packet and as an inflated body past the maximum.
        let mut over = INPUT_BATCH_MAGIC.to_vec();
        over.extend(vec![0u8; CHUNK]);
        assert!(decode_inputs(&over).is_err());
        let bomb = reencoded(&vec![0u8; INPUT_BATCH_BODY_LIMIT + 1]);
        assert!(decode_inputs(&bomb).is_err());

        // The worst batch the codec can build: eleven frames whose identity is
        // not the shared header's keep the fixed 80-byte encoding, and the
        // newest frame changes all five full-precision values.
        let maximal: VecDeque<_> = (1..=12)
            .map(|sequence| {
                pilot_at(
                    1,
                    u64::from(sequence == 12),
                    sequence,
                    sequence * 16_000_000,
                    ChassisCommand {
                        forward_m_s: f64::from_bits(0x3ff0_0000_0000_0001 + sequence),
                        left_m_s: -f64::from_bits(0x3ff8_0000_0000_0003 + sequence),
                        yaw_rate_rad_s: f64::from_bits(0x3fe0_0000_0000_0007 + sequence),
                        aim_yaw_rad: -f64::from_bits(0x4000_0000_0000_0005 + sequence),
                        aim_pitch_rad: f64::from_bits(0x3fd0_0000_0000_0009 + sequence),
                    },
                )
            })
            .collect();
        assert!(input_batch(&maximal).unwrap().len() <= CHUNK);
    }
}
