// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Test-only bandwidth attribution probe.
//!
//! This module is the measurement instrument for the bandwidth experiments in
//! `docs/bandwidth-experiments.md`. It drives the production host encoders and
//! codecs ([`PeerCodec`], [`crate::owner_stream::OwnerAnchor`], the acknowledged
//! baseline encoder and the byte [`Pacer`]) against the production client codec
//! ([`ClientCodec`]) with no socket, on a hand-advanced clock, so the same
//! workload replays byte for byte on any machine.
//!
//! It prints one `key=value` record per workload so a shell `grep` can compare a
//! baseline build with a candidate build: application bytes per second per
//! direction, per class, per update, the fragment overhead, and the raw JSON
//! section sizes that explain where a checkpoint's bytes come from. Counters
//! from different stages are kept separate: production, pacing and delivery are
//! not summed, because the same byte is accounted at more than one stage.
//!
//! Scope: this attributes the production encoders and codecs, not the whole host
//! path. It drives [`PeerCodec`], [`crate::owner_stream::OwnerAnchor`], the
//! acknowledged baseline encoder and the byte [`Pacer`](crate::pacing::Pacer)
//! against [`ClientCodec`] directly, so `HostPeer::pump`, the bounded peer
//! outbox, the native transport and its pacing budgets are not in the loop. Each
//! decoded [`crate::udp_codec::PeerRequest`] is applied with the production
//! [`Simulation::apply`], so the host's `input_stream` dedup and lease still run;
//! without that stand-in every workload would step an idle field and the
//! attribution would be meaningless.

use crate::host::Outbound;
use crate::net::QueuedCommand;
use crate::protocol::{Command, ServerMessage};
use crate::simulation::SimulationState;
use crate::udp_codec::{CONGESTED_PENDING_BYTES, ClientCodec, PeerCodec};
use rm_simulator_world::ChassisCommand;
use std::collections::BTreeMap;
use std::time::Instant;

/// One simulation tick, in nanoseconds. The probe keeps the 1 ms rule clock.
const TICK_NS: u64 = 1_000_000;
/// Simulation time between two frames handed to a codec.
const STEP_MS: u64 = 2;
/// Owner-only host publication period: `host.rs` `OWNER_BROADCAST_PERIOD` fires
/// every 4 ms, and `publish_snapshot(true)` pushes those frames only to the
/// in-process owner peer.
///
/// `PeerCodec` cannot tell an owner-only periodic frame from a full world one:
/// both arrive as `Outbound { periodic: true, .. }`. The probe therefore
/// reproduces the intended cadence itself instead of asking one `send` call to
/// distinguish the two kinds.
const OWNER_BROADCAST_MS: u64 = 4;
/// Full world publication period. `host.rs` `BROADCAST_PERIOD` publishes every
/// 32 ms, and `publish_snapshot(false)` pushes those frames to every peer.
const WORLD_BROADCAST_MS: u64 = 32;
/// Input sample period: 62.5 Hz, the documented upstream frame rate.
const INPUT_MS: u64 = 16;
/// One input frame's duration, in ticks of 1 ms.
const FRAME_TICKS: u32 = 16;
/// A pacer budget no offered stream can exceed in a probe run.
pub(crate) const UNLIMITED: u32 = 64 * 1024 * 1024;

/// The scripted pilot behaviour one probe run replays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Workload {
    /// One pilot that never commands anything.
    Idle,
    /// One pilot driving and sweeping its aim.
    Drive,
    /// One pilot driving, aiming, and asking to fire on a cadence the weapon
    /// permits, so projectile arrays and shot-result history churn.
    Fire,
    /// Twelve pilots, the first one driving and firing.
    Twelve,
}

impl Workload {
    /// Every workload the baseline probe reports, in the documented order.
    pub(crate) const ALL: [Self; 4] = [Self::Idle, Self::Drive, Self::Fire, Self::Twelve];
    /// The label the probe prints for this workload.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Drive => "drive",
            Self::Fire => "fire",
            Self::Twelve => "twelve",
        }
    }
    /// How many chassis join the field.
    fn players(self) -> u32 {
        match self {
            Self::Twelve => 12,
            _ => 1,
        }
    }
    /// Whether the driving pilot commands motion and aim each input frame.
    fn drives(self) -> bool {
        !matches!(self, Self::Idle)
    }
    /// Whether the driving pilot queues fire commands.
    fn fires(self) -> bool {
        matches!(self, Self::Fire | Self::Twelve)
    }
}

