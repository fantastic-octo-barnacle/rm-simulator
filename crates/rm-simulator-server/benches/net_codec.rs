// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Network codec microbenchmarks: packed checkpoint encoding, dictionary
//! compression at several levels, decompression, bitpack decoding and the full
//! acknowledged-baseline round trip.
//!
//! Samples are deterministic checkpoints from [`rm_simulator_server::workload`]
//! at the 32 ms world cadence, so runs compare byte for byte across machines.
//! The production checkpoint compressor itself is crate-private, so the
//! compression groups use the same construction (level, embedded dictionary,
//! `RMBZ` framing, no-bloat passthrough) through the public [`zstd`] bulk API;
//! the `udp_roundtrip` group drives the real [`udp_snapshot::Encoder`] and
//! [`udp_snapshot::Decoder`] instead. Sizes per level print once during setup;
//! times are what Criterion reports.
//!
//! Run with `cargo bench -p rm-simulator-server --locked --bench net_codec`.
use criterion::{BenchmarkId, Throughput, criterion_group, criterion_main};
use criterion::{Criterion, measurement::WallTime};
use rm_simulator_server::binary_snapshot::bitpack;
use rm_simulator_server::protocol::Command;
use rm_simulator_server::simulation::SimulationState;
use rm_simulator_server::{compression, snapshot_codec, udp_snapshot, workload};
use std::hint::black_box;
use std::time::Duration;

/// Frames per workload sample set, at the 32 ms world publication cadence.
const FRAMES: usize = 128;
/// Dictionary compression levels swept against the production level 6.
const LEVELS: [i32; 5] = [1, 3, 6, 9, 12];
/// The production checkpoint level; mirrors `CHECKPOINT_LEVEL`, which the
/// bench cannot name because it is crate-private.
const PRODUCTION_LEVEL: i32 = 6;

/// One deterministic sample set: checkpoint states plus the packed `RMB1`
/// bytes and dictionary-framed bytes the live path derives.
struct Samples {
    /// Workload label for [`BenchmarkId`].
    name: &'static str,
    /// Checkpoint states the [`udp_snapshot::Encoder`] consumes.
    states: Vec<SimulationState>,
    /// Fine-quantized, bitpacked `RMB1` frames.
    packed: Vec<Vec<u8>>,
    /// Production-equivalent dictionary frames (or bare `RMB1` when
    /// the no-bloat gate fires).
    framed: Vec<Vec<u8>>,
}

/// Frames a packed checkpoint exactly like the production compressor: `RMBZ`
/// dictionary frame, or the packed frame itself when compression would grow
/// it.
fn frame_checkpoint(compressor: &mut zstd::bulk::Compressor<'static>, packed: &[u8]) -> Vec<u8> {
    let body = compressor
        .compress(packed)
        .expect("bench checkpoint compression");
    if rm_simulator_server::binary_snapshot::MAGIC.len() + body.len() >= packed.len() {
        return packed.to_vec();
    }
    let mut framed = rm_simulator_server::binary_snapshot::MAGIC.to_vec();
    framed.extend(body);
    framed
}

/// Builds one sample set by driving the shared measurement field.
fn samples(name: &'static str, players: usize, drive: bool, fire: bool) -> Samples {
    let (mut simulation, chassis) = workload::simulation(players);
    let driver = chassis[0];
    let mut states = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES as u64 {
        if drive {
            simulation
                .apply(&Command::Chassis {
                    chassis: driver,
                    command: workload::constant_drive(),
                })
                .unwrap();
        }
        if fire && frame.is_multiple_of(4) {
            let _ = simulation.apply(&Command::Fire {
                shooter: driver,
                timing: None,
            });
        }
        simulation.step(32).unwrap();
        let mut state = simulation.state();
        state.snapshot_id = frame + 1;
        states.push(state);
    }
    let mut packed = Vec::with_capacity(FRAMES);
    let mut framed = Vec::with_capacity(FRAMES);
    let mut compressor = zstd::bulk::Compressor::with_dictionary(
        PRODUCTION_LEVEL,
        rm_simulator_server::binary_snapshot::dictionary(),
    )
    .unwrap();
    for state in &states {
        let node = snapshot_codec::checkpoint_node(state).unwrap();
        let bytes = bitpack::encode(&node, None, state.input_epoch, 0).unwrap();
        framed.push(frame_checkpoint(&mut compressor, &bytes));
        packed.push(bytes);
    }
    Samples {
        name,
        states,
        packed,
        framed,
    }
}

