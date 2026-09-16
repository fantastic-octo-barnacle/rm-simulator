// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! GNS UDP and TCP transports for the authoritative host and clients. TCP readers
//! submit messages to the host mailbox; writer workers send ordered JSON-lines
//! replies, one full snapshot per periodic frame. A slow client whose bounded
//! outbox fills or whose socket stalls past [`WRITE_TIMEOUT`] is dropped.
//! Broadcast encoding is shared by writers.
pub use crate::host::BROADCAST_PERIOD;
#[path = "presentation_clock.rs"]
mod presentation_clock;
use crate::host::{Host, HostHandle, PeerRegistration};
use crate::lifecycle::{ConnectionStop, Listener, Stop};
#[path = "gns_transport.rs"]
mod gns_transport;
/// Bounded per-peer output; only an unsent periodic frame is replaceable.
#[path = "outbox.rs"]
pub(crate) mod outbox;
pub use crate::protocol::describe_seat;
use crate::protocol::{
    ClientMessage, Command, PROTOCOL_VERSION, PlayerInfo, Robot, Role, ServerMessage, Welcome,
    read_message, write_message,
};
use crate::simulation::{Simulation, SimulationState};
use rm_simulator_world::Team;
use std::io::{self, BufReader};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

/// Serialize native GNS tests, including lobby admission, because packet loss
/// and lag configuration affect every GNS socket in this process.
#[cfg(test)]
pub(crate) static NATIVE_TEST: Mutex<()> = Mutex::new(());

/// Gameplay transport. TCP remains available for protocol tools and comparisons.
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    /// Valve GameNetworkingSockets over UDP, the gameplay transport.
    #[default]
    Gns,
    /// Ordered TCP lines, kept for protocol tools and transport comparisons.
    Tcp,
}

/// A client must say hello within this long.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// A peer that accepts no bytes for this long is disconnected.
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Lines queued for one client before it is dropped as unresponsive; the
/// writer coalesces snapshots, so this only fills while a write is stuck.
pub(crate) const OUTBOX_CAPACITY: usize = 256;
/// Pending client commands, never coalesced across aim/fire or match controls.
const CLIENT_COMMAND_CAPACITY: usize = 256;
/// Unread notices and rejections are bounded by both count and UTF-8 bytes.
const CLIENT_NOTICE_CAPACITY: usize = 256;
const CLIENT_NOTICE_BYTES: usize = 1 << 20;

/// An owned host. Dropping it stops its clock, listener and peer workers.
pub struct Server {
    host: Host,
    handle: HostHandle,
    listener: Option<Listener>,
    udp: Option<gns_transport::UdpListener>,
    stop: Stop,
    owner: Mutex<Option<LocalOwner>>,
}

impl Server {
    /// Create a host with no gameplay socket. Only the typed owner connection
    /// and optional operator interfaces can reach it. `ready` releases the
    /// startup hold; call `spawn_clock` separately to enable real-time ticks.
    pub fn in_process(simulation: Simulation, ready: bool) -> io::Result<Self> {
        let host = Host::new(simulation, ready)?;
        let handle = host.handle();
        let stop = host.stop_signal();
        Ok(Self {
            host,
            handle,
            stop,
            listener: None,
            udp: None,
            owner: Mutex::new(None),
        })
    }
    /// Transfer the simulation to its owner worker and start accepting clients.
    /// Start real-time pacing with [`Server::spawn_clock`] or [`Server::run_clock`].
    pub fn bind(addr: impl ToSocketAddrs, simulation: Simulation) -> io::Result<Server> {
        Self::bind_with_readiness(addr, simulation, true)
    }

    /// Prepare a host whose clock will stay held until local scenery is ready.
    pub fn bind_suspended(addr: impl ToSocketAddrs, simulation: Simulation) -> io::Result<Self> {
        Self::bind_with_readiness(addr, simulation, false)
    }

    fn bind_with_readiness(
        addr: impl ToSocketAddrs,
        simulation: Simulation,
        ready: bool,
    ) -> io::Result<Self> {
        let socket = TcpListener::bind(addr)?;
        let host = Host::new(simulation, ready)?;
        let handle = host.handle();
        let stop = host.stop_signal();
        let accepting = handle.clone();
        let listener = Listener::start(socket, stop.clone(), "rm-accept", move |stream| {
            serve_peer(accepting.clone(), stream, None);
        })?;
        Ok(Server {
            host,
            handle,
            listener: Some(listener),
            udp: None,
            stop,
            owner: Mutex::new(None),
        })
    }
    /// Create the privileged local connection in-process. Network hellos cannot
    /// request owner authority or override their server-assigned spawn.
    pub fn connect_owner(
        &self,
        name: &str,
        team: Team,
        role: Role,
        robot: Robot,
        spawn_m: [f64; 3],
        yaw_deg: f64,
    ) -> anyhow::Result<Client> {
        let mut owner = self.owner.lock().unwrap_or_else(|p| p.into_inner());
        anyhow::ensure!(owner.is_none(), "an owner is already connected");
        anyhow::ensure!(!self.stop.wait(Duration::ZERO), "server is stopped");
        let stop = Stop::default();
        let (sender, receiver) = outbox::channel(OUTBOX_CAPACITY);
        let welcome = self
            .handle
            .join(PeerRegistration {
                password: String::new(),
                name: name.into(),
                team: Some(team),
                role,
                robot,
                owner_spawn: Some((spawn_m, yaw_deg)),
                outbox: sender,
                stream: ConnectionStop::Worker(stop.clone()),
            })
            .map_err(anyhow::Error::msg)?;
        let inbox = Arc::new(ClientInbox::for_transport(
            "local",
            crate::clock::TimeSource::system(),
        ));
        let (outbox, commands) = mpsc::sync_channel(CLIENT_COMMAND_CAPACITY);
        // Construct the guard before spawning: failure must release the seat.
        let mut connection = LocalOwner {
            stop: stop.clone(),
            handle: self.handle.clone(),
            id: welcome.client_id,
            workers: Vec::new(),
        };
        let handle = self.handle.clone();
        let stopping = stop.clone();
        let incoming = inbox.clone();
        let id = welcome.client_id;
        connection
            .workers
            .push(
                thread::Builder::new()
                    .name("rm-local-input".into())
                    .spawn(move || {
                        let result = (|| -> Result<(), String> {
                            while !stopping.wait(Duration::ZERO) {
                                let queued: QueuedCommand = match commands
                                    .recv_timeout(Duration::from_millis(2))
                                {
                                    Ok(Some(queued)) => queued,
                                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                };
                                if let Some(command) = queued.command {
                                    let message = ClientMessage::Command(command);
                                    incoming
                                        .observer
                                        .client("dequeue", Some(id), &message, None);
                                    handle.message(id, message)?;
                                }
                                if let Some(nonce) = queued.time_probe {
                                    handle.message(id, ClientMessage::TimeProbe { nonce })?;
                                }
                                if let Some(nonce) = queued.confirmation {
                                    handle.message(id, ClientMessage::Ping { nonce })?;
                                }
                            }
                            Ok(())
                        })();
                        if let Err(reason) = result {
                            incoming.fail(reason);
                        }
                        let _ = handle.leave(id);
                    })?,
            );
        let incoming = inbox.clone();
        let stopping = stop.clone();
        connection
            .workers
            .push(
                thread::Builder::new()
                    .name("rm-local-state".into())
                    .spawn(move || {
                        while let Some(frame) = receiver.recv() {
                            // Typed publication preserves barriers without serialization or sockets.
                            if stopping.wait(Duration::ZERO) {
                                break;
                            }
                            let started = Instant::now();
                            let result = incoming.publish(frame.message().clone());
                            incoming.observer.work("local_publish", started.elapsed());
                            if let Err(error) = result {
                                incoming.fail(error.to_string());
                                break;
                            }
                        }
                        incoming.fail("host closed the local connection".into());
                        stopping.request();
                    })?,
            );
        let client = Client {
            stream: ConnectionStop::Worker(stop),
            outbox,
            inbox,
            welcome,
            latest: None,
            roster: Vec::new(),
            disconnected: None,
            sent_confirmation: 0,
            timing: ClientTiming::default(),
            transport_stats: None,
            host_telemetry: None,
            delivery_stats: None,
            owner_anchor: None,
            acknowledged: 0,
        };
        *owner = Some(connection);
        Ok(client)
    }

    /// Release the startup clock hold without waiting for the simulation worker.
    pub fn ready(&self) {
        self.handle.ready();
    }

    /// A cloneable handle to the hosted simulation, for operators and transports.
    pub fn handle(&self) -> HostHandle {
        self.handle.clone()
    }

    /// A lazy capture job. Invoke on a background worker, never in a frame update.
    pub fn collision_geometry(
        &self,
    ) -> Box<dyn FnOnce() -> Result<rm_simulator_world::StaticGeometry, String> + Send> {
        let handle = self.handle.clone();
        Box::new(move || handle.static_geometry())
    }

    /// Capture moving colliders without making the render thread wait for a tick.
    pub fn request_dynamic_collision_geometry(
        &self,
    ) -> Result<Receiver<crate::host::CollisionCapture>, String> {
        self.handle.try_dynamic_geometry()
    }

    /// The bound gameplay address, or `None` for a socket-free in-process host.
    pub fn listening_addr(&self) -> Option<SocketAddr> {
        self.listener
            .as_ref()
            .map(|listener| listener.local_addr)
            .or_else(|| self.udp.as_ref().map(|listener| listener.local_addr))
    }
    /// The bound address of the active listener, TCP or UDP.
    /// Panics for an in-process host; use `listening_addr` when either mode is possible.
    pub fn local_addr(&self) -> SocketAddr {
        self.listening_addr()
            .expect("in-process host has no gameplay listener")
    }
    /// Everyone connected, in arrival order.
    pub fn roster(&self) -> Vec<PlayerInfo> {
        self.handle.roster().unwrap_or_default()
    }
    /// How many peers are connected, zero if the worker has stopped.
    pub fn peer_count(&self) -> usize {
        self.handle.peer_count().unwrap_or_default()
    }
    /// Send the current field state to every client.
    pub fn broadcast_snapshot(&self) {
        let _ = self.handle.broadcast_snapshot(false);
    }

    /// Start the same clock used by the headless host on an owned worker.
    /// Failures stop the host and are available through [Self::failure].
    pub fn spawn_clock(&self) -> io::Result<()> {
        self.handle.start_clock()
    }

