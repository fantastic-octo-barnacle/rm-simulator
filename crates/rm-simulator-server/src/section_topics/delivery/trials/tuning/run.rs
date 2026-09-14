// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Instrumentation parity and isolated diagnostic trials, not parameter tuning.
use super::super::stage3;
use super::{
    link::{Schedule, TimeLink},
    *,
};
use presentation::{Cadence, View};

#[derive(Default)]
struct Controls {
    confirmations: BTreeMap<u64, u64>,
    pongs: BTreeMap<u64, u64>,
    confirmation_ms: Vec<f64>,
    pong_ms: Vec<f64>,
    application_confirmations: u64,
}
impl Controls {
    fn receive(
        &mut self,
        message: &ServerMessage,
        ms: u64,
        offered: &BTreeMap<u64, u64>,
        sources: &[SimulationState],
        compact: bool,
    ) -> bool {
        match message {
            ServerMessage::Snapshot(state) if offered.contains_key(&state.snapshot_id) => {
                let original = ServerMessage::Snapshot(Box::new(
                    sources[state.snapshot_id as usize - 1].clone(),
                ));
                let expected = if compact {
                    snapshot_codec::decode_player_message(&snapshot_codec::encode_player_message(
                        &original,
                    ))
                    .unwrap()
                } else {
                    serde_json::from_slice::<ServerMessage>(&serde_json::to_vec(&original).unwrap())
                        .unwrap()
                };
                assert_eq!(
                    *message, expected,
                    "confirmation must preserve its complete independent representation"
                );
                assert!(self.confirmations.insert(state.snapshot_id, ms).is_none());
                self.confirmation_ms
                    .push((ms - offered[&state.snapshot_id]) as f64);
                true
            }
            ServerMessage::Pong { nonce } => {
                assert!(
                    self.confirmations.contains_key(nonce),
                    "Pong overtook confirmation"
                );
                assert!(self.pongs.insert(*nonce, ms).is_none());
                self.pong_ms.push((ms - offered[nonce]) as f64);
                true
            }
            _ => false,
        }
    }
    fn json(&self, offered: usize) -> serde_json::Value {
        serde_json::json!({"offered_pairs":offered,"confirmation_latency_ms":distribution(&self.confirmation_ms),
            "pong_latency_ms":distribution(&self.pong_ms),"pending_confirmations_at_end":offered-self.confirmations.len(),
            "pending_pongs_at_end":offered-self.pongs.len(),"confirmation_before_pong":true, "application_confirmations":self.application_confirmations})
    }
}
// Observe complete independent controls before ClientCodec's stale-snapshot filter.
// Reliable RMG1 fragments arrive in carrier order; this mirror has no feedback
// and cannot affect either codec. Baseline envelopes are not application controls.
#[derive(Default)]
struct ReliableMirror {
    revision: Option<u64>,
    bytes: Vec<u8>,
}
impl ReliableMirror {
    fn receive(&mut self, packet: &[u8]) -> Option<ServerMessage> {
        if !packet.starts_with(b"RMG1") || packet[4] != 1 {
            return None;
        }
        let revision = u64::from_le_bytes(packet[5..13].try_into().unwrap());
        let total = u32::from_le_bytes(packet[13..17].try_into().unwrap()) as usize;
        let index = u32::from_le_bytes(packet[17..21].try_into().unwrap()) as usize;
        // RMG1 uses a 21-byte header and 1000-byte payload pieces.
        if index == 0 {
            assert!(self.bytes.is_empty());
            self.revision = Some(revision);
        }
        assert_eq!(self.revision, Some(revision));
        assert_eq!(self.bytes.len(), index * 1000);
        self.bytes.extend_from_slice(&packet[21..]);
        assert!(self.bytes.len() <= total);
        if self.bytes.len() != total {
            return None;
        }
        let raw = crate::compression::decompress(&self.bytes, FRAME_LIMIT).unwrap();
        self.bytes.clear();
        self.revision = None;
        if udp_snapshot::parse(&raw).unwrap().is_some() {
            return None;
        }
        Some(snapshot_codec::decode_player_message(&raw).unwrap())
    }
}
fn control_message(message: &ServerMessage, offered: &BTreeMap<u64, u64>) -> bool {
    matches!(message, ServerMessage::Snapshot(s) if offered.contains_key(&s.snapshot_id))
        || matches!(message, ServerMessage::Pong { .. })
}
fn hash_packet(hash: &mut Sha256, ms: u64, lane: &str, packet: &Datagram) {
    hash.update(ms.to_le_bytes());
    hash.update(lane.as_bytes());
    hash.update([u8::from(packet.reliable)]);
    hash.update((packet.bytes.len() as u64).to_le_bytes());
    hash.update(&packet.bytes);
}
fn hash_message(hash: &mut Sha256, ms: u64, lane: &str, message: &ServerMessage) {
    let raw = serde_json::to_vec(message).unwrap();
    hash.update(ms.to_le_bytes());
    hash.update(lane.as_bytes());
    hash.update((raw.len() as u64).to_le_bytes());
    hash.update(raw);
}
fn recover(out: &mut [Option<u64>; 3], end: u64, now: u64, checkpoint: u64, view: &View) {
    for (i, capture) in [
        checkpoint,
        view.chassis.as_ref().map_or(0, |p| p.capture),
        view.projectiles.as_ref().map_or(0, |p| p.capture),
    ]
    .into_iter()
    .enumerate()
    {
        if capture * 16 >= end && out[i].is_none() {
            out[i] = Some(now - end);
        }
    }
}
fn trial(
    sources: &[SimulationState],
    seed: u64,
    profile: &str,
    warmup: u64,
    enabled: bool,
) -> serde_json::Value {
    crate::compression::reset_test_contexts();
    let epoch = Instant::now();
    let cadence = Cadence {
        chassis_ms: 32,
        projectiles_ms: 64,
        checkpoint_ms: 128,
    };
    let rate = if profile == "limited" {
        40 * 1024
    } else {
        512 * 1024
    };
    let schedule = Schedule::new(
        profile,
        seed ^ 0x7000,
        sources.len() as u64 * 16,
        warmup * 16,
    );
    let impairment_hash = schedule.hash();
    let blackout_end = schedule.blackout_end_ms;
    let mut whole_link = TimeLink::new(schedule.clone());
    let mut section_link = TimeLink::new(schedule);
    let trace = Rc::new(RefCell::new(Recorder::default()));
    let mut wire_hash = Sha256::new();
    let mut message_hash = Sha256::new();
    let mut controls = BTreeMap::new();
    let mut whole_controls = Controls::default();
    let mut reliable_mirror = ReliableMirror::default();
    let mut section_controls = Controls::default();
    let mut whole_recovery = [None; 3];
    let mut section_recovery = [None; 3];
    let mut truth_robots = 0_u64;
    let mut truth_projectiles = 0_u64;
    let mut shown_projectiles = [0_u64; 2];
    let mut state_ages: [Vec<f64>; 2] = Default::default();
    let mut host = PeerCodec::new(epoch, rate, false);
    host.joined(None);
    let mut client = ClientCodec::new(epoch, 10 * 1024, 12);
    let mut sender = Sender::new(rate);
    let mut receiver = Receiver::new(10 * 1024);
    if enabled {
        sender.queue.trace = Some(trace.clone());
        receiver.parts.trace = Some(trace.clone());
    }
    let mut whole = Observed::default();
    let mut sections = Observed::default();
    let mut whole_view = View::default();
    let mut whole_error = stage3::Error::default();
    let mut section_error = stage3::Error::default();
    let measured_ms = (sources.len() as u64 - warmup) * 16;
    for ms in (2..=sources.len() as u64 * 16).step_by(2) {
        let now = Duration::from_millis(ms);
        trace.borrow_mut().now_ms = ms;
        if ms == warmup * 16 + 2 {
            whole.reset_window();
            sections.reset_window();
        }
        if ms % 16 == 0 {
            let state = &sources[ms as usize / 16 - 1];
            let mut outbound = Outbound::new(ServerMessage::Snapshot(Box::new(state.clone())));
            outbound.periodic = true;
            if ms % 32 == 0 {
                host.send(&outbound, epoch + now, 0).unwrap();
            }
            sender.publish_cadenced(state, now, cadence).unwrap();
            if ms > warmup * 16
                && ((ms - warmup * 16 >= 1008 && (ms - warmup * 16 - 1008).is_multiple_of(4992))
                    || ms == warmup * 16 + 20_016)
            {
                assert!(!ms.is_multiple_of(32));
                controls.insert(state.snapshot_id, ms);
                for message in [
                    ServerMessage::Snapshot(Box::new(state.clone())),
                    ServerMessage::Pong {
                        nonce: state.snapshot_id,
                    },
                ] {
                    host.send(&Outbound::new(message.clone()), epoch + now, 0)
                        .unwrap();
                    sender.control(&message).unwrap();
                }
            }
        }
        // Both legs use the identical hand-advanced clock and captured source.
        while let Some(packet) = host.next(epoch + now).unwrap() {
            whole.down_bytes += packet.bytes.len() as u64;
            whole.down_packets += 1;
            hash_packet(&mut wire_hash, ms, "whole_down", &packet);
            whole_link.send(0, ms, packet);
        }
        for packet in whole_link.take(0, ms) {
            if let Some(message) = reliable_mirror.receive(&packet) {
                whole_controls.receive(&message, ms, &controls, sources, true);
            }
            if let Some(ClientEvent::Message(message)) =
                client.receive(&packet, epoch + now).unwrap()
            {
                hash_message(&mut message_hash, ms, "whole", &message);
                if control_message(&message, &controls) {
                    whole_controls.application_confirmations +=
                        u64::from(matches!(message, ServerMessage::Snapshot(_)));
                    continue;
                }
                if let ServerMessage::Snapshot(ref state) = message {
                    whole_view.checkpoint(state);
                }
                whole.message(message, sources);
            }
        }
        client.acknowledge(epoch + now).unwrap();
        while let Some(packet) = client.next(epoch + now).unwrap() {
            whole.up_bytes += packet.bytes.len() as u64;
            whole.up_packets += 1;
            hash_packet(&mut wire_hash, ms, "whole_up", &packet);
            whole_link.send(1, ms, packet);
        }
        for packet in whole_link.take(1, ms) {
            host.receive(&packet, epoch + now).unwrap();
        }
        while let Some(packet) = sender.next(now).unwrap() {
            sections.down_bytes += packet.bytes.len() as u64;
            sections.down_packets += 1;
            hash_packet(&mut wire_hash, ms, "section_down", &packet);
            section_link.send(0, ms, packet);
        }
        for packet in section_link.take(0, ms) {
            for message in receiver.receive(&packet, now).unwrap() {
                hash_message(&mut message_hash, ms, "section", &message);
                if section_controls.receive(&message, ms, &controls, sources, false) {
                    section_controls.application_confirmations +=
                        u64::from(matches!(message, ServerMessage::Snapshot(_)));
                } else {
                    sections.message(message, sources);
                }
            }
        }
        if let Some(packet) = receiver.feedback(now).unwrap() {
            sections.up_bytes += packet.bytes.len() as u64;
            sections.up_packets += 1;
            hash_packet(&mut wire_hash, ms, "section_up", &packet);
            section_link.send(1, ms, packet);
        }
        for packet in section_link.take(1, ms) {
            sender.feedback(&packet, now).unwrap();
        }
        sections.chassis_ns = receiver
            .presentation()
            .chassis
            .as_ref()
            .map_or(0, |p| p.capture * 16_000_000);
        whole.checkpoint_ns = whole.latest * 16_000_000;
        whole.chassis_ns = whole_view
            .chassis
            .as_ref()
            .map_or(0, |p| p.capture * 16_000_000);
        sections.checkpoint_ns = sections.latest * 16_000_000;
        sections.max_queue_bytes = sections.max_queue_bytes.max(sender.queued_bytes());
        sections.max_cache_bytes = sections.max_cache_bytes.max(receiver.cached_bytes());
        sections.max_partial_bytes = sections.max_partial_bytes.max(receiver.partial_bytes());
        assert!(
            receiver.pending() <= WINDOW
                && receiver.partial_bytes() <= FRAME_LIMIT
                && sender.queued_bytes() <= QUEUE_LIMIT
        );
        if enabled {
            trace.borrow_mut().sample(&sender, &receiver);
        }
        if let Some(end) = blackout_end
            && ms >= end
        {
            recover(&mut whole_recovery, end, ms, whole.latest, &whole_view);
            recover(
                &mut section_recovery,
                end,
                ms,
                sections.latest,
                receiver.presentation(),
            );
        }
        if ms > warmup * 16 {
            let truth_ns = sources[(ms as usize / 16).saturating_sub(1)].field.time_ns;
            for (i, latest) in [whole.latest, sections.latest].into_iter().enumerate() {
                if latest > 0 {
                    state_ages[i].push(
                        truth_ns.saturating_sub(sources[latest as usize - 1].field.time_ns) as f64
                            / 1e6,
                    );
                }
            }
            whole.sample(now);
            sections.sample(now);
            if ms % 16 == 0 {
                let truth = &sources[ms as usize / 16 - 1];
                truth_robots += truth.field.chassis.len() as u64;
                truth_projectiles += truth.field.projectiles.len() as u64;
                shown_projectiles[0] += whole_view
                    .projectiles
                    .as_ref()
                    .map_or(0, |p| p.values.len()) as u64;
                shown_projectiles[1] += receiver
                    .presentation()
                    .projectiles
                    .as_ref()
                    .map_or(0, |p| p.values.len()) as u64;
                whole_error.sample(&whole_view, truth);
                section_error.sample(receiver.presentation(), truth);
            }
        }
    }
    let mut digest = Sha256::new();
    for source in sources {
        digest.update(snapshot_codec::encode_player_message(
            &ServerMessage::Snapshot(Box::new(source.clone())),
        ));
    }
    let first_shots = if warmup == 0 {
        0
    } else {
        sources[warmup as usize - 1].field.shots_fired
    };
    let mut whole_json = whole.json();
    let mut sections_json = sections.json();
    whole_json["checkpoint_age_ms"] = distribution(&whole.checkpoint_ages);
    whole_json["available_chassis_age_ms"] = distribution(&whole.chassis_ages);
    sections_json["checkpoint_age_ms"] = distribution(&sections.checkpoint_ages);
    sections_json["available_chassis_age_ms"] = distribution(&sections.chassis_ages);
    let diagnostics = enabled.then(|| trace.borrow().json());
    if enabled {
        assert_eq!(trace.borrow().datagram_bytes, sender.stats().sent_bytes);
        assert_eq!(trace.borrow().datagrams, sender.stats().sent_packets);
    }
    serde_json::json!({
        "checkpoint_simulation_state_age_ms":{"whole":distribution(&state_ages[0]),"sections":distribution(&state_ages[1])},
        "upstream_lifetime_bytes_datagrams":receiver.feedback_counts(),
        "diagnostics": diagnostics,
        "compression": {"mode": format!("{:?}", crate::compression::selected().mode), "level": crate::compression::selected().level},
        "wire_sha256":format!("{:x}", wire_hash.finalize()),
        "message_sha256":format!("{:x}", message_hash.finalize()),
        "impairment_sha256":impairment_hash,
        "whole_controls":whole_controls.json(controls.len()), "section_controls":section_controls.json(controls.len()),
        "blackout_recovery_ms_checkpoint_chassis_projectiles":{"whole":whole_recovery,"sections":section_recovery},
        "coverage_denominators":{"truth_robot_samples":truth_robots,"truth_projectile_samples":truth_projectiles,"whole_shown_projectile_samples":shown_projectiles[0],"section_shown_projectile_samples":shown_projectiles[1]},
        "cadence": cadence, "whole_pose_error": whole_error.json(), "section_pose_error": section_error.json(),
        "seed": seed, "profile": profile, "players": sources[0].field.chassis.len(),
        "warmup_ms": warmup * 16, "measured_ms": measured_ms, "offered_source_captures": sources.len() as u64 - warmup,
        "captured_stream_sha256": format!("{:x}", digest.finalize()),
        "shots_launched_measured": sources.last().unwrap().field.shots_fired - first_shots,
        "downstream_budget_bytes_s": rate, "upstream_budget_bytes_s": 10 * 1024,
        "whole": whole_json, "sections": sections_json,
        "downstream_change_percent": 100. * (sections.down_bytes as f64 / whole.down_bytes as f64 - 1.),
        "p95_checkpoint_age_delta_ms": sections_json["checkpoint_age_ms"]["p95"].as_f64().unwrap() - whole_json["checkpoint_age_ms"]["p95"].as_f64().unwrap(),
        "p95_available_chassis_age_delta_ms": sections_json["available_chassis_age_ms"]["p95"].as_f64().unwrap() - whole_json["available_chassis_age_ms"]["p95"].as_f64().unwrap(),
        "sender_lifetime": sender.stats(),
        "scope": "T1 time-bin impairments and injected confirmation/Pong controls; one observing peer; no native wire/retransmission bytes, player input, owner anchors, render or prediction timings; held poses only"
    })
}

