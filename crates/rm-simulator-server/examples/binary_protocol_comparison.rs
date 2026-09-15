// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Runnable binary/fixed-point protocol experiment, isolated from live sockets.
//! Run with `cargo run --release --locked -p rm-simulator-server --example
//! binary_protocol_comparison > results.csv`. CSV goes to stdout, numerical
//! error and restored-world replay results to stderr. No CAD or new dependency.
//! Pass the directory from `train_binary_dictionaries` as the sole argument to
//! also measure new dictionaries trained specifically for each binary format.
//!
//! Lossless candidates reproduce every compact JSON value, including f64 bits.
//! Fixed-point candidates preserve every value after explicit motion rounding;
//! quaternion normalization happens only in the physics adapter. Baselines
//! rotate every 32 frames with immediate acknowledgement, like the production
//! encoder on a loss-free link. This is a codec trial, not a transport trial.
#[path = "support/binary_dictionary.rs"]
mod binary_dictionary;
#[path = "support/bitpack.rs"]
mod bitpack;
#[path = "support/fixed_point.rs"]
mod fixed_point;

use fixed_point::Quantization;
use rm_simulator_server::{
    compression::{Codec, Compressor, Decompressor},
    protocol::{Command, ServerMessage},
    snapshot_codec::{decode_player_message, encode_player_message},
    udp_snapshot::{self, Decoder, Encoder},
    workload,
};
use rm_simulator_world::{ChassisCommand, Field, FieldConfig, Team};
use serde_json::Value;
use std::time::Instant;

const FRAMES: usize = 320;
const STEP_MS: u64 = 32;
const LIMIT: usize = 4 << 20;
const FORMATS: [(&str, bool, bool, Quantization); 8] = [
    ("binary-full", false, false, Quantization::None),
    ("binary-delta", false, true, Quantization::None),
    ("bit-full", true, false, Quantization::None),
    ("bit-delta", true, true, Quantization::None),
    ("fixed-full", true, false, Quantization::Coarse),
    ("fixed-delta", true, true, Quantization::Coarse),
    ("fixed-fine-delta", true, true, Quantization::Fine),
    ("fixed-chassis-delta", true, true, Quantization::Chassis),
];

#[derive(Default)]
struct Measurement {
    bytes: usize,
    fragments: usize,
    deltas: usize,
    encode_ns: Vec<u128>,
    decode_ns: Vec<u128>,
}
impl Measurement {
    fn add(&mut self, bytes: usize) {
        self.bytes += bytes;
        self.fragments += bytes.div_ceil(1000);
    }
    fn print(&self, workload: &str, format: &str, compression: &str) {
        let seconds = FRAMES as f64 * STEP_MS as f64 / 1000.;
        println!(
            "{workload},{format},{compression},{FRAMES},{},{},{},{:.3},{:.3},{:.3},{:.3}",
            self.bytes,
            self.fragments,
            self.deltas,
            self.bytes as f64 * 8. / seconds / 1000.,
            (self.bytes + self.fragments * 21) as f64 * 8. / seconds / 1000.,
            self.encode_ns.iter().sum::<u128>() as f64 / FRAMES as f64 / 1000.,
            self.decode_ns.iter().sum::<u128>() as f64 / FRAMES as f64 / 1000.
        );
    }
}

fn main() {
    // Optional directory produced by `train_binary_dictionaries`. Dictionary
    // generation stays outside measurement so training cannot contaminate CPU timings.
    let dictionary_dir = std::env::args().nth(1).map(std::path::PathBuf::from);
    assert!(
        std::env::args().len() <= 2,
        "usage: binary_protocol_comparison [DICTIONARY_DIRECTORY]"
    );
    println!(
        "workload,format,compression,frames,payload_bytes,fragments,deltas,payload_kbps,framed_kbps,encode_mean_us,decode_mean_us"
    );
    for (name, players, firing, dynamic) in [
        ("idle", 1, false, false),
        ("drive", 2, false, false),
        ("twelve", 12, false, false),
        ("fire", 2, true, false),
        ("skirmish", 12, true, true),
    ] {
        let checkpoints = checkpoint_stream(name, players, firing, dynamic);
        for (compression, codec) in [
            ("none", None),
            ("deflate-1", Some(Codec::deflate(1))),
            ("zstd-3", Some(Codec::zstd(3))),
            ("zstd-dict-3", Some(Codec::zstd_dictionary(3))),
        ] {
            json_full(name, compression, codec, &checkpoints);
            if let Some(codec) = codec {
                production(name, compression, codec, &checkpoints);
            }
            for (format, packed, delta, fixed) in FORMATS {
                candidate(
                    name,
                    format,
                    compression,
                    codec,
                    &checkpoints,
                    (packed, delta, fixed),
                    None,
                );
            }
        }
        if let Some(directory) = &dictionary_dir {
            for (format, packed, delta, mode) in FORMATS {
                let profile = binary_dictionary::name(packed, mode);
                let dictionary = std::fs::read(directory.join(format!("{profile}.zstd")))
                    .expect("trained binary dictionary");
                candidate(
                    name,
                    format,
                    "zstd-retrained-3",
                    None,
                    &checkpoints,
                    (packed, delta, mode),
                    Some(&dictionary),
                );
            }
        }
        for mode in [
            Quantization::Coarse,
            Quantization::Fine,
            Quantization::Chassis,
        ] {
            replay(name, &checkpoints, mode);
        }
    }
}