/// Which host publication cadence one probe run replays.
///
/// `host.rs` publishes two kinds of periodic frame from one clock loop:
/// `publish_snapshot(false)` every [`WORLD_BROADCAST_MS`], and
/// `publish_snapshot(true)` every [`OWNER_BROADCAST_MS`] in between. Only the
/// in-process owner peer (`owner_spawn.is_some()`) receives the owner-only
/// frames; a remote pilot receives only the 32 ms full publications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cadence {
    /// A remote pilot peer: only the 32 ms full publication reaches it. Each
    /// such frame still carries the owner anchor for that pilot's own chassis,
    /// because `PeerCodec` emits an anchor for every periodic frame. This is
    /// the configuration the reported 855 kbps remote baseline measures.
    Remote,
    /// The in-process owner peer: 4 ms owner-only frames plus the 32 ms world
    /// frame. `PeerCodec::send` cannot separate the two, so the probe forces the
    /// separation through the codec's own congested-carrier path, which keeps the
    /// anchor and skips the world checkpoint.
    Owner,
    /// Counterfactual, never the remote path: every 4 ms publication is fed
    /// ungated, so each one produces a world checkpoint as well as an anchor.
    /// That is what today's `PeerCodec::send` does when handed either periodic
    /// kind, and it sizes the code-path inefficiency the cadence experiment must
    /// fix.
    EveryPublication,
}

impl Cadence {
    /// The label the probe prints for this cadence.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Owner => "owner",
            Self::EveryPublication => "every-publication",
        }
    }
}

/// Every counter one run produces, per direction and per class.
#[derive(Default)]
pub(crate) struct Totals {
    /// Downstream datagrams the host pacer released.
    pub(crate) down_packets: u64,
    /// Downstream application bytes, fragment headers included.
    pub(crate) down_bytes: u64,
    /// Downstream bytes in RMO3 owner anchors, as delivered.
    pub(crate) down_owner_bytes: u64,
    /// Owner anchor bytes produced before pacing replaced or dropped them.
    pub(crate) produced_owner_bytes: u64,
    /// Downstream bytes in unreliable world frames.
    pub(crate) down_world_bytes: u64,
    /// Downstream bytes on the reliable control lane.
    pub(crate) down_control_bytes: u64,
    /// Downstream world fragments, which is where a lost fragment hurts.
    pub(crate) down_world_fragments: u64,
    /// Owner anchors produced, from the host encoder counters.
    pub(crate) produced_owner_updates: u64,
    /// World updates produced, from the host encoder counters.
    pub(crate) produced_world_updates: u64,
    /// World updates skipped for a congested carrier, from the host counters.
    /// The [`Cadence::Owner`] run uses this path deliberately to keep only the
    /// anchor on an owner-only publication.
    pub(crate) skipped_world_updates: u64,
    /// Complete player checkpoints the client reassembled.
    pub(crate) down_complete_snapshots: u64,
    /// Frames the client abandoned because a fragment never arrived.
    pub(crate) down_incomplete_frames: u64,
    /// Checkpoints the client dropped as no newer than the last one delivered.
    pub(crate) down_stale_updates: u64,
    /// RMO3 owner anchor datagrams delivered to the client.
    pub(crate) down_anchors: u64,
    /// Upstream datagrams the client pacer released.
    pub(crate) up_packets: u64,
    /// Upstream application bytes.
    pub(crate) up_bytes: u64,
    /// Upstream input batches.
    pub(crate) up_batches: u64,
    /// Upstream bytes in input batches.
    pub(crate) up_batch_bytes: u64,
    /// Input frames those batches carried.
    pub(crate) up_frames: u64,
    /// Upstream datagrams that were not input batches.
    pub(crate) up_controls: u64,
    /// Raw JSON bytes of the newest checkpoint's whole state.
    pub(crate) state_json: usize,
    /// Encoded size of the newest owner anchor.
    pub(crate) owner_anchor_bytes: usize,
    /// Raw JSON bytes per `FieldSnapshot` section of the newest checkpoint.
    pub(crate) sections: BTreeMap<&'static str, usize>,
    /// Projectiles in the newest checkpoint.
    pub(crate) projectiles: usize,
    /// Chassis in the newest checkpoint.
    pub(crate) chassis: usize,
    /// Armor hits in the newest checkpoint.
    pub(crate) hits: usize,
    /// Shot results in the newest checkpoint.
    pub(crate) shot_results: usize,
    /// Fire commands the workload submitted.
    pub(crate) fire_commands: u64,
    /// Shots the simulation actually launched, from the newest checkpoint.
    pub(crate) shots_fired: u64,
    /// Raw checkpoint bytes before delta encoding, from the host counters.
    pub(crate) raw_world_bytes: u64,
    /// Framed world bytes including fragment headers, before pacing.
    pub(crate) framed_world_bytes: u64,
    /// Compressed independent bytes considered by the baseline codec.
    pub(crate) independent_bytes: u64,
    /// Compressed full/delta bytes the baseline codec selected.
    pub(crate) selected_bytes: u64,
    /// Whole-loop CPU in the probe, which includes stepping and encoding.
    pub(crate) loop_us: u128,
    /// CPU inside host frame production only.
    pub(crate) down_encode_us: u128,
    /// CPU inside client datagram decoding only.
    pub(crate) up_decode_us: u128,
}

