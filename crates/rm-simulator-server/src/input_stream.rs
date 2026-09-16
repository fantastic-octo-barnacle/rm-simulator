// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Sequenced held input with bounded evidence retention and a simulation-time lease.
use rm_simulator_world::ChassisCommand;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Stop lease in ns. Drive is neutralised once this long passes with no newly
/// applied input, so a lost release cannot leave a chassis driving.
pub const INPUT_LEASE_NS: u64 = 250_000_000;
/// Cap on retained replay evidence frames and on the pending schedule. Evidence
/// this many sequences behind the executed one is dropped as unreplayable.
pub const MAX_INPUT_HISTORY: usize = 256;
/// One sampled control state. Duration describes the client's sampled interval;
/// it never grants permission to advance the server clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputFrame {
    /// Host control epoch, changed across pause/resume.
    pub input_epoch: u64,
    /// Client sequence, monotonic within a life. A frame at or below the
    /// executed sequence never applies again.
    pub sequence: u64,
    /// Server-clock time in ns the client sampled this state for.
    pub sampled_time_ns: u64,
    /// Ticks of `tick_ns` the sample covers; a valid frame carries 1 to 32.
    /// It describes the client sample interval, so it scales with the rate.
    pub duration_ticks: u32,
    /// Chassis placement revision this input belongs to. Input naming another
    /// life is refused rather than applied to the current one.
    pub placement_revision: u64,
    /// Body velocity in m/s and gun aim in rad at that sample.
    pub command: ChassisCommand,
}
/// One peer's sequenced held input: a bounded replay history, a schedule of
/// transitions, and the single control state the host currently applies.
#[derive(Default)]
pub struct InputStream {
    /// Latest counters, sent to the owner about once per second.
    pub telemetry: crate::network_stats::InputTelemetry,
    /// Retained immutable evidence frames in sequence order, oldest first.
    history: VecDeque<InputFrame>,
    /// Arrival margins in ns of the last 64 newly received sequences, for the p05.
    arrival_margins: VecDeque<i64>,
    /// Placement revision this stream belongs to; `None` until the first frame.
    revision: Option<u64>,
    /// Highest executed sequence, or zero before any input applies.
    latest: u64,
    /// Scheduled frames not yet eligible, ordered by sequence.
    pending: VecDeque<InputFrame>,
    /// Server time in ns of the last applied input; the lease counts from here.
    refreshed_ns: u64,
    /// Command currently applied, cleared when the lease expires or is cancelled.
    held: Option<ChassisCommand>,
}
impl InputStream {
    /// Schedule a complete transition, applying it now only when its tick is current.
    /// Fresh late transitions apply at receipt. Repeated copies cannot renew the lease.
    ///
    /// `now_ns` is host simulation time in ns and `placement_revision` the life
    /// the sender claims. A sample more than 200 ms ahead of `now_ns`, one more
    /// than 1 s behind it, or one naming another life is an error. A sequence at
    /// or below the executed one never applies. Errors when the schedule is
    /// full. Returns a command to apply when this call changed the held control,
    /// including the neutral an expired lease produces.
    pub fn receive(
        &mut self,
        frame: InputFrame,
        now_ns: u64,
        placement_revision: u64,
    ) -> Result<Option<ChassisCommand>, &'static str> {
        if frame.sampled_time_ns > now_ns.saturating_add(200_000_000)
            || now_ns.saturating_sub(frame.sampled_time_ns) > 1_000_000_000
        {
            return Err("input timestamp outside admission window");
        }
        if frame.placement_revision != placement_revision {
            return Err("input belongs to another life");
        }
        if self.revision != Some(placement_revision) {
            *self = Self::default();
            self.revision = Some(placement_revision);
        }
        self.record_evidence(frame, now_ns, placement_revision)?;
        if frame.sequence <= self.latest
            || now_ns.saturating_sub(frame.sampled_time_ns) > INPUT_LEASE_NS
        {
            return Ok(None);
        }
        if !self.pending.iter().any(|p| p.sequence == frame.sequence) {
            if frame.sequence > self.telemetry.received_sequence {
                self.telemetry.received_sequence = frame.sequence;
                self.telemetry.placement_revision = placement_revision;
                self.telemetry.arrival_margin_ns =
                    (i128::from(frame.sampled_time_ns) - i128::from(now_ns))
                        .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
                self.telemetry.late_inputs += u64::from(frame.sampled_time_ns < now_ns);
                self.telemetry.arrival_samples += 1;
                self.arrival_margins
                    .push_back(self.telemetry.arrival_margin_ns);
                if self.arrival_margins.len() > 64 {
                    self.arrival_margins.pop_front();
                }
                let mut margins: Vec<_> = self.arrival_margins.iter().copied().collect();
                margins.sort_unstable();
                self.telemetry.arrival_p05_ns = margins[(margins.len() - 1) / 20];
            }
            if self.pending.len() >= MAX_INPUT_HISTORY {
                return Err("input schedule full");
            }
            let index = self
                .pending
                .iter()
                .position(|p| p.sequence > frame.sequence)
                .unwrap_or(self.pending.len());
            self.pending.insert(index, frame);
        }
        Ok(self.advance(now_ns))
    }
    /// Consume eligible transitions once. The host never revisits an earlier tick.
    ///
    /// `now_ns` is host simulation time in ns. Frames run in sequence order and
    /// only when their sequence is newer than the last executed one, so a late
    /// duplicate returns nothing. Returns the newest command that changed, or
    /// the neutral from an expired lease.
    pub fn advance(&mut self, now_ns: u64) -> Option<ChassisCommand> {
        let mut changed = None;
        while self
            .pending
            .front()
            .is_some_and(|p| p.sampled_time_ns <= now_ns)
        {
            let frame = self.pending.pop_front().expect("eligible input");
            if frame.sequence > self.latest {
                self.latest = frame.sequence;
                self.telemetry.executed_sequence = frame.sequence;
                self.telemetry.intended_time_ns = frame.sampled_time_ns;
                self.telemetry.executed_time_ns = now_ns;
                self.refreshed_ns = now_ns;
                self.held = Some(frame.command);
                changed = self.held;
            }
        }
        changed.or_else(|| self.expire(now_ns))
    }
    /// Retain immutable replay evidence without applying or refreshing live controls.
    ///
    /// A repeated sequence must carry identical contents, so an attacker or a
    /// reordered duplicate cannot change what a replay of that sequence did.
    /// Rejects a zero sequence, a duration outside 1 to 32 ticks, a non-finite
    /// command and a life mismatch. `now_ns` is host simulation time in ns and
    /// bounds the same admission window as [`InputStream::receive`]. Evidence
    /// more than [`MAX_INPUT_HISTORY`] sequences behind the executed one is
    /// dropped silently, and evidence times must not move backwards. Never
    /// advances the held command or renews the lease.
    pub fn record_evidence(
        &mut self,
        frame: InputFrame,
        now_ns: u64,
        placement_revision: u64,
    ) -> Result<(), &'static str> {
        if frame.sequence == 0
            || !(1..=32).contains(&frame.duration_ticks)
            || !frame.command.is_finite()
            || frame.placement_revision != placement_revision
        {
            return Err("invalid input frame");
        }
        if let Some(previous) = self
            .history
            .iter()
            .find(|old| old.sequence == frame.sequence)
        {
            return if *previous == frame {
                Ok(())
            } else {
                Err("input sequence changed contents")
            };
        }
        if frame.sampled_time_ns > now_ns.saturating_add(200_000_000)
            || now_ns.saturating_sub(frame.sampled_time_ns) > 1_000_000_000
        {
            return Err("input timestamp outside admission window");
        }
        if frame.sequence.saturating_add(MAX_INPUT_HISTORY as u64) < self.latest {
            return Ok(());
        }
        let position = self
            .history
            .iter()
            .position(|old| old.sequence > frame.sequence)
            .unwrap_or(self.history.len());
        if let Some(previous) = position.checked_sub(1).and_then(|i| self.history.get(i))
            && previous.sampled_time_ns > frame.sampled_time_ns
        {
            return Err("input time moved backwards");
        }
        if self
            .history
            .get(position)
            .is_some_and(|next| next.sampled_time_ns < frame.sampled_time_ns)
        {
            return Err("input time moved backwards");
        }
        self.history.insert(position, frame);
        while self.history.len() > MAX_INPUT_HISTORY {
            self.history.pop_front();
        }
        Ok(())
    }
    /// Neutralize motion once after the lease expires. Preserve the last aim.
    ///
    /// `now_ns` is host simulation time in ns. Calls inside the lease return
    /// nothing, and a second call after expiry changes nothing, so one lost
    /// release yields one neutral command.
    pub fn expire(&mut self, now_ns: u64) -> Option<ChassisCommand> {
        if now_ns.saturating_sub(self.refreshed_ns) < INPUT_LEASE_NS {
            return None;
        }
        self.telemetry.lease_expiries += u64::from(self.held.is_some());
        self.neutralize()
    }
    /// Drop the pending schedule and neutralize motion while preserving the last
    /// aim. A host calls this when a life ends, the world pauses or the chassis
    /// leaves, so no command survives the change.
    pub fn cancel(&mut self) -> Option<ChassisCommand> {
        self.pending.clear();
        self.neutralize()
    }
    /// Give up the held command while keeping its aim; the drive terms return to
    /// zero. Returns `None` when nothing was held.
    fn neutralize(&mut self) -> Option<ChassisCommand> {
        self.held.take().map(|held| ChassisCommand {
            aim_yaw_rad: held.aim_yaw_rad,
            aim_pitch_rad: held.aim_pitch_rad,
            ..Default::default()
        })
    }
    /// Whether this stream belongs to `revision`. A false answer makes the
    /// stream disposable, and its cancellation is what stops a stale life from
    /// continuing to drive.
    pub fn belongs_to(&self, revision: Option<u64>) -> bool {
        self.revision == revision
    }
    /// Retained replay evidence frames in sequence order, oldest first.
    pub fn history(&self) -> &VecDeque<InputFrame> {
        &self.history
    }
    /// Highest executed sequence, or zero before any input applies.
    pub fn latest(&self) -> u64 {
        self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(sequence: u64, forward_m_s: f64) -> InputFrame {
        InputFrame {
            input_epoch: 0,
            sequence,
            sampled_time_ns: sequence * 16_000_000,
            duration_ticks: 16,
            placement_revision: 0,
            command: ChassisCommand {
                forward_m_s,
                ..Default::default()
            },
        }
    }
    #[test]
    fn telemetry_distinguishes_arrival_execution_and_duplicate_lease() {
        let mut stream = InputStream::default();
        stream.receive(frame(1, 2.), 8_000_000, 0).unwrap();
        assert_eq!(stream.telemetry.received_sequence, 1);
        assert_eq!(stream.telemetry.executed_sequence, 0);
        stream.advance(16_000_000);
        assert_eq!(stream.telemetry.executed_time_ns, 16_000_000);
        stream.receive(frame(1, 2.), 20_000_000, 0).unwrap();
        assert_eq!(stream.telemetry.late_inputs, 0);
        stream.expire(266_000_000);
        stream.expire(300_000_000);
        assert_eq!(stream.telemetry.lease_expiries, 1);
        let message = crate::protocol::ServerMessage::Telemetry(
            crate::network_stats::HostTelemetry(1, 300_000_000, 7, stream.telemetry),
        );
        let json = crate::snapshot_codec::encode_player_message(&message);
        assert!(crate::compression::compress(&json).len() + 21 < 256);
    }
    #[test]
    fn schedules_once_and_applies_fresh_late_transitions() {
        let mut stream = InputStream::default();
        assert!(stream.receive(frame(2, 2.), 0, 0).unwrap().is_none());
        assert!(stream.advance(31_000_000).is_none());
        assert_eq!(stream.advance(32_000_000).unwrap().forward_m_s, 2.);
        assert!(
            stream
                .receive(frame(1, 0.), 40_000_000, 0)
                .unwrap()
                .is_none()
        );
        assert!(stream.advance(41_000_000).is_none());
        assert_eq!(stream.expire(282_000_000).unwrap().forward_m_s, 0.);
        assert_eq!(
            stream
                .receive(frame(3, 2.), 60_000_000, 0)
                .unwrap()
                .unwrap()
                .forward_m_s,
            2.
        );
        assert!(stream.advance(61_000_000).is_none());
    }

    #[test]
    fn loss_redundancy_and_reordering_never_restore_an_old_drive() {
        let mut stream = InputStream::default();
        assert_eq!(
            stream
                .receive(frame(1, 2.), 16_000_000, 0)
                .unwrap()
                .unwrap()
                .forward_m_s,
            2.
        );
        assert_eq!(
            stream
                .receive(frame(3, 0.), 48_000_000, 0)
                .unwrap()
                .unwrap()
                .forward_m_s,
            0.
        );
        assert!(
            stream
                .receive(frame(2, 2.), 50_000_000, 0)
                .unwrap()
                .is_none()
        );
        assert!(
            stream
                .receive(frame(3, 0.), 60_000_000, 0)
                .unwrap()
                .is_none()
        );
        assert_eq!(stream.history().len(), 3);
        assert!(stream.receive(frame(3, 3.), 60_000_000, 0).is_err());
    }
    #[test]
    fn replayed_packets_cannot_keep_drive_alive_and_resets_reject_old_inputs() {
        let mut stream = InputStream::default();
        stream.receive(frame(1, 2.), 16_000_000, 0).unwrap();
        stream.receive(frame(1, 2.), 250_000_000, 0).unwrap();
        assert_eq!(stream.expire(266_000_000).unwrap().forward_m_s, 0.);
        assert!(stream.expire(300_000_000).is_none());
        assert!(stream.receive(frame(2, 2.), 32_000_000, 1).is_err());
    }
    #[test]
    fn evidence_never_drives_or_renews_a_lease_but_fresh_input_still_can() {
        let mut stream = InputStream::default();
        let drive = frame(1, 2.);
        stream.record_evidence(drive, 16_000_000, 0).unwrap();
        assert_eq!(stream.latest(), 0);
        assert!(stream.expire(1_000_000_000).is_none());
        assert!(stream.receive(drive, 16_000_000, 0).unwrap().is_some());
        stream
            .record_evidence(frame(2, 3.), 200_000_000, 0)
            .unwrap();
        assert_eq!(stream.latest(), 1);
        assert_eq!(stream.expire(266_000_000).unwrap().forward_m_s, 0.);
        assert!(stream.record_evidence(drive, 2_000_000_000, 0).is_ok());
        assert!(stream.receive(drive, 2_000_000_000, 0).is_err());
    }
}
