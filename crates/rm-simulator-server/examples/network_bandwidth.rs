// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Reproducible JSON snapshot workload; no CAD assets or wall-clock pacing.
//! Measures the live player path: the compact independent checkpoint and the
//! acknowledged UDP baseline codec that carries it.
use rm_simulator_server::{protocol::Command, workload};

fn main() {
    let step_ms = 16;
    let frames = 250;
    let seconds = (frames * step_ms) as f64 / 1000.;
    for (players, firing) in [(0, false), (2, false), (12, false), (2, true)] {
        // One builder, shared with the probe and the other measurement examples,
        // so every harness drives the same field.
        let (mut simulation, chassis) = workload::simulation(players);
        for &chassis in &chassis {
            simulation
                .apply(&Command::Chassis {
                    chassis,
                    command: workload::constant_drive(),
                })
                .unwrap();
        }
        let mut encoder = rm_simulator_server::udp_snapshot::Encoder::default();
        let mut decoder = rm_simulator_server::udp_snapshot::Decoder::default();
        let mut encode_us = 0;
        let mut decode_us = 0;
        let mut total = 0;
        let mut sections = std::collections::BTreeMap::<String, usize>::new();
        for frame in 0..frames {
            simulation.step(step_ms).unwrap();
            if firing && frame % (128 / step_ms) == 0 {
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
                &rm_simulator_server::compression::decompress(&wire, 4 << 20).unwrap(),
            )
            .unwrap()
            .unwrap();
            let (received, feedback) = decoder.receive(parsed).unwrap();
            if let Some(rm_simulator_server::protocol::ServerMessage::Snapshot(state)) = received {
                assert_eq!(state.field.tick, (frame + 1) * step_ms);
            }
            decode_us += started.elapsed().as_micros();
            if let Some(feedback) = feedback
                && let Some(retire) = encoder.feedback(feedback)
                && let (_, Some(retired)) = decoder.receive(retire).unwrap()
            {
                encoder.feedback(retired);
            }
            total += serde_json::to_vec(&state).unwrap().len();
            let value = serde_json::to_value(&state.field).unwrap();
            for (name, value) in value.as_object().unwrap() {
                *sections.entry(name.clone()).or_default() +=
                    serde_json::to_vec(value).unwrap().len();
            }
        }
        println!(
            "{players} moving players, firing={firing}: mean {} bytes/state, {:.1} bytes/s at {:.3} Hz",
            total / frames as usize,
            total as f64 / seconds,
            1000. / step_ms as f64
        );
        println!(
            "  UDP baselines: {} deltas, independent/sent bytes {}/{}",
            encoder.deltas, encoder.full_bytes, encoder.sent_bytes
        );
        println!(
            "  saved {:.1}%, encoding mean {} us/update",
            100. * (1. - encoder.sent_bytes as f64 / encoder.full_bytes as f64),
            encode_us / frames as u128
        );
        println!("  decoding mean {} us/update", decode_us / frames as u128);
        println!(
            "  mean section bytes: {:?}",
            sections
                .into_iter()
                .map(|(k, v)| (k, v / frames as usize))
                .collect::<Vec<_>>()
        );
    }
}