#[test]
fn instrumentation_preserves_packets_and_messages() {
    let sources = captures_at(2, 192, 71, 16);
    for profile in ["clean", "limited"] {
        let plain = trial(&sources, 71, profile, 0, false);
        let mut measured = trial(&sources, 71, profile, 0, true);
        assert!(measured["diagnostics"].is_object());
        measured["diagnostics"] = serde_json::Value::Null;
        assert_eq!(
            plain, measured,
            "instrumentation changed behavior: {profile}"
        );
    }
}
#[test]
#[ignore = "T1 diagnostic cases: build first, then run isolated"]
fn t1_diagnostics() {
    // Development IDs; never consume T2 or validation seed partitions.
    for players in [2, 12] {
        let sources = captures_at(players, 626 + 3750, 71, 16);
        for profile in ["clean", "limited", "blackout"] {
            let plain = trial(&sources, 71, profile, 626, false);
            let row = trial(&sources, 71, profile, 626, true);
            let mut comparable = row.clone();
            comparable["diagnostics"] = serde_json::Value::Null;
            assert_eq!(
                plain, comparable,
                "full diagnostic parity: {players} {profile}"
            );
            assert!(row["sections"]["delivered_checkpoints"].as_u64().unwrap() > 0);
            println!("RM_SECTION_T1 {row}");
        }
    }
}

#[test]
fn paused_source_time_does_not_hide_transport_freshness() {
    let mut sources = captures_at(2, 192, 71, 16);
    let paused_field = sources[63].field.clone();
    for state in &mut sources[64..] {
        state.field = paused_field.clone();
        state.paused = true;
    }
    let plain = trial(&sources, 71, "clean", 64, false);
    let mut measured = trial(&sources, 71, "clean", 64, true);
    assert!(
        measured["diagnostics"]["checkpoint_events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source_capture_ms"].as_u64().unwrap()
                > e["simulation_time_ns"].as_u64().unwrap() / 1_000_000)
    );
    assert_eq!(
        measured["checkpoint_simulation_state_age_ms"]["sections"]["p95"],
        0.0
    );
    assert!(
        measured["sections"]["checkpoint_age_ms"]["p95"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    measured["diagnostics"] = serde_json::Value::Null;
    assert_eq!(plain, measured);
}
