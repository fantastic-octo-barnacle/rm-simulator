// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Compares the selectable wire codecs on identical simulated checkpoints.
//!
//! Three streams are measured for every canonical probe workload:
//!
//! - `envelope`: the independent checkpoint envelopes, identical bytes for
//!   every codec, compressed and inflated with each candidate. This isolates
//!   the codec itself from the encoder's delta-versus-independent choice.
//! - `selected`: the full acknowledged-baseline encoder driven per codec, so
//!   the delta choice, the wire bytes and the decode path are all production
//!   code. A codec that compresses deltas better sends more of them.
//! - `envelope-loo`: a dictionary trained on the other three workloads and
//!   evaluated on the fourth, as a second check alongside the embedded
//!   dictionary trained on separate synthetic gameplay scenarios.
//!
//! One `key=value` record per codec and workload so a shell can diff runs.
//! Timings are CPU time on one process, not network latency, and they exclude
//! socket, GNS and pacing overhead. Run with `--release`; the environment's
//! codec selection does not affect this example, which builds one compressor
//! per candidate.
//!
//! ```sh
//! cargo run --release --locked -p rm-simulator-server \
//!     --example compression_comparison
//! ```
use rm_simulator_server::{
    compression::{Codec, Compressor, Decompressor, dictionary},
    protocol::{Command, ServerMessage},
    snapshot_codec::encode_player_message,
    udp_snapshot::{self, Decoder as BaselineDecoder, Encoder as BaselineEncoder, Wire},
    workload,
};
use serde_json::Value;
use std::time::Instant;

/// Application bytes carried by one fragment; `udp_codec::CHUNK`.
const CHUNK: usize = 1000;
/// Inflate ceiling, so a bug fails the run rather than allocating forever.
const LIMIT: usize = 4 << 20;
/// Simulation step between two checkpoints, in milliseconds.
const STEP_MS: u64 = 16;
/// Checkpoints per workload.
const FRAMES: usize = 250;
/// Frames between two shots of the firing workload.
const FIRE_PERIOD: u64 = 128 / STEP_MS;
/// Dictionary size for the leave-one-out training runs, matching the embedded
/// asset.
const DICTIONARY_BYTES: usize = 32 * 1024;
/// ZSTD level for the leave-one-out runs, matching the recommended candidate.
const LEAVE_ONE_OUT_LEVEL: i32 = 3;

/// One canonical probe workload: its compact checkpoints and the independent
/// envelopes every codec is compared on.
struct Workload {
    /// Workload name used in the records.
    name: &'static str,
    /// Compact checkpoint frames, one per publication.
    checkpoints: Vec<Vec<u8>>,
    /// The uncompressed independent envelope of each checkpoint.
    envelopes: Vec<Vec<u8>>,
}

fn main() {
    let seconds = FRAMES as f64 * STEP_MS as f64 / 1000.;
    println!("dictionary_bytes={}", dictionary().len());
    let codecs = [
        ("deflate-1", Codec::deflate(1)),
        ("deflate-4", Codec::deflate(4)),
        ("zstd-1", Codec::zstd(1)),
        ("zstd-3", Codec::zstd(3)),
        ("zstd-9", Codec::zstd(9)),
        ("zstd-dict-1", Codec::zstd_dictionary(1)),
        ("zstd-dict-3", Codec::zstd_dictionary(3)),
        ("zstd-dict-9", Codec::zstd_dictionary(9)),
    ];
    let workloads: Vec<Workload> = [
        ("idle", 0usize, false),
        ("drive", 2, false),
        ("twelve", 12, false),
        ("fire", 2, true),
    ]
    .into_iter()
    .map(|(name, players, firing)| {
        let checkpoints = checkpoint_stream(players, firing);
        let envelopes = checkpoints
            .iter()
            .map(|compact| {
                let state: Value = serde_json::from_slice(compact).unwrap();
                udp_snapshot::envelope_bytes(&Wire::Independent { epoch: 0, state })
            })
            .collect();
        Workload {
            name,
            checkpoints,
            envelopes,
        }
    })
    .collect();
    for workload in &workloads {
        for (name, codec) in codecs {
            envelope_row(workload.name, name, codec, &workload.envelopes, seconds);
        }
        for (name, codec) in codecs {
            selected_row(workload.name, name, codec, &workload.checkpoints, seconds);
        }
    }
    leave_one_out_rows(&workloads, seconds);
}