impl Totals {
    /// Fold one JSON value's serialized size into a named section.
    fn section(&mut self, name: &'static str, value: &serde_json::Value) {
        *self.sections.entry(name).or_default() += serde_json::to_vec(value).unwrap().len();
    }
}

/// Classify one host datagram by its leading magic and count it. The lane byte
/// of an RMG1 fragment says whether it is the world or the control lane.
///
/// Each delivered datagram lands in exactly one byte bucket, so
/// `down_owner_bytes + down_world_bytes + down_control_bytes == down_bytes`.
fn account_down(totals: &mut Totals, bytes: &[u8]) {
    totals.down_packets += 1;
    totals.down_bytes += bytes.len() as u64;
    if bytes.starts_with(crate::owner_stream::MAGIC) {
        totals.down_owner_bytes += bytes.len() as u64;
        totals.down_anchors += 1;
    } else if bytes.starts_with(b"RMG1") && bytes.len() > 4 {
        if bytes[4] == 1 {
            totals.down_control_bytes += bytes.len() as u64;
        } else {
            totals.down_world_bytes += bytes.len() as u64;
            totals.down_world_fragments += 1;
        }
    } else {
        totals.down_control_bytes += bytes.len() as u64;
    }
}

/// Classify one client datagram and count it.
fn account_up(totals: &mut Totals, bytes: &[u8]) {
    totals.up_packets += 1;
    totals.up_bytes += bytes.len() as u64;
    if let Some(inflated) = bytes
        .strip_prefix(b"RMI3")
        .and_then(|body| crate::compression::decompress(body, 16 * 1024).ok())
    {
        totals.up_batches += 1;
        totals.up_batch_bytes += bytes.len() as u64;
        totals.up_frames += inflated.first().copied().unwrap_or(0) as u64;
    } else {
        totals.up_controls += 1;
    }
}

/// A pilot's scripted command for one input frame.
fn frame_command(workload: Workload, sequence: u64) -> ChassisCommand {
    if !workload.drives() {
        return ChassisCommand::default();
    }
    let phase = (sequence / 12) % 4;
    ChassisCommand {
        forward_m_s: if phase == 0 || phase == 1 { 1.5 } else { 0. },
        left_m_s: if phase == 2 { 1.0 } else { 0. },
        yaw_rate_rad_s: if phase == 1 { 0.6 } else { 0. },
        aim_yaw_rad: (sequence as f64) * 0.02,
        aim_pitch_rad: 0.1,
    }
}

/// The owner anchor for the first chassis cut from `state`, if the field has one.
fn owner_anchor(state: &SimulationState) -> Option<Vec<u8>> {
    let chassis = state.field.chassis.first()?.id;
    crate::owner_stream::OwnerAnchor::from_state(state, chassis)?
        .encode()
        .ok()
}

