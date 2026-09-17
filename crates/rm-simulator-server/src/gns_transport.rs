// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Standalone Valve GameNetworkingSockets. The shared peer codec selects reliable
//! and unreliable lanes and encodes periodic state against acknowledged baselines.
//! All native I/O stays on workers.
use super::*;
use crate::udp_codec::{ClientCodec, HostPeer, io_error};
use gns::sys::{ESteamNetworkingConfigValue as Config, ESteamNetworkingConnectionState as State};
use gns::{GnsConfig, GnsConnection, GnsGlobal, GnsSocket, IsReady, IsServer, SendFlags};
use std::collections::BTreeMap;

/// Most the serving worker waits on one poll, 2 ms. A shutdown request wakes
/// the wait at once.
const POLL: Duration = Duration::from_millis(2);
/// Connections accepted at once. A connecting peer beyond this is closed.
const MAX_PEERS: usize = 64;
/// Native pending bytes on one connection, reliable plus unreliable, past which
/// the peer is dropped instead of growing an unbounded backlog.
const MAX_PENDING_BYTES: u32 = 512 * 1024;

/// Resolve an address, taking the first candidate.
fn address(addr: impl ToSocketAddrs) -> io::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| io_error("no socket address"))
}
/// Raise this connection's native send buffer to 8 MiB and its send rate
/// ceiling to 2 MiB/s, and set the initial and connected timeouts to 5 s each.
fn configure(global: &GnsGlobal, connection: GnsConnection) -> io::Result<()> {
    for (key, value) in [
        (
            Config::k_ESteamNetworkingConfig_SendBufferSize,
            8 * 1024 * 1024,
        ),
        // The library defaults both bounds to 256 KiB/s, a hard cap that
        // full snapshots can exceed. Allow congestion control room to grow.
        (
            Config::k_ESteamNetworkingConfig_SendRateMax,
            2 * 1024 * 1024,
        ),
        (Config::k_ESteamNetworkingConfig_TimeoutInitial, 5000),
        (Config::k_ESteamNetworkingConfig_TimeoutConnected, 5000),
    ] {
        global
            .utils()
            .set_connection_config_value(connection, key, GnsConfig::Int32(value))
            .map_err(io_error)?;
    }
    Ok(())
}
/// Send one application packet. A reliable packet is retransmitted until it is
/// acknowledged; an unreliable one may be discarded when congested, and that
/// discard is not an error because the next snapshot is independent.
fn send<S: IsReady>(
    socket: &GnsSocket<S>,
    connection: GnsConnection,
    bytes: Vec<u8>,
    reliable: bool,
    observer: &crate::network_trace::Observer,
    peer: Option<u32>,
) -> io::Result<()> {
    observer.packet("submit_native", peer, &bytes);
    let length = bytes.len();
    let flags = if reliable {
        SendFlags::RELIABLE | SendFlags::NO_NAGLE
    } else {
        SendFlags::UNRELIABLE | SendFlags::NO_DELAY | SendFlags::NO_NAGLE
    };
    let message = GnsGlobal::get()
        .map_err(io_error)?
        .utils()
        .allocate_message(connection, flags, bytes);
    match socket.send_message(message) {
        Ok(_) => {}
        // NO_DELAY may discard a congested periodic fragment. The next
        // independent snapshot repairs that loss without disconnecting.
        Err(gns::GnsError::Api(gns::sys::EResult::k_EResultIgnored)) if !reliable => {
            observer.record(crate::network_trace::Event {
                stage: "discard_native",
                kind: "unreliable",
                bytes: Some(length),
                peer,
                ..Default::default()
            });
        }
        Err(error) => return Err(io_error(error)),
    }
    Ok(())
}