/// Trains one dictionary per workload from the **other** workloads only, so the
/// embedded asset can be compared with another dictionary that never saw the
/// frames it compresses. The measured bytes include the four-byte
/// `RMZ2` prefix each wire frame would carry.
fn leave_one_out_rows(workloads: &[Workload], seconds: f64) {
    for (held_out_index, held_out) in workloads.iter().enumerate() {
        let samples: Vec<&[u8]> = workloads
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != held_out_index)
            .flat_map(|(_, workload)| workload.envelopes.iter().map(Vec::as_slice))
            .collect();
        let trained = zstd::dict::from_samples(&samples, DICTIONARY_BYTES).expect("train");
        let mut compressor =
            zstd::bulk::Compressor::with_dictionary(LEAVE_ONE_OUT_LEVEL, &trained).unwrap();
        let mut decompressor = zstd::bulk::Decompressor::with_dictionary(&trained).unwrap();
        let mut compress_ns = Vec::with_capacity(held_out.envelopes.len());
        let mut decompress_ns = Vec::with_capacity(held_out.envelopes.len());
        let mut bytes = 0;
        let mut fragments = 0;
        for raw in &held_out.envelopes {
            let started = Instant::now();
            let compressed = compressor.compress(raw).expect("compress");
            compress_ns.push(started.elapsed().as_nanos());
            bytes += compressed.len() + compression_prefix();
            fragments += (compressed.len() + compression_prefix()).div_ceil(CHUNK);
            let started = Instant::now();
            let restored = decompressor.decompress(&compressed, raw.len() + 1).unwrap();
            decompress_ns.push(started.elapsed().as_nanos());
            assert_eq!(&restored, raw);
        }
        let frames = held_out.envelopes.len();
        println!(
            "stream=envelope-loo workload={} codec=zstd-dict-{LEAVE_ONE_OUT_LEVEL} frames={frames} samples={} dictionary_bytes={} bytes={bytes} fragments={fragments} kbps={:.3} compress_mean_us={:.3} compress_p95_us={:.3} decompress_mean_us={:.3} decompress_p95_us={:.3}",
            held_out.name,
            samples.len(),
            trained.len(),
            bytes as f64 * 8. / seconds / 1000.,
            mean_us(&compress_ns),
            p95_us(&compress_ns),
            mean_us(&decompress_ns),
            p95_us(&decompress_ns),
        );
    }
}

/// Bytes of the `RMZ2` prefix a dictionary frame carries on the wire.
fn compression_prefix() -> usize {
    rm_simulator_server::compression::ZSTD_DICTIONARY_MAGIC.len()
}

/// Compresses and inflates identical envelopes with one codec.
fn envelope_row(workload: &str, name: &str, codec: Codec, envelopes: &[Vec<u8>], seconds: f64) {
    let mut compressor = Compressor::new(codec);
    let mut decompressor = Decompressor::new();
    let mut compress_ns = Vec::with_capacity(envelopes.len());
    let mut decompress_ns = Vec::with_capacity(envelopes.len());
    let mut bytes = 0;
    let mut fragments = 0;
    for raw in envelopes {
        let started = Instant::now();
        let compressed = compressor.compress(raw);
        compress_ns.push(started.elapsed().as_nanos());
        bytes += compressed.len();
        fragments += compressed.len().div_ceil(CHUNK);
        let started = Instant::now();
        let restored = decompressor
            .decompress(&compressed, raw.len() + 1)
            .expect("inflate envelope");
        decompress_ns.push(started.elapsed().as_nanos());
        assert_eq!(&restored, raw, "codec {name} changed the envelope");
    }
    let frames = envelopes.len();
    println!(
        "stream=envelope workload={workload} codec={name} frames={frames} bytes={bytes} fragments={fragments} kbps={:.3} compress_mean_us={:.3} compress_p95_us={:.3} decompress_mean_us={:.3} decompress_p95_us={:.3}",
        bytes as f64 * 8. / seconds / 1000.,
        mean_us(&compress_ns),
        p95_us(&compress_ns),
        mean_us(&decompress_ns),
        p95_us(&decompress_ns),
    );
}