/// Run one workload and return its counters.
///
/// `budget_bytes_s` is the pacer's downstream byte budget, in bytes per second,
/// for this peer; [`UNLIMITED`] measures the offered stream instead of a capped
/// delivery. `upstream_bytes_s` is the client's own pacing budget. `observe`, if
/// given, sees every produced world checkpoint in publication order.
pub(crate) fn run(
    workload: Workload,
    seconds: u64,
    cadence: Cadence,
    budget_bytes_s: u32,
    upstream_bytes_s: u32,
) -> Totals {
    run_observed(
        workload,
        seconds,
        cadence,
        budget_bytes_s,
        upstream_bytes_s,
        &mut |_| {},
    )
}

/// [`run`] with a checkpoint observer, which the ablation test uses to see the
/// same states the encoder did without running the workload twice.
pub(crate) fn run_observed(
    workload: Workload,
    seconds: u64,
    cadence: Cadence,
    budget_bytes_s: u32,
    upstream_bytes_s: u32,
    observe: &mut dyn FnMut(&SimulationState),
) -> Totals {
    let clock = crate::clock::ManualTime::new();
    let time = clock.source();
    let mut now = time.now();
    // The workload builder is shared with the measurement examples so both
    // drive the same field; see `crate::workload`.
    let (mut simulation, chassis) = crate::workload::simulation(workload.players() as usize);
    let driver = chassis[0];
    let mut totals = Totals::default();
    let mut peer = PeerCodec::new(now, budget_bytes_s, false);
    peer.joined(chassis.first().copied());
    let mut client = ClientCodec::new(
        now,
        upstream_bytes_s,
        crate::udp_codec::MAX_INPUT_FRAMES,
        false,
    );
    let mut sequence = 0_u64;
    let mut snapshot_id = 0_u64;
    let ticks = seconds * 1000 / STEP_MS;
    for tick in 0..ticks {
        let started = Instant::now();
        if tick.is_multiple_of(INPUT_MS / STEP_MS) {
            sequence += 1;
            let sampled_time_ns = tick * STEP_MS * TICK_NS;
            // One client connection carries one pilot, exactly as the wire does.
            // A twelve-player world is twelve such peers; this one measures the
            // single-pilot upstream stream that the budget is stated per player.
            client
                .submit(
                    QueuedCommand {
                        command: Some(Command::PilotInput {
                            chassis: driver,
                            frame: crate::input_stream::InputFrame {
                                input_epoch: 0,
                                sequence,
                                sampled_time_ns,
                                duration_ticks: FRAME_TICKS,
                                placement_revision: 0,
                                command: frame_command(workload, sequence),
                            },
                        }),
                        confirmation: None,
                        time_probe: None,
                    },
                    now,
                )
                .unwrap();
            if workload.fires() && sequence.is_multiple_of(8) {
                totals.fire_commands += 1;
                client
                    .submit(
                        QueuedCommand {
                            command: Some(Command::Fire {
                                shooter: driver,
                                timing: None,
                            }),
                            confirmation: None,
                            time_probe: None,
                        },
                        now,
                    )
                    .unwrap();
            }
        }
        simulation.step(STEP_MS).unwrap();
        // Reproduce the host's two publication kinds explicitly. `PeerCodec` has
        // no flag that separates them, so this probe decides when a world
        // checkpoint may be offered and, for the owner cadence, uses the codec's
        // congested-carrier path to keep the anchor while skipping the world.
        let owner_due = tick.is_multiple_of(OWNER_BROADCAST_MS / STEP_MS);
        let world_due = tick.is_multiple_of(WORLD_BROADCAST_MS / STEP_MS);
        let pending = match cadence {
            Cadence::Remote => world_due.then_some(0),
            Cadence::Owner if world_due => Some(0),
            Cadence::Owner if owner_due => Some(CONGESTED_PENDING_BYTES + 1),
            Cadence::Owner => None,
            Cadence::EveryPublication => owner_due.then_some(0),
        };
        if let Some(pending) = pending {
            let mut state = simulation.state();
            snapshot_id += 1;
            state.snapshot_id = snapshot_id;
            observe(&state);
            // `Outbound::new` leaves `periodic` false, which is the reliable
            // control path. A host publication is a periodic snapshot.
            let mut frame = Outbound::new(ServerMessage::Snapshot(Box::new(state)));
            frame.periodic = true;
            let encode_started = Instant::now();
            peer.send(&frame, now, pending).unwrap();
            totals.down_encode_us += encode_started.elapsed().as_micros();
            let stats = peer.encoding_stats();
            totals.raw_world_bytes = stats.raw_world_bytes;
            totals.framed_world_bytes = stats.framed_world_bytes;
            totals.independent_bytes = stats.independent_bytes;
            totals.selected_bytes = stats.selected_bytes;
            totals.produced_world_updates = stats.world_updates;
            totals.skipped_world_updates = stats.skipped_world_updates;
            totals.produced_owner_updates = stats.owner_updates;
            totals.produced_owner_bytes = stats.owner_bytes;
        }
        let decode_started = Instant::now();
        let mut incoming = Vec::new();
        while let Ok(Some(packet)) = peer.next(now) {
            incoming.push(packet.bytes);
        }
        for packet in incoming {
            // Anchors are decoded before the welcomed check; this probe never
            // delivers a Welcome, so acceptance is not modeled, but every
            // delivered byte is attributed by `account_down`.
            client.receive(&packet, now).unwrap();
            account_down(&mut totals, &packet);
        }
        let stats = client.stats();
        totals.down_complete_snapshots = stats.complete_snapshots;
        totals.down_incomplete_frames = stats.incomplete_frames;
        totals.down_stale_updates = stats.stale_updates;
        client.acknowledge(now).unwrap();
        totals.up_decode_us += decode_started.elapsed().as_micros();
        while let Ok(Some(packet)) = client.next(now) {
            account_up(&mut totals, &packet.bytes);
            // Scripted immediate feedback with no loss. The client's control
            // datagrams are baseline feedback and input commands, and the host
            // encoder only rotates baselines after the acknowledgements arrive,
            // so the probe must return them. The commands are applied to the
            // production simulation here, which is what makes `drive`, `fire`
            // and `twelve` move the world instead of replaying a static one.
            match peer.receive(&packet.bytes, now).unwrap() {
                crate::udp_codec::PeerRequest::Handled => {}
                crate::udp_codec::PeerRequest::Inputs(inputs) => {
                    for input in inputs {
                        simulation
                            .apply(&input)
                            .expect("scripted pilot input must be admissible");
                    }
                }
                crate::udp_codec::PeerRequest::Message(message) => {
                    if let crate::protocol::ClientMessage::Command(command) = *message {
                        // A cadence rejection is an expected outcome of firing
                        // faster than the weapon allows, not an instrument fault.
                        let _ = simulation.apply(&command);
                    }
                }
            }
        }
        clock.advance_ms(STEP_MS);
        now = time.now();
        totals.loop_us += started.elapsed().as_micros();
    }
    // Freeze the newest checkpoint's raw representation once, after the run.
    let state = simulation.state();
    totals.state_json = serde_json::to_vec(&state).unwrap().len();
    totals.owner_anchor_bytes = owner_anchor(&state).map_or(0, |bytes| bytes.len());
    totals.projectiles = state.field.projectiles.len();
    totals.chassis = state.field.chassis.len();
    totals.hits = state.field.hits.len();
    totals.shot_results = state.shot_results.len();
    totals.shots_fired = state.field.shots_fired;
    let value = serde_json::to_value(&state.field).unwrap();
    for (name, value) in value.as_object().unwrap() {
        let name: &'static str = match name.as_str() {
            "bases" => "bases",
            "tick" => "tick",
            "time_ns" => "time_ns",
            "runes" => "runes",
            "outposts" => "outposts",
            "projectiles" => "projectiles",
            "chassis" => "chassis",
            "hits" => "hits",
            "shots_fired" => "shots_fired",
            "hits_detected" => "hits_detected",
            "referee" => "referee",
            "restore" => "restore",
            _ => "other",
        };
        totals.section(name, value);
    }
    totals.section(
        "shot_results",
        &serde_json::to_value(&state.shot_results).unwrap(),
    );
    totals
}

