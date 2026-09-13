// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Reproducible JSON snapshot workload; no CAD assets or wall-clock pacing.
//! Measures the live player path: the compact independent checkpoint and the
//! acknowledged UDP baseline codec that carries it.
use rm_simulator_server::{layout::ChassisSpawner, protocol::Command, simulation::Simulation};
use rm_simulator_world::{ChassisCommand, ChassisConfig, Field, FieldConfig, RefereeConfig, Team};

fn main() {
    for (players, firing) in [(0, false), (2, false), (12, false), (2, true)] {
        let mut config = FieldConfig {
            referee: Some(RefereeConfig::alternating(2, 2)),
            ..Default::default()
        };
        config.runes.push(config.runes[0]);
        let mut simulation =
            Simulation::new(Field::new(&config).unwrap(), false).with_spawner(ChassisSpawner {
                config: ChassisConfig::default(),
                terrain: None,
            });
        for i in 0..players {
            let chassis = simulation
                .spawn_chassis(if i % 2 == 0 { Team::Red } else { Team::Blue })
                .unwrap();
            simulation
                .apply(&Command::Chassis {
                    chassis,
                    command: ChassisCommand {
                        forward_m_s: 1.,
                        yaw_rate_rad_s: 0.4,
                        ..Default::default()
                    },
                })
                .unwrap();
        }
        let mut encoder = rm_simulator_server::udp_snapshot::Encoder::default();
        let mut decoder = rm_simulator_server::udp_snapshot::Decoder::default();
        let mut encode_us = 0;
        let mut decode_us = 0;
        let mut total = 0;
        let mut udp_bytes = 0;
        let mut udp_encode_us = 0;
        let mut sections = std::collections::BTreeMap::<String, usize>::new();
        for frame in 0..250 {
            simulation.step(16).unwrap();
            if firing && frame % 8 == 0 {
                simulation
                    .apply(&Command::Fire {
                        shooter: 0,
                        timing: None,
                    })
                    .unwrap();
            }
            let mut state = simulation.state();
            state.snapshot_id = frame + 1;
            let message =
                rm_simulator_server::protocol::ServerMessage::Snapshot(Box::new(state.clone()));
            let started = std::time::Instant::now();
            let compact = rm_simulator_server::snapshot_codec::encode_player_message(&message);
            let wire = encoder.snapshot(state.input_epoch, &compact).unwrap();
            encode_us += started.elapsed().as_micros();
            let started = std::time::Instant::now();
            let parsed = rm_simulator_server::udp_snapshot::parse(
                &miniz_oxide::inflate::decompress_to_vec(&wire).unwrap(),
            )
            .unwrap()
            .unwrap();
            let (received, feedback) = decoder.receive(parsed).unwrap();
            if let Some(rm_simulator_server::protocol::ServerMessage::Snapshot(state)) = received {
                assert_eq!(state.field.tick, (frame + 1) * 16);
            }
            decode_us += started.elapsed().as_micros();
            if let Some(feedback) = feedback
                && let Some(retire) = encoder.feedback(feedback)
                && let (_, Some(retired)) = decoder.receive(retire).unwrap()
            {
                encoder.feedback(retired);
            }
            let started = std::time::Instant::now();
            let json = serde_json::to_vec(&message).unwrap();
            let compressed = miniz_oxide::deflate::compress_to_vec(&json, 1);
            udp_encode_us += started.elapsed().as_micros();
            udp_bytes += compressed.len() + compressed.len().div_ceil(1000) * 21;
            total += serde_json::to_vec(&state).unwrap().len();
            let value = serde_json::to_value(&state.field).unwrap();
            for (name, value) in value.as_object().unwrap() {
                *sections.entry(name.clone()).or_default() +=
                    serde_json::to_vec(value).unwrap().len();
            }
        }
        println!(
            "{players} moving players, firing={firing}: mean {} bytes/state, {} bytes/s at 62.5 Hz",
            total / 250,
            total / 4
        );
        println!(
            "  GNS application bytes/s: {}, encoding mean {} us/update (excludes UDP/GNS headers, retransmissions and Pongs)",
            udp_bytes / 4,
            udp_encode_us / 250
        );
        println!(
            "  UDP baselines: {} deltas, independent/sent bytes {}/{}",
            encoder.deltas, encoder.full_bytes, encoder.sent_bytes
        );
        println!(
            "  saved {:.1}%, encoding mean {} us/update",
            100. * (1. - encoder.sent_bytes as f64 / encoder.full_bytes as f64),
            encode_us / 250
        );
        println!("  decoding mean {} us/update", decode_us / 250);
        println!(
            "  mean section bytes: {:?}",
            sections
                .into_iter()
                .map(|(k, v)| (k, v / 250))
                .collect::<Vec<_>>()
        );
    }
}
