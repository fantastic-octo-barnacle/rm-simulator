// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Prediction contracts and bounded replay, independent of rendering and sockets.
use crate::{cad_assets::CadAssets, layout::LayoutOptions};
use rm_simulator_world::{
    ChassisCommand, ChassisSnapshot, Field, FieldSnapshot, StaticGeometry, tick_ns,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, sync::Arc};

/// Verified package and construction settings. Unknown/custom worlds do not
/// advertise prediction. Asset files are already checked by CadAssets loading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PredictionScene {
    /// Whether the host built terrain into the field. A false value predicts on
    /// a bare field whose catch floor is at height zero.
    pub terrain: bool,
    /// SHA-256 over the two package manifests. A client whose package hashes
    /// differently cannot predict.
    pub package_sha256: Option<String>,
}
impl PredictionScene {
    /// Record the construction settings a client needs, from the same CAD and
    /// options that built the field. Returns None when terrain is enabled but
    /// the package manifests cannot be read.
    pub fn from_cad(cad: &CadAssets, options: &LayoutOptions) -> Option<Self> {
        Some(Self {
            terrain: options.terrain,
            package_sha256: if options.terrain {
                Some(package_hash(cad)?)
            } else {
                None
            },
        })
    }
    /// Rebuild the client's static geometry and catch floor height in metres
    /// from a local package. Fails when the package hash differs from the
    /// host's or when the terrain or the throwaway field cannot be built.
    pub fn geometry(&self, cad: &CadAssets) -> Result<(StaticGeometry, f64), String> {
        if !self.terrain {
            return Ok((StaticGeometry::default(), 0.0));
        }
        if self.package_sha256.is_none() || package_hash(cad) != self.package_sha256 {
            return Err("prediction disabled: field package differs from host".into());
        }
        let terrain = crate::layout::load_terrain(cad).map_err(|e| e.to_string())?;
        let mut field = rm_simulator_world::Field::new(&crate::layout::field_config(
            cad,
            &LayoutOptions {
                rune: Some(rm_simulator_world::RuneKind::Small),
                outpost_speed_rad_s: 0.0,
                terrain: true,
                referee: true,
                physics_rate_hz: 1000,
                projectile_policy: Default::default(),
            },
        ))
        .map_err(|e| e.to_string())?;
        crate::layout::add_terrain(&mut field, &terrain).map_err(|e| e.to_string())?;
        Ok((
            field.static_geometry_snapshot(),
            crate::layout::CATCH_FLOOR_M,
        ))
    }
}
fn package_hash(cad: &CadAssets) -> Option<String> {
    let mut hash = Sha256::new();
    for file in ["manifest.json", "equipment/manifest.json"] {
        let bytes = std::fs::read(cad.root.join(file)).ok()?;
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    Some(format!("{:x}", hash.finalize()))
}

/// Largest number of unfinalized inputs the client retains. Overflow disables
/// replay until a checkpoint finalizes enough of them.
pub const MAX_PENDING_INPUTS: usize = 256;
/// Longest one-shot replay horizon: 200 ms past the checkpoint, in ns. A later
/// target time is clamped to it.
pub const MAX_PREDICTION_NS: u64 = 200_000_000;
/// One second of receipt silence, plus room for ordinary transit and scheduled input lead.
pub const MAX_CONTINUOUS_REPLAY_NS: u64 = 1_500_000_000;
/// One unfinalized control sample on the client's timeline.
#[derive(Clone, Copy, Debug)]
pub struct PendingInput {
    /// Client-assigned identity. Replay skips an input whose sequence it has
    /// already applied in this rebuild, so a repeated frame cannot move the
    /// owner twice.
    pub sequence: u64,
    /// Intended simulation time of the sample, in ns.
    pub time_ns: u64,
    /// Command to apply at `time_ns`.
    pub command: ChassisCommand,
}
/// Client-owned history. Overflow disables replay until the host catches up,
/// instead of silently losing an input transition such as releasing a key.
#[derive(Default)]
pub struct InputHistory {
    next_sequence: u64,
    last_sampled_ns: u64,
    pending: VecDeque<PendingInput>,
    overflowed: bool,
}
impl InputHistory {
    /// Number, clamp and store one sample. The time never moves backwards, so a
    /// corrected client clock cannot reorder a transition. Returns None only
    /// when the sequence counter is exhausted.
    pub fn push(&mut self, time_ns: u64, command: ChassisCommand) -> Option<PendingInput> {
        self.next_sequence = self.next_sequence.checked_add(1)?;
        let time_ns = time_ns.max(self.last_sampled_ns);
        self.last_sampled_ns = time_ns;
        let input = PendingInput {
            sequence: self.next_sequence,
            time_ns,
            command,
        };
        if self.pending.len() == MAX_PENDING_INPUTS {
            self.overflowed = true;
            self.pending.pop_front();
        }
        self.pending.push_back(input);
        Some(input)
    }
    /// Discard finalized intervals even when the corresponding packet was lost.
    /// A checkpoint at T contains steps ending at T; transitions at T remain future.
    pub fn finalize(&mut self, time_ns: u64) {
        self.pending.retain(|input| input.time_ns >= time_ns);
        if self.pending.len() < MAX_PENDING_INPUTS {
            self.overflowed = false;
        }
    }
    /// Drop every pending input and clear the overflow flag. The sequence
    /// counter keeps rising, so a stale input cannot pass as a new one.
    pub fn reset(&mut self) {
        self.last_sampled_ns = 0;
        self.pending.clear();
        self.overflowed = false;
    }
    /// The pending inputs in time order, or None while overflowed. None means
    /// replay is disabled until finalization frees space.
    pub fn inputs(&self) -> Option<Vec<PendingInput>> {
        (!self.overflowed).then(|| self.pending.iter().copied().collect())
    }
}

/// One reconciliation request: the collision context the client last received,
/// the owner checkpoint to start from, and the inputs to replay over it.
pub struct Replay {
    /// The last full snapshot, the whole field this replay is rebuilt from.
    pub snapshot: Arc<FieldSnapshot>,
    /// Identity of the collision context the client last received. A change
    /// makes the replayer restore a different field.
    pub context_id: u64,
    /// Simulation time of that context, in ns. Peer poses are extrapolated from
    /// it.
    pub context_time_ns: u64,
    /// Identity of the owner checkpoint: an owner anchor when one is newer
    /// than the snapshot, otherwise the snapshot itself.
    pub snapshot_id: u64,
    /// Chassis id of the owner this replay predicts.
    pub chassis: u32,
    /// Every chassis to stand up, the owner taken from its checkpoint.
    pub states: Vec<ChassisSnapshot>,
    /// Checkpoint simulation time in ns. Replay starts at the whole tick at or
    /// before it.
    pub snapshot_time_ns: u64,
    /// Simulation time the replay should reach, in ns, clamped by the horizon.
    pub target_time_ns: u64,
    /// Inputs to apply, in ascending intended time.
    pub inputs: Vec<PendingInput>,
}
impl Replay {
    /// Replay once on a field of its own. The persistent client worker keeps a
    /// [`Replayer`] instead, so it rebuilds only when the checkpoint changes.
    pub fn run(
        &self,
        geometry: &StaticGeometry,
        floor_height_m: f64,
    ) -> Result<ChassisSnapshot, &'static str> {
        Replayer::new(geometry.clone(), floor_height_m).advance(
            0,
            self,
            MAX_PREDICTION_NS,
            true,
            &mut |_| {},
        )
    }
}

