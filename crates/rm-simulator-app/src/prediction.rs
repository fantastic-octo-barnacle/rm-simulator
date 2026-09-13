// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! One replaceable replay request and result; physics never waits on rendering.
use rm_simulator_server::prediction::{Replay, ReplayEvent, Replayer};
use rm_simulator_world::{ChassisSnapshot, StaticGeometry};
use std::sync::{Arc, Condvar, Mutex};

/// A local shot the replay has not placed yet: its flight id, the world time in
/// ns the firing input was sampled at, and the chassis command in force then.
#[derive(Clone, Copy)]
pub struct ShotSample {
    /// The client's flight id, echoed back with the resolved muzzle.
    pub id: u64,
    /// World time in ns at which the input was sampled, so the muzzle belongs to
    /// the pose the player actually aimed from.
    pub time_ns: u64,
    /// The chassis command carrying the aim and drive of that tick.
    pub aim: rm_simulator_world::ChassisCommand,
}
#[derive(Default)]
struct Mailbox {
    correction: rm_simulator_server::network_stats::CorrectionStats,
    request: Option<(u64, Replay)>,
    result: Option<(
        u64,
        rm_simulator_server::prediction::PredictionProof,
        ChassisSnapshot,
    )>,
    completed: u64,
    replay_ms: f64,
    replay_ticks: u64,
    stopped: bool,
    shots: Vec<ShotSample>,
    muzzles: Vec<(u64, u64, rm_simulator_world::Pose)>,
}
/// Own-chassis replay on one worker thread with a single request and result
/// slot. The app posts the newest checkpoint and pending inputs and collects the
/// previous result, so a slow replay never blocks a frame and a newer request
/// replaces an older one.
pub struct PredictionWorker {
    #[cfg(test)]
    synchronous: bool,
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}
impl PredictionWorker {
    /// Spawn the worker. It restores a whole `Field` from each checkpoint plus
    /// `geometry` and `floor_height_m`, then replays unfinalized inputs through
    /// the ordinary step, command and fire paths.
    pub fn new(geometry: StaticGeometry, floor_height_m: f64) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("chassis-prediction".into())
            .spawn(move || {
                let mut replayer = Replayer::new(geometry, floor_height_m);
                let mut history = rm_simulator_server::network_stats::CorrectionHistory::default();
                let mut shot_history: std::collections::VecDeque<(u64, ChassisSnapshot)> =
                    std::collections::VecDeque::new();
                let mut shot_epoch = None;
                loop {
                    let ((epoch, replay), shots) = {
                        let (lock, wake) = &*worker;
                        let Ok(mut mailbox) = lock.lock() else { return };
                        while mailbox.request.is_none() && !mailbox.stopped {
                            let Ok(next) = wake.wait(mailbox) else { return };
                            mailbox = next;
                        }
                        if mailbox.stopped {
                            return;
                        }
                        (
                            mailbox.request.take().expect("request woke worker"),
                            std::mem::take(&mut mailbox.shots),
                        )
                    };
                    if shot_epoch != Some(epoch) {
                        shot_history.clear();
                        shot_epoch = Some(epoch);
                    }
                    let mut muzzles = Vec::new();
                    for shot in &shots {
                        if let Some((_, owner)) =
                            shot_history.iter().rev().find(|(t, _)| *t == shot.time_ns)
                        {
                            muzzles.push((
                                epoch,
                                shot.id,
                                rm_simulator_server::prediction::muzzle_for(owner, shot.aim),
                            ));
                        }
                    }
                    let shots: Vec<_> = shots
                        .into_iter()
                        .filter(|s| !muzzles.iter().any(|(_, id, _)| *id == s.id))
                        .collect();
                    // A shot sampled inside an interval this world already
                    // replayed has to be reached again, so rebuild and re-run it.
                    let rewind = shots.iter().any(|shot| {
                        shot.time_ns >= replay.snapshot_time_ns && shot.time_ns < replayer.time_ns()
                    });
                    let started = std::time::Instant::now();
                    let mut replay_ticks = 0;
                    let result = replayer.advance(
                        epoch,
                        &replay,
                        rm_simulator_server::prediction::MAX_CONTINUOUS_REPLAY_NS,
                        rewind,
                        &mut |event| match event {
                            ReplayEvent::Rebuilt => {
                                let Some(owner) =
                                    replay.states.iter().find(|s| s.id == replay.chassis)
                                else {
                                    return;
                                };
                                history.compare(epoch, replay.snapshot_time_ns, owner);
                                let age = replay
                                    .snapshot_time_ns
                                    .saturating_sub(replay.context_time_ns);
                                history.stats.context_age_ns = Some(age);
                                history.stats.context_hint = Some(if age > 100_000_000 {
                                    "stale_collision_context"
                                } else if replay.states.iter().any(|other| {
                                    other.id != owner.id
                                        && other
                                            .pose
                                            .translation_m
                                            .iter()
                                            .zip(owner.pose.translation_m)
                                            .map(|(a, b)| (a - b).powi(2))
                                            .sum::<f64>()
                                            < 2.25
                                }) {
                                    "nearby_robot_possible_contact"
                                } else if owner.wheels.iter().any(|wheel| wheel.contact.is_some()) {
                                    "wheel_contact"
                                } else {
                                    "unknown"
                                });
                            }
                            ReplayEvent::Before { time_ns, owner } => {
                                shot_history.push_back((time_ns, owner.clone()));
                                while shot_history.len() > 512 {
                                    shot_history.pop_front();
                                }
                                for shot in shots.iter().filter(|s| s.time_ns == time_ns) {
                                    muzzles.push((
                                        epoch,
                                        shot.id,
                                        rm_simulator_server::prediction::muzzle_for(
                                            owner, shot.aim,
                                        ),
                                    ));
                                }
                            }
                            ReplayEvent::After { time_ns, owner } => {
                                replay_ticks += 1;
                                history.record(time_ns, owner);
                            }
                        },
                    );
                    let Ok(mut mailbox) = worker.0.lock() else {
                        return;
                    };
                    mailbox.completed += 1;
                    mailbox.replay_ms = started.elapsed().as_secs_f64() * 1000.;
                    mailbox.replay_ticks = replay_ticks;
                    mailbox.correction = history.stats.clone();
                    mailbox.muzzles = muzzles;
                    if let Ok(state) = result {
                        mailbox.result = Some((
                            epoch,
                            rm_simulator_server::prediction::PredictionProof {
                                snapshot_id: replay.snapshot_id,
                                target_time_ns: replayer.time_ns(),
                                last_input_sequence: replayer.last_input_sequence(),
                            },
                            state,
                        ));
                    }
                    #[cfg(test)]
                    worker.1.notify_all();
                }
            })?;
        Ok(Self {
            shared,
            #[cfg(test)]
            synchronous: false,
        })
    }
    /// Fence each submitted replay in deterministic traces, retaining the
    /// ordinary one-request delay when returning the previous result.
    #[cfg(test)]
    pub fn synchronize_for_test(&mut self) {
        self.synchronous = true;
    }
    /// Replace the pending shot samples and return the muzzle poses the last
    /// replay resolved, as `(epoch, flight id, pose)` triples. A triple from an
    /// older epoch than the session's must be discarded.
    pub fn shot_samples(
        &self,
        shots: Vec<ShotSample>,
    ) -> Vec<(u64, u64, rm_simulator_world::Pose)> {
        let Ok(mut mailbox) = self.shared.0.try_lock() else {
            return Vec::new();
        };
        mailbox.shots = shots;
        std::mem::take(&mut mailbox.muzzles)
    }
    /// Replay counters for the console's `state` reply: how many replays have
    /// completed, and the duration in ms and tick count of the last one. `None`
    /// while the worker holds its lock.
    pub fn diagnostics(&self) -> Option<serde_json::Value> {
        self.shared.0.try_lock().ok().map(|m| {
            serde_json::json!({
                "completed": m.completed,
                "last_replay_ms": m.replay_ms,
                "last_replay_ticks": m.replay_ticks,
            })
        })
    }
    /// The newest comparison of the predicted owner against the host's
    /// checkpoint: translation error in m, body rotation and held aim error in
    /// rad, velocity error in m/s, and the collision-context age and clue. `None`
    /// while the worker holds its lock or before the first comparison.
    pub fn correction_stats(&self) -> Option<rm_simulator_server::network_stats::CorrectionStats> {
        self.shared.0.try_lock().ok().map(|m| m.correction.clone())
    }
    /// Take a completed replay before sampling assist, without posting an old
    /// input request. The result carries its epoch, proof and physical owner pose.
    pub fn take_result(
        &self,
    ) -> Option<(
        u64,
        rm_simulator_server::prediction::PredictionProof,
        ChassisSnapshot,
    )> {
        #[cfg(test)]
        let mut mailbox = if self.synchronous {
            self.shared.0.lock().ok()?
        } else {
            self.shared.0.try_lock().ok()?
        };
        #[cfg(not(test))]
        let mut mailbox = self.shared.0.try_lock().ok()?;
        mailbox.result.take()
    }
    /// Post `replay` at `epoch` and return the previous result as
    /// `(epoch, proof, owner state)`. `None` means the worker held its lock, in
    /// which case the request was dropped, or that no result was ready yet.
    pub fn exchange(
        &self,
        epoch: u64,
        replay: Replay,
    ) -> Option<(
        u64,
        rm_simulator_server::prediction::PredictionProof,
        ChassisSnapshot,
    )> {
        #[cfg(test)]
        let mut mailbox = if self.synchronous {
            self.shared.0.lock().ok()?
        } else {
            self.shared.0.try_lock().ok()?
        };
        #[cfg(not(test))]
        let mut mailbox = self.shared.0.try_lock().ok()?;
        #[cfg(test)]
        let completed = mailbox.completed;
        mailbox.request = Some((epoch, replay));
        let result = mailbox.result.take();
        self.shared.1.notify_one();
        #[cfg(test)]
        if self.synchronous {
            while mailbox.completed == completed {
                mailbox = self
                    .shared
                    .1
                    .wait(mailbox)
                    .expect("prediction worker fence");
            }
        }
        result
    }
}
impl Drop for PredictionWorker {
    fn drop(&mut self) {
        // Workers hold this lock only for mailbox swaps, never physics work.
        if let Ok(mut mailbox) = self.shared.0.lock() {
            mailbox.stopped = true;
            mailbox.request = None;
            self.shared.1.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_server::prediction::PendingInput;
    use rm_simulator_world::{
        ChassisCommand, ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, Team,
    };
    #[test]
    fn moving_shot_capture_uses_its_exact_tick_even_after_prediction_passes_it() {
        let mut field = Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                team: Team::Red,
                spawn: Pose::at([0., 0., ChassisConfig::default().rest_height_m()]),
            }],
            ..Default::default()
        })
        .unwrap();
        field.step(200).unwrap();
        let baseline = field.snapshot();
        let worker = PredictionWorker::new(field.static_geometry_snapshot(), 0.).unwrap();
        let command = ChassisCommand {
            forward_m_s: 2.,
            aim_yaw_rad: 0.7,
            ..Default::default()
        };
        field.command_chassis(0, command).unwrap();
        field.step(32).unwrap();
        let expected =
            rm_simulator_server::prediction::muzzle_for(&field.snapshot().chassis[0], command);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut advanced = false;
        loop {
            if advanced {
                let captures = worker.shot_samples(vec![ShotSample {
                    id: 1,
                    time_ns: 232_000_000,
                    aim: command,
                }]);
                if let Some((_, _, muzzle)) = captures.first() {
                    assert!(
                        muzzle
                            .translation_m
                            .into_iter()
                            .zip(expected.translation_m)
                            .all(|(a, b)| (a - b).abs() < 0.01)
                    );
                    break;
                }
            }
            if let Some((_, proof, _)) = worker.exchange(
                1,
                Replay {
                    snapshot: Arc::new(baseline.clone()),
                    snapshot_id: 1,
                    context_id: 1,
                    context_time_ns: baseline.time_ns,
                    chassis: 0,
                    states: baseline.chassis.clone(),
                    snapshot_time_ns: baseline.time_ns,
                    target_time_ns: 264_000_000,
                    inputs: vec![PendingInput {
                        sequence: 1,
                        time_ns: baseline.time_ns,
                        command,
                    }],
                },
            ) {
                advanced |= proof.target_time_ns == 264_000_000;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn snapshot_gap_keeps_physics_running_then_discards_finalized_movement() {
        let mut field = Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                team: Team::Red,
                spawn: Pose::at([0., 0., ChassisConfig::default().rest_height_m()]),
            }],
            ..Default::default()
        })
        .unwrap();
        field.step(200).unwrap();
        let baseline = field.snapshot();
        let worker = PredictionWorker::new(field.static_geometry_snapshot(), 0.).unwrap();
        let input = PendingInput {
            sequence: 1,
            time_ns: baseline.time_ns,
            command: ChassisCommand {
                forward_m_s: 1.5,
                ..Default::default()
            },
        };
        let wait = |snapshot_id, time_ns, target_time_ns| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some((_, proof, state)) = worker.exchange(
                    1,
                    Replay {
                        snapshot: Arc::new(baseline.clone()),
                        snapshot_id,
                        chassis: 0,
                        states: baseline.chassis.clone(),
                        snapshot_time_ns: time_ns,
                        context_time_ns: time_ns,
                        context_id: 0,
                        target_time_ns,
                        inputs: vec![input],
                    },
                ) && proof.snapshot_id == snapshot_id
                    && proof.target_time_ns == target_time_ns
                {
                    break state;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let first = wait(1, baseline.time_ns, 400_000_000);
        let continued = wait(1, baseline.time_ns, 800_000_000);
        assert!(continued.pose.translation_m[0] > first.pose.translation_m[0] + 0.3);
        let corrected = wait(2, 800_000_000, 850_000_000);
        assert!(corrected.pose.translation_m[0].abs() < 0.01);
        assert_eq!(field.snapshot(), baseline);
    }
}