/// All sample sets, built once per benchmark binary invocation.
fn all_samples() -> Vec<Samples> {
    vec![
        samples("idle", 1, false, false),
        samples("drive", 2, true, false),
        samples("fire", 2, true, true),
        samples("twelve", 12, true, true),
    ]
}

/// Quantize plus bitpack one independent checkpoint per iteration.
fn bench_encode(c: &mut Criterion<WallTime>) {
    let mut group = c.benchmark_group("quantize_bitpack");
    for set in all_samples() {
        let bytes = set.packed.iter().map(Vec::len).sum::<usize>() as u64;
        group.throughput(Throughput::Bytes(bytes / FRAMES as u64));
        group.bench_with_input(BenchmarkId::from_parameter(set.name), &set, |b, set| {
            let mut index = 0;
            b.iter(|| {
                let state = black_box(&set.states[index % FRAMES]);
                index += 1;
                let node = snapshot_codec::checkpoint_node(state).unwrap();
                black_box(bitpack::encode(&node, None, state.input_epoch, 0).unwrap());
            });
        });
    }
    group.finish();
}

/// Dictionary-compress packed checkpoints at the production level.
fn bench_compress(c: &mut Criterion<WallTime>) {
    let mut group = c.benchmark_group("dict_compress");
    for set in all_samples() {
        let bytes = set.packed.iter().map(Vec::len).sum::<usize>() as u64;
        group.throughput(Throughput::Bytes(bytes / FRAMES as u64));
        group.bench_with_input(BenchmarkId::from_parameter(set.name), &set, |b, set| {
            let mut compressor = zstd::bulk::Compressor::with_dictionary(
                PRODUCTION_LEVEL,
                rm_simulator_server::binary_snapshot::dictionary(),
            )
            .unwrap();
            let mut index = 0;
            b.iter(|| {
                let packed = &set.packed[index % FRAMES];
                index += 1;
                black_box(frame_checkpoint(&mut compressor, black_box(packed)));
            });
        });
    }
    group.finish();
}

/// Compression time and output size across levels; sizes print once in setup.
fn bench_levels(c: &mut Criterion<WallTime>) {
    let mut group = c.benchmark_group("dict_compress_level");
    for set in all_samples() {
        let packed_bytes = set.packed.iter().map(Vec::len).sum::<usize>();
        for level in LEVELS {
            let mut compressor = zstd::bulk::Compressor::with_dictionary(
                level,
                rm_simulator_server::binary_snapshot::dictionary(),
            )
            .unwrap();
            let framed_bytes: usize = set
                .packed
                .iter()
                .map(|packed| frame_checkpoint(&mut compressor, packed).len())
                .sum();
            println!(
                "level={level} workload={} packed_bytes={packed_bytes} framed_bytes={framed_bytes}",
                set.name
            );
            group.throughput(Throughput::Bytes(packed_bytes as u64 / FRAMES as u64));
            group.bench_with_input(
                BenchmarkId::new(set.name, level),
                &(&set.packed, level),
                |b, (packed, level)| {
                    let mut compressor = zstd::bulk::Compressor::with_dictionary(
                        *level,
                        rm_simulator_server::binary_snapshot::dictionary(),
                    )
                    .unwrap();
                    let mut index = 0;
                    b.iter(|| {
                        let packed_frame = &packed[index % FRAMES];
                        index += 1;
                        black_box(frame_checkpoint(&mut compressor, black_box(packed_frame)));
                    });
                },
            );
        }
    }
    group.finish();
}