/// What a replay reports while it runs, so a caller can sample the owner at an
/// exact tick without keeping a second physics world of its own.
pub enum ReplayEvent<'a> {
    /// The field was rebuilt from this request's checkpoint.
    Rebuilt,
    /// The owner's state at `time_ns`, before the tick that starts there.
    Before {
        /// Tick time of the sample, in ns.
        time_ns: u64,
        /// The owner at that time.
        owner: &'a ChassisSnapshot,
    },
    /// The owner's state at `time_ns`, after the tick that ended there.
    After {
        /// Tick time of the sample, in ns.
        time_ns: u64,
        /// The owner at that time.
        owner: &'a ChassisSnapshot,
    },
}

/// A client's own copy of the field, replayed forward from the last checkpoint
/// the host sent. It is a full [`Field`]: the same rules, the same physics and
/// the same commands the host runs, restored from a snapshot instead of built
/// from CAD. Rebuilding is the expensive part, so it happens only when the
/// checkpoint identity changes; otherwise the same world keeps stepping.
///
/// Peers are estimated collision context, not authority: every tick puts them
/// back on their extrapolated observed pose. Solver warm starts cannot be
/// checkpointed, so a rebuild restarts them; the next checkpoint corrects that.
pub struct Replayer {
    geometry: StaticGeometry,
    floor_height_m: f64,
    field: Option<Field>,
    /// Epoch and checkpoint identity the retained field was restored from.
    baseline: Option<(u64, u64, u64)>,
    time_ns: u64,
    last_sequence: u64,
}
impl Replayer {
    /// A replayer over a verified copy of the host's geometry and its catch
    /// floor height in metres. No field is restored until the first advance.
    pub fn new(geometry: StaticGeometry, floor_height_m: f64) -> Self {
        Self {
            geometry,
            floor_height_m,
            field: None,
            baseline: None,
            time_ns: 0,
            last_sequence: 0,
        }
    }
    /// How far the retained field has been replayed.
    pub fn time_ns(&self) -> u64 {
        self.time_ns
    }
    /// The last input applied to the retained field.
    pub fn last_input_sequence(&self) -> u64 {
        self.last_sequence
    }
    /// Step the owner's robot up to `target_time_ns`, at most `horizon_ns` past
    /// the checkpoint. `rewind` forces a rebuild even when the checkpoint is
    /// unchanged, for a caller that must revisit an already replayed tick.
    pub fn advance(
        &mut self,
        epoch: u64,
        replay: &Replay,
        horizon_ns: u64,
        rewind: bool,
        observer: &mut dyn FnMut(ReplayEvent),
    ) -> Result<ChassisSnapshot, &'static str> {
        let start_ns = replay.snapshot_time_ns / tick_ns() * tick_ns();
        let baseline = (epoch, replay.snapshot_id, replay.context_id);
        if rewind || self.baseline != Some(baseline) {
            self.field = Some(self.restore(replay, start_ns)?);
            self.baseline = Some(baseline);
            self.time_ns = start_ns;
            self.last_sequence = 0;
            observer(ReplayEvent::Rebuilt);
        }
        let field = self.field.as_mut().ok_or("prediction unavailable")?;
        let end = replay
            .target_time_ns
            .min(start_ns.saturating_add(horizon_ns));
        loop {
            for input in replay
                .inputs
                .iter()
                .filter(|input| input.time_ns >= start_ns && input.time_ns <= self.time_ns)
            {
                if input.sequence > self.last_sequence {
                    field
                        .command_chassis(replay.chassis, input.command)
                        .map_err(|_| "replayed command rejected")?;
                    self.last_sequence = input.sequence;
                }
            }
            let owner = field
                .chassis_snapshot(replay.chassis)
                .ok_or("prediction chassis missing")?;
            observer(ReplayEvent::Before {
                time_ns: self.time_ns,
                owner: &owner,
            });
            if self.time_ns.saturating_add(tick_ns()) > end {
                return Ok(owner);
            }
            for remote in replay
                .states
                .iter()
                .filter(|state| state.id != replay.chassis)
            {
                let mut state = remote.clone();
                crate::view::extrapolate(
                    &mut state,
                    self.time_ns.saturating_sub(replay.context_time_ns),
                );
                state.command = Default::default();
                field
                    .reset_chassis(&state)
                    .map_err(|_| "replay peer changed")?;
            }
            field.step(1).map_err(|_| "replay stepping failed")?;
            self.time_ns = self.time_ns.saturating_add(tick_ns());
            let owner = field
                .chassis_snapshot(replay.chassis)
                .ok_or("prediction chassis missing")?;
            observer(ReplayEvent::After {
                time_ns: self.time_ns,
                owner: &owner,
            });
        }
    }
    /// Stand the whole field up again at the owner's checkpoint tick. The rules
    /// are time-parameterised, so runes, outposts and the referee catch up from
    /// the context tick on the first step instead of being extrapolated.
    fn restore(&self, replay: &Replay, start_ns: u64) -> Result<Field, &'static str> {
        let mut context = (*replay.snapshot).clone();
        context.tick = start_ns / tick_ns();
        context.time_ns = start_ns;
        context.chassis = replay.states.clone();
        for remote in context
            .chassis
            .iter_mut()
            .filter(|state| state.id != replay.chassis)
        {
            crate::view::extrapolate(remote, start_ns.saturating_sub(replay.context_time_ns));
        }
        if !context
            .chassis
            .iter()
            .any(|state| state.id == replay.chassis)
        {
            return Err("prediction chassis missing");
        }
        Field::restore(&context, &self.geometry, self.floor_height_m)
            .map_err(|_| "cannot restore the replay field")
    }
}