    /// Start the worker's clock and wait until stopped or stepping fails.
    pub fn run_clock(&self) -> anyhow::Result<()> {
        self.handle.start_clock()?;
        self.host.wait().map_err(anyhow::Error::msg)
    }

    /// Request shutdown without waiting for the current physics step.
    pub fn stop(&self) {
        self.host.stop();
        if let Some(owner) = self
            .owner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            owner.stop.request();
        }
    }

    /// The background clock's terminal error, if stepping failed.
    pub fn failure(&self) -> Option<String> {
        self.host.failure()
    }

    /// Stop accepting, disconnect all peers and wait for owned workers.
    /// Call outside gameplay updates; completion waits for in-flight simulation work.
    pub fn shutdown(&mut self) {
        self.stop();
        if let Some(listener) = &mut self.listener {
            listener.shutdown();
        }
        if let Some(listener) = &mut self.udp {
            listener.shutdown();
        }
        self.owner.lock().unwrap_or_else(|p| p.into_inner()).take();
        self.host.shutdown();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Owns the embedded seat and joins its typed delivery workers on server teardown.
struct LocalOwner {
    stop: Stop,
    handle: HostHandle,
    id: u32,
    workers: Vec<thread::JoinHandle<()>>,
}
impl Drop for LocalOwner {
    fn drop(&mut self) {
        self.stop.request();
        let _ = self.handle.leave(self.id);
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

struct PeerWriter {
    stream: TcpStream,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for PeerWriter {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve_peer(handle: HostHandle, stream: TcpStream, owner_spawn: Option<([f64; 3], f64)>) {
    let address = stream
        .peer_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| "?".into());
    if let Err(error) = talk(&handle, stream, owner_spawn) {
        eprintln!("client {address}: {error}");
    }
}

fn talk(
    handle: &HostHandle,
    stream: TcpStream,
    owner_spawn: Option<([f64; 3], f64)>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let (name, wanted_team, role, robot, password) =
        match read_message::<ClientMessage, _>(&mut reader)? {
            Some(ClientMessage::Hello {
                password,
                protocol,
                name,
                team,
                role,
                robot,
                tick_ns,
            }) => {
                if protocol != PROTOCOL_VERSION {
                    let mut stream = stream;
                    write_message(
                        &mut stream,
                        &ServerMessage::Rejected {
                            reason: crate::protocol::version_mismatch(PROTOCOL_VERSION, protocol),
                        },
                    )?;
                    return Ok(());
                }
                // One match runs at one physics rate, so a peer predicting at a
                // different tick length is refused before it holds a seat.
                if tick_ns != rm_simulator_world::tick_ns() {
                    let mut stream = stream;
                    write_message(
                        &mut stream,
                        &ServerMessage::Rejected {
                            reason: crate::protocol::rate_mismatch(
                                rm_simulator_world::tick_ns(),
                                tick_ns,
                            ),
                        },
                    )?;
                    return Ok(());
                }
                (name, team, role, robot, password)
            }
            _ => return Ok(()),
        };
    stream.set_read_timeout(None)?;
    let (sender, inbox) = outbox::channel(OUTBOX_CAPACITY);
    let writer_stream = stream.try_clone()?;
    let shutdown_stream = stream.try_clone()?;
    let peer_stream = stream.try_clone()?;
    writer_stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let writer = thread::Builder::new()
        .name("rm-peer-writer".into())
        .spawn(move || write_loop(writer_stream, inbox))?;
    let _writer = PeerWriter {
        stream: shutdown_stream,
        worker: Some(writer),
    };
    let welcome = match handle.join(PeerRegistration {
        password,
        name,
        team: wanted_team,
        role,
        robot,
        owner_spawn,
        outbox: sender,
        stream: ConnectionStop::Tcp(peer_stream),
    }) {
        Ok(welcome) => welcome,
        Err(reason) => {
            write_message(
                &mut stream.try_clone()?,
                &ServerMessage::Rejected { reason },
            )?;
            return Ok(());
        }
    };
    let id = welcome.client_id;
    let result = command_loop(handle, id, &mut reader);
    let _ = handle.leave(id);
    result
}

fn command_loop(handle: &HostHandle, id: u32, reader: &mut BufReader<TcpStream>) -> io::Result<()> {
    while let Some(message) = read_message::<ClientMessage, _>(reader)? {
        handle
            .message(id, message)
            .map_err(|reason| io::Error::new(io::ErrorKind::ConnectionAborted, reason))?;
    }
    Ok(())
}

/// Drain the outbox to the socket; a failed or timed-out write closes the
/// socket so the reader thread ends too.
fn write_loop(mut stream: TcpStream, inbox: outbox::Receiver) {
    let result = write_all_lines(&mut stream, &inbox);
    if result.is_err() {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn write_all_lines(stream: &mut TcpStream, inbox: &outbox::Receiver) -> io::Result<()> {
    use std::io::Write;
    let observer =
        crate::network_trace::Observer::new("tcp-host", crate::clock::TimeSource::system());
    while let Some(line) = inbox.recv() {
        let started = Instant::now();
        let bytes = line.encoded().as_bytes();
        observer.work("tcp_encode", started.elapsed());
        stream.write_all(bytes)?;
        observer.server("tcp_write", line.message());
        observer.record(crate::network_trace::Event {
            stage: "tcp_bytes",
            kind: "json_line",
            bytes: Some(bytes.len()),
            ..Default::default()
        });
        stream.flush()?;
    }
    Ok(())
}

/// Latest replaceable state and ordered discrete events from the reader.
#[derive(Default)]
struct Incoming {
    transport_stats: Option<crate::network_stats::TransportStats>,
    host_telemetry: Option<crate::network_stats::HostTelemetry>,
    delivery_stats: Option<crate::pacing::QueueStats>,
    owner_anchor: Option<crate::owner_stream::OwnerAnchor>,
    shot_results: Vec<crate::protocol::ShotResult>,
    hits: Vec<(u64, u64, rm_simulator_world::ArmorHit)>,
    scheduled_shots: Vec<(u32, u64, u64)>,
    snapshot: Option<Box<SimulationState>>,
    received_at: Option<Instant>,
    checkpoint_intervals: crate::network_trace::EventSamples,
    time_sample: Option<(u64, u64, bool, Instant)>,
    roster: Option<Vec<PlayerInfo>>,
    notices: Vec<String>,
    notice_bytes: usize,
    failure: Option<String>,
    acknowledged: u64,
}
/// Arrival times are read through `time`, not [`Instant::now`], so a session
/// that injects a clock measures transit on that clock all the way down.
struct ClientInbox {
    transport: &'static str,
    observer: crate::network_trace::Observer,
    data: Mutex<Incoming>,
    changed: Condvar,
    time: crate::clock::TimeSource,
}
impl Default for ClientInbox {
    fn default() -> Self {
        Self::with_time(crate::clock::TimeSource::system())
    }
}
impl ClientInbox {
    fn with_time(time: crate::clock::TimeSource) -> Self {
        Self::for_transport("tcp", time)
    }
    fn for_transport(transport: &'static str, time: crate::clock::TimeSource) -> Self {
        Self {
            transport,
            observer: crate::network_trace::Observer::new(transport, time.clone()),
            data: Mutex::new(Incoming::default()),
            changed: Condvar::new(),
            time,
        }
    }
    /// Reader-side publication. Drop replaced snapshots after releasing the lock.
    fn publish(&self, message: ServerMessage) -> io::Result<()> {
        self.observer.server("publish", &message);
        let message = match message {
            ServerMessage::Rejected { reason } => {
                ServerMessage::Notice(format!("rejected: {reason}"))
            }
            message => message,
        };
        let mut data = self.data.lock().unwrap_or_else(|p| p.into_inner());
        if data.failure.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "connection closed",
            ));
        }
        let replaced = match message {
            ServerMessage::DeliveryStats(stats) => {
                data.delivery_stats = Some(stats);
                None
            }
            ServerMessage::Hit {
                epoch,
                event_id,
                hit,
            } => {
                if data.hits.len() >= CLIENT_NOTICE_CAPACITY {
                    return Err(io::Error::other("too many unread hit events"));
                }
                data.hits.push((epoch, event_id, hit));
                None
            }
            ServerMessage::Telemetry(stats) => {
                data.host_telemetry = Some(stats);
                None
            }
            ServerMessage::ShotScheduled {
                shooter,
                shot_id,
                intended_time_ns,
            } => {
                if data.scheduled_shots.len() >= CLIENT_NOTICE_CAPACITY {
                    return Err(io::Error::other("too many unread shot receipts"));
                }
                data.scheduled_shots
                    .push((shooter, shot_id, intended_time_ns));
                None
            }
            ServerMessage::ShotResult(result) => {
                if data.shot_results.len() >= CLIENT_NOTICE_CAPACITY {
                    return Err(io::Error::other("too many unread shot results"));
                }
                data.shot_results.push(result);
                None
            }
            ServerMessage::Snapshot(snapshot) => {
                let now = self.time.now();
                if let Some(previous) = data.received_at {
                    data.checkpoint_intervals
                        .record(now.saturating_duration_since(previous).as_secs_f64() * 1000.);
                }
                data.received_at = Some(now);
                data.snapshot.replace(snapshot).map(ServerMessage::Snapshot)
            }
            ServerMessage::Roster(roster) => data.roster.replace(roster).map(ServerMessage::Roster),
            ServerMessage::Notice(text) | ServerMessage::Rejected { reason: text } => {
                if data.notices.len() == CLIENT_NOTICE_CAPACITY
                    || text.len() > CLIENT_NOTICE_BYTES.saturating_sub(data.notice_bytes)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "host sent too many unread notices or rejections",
                    ));
                }
                data.notice_bytes += text.len();
                data.notices.push(text);
                None
            }
            ServerMessage::Pong { nonce } => {
                data.acknowledged = data.acknowledged.max(nonce);
                None
            }
            ServerMessage::TimeSample {
                nonce,
                time_ns,
                paused,
            } => {
                data.time_sample = Some((nonce, time_ns, paused, self.time.now()));
                None
            }
            ServerMessage::Welcome(_) => None,
            // The UDP client codec consumes the configuration frame before the
            // inbox sees it; no other transport carries one.
            ServerMessage::OwnerConfig(_) => None,
        };
        drop(data);
        self.changed.notify_one();
        drop(replaced);
        Ok(())
    }
    fn fail(&self, reason: String) {
        self.data
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .failure
            .get_or_insert(reason);
        self.changed.notify_one();
    }
    /// Frame-facing calls never wait for a worker holding the inbox lock.
    fn try_data(&self) -> Option<MutexGuard<'_, Incoming>> {
        match self.data.try_lock() {
            Ok(data) => Some(data),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

/// One queued client-leg item. The writer emits the parts in this order, so a
/// barrier always covers the command queued with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct QueuedCommand {
    /// Command to write first, before any barrier.
    pub(crate) command: Option<Command>,
    /// State barrier nonce. The host answers it with a confirmation snapshot
    /// and then a Pong, after every earlier command has been applied.
    pub(crate) confirmation: Option<u64>,
    /// Timing probe nonce, written between the command and the barrier.
    pub(crate) time_probe: Option<u64>,
}

/// A connection to a host. Socket reads and command writes run on workers;
/// frame-facing methods never wait for I/O. Connect and wait_snapshot are
/// blocking startup operations.
pub struct Client {
    /// Retained for shutdown only; commands are written by a transport worker.
    stream: ConnectionStop,
    timing: ClientTiming,
    transport_stats: Option<crate::network_stats::TransportStats>,
    host_telemetry: Option<crate::network_stats::HostTelemetry>,
    delivery_stats: Option<crate::pacing::QueueStats>,
    owner_anchor: Option<crate::owner_stream::OwnerAnchor>,
    outbox: SyncSender<Option<QueuedCommand>>,
    inbox: Arc<ClientInbox>,
    welcome: Welcome,
    latest: Option<SimulationState>,
    roster: Vec<PlayerInfo>,
    disconnected: Option<String>,
    sent_confirmation: u64,
    acknowledged: u64,
}

impl Client {
    /// Connect, say hello as the default robot and wait for the welcome. Run
    /// this off the UI thread.
    pub fn connect(
        addr: impl ToSocketAddrs,
        name: &str,
        team: Option<Team>,
        role: Role,
    ) -> anyhow::Result<Client> {
        Self::connect_with_password(addr, name, team, role, Robot::default(), "")
    }

    /// Connect, say hello naming the robot to drive and a lobby password, and
    /// wait for the welcome. Run this off the UI thread.
    pub fn connect_with_password(
        addr: impl ToSocketAddrs,
        name: &str,
        team: Option<Team>,
        role: Role,
        robot: Robot,
        password: &str,
    ) -> anyhow::Result<Client> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| anyhow::anyhow!("no host address"))?;
        Self::from_stream_with_password(
            TcpStream::connect_timeout(&addr, HELLO_TIMEOUT)?,
            name,
            team,
            role,
            robot,
            password,
        )
    }

    fn from_stream_with_password(
        stream: TcpStream,
        name: &str,
        team: Option<Team>,
        role: Role,
        robot: Robot,
        password: &str,
    ) -> anyhow::Result<Client> {
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
        let mut writer = stream.try_clone()?;
        write_message(
            &mut writer,
            &ClientMessage::Hello {
                password: password.into(),
                protocol: PROTOCOL_VERSION,
                name: name.to_string(),
                team,
                role,
                robot,
                tick_ns: rm_simulator_world::tick_ns(),
            },
        )?;
        stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let welcome = match read_message::<ServerMessage, _>(&mut reader)? {
            Some(ServerMessage::Welcome(welcome)) => *welcome,
            Some(ServerMessage::Rejected { reason }) => anyhow::bail!("host refused: {reason}"),
            other => anyhow::bail!("host did not answer the hello: {other:?}"),
        };
        stream.set_read_timeout(None)?;
        let (outbox, commands) = mpsc::sync_channel(CLIENT_COMMAND_CAPACITY);
        let inbox = Arc::new(ClientInbox::default());
        // Construct the shutdown guard before spawning either worker: a failed
        // spawn closes the socket and releases any worker already started.
        let client = Client {
            stream: ConnectionStop::Tcp(stream),
            outbox,
            inbox: inbox.clone(),
            welcome,
            latest: None,
            roster: Vec::new(),
            disconnected: None,
            sent_confirmation: 0,
            timing: ClientTiming::default(),
            transport_stats: None,
            host_telemetry: None,
            delivery_stats: None,
            owner_anchor: None,
            acknowledged: 0,
        };
        let failures = inbox.clone();
        thread::Builder::new()
            .name("rm-client-writer".into())
            .spawn(move || {
                if let Err(error) =
                    write_client_commands(&mut writer, commands, Some(&failures.observer))
                {
                    failures.fail(format!("sending commands: {error}"));
                }
                let _ = writer.shutdown(Shutdown::Both);
            })?;
        let stop_writer = client.outbox.clone();
        thread::Builder::new()
            .name("rm-client-reader".into())
            .spawn(move || {
                read_loop(&mut reader, &inbox);
                let _ = reader.get_ref().shutdown(Shutdown::Both);
                // Wake an idle writer; a full queue means it is already awake
                // and the closed socket will terminate its next write.
                let _ = stop_writer.try_send(None);
            })?;
        Ok(client)
    }
    /// The seat the host granted in answer to this client's hello.
    pub fn welcome(&self) -> &Welcome {
        &self.welcome
    }
    /// Queue one command in order. Success means queued, not acknowledged by
    /// the host. A full queue disconnects explicitly instead of dropping commands.
    pub fn send(&mut self, command: Command) -> io::Result<()> {
        self.queue(QueuedCommand {
            command: Some(command),
            confirmation: None,
            time_probe: None,
        })
    }

    /// Queue a command followed by a state barrier. Poll until confirmed before
    /// capturing a frame that must include this command's result or rejection.
    pub fn send_confirmed(&mut self, command: Command) -> io::Result<()> {
        let nonce = self
            .sent_confirmation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("command confirmation overflow"))?;
        self.queue(QueuedCommand {
            command: Some(command),
            confirmation: Some(nonce),
            time_probe: None,
        })?;
        self.sent_confirmation = nonce;
        Ok(())
    }

    /// Request a snapshot barrier after all previously queued commands.
    pub fn confirm(&mut self) -> io::Result<()> {
        let nonce = self
            .sent_confirmation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("command confirmation overflow"))?;
        self.queue(QueuedCommand {
            command: None,
            confirmation: Some(nonce),
            time_probe: None,
        })?;
        self.sent_confirmation = nonce;
        Ok(())
    }

    /// Whether the newest barrier queued by [`Client::send_confirmed`] or
    /// [`Client::confirm`] has been answered. [`Client::poll`] updates it.
    pub fn commands_confirmed(&self) -> bool {
        self.acknowledged >= self.sent_confirmation
    }

    fn queue(&mut self, queued: QueuedCommand) -> io::Result<()> {
        if let Some(data) = self.inbox.try_data()
            && let Some(failure) = &data.failure
        {
            self.disconnected.get_or_insert_with(|| failure.clone());
        }
        if let Some(reason) = &self.disconnected {
            return Err(io::Error::new(io::ErrorKind::NotConnected, reason.clone()));
        }
        if let Some(command) = queued.command {
            self.inbox.observer.client(
                "enqueue_attempt",
                Some(self.welcome.client_id),
                &ClientMessage::Command(command),
                None,
            );
        }
        let error = match self.outbox.try_send(Some(queued)) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(_)) => io::Error::new(
                io::ErrorKind::WouldBlock,
                "connection too slow: outgoing command queue is full",
            ),
            Err(TrySendError::Disconnected(_)) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "command writer stopped")
            }
        };
        self.disconnected = Some(error.to_string());
        let _ = self.stream.shutdown(Shutdown::Both);
        Err(error)
    }
    /// Take the newest snapshot and roster, with bounded work per frame.
    pub fn poll(&mut self) {
        let Some(mut data) = self.inbox.try_data() else {
            return;
        };
        self.acknowledged = data.acknowledged;
        if let Some(stats) = data.delivery_stats.take() {
            self.delivery_stats = Some(stats);
        }
        if let Some(stats) = data.host_telemetry.take() {
            let initial = self.initial_input_lead_ns();
            self.timing.input_lead.observe(stats, initial);
            self.host_telemetry = Some(stats);
        }
        if let Some(stats) = data.transport_stats.take() {
            self.transport_stats = Some(stats);
        }
        let owner_anchor = data.owner_anchor.take();
        let received_at = data.received_at;
        let time_sample = data.time_sample.take();
        let snapshot = data.snapshot.take();
        let roster = data.roster.take();
        let failure = data.failure.clone();
        drop(data);
        if let Some(snapshot) = snapshot {
            self.inbox.observer.record(crate::network_trace::Event {
                stage: "consume",
                kind: "snapshot",
                snapshot: Some(snapshot.snapshot_id),
                simulation_ns: Some(snapshot.field.time_ns),
                ..Default::default()
            });
            let at = received_at.unwrap_or_else(|| self.timing.now());
            self.timing.observe(&snapshot, at);
            self.latest = Some(*snapshot);
        }
        if let Some(anchor) = owner_anchor {
            self.timing.clock.observe(
                anchor.input_epoch,
                self.timing.elapsed_ns(),
                anchor.time_ns,
                anchor.paused,
            );
            self.owner_anchor = Some(anchor);
        }
        if let Some(roster) = roster {
            self.roster = roster;
        }
        if let Some(failure) = failure {
            self.disconnected.get_or_insert(failure);
        }
        if let Some(sample) = time_sample {
            self.timing.sample(sample);
        }
    }
    /// Drain scheduled shot receipts as `(shooter chassis id, shot id, intended
    /// simulation time in ns)`, oldest first. A receipt means admitted, not
    /// executed.
    pub fn take_scheduled_shots(&self) -> Vec<(u32, u64, u64)> {
        self.inbox.try_data().map_or_else(Vec::new, |mut data| {
            std::mem::take(&mut data.scheduled_shots)
        })
    }
    /// Latest host downstream queue report, absent for TCP and embedded peers.
    pub fn delivery_stats(&self) -> Option<&crate::pacing::QueueStats> {
        self.delivery_stats.as_ref()
    }
    /// Drain reliable contact events as `(epoch, event id, scored contact)` in
    /// host order. Snapshot coalescing never replaces these events.
    pub fn take_hits(&self) -> Vec<(u64, u64, rm_simulator_world::ArmorHit)> {
        self.inbox
            .try_data()
            .map_or_else(Vec::new, |mut data| std::mem::take(&mut data.hits))
    }
    /// Drain executed or rejected shot results, oldest first.
    pub fn take_shot_results(&self) -> Vec<crate::protocol::ShotResult> {
        self.inbox
            .try_data()
            .map_or_else(Vec::new, |mut data| std::mem::take(&mut data.shot_results))
    }
    /// Re-anchor presentation after local loading; loading time is not simulation time.
    pub fn reset_presentation_clock(&mut self, time_ns: u64, paused: bool) {
        self.timing.clock = Default::default();
        self.timing.clock.observe(
            self.latest.as_ref().map_or(0, |s| s.input_epoch),
            self.timing.elapsed_ns(),
            time_ns,
            paused,
        );
    }
    /// Take the newest owner anchor, if one arrived since the last call.
    pub fn take_owner_anchor(&mut self) -> Option<crate::owner_stream::OwnerAnchor> {
        self.owner_anchor.take()
    }
    /// The newest host input-timing telemetry, sampled about once per second.
    pub fn host_telemetry(&self) -> Option<crate::network_stats::HostTelemetry> {
        self.host_telemetry
    }
    /// A local diagnostics snapshot for the network overlay. Native transport
    /// counters appear only when the connection reports them.
    pub fn network_stats(&self) -> crate::network_stats::NetworkStats {
        let elapsed = self.timing.elapsed_ns() as f64 / 1e6;
        crate::network_stats::NetworkStats {
            schema_version: 1,
            connection_generation: u64::from(self.welcome.client_id),
            transport: self.inbox.transport.into(),
            sampled_elapsed_ms: elapsed,
            stale: self.disconnected.is_some(),
            native_sample_age_ms: self
                .transport_stats
                .as_ref()
                .map(|s| (elapsed - s.sampled_elapsed_ms).max(0.)),
            native: self.transport_stats.clone(),
            native_rate_source: "GNS real-time estimate; provider-defined window and byte accounting",
            loss_percent: None,
            loss_unavailable_reason: "wrapper quality score is not packet loss",
            app_rtt_ms: self.round_trip_ns().map(|n| n as f64 / 1e6),
            pending_response_age_ms: self
                .timing
                .pending
                .map(|(_, at)| at.elapsed().as_secs_f64() * 1000.),
        }
    }
    /// Cumulative local message/byte counters and optional disk trace status.
    /// Returns `None` while a transport worker holds the diagnostics lock.
    pub fn trace_report(&self) -> Option<crate::network_trace::Report> {
        self.inbox.observer.report()
    }
    /// The latest reliable probe round trip in ns, or `None` before one returns.
    pub fn round_trip_ns(&self) -> Option<u64> {
        self.timing.rtts.back().copied()
    }
    /// Bounded individual probe round trips in ms, identified independently of value.
    pub fn round_trip_samples(&self) -> &crate::network_trace::EventSamples {
        &self.timing.rtt_samples
    }
    /// Intervals in ms between decoded complete checkpoint arrivals, before inbox
    /// coalescing. Returns `None` instead of waiting for a transport worker's lock.
    pub fn checkpoint_interval_samples(&self) -> Option<crate::network_trace::EventSamples> {
        self.inbox
            .data
            .try_lock()
            .ok()
            .map(|data| data.checkpoint_intervals.clone())
    }
    /// Includes the lower bound from an unanswered probe, useful during upstream gaps.
    pub fn response_delay_ns(&self) -> Option<u64> {
        self.round_trip_ns()
            .into_iter()
            .chain(
                self.timing
                    .pending
                    .map(|(_, sent)| sent.elapsed().as_nanos().min(u64::MAX as u128) as u64),
            )
            .max()
    }
    /// Read every wall clock through `time`. A session sets this once, before
    /// its first snapshot; a test clock then paces probes and expiry exactly.
    /// Discards the estimate built so far, so this is a construction-time call.
    pub fn set_time_source(&mut self, time: crate::clock::TimeSource) {
        self.timing = ClientTiming::new(time);
    }
    /// Reset arrival feedback across control epochs and robot placements.
    pub fn reset_input_timing(&mut self, epoch: u64, revision: u64) {
        self.timing.input_lead.reset(epoch, revision);
    }
    /// Bounded lead from host arrival feedback, seeded from the timing probe.
    pub fn input_lead_ns(&self) -> u64 {
        self.timing.input_lead.get(self.initial_input_lead_ns())
    }
    fn initial_input_lead_ns(&self) -> u64 {
        // Reliable probe retries include loss recovery. Do not turn a single
        // retransmission spike into extra delay for every scheduled control.
        (self.timing.rtts.iter().copied().min().unwrap_or(0) / 2 + 32_000_000).min(150_000_000)
    }
    /// The estimated simulation time in ns at the current local reading. The
    /// estimate never advances host physics.
    pub fn presentation_time_ns(&self) -> u64 {
        self.timing.clock.at(self.timing.elapsed_ns())
    }
    /// The timing stamp for a shot fired now: local elapsed ns and estimated
    /// simulation time in ns. Observed pose fields stay unset on this path.
    pub fn fire_timing(&self) -> crate::protocol::FireTiming {
        crate::protocol::FireTiming {
            client_elapsed_ns: self.timing.elapsed_ns(),
            estimated_simulation_time_ns: self.presentation_time_ns(),
            observed_snapshot_time_ns: None,
            observed_chassis_pose: None,
            observed_muzzle_pose: None,
        }
    }
    /// Send at most one timing probe per second, with one outstanding and a 3 s timeout.
    pub fn synchronize_clock(&mut self) -> io::Result<()> {
        let now = self.timing.now();
        if now.duration_since(self.timing.last_probe) < Duration::from_secs(1) {
            return Ok(());
        }
        if let Some((_, sent)) = self.timing.pending
            && now.duration_since(sent) < Duration::from_secs(3)
        {
            return Ok(());
        }
        let nonce = self.timing.elapsed_ns();
        self.queue(QueuedCommand {
            command: None,
            confirmation: None,
            time_probe: Some(nonce),
        })?;
        self.timing.pending = Some((nonce, now));
        self.timing.last_probe = now;
        Ok(())
    }
    /// Block until a newly received snapshot arrives, the connection ends,
    /// or the deadline passes. Used only during background session startup.
    /// The deadline is real time even under a test clock: this call parks on
    /// the reader thread, which a held clock would never release.
    pub fn wait_snapshot(&mut self, timeout: Duration) -> Option<&SimulationState> {
        if self.disconnected.is_some() {
            return None;
        }
        let deadline = Instant::now() + timeout;
        let mut data = self.inbox.data.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if data.snapshot.is_some() || data.failure.is_some() {
                let owner_anchor = data.owner_anchor.take();
                let received_at = data.received_at;
                let snapshot = data.snapshot.take();
                let roster = data.roster.take();
                let failure = data.failure.clone();
                drop(data);
                if let Some(anchor) = owner_anchor {
                    self.timing.clock.observe(
                        anchor.input_epoch,
                        self.timing.elapsed_ns(),
                        anchor.time_ns,
                        anchor.paused,
                    );
                    self.owner_anchor = Some(anchor);
                }
                if let Some(roster) = roster {
                    self.roster = roster;
                }
                if let Some(failure) = failure {
                    self.disconnected.get_or_insert(failure);
                    return None;
                }
                if let Some(state) = &snapshot {
                    let at = received_at.unwrap_or_else(|| self.timing.now());
                    self.timing.observe(state, at);
                }
                self.latest = snapshot.map(|snapshot| *snapshot);
                return self.latest.as_ref();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let (next, _) = self
                .inbox
                .changed
                .wait_timeout(data, remaining)
                .unwrap_or_else(|p| p.into_inner());
            data = next;
        }
    }
    /// The newest state received, if any.
    pub fn state(&self) -> Option<&SimulationState> {
        self.latest.as_ref()
    }
    /// Take the newest received state, leaving none behind.
    pub fn take_state(&mut self) -> Option<SimulationState> {
        self.latest.take()
    }
    /// Everyone the host lists as connected, as of the last poll.
    pub fn roster(&self) -> &[PlayerInfo] {
        &self.roster
    }
    /// Drain ordered notices and rejections; if publication is in progress,
    /// leave them for the next call instead of waiting on the reader.
    pub fn take_notices(&mut self) -> Vec<String> {
        let Some(mut data) = self.inbox.try_data() else {
            return Vec::new();
        };
        data.notice_bytes = 0;
        std::mem::take(&mut data.notices)
    }
    /// Why the connection ended, once it has.
    pub fn disconnected(&self) -> Option<&str> {
        self.disconnected.as_deref()
    }
}

