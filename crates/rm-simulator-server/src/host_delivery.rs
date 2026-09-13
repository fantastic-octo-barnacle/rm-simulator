// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Host-side delivery traces: what the bounded outbox, its ordering barriers
//! and the clock-probe path do when a client reads slowly or not at all. No
//! sleeps, CAD assets, external services or host-clock advancement.
//!
//! Transit impairment itself lives in [`crate::scripted_link`], and the
//! end-to-end client trace over it is the app crate's `net_harness`.
use super::*;
use crate::host::Outbound;
use rm_simulator_world::{Field, FieldConfig};
use std::io::{Cursor, Read};

// Also exercise the production JSON-lines decoder with split reads.
/// A reader that hands out at most 7 bytes per call, so one message spans
/// several reads.
struct Fragments(Cursor<Vec<u8>>);
impl Read for Fragments {
    /// Read at most 7 bytes, or fewer when the buffer is smaller.
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = out.len().min(7);
        self.0.read(&mut out[..count])
    }
}
/// Decode one JSON-lines message from `bytes` and assert that no second
/// message follows.
fn decode<T: serde::de::DeserializeOwned>(bytes: String) -> T {
    let mut reader = BufReader::new(Fragments(Cursor::new(bytes.into_bytes())));
    let message = read_message(&mut reader).unwrap().unwrap();
    assert!(read_message::<T, _>(&mut reader).unwrap().is_none());
    message
}

/// A host with one loopback peer whose writer nothing drains. The fixture
/// reports the receiver so a test can inspect and drain the outbox itself.
fn fixture() -> (Host, Client, outbox::Receiver) {
    let host = Host::new(
        Simulation::new(Field::new(&FieldConfig::default()).unwrap(), true),
        true,
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (peer, _) = listener.accept().unwrap();
    let (sender, receiver) = outbox::channel(OUTBOX_CAPACITY);
    let welcome = host
        .handle()
        .join(PeerRegistration {
            password: String::new(),
            name: "impaired referee".into(),
            team: None,
            role: Role::Referee,
            owner_spawn: None,
            outbox: sender,
            stream: ConnectionStop::Tcp(peer),
        })
        .unwrap();
    let (outbox, _commands) = mpsc::sync_channel(CLIENT_COMMAND_CAPACITY);
    let client = Client {
        stream: ConnectionStop::Tcp(stream),
        outbox,
        inbox: Arc::default(),
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
    while receiver.queued() > 0 {
        receiver.recv().unwrap();
    }
    (host, client, receiver)
}

/// A writer that never drains keeps the confirmation barrier and the Pong in
/// order and holds the periodic backlog at a few frames, not one per tick.
#[test]
fn impairment_stalled_writer_keeps_barriers_and_bounds_periodic_backlog() {
    let (host, mut client, receiver) = fixture();
    for tick in 1..=1_000 {
        host.handle().apply(&Command::Step { ticks: 1 }).unwrap();
        if tick == 500 {
            host.handle()
                .message(client.welcome.client_id, ClientMessage::Ping { nonce: 1 })
                .unwrap();
        }
        host.handle().broadcast_snapshot(false).unwrap();
    }
    host.handle().state().unwrap();
    assert_eq!(receiver.queued(), 4);
    client.sent_confirmation = 1;
    let mut ticks = Vec::new();
    while receiver.queued() > 0 {
        let message = decode(receiver.recv().unwrap().encoded().to_owned());
        if let ServerMessage::Snapshot(state) = &message {
            ticks.push(state.field.tick);
        }
        client.inbox.publish(message).unwrap();
        client.poll();
        if client.commands_confirmed() {
            assert!(client.state().unwrap().field.tick >= 500);
        }
    }
    assert_eq!(ticks, [499, 500, 1_000]);
    assert!(client.commands_confirmed());
}

/// Reliable frames never displace each other, so a flood past the outbox
/// capacity fails while the queued frames keep their order.
#[test]
fn impairment_reliable_confirmation_flood_fails_at_the_bound() {
    let (sender, receiver) = outbox::channel(OUTBOX_CAPACITY);
    for nonce in 0..OUTBOX_CAPACITY {
        sender
            .push(
                Arc::new(Outbound::new(ServerMessage::Pong {
                    nonce: nonce as u64,
                })),
                false,
            )
            .unwrap();
    }
    assert!(
        sender
            .push(
                Arc::new(Outbound::new(ServerMessage::Pong { nonce: 999 })),
                false
            )
            .is_err()
    );
    assert_eq!(receiver.queued(), OUTBOX_CAPACITY);
    for nonce in 0..OUTBOX_CAPACITY {
        assert!(
            matches!(receiver.recv().unwrap().message(), ServerMessage::Pong { nonce: actual } if *actual == nonce as u64)
        );
    }
}

/// A time probe answers with a sample and never confirms a queued command or
/// advances a tick. A Ping that follows keeps the usual order: a full snapshot,
/// then the Pong.
#[test]
fn impairment_clock_samples_do_not_acknowledge_commands_or_advance_ticks() {
    let (host, mut client, receiver) = fixture();
    client.sent_confirmation = 42;
    host.handle()
        .message(
            client.welcome.client_id,
            ClientMessage::TimeProbe { nonce: 42 },
        )
        .unwrap();
    assert_eq!(host.handle().state().unwrap().field.tick, 0);
    assert_eq!(receiver.queued(), 1);
    let message = decode(receiver.recv().unwrap().encoded().to_owned());
    assert!(matches!(
        message,
        ServerMessage::TimeSample {
            nonce: 42,
            time_ns: 0,
            paused: true
        }
    ));
    client.inbox.publish(message).unwrap();
    client.poll();
    assert!(!client.commands_confirmed());
    host.handle()
        .message(client.welcome.client_id, ClientMessage::Ping { nonce: 42 })
        .unwrap();
    host.handle().state().unwrap();
    assert_eq!(receiver.queued(), 2);
    assert!(matches!(
        receiver.recv().unwrap().message(),
        ServerMessage::Snapshot(_)
    ));
    assert!(matches!(
        receiver.recv().unwrap().message(),
        ServerMessage::Pong { nonce: 42 }
    ));
}
