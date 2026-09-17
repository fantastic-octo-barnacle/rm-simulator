// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Remote pose history only. Commands, HP and local chassis remain authoritative.
use crate::math::{quat_normalize, quat_slerp};
use rm_simulator_world::{ChassisSnapshot, FieldSnapshot, Pose};
use std::collections::VecDeque;

/// App presentation setting, not a rulebook delay.
pub const DELAY_NS: u64 = 64_000_000;
/// Frames retained. Playback clamps to the oldest and newest frame, so a view
/// time outside the window holds an endpoint instead of extrapolating far.
const CAPACITY: usize = 32;
/// A bounded window of remote poses for interpolation. It carries pose history
/// only: commands, HP and the local chassis stay authoritative elsewhere.
#[derive(Default)]
pub struct RemoteHistory {
    /// One frame per host snapshot: time in ns, the chassis states then, and the
    /// caller's frame id.
    frames: VecDeque<(u64, Vec<ChassisSnapshot>, u64)>,
    /// Pause state of the newest frame. A change clears the window, because
    /// poses from either side of a pause are not one continuous motion.
    paused: bool,
}
impl RemoteHistory {
    /// Oldest and newest retained frame times in ns, or `None` while empty.
    pub fn bounds_ns(&self) -> Option<(u64, u64)> {
        Some((self.frames.front()?.0, self.frames.back()?.0))
    }
    /// Push one frame and identify it by its own time in ns plus one.
    pub fn push(&mut self, snapshot: &FieldSnapshot, paused: bool) {
        self.push_identified(snapshot, paused, snapshot.time_ns.saturating_add(1));
    }
    /// Push one frame tagged with `id`, dropping history that no longer fits.
    ///
    /// A pause change or a time that moves backwards clears the window. A
    /// chassis that despawned, changed placement revision, changed defeat state,
    /// changed config or changed team is removed from every retained frame, so a
    /// later join or revival is never interpolated from an earlier life. A frame
    /// at the same time replaces the previous one, and the oldest frame leaves at
    /// `CAPACITY`.
    pub fn push_identified(&mut self, snapshot: &FieldSnapshot, paused: bool, id: u64) {
        if paused != self.paused
            || self
                .frames
                .back()
                .is_some_and(|(time, _, _)| snapshot.time_ns < *time)
        {
            self.frames.clear();
        }
        self.paused = paused;
        // Remove despawned entities and all history preceding a placement or defeat.
        for (_, chassis, _) in &mut self.frames {
            chassis.retain(|old| {
                snapshot.chassis.iter().any(|new| {
                    new.id == old.id
                        && new.placement_revision == old.placement_revision
                        && new.defeated == old.defeated
                        && new.config == old.config
                        && new.team == old.team
                })
            });
        }
        if self
            .frames
            .back()
            .is_some_and(|(time, _, _)| *time == snapshot.time_ns)
        {
            self.frames.pop_back();
        }
        if self.frames.len() == CAPACITY {
            self.frames.pop_front();
        }
        self.frames
            .push_back((snapshot.time_ns, snapshot.chassis.clone(), id));
    }
    /// The frame pair bracketing `time_ns`, with the clamped sample time.
    fn bracket(&self, time_ns: u64) -> Option<(u64, u64, u64)> {
        let first = self.frames.front()?;
        let last = self.frames.back()?;
        let sample = time_ns.clamp(first.0, last.0);
        let after = self.frames.iter().find(|frame| frame.0 >= sample)?;
        let before = self.frames.iter().rev().find(|frame| frame.0 <= sample)?;
        Some((before.2, after.2, sample))
    }
    /// Sample known poses for chassis `id` at `time_ns`, clamping to the retained
    /// window instead of reading past its ends.
    ///
    /// `time_ns` is host simulation time in ns. A live, non-defeated chassis is
    /// then extrapolated up to 100 ms past the newest frame. Returns `None` when
    /// no retained frame carries that chassis.
    pub fn sample(&self, id: u32, time_ns: u64) -> Option<ChassisSnapshot> {
        let (before, after, sample) = self.bracket(time_ns)?;
        let before = self.frames.iter().find(|f| f.2 == before)?;
        let after = self.frames.iter().find(|f| f.2 == after)?;
        let mut state = sample_chassis(
            (before.0, &before.1),
            (after.0, &after.1),
            &self.frames.back()?.1,
            id,
            sample,
        )?;
        if !self.paused && !state.defeated {
            extrapolate(&mut state, time_ns.saturating_sub(after.0).min(100_000_000));
        }
        Some(state)
    }
}
/// Bounded visual/collision estimate during missing remote updates.
///
/// `delta_ns` is the extrapolation span in ns, capped at 100 ms whatever the
/// caller passes. Only translation advances, at the state's own linear velocity
/// in m/s; orientation and aim hold, so a guess never invents a turn. The body,
/// turret and wheel hubs move together.
pub fn extrapolate(state: &mut ChassisSnapshot, delta_ns: u64) {
    let seconds = delta_ns.min(100_000_000) as f64 * 1e-9;
    let offset = state.velocity_m_s.map(|v| v * seconds);
    for (i, delta) in offset.into_iter().enumerate() {
        state.pose.translation_m[i] += delta;
        state.turret.translation_m[i] += delta;
        for wheel in &mut state.wheels {
            wheel.hub_m[i] += delta;
        }
    }
}
/// A single global frame pair, with the latest lifecycle as the fallback.
/// Sharing this function keeps display and validation identical at joins/revivals.
///
/// `before` and `after` carry a frame time in ns and that frame's chassis states,
/// `latest` is the newest frame's states, and `time_ns` is the sample time in ns.
/// Only states matching `id` and the latest placement revision, defeat state,
/// config and team are blended, so two lives are never mixed. A state missing
/// from the later frame falls back to the latest pose, and one missing from the
/// earlier frame falls back to that later pose, so a fresh placement is never
/// blended with a stale body. A zero-length interval takes the later pose.
pub fn sample_chassis(
    before: (u64, &[ChassisSnapshot]),
    after: (u64, &[ChassisSnapshot]),
    latest: &[ChassisSnapshot],
    id: u32,
    time_ns: u64,
) -> Option<ChassisSnapshot> {
    let root = latest.iter().find(|c| c.id == id)?;
    let same = |c: &&ChassisSnapshot| {
        c.id == root.id
            && c.placement_revision == root.placement_revision
            && c.defeated == root.defeated
            && c.config == root.config
            && c.team == root.team
    };
    let next = after.1.iter().find(same).unwrap_or(root);
    let fraction = if after.0 == before.0 {
        1.
    } else {
        time_ns.saturating_sub(before.0) as f64 / (after.0 - before.0) as f64
    };
    Some(
        before
            .1
            .iter()
            .find(same)
            .map_or_else(|| next.clone(), |old| blend_chassis(old, next, fraction)),
    )
}
/// Linear interpolation between two world-space vectors, with `t` in 0 to 1.
fn vector(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}
/// Interpolate two poses with `t` in 0 to 1. The quaternion takes the shortest
/// arc and is renormalised, so the result stays a unit rotation.
fn pose(a: Pose, b: Pose, t: f64) -> Pose {
    Pose {
        translation_m: vector(a.translation_m, b.translation_m, t),
        rotation_wxyz: quat_normalize(quat_slerp(a.rotation_wxyz, b.rotation_wxyz, t)),
    }
}
/// Blend `a` into `b` at fraction `t` in 0 to 1 and return the result.
///
/// Body and turret poses and the wheel hubs interpolate, and wheel spin takes
/// the shortest arc so a roll past pi does not spin backwards. Every other field
/// comes from `b`, because only `b` describes the newer lifecycle.
pub fn blend_chassis(a: &ChassisSnapshot, b: &ChassisSnapshot, t: f64) -> ChassisSnapshot {
    let mut result = b.clone();
    result.pose = pose(a.pose, b.pose, t);
    result.turret = pose(a.turret, b.turret, t);
    for (wheel, old) in result.wheels.iter_mut().zip(&a.wheels) {
        wheel.hub_m = vector(old.hub_m, wheel.hub_m, t);
        let delta = (wheel.spin_rad - old.spin_rad + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        wheel.spin_rad = old.spin_rad + delta * t;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{ChassisConfig, ChassisPlacement, Field, FieldConfig, Team};
    fn state() -> FieldSnapshot {
        Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                spawn: Pose::at([0., 0., 1.]),
                team: Team::Red,
                kind: rm_simulator_world::RobotKind::Infantry,
                performance: None,
            }],
            ..Default::default()
        })
        .unwrap()
        .snapshot()
    }
    #[test]
    fn jittered_samples_interpolate_positions_and_shortest_rotation_then_hold() {
        let mut a = state();
        let mut history = RemoteHistory::default();
        let yaw = |r: f64| [(r / 2.).cos(), 0., 0., (r / 2.).sin()];
        a.chassis[0].pose.rotation_wxyz = yaw(179_f64.to_radians());
        history.push(&a, false);
        let mut b = a.clone();
        b.time_ns = 56_000_000;
        b.chassis[0].pose.translation_m[0] = 2.;
        b.chassis[0].pose.rotation_wxyz = yaw(-179_f64.to_radians());
        history.push(&b, false);
        let halfway = history.sample(0, 28_000_000).unwrap();
        assert_eq!(halfway.pose.translation_m[0], 1.);
        assert!(halfway.pose.rotation_wxyz[0].abs() < 1e-12);
        assert_eq!(history.sample(0, 999_000_000).unwrap(), b.chassis[0]);
        assert_eq!(a.chassis[0].pose.translation_m[0], 0.);
    }
    #[test]
    fn same_tick_placement_defeat_despawn_and_pause_discard_stale_poses() {
        let mut frame = state();
        let mut history = RemoteHistory::default();
        history.push(&frame, false);
        frame.time_ns = 16_000_000;
        frame.chassis[0].pose.translation_m[0] = 0.1;
        history.push(&frame, false);
        frame.chassis[0].placement_revision += 1;
        frame.chassis[0].pose.translation_m[0] = 0.2;
        history.push(&frame, false);
        assert_eq!(history.sample(0, 0).unwrap().pose.translation_m[0], 0.2);
        frame.chassis[0].defeated = true;
        history.push(&frame, false);
        assert!(history.sample(0, 0).unwrap().defeated);
        history.push(&frame, true);
        assert_eq!(history.frames.len(), 1);
        frame.chassis.clear();
        history.push(&frame, false);
        assert!(history.sample(0, 0).is_none());
    }
    #[test]
    fn history_is_bounded_and_time_reversal_resets_it() {
        let mut frame = state();
        let mut history = RemoteHistory::default();
        for i in 0..1000 {
            frame.time_ns = i;
            history.push(&frame, false);
        }
        assert_eq!(history.frames.len(), CAPACITY);
        frame.time_ns = 0;
        history.push(&frame, false);
        assert_eq!(history.frames.len(), 1);
    }
}