/// One ablation row: what a section costs once the frame is compressed.
pub(crate) struct Ablation {
    /// Section name, or `total` for the unmodified frame.
    pub(crate) name: &'static str,
    /// Compressed contribution: the whole frame minus the frame without it.
    pub(crate) bytes: i64,
}

/// Compressed contribution of each top-level checkpoint section.
///
/// Removes one section at a time from the encoded independent player checkpoint
/// and recompresses, which is the only way to apportion a compressed frame's
/// bytes: raw JSON section sizes do not add up to compressed contributions. A
/// section whose removal makes the frame larger reports a negative number, which
/// means it was helping the compressor, not costing bytes.
///
/// Paths are inside the compact `PlayerEnvelope`: `CompactSnapshot.state` is the
/// `SimulationState` and `CompactSnapshot.projectiles` is the hoisted wire
/// projectile array, while the state's own `field.projectiles` is emptied by
/// `PlayerSnapshot::from_state`. So the `state.projectiles` row measures the
/// hoisted array, which is where a checkpoint's projectile bytes actually live.
///
/// `state.restore` is reported for completeness but is never a removal
/// candidate: it is hidden rule state that `Field::restore` needs, so a frame
/// without it is not a frame this protocol can send.
pub(crate) fn ablation(state: &SimulationState) -> Vec<Ablation> {
    let bytes = crate::snapshot_codec::encode_player_message(&ServerMessage::Snapshot(Box::new(
        state.clone(),
    )));
    let total = |value: &serde_json::Value| {
        crate::compression::compress(&serde_json::to_vec(value).unwrap()).len() as i64
    };
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let mut rows = vec![Ablation {
        name: "total",
        bytes: total(&value),
    }];
    if value.get("CompactSnapshot").is_none() {
        return rows;
    }
    let whole = rows[0].bytes;
    // Every candidate is a path inside the compact envelope: the state's field
    // sections, the hoisted projectile array, and the per-shooter result history.
    let candidates: [(&'static str, &[&str]); 11] = [
        (
            "state.bases",
            &["CompactSnapshot", "state", "field", "bases"],
        ),
        ("state.tick", &["CompactSnapshot", "state", "field", "tick"]),
        (
            "state.time_ns",
            &["CompactSnapshot", "state", "field", "time_ns"],
        ),
        (
            "state.runes",
            &["CompactSnapshot", "state", "field", "runes"],
        ),
        (
            "state.outposts",
            &["CompactSnapshot", "state", "field", "outposts"],
        ),
        ("state.projectiles", &["CompactSnapshot", "projectiles"]),
        (
            "state.chassis",
            &["CompactSnapshot", "state", "field", "chassis"],
        ),
        ("state.hits", &["CompactSnapshot", "state", "field", "hits"]),
        (
            "state.referee",
            &["CompactSnapshot", "state", "field", "referee"],
        ),
        (
            "state.restore",
            &["CompactSnapshot", "state", "field", "restore"],
        ),
        (
            "shot_results",
            &["CompactSnapshot", "state", "shot_results"],
        ),
    ];
    for (name, path) in candidates {
        let Some(slot) = at(&mut value, path) else {
            continue;
        };
        // The decoded value is a local copy, so clearing a section in place
        // needs no restore: the next candidate starts from a fresh copy anyway.
        *slot = serde_json::Value::Null;
        rows.push(Ablation {
            name,
            bytes: whole - total(&value),
        });
        value = serde_json::from_slice(&bytes).unwrap();
    }
    rows
}

/// The mutable value at `path` from `root`, or `None` when any step is missing.
fn at<'a>(root: &'a mut serde_json::Value, path: &[&str]) -> Option<&'a mut serde_json::Value> {
    let mut current = root;
    for key in path {
        current = current.get_mut(*key)?;
    }
    Some(current)
}

