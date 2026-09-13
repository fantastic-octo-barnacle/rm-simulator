// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Local diagnostics. Missing native measurements are not zero or inferred loss.
use serde::{Deserialize, Serialize};

/// One transport sample: native values where the transport reports them, local
/// counters where only the codec can count.
///
/// An `Option` that is `None` means the transport offered no measurement. A
/// missing native measurement is not zero and never implies inferred loss.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TransportStats {
    /// Milliseconds from the transport epoch when this sample was taken; zero
    /// when the sample carries only local codec counters.
    pub sampled_elapsed_ms: f64,
    /// Transport round-trip estimate in ms; `None` when the provider has none.
    pub transport_rtt_ms: Option<f64>,
    /// Application bytes per second the provider sent over its own window.
    pub send_bytes_per_s: Option<f64>,
    /// Application bytes per second the provider received over its own window.
    pub receive_bytes_per_s: Option<f64>,
    /// Provider estimate of how long a packet queued now waits, in ms.
    pub send_queue_ms: Option<f64>,
    /// Bytes waiting on the reliable lane, in bytes.
    pub pending_reliable_bytes: Option<u32>,
    /// Bytes waiting on the unreliable lane, in bytes.
    pub pending_unreliable_bytes: Option<u32>,
    /// Datagrams abandoned before every fragment arrived, dropped by frame
    /// lifetime or by reassembly capacity.
    pub incomplete_frames: u64,
    /// Complete snapshots discarded because their id was not newer than the
    /// last accepted one, so an old frame cannot overwrite newer state.
    pub stale_updates: u64,
    /// Snapshots reassembled from all their fragments and accepted.
    pub complete_snapshots: u64,
    /// Bytes held in the outbound pacer queue.
    pub application_queued_bytes: usize,
    /// Unsent replaceable updates discarded because a newer copy arrived first,
    /// counted across owner anchors, world transfers and duplicate control.
    pub replaced_unsent: u64,
    /// Unsent unreliable packets discarded as stale, plus world transfers the
    /// queue bounds refused.
    pub expired_unsent: u64,
    /// Deltas applied against a pinned acknowledged baseline.
    pub decoded_deltas: u64,
    /// Deltas refused because the baseline they name was not pinned, which
    /// costs recovery rather than correctness.
    pub missing_baselines: u64,
}

/// Cumulative encoding costs before pacing; these are produced bytes, not wire traffic.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EncodingStats {
    /// Periodic world snapshots encoded, excluding those skipped for native backlog.
    pub world_updates: u64,
    /// World snapshots skipped because the native transport was congested.
    pub skipped_world_updates: u64,
    /// JSON checkpoint bytes before delta encoding/compression, when deltas are enabled.
    pub raw_world_bytes: u64,
    /// Encoded world bytes including application fragment headers, before pacing drops.
    pub framed_world_bytes: u64,
    /// Owner datagrams produced, before replacement or pacing drops.
    pub owner_updates: u64,
    /// Owner datagram bytes produced, before replacement or pacing drops.
    pub owner_bytes: u64,
    /// Compressed independent bytes considered by the baseline codec for comparison.
    pub independent_bytes: u64,
    /// Compressed full/delta bytes selected by the baseline codec, before fragment headers.
    pub selected_bytes: u64,
}

/// One connection's diagnostics report, combining the client's own timing with
/// whatever native transport sample is available.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStats {
    /// Wire schema version of this report; a reader must not assume two
    /// versions describe the same fields.
    pub schema_version: u32,
    /// Client id from this session's welcome, monotonically assigned by the
    /// host, so a reconnecting peer reads as a new generation.
    pub connection_generation: u64,
    /// Transport that produced the sample: `gns`, `tcp`, or in-process `local`.
    pub transport: String,
    /// Elapsed milliseconds on the reporting side when this report was built.
    pub sampled_elapsed_ms: f64,
    /// The connection is gone, so later samples describe no live peer.
    pub stale: bool,
    /// Age in ms of the last native sample; `None` when none ever arrived.
    pub native_sample_age_ms: Option<f64>,
    /// Last native sample, absent when the transport reports none.
    pub native: Option<TransportStats>,
    /// What the native rates measure, in the provider's own accounting terms.
    pub native_rate_source: &'static str,
    /// Packet loss percentage; `None` when no loss measurement exists for this
    /// transport, never because loss was seen to be zero.
    pub loss_percent: Option<f64>,
    /// Why `loss_percent` is absent. A wrapper quality score is not packet loss.
    pub loss_unavailable_reason: &'static str,
    /// Application round-trip estimate in ms from the client's own probes.
    pub app_rtt_ms: Option<f64>,
    /// Age in ms of an unanswered probe, a lower bound on response delay while
    /// no answer has arrived.
    pub pending_response_age_ms: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unavailable_native_values_serialize_as_null() {
        let value = serde_json::to_value(TransportStats::default()).unwrap();
        assert!(value["transport_rtt_ms"].is_null());
        assert!(value["send_bytes_per_s"].is_null());
    }
}

