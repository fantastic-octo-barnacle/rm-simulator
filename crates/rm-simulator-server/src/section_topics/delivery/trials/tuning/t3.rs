// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! One fixed T3 completion-order ablation; reuse T2 development blocks.
use super::*;
use run::Experiment;

fn variants(seed: u64) -> Vec<(&'static str, Experiment)> {
    let base = Experiment {
        cadence: presentation::Cadence {
            chassis_ms: 32,
            projectiles_ms: 64,
            checkpoint_ms: 128,
        },
        weights: Some([2, 1, 1, 1]),
        completion_priority: false,
        whole: false,
        sections: true,
        impairment_seed: 0x2000_0000 + seed,
    };
    vec![
        (
            "rr-128",
            Experiment {
                weights: None,
                ..base
            },
        ),
        ("drr-128-211", base),
        (
            "completion-128-211",
            Experiment {
                completion_priority: true,
                ..base
            },
        ),
    ]
}
#[test]
fn completion_policy_keeps_exact_checkpoints_and_instrumentation_parity() {
    let sources = captures_at(2, 256, 71, 16);
    let experiment = variants(101)[2].1;
    for profile in ["clean", "limited", "blackout"] {
        let plain = t2::measured(&sources, profile, experiment, 0, false);
        let mut traced = t2::measured(&sources, profile, experiment, 0, true);
        traced["diagnostics"] = serde_json::Value::Null;
        assert_eq!(plain, traced);
        assert!(plain["metrics"]["delivered_checkpoints"].as_u64().unwrap() > 0);
    }
}
#[test]
#[ignore = "T3 block: build first; 18 fixed runs, no tuning during screen"]
fn t3_block() {
    assert_eq!(
        crate::compression::selected(),
        crate::compression::Codec::zstd_dictionary(3)
    );
    let players: u32 = std::env::var("RM_SECTION_T3_PLAYERS")
        .unwrap()
        .parse()
        .unwrap();
    let profile = std::env::var("RM_SECTION_T3_PROFILE").unwrap();
    assert!([2, 12].contains(&players) && ["clean", "rtt", "limited"].contains(&profile.as_str()));
    let seed: u64 = std::env::var("RM_SECTION_T3_SEED")
        .map(|value| value.parse().unwrap())
        .unwrap_or(101);
    assert!([101, 102, 103].contains(&seed));
    let sources = captures_at(players, 626 + 3750, 0x1000_0000 + seed, 16);
    let profile_id = ["clean", "rtt", "limited"]
        .iter()
        .position(|&p| p == profile)
        .unwrap();
    let mut order = variants(seed);
    if seed != 101 {
        order.retain(|(id, _)| *id != "rr-128");
    }
    let mut random = 0x4000_0000_u64 + seed + players as u64 * 17 + profile_id as u64;
    for i in (1..order.len()).rev() {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        order.swap(i, (random >> 32) as usize % (i + 1));
    }
    for (ordinal, (id, experiment)) in order.into_iter().enumerate() {
        let mut row = t2::measured_seed(&sources, &profile, experiment, 626, true, seed);
        row["variant"] = id.into();
        row["execution_ordinal"] = ordinal.into();
        row["scope"]="T3 completion-order ablation on reused T2 development blocks; no independent validation".into();
        if seed != 101 {
            row["scope"] = "T3 fixed-policy development replication; no holdout validation".into();
        }
        println!("RM_SECTION_T3 {row}");
    }
}

#[test]
#[ignore = "Export seed 102 truth poses for a paired audit of existing T3 arrivals"]
fn projectile_truth() {
    let sources = captures_at(2, 626 + 3750, 0x1000_0000 + 102, 16);
    let mut digest = Sha256::new();
    for source in &sources {
        digest.update(snapshot_codec::encode_player_message(
            &ServerMessage::Snapshot(Box::new(source.clone())),
        ));
        let positions: Vec<_> = source
            .field
            .projectiles
            .iter()
            .map(|p| (p.id, p.position_m))
            .collect();
        println!(
            "RM_PROJECTILE_TRUTH {}",
            serde_json::json!([source.snapshot_id, source.field.time_ns, positions])
        );
    }
    println!(
        "RM_PROJECTILE_SOURCE_HASH {digest:x}",
        digest = digest.finalize()
    );
}