fn checkpoint_stream(name: &str, players: usize, firing: bool, dynamic: bool) -> Vec<Vec<u8>> {
    let (mut simulation, mut ids) = workload::simulation(players);
    let mut stream = Vec::new();
    let mut owner_bytes = 0;
    for frame in 0..FRAMES {
        if dynamic && frame == 150 {
            simulation.remove_chassis(ids.pop().unwrap()).unwrap();
        }
        if dynamic && frame == 200 {
            ids.push(simulation.spawn_chassis(Team::Blue).unwrap());
        }
        if frame == 0 || (dynamic && frame.is_multiple_of(8)) {
            for &id in &ids {
                let phase = frame as f64 * 0.03 + f64::from(id);
                // The steady runs drive the shared constant command; the idle
                // run stands still and the dynamic one scripts its own motion.
                let command = if name == "idle" {
                    ChassisCommand::default()
                } else if dynamic {
                    ChassisCommand {
                        forward_m_s: 2. * phase.sin(),
                        left_m_s: phase.cos(),
                        yaw_rate_rad_s: 0.4,
                        aim_yaw_rad: phase.sin(),
                        aim_pitch_rad: 0.2 * phase.cos(),
                    }
                } else {
                    workload::constant_drive()
                };
                simulation
                    .apply(&Command::Chassis {
                        chassis: id,
                        command,
                    })
                    .unwrap();
            }
        }
        simulation.step(STEP_MS).unwrap();
        if firing && frame.is_multiple_of(4) {
            for &id in ids.iter().take(if dynamic { players } else { 1 }) {
                simulation
                    .apply(&Command::Fire {
                        shooter: id,
                        timing: None,
                    })
                    .unwrap();
            }
        }
        let mut state = simulation.state();
        state.snapshot_id = frame as u64 + 1;
        owner_bytes += rm_simulator_server::owner_stream::OwnerAnchor::from_state(&state, ids[0])
            .unwrap()
            .encode()
            .unwrap()
            .len();
        stream.push(encode_player_message(&ServerMessage::Snapshot(Box::new(
            state,
        ))));
    }
    eprintln!(
        "owner workload={name} bytes={owner_bytes} payload_kbps={:.3}",
        owner_bytes as f64 * 8. / (FRAMES as f64 * STEP_MS as f64)
    );
    stream
}

fn json_full(name: &str, compression: &str, codec: Option<Codec>, checkpoints: &[Vec<u8>]) {
    let mut compressor = codec.map(Compressor::new);
    let mut inflater = Decompressor::new();
    let mut decoder = Decoder::default();
    let mut m = Measurement::default();
    for checkpoint in checkpoints {
        let started = Instant::now();
        let state = serde_json::from_slice(checkpoint).unwrap();
        let raw =
            udp_snapshot::envelope_bytes(&udp_snapshot::Wire::Independent { epoch: 0, state });
        let bytes = compress(&mut compressor, &raw);
        m.encode_ns.push(started.elapsed().as_nanos());
        m.add(bytes.len());
        let started = Instant::now();
        let raw = if codec.is_some() {
            inflater.decompress(&bytes, LIMIT).unwrap()
        } else {
            bytes
        };
        let (message, _) = decoder
            .receive(udp_snapshot::parse(&raw).unwrap().unwrap())
            .unwrap();
        m.decode_ns.push(started.elapsed().as_nanos());
        assert_eq!(message.unwrap(), decode_player_message(checkpoint).unwrap());
    }
    m.print(name, "json-full", compression);
}