/// Inflate dictionary frames through the production decompressor.
fn bench_decompress(c: &mut Criterion<WallTime>) {
    let mut group = c.benchmark_group("dict_decompress");
    for set in all_samples() {
        let bytes = set.framed.iter().map(Vec::len).sum::<usize>() as u64;
        group.throughput(Throughput::Bytes(bytes / FRAMES as u64));
        group.bench_with_input(BenchmarkId::from_parameter(set.name), &set, |b, set| {
            let mut index = 0;
            b.iter(|| {
                let framed = &set.framed[index % FRAMES];
                index += 1;
                black_box(compression::decompress(black_box(framed), 4 << 20).unwrap());
            });
        });
    }
    group.finish();
}

/// Bitpack-decode packed checkpoints against no baseline.
fn bench_decode(c: &mut Criterion<WallTime>) {
    let mut group = c.benchmark_group("bitpack_decode");
    for set in all_samples() {
        let bytes = set.packed.iter().map(Vec::len).sum::<usize>() as u64;
        group.throughput(Throughput::Bytes(bytes / FRAMES as u64));
        group.bench_with_input(BenchmarkId::from_parameter(set.name), &set, |b, set| {
            let mut index = 0;
            b.iter(|| {
                let epoch = set.states[index % FRAMES].input_epoch;
                let packed = &set.packed[index % FRAMES];
                index += 1;
                black_box(
                    snapshot_codec::decode_checkpoint(black_box(packed), None, epoch, false)
                        .unwrap(),
                );
            });
        });
    }
    group.finish();
}

/// The full acknowledged-baseline round trip per frame: snapshot, inflate,
/// parse, receive, and the feedback/retire handshake, cycling sample states
/// so the encoder and decoder rotate baselines as they do live.
fn bench_roundtrip(c: &mut Criterion<WallTime>) {
    let mut group = c.benchmark_group("udp_roundtrip");
    group.measurement_time(Duration::from_secs(8));
    for set in all_samples() {
        group.bench_with_input(BenchmarkId::from_parameter(set.name), &set, |b, set| {
            let mut encoder = udp_snapshot::Encoder::new();
            let mut decoder = udp_snapshot::Decoder::default();
            // Warm the baseline rotation before measuring steady state.
            for state in set.states.iter().take(32) {
                let wire = encoder.snapshot(state.input_epoch, state).unwrap();
                let parsed = udp_snapshot::parse(&compression::decompress(&wire, 4 << 20).unwrap())
                    .unwrap()
                    .unwrap();
                let (_, feedback) = decoder.receive(parsed).unwrap();
                if let Some(feedback) = feedback
                    && let Some(retire) = encoder.feedback(feedback)
                {
                    let (_, retired) = decoder.receive(retire).unwrap();
                    encoder.feedback(retired.unwrap());
                }
            }
            let mut index = 0;
            b.iter(|| {
                let state = &set.states[index % FRAMES];
                index += 1;
                let wire = encoder
                    .snapshot(state.input_epoch, black_box(state))
                    .unwrap();
                let parsed = udp_snapshot::parse(&compression::decompress(&wire, 4 << 20).unwrap())
                    .unwrap()
                    .unwrap();
                let (message, feedback) = decoder.receive(parsed).unwrap();
                black_box(message);
                if let Some(feedback) = feedback
                    && let Some(retire) = encoder.feedback(feedback)
                {
                    let (_, retired) = decoder.receive(retire).unwrap();
                    encoder.feedback(retired.unwrap());
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_encode,
    bench_compress,
    bench_levels,
    bench_decompress,
    bench_decode,
    bench_roundtrip
);
criterion_main!(benches);
