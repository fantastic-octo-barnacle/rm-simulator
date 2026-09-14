// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Cadence matrix on shared captures; presentation is never restored.
use super::super::presentation::{Cadence, View};
use super::*;

#[derive(Default)]
pub(super) struct Error {
    body_m: Vec<f64>,
    aim_rad: Vec<f64>,
    projectile_m: Vec<f64>,
    projectile_age_ms: Vec<f64>,
    missing_robots: u64,
    missing_projectiles: u64,
    stale_projectiles: u64,
}
impl Error {
    pub(super) fn sample(&mut self, view: &View, truth: &SimulationState) {
        for robot in &truth.field.chassis {
            if let Some(shown) = view.chassis.as_ref().and_then(|p| {
                p.values
                    .iter()
                    .find(|r| r.id == robot.id && r.placement_revision == robot.placement_revision)
            }) {
                self.body_m
                    .push(distance(shown.pose.translation_m, robot.pose.translation_m));
                let dot: f64 = shown
                    .turret
                    .rotation_wxyz
                    .iter()
                    .zip(robot.turret.rotation_wxyz)
                    .map(|(a, b)| a * b)
                    .sum();
                self.aim_rad.push(2. * dot.abs().clamp(0., 1.).acos());
            } else {
                self.missing_robots += 1;
            }
        }
        if let Some(poses) = &view.projectiles {
            self.projectile_age_ms
                .push(truth.field.time_ns.saturating_sub(poses.time_ns) as f64 / 1e6);
            for ball in &truth.field.projectiles {
                if let Some(shown) = poses.values.iter().find(|r| r.0 == ball.id) {
                    self.projectile_m.push(distance(shown.1, ball.position_m));
                } else {
                    self.missing_projectiles += 1;
                }
            }
            self.stale_projectiles += poses
                .values
                .iter()
                .filter(|p| !truth.field.projectiles.iter().any(|b| b.id == p.0))
                .count() as u64;
        } else {
            self.missing_projectiles += truth.field.projectiles.len() as u64;
        }
    }
    pub(super) fn json(&self) -> serde_json::Value {
        fn stats(values: &[f64]) -> serde_json::Value {
            if values.is_empty() {
                return serde_json::Value::Null;
            }
            let mut v = values.to_vec();
            v.sort_by(f64::total_cmp);
            serde_json::json!({"samples":v.len(), "p95":v[v.len()*95/100], "max":v.last(), "mean":v.iter().sum::<f64>()/v.len() as f64})
        }
        serde_json::json!({"body_m":stats(&self.body_m), "aim_rad":stats(&self.aim_rad), "projectile_m":stats(&self.projectile_m), "projectile_age_ms":stats(&self.projectile_age_ms), "missing_robot_samples":self.missing_robots, "missing_projectile_samples":self.missing_projectiles, "stale_projectile_samples":self.stale_projectiles})
    }
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}
fn trial(
    sources: &[SimulationState],
    seed: u64,
    profile: &str,
    warmup: u64,
    cadence: Cadence,
) -> serde_json::Value {
    let epoch = Instant::now();
    let (rate, rules) = rules(profile, warmup / 2);
    let mut whole_link = Link::new(epoch, seed, rules.clone(), rules.clone());
    let mut section_link = Link::new(epoch, seed, rules.clone(), rules);
    let mut host = PeerCodec::new(epoch, rate, false);
    host.joined(None);
    let mut client = ClientCodec::new(epoch, 10 * 1024, 12);
    let mut sender = Sender::new(rate);
    let mut receiver = Receiver::new(10 * 1024);
    let mut whole = Observed::default();
    let mut sections = Observed::default();
    let mut whole_view = View::default();
    let mut whole_error = Error::default();
    let mut section_error = Error::default();
    let measured_ms = (sources.len() as u64 - warmup) * 16;
    for ms in (2..=sources.len() as u64 * 16).step_by(2) {
        let now = Duration::from_millis(ms);
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
            .presentation()
            .chassis
            .as_ref()
            .map_or(0, |p| p.time_ns);
        sections.max_queue_bytes = sections.max_queue_bytes.max(sender.queued_bytes());
        sections.max_cache_bytes = sections.max_cache_bytes.max(receiver.cached_bytes());
        sections.max_partial_bytes = sections.max_partial_bytes.max(receiver.partial_bytes());
        assert!(
            receiver.pending() <= WINDOW
                && receiver.partial_bytes() <= FRAME_LIMIT
                && sender.queued_bytes() <= QUEUE_LIMIT
        );
        if ms > warmup * 16 {
            whole.sample(now);
            sections.sample(now);
            if ms % 16 == 0 {
                let truth = &sources[ms as usize / 16 - 1];
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
    let whole_json = whole.json();
    let sections_json = sections.json();
    serde_json::json!({
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
        "scope": "scripted application datagrams, one observing peer, synthetic field; no native wire, inputs, owner anchors, render/prediction timings; headless zero-order-held poses, errors sampled against exact 16ms captures"
    })
}
#[test]
fn cadence_smoke() {
    let sources = captures_at(2, 128, 1, 16);
    let row = trial(
        &sources,
        1,
        "clean",
        0,
        Cadence {
            chassis_ms: 16,
            projectiles_ms: 32,
            checkpoint_ms: 128,
        },
    );
    assert!(row["sections"]["delivered_checkpoints"].as_u64().unwrap() >= 15);
    assert!(
        row["sections"]["available_chassis_age_ms"]["p95"]
            .as_f64()
            .unwrap()
            < row["sections"]["checkpoint_age_ms"]["p95"]
                .as_f64()
                .unwrap()
    );
}
#[test]
#[ignore = "stage-3 cadence matrix; run isolated after building"]
fn stage3_trials() {
    let number = |name: &str, default| {
        std::env::var(name)
            .ok()
            .map(|s| s.parse::<u64>().unwrap())
            .unwrap_or(default)
    };
    let frames = number("RM_SECTION_TRIAL_FRAMES", 3750);
    let warmup = number("RM_SECTION_TRIAL_WARMUP", 626);
    let seeds = number("RM_SECTION_TRIAL_SEEDS", 5);
    for seed in 1..=seeds {
        for players in [2, 12] {
            let sources = captures_at(players, frames + warmup, seed, 16);
            let screening = std::env::var("RM_SECTION_STAGE3_SCREEN").is_ok();
            let profiles: &[&str] = if screening {
                &["clean", "rtt100_loss", "limited"]
            } else {
                &["clean", "rtt40", "rtt100_loss", "blackout", "limited"]
            };
            for profile in profiles {
                for chassis_ms in [16, 32] {
                    for projectiles_ms in [32, 64] {
                        for checkpoint_ms in [32, 64, 128] {
                            if let Ok(selected) = std::env::var("RM_SECTION_STAGE3_CADENCE")
                                && selected
                                    != format!("{chassis_ms}/{projectiles_ms}/{checkpoint_ms}")
                            {
                                continue;
                            }
                            println!(
                                "RM_SECTION_STAGE3 {}",
                                trial(
                                    &sources,
                                    seed,
                                    profile,
                                    warmup,
                                    Cadence {
                                        chassis_ms,
                                        projectiles_ms,
                                        checkpoint_ms
                                    }
                                )
                            );
                        }
                    }
                }
            }
        }
    }
}