/// One peer's input execution observations, reset with its control epoch/life.
/// Tuple encoding keeps the once-per-second feedback payload small.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct InputTelemetry {
    /// Placement revision recorded with the newest received sequence.
    pub placement_revision: u64,
    /// Newly received sequences counted so far; duplicates do not raise it.
    pub arrival_samples: u64,
    /// Fifth percentile of the last 64 newly received sequence margins.
    pub arrival_p05_ns: i64,
    /// Highest sequence received, in sequence numbers.
    pub received_sequence: u64,
    /// Highest sequence the stream applied, in sequence numbers.
    pub executed_sequence: u64,
    /// Client sample time of the newest received sequence, in ns.
    pub intended_time_ns: u64,
    /// Server time the newest executed sequence ran at, in ns. Its excess over
    /// `intended_time_ns` is the execution lateness.
    pub executed_time_ns: u64,
    /// Signed margin in ns of the newest received sequence: sample time minus
    /// arrival time, so a negative value means the input arrived late.
    pub arrival_margin_ns: i64,
    /// Newly received sequences whose sample time already lay before receipt.
    pub late_inputs: u64,
    /// Times the stop lease expired with drive still held, each neutralising it.
    pub lease_expiries: u64,
}
/// One peer's input execution feedback, encoded as a tuple to keep the
/// once-per-second payload small: host control epoch, host time in ns, chassis
/// id, then the observations.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostTelemetry(
    /// Host control epoch that gives the observations their context.
    pub u64,
    /// Host simulation time when the feedback was built, in ns.
    pub u64,
    /// Chassis id the observations belong to; a host sends one peer its own.
    pub u32,
    /// Input execution observations for that chassis.
    pub InputTelemetry,
);

/// Differences between the predicted owner and the authoritative owner at one
/// exact tick, with the context clues that held at that tick.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CorrectionStats {
    /// Age in ns of the collision context the compared checkpoint was built
    /// from, when the prediction worker reported one.
    pub context_age_ns: Option<u64>,
    /// Qualitative context clue at that tick: stale collision context, a
    /// possible nearby robot contact, a wheel contact, or unknown. A clue, not
    /// a proven cause of the measured error.
    pub context_hint: Option<&'static str>,
    /// Comparisons that found a prediction record on the exact tick.
    pub comparisons: u64,
    /// Comparisons with no prediction record on that tick. No record is not a
    /// zero error.
    pub unavailable: u64,
    /// Host simulation time of the most recent comparison, in ns.
    pub tick_ns: Option<u64>,
    /// Generation counter of the prediction record the comparison used.
    pub prediction_generation: u64,
    /// Predicted-to-authoritative body translation error in metres.
    pub position_m: Option<f64>,
    /// Predicted-to-authoritative body rotation error in radians, measured on
    /// the shortest arc.
    pub orientation_rad: Option<f64>,
    /// Predicted-to-authoritative body linear velocity error in m/s.
    pub velocity_m_s: Option<f64>,
    /// Largest per-axis predicted-to-authoritative held aim error, in radians,
    /// taken with wrap-around so a heading across zero does not read as large.
    pub aim_rad: Option<f64>,
}

/// One exact-tick prediction record: tick time in ns, the generation counter
/// stamped when it was written, the owner pose, its body velocity in m/s and
/// its held aim in rad.
type RobotRecord = (u64, u64, rm_simulator_world::Pose, [f64; 3], [f64; 2]);