impl Drop for Client {
    /// Closing the socket ends the reader and interrupts any blocked write.
    /// The reader also wakes the writer if it is waiting for commands.
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// A client's transport leg over a datagram lane the caller drives.
///
/// The GNS worker runs the same client codec over a
/// socket and a thread. This one has neither: `deliver` hands it a datagram,
/// `pump` decodes what arrived and encodes what the session queued, and
/// `take_outgoing` returns what the carrier should transmit. Every deadline
/// comes from `time`, so a scripted link and a held clock reproduce a session
/// exactly.
pub struct ClientLeg {
    codec: crate::udp_codec::ClientCodec,
    inbox: Arc<ClientInbox>,
    sender: Option<SyncSender<Option<QueuedCommand>>>,
    commands: Receiver<Option<QueuedCommand>>,
    welcome: Option<Welcome>,
    incoming: std::collections::VecDeque<Vec<u8>>,
    outgoing: Vec<crate::pacing::Datagram>,
    time: crate::clock::TimeSource,
}
impl ClientLeg {
    /// Open a leg and queue its hello. `rate_bytes_per_s` is the upstream pacing
    /// budget the GNS client reads from `RM_NET_UP_KIB_S`.
    pub fn new(
        name: &str,
        team: Option<Team>,
        role: Role,
        time: crate::clock::TimeSource,
        rate_bytes_per_s: u32,
    ) -> io::Result<Self> {
        let (sender, commands) = mpsc::sync_channel(CLIENT_COMMAND_CAPACITY);
        Ok(Self {
            codec: crate::udp_codec::ClientCodec::new(
                time.now(),
                rate_bytes_per_s,
                crate::udp_codec::MAX_INPUT_FRAMES,
            ),
            inbox: Arc::new(ClientInbox::for_transport("gns", time.clone())),
            sender: Some(sender),
            commands,
            welcome: None,
            incoming: std::collections::VecDeque::new(),
            outgoing: vec![crate::pacing::Datagram {
                bytes: crate::udp_codec::ClientCodec::hello(name, team, role)?,
                reliable: true,
            }],
            time,
        })
    }
    /// Hand the leg one datagram from the carrier. It is decoded by `pump`.
    pub fn deliver(&mut self, payload: Vec<u8>) {
        self.incoming.push_back(payload);
    }
    /// Decode everything delivered, encode everything the session queued, and
    /// pace out the result. A decoding failure closes the connection, exactly
    /// as it does on the socket worker.
    pub fn pump(&mut self) -> io::Result<()> {
        let now = self.time.now();
        let result = (|| -> io::Result<()> {
            while let Some(payload) = self.incoming.pop_front() {
                match self.codec.receive(&payload, now)? {
                    Some(crate::udp_codec::ClientEvent::Welcome(welcome)) => {
                        self.welcome = Some(*welcome)
                    }
                    Some(crate::udp_codec::ClientEvent::Anchor(anchor)) => {
                        self.inbox
                            .data
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .owner_anchor = Some(*anchor)
                    }
                    Some(crate::udp_codec::ClientEvent::Message(message)) => {
                        self.inbox.publish(message)?
                    }
                    None => {}
                }
            }
            self.codec.acknowledge(now)?;
            loop {
                match self.commands.try_recv() {
                    Ok(Some(queued)) => self.codec.submit(queued, now)?,
                    Ok(None) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => break,
                }
            }
            while let Some(packet) = self.codec.next(now)? {
                self.outgoing.push(packet);
            }
            Ok(())
        })();
        if let Err(error) = &result {
            self.inbox.fail(error.to_string());
        }
        result
    }
    /// Drain the datagrams the carrier should transmit, in pacing order.
    pub fn take_outgoing(&mut self) -> Vec<crate::pacing::Datagram> {
        std::mem::take(&mut self.outgoing)
    }
    /// The seat the host granted, once it has answered this leg's hello.
    pub fn welcome(&self) -> Option<&Welcome> {
        self.welcome.as_ref()
    }
    /// A checkpoint is waiting, so [`Client::wait_snapshot`] returns without parking.
    pub fn snapshot_ready(&self) -> bool {
        self.inbox
            .data
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .snapshot
            .is_some()
    }
    /// Delivery counters, including the pinned-baseline count a delta decoder
    /// must never grow past two.
    pub fn transport_stats(&self) -> crate::network_stats::TransportStats {
        self.codec.stats()
    }
    /// How many acknowledged delta baselines the decoder retains. It must never
    /// grow past two.
    pub fn pinned_baselines(&self) -> usize {
        self.codec.pinned_baselines()
    }
}

impl Client {
    /// Build a client over `leg`, once its welcome has arrived. The leg keeps
    /// carrying datagrams; this only takes the command queue and the inbox.
    /// Public because a harness outside this crate drives both halves.
    pub fn over_link(leg: &mut ClientLeg, time: crate::clock::TimeSource) -> io::Result<Client> {
        let welcome = leg
            .welcome
            .clone()
            .ok_or_else(|| io::Error::other("the host has not welcomed this leg yet"))?;
        let outbox = leg
            .sender
            .take()
            .ok_or_else(|| io::Error::other("this leg already has a client"))?;
        Ok(Client {
            stream: ConnectionStop::Worker(Stop::default()),
            timing: ClientTiming::new(time),
            transport_stats: None,
            host_telemetry: None,
            delivery_stats: None,
            owner_anchor: None,
            outbox,
            inbox: leg.inbox.clone(),
            welcome,
            latest: None,
            roster: Vec::new(),
            disconnected: None,
            sent_confirmation: 0,
            acknowledged: 0,
        })
    }
}

fn write_client_commands(
    writer: &mut impl io::Write,
    commands: Receiver<Option<QueuedCommand>>,
    observer: Option<&crate::network_trace::Observer>,
) -> io::Result<()> {
    while let Ok(Some(queued)) = commands.recv() {
        if let Some(command) = queued.command {
            let message = ClientMessage::Command(command);
            if let Some(observer) = observer {
                observer.client("dequeue", None, &message, None);
            }
            write_message(writer, &message)?;
        }
        if let Some(nonce) = queued.time_probe {
            write_message(writer, &ClientMessage::TimeProbe { nonce })?;
        }
        if let Some(nonce) = queued.confirmation {
            write_message(writer, &ClientMessage::Ping { nonce })?;
        }
    }
    Ok(())
}

fn read_loop(reader: &mut BufReader<TcpStream>, inbox: &ClientInbox) {
    loop {
        let result = match read_message::<ServerMessage, _>(reader) {
            Ok(Some(message)) => inbox.publish(message),
            Ok(None) => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "host closed the connection",
            )),
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            inbox.fail(error.to_string());
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::ChassisSpawner;
    use rm_simulator_world::{
        ChassisCommand, ChassisConfig, Field, FieldConfig, MatchPhase, RefereeCommand,
        RefereeConfig,
    };

    #[test]
    fn link_client_reports_gns_transport() {
        let time = crate::clock::ManualTime::new();
        let leg = ClientLeg::new("pilot", None, Role::Spectator, time.source(), 65536).unwrap();
        assert_eq!(leg.inbox.transport, "gns");
    }

    #[test]
    fn checkpoint_arrival_samples_survive_inbox_coalescing() {
        let clock = crate::clock::ManualTime::new();
        let inbox = ClientInbox::with_time(clock.source());
        let state = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true).state();
        for delay in [0, 32, 64] {
            clock.advance_ms(delay);
            inbox
                .publish(ServerMessage::Snapshot(Box::new(state.clone())))
                .unwrap();
        }
        let data = inbox.data.lock().unwrap();
        let samples = serde_json::to_value(&data.checkpoint_intervals).unwrap();
        assert_eq!(samples["total"], 2);
        assert_eq!(samples["values"], serde_json::json!([[1, 32.], [2, 64.]]));
    }

