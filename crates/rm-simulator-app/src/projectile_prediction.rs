// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Provisional balls, fired into a field restored from the last snapshot.
//! Results are presentation only and never mutate authoritative state.
use rm_simulator_server::prediction::muzzle_for;
use rm_simulator_world::{
    ChassisSnapshot, Field, FieldSnapshot, ProjectileSnapshot, Shot, StaticGeometry, tick_ns,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex},
};

/// Work per exchange stays bounded so the render thread is never blocked by a
/// client that fell far behind the host.
const MAX_REPLAY_NS: u64 = 1_000_000_000;
/// The provisional ball ids a client draws are its own, never the host's.
const PROVISIONAL_BIT: u64 = 1 << 63;

/// One provisional ball the client draws for its own unconfirmed shot. It is
/// presentation only; the host's accepted ball replaces it by shot id.
#[derive(Clone)]
pub struct Flight {
    /// Client shot id, unique for the session and never reused. It keys the
    /// flight in a [`Request`] and the ball in an [`Output`].
    pub id: u64,
    /// Muzzle pose in world FLU metres. A provisional flight uses [`muzzle`];
    /// the host's captured muzzle overwrites it once the shot is accepted.
    pub muzzle: rm_simulator_world::Pose,
    /// World time in ns at which the ball appears, so a lagging replay still
    /// launches it at the sampled tick. The host's executed time replaces the
    /// client's estimate on acceptance.
    pub launched_ns: u64,
    /// Weapon configuration the ball flies with: caliber and launch speed in
    /// metres per second along the muzzle's +x.
    pub shot: Shot,
    /// Host projectile id once the host accepted the shot, `None` while the
    /// flight is still a guess. A set value makes the client draw the host's own
    /// ball rather than its replay.
    pub authoritative: Option<u64>,
}
/// One replay request: the checkpoint to restore, the shooter's own prediction
/// and the provisional flights to launch into that restore.
pub struct Request {
    /// Prediction epoch. A change makes the worker rebuild its field, and lets
    /// the caller drop an output that an older epoch produced.
    pub epoch: u64,
    /// Id of the checkpoint in `snapshot`. A change also forces a rebuild.
    pub snapshot_id: u64,
    /// Last authoritative checkpoint, the rules' hidden state included, that the
    /// worker restores a whole `Field` from. Provisional balls meet the same
    /// terrain, armor and moving equipment as the host's ball.
    pub snapshot: FieldSnapshot,
    /// The shooter's presented chassis. The restore applies it with the client's
    /// own command, so the local prediction drives it, while every other chassis
    /// is placed with a cleared command and coasts on the sampled pose.
    pub own: ChassisSnapshot,
    /// Client time in ns to advance the restored field to. The worker clamps the
    /// replay to `MAX_REPLAY_NS` past its current field time.
    pub time_ns: u64,
    /// Provisional flights to launch and report. The worker reads at most the
    /// first 128.
    pub flights: Vec<Flight>,
}
/// Provisional ball states one request produced, for the caller to draw.
pub struct Output {
    /// Epoch of the request that produced this output. The caller drops the
    /// output when the prediction epoch has moved on.
    pub epoch: u64,
    /// Ball state keyed by [`Flight::id`]. Each ball's own id also carries
    /// `PROVISIONAL_BIT`, so a provisional ball cannot be mistaken for a host
    /// projectile id.
    pub projectiles: BTreeMap<u64, ProjectileSnapshot>,
}
#[derive(Default)]
struct Mailbox {
    request: Option<Request>,
    output: Option<Output>,
    stopped: bool,
}
/// Background thread that owns one restored `Field` for the client. It replays
/// provisional flights off the render thread and never mutates authoritative
/// state. Dropping the worker asks the thread to stop; the thread is detached
/// and exits on its own.
pub struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}
/// One restored field the client fires its unconfirmed shots into. It is the
/// whole field, so a provisional ball meets the same terrain, armor and moving
/// equipment the host's ball will. It is rebuilt on each new snapshot, which is
/// also how a confirmed shot is corrected: the host's own ball arrives in the
/// snapshot and replaces the guess.
struct Scene {
    field: Field,
    epoch: u64,
    snapshot_id: u64,
    time_ns: u64,
    /// Flight id to the ball this field gave it.
    fired: BTreeMap<u64, u64>,
}
impl Scene {
    fn restore(request: &Request, geometry: &StaticGeometry, floor_height_m: f64) -> Option<Self> {
        let mut field = Field::restore(&request.snapshot, geometry, floor_height_m).ok()?;
        // The owner stands where its own prediction puts it; peers coast on the
        // pose the caller sampled, with no command of their own to drive them.
        let _ = field.reset_chassis(&request.own);
        for state in &request.snapshot.chassis {
            if state.id != request.own.id {
                let mut peer = state.clone();
                peer.command = Default::default();
                let _ = field.reset_chassis(&peer);
            }
        }
        Some(Self {
            time_ns: field.time_ns(),
            field,
            epoch: request.epoch,
            snapshot_id: request.snapshot_id,
            fired: BTreeMap::new(),
        })
    }
    /// Launch every unconfirmed shot at its own tick and step to `end`.
    fn advance(&mut self, flights: &[&Flight], end: u64) {
        loop {
            for flight in flights {
                if flight.authoritative.is_none()
                    && flight.launched_ns <= self.time_ns
                    && !self.fired.contains_key(&flight.id)
                    && let Ok(id) = self.field.fire(flight.muzzle, flight.shot, None)
                {
                    self.fired.insert(flight.id, id);
                }
            }
            if self.time_ns.saturating_add(tick_ns()) > end || self.field.step(1).is_err() {
                return;
            }
            self.time_ns = self.time_ns.saturating_add(tick_ns());
        }
    }
}
impl Worker {
    /// Spawn the worker with the client's verified collision geometry and the
    /// floor height in metres; `Field::restore` needs both and both stay fixed
    /// for the worker's life. Returns an error when the thread cannot be spawned.
    pub fn new(geometry: StaticGeometry, floor: f64) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let thread = shared.clone();
        std::thread::Builder::new()
            .name("projectile-prediction".into())
            .spawn(move || {
                let mut scene: Option<Scene> = None;
                loop {
                    let request = {
                        let Ok(mut mailbox) = thread.0.lock() else {
                            return;
                        };
                        while mailbox.request.is_none() && !mailbox.stopped {
                            let Ok(next) = thread.1.wait(mailbox) else {
                                return;
                            };
                            mailbox = next;
                        }
                        if mailbox.stopped {
                            return;
                        }
                        mailbox.request.take().expect("prediction request")
                    };
                    if scene.as_ref().is_none_or(|scene| {
                        scene.epoch != request.epoch || scene.snapshot_id != request.snapshot_id
                    }) {
                        scene = Scene::restore(&request, &geometry, floor);
                    }
                    let mut projectiles = BTreeMap::new();
                    if let Some(scene) = &mut scene {
                        let flights: Vec<&Flight> = request.flights.iter().take(128).collect();
                        let end = request
                            .time_ns
                            .max(scene.time_ns)
                            .min(scene.time_ns.saturating_add(MAX_REPLAY_NS));
                        scene.advance(&flights, end);
                        let balls = scene.field.projectile_snapshots();
                        for flight in flights {
                            let ball = flight
                                .authoritative
                                .or_else(|| scene.fired.get(&flight.id).copied())
                                .and_then(|id| balls.iter().find(|ball| ball.id == id));
                            if let Some(ball) = ball {
                                let mut state = ball.clone();
                                state.id = PROVISIONAL_BIT | flight.id;
                                state.launched_ns = flight.launched_ns;
                                projectiles.insert(flight.id, state);
                            }
                        }
                    }
                    if let Ok(mut mailbox) = thread.0.lock() {
                        mailbox.output = Some(Output {
                            epoch: request.epoch,
                            projectiles,
                        });
                    }
                }
            })?;
        Ok(Self { shared })
    }
    /// Hand a request to the worker and take the output of the previous one
    /// without blocking. Returns `None` while the worker holds the lock, so a
    /// caller that fell behind skips a frame instead of stalling the render
    /// thread, and a request that replaces an unread one is dropped.
    pub fn exchange(&self, request: Request) -> Option<Output> {
        let Ok(mut mailbox) = self.shared.0.try_lock() else {
            return None;
        };
        mailbox.request = Some(request);
        let result = mailbox.output.take();
        self.shared.1.notify_one();
        result
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut m) = self.shared.0.lock() {
            m.stopped = true;
            self.shared.1.notify_one();
        }
    }
}