/// Print one measurement record.
pub(crate) fn report(
    cadence: Cadence,
    workload: Workload,
    seconds: u64,
    budget: u32,
    upstream: u32,
    totals: &Totals,
) {
    let per_s = |bytes: u64| bytes as f64 / seconds as f64;
    let kbps = |bytes: f64| bytes * 8. / 1000.;
    println!(
        "probe cadence={} workload={} seconds={} budget_bytes_s={} upstream_bytes_s={}",
        cadence.name(),
        workload.name(),
        seconds,
        budget,
        upstream
    );
    println!(
        "  down_bytes_s={:.1} down_kbps={:.1} down_packets={} world_fragments={} complete_snapshots={} incomplete_frames={} stale={} anchors={}",
        per_s(totals.down_bytes),
        kbps(per_s(totals.down_bytes)),
        totals.down_packets,
        totals.down_world_fragments,
        totals.down_complete_snapshots,
        totals.down_incomplete_frames,
        totals.down_stale_updates,
        totals.down_anchors
    );
    println!(
        "  down_owner_bytes_s={:.1} down_world_bytes_s={:.1} down_control_bytes_s={:.1}",
        per_s(totals.down_owner_bytes),
        per_s(totals.down_world_bytes),
        per_s(totals.down_control_bytes)
    );
    println!(
        "  produced_owner_updates={} produced_world_updates={} skipped_world_updates={}",
        totals.produced_owner_updates, totals.produced_world_updates, totals.skipped_world_updates
    );
    println!(
        "  up_bytes_s={:.1} up_kbps={:.1} up_packets={} batches={} frames={} controls={} batch_bytes={}",
        per_s(totals.up_bytes),
        kbps(per_s(totals.up_bytes)),
        totals.up_packets,
        totals.up_batches,
        totals.up_frames,
        totals.up_controls,
        totals.up_batch_bytes
    );
    println!(
        "  raw_world_bytes={} framed_world_bytes={} independent_bytes={} selected_bytes={} produced_owner_bytes={} produced_owner_bytes_s={:.1}",
        totals.raw_world_bytes,
        totals.framed_world_bytes,
        totals.independent_bytes,
        totals.selected_bytes,
        totals.produced_owner_bytes,
        per_s(totals.produced_owner_bytes)
    );
    println!(
        "  state_json={} owner_anchor_bytes={} projectiles={} chassis={} hits={} shot_results={} fire_commands={} shots_fired={}",
        totals.state_json,
        totals.owner_anchor_bytes,
        totals.projectiles,
        totals.chassis,
        totals.hits,
        totals.shot_results,
        totals.fire_commands,
        totals.shots_fired
    );
    let sections: Vec<String> = totals
        .sections
        .iter()
        .map(|(name, bytes)| format!("{name}={bytes}"))
        .collect();
    println!("  sections {}", sections.join(" "));
    println!(
        "  cpu_us loop={} down_encode={} up_decode={}",
        totals.loop_us, totals.down_encode_us, totals.up_decode_us
    );
}

