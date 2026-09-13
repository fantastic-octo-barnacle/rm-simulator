// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! The per-peer UDP codec, with no socket in it.
//!
//! One peer's whole wire behaviour lives here: the RMG1 fragment framing, the
//! RMI2 input batches, the RMC1 compressed commands, the RMO3 owner anchor, the
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
use rm_simulator_world::Team;
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
/// First four bytes of a deflated pilot input batch.
const INPUT_BATCH_MAGIC: &[u8; 4] = b"RMI2";
/// Ceiling for inflating one input batch, so a hostile peer cannot force a
/// larger allocation.
const INPUT_BATCH_LIMIT: usize = 16 * 1024;
/// Host frames queued past this native backlog skip their periodic checkpoint.
pub(crate) const CONGESTED_PENDING_BYTES: u32 = 64 * 1024;

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

/// Encodes an input batch as `RMI2` followed by a deflate stream: one count byte
/// and then 80 bytes per frame. Refuses an empty batch, more than 12 frames and
/// any command that is not pilot input.
fn input_batch(inputs: &VecDeque<Command>) -> io::Result<Vec<u8>> {
    if inputs.is_empty() || inputs.len() > 12 {
        return Err(io_error("invalid input count"));
    }
    let mut bytes = vec![inputs.len() as u8];
    for input in inputs {
        let Command::PilotInput { chassis, frame } = input else {
            return Err(io_error("non-pilot input"));
        };
        bytes.extend(chassis.to_le_bytes());
        for n in [
            frame.input_epoch,
            frame.sequence,
            frame.sampled_time_ns,
            frame.placement_revision,
        ] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend(frame.duration_ticks.to_le_bytes());
        for n in [
            frame.command.forward_m_s,
            frame.command.left_m_s,
            frame.command.yaw_rate_rad_s,
            frame.command.aim_yaw_rad,
            frame.command.aim_pitch_rad,
        ] {
            bytes.extend(n.to_le_bytes());
        }
    }
    let mut packet = INPUT_BATCH_MAGIC.to_vec();
    packet.extend(miniz_oxide::deflate::compress_to_vec(&bytes, 1));
    Ok(packet)
}
/// Decodes an `RMI2` input batch in encoded order, oldest first. Refuses a
/// missing magic, an inflated size that does not match the count, a count
/// outside 1..=12 and any non-finite command.
fn decode_inputs(packet: &[u8]) -> io::Result<Vec<Command>> {
    let compressed = packet
        .strip_prefix(INPUT_BATCH_MAGIC)
        .ok_or_else(|| io_error("invalid input batch"))?;
    let bytes =
        miniz_oxide::inflate::decompress_to_vec_with_limit(compressed, 961).map_err(io_error)?;
    let count = bytes.first().copied().unwrap_or(0) as usize;
    if !(1..=12).contains(&count) || bytes.len() != 1 + count * 80 {
        return Err(io_error("invalid input batch length"));
    }
    bytes[1..]
        .as_chunks::<80>()
        .0
        .iter()
        .map(|frame| {
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
        })
        .collect()
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
            if let Some(retire) = self.encoder.feedback(feedback) {
                let bytes = crate::udp_snapshot::encode(retire);
                self.queue_control(now, &bytes)?;
            }
            return Ok(PeerRequest::Handled);
        }
        if payload.starts_with(INPUT_BATCH_MAGIC) {
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
        // a backlog skips below.
        if frame.periodic
            && let Some(chassis) = self.chassis
            && let ServerMessage::Snapshot(state) = frame.message()
            && let Some(anchor) = crate::owner_stream::OwnerAnchor::from_state(state, chassis)
        {
            // Oversize anchors never fragment. Full checkpoints remain the recovery path.
            if let Ok(bytes) = anchor.encode() {
                self.encoding.owner_updates += 1;
                self.encoding.owner_bytes += bytes.len() as u64;
                self.pacer.owner(self.elapsed(now), bytes);
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
    /// arrived before the welcome or is no newer than the last one, for an
    /// incomplete frame and for a frame that decoded to nothing. Errors on an
    /// invalid welcome, a protocol mismatch and any framing or baseline failure.
    pub(crate) fn receive(
        &mut self,
        payload: &[u8],
        now: Instant,
    ) -> io::Result<Option<ClientEvent>> {
        if payload.starts_with(crate::owner_stream::MAGIC) {
            let anchor = crate::owner_stream::OwnerAnchor::decode(payload)?;
            if !self.welcomed || anchor.snapshot_id <= self.latest_owner {
                return Ok(None);
            }
            self.latest_owner = anchor.snapshot_id;
            return Ok(Some(ClientEvent::Anchor(Box::new(anchor))));
        }
        let Some(message) = self.frames.receive(payload, now)? else {
            return Ok(None);
        };
        if let ServerMessage::Welcome(welcome) = message {
            if self.welcomed || welcome.protocol != crate::protocol::PROTOCOL_VERSION {
                return Err(io_error("invalid welcome"));
            }
            self.welcomed = true;
            return Ok(Some(ClientEvent::Welcome(welcome)));
        }
        Ok(Some(ClientEvent::Message(message)))
    }
    /// Answer every baseline the decoder stored, retired or found missing.
    pub(crate) fn acknowledge(&mut self, now: Instant) -> io::Result<()> {
        while let Some(feedback) = self.frames.feedback.pop_front() {
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
    use rm_simulator_world::{Field, FieldConfig};

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
}