/// Lightweight exact-tick records, bounded to two seconds. No meshes or world clones.
#[derive(Default)]
pub struct CorrectionHistory {
    /// Placement epoch the records belong to. A different epoch clears them,
    /// so a new life is never compared against an old one.
    epoch: Option<u64>,
    /// Write counter stamped into each record; wrapping, unique within an epoch.
    generation: u64,
    /// Predicted owner records, oldest first, capped at 2001 samples.
    records: std::collections::VecDeque<RobotRecord>,
    /// Latest measurement, read for diagnostics after each comparison.
    pub stats: CorrectionStats,
}
impl CorrectionHistory {
    /// Compare `state` against the predicted record for exactly `time_ns`.
    ///
    /// `time_ns` is host simulation time in ns. A record on that tick fills the
    /// measurement fields; no record counts as unavailable instead, because a
    /// missing measurement is not zero error. A new `epoch` clears the history,
    /// and records at or after `time_ns` are dropped so one tick is compared
    /// once. Everything else in `stats` is reset per call.
    pub fn compare(
        &mut self,
        epoch: u64,
        time_ns: u64,
        state: &rm_simulator_world::ChassisSnapshot,
    ) {
        if self.epoch != Some(epoch) {
            self.records.clear();
            self.epoch = Some(epoch);
        }
        self.stats.position_m = None;
        self.stats.orientation_rad = None;
        self.stats.velocity_m_s = None;
        self.stats.aim_rad = None;
        self.stats.tick_ns = Some(time_ns);
        if let Some((_, generation, pose, velocity, aim)) =
            self.records.iter().find(|(t, ..)| *t == time_ns)
        {
            fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
                a.into_iter()
                    .zip(b)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt()
            }
            self.stats.comparisons += 1;
            self.stats.prediction_generation = *generation;
            self.stats.position_m = Some(distance(pose.translation_m, state.pose.translation_m));
            let dot = pose
                .rotation_wxyz
                .into_iter()
                .zip(state.pose.rotation_wxyz)
                .map(|(a, b)| a * b)
                .sum::<f64>();
            self.stats.orientation_rad = Some(2. * dot.abs().clamp(0., 1.).acos());
            self.stats.velocity_m_s = Some(distance(*velocity, state.velocity_m_s));
            self.stats.aim_rad = Some(
                aim.iter()
                    .zip(state.held_aim_rad)
                    .map(|(a, b)| {
                        ((a - b + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU)
                            - std::f64::consts::PI)
                            .abs()
                    })
                    .fold(0., f64::max),
            );
        } else {
            self.stats.unavailable += 1;
        }
        self.generation = self.generation.wrapping_add(1);
        self.records.retain(|(t, ..)| *t < time_ns);
    }
    /// Store the owner's predicted pose at `time_ns` for a later comparison.
    ///
    /// `time_ns` is host simulation time in ns. The oldest record is dropped
    /// past the 2001-sample cap, so the history stays a little over two seconds
    /// long and never grows with the uptime.
    pub fn record(&mut self, time_ns: u64, state: &rm_simulator_world::ChassisSnapshot) {
        while self.records.len() >= 2001 {
            self.records.pop_front();
        }
        self.records.push_back((
            time_ns,
            self.generation,
            state.pose,
            state.velocity_m_s,
            state.held_aim_rad,
        ));
    }
}

#[cfg(test)]
mod correction_tests {
    use super::*;
    #[test]
    fn compare_exact_tick_before_replay_and_clear_across_epochs() {
        let mut field =
            rm_simulator_world::Field::new(&rm_simulator_world::FieldConfig::default()).unwrap();
        field
            .add_chassis(&rm_simulator_world::ChassisPlacement {
                team: rm_simulator_world::Team::Red,
                spawn: rm_simulator_world::Pose::default(),
                config: rm_simulator_world::ChassisConfig::default(),
            })
            .unwrap();
        let mut state = field.snapshot().chassis.remove(0);
        let mut history = CorrectionHistory::default();
        history.compare(1, 0, &state);
        history.record(1_000_000, &state);
        state.pose.translation_m[0] += 0.25;
        history.compare(1, 1_000_000, &state);
        assert_eq!(history.stats.position_m, Some(0.25));
        assert_eq!(history.stats.comparisons, 1);
        history.compare(1, 2_000_000, &state);
        assert_eq!(history.stats.position_m, None);
        history.record(3_000_000, &state);
        history.compare(2, 3_000_000, &state);
        assert_eq!(history.stats.position_m, None);
        for tick in 0..10_000 {
            history.record(tick, &state);
        }
        assert_eq!(history.records.len(), 2001);
    }
}