fn production(name: &str, compression: &str, codec: Codec, checkpoints: &[Vec<u8>]) {
    let mut encoder = Encoder::with_codec(codec);
    let mut decoder = Decoder::default();
    let mut inflater = Decompressor::new();
    let mut m = Measurement::default();
    for checkpoint in checkpoints {
        let started = Instant::now();
        let bytes = encoder.snapshot(0, checkpoint).unwrap();
        m.encode_ns.push(started.elapsed().as_nanos());
        m.add(bytes.len());
        let started = Instant::now();
        let raw = inflater.decompress(&bytes, LIMIT).unwrap();
        let (actual, feedback) = decoder
            .receive(udp_snapshot::parse(&raw).unwrap().unwrap())
            .unwrap();
        m.decode_ns.push(started.elapsed().as_nanos());
        assert_eq!(actual.unwrap(), decode_player_message(checkpoint).unwrap());
        if let Some(feedback) = feedback
            && let Some(retire) = encoder.feedback(feedback)
            && let (_, Some(retired)) = decoder.receive(retire).unwrap()
        {
            encoder.feedback(retired);
        }
    }
    m.deltas = encoder.deltas as usize;
    m.print(name, "json-selected", compression);
}

fn candidate(
    name: &str,
    format: &str,
    compression: &str,
    codec: Option<Codec>,
    checkpoints: &[Vec<u8>],
    (packed, delta, mode): (bool, bool, Quantization),
    dictionary: Option<&[u8]>,
) {
    let fixed = mode != Quantization::None;
    let mut trained = dictionary.map(|bytes| binary_dictionary::Codec::new(bytes).unwrap());
    let mut compressor = codec.map(Compressor::new);
    let mut inflater = Decompressor::new();
    let mut sender_base = None;
    let mut receiver_base = None;
    let mut base_id = 0;
    let mut m = Measurement::default();
    for (frame, checkpoint) in checkpoints.iter().enumerate() {
        let started = Instant::now();
        let mut state: Value = serde_json::from_slice(checkpoint).unwrap();
        if fixed {
            fixed_point::checkpoint(&mut state, &mut fixed_point::Errors::default(), mode);
        }
        let proposing = delta && frame.is_multiple_of(32);
        if proposing {
            base_id += 1;
        }
        let full = bitpack::encode(
            &state,
            None,
            0,
            if proposing { base_id } else { 0 },
            packed,
            fixed,
        )
        .unwrap();
        let mut wire = compress_candidate(&mut compressor, &mut trained, &full);
        if delta && !proposing {
            let patch =
                bitpack::encode(&state, sender_base.as_ref(), 0, base_id, packed, fixed).unwrap();
            let patch = compress_candidate(&mut compressor, &mut trained, &patch);
            if patch.len() < wire.len() {
                wire = patch;
                m.deltas += 1;
            }
        }
        m.encode_ns.push(started.elapsed().as_nanos());
        m.add(wire.len());
        let started = Instant::now();
        let raw = if let Some(trained) = &mut trained {
            trained.decompress(&wire, LIMIT).unwrap()
        } else if codec.is_some() {
            inflater.decompress(&wire, LIMIT).unwrap()
        } else {
            wire
        };
        let (restored, id) =
            bitpack::decode(&raw, receiver_base.as_ref().map(|v| (v, base_id)), 0).unwrap();
        // Include the existing Value-to-protocol bridge in both decode timings.
        let mut message = decode_player_message(&serde_json::to_vec(&restored).unwrap()).unwrap();
        if fixed {
            fixed_point::normalize(&mut message).unwrap();
        }
        m.decode_ns.push(started.elapsed().as_nanos());
        assert!(
            bitpack::exact(&restored, &state),
            "{name}/{format}/{compression}/{frame}"
        );
        if proposing {
            assert_eq!(id, base_id);
            sender_base = Some(state);
            receiver_base = Some(restored);
        }
    }
    m.print(name, format, compression);
}

fn compress_candidate(
    standard: &mut Option<Compressor>,
    trained: &mut Option<binary_dictionary::Codec>,
    raw: &[u8],
) -> Vec<u8> {
    if let Some(trained) = trained {
        trained.compress(raw)
    } else {
        compress(standard, raw)
    }
}

