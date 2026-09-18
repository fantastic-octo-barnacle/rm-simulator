// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! One worker owns a live simulation, its roster and command ordering.
//!
//! Transports submit typed requests through a bounded mailbox. The worker
//! captures and queues snapshots in that same order; socket writers encode
//! them afterwards. No caller can acquire or mutate the hosted simulation.
use crate::clock::TimeSource;
use crate::lifecycle::Stop;
use crate::protocol::{
    ChassisAssignment, ClientMessage, Command, PROTOCOL_VERSION, PlayerInfo, Robot, Role,
    ServerMessage, Welcome,
};
use crate::simulation::{Simulation, SimulationState};
use rm_simulator_world::{FieldSnapshot, StaticGeometry, Team};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const REQUEST_CAPACITY: usize = 256;
const CLOCK_PERIOD: Duration = Duration::from_millis(2);
/// Snapshot broadcast period for network players.
// Full independent UDP checkpoints are larger than deltas. 31 Hz keeps
// ordinary one-player frames below the transport's initial bandwidth budget.
pub const BROADCAST_PERIOD: Duration = Duration::from_millis(32);
/// Embedded-client latency setting, independent of the 1 ms rule tick.
const OWNER_BROADCAST_PERIOD: Duration = Duration::from_millis(4);

/// A message shared by all recipients. Encoding runs once, on a socket writer.
pub(crate) struct Outbound {
    message: ServerMessage,
    compressed: OnceLock<Vec<u8>>,
    uncompressed: OnceLock<Vec<u8>>,
    /// True on a periodic snapshot, whose unsent tail the peer outbox may
    /// replace. Reliable messages leave it false and are never replaced.
    pub(crate) periodic: bool,
}
impl Outbound {
    pub(crate) fn new(message: ServerMessage) -> Self {
        Self {
            message,
            compressed: OnceLock::new(),
            uncompressed: OnceLock::new(),
            periodic: false,
        }
    }
    pub(crate) fn message(&self) -> &ServerMessage {
        &self.message
    }
    /// The framed wire bytes for this message, cached separately per framing.
    /// `raw` selects the loopback transport's uncompressed frame; the network
    /// path uses the ZSTD frame. Both caches live for the message's life, so a
    /// broadcast encodes at most once per framing.
    pub(crate) fn body(&self, raw: bool) -> &[u8] {
        if raw {
            self.uncompressed.get_or_init(|| {
                crate::compression::encode(
                    true,
                    &crate::snapshot_codec::encode_player_message(&self.message),
                )
            })
        } else {
            self.compressed.get_or_init(|| {
                crate::compression::compress(&crate::snapshot_codec::encode_player_message(
                    &self.message,
                ))
            })
        }
    }
}

/// One transport's typed admission request for a socket, decided on the worker.
pub(crate) struct PeerRegistration {
    /// Lobby password the peer offered. The worker checks it unless the
    /// in-process owner is registering.
    pub(crate) password: String,
    /// Display name used in join and leave notices and in the roster.
    pub(crate) name: String,
    /// Requested team. The referee gets none, and a missing team is balanced.
    pub(crate) team: Option<Team>,
    /// Seat the peer asked for.
    pub(crate) role: Role,
    /// The robot a pilot asked to drive; ignored for the other seats.
    pub(crate) robot: Robot,
    /// Owner-only placement as an FLU position in metres and a heading in
    /// degrees. `Some` also grants authority no network hello can claim.
    pub(crate) owner_spawn: Option<([f64; 3], f64)>,
    /// Bounded output queue the worker pushes this peer's messages onto.
    pub(crate) outbox: crate::net::outbox::Sender,
    /// Cancellation the transport shares with this connection's worker.
    pub(crate) stream: Stop,
}
struct Peer {
    info: PlayerInfo,
    owner: bool,
    last_telemetry: Instant,
    outbox: crate::net::outbox::Sender,
    stream: Stop,
}
impl Peer {
    fn push(&self, message: Arc<Outbound>, replaceable: bool) {
        if self.outbox.push(message, replaceable).is_err() {
            // The reader reports Leave; until then this peer retains its seat.
            self.stream.request();
        }
    }
}

/// Position of an accepted operator command in this host's application order.
/// The tick is measured before applying it, including for explicit Step commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandReceipt {
    /// Zero-based ordinal among the commands this host has applied. Operators
    /// and network players share the one order.
    pub sequence: u64,
    /// Simulation tick measured before the command applied, so a Step receipt
    /// reports the tick it started from.
    pub tick: u64,
}

/// Physics shapes and visual state captured in one worker request.
/// Consumers must display both together instead of mixing presentation clocks.
pub struct CollisionCapture {
    /// Moving-collider physics shapes from the capture's worker turn.
    pub geometry: StaticGeometry,
    /// Field state from that same worker turn.
    pub snapshot: FieldSnapshot,
}

type Reply<T> = SyncSender<T>;
enum Request {
    Apply(Command, Reply<Result<CommandReceipt, String>>),
    State(Reply<SimulationState>),
    FireRecords(Reply<Vec<crate::simulation::FireRecord>>),
    Roster(Reply<Vec<PlayerInfo>>),
    Join(PeerRegistration, Reply<Result<Welcome, String>>),
    Leave(u32),
    Client(u32, ClientMessage),
    PilotBatch(u32, Vec<Command>),
    Broadcast(bool),
    StartClock(Reply<io::Result<()>>),
    StaticGeometry(Reply<StaticGeometry>),
    DynamicGeometry(Reply<CollisionCapture>),
    #[cfg(test)]
    Stall(Reply<()>, Receiver<()>),
}

#[derive(Default)]
struct Control {
    stop: Stop,
    completed: Stop,
    ready: AtomicBool,
    failure: Mutex<Option<String>>,
    final_state: Mutex<Option<SimulationState>>,
}

/// Notify waiters even if terminal state capture or worker cleanup panics.
struct WorkerCompletion(Arc<Control>);
impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0
                .failure
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .get_or_insert_with(|| "simulation worker panicked during cleanup".into());
        }
        self.0.stop.request();
        self.0.completed.request();
    }
}

