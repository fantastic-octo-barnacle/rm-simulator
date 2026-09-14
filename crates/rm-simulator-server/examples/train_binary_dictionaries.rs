// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Train format-specific dictionaries without changing the live wire dictionary.
//! Usage: `cargo run --release --locked -p rm-simulator-server --example
//! train_binary_dictionaries -- OUTPUT_DIRECTORY > training.csv`.
//! Training uses independent synthetic scenarios, never evaluation checkpoints.
//! Each candidate contributes full frames and deltas against 32-frame retained
//! baselines. Dictionaries serve both forms, including recovery full frames.
#[path = "support/binary_dictionary.rs"]
mod binary_dictionary;
#[path = "support/bitpack.rs"]
mod bitpack;
#[path = "support/dictionary_workloads.rs"]
mod dictionary_workloads;
// Training rounds wire values but never applies them to a physics world.
#[allow(dead_code)]
#[path = "support/fixed_point.rs"]
mod fixed_point;

use fixed_point::Quantization;
use serde_json::Value;
use sha2::{Digest, Sha256};

fn main() {
    let directory = std::env::args()
        .nth(1)
        .expect("provide an output directory; the production asset is never replaced");
    std::fs::create_dir_all(&directory).unwrap();
    let workloads: Vec<_> = dictionary_workloads::SCENARIOS
        .iter()
        .map(|scenario| {
            let values = dictionary_workloads::checkpoints(scenario);
            eprintln!(
                "training scenario={} chassis={} frames={}",
                scenario.name, scenario.chassis, scenario.frames
            );
            values
        })
        .collect();
    println!("profile,samples,sample_bytes,samples_sha256,dictionary_bytes,dictionary_sha256");
    for (packed, mode) in [
        (false, Quantization::None),
        (true, Quantization::None),
        (true, Quantization::Coarse),
        (true, Quantization::Fine),
        (true, Quantization::Chassis),
    ] {
        let mut samples = Vec::new();
        for workload in &workloads {
            let mut base = None;
            let mut id = 0;
            for (frame, checkpoint) in workload.iter().enumerate() {
                let mut value = checkpoint.clone();
                fixed_point::checkpoint(&mut value, &mut fixed_point::Errors::default(), mode);
                let epoch = value["CompactSnapshot"]["state"]["input_epoch"]
                    .as_u64()
                    .unwrap();
                let proposing = frame.is_multiple_of(32);
                if proposing {
                    id += 1;
                }
                samples.push(bitpack::encode(
                    &value,
                    None,
                    epoch,
                    if proposing { id } else { 0 },
                    packed,
                    mode != Quantization::None,
                ));
                if proposing {
                    base = Some(value.clone());
                } else {
                    samples.push(bitpack::encode(
                        &value,
                        base.as_ref(),
                        epoch,
                        id,
                        packed,
                        mode != Quantization::None,
                    ));
                }
                // Verify full and delta samples against the same retained state.
                for sample in samples.iter().rev().take(if proposing { 1 } else { 2 }) {
                    let decoded =
                        bitpack::decode(sample, base.as_ref().map(|b: &Value| (b, id)), epoch)
                            .unwrap()
                            .0;
                    assert!(bitpack::exact(&decoded, &value));
                }
            }
        }
        let dictionary = zstd::dict::from_samples(&samples, 32 * 1024).unwrap();
        // Check determinism explicitly before publishing a generated artifact.
        assert_eq!(
            dictionary,
            zstd::dict::from_samples(&samples, 32 * 1024).unwrap()
        );
        let mut codec = binary_dictionary::Codec::new(&dictionary).unwrap();
        let mut hash = Sha256::new();
        for sample in &samples {
            hash.update((sample.len() as u64).to_le_bytes());
            hash.update(sample);
            let compressed = codec.compress(sample);
            assert_eq!(
                codec.decompress(&compressed, sample.len()).unwrap(),
                *sample
            );
        }
        let name = binary_dictionary::name(packed, mode);
        let path = std::path::Path::new(&directory).join(format!("{name}.zstd"));
        std::fs::write(path, &dictionary).unwrap();
        println!(
            "{name},{},{},{:x},{},{:x}",
            samples.len(),
            samples.iter().map(Vec::len).sum::<usize>(),
            hash.finalize(),
            dictionary.len(),
            Sha256::digest(&dictionary)
        );
    }
}