/// A bound GNS listener and the worker that serves it. Dropping it stops and
/// joins the worker.
pub(super) struct UdpListener {
    /// Address the listen socket is actually bound to, with the port the OS chose.
    pub(super) local_addr: SocketAddr,
    stop: Stop,
    worker: Option<thread::JoinHandle<()>>,
}
impl UdpListener {
    /// Bind a GNS socket and start the serving worker. A requested port of zero
    /// is reserved through a plain UDP bind first, because the library needs a
    /// nonzero port and another socket may claim the reservation before the
    /// GNS bind. A worker failure is logged and requests the stop signal, so a
    /// dead listener does not leave the host running as if it were reachable.
    pub(super) fn start(
        addr: impl ToSocketAddrs,
        handle: HostHandle,
        stop: Stop,
    ) -> io::Result<Self> {
        let addr = address(addr)?;
        let global = GnsGlobal::get().map_err(io_error)?;
        // GNS requires a nonzero port. Ask the OS for a candidate and retry
        // if another socket claims it between releasing it and the GNS bind.
        let socket = if addr.port() == 0 {
            let mut bound = None;
            for _ in 0..16 {
                let reservation = std::net::UdpSocket::bind(addr)?;
                let port = reservation.local_addr()?.port();
                drop(reservation);
                if let Ok(socket) = GnsSocket::new(global).listen(addr.ip(), port) {
                    bound = Some(socket);
                    break;
                }
            }
            bound.ok_or_else(|| io_error("could not bind an ephemeral GNS port"))?
        } else {
            GnsSocket::new(global)
                .listen(addr.ip(), addr.port())
                .map_err(io_error)?
        };
        let (ip, port) = socket
            .get_listen_socket_address()
            .ok_or_else(|| io_error("missing listen address"))?;
        let stopping = stop.clone();
        let worker = thread::Builder::new()
            .name("rm-gns-host".into())
            .spawn(move || {
                if let Err(error) = serve(socket, global, &handle, &stopping) {
                    eprintln!("GNS host: {error}");
                    stopping.request();
                }
            })?;
        Ok(Self {
            local_addr: SocketAddr::new(ip, port),
            stop,
            worker: Some(worker),
        })
    }
    /// Stop and join the serving worker. Idempotent: a second call finds no
    /// worker left to join.
    pub(super) fn shutdown(&mut self) {
        self.stop.request();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Drop for UdpListener {
    /// Stop and join the serving worker.
    fn drop(&mut self) {
        self.shutdown();
    }
}
/// Close one peer, naming why. Silent disconnects are the hardest GNS fault to
/// diagnose, so every reason reaches the operator.
fn remove(
    socket: &GnsSocket<IsServer>,
    peers: &mut BTreeMap<GnsConnection, HostPeer>,
    connection: GnsConnection,
    reason: &str,
) {
    if let Some(peer) = peers.remove(&connection) {
        let encoding = peer.encoding_stats();
        eprintln!(
            "gns peer {connection:?}: {reason} (world={} skipped_congested={} replaced_unsent={} framed_world_bytes={})",
            encoding.world_updates,
            encoding.skipped_world_updates,
            peer.replaced_unsent(),
            encoding.framed_world_bytes,
        );
        let detail = std::ffi::CString::new(reason).ok();
        let _ = socket.close_connection(connection, 1000, detail.as_deref(), false);
    }
}
/// One parseable log line per admitted peer: cumulative snapshot encodes,
/// congestion skips and pacer replacements since admission. A peer whose
/// `skipped_congested` climbs while `world` stalls is the one starving its
/// client's auto-aim observation.
fn log_peer_stats(peer: &HostPeer) {
    let Some(id) = peer.client_id() else {
        return;
    };
    let encoding = peer.encoding_stats();
    eprintln!(
        "net peer={id} world={} skipped_congested={} replaced_unsent={} framed_world_bytes={} owner_updates={}",
        encoding.world_updates,
        encoding.skipped_world_updates,
        peer.replaced_unsent(),
        encoding.framed_world_bytes,
        encoding.owner_updates,
    );
}
/// How much this connection already owes the native send buffer. A peer that
/// cannot drain is dropped rather than allowed to grow an unbounded backlog.
fn backlog(socket: &GnsSocket<IsServer>, connection: GnsConnection) -> io::Result<u32> {
    let (status, _) = socket
        .get_connection_real_time_status(connection, 0)
        .map_err(io_error)?;
    let pending = status
        .pending_bytes_reliable()
        .saturating_add(status.pending_bytes_unreliable());
    if pending > MAX_PENDING_BYTES || status.approximated_queue_time() > WRITE_TIMEOUT {
        return Err(io_error("GNS peer send backlog"));
    }
    Ok(pending)
}
/// Move datagrams between the socket and one [`HostPeer`] per connection. All
/// framing, pacing and host submission lives in [`crate::udp_codec`].
fn serve(
    socket: GnsSocket<IsServer>,
    global: &'static GnsGlobal,
    handle: &HostHandle,
    stop: &Stop,
) -> io::Result<()> {
    let observer =
        crate::network_trace::Observer::new("gns-host", crate::clock::TimeSource::system());
    let mut peers = BTreeMap::<GnsConnection, HostPeer>::new();
    let mut last_stats_log = Instant::now();
    let result = (|| {
        while !stop.wait(POLL) {
            global.poll_callbacks();
            if last_stats_log.elapsed() >= Duration::from_secs(10) {
                last_stats_log = Instant::now();
                for peer in peers.values() {
                    log_peer_stats(peer);
                }
            }
            for event in socket.receive_events() {
                let connection = event.connection();
                match event.info().state() {
                    State::k_ESteamNetworkingConnectionState_Connecting => {
                        if peers.len() >= MAX_PEERS {
                            let _ = socket.close_connection(connection, 1000, None, false);
                            continue;
                        }
                        if configure(global, connection)
                            .and_then(|()| socket.accept(connection).map_err(io_error))
                            .is_err()
                        {
                            let _ = socket.close_connection(connection, 1000, None, false);
                            continue;
                        }
                        peers.entry(connection).or_insert_with(|| {
                            HostPeer::new(
                                handle.clone(),
                                Instant::now(),
                                crate::pacing::configured_rate(
                                    "RM_NET_DOWN_KIB_S",
                                    crate::pacing::downstream_default(),
                                ),
                                false,
                            )
                        });
                    }
                    State::k_ESteamNetworkingConnectionState_ClosedByPeer => {
                        remove(&socket, &mut peers, connection, "closed by peer")
                    }
                    State::k_ESteamNetworkingConnectionState_ProblemDetectedLocally => remove(
                        &socket,
                        &mut peers,
                        connection,
                        "connection problem detected locally",
                    ),
                    _ => {}
                }
            }
            for message in socket.receive_messages::<64>().map_err(io_error)? {
                let connection = message.connection();
                let Some(peer) = peers.get_mut(&connection) else {
                    continue;
                };
                observer.packet("receive", peer.client_id(), message.payload());
                let started = Instant::now();
                let result = peer.deliver(message.payload(), started);
                observer.work("host_decode_submit", started.elapsed());
                if let Err(error) = result {
                    remove(&socket, &mut peers, connection, &error.to_string());
                }
            }
            let mut closing: Vec<(GnsConnection, String)> = Vec::new();
            for (&connection, peer) in &mut peers {
                if peer.closed() {
                    closing.push((connection, "host closed this seat".into()));
                    continue;
                }
                if peer.client_id().is_none() {
                    if peer.admitted().elapsed() > HELLO_TIMEOUT {
                        closing.push((connection, "no hello before the timeout".into()));
                    }
                    continue;
                }
                // Limit work per peer, including native pending bytes. Never move
                // an unbounded application backlog into the library's send buffer.
                let started = Instant::now();
                let result = backlog(&socket, connection)
                    .and_then(|pending| peer.pump(started, pending, 16, 16));
                observer.work("host_encode_pace", started.elapsed());
                match result {
                    Ok(()) => {
                        for packet in peer.take_outgoing() {
                            if let Err(error) = send(
                                &socket,
                                connection,
                                packet.bytes,
                                packet.reliable,
                                &observer,
                                peer.client_id(),
                            ) {
                                closing.push((connection, error.to_string()));
                                break;
                            }
                        }
                    }
                    Err(error) => closing.push((connection, error.to_string())),
                }
            }
            for (connection, reason) in closing {
                remove(&socket, &mut peers, connection, &reason);
            }
        }
        Ok(())
    })();
    for connection in peers.keys().copied().collect::<Vec<_>>() {
        remove(&socket, &mut peers, connection, "host stopped");
    }
    result
}

impl Server {
    /// Transfer the simulation to its owner worker and bind Valve
    /// GameNetworkingSockets over UDP to accept clients.
    /// Start real-time pacing with [`Server::spawn_clock`] or [`Server::run_clock`].
    pub fn bind(addr: impl ToSocketAddrs, simulation: Simulation) -> io::Result<Self> {
        Self::udp_with_readiness(addr, simulation, true)
    }
    /// Prepare a host whose clock and command handling stay held until
    /// [`HostHandle::ready`] releases them. The app uses this while it loads
    /// local scenery, so a joining client cannot command a field that is not
    /// built yet.
    pub fn bind_suspended(addr: impl ToSocketAddrs, simulation: Simulation) -> io::Result<Self> {
        Self::udp_with_readiness(addr, simulation, false)
    }
    /// Build the host, its handle and a GNS listener with the given readiness.
    fn udp_with_readiness(
        addr: impl ToSocketAddrs,
        simulation: Simulation,
        ready: bool,
    ) -> io::Result<Self> {
        let host = Host::new(simulation, ready)?;
        let handle = host.handle();
        let stop = host.stop_signal();
        let udp = UdpListener::start(addr, handle.clone(), stop.clone())?;
        Ok(Self {
            host,
            handle,
            udp: Some(udp),
            stop,
            owner: Mutex::new(None),
        })
    }
}
impl Client {
    /// Connect using standalone GameNetworkingSockets. No Steam login is needed.
    /// The worker only moves datagrams; the client codec owns the wire.
    pub fn connect_udp(
        addr: impl ToSocketAddrs,
        name: &str,
        team: Option<Team>,
        role: Role,
    ) -> anyhow::Result<Self> {
        Self::connect_udp_with_password(addr, name, team, role, Robot::default(), "")
    }
    /// Connect over GNS naming the robot to drive and a join password. The
    /// hello must be answered within `HELLO_TIMEOUT` or the connection attempt
    /// fails and its worker stops. Commands queue on the returned client; this
    /// call blocks until the welcome arrives, so run it off the UI thread.
    pub fn connect_udp_with_password(
        addr: impl ToSocketAddrs,
        name: &str,
        team: Option<Team>,
        role: Role,
        robot: Robot,
        password: &str,
    ) -> anyhow::Result<Self> {
        let addr = address(addr)?;
        let global = GnsGlobal::get().map_err(io_error)?;
        let socket = GnsSocket::new(global)
            .connect(addr.ip(), addr.port())
            .map_err(io_error)?;
        let connection = socket.connection();
        configure(global, connection)?;
        let stop = Stop::default();
        let inbox = Arc::new(ClientInbox::for_transport(
            "gns",
            crate::clock::TimeSource::system(),
        ));
        let (outbox, commands) =
            mpsc::sync_channel::<Option<QueuedCommand>>(CLIENT_COMMAND_CAPACITY);
        let (welcome_tx, welcome_rx) = mpsc::sync_channel(1);
        let hello = ClientCodec::hello_with_password(name, team, role, robot, password)?;
        let stopping = stop.clone();
        let incoming = inbox.clone();
        let timing = ClientTiming::default();
        let epoch = timing.epoch;
        thread::Builder::new()
            .name("rm-gns-client".into())
            .spawn(move || {
                let result = (|| {
                    send(&socket, connection, hello, true, &incoming.observer, None)?;
                    let mut driver = ClientDriver::new(
                        epoch,
                        crate::pacing::configured_rate("RM_NET_UP_KIB_S", crate::pacing::upstream_default()),
                        false,
                        incoming.clone(),
                        commands,
                        PumpBudget { commands: Some(64), packets: Some(16) },
                    );
                    let mut last_stats = Instant::now() - Duration::from_secs(1);
                    while !stopping.wait(POLL) {
                        global.poll_callbacks();
                        for event in socket.receive_events() {
                            if matches!(
                                event.info().state(),
                                State::k_ESteamNetworkingConnectionState_ClosedByPeer
                                    | State::k_ESteamNetworkingConnectionState_ProblemDetectedLocally
                            ) {
                                return Err(io_error(format!(
                                    "GNS disconnected: {}",
                                    event.info().end_debug()
                                )));
                            }
                        }
                        for message in socket.receive_messages::<256>().map_err(io_error)? {
                            incoming.observer.packet("receive", None, message.payload());
                            if let Some(welcome) = driver.receive(message.payload(), Instant::now())? {
                                welcome_tx.try_send(Ok(welcome)).map_err(io_error)?;
                            }
                        }
                        if last_stats.elapsed() >= Duration::from_millis(250) {
                            if let Ok((status, _)) =
                                socket.get_connection_real_time_status(connection, 0)
                            {
                                let stats = crate::network_stats::TransportStats {
                                    sampled_elapsed_ms: epoch.elapsed().as_secs_f64() * 1000.,
                                    transport_rtt_ms: (status.ping() < i32::MAX as u32)
                                        .then_some(status.ping() as f64),
                                    send_bytes_per_s: Some(status.out_bytes_per_sec() as f64),
                                    receive_bytes_per_s: Some(status.in_bytes_per_sec() as f64),
                                    send_queue_ms: Some(
                                        status.approximated_queue_time().as_secs_f64() * 1000.,
                                    ),
                                    pending_reliable_bytes: Some(status.pending_bytes_reliable()),
                                    pending_unreliable_bytes: Some(
                                        status.pending_bytes_unreliable(),
                                    ),
                                    ..driver.stats()
                                };
                                incoming
                                    .data
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .transport_stats = Some(stats);
                            }
                            last_stats = Instant::now();
                        }
                        if !driver.submit_pending(Instant::now, || {
                            let (status, _) = socket
                                .get_connection_real_time_status(connection, 0)
                                .map_err(io_error)?;
                            if status.pending_bytes_reliable() > MAX_PENDING_BYTES
                                || status.approximated_queue_time() > WRITE_TIMEOUT
                            {
                                return Err(io_error("GNS command backlog"));
                            }
                            Ok(())
                        })? {
                            return Ok(());
                        }
                        driver.send_pending(Instant::now, |packet| {
                            send(&socket, connection, packet.bytes, packet.reliable, &incoming.observer, None)
                        })?;
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    let reason = hello_failure(&error.to_string());
                    incoming.fail(reason.clone());
                    let _ = welcome_tx.try_send(Err(reason));
                }
                let _ = socket.close_connection(connection, 1000, None, false);
            })?;
        let welcome = match welcome_rx.recv_timeout(HELLO_TIMEOUT) {
            Ok(Ok(welcome)) => welcome,
            Ok(Err(reason)) => {
                stop.request();
                anyhow::bail!("{reason}");
            }
            Err(error) => {
                stop.request();
                anyhow::bail!("GNS hello failed: {error}");
            }
        };
        Ok(Self::new(stop, timing, outbox, inbox, welcome))
    }
}
/// Preserves peer rejection details, including the first release's legacy reason.
fn hello_failure(reason: &str) -> String {
    if reason.contains("incompatible protocol") {
        "Version mismatch. Update both games to the same version.".into()
    } else {
        reason.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::udp_codec::Frames;
    use rm_simulator_world::{Field, FieldConfig};

    fn wait(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "UDP operation timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn paced_pilot_gets_anchors_and_compressed_shot_retries_execute_once() {
        let _serial = NATIVE_TEST.lock().unwrap_or_else(|p| p.into_inner());
        let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), false)
            .with_spawner(crate::layout::ChassisSpawner {
                config: Default::default(),
                terrain: None,
            });
        let server = Server::bind("127.0.0.1:0", simulation).unwrap();
        server.spawn_clock().unwrap();
        let mut client =
            Client::connect_udp(server.local_addr(), "pilot", None, Role::Pilot).unwrap();
        wait(|| {
            client.poll();
            client.state().is_some()
        });
        let state = client.state().unwrap();
        let shooter = client.welcome.chassis.as_ref().unwrap().id;
        let frame = crate::input_stream::InputFrame {
            input_epoch: state.input_epoch,
            sequence: 1,
            sampled_time_ns: state.field.time_ns,
            duration_ticks: 16,
            placement_revision: state
                .field
                .chassis
                .iter()
                .find(|c| c.id == shooter)
                .unwrap()
                .placement_revision,
            command: Default::default(),
        };
        for _ in 0..8 {
            client
                .send(Command::FireAimed {
                    shooter,
                    shot_id: 1,
                    input: frame,
                })
                .unwrap();
        }
        let mut accepted = false;
        let mut anchor = false;
        wait(|| {
            client.poll();
            anchor |= client.take_owner_anchor().is_some();
            let results = client.take_shot_results();
            for result in &results {
                assert!(result.result.is_ok(), "{result:?}");
            }
            assert!(
                client.disconnected().is_none(),
                "{:?}",
                client.disconnected()
            );
            accepted |= results.iter().any(|r| r.shot_id == 1 && r.result.is_ok());
            accepted && anchor
        });
        assert!(client.disconnected().is_none());
        assert_eq!(server.handle().state().unwrap().field.shots_fired, 1);
    }
    #[test]
    fn udp_commands_confirm_state_and_local_owner_works_offline() {
        let _serial = NATIVE_TEST.lock().unwrap_or_else(|p| p.into_inner());
        let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true);
        let mut server = Server::bind("127.0.0.1:0", simulation).unwrap();
        let address = server.local_addr();
        let mut owner = server
            .connect_owner(
                "local",
                Team::Red,
                Role::Referee,
                Robot::default(),
                [0.; 3],
                0.,
            )
            .unwrap();
        let mut guest = Client::connect_udp(address, "guest", None, Role::Spectator).unwrap();
        owner.send_confirmed(Command::Step { ticks: 16 }).unwrap();
        wait(|| {
            owner.poll();
            owner.commands_confirmed()
        });
        guest.send_confirmed(Command::Step { ticks: 777 }).unwrap();
        wait(|| {
            guest.poll();
            guest.commands_confirmed()
        });
        assert_eq!(guest.state().unwrap().field.tick, 16);
        assert_eq!(server.handle().snapshot().unwrap().tick, 16);
        drop(owner);
        drop(guest);
        server.shutdown();
        // GNS closes raw sockets on its service thread after CloseListenSocket
        // returns. Joining our worker does not wait for that native cleanup.
        // Still require the port to be released within the bounded wait.
        let mut rebound = None;
        wait(|| match std::net::UdpSocket::bind(address) {
            Ok(socket) => {
                rebound = Some(socket);
                true
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => false,
            Err(error) => panic!("could not rebind UDP listener {address}: {error}"),
        });
    }

    #[test]
    fn reliable_commands_survive_native_packet_loss_and_lag() {
        let _serial = NATIVE_TEST.lock().unwrap_or_else(|p| p.into_inner());
        let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true);
        let server = Server::bind("127.0.0.1:0", simulation).unwrap();
        let global = GnsGlobal::get().unwrap();
        let address = server.local_addr();
        let socket = GnsSocket::new(global)
            .connect(address.ip(), address.port())
            .unwrap();
        let connection = socket.connection();
        configure(global, connection).unwrap();
        struct ResetImpairment;
        impl Drop for ResetImpairment {
            fn drop(&mut self) {
                let utils = GnsGlobal::get().unwrap().utils();
                utils
                    .set_global_config_value(
                        Config::k_ESteamNetworkingConfig_FakePacketLoss_Send,
                        GnsConfig::Float(0.),
                    )
                    .unwrap();
                utils
                    .set_global_config_value(
                        Config::k_ESteamNetworkingConfig_FakePacketLag_Send,
                        GnsConfig::Int32(0),
                    )
                    .unwrap();
            }
        }
        let _reset = ResetImpairment;
        for (key, value) in [
            (
                Config::k_ESteamNetworkingConfig_FakePacketLoss_Send,
                GnsConfig::Float(20.),
            ),
            (
                Config::k_ESteamNetworkingConfig_FakePacketLag_Send,
                GnsConfig::Int32(20),
            ),
        ] {
            global.utils().set_global_config_value(key, value).unwrap();
        }
        for message in [
            ClientMessage::Hello {
                password: String::new(),
                protocol: PROTOCOL_VERSION,
                name: "lossy referee".into(),
                team: None,
                role: Role::Referee,
                robot: Robot::default(),
            },
            ClientMessage::Command(Command::Step { ticks: 17 }),
            ClientMessage::Command(Command::Step { ticks: 19 }),
            ClientMessage::Ping { nonce: 1 },
        ] {
            send(
                &socket,
                connection,
                crate::snapshot_codec::encode_client_message(&message),
                true,
                &crate::network_trace::Observer::new("test", crate::clock::TimeSource::system()),
                None,
            )
            .unwrap();
        }
        let mut frames = Frames::default();
        let mut tick = None;
        let mut confirmed = false;
        wait(|| {
            global.poll_callbacks();
            for message in socket.receive_messages::<256>().unwrap() {
                match frames.receive(message.payload(), Instant::now()).unwrap() {
                    Some(ServerMessage::Snapshot(state)) => tick = Some(state.field.tick),
                    Some(ServerMessage::Pong { nonce: 1 }) => {
                        assert_eq!(tick, Some(36));
                        confirmed = true;
                    }
                    _ => {}
                }
            }
            confirmed
        });
        socket
            .close_connection(connection, 1000, None, false)
            .unwrap();
    }

    #[test]
    fn udp_referee_confirmation_precedes_pong_and_shutdown_disconnects() {
        let _serial = NATIVE_TEST.lock().unwrap_or_else(|p| p.into_inner());
        let simulation = Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true);
        let mut server = Server::bind("127.0.0.1:0", simulation).unwrap();
        let mut client =
            Client::connect_udp(server.local_addr(), "referee", None, Role::Referee).unwrap();
        client.send_confirmed(Command::Step { ticks: 33 }).unwrap();
        wait(|| {
            client.poll();
            client.commands_confirmed()
        });
        assert_eq!(client.state().unwrap().field.tick, 33);
        server.shutdown();
        wait(|| {
            client.poll();
            client.disconnected().is_some()
        });
    }
}