/// Cloneable access to a hosted simulation. Queries and operator commands wait
/// for their turn in the mailbox; frame-facing code uses the nonblocking methods.
#[derive(Clone)]
pub struct HostHandle {
    requests: SyncSender<Request>,
    control: Arc<Control>,
}
impl HostHandle {
    fn send(&self, request: Request) -> Result<(), String> {
        if self.is_stopped() {
            return Err("host is stopped".into());
        }
        self.requests
            .send(request)
            .map_err(|_| "host is stopped".into())
    }
    fn request<T>(&self, build: impl FnOnce(Reply<T>) -> Request) -> Result<T, String> {
        let (reply, result) = mpsc::sync_channel(1);
        self.send(build(reply))?;
        result
            .recv()
            .map_err(|_| "host stopped before replying".into())
    }
    fn try_send(&self, request: Request) -> Result<(), String> {
        if self.is_stopped() {
            return Err("host is stopped".into());
        }
        self.requests
            .try_send(request)
            .map_err(|error| error.to_string())
    }
    /// Apply an operator command at the next available simulation boundary.
    /// Network player permissions are enforced by the separate client request path.
    /// Like the host's HTTP panel, operators may edit the field during loading.
    pub fn apply(&self, command: &Command) -> Result<CommandReceipt, String> {
        self.request(|reply| Request::Apply(*command, reply))?
    }
    /// Capture current state in mailbox order. After shutdown, return the final
    /// read-only state, which includes removal of all connected players' chassis.
    pub fn fire_records(&self) -> Result<Vec<crate::simulation::FireRecord>, String> {
        self.request(Request::FireRecords)
    }
    /// Capture the current state in mailbox order. If the worker has stopped,
    /// return the final state it captured, which has no player chassis left.
    pub fn state(&self) -> Result<SimulationState, String> {
        self.request(Request::State).or_else(|error| {
            self.control
                .final_state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .ok_or(error)
        })
    }
    /// The `field` of [`Self::state`] at this request's worker turn.
    pub fn snapshot(&self) -> Result<FieldSnapshot, String> {
        self.state().map(|state| state.field)
    }
    /// Everyone connected, in arrival order, at this request's worker turn.
    pub fn roster(&self) -> Result<Vec<PlayerInfo>, String> {
        self.request(Request::Roster)
    }
    /// How many peers are connected, counted from a roster capture in order.
    pub fn peer_count(&self) -> Result<usize, String> {
        self.roster().map(|roster| roster.len())
    }
    /// Whether shutdown has been requested; a stopped host refuses new requests.
    pub fn is_stopped(&self) -> bool {
        self.control.stop.wait(Duration::ZERO)
    }
    /// Release the loading hold without blocking the app's frame thread.
    pub fn ready(&self) {
        self.control.ready.store(true, Ordering::Release);
    }
    /// Start periodic stepping and snapshot publication on the worker. Fails
    /// with `AlreadyExists` when the clock already runs, and with
    /// `NotConnected` when the worker has stopped.
    pub fn start_clock(&self) -> io::Result<()> {
        self.request(Request::StartClock)
            .map_err(|error| io::Error::new(io::ErrorKind::NotConnected, error))?
    }
    /// Request a broadcast without waiting for mailbox capacity. A full mailbox
    /// rejects this request; the clock's periodic publications continue normally.
    pub fn broadcast_snapshot(&self, owner_only: bool) -> Result<(), String> {
        self.try_send(Request::Broadcast(owner_only))
    }
    /// Admit one transport's peer and reply with its seat. The worker assigns
    /// the client id, team, role and chassis in its own order, and refuses a
    /// wrong password, a second owner or a failed spawn.
    pub(crate) fn join(&self, peer: PeerRegistration) -> Result<Welcome, String> {
        self.request(|reply| Request::Join(peer, reply))?
    }
    /// Drop the peer with `id`, closing its stream and removing its chassis in
    /// mailbox order. An unknown id is ignored.
    pub(crate) fn leave(&self, id: u32) -> Result<(), String> {
        self.send(Request::Leave(id))
    }
    /// Submit one client message in order. Any answer, including a rejection,
    /// arrives on the peer's own outbox rather than through this call.
    pub(crate) fn message(&self, id: u32, message: ClientMessage) -> Result<(), String> {
        self.send(Request::Client(id, message))
    }
    /// Submit one datagram's pilot inputs for `id`, applied newest sequence
    /// first. A batch holds 1 to 12 frames, each a `PilotInput`.
    pub(crate) fn pilot_batch(&self, id: u32, inputs: Vec<Command>) -> Result<(), String> {
        if inputs.is_empty()
            || inputs.len() > 12
            || inputs
                .iter()
                .any(|c| !matches!(c, Command::PilotInput { .. }))
        {
            return Err("invalid pilot batch".into());
        }
        self.send(Request::PilotBatch(id, inputs))
    }
    /// Capture the fixed collision geometry at this request's worker turn.
    pub fn static_geometry(&self) -> Result<StaticGeometry, String> {
        self.request(Request::StaticGeometry)
    }
    /// Queue a capture without waiting for the simulation or a busy mailbox.
    pub fn try_dynamic_geometry(&self) -> Result<Receiver<CollisionCapture>, String> {
        let (reply, result) = mpsc::sync_channel(1);
        self.try_send(Request::DynamicGeometry(reply))?;
        Ok(result)
    }
    /// Test hook: park the worker until the returned sender is dropped.
    #[cfg(test)]
    pub(crate) fn stall(&self) -> SyncSender<()> {
        let (entered, started) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        self.send(Request::Stall(entered, wait)).unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        release
    }
}