/// Drives the production baseline encoder, inflater and baseline decoder with
/// one codec, so the delta choice is the codec's own.
fn selected_row(workload: &str, name: &str, codec: Codec, checkpoints: &[Vec<u8>], seconds: f64) {
    let mut encoder = BaselineEncoder::with_codec(codec);
    let mut decoder = BaselineDecoder::default();
    let mut inflater = Decompressor::new();
    let mut encode_ns = Vec::with_capacity(checkpoints.len());
    let mut decode_ns = Vec::with_capacity(checkpoints.len());
    let mut bytes = 0;
    let mut fragments = 0;
    let mut retirements = 0;
    for (frame, compact) in checkpoints.iter().enumerate() {
        let started = Instant::now();
        let wire = encoder.snapshot(0, compact).expect("encode checkpoint");
        encode_ns.push(started.elapsed().as_nanos());
        bytes += wire.len();
        fragments += wire.len().div_ceil(CHUNK);
        let started = Instant::now();
        let plain = inflater.decompress(&wire, LIMIT).expect("inflate frame");
        let parsed = udp_snapshot::parse(&plain)
            .unwrap()
            .expect("snapshot frame");
        let (message, feedback) = decoder.receive(parsed).expect("decode frame");
        assert!(
            matches!(&message, Some(ServerMessage::Snapshot(state)) if state.snapshot_id == frame as u64 + 1),
            "codec {name} lost checkpoint {frame}"
        );
        if let Some(feedback) = feedback
            && let Some(retire) = encoder.feedback(feedback)
            && let (_, Some(retired)) = decoder.receive(retire).expect("retire baseline")
        {
            assert!(encoder.feedback(retired).is_none());
            retirements += 1;
        }
        decode_ns.push(started.elapsed().as_nanos());
    }
    assert!(
        retirements >= checkpoints.len().saturating_sub(1) / 32,
        "codec {name} stopped rotating acknowledged baselines"
    );
    let frames = checkpoints.len();
    println!(
        "stream=selected workload={workload} codec={name} frames={frames} bytes={bytes} fragments={fragments} deltas={} independent_or_full={} kbps={:.3} compress_mean_us={:.3} compress_p95_us={:.3} decode_mean_us={:.3} decode_p95_us={:.3}",
        encoder.deltas,
        frames as u64 - encoder.deltas,
        bytes as f64 * 8. / seconds / 1000.,
        mean_us(&encode_ns),
        p95_us(&encode_ns),
        mean_us(&decode_ns),
        p95_us(&decode_ns),
    );
}

/// The compact checkpoint stream one workload publishes, exactly as the
/// canonical probe and `network_bandwidth` build it.
fn checkpoint_stream(players: usize, firing: bool) -> Vec<Vec<u8>> {
    let (mut simulation, chassis) = workload::simulation(players);
    for &chassis in &chassis {
        simulation
            .apply(&Command::Chassis {
                chassis,
                command: workload::constant_drive(),
            })
            .unwrap();
    }
    let mut stream = Vec::with_capacity(FRAMES);
    for frame in 0..FRAMES as u64 {
        simulation.step(STEP_MS).unwrap();
        if firing && frame.is_multiple_of(FIRE_PERIOD) {
            simulation
                .apply(&Command::Fire {
                    shooter: 0,
                    timing: None,
                })
                .unwrap();
        }
        let mut state = simulation.state();
        state.snapshot_id = frame + 1;
        stream.push(encode_player_message(&ServerMessage::Snapshot(Box::new(
            state,
        ))));
    }
    stream
}

/// Arithmetic mean of a nanosecond sample set, in microseconds.
fn mean_us(samples: &[u128]) -> f64 {
    samples.iter().sum::<u128>() as f64 / samples.len() as f64 / 1000.
}

/// Nearest-rank 95th percentile of a nanosecond sample set, in microseconds.
fn p95_us(samples: &[u128]) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() * 95 / 100] as f64 / 1000.
}