/// Muzzle pose in world FLU metres for `own`, matching the pose the host derives
/// for the same turret and barrel. A provisional flight starts here until the
/// host's captured muzzle arrives. `command` is passed through for call-site
/// symmetry with the fire path and is not read.
pub fn muzzle(
    own: &ChassisSnapshot,
    command: rm_simulator_world::ChassisCommand,
) -> rm_simulator_world::Pose {
    muzzle_for(own, command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{Caliber, ChassisConfig, ChassisPlacement, FieldConfig, Pose, Team};
    #[test]
    fn authoritative_checkpoint_replaces_provisional_trajectory_and_epoch_clears_it() {
        let field = Field::new(&FieldConfig {
            chassis: vec![ChassisPlacement {
                config: ChassisConfig::default(),
                team: Team::Red,
                kind: rm_simulator_world::RobotKind::Infantry,
                spawn: Pose::at([-5., 0., 1.]),
            }],
            ..Default::default()
        })
        .unwrap();
        let worker = Worker::new(field.static_geometry_snapshot(), 0.).unwrap();
        let mut snapshot = field.snapshot();
        let mut flight = Flight {
            id: 1,
            muzzle: Pose::at([0., 0., 2.]),
            launched_ns: 0,
            shot: Shot::at_limit(Caliber::Mm17),
            authoritative: None,
        };
        let wait = |snapshot: &FieldSnapshot, flight: &Flight, epoch, accepted| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Some(output) = worker.exchange(Request {
                    epoch,
                    snapshot_id: if accepted { 2 } else { 1 },
                    snapshot: snapshot.clone(),
                    own: snapshot.chassis[0].clone(),
                    time_ns: 100_000_000,
                    flights: if epoch == 2 {
                        vec![]
                    } else {
                        vec![flight.clone()]
                    },
                }) && output.epoch == epoch
                {
                    if epoch == 2 {
                        assert!(output.projectiles.is_empty());
                        break None;
                    }
                    if let Some(p) = output.projectiles.get(&1)
                        && (!accepted || p.position_m[1] > 1.9)
                    {
                        break Some(p.clone());
                    }
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        };
        let provisional = wait(&snapshot, &flight, 1, false).unwrap();
        assert!(provisional.position_m[0] > 1.);
        flight.authoritative = Some(7);
        snapshot.tick = 100;
        snapshot.time_ns = 100_000_000;
        snapshot.projectiles.push(ProjectileSnapshot {
            id: 7,
            caliber: Caliber::Mm17,
            launched_ns: 0,
            position_m: [0., 2., 2.],
            velocity_m_s: [0., 20., 0.],
            angular_velocity_rad_s: [0.; 3],
            shooter: None,
            first_contact_ns: None,
            dwell_since_ns: None,
        });
        let corrected = wait(&snapshot, &flight, 1, true).unwrap();
        assert_eq!(corrected.id, provisional.id);
        assert_eq!(corrected.position_m, [0., 2., 2.]);
        wait(&snapshot, &flight, 2, true);
        assert_eq!(field.snapshot().shots_fired, 0);
    }
}
