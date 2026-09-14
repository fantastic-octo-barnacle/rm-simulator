// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Stage-3 headless presentation experiment. Independent complete pose lists can
//! advance before a checkpoint; they never become a `SimulationState` or restore
//! input. No interpolation, extrapolation or app integration is implied.
//!
//! ```
//! use rm_simulator_server::section_topics::delivery::{Sender, presentation::Cadence};
//! use rm_simulator_server::simulation::Simulation;
//! use rm_simulator_world::{Field, FieldConfig};
//! use std::time::Duration;
//! let mut state = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false).state();
//! state.snapshot_id = 1;
//! let mut sender = Sender::new(512 * 1024);
//! sender.publish_cadenced(&state, Duration::from_millis(16), Cadence {
//!     chassis_ms: 16, projectiles_ms: 64, checkpoint_ms: 128,
//! }).unwrap();
//! ```
use super::*;
use rm_simulator_world::Pose;

pub(super) const CHASSIS: usize = REPAIR_MANIFEST + 1;
pub(super) const PROJECTILES: usize = REPAIR_MANIFEST + 2;

/// Cadences in injected-clock milliseconds, independent of the physics tick.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Cadence {
    /// Period between chassis pose and aim publications.
    pub chassis_ms: u64,
    /// Period between projectile position publications.
    pub projectiles_ms: u64,
    /// Period between complete restorable checkpoints.
    pub checkpoint_ms: u64,
}
/// A drawable robot identity and pose; configuration remains checkpoint-owned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Robot {
    /// Stable field identity.
    pub id: u32,
    /// Placement/revival generation; never interpolate across generations.
    pub placement_revision: u64,
    /// Body pose in FLU metres.
    pub pose: Pose,
    /// Actual gun pose in FLU metres.
    pub turret: Pose,
    /// Whether the robot is defeated at capture time.
    pub defeated: bool,
}
/// Complete pose list for one topic at one simulation time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Poses<T> {
    /// Reset epoch, ordered before simulation time.
    pub epoch: u64,
    /// Source time in nanoseconds; may remain constant while paused.
    pub time_ns: u64,
    /// Source publication identity, increasing even while paused.
    pub capture: u64,
    /// Complete roster: absence removes an entity from this topic.
    pub values: Vec<T>,
}
/// Independently timestamped presentation state, unsuitable for physics restore.
#[derive(Default)]
pub struct View {
    epoch: Option<u64>,
    /// Latest complete robot pose list.
    pub chassis: Option<Poses<Robot>>,
    /// Latest complete projectile list: stable identity and FLU position in metres.
    pub projectiles: Option<Poses<(u64, [f64; 3])>>,
}
impl View {
    fn epoch(&mut self, epoch: u64) -> bool {
        if self.epoch.is_some_and(|old| old > epoch) {
            return false;
        }
        if self.epoch != Some(epoch) {
            self.chassis = None;
            self.projectiles = None;
            self.epoch = Some(epoch);
        }
        true
    }
    pub(super) fn receive(&mut self, class: usize, bytes: &[u8]) -> io::Result<()> {
        let raw = crate::compression::decompress(bytes, FRAME_LIMIT)
            .map_err(|_| invalid("invalid presentation frame"))?;
        if class == CHASSIS {
            let poses: Poses<Robot> = serde_json::from_slice(&raw)?;
            if poses.values.iter().any(|v| {
                !v.pose
                    .translation_m
                    .iter()
                    .chain(v.turret.translation_m.iter())
                    .all(|x| x.is_finite())
            }) {
                return Err(invalid("nonfinite robot pose"));
            }
            if self.epoch(poses.epoch) {
                accept(&mut self.chassis, poses)?;
            }
        } else {
            let poses: Poses<(u64, [f64; 3])> = serde_json::from_slice(&raw)?;
            if poses
                .values
                .iter()
                .any(|v| !v.1.iter().all(|x| x.is_finite()))
            {
                return Err(invalid("nonfinite projectile pose"));
            }
            if self.epoch(poses.epoch) {
                accept(&mut self.projectiles, poses)?;
            }
        }
        Ok(())
    }
    pub(super) fn checkpoint(&mut self, state: &SimulationState) {
        if self.epoch(state.input_epoch) {
            accept(&mut self.chassis, robots(state)).expect("validated checkpoint identity");
            accept(&mut self.projectiles, balls(state)).expect("validated checkpoint identity");
        }
    }
}
fn accept<T>(slot: &mut Option<Poses<T>>, poses: Poses<T>) -> io::Result<()> {
    if poses.capture == 0 {
        return Err(invalid("zero presentation capture"));
    }
    if slot
        .as_ref()
        .is_none_or(|old| poses.capture > old.capture && poses.time_ns >= old.time_ns)
    {
        *slot = Some(poses);
    }
    Ok(())
}
fn robots(state: &SimulationState) -> Poses<Robot> {
    Poses {
        epoch: state.input_epoch,
        time_ns: state.field.time_ns,
        capture: state.snapshot_id,
        values: state
            .field
            .chassis
            .iter()
            .map(|v| Robot {
                id: v.id,
                placement_revision: v.placement_revision,
                pose: v.pose,
                turret: v.turret,
                defeated: v.defeated,
            })
            .collect(),
    }
}
fn balls(state: &SimulationState) -> Poses<(u64, [f64; 3])> {
    Poses {
        epoch: state.input_epoch,
        time_ns: state.field.time_ns,
        capture: state.snapshot_id,
        values: state
            .field
            .projectiles
            .iter()
            .map(|v| (v.id, v.position_m))
            .collect(),
    }
}
impl Sender {
    /// Offers due checkpoints and independent pose lists using the same byte
    /// budget and bounded fragment queues. Call on each source capture, including
    /// the first capture of an epoch, at 16 ms boundaries of `now`. Periods must
    /// be positive multiples of 16 ms. The injected clock schedules publication
    /// even while simulation time is paused.
    /// Checkpoint capture always includes every section from that exact state.
    pub fn publish_cadenced(
        &mut self,
        state: &SimulationState,
        now: Duration,
        cadence: Cadence,
    ) -> io::Result<()> {
        if [
            cadence.chassis_ms,
            cadence.projectiles_ms,
            cadence.checkpoint_ms,
        ]
        .iter()
        .any(|ms| *ms == 0 || !ms.is_multiple_of(16))
        {
            return Err(invalid("invalid presentation cadence"));
        }
        if state.snapshot_id == 0
            || state.field.restore.is_none()
            || state.field.tick.checked_mul(rm_simulator_world::tick_ns())
                != Some(state.field.time_ns)
            || self.presentation_offered.is_some_and(|(epoch, id, at)| {
                (state.input_epoch, state.snapshot_id) <= (epoch, id) || now < at
            })
        {
            return Err(invalid("invalid cadenced capture identity or clock"));
        }
        self.presentation_offered = Some((state.input_epoch, state.snapshot_id, now));
        let first = self.encoder.epoch != Some(state.input_epoch);
        let ms = now.as_millis();
        if first || ms.is_multiple_of(cadence.checkpoint_ms as u128) {
            self.publish(state, now)?;
        }
        if first || ms.is_multiple_of(cadence.chassis_ms as u128) {
            self.queue.offer(
                CHASSIS,
                crate::compression::compress(&serde_json::to_vec(&robots(state))?),
            )?;
        }
        if first || ms.is_multiple_of(cadence.projectiles_ms as u128) {
            self.queue.offer(
                PROJECTILES,
                crate::compression::compress(&serde_json::to_vec(&balls(state))?),
            )?;
        }
        Ok(())
    }
}
impl Receiver {
    /// Returns pose lists for headless presentation only. Newer lists survive
    /// older checkpoint completion; epoch changes discard both old rosters.
    pub fn presentation(&self) -> &View {
        &self.presentation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::Simulation;
    use rm_simulator_world::{Field, FieldConfig};
    fn state(id: u64, epoch: u64) -> SimulationState {
        let mut s = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false).state();
        s.snapshot_id = id;
        s.input_epoch = epoch;
        s
    }
    #[test]
    fn early_pose_cannot_promote_checkpoint_and_old_checkpoint_cannot_roll_it_back() {
        let mut receiver = Receiver::new(10240);
        let newer = state(3, 0);
        let bytes = crate::compression::compress(&serde_json::to_vec(&robots(&newer)).unwrap());
        let mut queue = Queue::new(512 * 1024);
        queue.offer(CHASSIS, bytes).unwrap();
        while let Some(p) = queue.next(Duration::from_millis(32)).unwrap() {
            assert!(
                receiver
                    .receive(&p.bytes, Duration::from_millis(32))
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(receiver.decoder.completed, 0);
        receiver.presentation.checkpoint(&state(2, 0));
        assert_eq!(receiver.presentation.chassis.as_ref().unwrap().capture, 3);
        receiver.presentation.checkpoint(&state(1, 1));
        assert_eq!(receiver.presentation.chassis.as_ref().unwrap().epoch, 1);
        receiver.presentation.checkpoint(&newer);
        assert_eq!(receiver.presentation.chassis.as_ref().unwrap().epoch, 1);
    }
    #[test]
    fn complete_rosters_remove_entities_and_paused_captures_advance() {
        let mut view = View::default();
        let mut poses = balls(&state(1, 0));
        poses.values.push((7, [1., 2., 3.]));
        accept(&mut view.projectiles, poses).unwrap();
        view.epoch = Some(0);
        view.checkpoint(&state(2, 0));
        assert!(view.projectiles.as_ref().unwrap().values.is_empty());
        assert_eq!(view.projectiles.as_ref().unwrap().capture, 2);
    }
    #[test]
    fn permanently_lost_checkpoint_and_projectiles_still_allow_fresh_chassis() {
        let mut sender = Sender::new(40 * 1024);
        let mut receiver = Receiver::new(10 * 1024);
        let cadence = Cadence {
            chassis_ms: 16,
            projectiles_ms: 32,
            checkpoint_ms: 128,
        };
        for ms in (16..=2048).step_by(16) {
            let mut s = state(ms / 16, 0);
            s.field.tick = ms;
            s.field.time_ns = ms * 1_000_000;
            let now = Duration::from_millis(ms);
            sender.publish_cadenced(&s, now, cadence).unwrap();
            while let Some(packet) = sender.next(now).unwrap() {
                let mut kept = packet.bytes[..4].to_vec();
                let mut offset = 4;
                while offset < packet.bytes.len() {
                    let length = u16::from_le_bytes(
                        packet.bytes[offset + 17..offset + 19].try_into().unwrap(),
                    ) as usize;
                    let end = offset + HEADER + length;
                    if packet.bytes[offset] as usize == CHASSIS {
                        kept.extend_from_slice(&packet.bytes[offset..end]);
                    }
                    offset = end;
                }
                if kept.len() > 4 {
                    assert!(receiver.receive(&kept, now).unwrap().is_empty());
                }
            }
            assert!(sender.queued_bytes() <= QUEUE_LIMIT);
            assert!(receiver.partial_bytes() <= FRAME_LIMIT);
        }
        assert_eq!(receiver.decoder.completed, 0);
        assert!(receiver.presentation.chassis.as_ref().unwrap().capture >= 120);
        assert!(receiver.presentation.projectiles.is_none());
    }
    #[test]
    fn pause_away_from_checkpoint_boundary_keeps_publishing() {
        let mut sender = Sender::new(512 * 1024);
        let mut receiver = Receiver::new(10 * 1024);
        let cadence = Cadence {
            chassis_ms: 16,
            projectiles_ms: 64,
            checkpoint_ms: 128,
        };
        for ms in (16..=256).step_by(16) {
            let mut s = state(ms / 16, 0);
            s.paused = true;
            s.field.tick = 16;
            s.field.time_ns = 16_000_000;
            let now = Duration::from_millis(ms);
            sender.publish_cadenced(&s, now, cadence).unwrap();
            while let Some(packet) = sender.next(now).unwrap() {
                receiver.receive(&packet.bytes, now).unwrap();
            }
        }
        assert!(receiver.decoder.completed >= 8);
        assert_eq!(
            receiver.presentation.chassis.as_ref().unwrap().time_ns,
            16_000_000
        );
        assert_eq!(receiver.presentation.chassis.as_ref().unwrap().capture, 16);
    }
    #[test]
    fn rejects_invalid_cadence_and_capture_regression() {
        let mut sender = Sender::new(10240);
        let cadence = Cadence {
            chassis_ms: 16,
            projectiles_ms: 32,
            checkpoint_ms: 128,
        };
        sender
            .publish_cadenced(&state(2, 0), Duration::ZERO, cadence)
            .unwrap();
        assert!(
            sender
                .publish_cadenced(&state(1, 0), Duration::ZERO, cadence)
                .is_err()
        );
        assert!(
            sender
                .publish_cadenced(
                    &state(3, 0),
                    Duration::ZERO,
                    Cadence {
                        chassis_ms: 0,
                        ..cadence
                    }
                )
                .is_err()
        );
    }
}