/// Owns the worker lifetime. Handles never keep the worker alive after its host
/// is dropped. Constructing a host starts request handling, but not its clock.
pub struct Host {
    handle: HostHandle,
    worker: Option<JoinHandle<()>>,
}
impl Host {
    /// Take ownership of `simulation` on a new worker paced by the system
    /// clock. `ready` decides whether the clock may step and whether player
    /// commands are admitted; `false` holds both until [`HostHandle::ready`].
    pub fn new(simulation: Simulation, ready: bool) -> io::Result<Self> {
        Self::with_time(simulation, ready, TimeSource::system())
    }
    /// Pace this host from `time` instead of the real clock. Rules still
    /// advance only in whole ticks; a test source makes the pacing exact.
    pub fn with_time(simulation: Simulation, ready: bool, time: TimeSource) -> io::Result<Self> {
        let (requests, receiver) = mpsc::sync_channel(REQUEST_CAPACITY);
        let control = Arc::new(Control {
            ready: AtomicBool::new(ready),
            ..Default::default()
        });
        let handle = HostHandle {
            requests,
            control: control.clone(),
        };
        let worker = thread::Builder::new()
            .name("rm-simulation".into())
            .spawn(move || {
                let _completion = WorkerCompletion(control.clone());
                let mut owner = Owner {
                    simulation,
                    peers: Vec::new(),
                    next_id: 1,
                    next_sequence: 0,
                    next_snapshot_id: 1,
                    next_hit_id: 1,
                    clock: None,
                    observer: crate::network_trace::Observer::new("simulation", time.clone()),
                    time,
                    control: control.clone(),
                };
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| owner.run(receiver)));
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("simulation worker panicked".into()),
                };
                *control.failure.lock().unwrap_or_else(|p| p.into_inner()) = error;
                owner.close();
                *control
                    .final_state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(owner.simulation.state());
            })?;
        Ok(Self {
            handle,
            worker: Some(worker),
        })
    }
    /// A cloneable handle for operators and transports. Dropping it does not
    /// stop the worker; only [`Host::shutdown`] or dropping the `Host` does.
    pub fn handle(&self) -> HostHandle {
        self.handle.clone()
    }
    /// The stop signal behind this host, for transports that own their threads.
    pub(crate) fn stop_signal(&self) -> Stop {
        self.handle.control.stop.clone()
    }
    /// Request shutdown without waiting for the worker to finish its turn.
    pub fn stop(&self) {
        self.handle.control.stop.request();
    }
    /// The worker's terminal error, once it has stopped with one.
    pub fn failure(&self) -> Option<String> {
        self.handle
            .control
            .failure
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    /// Wait for worker cleanup and final state capture, not just a stop request.
    pub fn wait(&self) -> Result<(), String> {
        while !self.handle.control.completed.wait(Duration::from_secs(1)) {}
        self.failure().map_or(Ok(()), Err)
    }
    /// Request shutdown and join the worker, so cleanup and the final state
    /// capture are complete on return.
    pub fn shutdown(&mut self) {
        self.stop();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Clock {
    last: Instant,
    next: Instant,
    last_broadcast: Instant,
    last_owner_broadcast: Instant,
    ready: bool,
}
impl Clock {
    fn new(ready: bool, now: Instant) -> Self {
        Self {
            last: now,
            next: now + CLOCK_PERIOD,
            last_broadcast: now,
            last_owner_broadcast: now,
            ready,
        }
    }
}
struct Owner {
    observer: crate::network_trace::Observer,
    simulation: Simulation,
    peers: Vec<Peer>,
    next_id: u32,
    next_sequence: u64,
    next_snapshot_id: u64,
    next_hit_id: u64,
    clock: Option<Clock>,
    /// The only wall clock this worker reads.
    time: TimeSource,
    control: Arc<Control>,
}
impl Owner {
    fn run(&mut self, requests: Receiver<Request>) -> Result<(), String> {
        while !self.control.stop.wait(Duration::ZERO) {
            // Check the deadline between requests, even under a command flood.
            self.advance_clock()?;
            for result in self.simulation.take_completed_shots() {
                if let Some(client) = self
                    .peers
                    .iter()
                    .find(|p| p.info.chassis == Some(result.shooter))
                    .map(|p| p.info.client_id)
                {
                    self.send_to(client, ServerMessage::ShotResult(result));
                }
            }
            let timeout = self.clock.as_ref().map_or(CLOCK_PERIOD, |clock| {
                clock.next.saturating_duration_since(self.time.now())
            });
            match requests.recv_timeout(timeout) {
                Ok(request) => {
                    if self.control.stop.wait(Duration::ZERO) {
                        break;
                    }
                    self.process(request);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    }
    fn advance_clock(&mut self) -> Result<(), String> {
        let now = self.time.now();
        let Some(clock) = &mut self.clock else {
            return Ok(());
        };
        if now < clock.next {
            return Ok(());
        }
        let ready = self.control.ready.load(Ordering::Acquire);
        let delta_ns = (ready && clock.ready)
            .then(|| u64::try_from((now - clock.last).as_nanos()).unwrap_or(u64::MAX));
        if let Some(delta_ns) = delta_ns {
            self.observe_simulation(|simulation, hits| simulation.advance_observed(delta_ns, hits))
                .map_err(|error| error.to_string())?;
        }
        let clock = self.clock.as_mut().expect("running clock");
        clock.ready = ready;
        clock.last = now;
        clock.next = now + CLOCK_PERIOD;
        if now - clock.last_broadcast >= BROADCAST_PERIOD {
            clock.last_broadcast = now;
            clock.last_owner_broadcast = now;
            self.publish_snapshot(false);
        } else if now - clock.last_owner_broadcast >= OWNER_BROADCAST_PERIOD {
            clock.last_owner_broadcast = now;
            self.publish_snapshot(true);
        }
        Ok(())
    }
    fn process(&mut self, request: Request) {
        match request {
            Request::Apply(command, reply) => {
                let _ = reply.send(self.apply(command));
            }
            Request::FireRecords(reply) => {
                let _ = reply.send(self.simulation.fire_records());
            }
            Request::State(reply) => {
                let _ = reply.send(self.simulation.state());
            }
            Request::Roster(reply) => {
                let _ = reply.send(self.roster());
            }
            Request::Join(peer, reply) => {
                let result = self.join(peer);
                // A cancelled admission must not strand a chassis or a seat.
                if let Err(error) = reply.send(result)
                    && let Ok(welcome) = error.0
                {
                    self.leave(welcome.client_id);
                }
            }
            Request::Leave(id) => self.leave(id),
            Request::Client(id, message) => self.client_message(id, message),
            Request::PilotBatch(id, mut inputs) => {
                // Newest first prevents redundant old transitions skewing arrival
                // feedback. No host tick can split one datagram's application.
                inputs.sort_by_key(|c| match c {
                    Command::PilotInput { frame, .. } => std::cmp::Reverse(frame.sequence),
                    _ => unreachable!(),
                });
                for input in inputs {
                    self.client_message(id, ClientMessage::Command(input));
                }
            }
            Request::Broadcast(owner_only) => self.publish_snapshot(owner_only),
            Request::StartClock(reply) => {
                let result = if self.clock.is_some() {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "server clock already started",
                    ))
                } else {
                    self.clock = Some(Clock::new(
                        self.control.ready.load(Ordering::Acquire),
                        self.time.now(),
                    ));
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Request::StaticGeometry(reply) => {
                let _ = reply.send(self.simulation.field().fixed_geometry_snapshot());
            }
            Request::DynamicGeometry(reply) => {
                let field = self.simulation.field();
                let _ = reply.send(CollisionCapture {
                    geometry: field.dynamic_geometry_snapshot(),
                    snapshot: field.snapshot(),
                });
            }
            #[cfg(test)]
            Request::Stall(entered, release) => {
                let _ = entered.send(());
                while !self.control.stop.wait(Duration::ZERO) {
                    match release.recv_timeout(CLOCK_PERIOD) {
                        Err(RecvTimeoutError::Timeout) => {}
                        _ => break,
                    }
                }
            }
        }
    }
    fn apply(&mut self, command: Command) -> Result<CommandReceipt, String> {
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or("command sequence overflow")?;
        let receipt = CommandReceipt {
            sequence: self.next_sequence,
            tick: self.simulation.field().tick(),
        };
        self.observe_simulation(|simulation, hits| simulation.apply_observed(&command, hits))?;
        self.next_sequence = next_sequence;
        Ok(receipt)
    }
    fn roster(&self) -> Vec<PlayerInfo> {
        self.peers.iter().map(|peer| peer.info.clone()).collect()
    }
    fn broadcast(&self, message: ServerMessage) {
        self.observer.server("host_publish", &message);
        let message = Arc::new(Outbound::new(message));
        for peer in &self.peers {
            peer.push(message.clone(), false);
        }
    }
    fn send_to(&self, id: u32, message: ServerMessage) {
        self.observer.server("host_publish", &message);
        if let Some(peer) = self.peers.iter().find(|peer| peer.info.client_id == id) {
            peer.push(Arc::new(Outbound::new(message)), false);
        }
    }
    /// Stream scored contacts into bounded peer outboxes during simulation work.
    /// Snapshot history is recovery data, never the source of reliable delivery.
    fn observe_simulation<T>(
        &mut self,
        run: impl FnOnce(&mut Simulation, &mut dyn FnMut(&rm_simulator_world::ArmorHit)) -> T,
    ) -> T {
        let epoch = self.simulation.input_epoch();
        let peers = &self.peers;
        let observer = &self.observer;
        let next_hit_id = &mut self.next_hit_id;
        run(&mut self.simulation, &mut |hit| {
            let event_id = *next_hit_id;
            *next_hit_id = event_id.checked_add(1).expect("hit event id overflow");
            let frame = Arc::new(Outbound::new(ServerMessage::Hit {
                epoch,
                event_id,
                hit: hit.clone(),
            }));
            observer.server("host_publish", frame.message());
            for peer in peers {
                peer.push(frame.clone(), false);
            }
        })
    }
    fn publish_snapshot(&mut self, owner_only: bool) {
        if !self.peers.iter().any(|peer| !owner_only || peer.owner) {
            return;
        }
        let now = self.time.now();
        let mut state = self.simulation.state();
        state.snapshot_id = self.next_snapshot_id;
        self.next_snapshot_id = self
            .next_snapshot_id
            .checked_add(1)
            .expect("snapshot id overflow");
        for peer in &mut self.peers {
            if !peer.owner
                && now.saturating_duration_since(peer.last_telemetry) >= Duration::from_secs(1)
            {
                if let Some(id) = peer.info.chassis
                    && let Some(stream) = self.simulation.input_streams.get(&id)
                {
                    let telemetry = crate::network_stats::HostTelemetry(
                        state.input_epoch,
                        state.field.time_ns,
                        id,
                        stream.telemetry,
                    );
                    peer.push(
                        Arc::new(Outbound::new(ServerMessage::Telemetry(telemetry))),
                        false,
                    );
                }
                peer.last_telemetry = now;
            }
        }
        let mut outbound = Outbound::new(ServerMessage::Snapshot(Box::new(state)));
        self.observer.server("host_publish", outbound.message());
        outbound.periodic = true;
        let frame = Arc::new(outbound);
        for peer in &self.peers {
            if !owner_only || peer.owner {
                peer.push(frame.clone(), true);
            }
        }
    }
    fn join(&mut self, registration: PeerRegistration) -> Result<Welcome, String> {
        let PeerRegistration {
            password,
            name,
            team,
            role,
            robot,
            owner_spawn,
            outbox,
            stream,
        } = registration;
        if owner_spawn.is_none() && self.simulation.password != password {
            return Err("Incorrect lobby password".into());
        }
        if owner_spawn.is_some() && self.peers.iter().any(|peer| peer.owner) {
            return Err("an owner is already connected".into());
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or("client id overflow")?;
        let team = if role == Role::Referee {
            None
        } else {
            Some(team.unwrap_or_else(|| {
                let count = |team| {
                    self.peers
                        .iter()
                        .filter(|peer| peer.info.team == Some(team) && peer.info.chassis.is_some())
                        .count()
                };
                if count(Team::Blue) < count(Team::Red) {
                    Team::Blue
                } else {
                    Team::Red
                }
            }))
        };
        let chassis = if let (Role::Pilot, Some(team)) = (role, team) {
            let spawned = match owner_spawn {
                Some((position, yaw)) => self.simulation.spawn_robot_at(team, robot, position, yaw),
                None => self.simulation.spawn_robot(team, robot),
            };
            if owner_spawn.is_some()
                && let Err(error) = &spawned
            {
                return Err(error.clone());
            }
            spawned.ok().map(|id| ChassisAssignment {
                id,
                config: self
                    .simulation
                    .field()
                    .chassis_config(id)
                    .expect("new chassis exists")
                    .clone(),
                robot,
            })
        } else {
            None
        };
        let welcome = Welcome {
            prediction_scene: self.simulation.prediction_scene(),
            protocol: PROTOCOL_VERSION,
            client_id: id,
            team,
            role,
            chassis: chassis.clone(),
            weapon: chassis.as_ref().map_or_else(
                || self.simulation.weapon(),
                |chassis| self.simulation.weapon_for(chassis.id),
            ),
            weapon_limits: chassis.as_ref().map_or_else(
                || self.simulation.weapon_limits(),
                |chassis| self.simulation.weapon_limits_for(chassis.id),
            ),
        };
        let peer = Peer {
            info: PlayerInfo {
                client_id: id,
                name: name.clone(),
                team,
                role,
                chassis: chassis.as_ref().map(|chassis| chassis.id),
                robot: chassis.map(|chassis| chassis.robot),
            },
            owner: owner_spawn.is_some(),
            last_telemetry: self.time.now(),
            outbox,
            stream,
        };
        peer.push(
            Arc::new(Outbound::new(ServerMessage::Welcome(Box::new(
                welcome.clone(),
            )))),
            false,
        );
        self.peers.push(peer);
        self.broadcast(ServerMessage::Notice(format!(
            "{name} joined as {}",
            crate::protocol::describe_seat(
                team,
                role,
                welcome.chassis.as_ref().map(|c| c.id),
                welcome.chassis.as_ref().map(|c| c.robot),
            )
        )));
        self.broadcast(ServerMessage::Roster(self.roster()));
        Ok(welcome)
    }
    fn leave(&mut self, id: u32) {
        let Some(index) = self.peers.iter().position(|peer| peer.info.client_id == id) else {
            return;
        };
        let peer = self.peers.remove(index);
        peer.stream.request();
        if let Some(chassis) = peer.info.chassis {
            let _ = self.simulation.remove_chassis(chassis);
        }
        self.broadcast(ServerMessage::Notice(format!("{} left", peer.info.name)));
        self.broadcast(ServerMessage::Roster(self.roster()));
    }
    fn close(&mut self) {
        for peer in self.peers.drain(..) {
            peer.stream.request();
            if let Some(chassis) = peer.info.chassis {
                let _ = self.simulation.remove_chassis(chassis);
            }
        }
    }
    fn client_message(&mut self, id: u32, message: ClientMessage) {
        self.observer.client(
            "host_receive",
            Some(id),
            &message,
            Some(self.simulation.field().time_ns()),
        );
        let Some(peer) = self.peers.iter().find(|peer| peer.info.client_id == id) else {
            return;
        };
        match message {
            ClientMessage::Hello { .. } => {}
            ClientMessage::TimeProbe { nonce } => {
                self.send_to(
                    id,
                    ServerMessage::TimeSample {
                        nonce,
                        time_ns: self.simulation.field().time_ns(),
                        paused: self.simulation.paused(),
                    },
                );
            }
            ClientMessage::Ping { nonce } => {
                let mut state = self.simulation.state();
                state.snapshot_id = self.next_snapshot_id;
                self.next_snapshot_id = self
                    .next_snapshot_id
                    .checked_add(1)
                    .expect("snapshot id overflow");
                self.send_to(id, ServerMessage::Snapshot(Box::new(state)));
                self.send_to(id, ServerMessage::Pong { nonce });
            }
            ClientMessage::Command(command) => {
                let own_chassis = peer.info.chassis;
                let refused = match command {
                    _ if !self.control.ready.load(Ordering::Acquire) => {
                        Some("host is still loading")
                    }
                    Command::PlaceChassis { .. } if !peer.owner => {
                        Some("only the embedded owner places a chassis")
                    }
                    Command::PilotInput { chassis, .. }
                    | Command::Respawn { chassis }
                    | Command::ResetRobot { chassis }
                    | Command::BuyAmmo { chassis, .. }
                    | Command::SetPerformance { chassis, .. }
                    | Command::ConfigureWeapon { chassis, .. }
                    | Command::Chassis { chassis, .. }
                    | Command::PlaceChassis { chassis, .. }
                        if Some(chassis) != own_chassis =>
                    {
                        Some("that chassis is not yours")
                    }
                    Command::Fire { .. } | Command::FireAimed { .. } if own_chassis.is_none() => {
                        Some("only a pilot with a chassis fires")
                    }
                    Command::Fire { shooter, .. } | Command::FireAimed { shooter, .. }
                        if Some(shooter) != own_chassis =>
                    {
                        Some("fire from your own chassis only")
                    }
                    Command::SpawnProjectile { .. } if !peer.owner || own_chassis.is_some() => {
                        Some("only the embedded free camera spawns projectiles")
                    }
                    command
                        if command.is_match_control()
                            && !(peer.info.role.referees() || peer.owner) =>
                    {
                        Some("only the referee runs the match")
                    }
                    _ => None,
                };
                // Stale refresh/evidence packets are expected under loss. Do not
                // amplify them into reliable rejection traffic. A dependent shot
                // receives its own explicit validation result.
                if refused.is_none()
                    && let Command::PilotInput { frame, .. } = command
                    && (frame.input_epoch != self.simulation.input_epoch()
                        || frame.sampled_time_ns
                            > self
                                .simulation
                                .field()
                                .time_ns()
                                .saturating_add(200_000_000)
                        || self
                            .simulation
                            .field()
                            .time_ns()
                            .saturating_sub(frame.sampled_time_ns)
                            > 1_000_000_000)
                {
                    return;
                }
                let authorized = refused.is_none();
                let result = refused.map_or_else(
                    || self.apply(command).map(|_| ()),
                    |reason| Err(reason.into()),
                );
                self.observer.client(
                    if result.is_ok() {
                        "host_accept"
                    } else {
                        "host_reject"
                    },
                    Some(id),
                    &ClientMessage::Command(command),
                    Some(self.simulation.field().time_ns()),
                );
                if authorized
                    && result.is_ok()
                    && let Command::FireAimed {
                        shooter, shot_id, ..
                    } = command
                    && self.simulation.is_shot_scheduled(shooter, shot_id)
                {
                    self.send_to(id, ServerMessage::ShotScheduled { shooter, shot_id });
                    return;
                }
                if let Command::FireAimed {
                    shooter, shot_id, ..
                } = command
                {
                    let result = authorized
                        .then(|| self.simulation.shot_result(shooter, shot_id).cloned())
                        .flatten()
                        .unwrap_or(crate::protocol::ShotResult {
                            executed_time_ns: None,
                            launch_speed_m_s: None,
                            shooter,
                            shot_id,
                            result: Err(result.err().unwrap_or_else(|| "shot not accepted".into())),
                        });
                    self.send_to(id, ServerMessage::ShotResult(result));
                } else if let Err(reason) = result {
                    self.send_to(id, ServerMessage::Rejected { reason });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ChassisSpawner;
    use rm_simulator_world::{ChassisConfig, Field, FieldConfig};

    fn host() -> Host {
        timed_host(TimeSource::system())
    }
    fn timed_host(time: TimeSource) -> Host {
        Host::with_time(
            Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true).with_spawner(
                ChassisSpawner {
                    config: ChassisConfig::default(),
                    terrain: None,
                },
            ),
            true,
            time,
        )
        .unwrap()
    }
    /// Two ordered round trips: the worker checks its clock before each receive,
    /// so the second reply is answered after an advance at the new reading.
    fn settle(handle: &HostHandle) {
        handle.roster().unwrap();
        handle.roster().unwrap();
    }
    fn peer(handle: &HostHandle, role: Role) -> (Welcome, crate::net::outbox::Receiver) {
        let (outbox, messages) = crate::net::outbox::channel(64);
        let welcome = handle
            .join(PeerRegistration {
                password: String::new(),
                name: "test".into(),
                team: None,
                role,
                robot: Robot::default(),
                owner_spawn: None,
                outbox,
                stream: Stop::default(),
            })
            .unwrap();
        (welcome, messages)
    }
    fn next(messages: &crate::net::outbox::Receiver) -> Arc<Outbound> {
        messages.recv().unwrap()
    }

    #[test]
    fn contacts_leave_as_reliable_events_without_a_snapshot_broadcast() {
        for ticks in [100, 2_500] {
            assert_contact_delivery(ticks);
        }
    }
    fn assert_contact_delivery(ticks: u64) {
        use rm_simulator_world::{BaseConfig, Caliber, Pose, Shot, Team};
        let field = Field::new(&FieldConfig {
            bases: vec![BaseConfig {
                team: Team::Red,
                plates: std::array::from_fn(|i| Pose::at([0., i as f64 * 0.5, 1.])),
                dart_offsets_m: [[0.; 3]; 2],
            }],
            runes: vec![],
            outposts: vec![],
            ..Default::default()
        })
        .unwrap();
        let host = Host::new(Simulation::new(field, true), true).unwrap();
        let handle = host.handle();
        let (_, messages) = peer(&handle, Role::Referee);
        for _ in 0..3 {
            next(&messages);
        }
        handle
            .apply(&Command::SpawnProjectile {
                muzzle: Pose::yawed([0.05, 0., 1.002], std::f64::consts::PI),
                shot: Shot {
                    caliber: Caliber::Mm17,
                    speed_m_s: 20.,
                },
            })
            .unwrap();
        handle.apply(&Command::Step { ticks }).unwrap();
        settle(&handle);
        let event = next(&messages);
        assert!(!event.periodic);
        let ServerMessage::Hit { event_id, hit, .. } = event.message() else {
            panic!("expected contact event: {:?}", event.message());
        };
        assert_eq!(*event_id, 1);
        assert!(hit.detected);
        let snapshot = handle.snapshot().unwrap();
        if ticks < 1_000 {
            assert_eq!(snapshot.hits[0], *hit);
        } else {
            assert!(snapshot.hits.is_empty(), "history should have expired");
        }
        handle.apply(&Command::Step { ticks: 1 }).unwrap();
        settle(&handle);
        assert!(
            messages.try_recv().is_none(),
            "contacts must not be republished"
        );
    }

    #[test]
    fn client_and_operator_commands_share_one_order_and_confirmation_boundary() {
        let host = host();
        let handle = host.handle();
        let (welcome, messages) = peer(&handle, Role::Referee);
        assert!(matches!(
            next(&messages).message(),
            ServerMessage::Welcome(_)
        ));
        assert!(matches!(
            next(&messages).message(),
            ServerMessage::Notice(_)
        ));
        assert!(matches!(
            next(&messages).message(),
            ServerMessage::Roster(_)
        ));
        let id = welcome.client_id;
        handle
            .message(id, ClientMessage::Command(Command::Step { ticks: 7 }))
            .unwrap();
        let receipt = handle.apply(&Command::Step { ticks: 11 }).unwrap();
        assert_eq!(
            receipt,
            CommandReceipt {
                sequence: 1,
                tick: 7
            }
        );
        handle
            .message(id, ClientMessage::Ping { nonce: 42 })
            .unwrap();
        handle.apply(&Command::Step { ticks: 13 }).unwrap();
        handle.broadcast_snapshot(false).unwrap();

        let confirmation = next(&messages);
        let ServerMessage::Snapshot(state) = confirmation.message() else {
            panic!("missing confirmation state")
        };
        assert_eq!(state.field.tick, 18);
        assert!(matches!(
            next(&messages).message(),
            ServerMessage::Pong { nonce: 42 }
        ));
        let periodic = next(&messages);
        let ServerMessage::Snapshot(state) = periodic.message() else {
            panic!("missing periodic state")
        };
        assert_eq!(state.field.tick, 31);
        // Even with no socket writer running, the host keeps applying commands.
        // Neither snapshot was encoded while the simulation worker owned it.
        assert!(confirmation.compressed.get().is_none());
        assert!(confirmation.uncompressed.get().is_none());
        assert!(periodic.compressed.get().is_none());
        assert!(periodic.uncompressed.get().is_none());
    }

    #[test]
    fn remote_pilots_cannot_manage_training_bots() {
        let host = host();
        let handle = host.handle();
        let (welcome, messages) = peer(&handle, Role::Pilot);
        for _ in 0..3 {
            next(&messages);
        }
        for command in [
            Command::SpawnBot {
                team: Team::Blue,
                spin_rad_s: 3.,
            },
            Command::RemoveBot { chassis: 0 },
            Command::ClearBots,
        ] {
            handle
                .message(welcome.client_id, ClientMessage::Command(command))
                .unwrap();
            assert!(
                matches!(next(&messages).message(), ServerMessage::Rejected {reason} if reason == "only the referee runs the match")
            );
        }
    }

    #[test]
    fn pilots_cannot_purchase_ammo_for_another_chassis() {
        let host = host();
        let handle = host.handle();
        let (welcome, messages) = peer(&handle, Role::Pilot);
        for _ in 0..3 {
            next(&messages);
        }
        let chassis = welcome.chassis.unwrap().id;
        for command in [
            Command::Respawn {
                chassis: chassis + 1,
            },
            Command::ResetRobot {
                chassis: chassis + 1,
            },
            Command::ConfigureWeapon {
                chassis: chassis + 1,
                weapon: Default::default(),
            },
        ] {
            handle
                .message(welcome.client_id, ClientMessage::Command(command))
                .unwrap();
            assert!(
                matches!(next(&messages).message(), ServerMessage::Rejected { reason } if reason == "that chassis is not yours")
            );
        }
        handle
            .message(
                welcome.client_id,
                ClientMessage::Command(Command::BuyAmmo {
                    chassis: chassis + 1,
                    caliber: rm_simulator_world::Caliber::Mm17,
                }),
            )
            .unwrap();
        assert!(
            matches!(next(&messages).message(), ServerMessage::Rejected { reason } if reason == "that chassis is not yours")
        );
    }

    #[test]
    fn pilot_input_checks_ownership_and_confirms_application_before_pong() {
        let host = host();
        let handle = host.handle();
        let (welcome, messages) = peer(&handle, Role::Pilot);
        for _ in 0..3 {
            next(&messages);
        }
        let chassis = welcome.chassis.unwrap().id;
        let placement_revision = handle
            .snapshot()
            .unwrap()
            .chassis
            .iter()
            .find(|c| c.id == chassis)
            .unwrap()
            .placement_revision;
        let command = |chassis, sequence| {
            ClientMessage::Command(Command::PilotInput {
                chassis,
                frame: crate::input_stream::InputFrame {
                    input_epoch: 0,
                    sequence,
                    sampled_time_ns: 0,
                    duration_ticks: 16,
                    placement_revision,
                    command: rm_simulator_world::ChassisCommand {
                        forward_m_s: 1.5,
                        ..Default::default()
                    },
                },
            })
        };
        handle
            .message(welcome.client_id, command(chassis + 1, 1))
            .unwrap();
        assert!(
            matches!(next(&messages).message(), ServerMessage::Rejected { reason } if reason == "that chassis is not yours")
        );
        handle
            .message(welcome.client_id, command(chassis, 1))
            .unwrap();
        handle
            .message(welcome.client_id, ClientMessage::Ping { nonce: 99 })
            .unwrap();
        let snapshot = next(&messages);
        let ServerMessage::Snapshot(state) = snapshot.message() else {
            panic!("confirmation missing")
        };
        assert_eq!(
            state
                .field
                .chassis
                .iter()
                .find(|c| c.id == chassis)
                .unwrap()
                .command
                .forward_m_s,
            1.5
        );
        assert!(matches!(
            next(&messages).message(),
            ServerMessage::Pong { nonce: 99 }
        ));
    }

    #[test]
    fn a_stalled_writer_keeps_only_the_latest_periodic_state() {
        let host = host();
        let handle = host.handle();
        let (_, messages) = peer(&handle, Role::Spectator);
        for _ in 0..3 {
            next(&messages);
        }
        for _ in 0..1_000 {
            handle.apply(&Command::Step { ticks: 1 }).unwrap();
            handle.broadcast_snapshot(false).unwrap();
        }
        // State queries wait for preceding publications to reach the outbox.
        assert_eq!(handle.state().unwrap().field.tick, 1_000);
        assert_eq!(messages.queued(), 1);
        let latest = next(&messages);
        assert!(matches!(latest.message(), ServerMessage::Snapshot(s) if s.field.tick == 1_000));
        assert!(latest.compressed.get().is_none());
        assert!(latest.uncompressed.get().is_none());
    }

    #[test]
    fn concurrent_operators_receive_a_total_order_of_application_ticks() {
        let host = host();
        let handle = host.handle();
        let start = Arc::new(std::sync::Barrier::new(3));
        let workers: Vec<_> = [3, 5]
            .into_iter()
            .map(|ticks| {
                let handle = handle.clone();
                let start = start.clone();
                thread::spawn(move || {
                    start.wait();
                    (0..20)
                        .map(|_| (handle.apply(&Command::Step { ticks }).unwrap(), ticks))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        start.wait();
        let mut applied: Vec<_> = workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect();
        applied.sort_by_key(|(receipt, _)| receipt.sequence);
        let mut tick = 0;
        for (sequence, (receipt, ticks)) in applied.into_iter().enumerate() {
            assert_eq!(
                receipt,
                CommandReceipt {
                    sequence: sequence as u64,
                    tick
                }
            );
            tick += ticks;
        }
        assert_eq!(handle.snapshot().unwrap().tick, tick);
    }

    #[test]
    fn bounded_mailbox_and_shutdown_release_pending_callers() {
        let mut host = host();
        let handle = host.handle();
        let release = handle.stall();
        for _ in 0..REQUEST_CAPACITY {
            handle.broadcast_snapshot(false).unwrap();
        }
        assert!(handle.broadcast_snapshot(false).is_err());
        assert!(handle.try_dynamic_geometry().is_err());
        let pending = handle.clone();
        let waiter = thread::spawn(move || pending.apply(&Command::Step { ticks: 10 }));
        // Stop does not need a free mailbox slot and wakes the stalled owner.
        host.shutdown();
        assert!(waiter.join().unwrap().is_err());
        drop(release);
        assert!(handle.is_stopped());
        assert!(handle.apply(&Command::Step { ticks: 1 }).is_err());
        assert_eq!(handle.snapshot().unwrap().tick, 0);
    }

    #[test]
    fn joins_and_leaves_keep_spawn_seats_and_final_state_consistent() {
        let mut host = host();
        let handle = host.handle();
        let (red, _red_messages) = peer(&handle, Role::Pilot);
        let (blue, _blue_messages) = peer(&handle, Role::Pilot);
        assert_eq!((red.team, blue.team), (Some(Team::Red), Some(Team::Blue)));
        let red_id = red.chassis.unwrap().id;
        let blue_id = blue.chassis.unwrap().id;
        handle.leave(red.client_id).unwrap();
        handle.leave(red.client_id).unwrap();
        let (replacement, _messages) = peer(&handle, Role::Pilot);
        assert_eq!(replacement.team, Some(Team::Red));
        assert!(replacement.chassis.unwrap().id > red_id.max(blue_id));
        assert_eq!(handle.roster().unwrap().len(), 2);
        host.stop();
        host.wait().unwrap();
        assert!(handle.snapshot().unwrap().chassis.is_empty());
        host.shutdown();
    }

    #[test]
    fn loading_holds_player_commands_but_preserves_operator_control() {
        let host = Host::new(
            Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true),
            false,
        )
        .unwrap();
        let handle = host.handle();
        let (welcome, messages) = peer(&handle, Role::Referee);
        for _ in 0..3 {
            next(&messages);
        }
        handle
            .message(
                welcome.client_id,
                ClientMessage::Command(Command::Step { ticks: 7 }),
            )
            .unwrap();
        let receipt = handle.apply(&Command::Step { ticks: 11 }).unwrap();
        assert_eq!(
            receipt,
            CommandReceipt {
                sequence: 0,
                tick: 0
            }
        );
        assert!(
            matches!(next(&messages).message(), ServerMessage::Rejected { reason } if reason.contains("loading"))
        );
        handle.ready();
        handle
            .message(
                welcome.client_id,
                ClientMessage::Command(Command::Step { ticks: 13 }),
            )
            .unwrap();
        assert_eq!(handle.snapshot().unwrap().tick, 24);
    }

    #[test]
    fn clock_advances_while_requests_keep_arriving() {
        let time = crate::clock::ManualTime::new();
        let host = timed_host(time.source());
        let handle = host.handle();
        handle.apply(&Command::Pause { paused: false }).unwrap();
        handle.start_clock().unwrap();
        let sending = Arc::new(AtomicBool::new(true));
        let flood = {
            let sending = sending.clone();
            let handle = handle.clone();
            thread::spawn(move || {
                while sending.load(Ordering::Relaxed) {
                    // Ordered, blocking queries keep work arriving even when
                    // the owner never observes a receive timeout.
                    if handle.roster().is_err() {
                        break;
                    }
                }
            })
        };
        // Elapsed readings, not the number of checks, decide how far rules run.
        for _ in 0..10 {
            time.advance(Duration::from_nanos(rm_simulator_world::tick_ns()));
        }
        sending.store(false, Ordering::Relaxed);
        flood.join().unwrap();
        settle(&handle);
        assert_eq!(handle.snapshot().unwrap().tick, 10);
        time.advance(Duration::from_nanos(7 * rm_simulator_world::tick_ns()));
        settle(&handle);
        assert_eq!(handle.snapshot().unwrap().tick, 17);
    }

    #[test]
    fn a_held_clock_reading_advances_nothing() {
        let time = crate::clock::ManualTime::new();
        let host = timed_host(time.source());
        let handle = host.handle();
        handle.apply(&Command::Pause { paused: false }).unwrap();
        handle.start_clock().unwrap();
        settle(&handle);
        assert_eq!(handle.snapshot().unwrap().tick, 0);
        // The clock is checked on a fixed 2 ms grid and runs whole ticks only;
        // a partial tick at the end of a reading is dropped, never banked.
        let tick = rm_simulator_world::tick_ns();
        let period = CLOCK_PERIOD.as_nanos() as u64;
        time.advance(Duration::from_nanos(period + 2 * tick));
        settle(&handle);
        assert_eq!(handle.snapshot().unwrap().tick, 2);
        // A small reading inside the next check window runs nothing.
        time.advance(Duration::from_nanos(tick / 4));
        settle(&handle);
        assert_eq!(handle.snapshot().unwrap().tick, 2);
        // Crossing the deadline runs the accumulated whole ticks.
        time.advance(Duration::from_nanos(period + 2 * tick));
        settle(&handle);
        assert_eq!(handle.snapshot().unwrap().tick, 4);
    }

    #[test]
    fn collision_capture_retains_the_state_from_its_worker_turn() {
        let mut field = Field::new(&FieldConfig {
            chassis: vec![rm_simulator_world::ChassisPlacement {
                team: Team::Red,
                kind: rm_simulator_world::RobotKind::Infantry,
                performance: None,
                config: Default::default(),
                spawn: rm_simulator_world::Pose::at([0.0, 0.0, 2.0]),
            }],
            ..Default::default()
        })
        .unwrap();
        field.step(123).unwrap();
        let expected = field.dynamic_geometry_snapshot().into_triangles();
        let snapshot = field.snapshot();
        let host = Host::new(Simulation::new(field, true), true).unwrap();
        let handle = host.handle();
        let pending = handle.try_dynamic_geometry().unwrap();
        handle.apply(&Command::Step { ticks: 100 }).unwrap();
        let capture = pending.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(capture.snapshot, snapshot);
        assert_eq!(capture.geometry.into_triangles(), expected);
        assert_eq!(handle.snapshot().unwrap().tick, 223);
    }

    #[test]
    fn ready_and_capture_requests_do_not_wait_for_a_busy_simulation() {
        let host = host();
        let handle = host.handle();
        let release = handle.stall();
        handle.ready();
        let capture = handle.try_dynamic_geometry().unwrap();
        assert!(matches!(capture.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(release);
        capture.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}
