// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Offline same-cadence comparison: identical captured states, immediate ACKs,
//! lossless delivery, production baseline selection and RMG1 fragmentation.
//! Counts both directions including baseline retirement; excludes UDP/GNS native
//! overhead, inputs, owner anchors, live hit messages, sockets and pacing.
use clap::Parser;
use rm_simulator_server::{
    protocol::{Command, ServerMessage},
    section_topics::{self, Frame},
    simulation::{Simulation, SimulationState},
    snapshot_codec, udp_snapshot,
};
use rm_simulator_world::{
    ChassisCommand, ChassisConfig, ChassisPlacement, Field, FieldConfig, Pose, RefereeConfig, Team,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, VecDeque},
    io,
};

#[derive(Parser)]
struct Args {
    /// Measured 32 ms captures (1875 = 60 seconds).
    #[arg(long, default_value_t = 1875)]
    frames: u64,
    /// Warm-up captures, excluded from byte totals (313 = 10.016 seconds).
    #[arg(long, default_value_t = 313)]
    warmup_frames: u64,
    /// Number of deterministic workload seeds, starting at 1.
    #[arg(long, default_value_t = 5)]
    seeds: u64,
}
#[derive(Default, Serialize)]
struct Totals {
    downstream_bytes: u64,
    downstream_fragments: u64,
    upstream_bytes: u64,
    upstream_datagrams: u64,
    checkpoints: u64,
    max_cached_payload_bytes: usize,
    section_downstream_bytes: BTreeMap<String, u64>,
}
impl Totals {
    fn frame(&mut self, frame: &Frame) -> io::Result<()> {
        // Identity changes header contents, never its length. No network in this probe.
        let packets = frame.packets(1)?;
        let bytes = packets
            .iter()
            .map(|packet| packet.len() as u64)
            .sum::<u64>();
        self.downstream_bytes += bytes;
        self.downstream_fragments += packets.len() as u64;
        let name = frame
            .topic()
            .map_or("manifest_or_whole", |topic| topic.name());
        *self
            .section_downstream_bytes
            .entry(name.into())
            .or_default() += bytes;
        Ok(())
    }
    fn feedback(&mut self, bytes: &[u8]) {
        self.upstream_bytes += bytes.len() as u64;
        self.upstream_datagrams += 1;
    }
}
fn inflate(bytes: &[u8]) -> io::Result<udp_snapshot::Wire> {
    let raw = rm_simulator_server::compression::decompress(bytes, 4 << 20)
        .map_err(|_| io::Error::other("invalid compressed probe frame"))?;
    udp_snapshot::parse(&raw)?.ok_or_else(|| io::Error::other("missing baseline envelope"))
}
fn whole(
    encoder: &mut udp_snapshot::Encoder,
    decoder: &mut udp_snapshot::Decoder,
    source: &SimulationState,
    raw: &[u8],
    expected: &ServerMessage,
    totals: &mut Totals,
) -> io::Result<()> {
    let mut queue = VecDeque::from([Frame {
        bytes: encoder.snapshot(source.input_epoch, raw)?,
        reliable: false,
    }]);
    if let Some(retire) = encoder.resend_retire() {
        queue.push_back(Frame {
            bytes: udp_snapshot::encode(retire),
            reliable: true,
        });
    }
    while let Some(frame) = queue.pop_front() {
        totals.frame(&frame)?;
        let (received, feedback) = decoder.receive(inflate(&frame.bytes)?)?;
        if let Some(received) = received {
            assert_eq!(&received, expected);
            totals.checkpoints += 1;
        }
        if let Some(feedback) = feedback {
            totals.feedback(&feedback.encode());
            if let Some(retire) = encoder.feedback(feedback) {
                queue.push_back(Frame {
                    bytes: udp_snapshot::encode(retire),
                    reliable: true,
                });
            }
        }
    }
    Ok(())
}
fn sections(
    encoder: &mut section_topics::Encoder,
    decoder: &mut section_topics::Decoder,
    source: &SimulationState,
    expected: &ServerMessage,
    totals: &mut Totals,
) -> io::Result<()> {
    let mut queue: VecDeque<_> = encoder.capture(source)?.into();
    while let Some(frame) = queue.pop_front() {
        totals.frame(&frame)?;
        let (received, feedback) = decoder.receive(&frame)?;
        if let Some(received) = received {
            assert_eq!(&ServerMessage::Snapshot(Box::new(received)), expected);
            totals.checkpoints += 1;
        }
        if let Some(feedback) = feedback {
            totals.feedback(&feedback);
            if let Some(retire) = encoder.feedback(&feedback)? {
                queue.push_back(retire);
            }
        }
    }
    totals.max_cached_payload_bytes = totals.max_cached_payload_bytes.max(decoder.cached_bytes());
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.frames == 0 || args.seeds == 0 {
        return Err("frames and seeds must be positive".into());
    }
    assert_eq!(rm_simulator_world::tick_ns(), 1_000_000);
    for seed in 1..=args.seeds {
        let mut cases = [
            ("idle", 1, false, false),
            ("drive", 2, true, false),
            ("fire", 2, true, true),
            ("twelve", 12, true, true),
        ];
        // Reproducibly shuffle workload order; every variant consumes the same capture.
        let mut rng = seed;
        for i in (1..cases.len()).rev() {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            cases.swap(i, (rng >> 32) as usize % (i + 1));
        }
        for (name, players, moving, firing) in cases {
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
            let mut simulation = Simulation::new(Field::new(&config)?, false);
            let initial = simulation.state().field.chassis[0].pose.translation_m;
            let mut whole_encoder = udp_snapshot::Encoder::default();
            let mut whole_decoder = udp_snapshot::Decoder::default();
            let mut section_encoder = section_topics::Encoder::default();
            let mut section_decoder = section_topics::Decoder::default();
            let mut control = Totals::default();
            let mut candidate = Totals::default();
            let mut start_shots = 0;
            for index in 0..args.warmup_frames + args.frames {
                if moving && index % 8 == 0 {
                    for chassis in 0..players {
                        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let turn = (rng >> 32) as f64 / u32::MAX as f64;
                        simulation.apply(&Command::Chassis {
                            chassis,
                            command: ChassisCommand {
                                forward_m_s: 1.,
                                yaw_rate_rad_s: 0.2 + turn * 0.4,
                                aim_yaw_rad: turn * 0.6,
                                aim_pitch_rad: 0.15,
                                ..Default::default()
                            },
                        })?;
                    }
                }
                if firing && index % 3 == 0 {
                    for shooter in 0..2 {
                        simulation.apply(&Command::Fire {
                            shooter,
                            timing: None,
                        })?;
                    }
                }
                simulation.step(32)?;
                let mut state = simulation.state();
                state.snapshot_id = index + 1;
                let raw = snapshot_codec::encode_player_message(&ServerMessage::Snapshot(
                    Box::new(state.clone()),
                ));
                let expected = snapshot_codec::decode_player_message(&raw)?;
                // Alternate codec execution order, though this probe makes no timing claim.
                if (index + seed) % 2 == 0 {
                    whole(
                        &mut whole_encoder,
                        &mut whole_decoder,
                        &state,
                        &raw,
                        &expected,
                        &mut control,
                    )?;
                    sections(
                        &mut section_encoder,
                        &mut section_decoder,
                        &state,
                        &expected,
                        &mut candidate,
                    )?;
                } else {
                    sections(
                        &mut section_encoder,
                        &mut section_decoder,
                        &state,
                        &expected,
                        &mut candidate,
                    )?;
                    whole(
                        &mut whole_encoder,
                        &mut whole_decoder,
                        &state,
                        &raw,
                        &expected,
                        &mut control,
                    )?;
                }
                if index + 1 == args.warmup_frames {
                    control = Totals::default();
                    candidate = Totals::default();
                    start_shots = state.field.shots_fired;
                }
            }
            assert_eq!(control.checkpoints, args.frames);
            assert_eq!(candidate.checkpoints, args.frames);
            let final_state = simulation.state();
            let shots = final_state.field.shots_fired - start_shots;
            if firing {
                assert!(shots > 0, "workload must actually launch");
            }
            let final_position = final_state.field.chassis[0].pose.translation_m;
            let displacement_m = initial
                .into_iter()
                .zip(final_position)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            if moving {
                assert!(displacement_m > 0.01, "workload must actually move");
            }
            let seconds = args.frames as f64 * 0.032;
            println!(
                "{}",
                serde_json::json!({
                    "scenario": name, "seed": seed, "players": players,
                    "measured_seconds": seconds, "warmup_seconds": args.warmup_frames as f64 * 0.032,
                    "publication_ms": 32, "physics_tick_ns": rm_simulator_world::tick_ns(),
                    "shots_launched_measured": shots, "owner_displacement_m_including_warmup": displacement_m,
                    "whole": control, "sections": candidate,
                    "downstream_change_percent": 100. * (candidate.downstream_bytes as f64 / control.downstream_bytes as f64 - 1.),
                    "both_directions_change_percent": 100. * ((candidate.downstream_bytes + candidate.upstream_bytes) as f64 / (control.downstream_bytes + control.upstream_bytes) as f64 - 1.),
                    "scope": "one peer, synthetic flat field, lossless immediate feedback; RMG1 included; native UDP/GNS, pacing, inputs and owner/hit streams excluded"
                })
            );
        }
    }
    Ok(())
}