fn compress(compressor: &mut Option<Compressor>, raw: &[u8]) -> Vec<u8> {
    compressor
        .as_mut()
        .map_or_else(|| raw.to_vec(), |c| c.compress(raw))
}

fn state(bytes: &[u8]) -> rm_simulator_server::simulation::SimulationState {
    let ServerMessage::Snapshot(state) = decode_player_message(bytes).unwrap() else {
        panic!("snapshot");
    };
    *state
}

/// Compare two restored worlds, isolating quantization from the solver warm
/// starts that *both* restores lose. Horizons are explicit simulation ticks.
fn replay(name: &str, checkpoints: &[Vec<u8>], mode: Quantization) {
    let mut errors = fixed_point::Errors::default();
    let mut position = [0_f64; 3];
    let mut rotation = [0_f64; 3];
    let mut projectile = [0_f64; 3];
    let mut membership = [0_usize; 3];
    let mut scoring = [0_usize; 3];
    let geometry = Field::new(&FieldConfig::default())
        .unwrap()
        .static_geometry_snapshot();
    for (index, checkpoint) in checkpoints.iter().enumerate() {
        let mut quantized: Value = serde_json::from_slice(checkpoint).unwrap();
        fixed_point::checkpoint(&mut quantized, &mut errors, mode);
        if !index.is_multiple_of(32) {
            continue;
        }
        let raw = bitpack::encode(&quantized, None, 0, 0, true, true).unwrap();
        let restored = bitpack::decode(&raw, None, 0).unwrap().0;
        let mut message = decode_player_message(&serde_json::to_vec(&restored).unwrap()).unwrap();
        fixed_point::normalize(&mut message).unwrap();
        let ServerMessage::Snapshot(rounded) = message else {
            panic!("snapshot");
        };
        let exact = state(checkpoint);
        let mut a = Field::restore(&exact.field, &geometry, 0.).unwrap();
        let mut b = Field::restore(&rounded.field, &geometry, 0.).unwrap();
        for (horizon, step) in [0, 32, 96].into_iter().enumerate() {
            a.step(step).unwrap();
            b.step(step).unwrap();
            let a = a.snapshot();
            let b = b.snapshot();
            for (a, b) in a.chassis.iter().zip(&b.chassis) {
                position[horizon] =
                    position[horizon].max(distance(a.pose.translation_m, b.pose.translation_m));
                let qa = a.pose.rotation_wxyz;
                let qb = b.pose.rotation_wxyz;
                let dot = qa
                    .into_iter()
                    .zip(qb)
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    .abs();
                rotation[horizon] = rotation[horizon].max(2. * dot.clamp(0., 1.).acos());
            }
            membership[horizon] += usize::from(
                !a.projectiles
                    .iter()
                    .map(|p| p.id)
                    .eq(b.projectiles.iter().map(|p| p.id)),
            );
            for a in &a.projectiles {
                if let Some(b) = b.projectiles.iter().find(|b| b.id == a.id) {
                    projectile[horizon] =
                        projectile[horizon].max(distance(a.position_m, b.position_m));
                }
            }
            scoring[horizon] += usize::from(
                a.hits_detected != b.hits_detected
                    || a.referee != b.referee
                    || a.bases
                        .iter()
                        .map(|b| (b.hp, b.shield_hp))
                        .ne(b.bases.iter().map(|b| (b.hp, b.shield_hp)))
                    || a.outposts
                        .iter()
                        .map(|o| o.hp)
                        .ne(b.outposts.iter().map(|o| o.hp)),
            );
        }
    }
    eprintln!(
        "quantization workload={name} mode={mode:?} max_component_position_m={} max_component_velocity_m_s={} max_angle_rad={} max_angular_velocity_rad_s={} max_quaternion_component={} escapes={}",
        errors.position_m,
        errors.velocity_m_s,
        errors.angle_rad,
        errors.angular_velocity_rad_s,
        errors.quaternion_component,
        errors.escapes
    );
    for (i, horizon) in [0, 32, 128].into_iter().enumerate() {
        eprintln!(
            "replay workload={name} mode={mode:?} samples={} horizon_ms={horizon} max_chassis_position_m={} max_chassis_rotation_rad={} max_projectile_position_m={} projectile_membership_disagreements={} scoring_disagreements={}",
            FRAMES.div_ceil(32),
            position[i],
            rotation[i],
            projectile[i],
            membership[i],
            scoring[i]
        );
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}
