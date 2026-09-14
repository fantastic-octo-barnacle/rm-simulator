// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Fixed one-seed T2 screen. Each process runs one block, sharing its captures.
use super::*;
use run::{Experiment, trial_configured};

const SEED: u64 = 101;
const WORKLOAD_SEED: u64 = 0x1000_0000 + SEED;
const IMPAIRMENT_SEED: u64 = 0x2000_0000 + SEED;

fn variants() -> Vec<(String, Experiment)> {
    let mut variants = Vec::new();
    let base = Experiment {
        cadence: presentation::Cadence {
            chassis_ms: 32,
            projectiles_ms: 64,
            checkpoint_ms: 128,
        },
        weights: None,
        completion_priority: false,
        whole: false,
        sections: true,
        impairment_seed: IMPAIRMENT_SEED,
    };
    variants.push((
        "whole".into(),
        Experiment {
            whole: true,
            sections: false,
            ..base
        },
    ));
    for period in [32, 64, 128] {
        let experiment = Experiment {
            cadence: presentation::Cadence {
                checkpoint_ms: period,
                ..base.cadence
            },
            ..base
        };
        variants.push((format!("rr-{period}"), experiment));
        for weights in [[1, 1, 1, 1], [2, 1, 1, 1], [1, 2, 1, 1], [1, 1, 2, 1]] {
            variants.push((
                format!("drr-{period}-{}{}{}", weights[0], weights[1], weights[2]),
                Experiment {
                    weights: Some(weights),
                    ..experiment
                },
            ));
        }
    }
    variants
}
pub(super) fn measured(
    sources: &[SimulationState],
    profile: &str,
    experiment: Experiment,
    warmup: u64,
    enabled: bool,
) -> serde_json::Value {
    measured_seed(sources, profile, experiment, warmup, enabled, SEED)
}

pub(super) fn measured_seed(
    sources: &[SimulationState],
    profile: &str,
    experiment: Experiment,
    warmup: u64,
    enabled: bool,
    seed: u64,
) -> serde_json::Value {
    let mut row = trial_configured(
        sources,
        0x1000_0000 + seed,
        profile,
        warmup,
        enabled,
        Some(experiment),
    );
    let path = if experiment.whole {
        "whole"
    } else {
        "sections"
    };
    let errors = if experiment.whole {
        "whole_pose_error"
    } else {
        "section_pose_error"
    };
    let controls = if experiment.whole {
        "whole_controls"
    } else {
        "section_controls"
    };
    let mut result = serde_json::json!({
        "metrics": row[path].take(), "errors": row[errors].take(), "controls": row[controls].take(),
        "diagnostics": row["diagnostics"].take(), "sender_lifetime": row["sender_lifetime"].take(),
        "coverage_denominators": row["coverage_denominators"].take(),
        "wire_sha256": row["wire_sha256"], "message_sha256": row["message_sha256"],
        "captured_stream_sha256": row["captured_stream_sha256"], "impairment_sha256": row["impairment_sha256"],
        "compression": row["compression"], "prepared_dictionary_by_copy": true, "cadence": row["cadence"], "weights": experiment.weights,
        "profile": profile, "players": row["players"], "seed_id": seed,
        "workload_seed": 0x1000_0000_u64 + seed, "impairment_seed": experiment.impairment_seed,
        "warmup_ms": row["warmup_ms"], "measured_ms": row["measured_ms"],
        "downstream_budget_bytes_s": row["downstream_budget_bytes_s"], "upstream_budget_bytes_s": row["upstream_budget_bytes_s"],
        "upstream_lifetime_bytes_datagrams": row["upstream_lifetime_bytes_datagrams"],
        "shots_launched_measured": row["shots_launched_measured"],
        "scope": "T2 single-seed screen; independent codec process context per replay; injected clock; no CPU/native-wire/prediction/render claims"
    });
    if experiment.completion_priority {
        result["completion_priority"] = row["completion_priority"].take();
    }
    result
}
#[test]
fn weighted_delivery_preserves_exact_state_and_observer_parity() {
    let sources = captures_at(2, 192, 71, 16);
    for (_, experiment) in (0..2)
        .flat_map(|_| variants())
        .filter(|(_, e)| e.weights.is_some())
    {
        let plain = measured(&sources, "limited", experiment, 0, false);
        let mut traced = measured(&sources, "limited", experiment, 0, true);
        traced["diagnostics"] = serde_json::Value::Null;
        assert_eq!(plain, traced);
        assert!(plain["metrics"]["delivered_checkpoints"].as_u64().unwrap() > 0);
    }
}
#[test]
#[ignore = "T2 block: prebuild, freeze manifest, run parallel independent processes"]
fn t2_block() {
    assert_eq!(
        crate::compression::selected(),
        crate::compression::Codec::zstd_dictionary(3)
    );
    let players: u32 = std::env::var("RM_SECTION_T2_PLAYERS")
        .unwrap()
        .parse()
        .unwrap();
    let profile = std::env::var("RM_SECTION_T2_PROFILE").unwrap();
    assert!([2, 12].contains(&players) && ["clean", "rtt", "limited"].contains(&profile.as_str()));
    let sources = captures_at(players, 626 + 3750, WORKLOAD_SEED, 16);
    let mut order = variants();
    let profile_id = ["clean", "rtt", "limited"]
        .iter()
        .position(|&p| p == profile)
        .unwrap();
    let mut random = 0x3000_0000 + SEED + players as u64 * 17 + profile_id as u64;
    for i in (1..order.len()).rev() {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        order.swap(i, (random >> 32) as usize % (i + 1));
    }
    let selected = std::env::var("RM_SECTION_T2_VARIANTS").ok();
    for (ordinal, (id, experiment)) in order.into_iter().enumerate() {
        if selected
            .as_ref()
            .is_some_and(|s| !s.split(',').any(|v| v == id))
        {
            continue;
        }
        let mut row = measured(&sources, &profile, experiment, 626, !experiment.whole);
        row["variant"] = id.into();
        row["execution_ordinal"] = ordinal.into();
        println!("RM_SECTION_T2 {row}");
    }
}
