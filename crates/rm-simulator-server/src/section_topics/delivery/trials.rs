// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Same-capture stage-2 trials against the production whole-envelope codecs.
//! No sockets, CAD, owner anchors, input traffic or rendered presentation.
use super::*;
use crate::{
    host::Outbound,
    protocol::Command,
    scripted_link::{Impairment, Link},
    simulation::Simulation,
    snapshot_codec,
    udp_codec::{ClientCodec, ClientEvent, PeerCodec},
};
use rm_simulator_world::{
    ChassisCommand, ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, RefereeConfig, Team,
};
use sha2::{Digest, Sha256};
use std::time::Instant;

#[derive(Default)]
struct Observed {
    down_bytes: u64,
    down_packets: u64,
    up_bytes: u64,
    up_packets: u64,
    snapshots: u64,
    latest: u64,
    checkpoint_ns: u64,
    chassis_ns: u64,
    checkpoint_ages: Vec<f64>,
    chassis_ages: Vec<f64>,
    missing_samples: u64,
    max_queue_bytes: usize,
    max_cache_bytes: usize,
    max_partial_bytes: usize,
}
impl Observed {
    fn reset_window(&mut self) {
        *self = Self {
            latest: self.latest,
            checkpoint_ns: self.checkpoint_ns,
            chassis_ns: self.chassis_ns,
            ..Default::default()
        };
    }
    fn message(&mut self, message: ServerMessage, sources: &[SimulationState]) {
        let ServerMessage::Snapshot(state) = message else {
            panic!("unexpected trial control")
        };
        let expected =
            snapshot_codec::decode_player_message(&snapshot_codec::encode_player_message(
                &ServerMessage::Snapshot(Box::new(sources[state.snapshot_id as usize - 1].clone())),
            ))
            .unwrap();
        assert_eq!(
            ServerMessage::Snapshot(state.clone()),
            expected,
            "checkpoint reconstruction drift"
        );
        assert!(
            state.snapshot_id > self.latest,
            "checkpoint regression or duplicate promotion"
        );
        self.latest = state.snapshot_id;
        self.checkpoint_ns = state.field.time_ns;
        self.chassis_ns = self.chassis_ns.max(state.field.time_ns);
        self.snapshots += 1;
    }
    fn sample(&mut self, now: Duration) {
        let ns = now.as_nanos() as u64;
        self.checkpoint_ages
            .push(ns.saturating_sub(self.checkpoint_ns) as f64 / 1e6);
        self.chassis_ages
            .push(ns.saturating_sub(self.chassis_ns) as f64 / 1e6);
        self.missing_samples += u64::from(self.latest == 0);
    }
    fn json(&self) -> serde_json::Value {
        let percentiles = |values: &[f64]| {
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);
            serde_json::json!({ "p50": sorted[sorted.len() / 2], "p95": sorted[sorted.len() * 95 / 100], "max": sorted.last().unwrap() })
        };
        serde_json::json!({
            "downstream_bytes": self.down_bytes, "downstream_datagrams": self.down_packets,
            "upstream_bytes": self.up_bytes, "upstream_datagrams": self.up_packets,
            "delivered_checkpoints": self.snapshots, "latest_checkpoint": self.latest,
            "checkpoint_age_ms": percentiles(&self.checkpoint_ages),
            "available_chassis_age_ms": percentiles(&self.chassis_ages),
            "samples_without_checkpoint": self.missing_samples,
            "peak_queued_payload_bytes": self.max_queue_bytes, "peak_cached_section_bytes": self.max_cache_bytes,
            "peak_partial_frame_bytes": self.max_partial_bytes,
        })
    }
}
fn captures(players: u32, count: u64, seed: u64) -> Vec<SimulationState> {
    captures_at(players, count, seed, 32)
}
fn captures_at(players: u32, count: u64, seed: u64, interval_ms: u64) -> Vec<SimulationState> {
    let chassis = ChassisConfig::default();
    let mut config = FieldConfig {
        referee: Some(RefereeConfig::alternating(2, 2)),
        chassis: (0..players)
            .map(|i| ChassisPlacement {
                team: if i % 2 == 0 { Team::Red } else { Team::Blue },
                config: chassis.clone(),
                spawn: Pose::at([
                    (i % 4) as f64 * 2. - 3.,
                    (i / 4) as f64 * 2. - 2.,
                    chassis.rest_height_m(),
                ]),
            })
            .collect(),
        ..Default::default()
    };
    config.runes.push(config.runes[0]);
    let mut simulation = Simulation::new(Field::new(&config).unwrap(), false);
    let initial = simulation.state().field.chassis[0].pose.translation_m;
    let mut rng = seed;
    let mut sources = Vec::new();
    for index in 0..count {
        if (index * interval_ms).is_multiple_of(256) {
            for chassis in 0..players {
                rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                let turn = (rng >> 32) as f64 / u32::MAX as f64;
                simulation
                    .apply(&Command::Chassis {
                        chassis,
                        command: ChassisCommand {
                            forward_m_s: 1.,
                            yaw_rate_rad_s: 0.2 + turn * 0.4,
                            aim_yaw_rad: turn * 0.6,
                            aim_pitch_rad: 0.15,
                            ..Default::default()
                        },
                    })
                    .unwrap();
            }
        }
        if (index * interval_ms).is_multiple_of(96) {
            for shooter in 0..2 {
                simulation
                    .apply(&Command::Fire {
                        shooter,
                        timing: None,
                    })
                    .unwrap();
            }
        }
        simulation.step(interval_ms).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = index + 1;
        sources.push(state);
    }
    let last = sources.last().unwrap();
    assert!(last.field.shots_fired > 0);
    assert!(
        initial
            .into_iter()
            .zip(last.field.chassis[0].pose.translation_m)
            .any(|(a, b)| (a - b).abs() > 0.01)
    );
    sources
}
fn rules(profile: &str, warmup: u64) -> (u32, Impairment) {
    let rules = match profile {
        "clean" => Impairment::default(),
        "rtt40" => Impairment {
            delay: Duration::from_millis(20),
            ..Default::default()
        },
        "rtt100_loss" => Impairment {
            delay: Duration::from_millis(50),
            jitter: Duration::from_millis(10),
            loss: 0.03,
            duplicate: 0.02,
            reorder_every: 17,
            reorder_hold: 2,
            recovery: Duration::from_millis(100),
            ..Default::default()
        },
        "blackout" => Impairment {
            delay: Duration::from_millis(20),
            recovery: Duration::from_millis(100),
            blackout: Some((
                Duration::from_millis(warmup * 32 + 20_000),
                Duration::from_millis(warmup * 32 + 20_500),
            )),
            ..Default::default()
        },
        "limited" => Impairment {
            delay: Duration::from_millis(50),
            jitter: Duration::from_millis(10),
            loss: 0.01,
            recovery: Duration::from_millis(100),
            ..Default::default()
        },
        _ => panic!("unknown trial profile"),
    };
    (
        if profile == "limited" {
            40 * 1024
        } else {
            512 * 1024
        },
        rules,
    )
}
fn trial(sources: &[SimulationState], seed: u64, profile: &str, warmup: u64) -> serde_json::Value {
    let epoch = Instant::now();
    let (rate, rules) = rules(profile, warmup);
    let mut whole_link = Link::new(epoch, seed, rules.clone(), rules.clone());
    let mut section_link = Link::new(epoch, seed, rules.clone(), rules);
    let mut host = PeerCodec::new(epoch, rate, false);
    host.joined(None);
    let mut client = ClientCodec::new(epoch, 10 * 1024, 12);
    let mut sender = Sender::new(rate);
    let mut receiver = Receiver::new(10 * 1024);
    let mut whole = Observed::default();
    let mut sections = Observed::default();
    let measured_ms = (sources.len() as u64 - warmup) * 32;
    for ms in (2..=sources.len() as u64 * 32).step_by(2) {
        let now = Duration::from_millis(ms);
        if ms == warmup * 32 + 2 {
            whole.reset_window();
            sections.reset_window();
        }
        if ms % 32 == 0 {
            let state = &sources[ms as usize / 32 - 1];
            let mut outbound = Outbound::new(ServerMessage::Snapshot(Box::new(state.clone())));
            outbound.periodic = true;
            host.send(&outbound, epoch + now, 0).unwrap();
            sender.publish(state, now).unwrap();
        }
        // Both legs use the identical hand-advanced clock and captured source.
        while let Some(packet) = host.next(epoch + now).unwrap() {
            whole.down_bytes += packet.bytes.len() as u64;
            whole.down_packets += 1;
            whole_link.send_to_client(epoch + now, packet);
        }
        for packet in whole_link.take_to_client(epoch + now) {
            if let Some(ClientEvent::Message(message)) =
                client.receive(&packet, epoch + now).unwrap()
            {
                whole.message(message, sources);
            }
        }
        client.acknowledge(epoch + now).unwrap();
        while let Some(packet) = client.next(epoch + now).unwrap() {
            whole.up_bytes += packet.bytes.len() as u64;
            whole.up_packets += 1;
            whole_link.send_to_host(epoch + now, packet);
        }
        for packet in whole_link.take_to_host(epoch + now) {
            host.receive(&packet, epoch + now).unwrap();
        }
        while let Some(packet) = sender.next(now).unwrap() {
            sections.down_bytes += packet.bytes.len() as u64;
            sections.down_packets += 1;
            section_link.send_to_client(epoch + now, packet);
        }
        for packet in section_link.take_to_client(epoch + now) {
            for message in receiver.receive(&packet, now).unwrap() {
                sections.message(message, sources);
            }
        }
        if let Some(packet) = receiver.feedback(now).unwrap() {
            sections.up_bytes += packet.bytes.len() as u64;
            sections.up_packets += 1;
            section_link.send_to_host(epoch + now, packet);
        }
        for packet in section_link.take_to_host(epoch + now) {
            sender.feedback(&packet, now).unwrap();
        }
        sections.chassis_ns = receiver
            .available_time_ns(Topic::Chassis)
            .unwrap_or(sections.chassis_ns);
        sections.max_queue_bytes = sections.max_queue_bytes.max(sender.queued_bytes());
        sections.max_cache_bytes = sections.max_cache_bytes.max(receiver.cached_bytes());
        sections.max_partial_bytes = sections.max_partial_bytes.max(receiver.partial_bytes());
        assert!(
            receiver.pending() <= WINDOW
                && receiver.partial_bytes() <= FRAME_LIMIT
                && sender.queued_bytes() <= QUEUE_LIMIT
        );
        if ms > warmup * 32 {
            whole.sample(now);
            sections.sample(now);
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
    let whole_json = whole.json();
    let sections_json = sections.json();
    serde_json::json!({
        "seed": seed, "profile": profile, "players": sources[0].field.chassis.len(),
        "warmup_ms": warmup * 32, "measured_ms": measured_ms, "offered_checkpoints": sources.len() as u64 - warmup,
        "captured_stream_sha256": format!("{:x}", digest.finalize()),
        "shots_launched_measured": sources.last().unwrap().field.shots_fired - first_shots,
        "downstream_budget_bytes_s": rate, "upstream_budget_bytes_s": 10 * 1024,
        "whole": whole_json, "sections": sections_json,
        "downstream_change_percent": 100. * (sections.down_bytes as f64 / whole.down_bytes as f64 - 1.),
        "p95_checkpoint_age_delta_ms": sections_json["checkpoint_age_ms"]["p95"].as_f64().unwrap() - whole_json["checkpoint_age_ms"]["p95"].as_f64().unwrap(),
        "p95_available_chassis_age_delta_ms": sections_json["available_chassis_age_ms"]["p95"].as_f64().unwrap() - whole_json["available_chassis_age_ms"]["p95"].as_f64().unwrap(),
        "sender_lifetime": sender.stats(),
        "scope": "scripted application datagrams, one observing peer, synthetic field; no native wire, inputs, owner anchors, render/prediction timings"
    })
}
#[test]
fn clean_same_capture_trial_delivers_current_coherent_checkpoints() {
    let sources = captures(2, 125, 1);
    let row = trial(&sources, 1, "clean", 0);
    assert!(row["whole"]["delivered_checkpoints"].as_u64().unwrap() >= 120);
    assert!(
        row["sections"]["delivered_checkpoints"].as_u64().unwrap() >= 120,
        "{row}"
    );
}
#[test]
#[ignore = "stage-2 five-seed byte/age trial; build first and run in isolation"]
fn stage2_trials() {
    let number = |name: &str, default| {
        std::env::var(name)
            .ok()
            .map(|s| s.parse::<u64>().unwrap())
            .unwrap_or(default)
    };
    let frames = number("RM_SECTION_TRIAL_FRAMES", 1875);
    let warmup = number("RM_SECTION_TRIAL_WARMUP", 313);
    let seeds = number("RM_SECTION_TRIAL_SEEDS", 5);
    assert!(frames > 0 && seeds > 0);
    for seed in 1..=seeds {
        for players in if seed % 2 == 0 { [12, 2] } else { [2, 12] } {
            let sources = captures(players, warmup + frames, seed);
            let mut profiles = ["clean", "rtt40", "rtt100_loss", "blackout", "limited"];
            let mut order = seed ^ players as u64;
            for i in (1..profiles.len()).rev() {
                order = order.wrapping_mul(6364136223846793005).wrapping_add(1);
                profiles.swap(i, (order >> 32) as usize % (i + 1));
            }
            for profile in profiles {
                println!(
                    "RM_SECTION_STAGE2 {}",
                    trial(&sources, seed, profile, warmup)
                );
            }
        }
    }
}

#[path = "trials/stage3.rs"]
mod stage3;

#[path = "trials/tuning.rs"]
pub(super) mod tuning;
