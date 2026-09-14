// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Trains the ZSTD checkpoint dictionary that `compression` embeds.
//!
//! Both peers of a connection must use the same dictionary, so it is compiled
//! into the binary instead of being negotiated on the wire. Train it from the
//! repository root and record the printed hash in
//! `crates/rm-simulator-server/assets/README.md`:
//!
//! ```sh
//! cargo run -p rm-simulator-server --example train_checkpoint_dictionary --locked
//! ```
//!
//! Samples are the exact uncompressed envelopes the production encoder feeds
//! its compressor: each checkpoint's independent envelope plus the delta the
//! acknowledged-baseline encoder builds against the previous checkpoint. They
//! come from deterministic synthetic gameplay runs at the production 32 ms
//! publication period: varied driving, turret sweeping, fire bursts, training
//! bots joining and leaving, referee match control and rune activity. A
//! dictionary pays off when the frames resemble the traffic it will see, so
//! these runs mix driving, firing and collection churn instead of one
//! straight-line loop. They are deliberately not the four workloads the
//! `compression_comparison` example evaluates, which keeps that measurement out
//! of sample.
//!
//! `zstd::dict::from_samples` is deterministic for a fixed sample set and
//! `zstd-sys` version, so the hash is reproducible. Retrain and update the
//! asset whenever the checkpoint schema changes.
#[path = "support/dictionary_workloads.rs"]
mod dictionary_workloads;
use dictionary_workloads::{SCENARIOS, Scenario};
use rm_simulator_server::{
    snapshot_codec::difference,
    udp_snapshot::{Wire, envelope_bytes},
};
use sha2::{Digest, Sha256};
/// Bytes of the production dictionary embedded in both peers.
const DICTIONARY_BYTES: usize = 32 * 1024;
/// Existing production asset; binary experiments write elsewhere.
const DEFAULT_OUTPUT: &str = "crates/rm-simulator-server/assets/checkpoint-dictionary.zstd";
fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_OUTPUT.into());
    let mut samples = Vec::new();
    for scenario in &SCENARIOS {
        let run = scenario_samples(scenario);
        let bytes: usize = run.iter().map(Vec::len).sum();
        println!(
            "scenario={} chassis={} frames={} samples={} sample_bytes={bytes}",
            scenario.name,
            scenario.chassis,
            scenario.frames,
            run.len(),
        );
        samples.extend(run);
    }
    let bytes: usize = samples.iter().map(Vec::len).sum();
    let dictionary =
        zstd::dict::from_samples(&samples, DICTIONARY_BYTES).expect("ZSTD dictionary training");
    std::fs::write(&output, &dictionary).expect("write dictionary");
    println!(
        "samples={} sample_bytes={bytes} dictionary_bytes={} output={output} sha256={:x}",
        samples.len(),
        dictionary.len(),
        Sha256::digest(&dictionary),
    );
}

/// Preserve the production trainer's original independent/previous-frame mix.
fn scenario_samples(scenario: &Scenario) -> Vec<Vec<u8>> {
    let mut previous = None;
    let mut samples = Vec::new();
    for value in dictionary_workloads::checkpoints(scenario) {
        samples.push(envelope_bytes(&Wire::Independent {
            epoch: 0,
            state: value.clone(),
        }));
        if let Some(baseline) = &previous {
            samples.push(envelope_bytes(&Wire::Delta {
                epoch: 0,
                base: 1,
                patch: difference(baseline, &value),
            }));
        }
        previous = Some(value);
    }
    samples
}