/// What a finished replay reached: the checkpoint it started from, the
/// simulation time it advanced to and the last input it applied. A session
/// discards a proof older than the state it already presents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredictionProof {
    /// Identity of the checkpoint the replay started from.
    pub snapshot_id: u64,
    /// Simulation time the replay reached, in ns.
    pub target_time_ns: u64,
    /// Highest input sequence applied during the replay.
    pub last_input_sequence: u64,
}

/// Update the target command without teleporting the actual motor state.
pub fn apply_aim(chassis: &mut ChassisSnapshot, command: ChassisCommand) {
    chassis.command = command;
}

/// Muzzle from the actual predicted gimbal, never from an unexecuted mouse target.
pub fn muzzle_for(chassis: &ChassisSnapshot, _command: ChassisCommand) -> rm_simulator_world::Pose {
    use crate::math::{add, rotate};
    rm_simulator_world::Pose {
        translation_m: add(
            chassis.turret.translation_m,
            rotate(
                chassis.turret.rotation_wxyz,
                [rm_simulator_world::chassis::MUZZLE_FORWARD_M, 0., 0.],
            ),
        ),
        rotation_wxyz: chassis.turret.rotation_wxyz,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, Team};
    fn field() -> Field {
        Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                team: Team::Red,
                kind: rm_simulator_world::RobotKind::Infantry,
                spawn: Pose::at([0.0, 0.0, ChassisConfig::default().rest_height_m()]),
            }],
            ..Default::default()
        })
        .unwrap()
    }
    fn drive() -> ChassisCommand {
        ChassisCommand {
            forward_m_s: 1.5,
            ..Default::default()
        }
    }
    fn distance(a: &ChassisSnapshot, b: &ChassisSnapshot) -> f64 {
        a.pose
            .translation_m
            .into_iter()
            .zip(b.pose.translation_m)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt()
    }
    #[test]
    fn finalized_checkpoint_discards_lost_inputs_without_waiting_for_sequence_ack() {
        let mut history = InputHistory::default();
        history.push(10_000_000, drive());
        history.push(20_000_000, ChassisCommand::default());
        history.finalize(20_000_000);
        assert_eq!(history.inputs().unwrap().len(), 1);
        assert_eq!(history.inputs().unwrap()[0].time_ns, 20_000_000);
        history.finalize(21_000_000);
        assert!(history.inputs().unwrap().is_empty());
    }
    #[test]
    fn history_bounds_overflow_and_clock_corrections() {
        let mut history = InputHistory::default();
        history.push(100, drive());
        // A backwards client clock never moves an input before its predecessor.
        history.push(90, ChassisCommand::default());
        assert_eq!(history.inputs().unwrap()[1].time_ns, 100);
        for _ in 0..MAX_PENDING_INPUTS {
            history.push(100, drive());
        }
        assert!(history.inputs().is_none());
        assert_eq!(history.pending.len(), MAX_PENDING_INPUTS);
        history.finalize(101);
        assert!(history.inputs().unwrap().is_empty());
        history.reset();
        assert!(history.push(100, drive()).unwrap().sequence > MAX_PENDING_INPUTS as u64);
    }
    #[test]
    fn restoring_flat_ramp_and_robot_contacts_bounds_correction() {
        for scenario in 0..3 {
            let mut host = field();
            if scenario == 1 {
                host.add_static_mesh(
                    vec![
                        [1.0, -2.0, 0.0],
                        [1.0, 2.0, 0.0],
                        [3.0, -2.0, 0.5],
                        [3.0, 2.0, 0.5],
                        [6.0, -2.0, 0.5],
                        [6.0, 2.0, 0.5],
                    ],
                    vec![[0, 2, 1], [1, 2, 3], [2, 4, 3], [3, 4, 5]],
                )
                .unwrap();
            }
            if scenario == 2 {
                host.add_chassis(&ChassisPlacement {
                    config: ChassisConfig::default(),
                    team: Team::Blue,
                    kind: rm_simulator_world::RobotKind::Infantry,
                    spawn: Pose::at([1.0, 0.0, ChassisConfig::default().rest_height_m()]),
                })
                .unwrap();
            }
            host.step(300).unwrap();
            host.command_chassis(0, drive()).unwrap();
            let geometry = host.static_geometry_snapshot();
            let mut maximum: f64 = 0.0;
            // The client keeps one replayer for the whole session and rebuilds
            // it per checkpoint; measure that path as well as a one-shot replay.
            let mut retained = Replayer::new(geometry.clone(), 0.);
            let mut retained_maximum: f64 = 0.;
            for round in 0..50 {
                let snapshot = host.snapshot();
                let replay = Replay {
                    snapshot: Arc::new(snapshot.clone()),
                    snapshot_id: round,
                    chassis: 0,
                    states: snapshot.chassis.clone(),
                    snapshot_time_ns: snapshot.time_ns,
                    context_time_ns: snapshot.time_ns,
                    context_id: round,
                    target_time_ns: snapshot.time_ns + 64 * tick_ns(),
                    inputs: Vec::new(),
                };
                let retained_state = retained
                    .advance(0, &replay, MAX_PREDICTION_NS, false, &mut |_| {})
                    .unwrap();
                let predicted = replay.run(&geometry, 0.0).unwrap();
                host.step(64).unwrap();
                maximum = maximum.max(distance(&predicted, &host.snapshot().chassis[0]));
                retained_maximum =
                    retained_maximum.max(distance(&retained_state, &host.snapshot().chassis[0]));
            }
            eprintln!("scenario {scenario}: maximum 64 ms correction {maximum:.6} m");
            eprintln!("scenario {scenario}: retained solver maximum {retained_maximum:.6} m");
            assert!(
                retained_maximum < 0.025,
                "retained scenario {scenario}: {retained_maximum} m"
            );
            assert!(maximum < 0.025, "scenario {scenario}: {maximum} m");
        }
    }
    #[test]
    fn delayed_jittered_refreshes_replay_start_and_stop_without_double_applying() {
        // Explicit delivery schedule: a 16 ms client refresh grid, 40-45 ms each
        // way and one 100 ms stall. Every packet eventually arrives, in order;
        // this does not model UDP loss.
        let mut host = crate::simulation::Simulation::new(field(), false);
        host.advance(200 * tick_ns()).unwrap();
        let geometry = host.field().static_geometry_snapshot();
        let mut history = InputHistory::default();
        let mut deliveries = VecDeque::new();
        let mut snapshots = VecDeque::new();
        let mut latest = host.state();
        let mut observed_start = false;
        let mut maximum: f64 = 0.0;
        let mut delivered_at = 0;
        for tick in 200..1200u64 {
            let time = tick * tick_ns();
            if tick.is_multiple_of(16) {
                let command = if (224..624).contains(&tick) {
                    drive()
                } else {
                    ChassisCommand::default()
                };
                let input = history.push(time, command).unwrap();
                let delay = 40 + tick % 6 + if tick == 624 { 100 } else { 0 };
                delivered_at = delivered_at.max(tick + delay);
                deliveries.push_back((delivered_at, input));
            }
            while deliveries.front().is_some_and(|(at, _)| *at <= tick) {
                let (_, input) = deliveries.pop_front().unwrap();
                host.apply(&crate::protocol::Command::PilotInput {
                    chassis: 0,
                    frame: crate::input_stream::InputFrame {
                        input_epoch: host.input_epoch(),
                        sequence: input.sequence,
                        sampled_time_ns: input.time_ns,
                        duration_ticks: 16,
                        placement_revision: host.field().chassis_revision(0).unwrap(),
                        command: input.command,
                    },
                })
                .unwrap();
            }
            if tick.is_multiple_of(16) {
                let at = (tick + 30 + tick % 13).max(snapshots.back().map_or(0, |(at, _)| *at));
                snapshots.push_back((at, host.state()));
            }
            while snapshots.front().is_some_and(|(at, _)| *at <= tick) {
                latest = snapshots.pop_front().unwrap().1;
                history.finalize(latest.field.time_ns);
            }
            if tick.is_multiple_of(8) {
                let predicted = Replay {
                    snapshot: Arc::new(latest.field.clone()),
                    snapshot_id: 0,
                    chassis: 0,
                    states: latest.field.chassis.clone(),
                    snapshot_time_ns: latest.field.time_ns,
                    context_time_ns: latest.field.time_ns,
                    context_id: 0,
                    target_time_ns: time,
                    inputs: history.inputs().unwrap(),
                }
                .run(&geometry, 0.0)
                .unwrap();
                maximum = maximum.max(distance(&predicted, &host.snapshot().chassis[0]));
                if tick == 240 {
                    observed_start = predicted.velocity_m_s[0] > 0.01;
                    assert!(host.snapshot().chassis[0].velocity_m_s[0].abs() < 0.01);
                }
                if tick > 1000 {
                    assert_eq!(predicted.command, ChassisCommand::default());
                    assert!(distance(&predicted, &host.snapshot().chassis[0]) < 0.01);
                }
            }
            host.advance(tick_ns()).unwrap();
        }
        assert!(
            observed_start,
            "local drive must respond before host receives input"
        );
        // Only inputs the last received checkpoint has not yet finalized remain.
        assert!(
            history
                .inputs()
                .unwrap()
                .iter()
                .all(|input| input.time_ns >= latest.field.time_ns)
        );
        assert!(maximum < 0.25, "correction {maximum} m");
        eprintln!("jitter/stall maximum position disagreement {maximum:.6} m");
    }
    #[test]
    fn replay_horizon_is_bounded_and_does_not_mutate_authority() {
        let mut host = field();
        host.command_chassis(0, drive()).unwrap();
        let before = host.snapshot();
        let mut replay = Replay {
            snapshot: Arc::new(before.clone()),
            snapshot_id: 0,
            chassis: 0,
            states: before.chassis.clone(),
            snapshot_time_ns: 0,
            context_time_ns: 0,
            context_id: 0,
            target_time_ns: u64::MAX,
            inputs: Vec::new(),
        };
        let capped = replay.run(&StaticGeometry::default(), 0.0).unwrap();
        replay.target_time_ns = MAX_PREDICTION_NS;
        assert_eq!(capped, replay.run(&StaticGeometry::default(), 0.0).unwrap());
        assert_eq!(before, host.snapshot());
    }
    #[test]
    fn input_time_stays_monotonic_after_finalization_and_clock_correction() {
        let mut history = InputHistory::default();
        let first = history.push(100_000_000, Default::default()).unwrap();
        history.finalize(100_000_001);
        assert!(history.inputs().unwrap().is_empty());
        let corrected = history.push(95_000_000, Default::default()).unwrap();
        assert_eq!(corrected.time_ns, first.time_ns);
        assert!(corrected.sequence > first.sequence);
    }
    #[test]
    fn overflow_recovers_when_discarded_inputs_are_finalized() {
        let mut history = InputHistory::default();
        for tick in 0..MAX_PENDING_INPUTS + 2 {
            history
                .push(tick as u64 * tick_ns(), Default::default())
                .unwrap();
        }
        assert!(history.inputs().is_none());
        history.finalize(3 * tick_ns());
        assert_eq!(history.inputs().unwrap().len(), MAX_PENDING_INPUTS - 1);
    }
}
