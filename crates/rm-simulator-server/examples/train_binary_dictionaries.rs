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

use rm_simulator_server::snapshot_codec::{checkpoint_node, decode_checkpoint};
use sha2::{Digest, Sha256};

fn main() {
    let directory = std::env::args()
        .nth(1)
        .expect("provide an output directory; the production asset is never replaced");
    std::fs::create_dir_all(&directory).unwrap();
    let workloads: Vec<_> = dictionary_workloads::SCENARIOS
        .iter()
        .map(|scenario| {
            let states = dictionary_workloads::checkpoints(scenario);
            eprintln!(
                "training scenario={} chassis={} frames={}",
                scenario.name, scenario.chassis, scenario.frames
            );
            states
        })
        .collect();
    println!("profile,samples,sample_bytes,samples_sha256,dictionary_bytes,dictionary_sha256");
    {
        let mut samples = Vec::new();
        for workload in &workloads {
            let mut base = None;
            let mut id = 0;
            for (frame, state) in workload.iter().enumerate() {
                let node = checkpoint_node(state).unwrap();
                let epoch = state.input_epoch;
                let proposing = frame.is_multiple_of(32);
                if proposing {
                    id += 1;
                }
                samples.push(
                    bitpack::encode(&node, None, epoch, if proposing { id } else { 0 }).unwrap(),
                );
                if proposing {
                    base = Some(node.clone());
                } else {
                    samples.push(bitpack::encode(&node, base.as_ref(), epoch, id).unwrap());
                }
                // Verify full and delta samples against the same retained state.
                for sample in samples.iter().rev().take(if proposing { 1 } else { 2 }) {
                    let (_, decoded) =
                        decode_checkpoint(sample, base.as_ref().map(|b| (b, id)), epoch, true)
                            .unwrap();
                    assert!(decoded.unwrap() == node);
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
        let name = "fixed-fine";
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