    #[test]
    fn probe_samples_count_equal_rtts_once_per_answer() {
        let clock = crate::clock::ManualTime::new();
        let mut timing = ClientTiming::new(clock.source());
        for nonce in [1, 2] {
            timing.pending = Some((nonce, clock.now()));
            clock.advance_ms(5);
            let reply = (nonce, 0, true, clock.now());
            timing.sample(reply);
            timing.sample(reply);
        }
        let samples = serde_json::to_value(&timing.rtt_samples).unwrap();
        assert_eq!(samples["values"], serde_json::json!([[1, 5.], [2, 5.]]));
    }

    fn host() -> (Server, HostHandle) {
        let config = FieldConfig {
            referee: Some(RefereeConfig::alternating(1, 2)),
            ..FieldConfig::default()
        };
        let field = Field::new(&config).unwrap();
        let simulation = Simulation::new(field, true).with_spawner(ChassisSpawner {
            config: ChassisConfig::default(),
            terrain: None,
        });
        let server = Server::bind("127.0.0.1:0", simulation).unwrap();
        let handle = server.handle();
        (server, handle)
    }
    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "{what}");
            thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn in_process_owner_preserves_barriers_without_sockets_or_serialization() {
        let mut server = Server::in_process(
            Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true).with_spawner(
                ChassisSpawner {
                    config: ChassisConfig::default(),
                    terrain: None,
                },
            ),
            true,
        )
        .unwrap();
        assert!(server.listening_addr().is_none());
        let handle = server.handle();
        let mut client = server
            .connect_owner(
                "local",
                Team::Red,
                Role::Pilot,
                Robot::default(),
                [0., 0., 0.5],
                0.,
            )
            .unwrap();
        assert!(matches!(client.stream, ConnectionStop::Worker(_)));
        assert_eq!(client.network_stats().transport, "local");
        client.send_confirmed(Command::Step { ticks: 17 }).unwrap();
        wait_until("typed confirmation", || {
            client.poll();
            client.commands_confirmed()
        });
        assert_eq!(client.state().unwrap().field.tick, 17);
        assert_eq!(
            client.trace_report().unwrap().stages["publish"]["snapshot"].bytes,
            None
        );
        drop(client);
        wait_until("local seat removed", || handle.peer_count().unwrap() == 0);
        assert!(handle.snapshot().unwrap().chassis.is_empty());
        server.shutdown();
    }

    #[test]
    fn scheduled_receipt_does_not_confirm_execution_and_batch_keeps_newest() {
        let (server, handle) = host();
        handle.apply(&Command::Pause { paused: false }).unwrap();
        let mut client =
            Client::connect(server.local_addr(), "pilot", Some(Team::Red), Role::Pilot).unwrap();
        let shooter = client.welcome().chassis.as_ref().unwrap().id;
        let state = handle.state().unwrap();
        let input = crate::input_stream::InputFrame {
            input_epoch: state.input_epoch,
            sequence: 1,
            sampled_time_ns: 64_000_000,
            duration_ticks: 16,
            placement_revision: state.field.chassis[0].placement_revision,
            command: Default::default(),
        };
        let shot = Command::FireAimed {
            shooter,
            shot_id: 1,
            input,
            timing: None,
        };
        client.send(shot).unwrap();
        client.confirm().unwrap();
        wait_until("scheduled shot confirmation", || {
            client.poll();
            client.commands_confirmed()
        });
        assert_eq!(
            client.take_scheduled_shots(),
            vec![(shooter, 1, 64_000_000)]
        );
        assert!(client.take_shot_results().is_empty());
        assert_eq!(client.state().unwrap().field.shots_fired, 0);
        let inputs = (1..=4)
            .map(|sequence| Command::PilotInput {
                chassis: shooter,
                frame: crate::input_stream::InputFrame {
                    sequence,
                    sampled_time_ns: 0,
                    ..input
                },
            })
            .collect();
        handle
            .pilot_batch(client.welcome().client_id, inputs)
            .unwrap();
        handle.apply(&Command::Step { ticks: 65 }).unwrap();
        client.send(shot).unwrap();
        client.confirm().unwrap();
        wait_until("executed shot confirmation", || {
            client.poll();
            client.commands_confirmed()
        });
        let results = client.take_shot_results();
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|r| r.executed_time_ns == Some(64_000_000) && r.result.is_ok())
        );
        assert_eq!(client.state().unwrap().field.shots_fired, 1);
        assert_eq!(client.state().unwrap().shot_results.len(), 1);
    }

    #[test]
    fn background_clock_steps_broadcasts_and_stops_without_a_render_loop() {
        let (mut server, simulation) = host();
        let mut client =
            Client::connect(server.local_addr(), "referee", None, Role::Referee).unwrap();
        server.spawn_clock().unwrap();
        assert_eq!(
            server.spawn_clock().unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(server.run_clock().is_err());
        assert_eq!(
            client
                .wait_snapshot(Duration::from_secs(5))
                .unwrap()
                .field
                .tick,
            0
        );
        client.send(Command::Step { ticks: 16 }).unwrap();
        wait_until("paused step did not arrive", || {
            client.poll();
            client
                .state()
                .is_some_and(|s| s.field.tick == 16 && s.paused)
        });
        client.send(Command::Pause { paused: false }).unwrap();
        wait_until("clock did not advance", || {
            client.poll();
            client.state().is_some_and(|s| s.field.tick > 16)
        });
        server.shutdown();
        let final_tick = simulation.snapshot().unwrap().tick;
        assert!(server.failure().is_none());
        server.shutdown();
        assert_eq!(simulation.snapshot().unwrap().tick, final_tick);
        assert_eq!(
            server.spawn_clock().unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        wait_until("client did not see shutdown", || {
            client.poll();
            client.disconnected().is_some()
        });
    }

    #[test]
    fn dropping_host_disconnects_pilots_releases_port_and_removes_chassis() {
        let (server, simulation) = host();
        let address = server.local_addr();
        let mut client = Client::connect(address, "pilot", None, Role::Pilot).unwrap();
        let _unfinished_hello = TcpStream::connect(address).unwrap();
        server.spawn_clock().unwrap();
        assert!(!simulation.snapshot().unwrap().chassis.is_empty());
        drop(server);
        assert!(simulation.snapshot().unwrap().chassis.is_empty());
        let _rebound = TcpListener::bind(address).expect("listener port was retained");
        wait_until("pilot stayed connected", || {
            client.poll();
            client.disconnected().is_some()
        });
    }

    #[test]
    fn blocking_clock_uses_the_same_stop_signal() {
        let (server, simulation) = host();
        simulation.apply(&Command::Pause { paused: false }).unwrap();
        let server = Arc::new(server);
        let clock = server.clone();
        let worker = thread::spawn(move || clock.run_clock());
        wait_until("blocking clock did not advance", || {
            simulation.snapshot().unwrap().tick > 0
        });
        server.stop();
        worker.join().unwrap().unwrap();
        Arc::try_unwrap(server).ok().unwrap().shutdown();
    }

    #[test]
    fn clock_failure_is_reported_and_disconnects_clients() {
        let mut field = Field::new(&FieldConfig {
            runes: Vec::new(),
            outposts: Vec::new(),
            ..FieldConfig::default()
        })
        .unwrap();
        field
            .step(u64::MAX / rm_simulator_world::tick_ns())
            .unwrap();
        let simulation = Simulation::new(field, false);
        let mut server = Server::bind("127.0.0.1:0", simulation).unwrap();
        let mut client =
            Client::connect(server.local_addr(), "watcher", None, Role::Spectator).unwrap();
        server.spawn_clock().unwrap();
        wait_until("clock error was not reported", || {
            server.failure().is_some()
        });
        assert!(server.failure().unwrap().contains("overflow"));
        wait_until("failed host stayed connected", || {
            client.poll();
            client.disconnected().is_some()
        });
        server.shutdown();
    }

    #[test]
    fn placement_is_owner_only_and_barriers_confirm_plain_commands() {
        let (server, simulation) = host();
        let mut owner = server
            .connect_owner(
                "owner",
                Team::Red,
                Role::Pilot,
                Robot::default(),
                [2.0, 3.0, 1.0],
                0.0,
            )
            .unwrap();
        let mut guest = Client::connect(server.local_addr(), "guest", None, Role::Pilot).unwrap();
        let own_id = owner.welcome().chassis.as_ref().unwrap().id;
        let guest_id = guest.welcome().chassis.as_ref().unwrap().id;
        guest
            .send_confirmed(Command::PlaceChassis {
                chassis: guest_id,
                position_m: [20.0, 0.0, 1.0],
                yaw_deg: 0.0,
            })
            .unwrap();
        owner
            .send_confirmed(Command::PlaceChassis {
                chassis: guest_id,
                position_m: [30.0, 0.0, 1.0],
                yaw_deg: 0.0,
            })
            .unwrap();
        wait_until("placement rejections", || {
            owner.poll();
            guest.poll();
            owner.commands_confirmed() && guest.commands_confirmed()
        });
        assert!(owner.take_notices().iter().any(|n| n.contains("not yours")));
        assert!(
            guest
                .take_notices()
                .iter()
                .any(|n| n.contains("embedded owner"))
        );
        owner
            .send(Command::PlaceChassis {
                chassis: own_id,
                position_m: [4.0, 5.0, 1.0],
                yaw_deg: 90.0,
            })
            .unwrap();
        owner.confirm().unwrap();
        assert!(!owner.commands_confirmed());
        wait_until("placement confirmation", || {
            owner.poll();
            owner.commands_confirmed()
        });
        let snapshot = owner.take_state().unwrap().field;
        assert_eq!(
            &snapshot
                .chassis
                .iter()
                .find(|c| c.id == own_id)
                .unwrap()
                .pose
                .translation_m[..2],
            &[4.0, 5.0]
        );
        assert_eq!(simulation.snapshot().unwrap().chassis.len(), 2);
    }
    #[test]
    fn owner_uses_custom_spawn_and_authority_cannot_be_claimed_over_tcp() {
        let (server, simulation) = host();
        let mut owner = server
            .connect_owner(
                "owner",
                Team::Red,
                Role::Pilot,
                Robot::default(),
                [2.0, 3.0, 1.0],
                90.0,
            )
            .unwrap();
        let id = owner.welcome().chassis.as_ref().unwrap().id;
        let mut guest =
            Client::connect(server.local_addr(), "owner", Some(Team::Red), Role::Pilot).unwrap();
        let snapshot = simulation.snapshot().unwrap();
        assert_eq!(snapshot.chassis.len(), 2);
        let chassis = snapshot
            .chassis
            .iter()
            .find(|chassis| chassis.id == id)
            .unwrap();
        assert_eq!(&chassis.pose.translation_m[..2], &[2.0, 3.0]);
        let heading = crate::math::rotate(chassis.pose.rotation_wxyz, [1.0, 0.0, 0.0]);
        assert!(heading[0].abs() < 1e-9 && (heading[1] - 1.0).abs() < 1e-9);
        assert!(
            server
                .connect_owner(
                    "second",
                    Team::Red,
                    Role::Pilot,
                    Robot::default(),
                    [0.0; 3],
                    0.0
                )
                .is_err()
        );
        owner.send_confirmed(Command::Step { ticks: 16 }).unwrap();
        guest.send_confirmed(Command::Step { ticks: 777 }).unwrap();
        wait_until("command barriers did not arrive", || {
            owner.poll();
            guest.poll();
            owner.commands_confirmed() && guest.commands_confirmed()
        });
        assert_eq!(simulation.snapshot().unwrap().tick, 16);
        assert!(
            guest
                .take_notices()
                .iter()
                .any(|n| n.contains("only the referee"))
        );
        assert_eq!(owner.roster().len(), 2);
        drop(server);
        assert!(simulation.snapshot().unwrap().chassis.is_empty());
    }

    #[test]
    fn owner_spectator_and_referee_keep_controls_without_getting_a_robot() {
        for role in [Role::Spectator, Role::Referee] {
            let (server, simulation) = host();
            let mut owner = server
                .connect_owner("owner", Team::Blue, role, Robot::default(), [0.0; 3], 0.0)
                .unwrap();
            assert!(owner.welcome().chassis.is_none());
            if role == Role::Referee {
                assert!(owner.welcome().team.is_none());
            }
            owner.send_confirmed(Command::Step { ticks: 16 }).unwrap();
            wait_until("owner control was not acknowledged", || {
                owner.poll();
                owner.commands_confirmed()
            });
            assert_eq!(owner.state().unwrap().field.tick, 16);
            assert!(simulation.snapshot().unwrap().chassis.is_empty());
        }
    }

    #[test]
    fn only_the_owner_can_fire_from_a_free_camera() {
        let (server, simulation) = host();
        let mut owner = server
            .connect_owner(
                "owner",
                Team::Red,
                Role::Spectator,
                Robot::default(),
                [0.0; 3],
                0.0,
            )
            .unwrap();
        let mut watcher =
            Client::connect(server.local_addr(), "watcher", None, Role::Spectator).unwrap();
        let fire = Command::SpawnProjectile {
            muzzle: rm_simulator_world::Pose::at([0.0, 0.0, 2.0]),
            shot: rm_simulator_world::Shot::at_limit(rm_simulator_world::Caliber::Mm17),
        };
        owner.send_confirmed(fire).unwrap();
        watcher.send_confirmed(fire).unwrap();
        wait_until("fire results did not arrive", || {
            owner.poll();
            watcher.poll();
            owner.commands_confirmed() && watcher.commands_confirmed()
        });
        let state = simulation.snapshot().unwrap();
        assert!(state.chassis.is_empty());
        assert_eq!(state.projectiles.len(), 1);
        assert!(
            watcher
                .take_notices()
                .iter()
                .any(|n| n.contains("embedded free camera"))
        );
    }

    #[test]
    fn suspended_clock_streams_state_but_holds_ticks_until_ready() {
        let field = Field::new(&FieldConfig::default()).unwrap();
        let simulation = Simulation::new(field, false);
        let server = Server::bind_suspended("127.0.0.1:0", simulation).unwrap();
        server.spawn_clock().unwrap();
        let mut owner = server
            .connect_owner(
                "owner",
                Team::Red,
                Role::Spectator,
                Robot::default(),
                [0.0; 3],
                0.0,
            )
            .unwrap();
        for _ in 0..5 {
            assert_eq!(
                owner
                    .wait_snapshot(Duration::from_secs(5))
                    .unwrap()
                    .field
                    .tick,
                0
            );
        }
        owner.send_confirmed(Command::Step { ticks: 16 }).unwrap();
        wait_until("loading rejection not confirmed", || {
            owner.poll();
            owner.commands_confirmed()
        });
        assert_eq!(owner.state().unwrap().field.tick, 0);
        assert!(
            owner
                .take_notices()
                .iter()
                .any(|n| n.contains("still loading"))
        );
        server.ready();
        wait_until("ready clock never advanced", || {
            owner.poll();
            owner.state().is_some_and(|s| s.field.tick > 0)
        });
    }

    #[test]
    fn confirmation_waits_for_a_result_snapshot_without_blocking_the_caller() {
        let (server, simulation) = host();
        let mut owner = server
            .connect_owner(
                "owner",
                Team::Red,
                Role::Referee,
                Robot::default(),
                [0.0; 3],
                0.0,
            )
            .unwrap();
        let guard = simulation.stall();
        owner.send_confirmed(Command::Step { ticks: 16 }).unwrap();
        owner.send_confirmed(Command::Step { ticks: 17 }).unwrap();
        owner.poll();
        assert!(!owner.commands_confirmed());
        // Taking the capture job also must not wait for the simulation mutex.
        let capture = server.collision_geometry();
        drop(guard);
        wait_until("confirmations did not arrive", || {
            owner.poll();
            owner.commands_confirmed()
        });
        assert_eq!(owner.state().unwrap().field.tick, 33);
        owner.send_confirmed(Command::Step { ticks: 0 }).unwrap();
        wait_until("rejection was not confirmed", || {
            owner.poll();
            owner.commands_confirmed()
        });
        assert_eq!(owner.state().unwrap().field.tick, 33);
        assert!(
            owner
                .take_notices()
                .iter()
                .any(|n| n.contains("step between"))
        );
        drop(capture);
    }

    #[test]
    fn frequent_owner_updates_do_not_change_remote_broadcasts() {
        let (server, _) = host();
        let mut owner = server
            .connect_owner(
                "owner",
                Team::Red,
                Role::Referee,
                Robot::default(),
                [0.0; 3],
                0.0,
            )
            .unwrap();
        let mut guest =
            Client::connect(server.local_addr(), "guest", None, Role::Spectator).unwrap();
        server.handle.broadcast_snapshot(true).unwrap();
        assert!(owner.wait_snapshot(Duration::from_secs(5)).is_some());
        assert!(guest.wait_snapshot(Duration::from_millis(20)).is_none());
        server.broadcast_snapshot();
        assert!(guest.wait_snapshot(Duration::from_secs(5)).is_some());
    }

    fn queued_client(capacity: usize) -> (Client, Receiver<Option<QueuedCommand>>, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (peer, _) = listener.accept().unwrap();
        let (outbox, commands) = mpsc::sync_channel(capacity);
        (
            Client {
                stream: ConnectionStop::Tcp(stream),
                outbox,
                inbox: Arc::default(),
                welcome: Welcome {
                    prediction_scene: None,
                    protocol: PROTOCOL_VERSION,
                    client_id: 1,
                    team: None,
                    role: Role::Spectator,
                    chassis: None,
                    weapon: Default::default(),
                    weapon_limits: Default::default(),
                    tick_ns: rm_simulator_world::tick_ns(),
                },
                latest: None,
                roster: Vec::new(),
                disconnected: None,
                sent_confirmation: 0,
                timing: ClientTiming::default(),
                transport_stats: None,
                host_telemetry: None,
                delivery_stats: None,
                owner_anchor: None,
                acknowledged: 0,
            },
            commands,
            peer,
        )
    }

    #[test]
    fn stalled_writer_does_not_block_commands_and_overflow_disconnects() {
        struct StalledWriter {
            entered: mpsc::Sender<()>,
            release: Receiver<()>,
            bytes: Vec<u8>,
        }
        impl io::Write for StalledWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.bytes.is_empty() {
                    self.entered.send(()).unwrap();
                    self.release.recv().unwrap();
                }
                // Exercise write_all across partial writes too.
                let count = bytes.len().min(3);
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (mut client, commands, mut peer) = queued_client(2);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut writer = StalledWriter {
                entered: entered_tx,
                release: release_rx,
                bytes: Vec::new(),
            };
            write_client_commands(&mut writer, commands, None).unwrap();
            writer.bytes
        });
        let ordered = [
            Command::Chassis {
                chassis: 1,
                command: ChassisCommand::default(),
            },
            Command::Fire {
                shooter: 1,
                timing: None,
            },
            Command::Chassis {
                chassis: 1,
                command: ChassisCommand {
                    aim_yaw_rad: 1.0,
                    ..Default::default()
                },
            },
        ];
        client.send(ordered[0]).unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        client.send(ordered[1]).unwrap();
        client.send(ordered[2]).unwrap();
        assert_eq!(
            client.send(Command::Step { ticks: 1 }).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(client.disconnected().unwrap().contains("queue is full"));
        peer.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        assert!(
            read_message::<ClientMessage, _>(&mut BufReader::new(&mut peer))
                .unwrap()
                .is_none()
        );
        release_tx.send(()).unwrap();
        drop(client);
        let bytes = worker.join().unwrap();
        let mut reader = BufReader::new(bytes.as_slice());
        for command in ordered {
            assert_eq!(
                read_message::<ClientMessage, _>(&mut reader).unwrap(),
                Some(ClientMessage::Command(command))
            );
        }
        assert!(
            read_message::<ClientMessage, _>(&mut reader)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn frame_methods_skip_a_busy_inbox_and_report_worker_failure() {
        let (mut client, commands, _peer) = queued_client(2);
        let inbox = client.inbox.clone();
        let guard = inbox.data.lock().unwrap();
        client.poll();
        assert!(client.take_notices().is_empty());
        client.send(Command::Step { ticks: 1 }).unwrap();
        assert_eq!(
            commands
                .try_recv()
                .unwrap()
                .and_then(|queued| queued.command),
            Some(Command::Step { ticks: 1 })
        );
        drop(guard);
        inbox.fail("sending commands: timed out".into());
        assert_eq!(
            client.send(Command::Step { ticks: 2 }).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        assert_eq!(client.disconnected(), Some("sending commands: timed out"));
    }

    #[test]
    fn incoming_state_is_coalesced_and_notices_keep_their_order() {
        let (mut client, _commands, _peer) = queued_client(2);
        let (_, simulation) = host();
        let mut state = simulation.state().unwrap();
        for tick in 0..1_000 {
            state.field.tick = tick;
            client
                .inbox
                .publish(ServerMessage::Snapshot(Box::new(state.clone())))
                .unwrap();
            client
                .inbox
                .publish(ServerMessage::Roster(vec![PlayerInfo {
                    client_id: tick as u32,
                    name: format!("player {tick}"),
                    team: None,
                    role: Role::Spectator,
                    chassis: None,
                    robot: None,
                }]))
                .unwrap();
        }
        client
            .inbox
            .publish(ServerMessage::Notice("first".into()))
            .unwrap();
        client
            .inbox
            .publish(ServerMessage::Rejected {
                reason: "second".into(),
            })
            .unwrap();
        client.poll();
        assert_eq!(client.take_state().unwrap().field.tick, 999);
        assert_eq!(client.roster()[0].client_id, 999);
        assert_eq!(client.take_notices(), ["first", "rejected: second"]);
        assert!(client.take_notices().is_empty());
        let data = client.inbox.data.lock().unwrap();
        assert!(data.snapshot.is_none());
        assert!(data.roster.is_none());
        assert_eq!(data.notice_bytes, 0);
    }

    #[test]
    fn unread_notices_are_bounded_by_count_and_bytes() {
        let inbox = ClientInbox::default();
        for _ in 0..CLIENT_NOTICE_CAPACITY {
            inbox.publish(ServerMessage::Notice(String::new())).unwrap();
        }
        assert!(inbox.publish(ServerMessage::Notice(String::new())).is_err());
        assert_eq!(
            inbox.data.lock().unwrap().notices.len(),
            CLIENT_NOTICE_CAPACITY
        );
        let inbox = ClientInbox::default();
        inbox
            .publish(ServerMessage::Notice("a".repeat(CLIENT_NOTICE_BYTES)))
            .unwrap();
        assert!(
            inbox
                .publish(ServerMessage::Rejected {
                    reason: String::new()
                })
                .is_err()
        );
        assert_eq!(inbox.data.lock().unwrap().notice_bytes, CLIENT_NOTICE_BYTES);
    }

    #[test]
    fn reader_reports_notice_overflow_and_eof() {
        let (client, _commands, mut peer) = queued_client(2);
        for _ in 0..CLIENT_NOTICE_CAPACITY {
            client
                .inbox
                .publish(ServerMessage::Notice(String::new()))
                .unwrap();
        }
        write_message(&mut peer, &ServerMessage::Notice("overflow".into())).unwrap();
        read_loop(
            &mut BufReader::new(match &client.stream {
                ConnectionStop::Tcp(stream) => stream.try_clone().unwrap(),
                ConnectionStop::Worker(_) => unreachable!(),
            }),
            &client.inbox,
        );
        assert!(
            client
                .inbox
                .data
                .lock()
                .unwrap()
                .failure
                .as_ref()
                .unwrap()
                .contains("too many unread")
        );
        let (mut client, _commands, peer) = queued_client(2);
        drop(peer);
        read_loop(
            &mut BufReader::new(match &client.stream {
                ConnectionStop::Tcp(stream) => stream.try_clone().unwrap(),
                ConnectionStop::Worker(_) => unreachable!(),
            }),
            &client.inbox,
        );
        client.poll();
        assert_eq!(client.disconnected(), Some("host closed the connection"));
    }

    #[test]
    fn startup_wait_wakes_on_snapshot_and_disconnect() {
        let (mut client, _commands, _peer) = queued_client(2);
        assert!(client.wait_snapshot(Duration::ZERO).is_none());
        let (_, simulation) = host();
        let state = simulation.state().unwrap();
        let inbox = client.inbox.clone();
        let publisher = thread::spawn(move || {
            inbox
                .publish(ServerMessage::Snapshot(Box::new(state)))
                .unwrap();
        });
        assert!(client.wait_snapshot(Duration::from_secs(5)).is_some());
        publisher.join().unwrap();
        let inbox = client.inbox.clone();
        let publisher = thread::spawn(move || inbox.fail("host closed".into()));
        assert!(client.wait_snapshot(Duration::from_secs(5)).is_none());
        publisher.join().unwrap();
        assert_eq!(client.disconnected(), Some("host closed"));
    }

    #[test]
    fn clients_get_chassis_by_team_commands_apply_and_snapshots_arrive() {
        let (server, simulation) = host();
        let mut pilot = Client::connect(server.local_addr(), "pilot", None, Role::Pilot).unwrap();
        let own = pilot.welcome().chassis.clone().expect("a chassis");
        assert_eq!(pilot.welcome().team, Some(Team::Red));
        // The next unassigned pilot balances onto blue; a spectator gets no
        // chassis; the referee gets neither chassis nor team.
        let mut rival = Client::connect(server.local_addr(), "rival", None, Role::Pilot).unwrap();
        assert_eq!(rival.welcome().team, Some(Team::Blue));
        let rival_chassis = rival.welcome().chassis.as_ref().unwrap().id;
        assert_ne!(rival_chassis, own.id);
        let mut watcher = Client::connect(
            server.local_addr(),
            "watcher",
            Some(Team::Red),
            Role::Spectator,
        )
        .unwrap();
        assert!(watcher.welcome().chassis.is_none());
        assert_eq!(watcher.welcome().team, Some(Team::Red));
        assert_ne!(watcher.welcome().client_id, pilot.welcome().client_id);
        let mut referee = Client::connect(
            server.local_addr(),
            "referee",
            Some(Team::Blue),
            Role::Referee,
        )
        .unwrap();
        assert_eq!(referee.welcome().team, None);
        assert_eq!(referee.welcome().role, Role::Referee);
        assert!(referee.welcome().chassis.is_none());
        assert_eq!(simulation.snapshot().unwrap().chassis.len(), 2);
        // Match control is the referee's; a pilot's attempt is refused.
        pilot
            .send(Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
        referee
            .send(Command::Referee(RefereeCommand::StartMatch))
            .unwrap();
        referee.send(Command::Step { ticks: 5_500 }).unwrap();
        wait_until("match never started", || {
            simulation.snapshot().unwrap().referee.unwrap().phase == MatchPhase::Running
        });
        server.broadcast_snapshot();
        let state = watcher
            .wait_snapshot(Duration::from_secs(5))
            .expect("snapshot");
        assert!(state.paused);
        assert_eq!(
            state.field.referee.as_ref().unwrap().phase,
            MatchPhase::Running
        );
        assert_eq!(state.field.tick, 5_500);
        assert_eq!(state.field.chassis.len(), 2);
        // Only your own chassis answers to you, and its aim reaches the
        // field; spectators cannot fire.
        rival
            .send(Command::Chassis {
                chassis: own.id,
                command: ChassisCommand {
                    forward_m_s: 1.0,
                    ..Default::default()
                },
            })
            .unwrap();
        pilot
            .send(Command::Chassis {
                chassis: own.id,
                command: ChassisCommand {
                    aim_yaw_rad: 1.0,
                    ..Default::default()
                },
            })
            .unwrap();
        watcher
            .send(Command::SpawnProjectile {
                muzzle: rm_simulator_world::Pose::at([0.0, 0.0, 1.0]),
                shot: rm_simulator_world::Shot::at_limit(rm_simulator_world::Caliber::Mm17),
            })
            .unwrap();
        rival
            .send(Command::Fire {
                shooter: own.id,
                timing: None,
            })
            .unwrap();
        referee.send(Command::Step { ticks: 1 }).unwrap();
        wait_until("rejections never arrived", || {
            watcher.poll();
            pilot.poll();
            rival.poll();
            rival.notices_contain("not yours")
                && rival.notices_contain("own chassis only")
                && watcher.notices_contain("embedded free camera")
                && pilot.notices_contain("only the referee")
        });
        wait_until("aim never applied", || {
            let snapshot = simulation.snapshot().unwrap();
            snapshot.chassis[0].command.aim_yaw_rad == 1.0
        });
        assert!(
            pilot
                .take_notices()
                .iter()
                .any(|n| n == "watcher joined as red, spectating")
        );
        assert_eq!(simulation.snapshot().unwrap().shots_fired, 0);
        // The roster lists everyone with their role and chassis.
        let roster = watcher.roster().to_vec();
        assert_eq!(roster.len(), 4);
        assert_eq!(roster[0].name, "pilot");
        assert_eq!(roster[0].chassis, Some(own.id));
        assert_eq!(roster[1].team, Some(Team::Blue));
        assert_eq!(roster[2].chassis, None);
        assert_eq!(roster[2].role, Role::Spectator);
        assert_eq!((roster[3].role, roster[3].team), (Role::Referee, None));
        assert!(referee.poll_notices_contain("referee joined as referee"));
        // When a pilot leaves its chassis goes with it and the roster shrinks.
        drop(pilot);
        wait_until("chassis never removed", || {
            simulation.snapshot().unwrap().chassis.len() == 1
        });
        wait_until("roster never shrank", || {
            watcher.poll();
            watcher.roster().len() == 3
        });
        assert!(watcher.take_notices().iter().any(|n| n == "pilot left"));
        // The next red pilot gets a fresh id, never the old one.
        let next = Client::connect(server.local_addr(), "next", None, Role::Pilot).unwrap();
        assert_eq!(next.welcome().team, Some(Team::Red));
        assert!(next.welcome().chassis.as_ref().unwrap().id > rival_chassis);
        wait_until("peers", || server.peer_count() == 4);
        assert!(next.disconnected().is_none());
    }
    impl Client {
        fn notices_contain(&self, text: &str) -> bool {
            self.inbox
                .data
                .lock()
                .unwrap()
                .notices
                .iter()
                .any(|n| n.contains(text))
        }
        fn poll_notices_contain(&mut self, text: &str) -> bool {
            self.poll();
            self.inbox
                .data
                .lock()
                .unwrap()
                .notices
                .iter()
                .any(|n| n == text)
        }
    }
    #[test]
    fn wrong_protocol_versions_are_refused() {
        let (server, _) = host();
        let mut stream = TcpStream::connect(server.local_addr()).unwrap();
        write_message(
            &mut stream,
            &ClientMessage::Hello {
                password: String::new(),
                protocol: 999,
                name: "old".into(),
                team: None,
                role: Role::Pilot,
                robot: Robot::default(),
                tick_ns: rm_simulator_world::tick_ns(),
            },
        )
        .unwrap();
        let mut reader = BufReader::new(stream);
        let answer = read_message::<ServerMessage, _>(&mut reader).unwrap();
        assert!(
            matches!(answer, Some(ServerMessage::Rejected { .. })),
            "{answer:?}"
        );
        assert!(
            read_message::<ServerMessage, _>(&mut reader)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_client_that_stops_reading_is_dropped() {
        let (server, _simulation) = host();
        let mut stream = TcpStream::connect(server.local_addr()).unwrap();
        write_message(
            &mut stream,
            &ClientMessage::Hello {
                password: String::new(),
                protocol: PROTOCOL_VERSION,
                name: "sloth".into(),
                team: None,
                role: Role::Spectator,
                robot: Robot::default(),
                tick_ns: rm_simulator_world::tick_ns(),
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while server.peer_count() == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.peer_count(), 1);
        // Give backpressure its own deadline after admission. Yield between
        // broadcasts so the producer cannot starve the socket writer on CI.
        let deadline = Instant::now() + Duration::from_secs(20);
        // Never read: the socket fills, then the outbox, then the peer goes.
        while server.peer_count() == 1 && Instant::now() < deadline {
            server.broadcast_snapshot();
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(server.peer_count(), 0);
        drop(stream);
    }
    #[test]
    fn resource_and_equipment_edits_are_referee_only() {
        let (server, simulation) = host();
        let mut pilot = Client::connect(server.local_addr(), "pilot", None, Role::Pilot).unwrap();
        let mut referee =
            Client::connect(server.local_addr(), "referee", None, Role::Referee).unwrap();
        for json in [
            r#"{"Referee":{"Gameplay":{"Gold":{"team":"Red","gold":777}}}}"#,
            r#"{"Referee":{"SetOutpostHp":{"outpost":0,"hp":1}}}"#,
        ] {
            let command: Command = serde_json::from_str(json).unwrap();
            assert!(command.is_match_control());
            pilot.send_confirmed(command).unwrap();
            wait_until("pilot rejection missing", || {
                pilot.poll();
                pilot.commands_confirmed()
            });
            assert!(
                pilot
                    .take_notices()
                    .iter()
                    .any(|n| n.contains("only the referee"))
            );
            assert_eq!(
                simulation
                    .snapshot()
                    .unwrap()
                    .referee
                    .unwrap()
                    .gameplay
                    .gold[0],
                0
            );
        }
        referee
            .send_confirmed(
                serde_json::from_str(
                    r#"{"Referee":{"Gameplay":{"Gold":{"team":"Red","gold":777}}}}"#,
                )
                .unwrap(),
            )
            .unwrap();
        wait_until("referee edit missing", || {
            referee.poll();
            referee.commands_confirmed()
        });
        assert_eq!(
            simulation
                .snapshot()
                .unwrap()
                .referee
                .unwrap()
                .gameplay
                .gold[0],
            777
        );
        assert_eq!(simulation.snapshot().unwrap().outposts[0].hp, 1500);
    }
}

#[cfg(test)]
#[path = "host_delivery.rs"]
mod host_delivery;

struct ClientTiming {
    /// Every wall-clock reading on the client leg comes from here.
    time: crate::clock::TimeSource,
    epoch: Instant,
    last_probe: Instant,
    pending: Option<(u64, Instant)>,
    rtts: std::collections::VecDeque<u64>,
    rtt_samples: crate::network_trace::EventSamples,
    clock: presentation_clock::ClockEstimate,
    input_lead: presentation_clock::InputLead,
}
impl Default for ClientTiming {
    fn default() -> Self {
        Self::new(crate::clock::TimeSource::system())
    }
}
impl ClientTiming {
    fn new(time: crate::clock::TimeSource) -> Self {
        let now = time.now();
        Self {
            time,
            epoch: now,
            last_probe: now,
            pending: None,
            rtts: Default::default(),
            rtt_samples: Default::default(),
            clock: Default::default(),
            input_lead: Default::default(),
        }
    }
    fn now(&self) -> Instant {
        self.time.now()
    }
    fn elapsed_ns(&self) -> u64 {
        self.local_ns(self.now())
    }
    fn local_ns(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch)
            .as_nanos()
            .min(u64::MAX as u128) as u64
    }
    fn observe(&mut self, state: &SimulationState, at: Instant) {
        self.clock.observe(
            state.input_epoch,
            self.local_ns(at),
            state.field.time_ns,
            state.paused,
        );
    }
    fn sample(&mut self, (nonce, time_ns, paused, received): (u64, u64, bool, Instant)) {
        let Some((expected, sent)) = self.pending else {
            return;
        };
        if nonce != expected {
            return;
        }
        self.pending = None;
        let rtt = received
            .saturating_duration_since(sent)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        if self.rtts.len() == 8 {
            self.rtts.pop_front();
        }
        self.rtts.push_back(rtt);
        self.rtt_samples.record(rtt as f64 / 1e6);
        // Prefer uncongested samples; symmetry is an estimate, not a guarantee.
        if rtt
            <= self
                .rtts
                .iter()
                .copied()
                .min()
                .unwrap_or(rtt)
                .saturating_add(5_000_000)
        {
            self.clock.synchronize(
                self.local_ns(received),
                time_ns.saturating_add(rtt / 2),
                paused,
            );
        }
    }
}