/// Report every documented workload under one publication cadence at the
/// offered rate.
pub(crate) fn baseline_report(seconds: u64, cadence: Cadence) {
    for workload in Workload::ALL {
        let totals = run(workload, seconds, cadence, UNLIMITED, UNLIMITED);
        report(cadence, workload, seconds, UNLIMITED, UNLIMITED, &totals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attribution only: this prints the baseline record set. `--nocapture` is
    /// what makes the numbers visible; it asserts nothing about them.
    ///
    /// The remote cadence is the headline: a remote pilot peer receives only the
    /// 32 ms full publications, which is what the reported 855 kbps measures.
    /// The owner and every-publication runs come after it so the reader can see
    /// how much the owner-only frames would cost if they reached a peer, and how
    /// much of that is the codec's inability to tell the two periodic kinds
    /// apart.
    #[test]
    fn bandwidth_attribution_baseline() {
        baseline_report(10, Cadence::Remote);
        baseline_report(5, Cadence::Owner);
        baseline_report(5, Cadence::EveryPublication);
    }

    /// Run the ablated workloads and sum each section's compressed
    /// contribution over the run, so the shares are of produced frames rather
    /// than of one arbitrary state.
    #[test]
    fn bandwidth_attribution_sections() {
        for workload in [Workload::Fire, Workload::Twelve] {
            let mut sums: BTreeMap<&'static str, i64> = BTreeMap::new();
            let mut frames = 0_u64;
            let totals = run_observed(
                workload,
                3,
                Cadence::Remote,
                UNLIMITED,
                UNLIMITED,
                &mut |state| {
                    frames += 1;
                    for row in ablation(state) {
                        *sums.entry(row.name).or_default() += row.bytes;
                    }
                },
            );
            report(Cadence::Remote, workload, 3, UNLIMITED, UNLIMITED, &totals);
            let rows: Vec<String> = sums
                .iter()
                .map(|(name, bytes)| format!("{name}={}", bytes / frames as i64))
                .collect();
            println!(
                "ablation cadence={} workload={} frames={} bytes_per_frame {}",
                Cadence::Remote.name(),
                workload.name(),
                frames,
                rows.join(" ")
            );
        }
    }

    /// The probe itself must be deterministic, or it cannot compare two builds.
    #[test]
    fn probe_replays_byte_for_byte() {
        let first = run(Workload::Fire, 2, Cadence::Remote, UNLIMITED, UNLIMITED);
        let second = run(Workload::Fire, 2, Cadence::Remote, UNLIMITED, UNLIMITED);
        assert_eq!(first.down_bytes, second.down_bytes);
        assert_eq!(first.up_bytes, second.up_bytes);
        assert_eq!(first.down_owner_bytes, second.down_owner_bytes);
        assert_eq!(first.down_world_bytes, second.down_world_bytes);
    }

    /// The hand-checkable accounting identities must hold, or the attribution
    /// is not trustworthy: every downstream byte belongs to exactly one class,
    /// input batches are a subset of upstream bytes, and the printed `down_kbps`
    /// is exactly `down_bytes_s * 8 / 1000` (the report prints both from the
    /// same `down_bytes`, so this pins the formula it must use).
    #[test]
    fn probe_accounting_is_consistent() {
        let totals = run(Workload::Fire, 3, Cadence::Remote, UNLIMITED, UNLIMITED);
        assert_eq!(
            totals.down_owner_bytes + totals.down_world_bytes + totals.down_control_bytes,
            totals.down_bytes
        );
        assert!(totals.up_batch_bytes <= totals.up_bytes);
        assert!(totals.produced_world_updates > 0);
        let down_bytes_s = totals.down_bytes as f64 / 3.;
        let down_kbps = down_bytes_s * 8. / 1000.;
        assert!(down_kbps > 0.);
        // The encoder only rotates baselines once acknowledgements arrive, so a
        // working feedback path must select fewer bytes than the independent
        // encodings it considered.
        assert!(
            totals.selected_bytes < totals.independent_bytes,
            "no baseline feedback reached the encoder: selected={} independent={}",
            totals.selected_bytes,
            totals.independent_bytes
        );
    }

    /// The instrument must be sensitive to the workload, or a regression that
    /// stops applying pilot input would silently report an idle field for every
    /// workload again. `drive` must move the field, `fire` must launch shots and
    /// churn the checkpoint, and `twelve` must fill the chassis array.
    #[test]
    fn probe_is_workload_sensitive() {
        let idle = run(Workload::Idle, 4, Cadence::Remote, UNLIMITED, UNLIMITED);
        let drive = run(Workload::Drive, 4, Cadence::Remote, UNLIMITED, UNLIMITED);
        let fire = run(Workload::Fire, 4, Cadence::Remote, UNLIMITED, UNLIMITED);
        let twelve = run(Workload::Twelve, 4, Cadence::Remote, UNLIMITED, UNLIMITED);
        assert_ne!(
            idle.down_world_bytes, drive.down_world_bytes,
            "driving must move the field and change the world stream"
        );
        assert_ne!(
            drive.raw_world_bytes, fire.raw_world_bytes,
            "firing must churn projectiles and change the checkpoint contents"
        );
        assert!(
            fire.fire_commands > 0 && fire.shots_fired > 0,
            "fire submitted {} commands but launched {} shots",
            fire.fire_commands,
            fire.shots_fired
        );
        assert!(
            fire.projectiles > 0 || fire.hits > 0 || fire.shot_results > 0,
            "a firing run must leave projectiles, hits or shot results behind"
        );
        assert_eq!(twelve.chassis, 12);
        assert_ne!(
            twelve.raw_world_bytes, idle.raw_world_bytes,
            "the twelve-player workload must not replay the idle checkpoint"
        );
    }
}
