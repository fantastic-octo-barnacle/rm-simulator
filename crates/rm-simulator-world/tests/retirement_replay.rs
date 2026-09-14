// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! A field restored from a checkpoint must spend and retire its
//! balls like the field it was captured from, for every arm of the experiment.
//! Solver warm starts are not restorable, so positions are compared with a
//! tolerance; the retirement decisions themselves must agree exactly.
use rm_simulator_world::{Caliber, Field, FieldConfig, Pose, Shot, projectile::ProjectilePolicy};

/// The arms under test: the adopted default (retirement at 2 m/s), the
/// rollback policy with retirement off, both restitutions, all three
/// retirement thresholds, the immediate ablation, the combination and both
/// lower TTLs.
fn arms() -> Vec<(&'static str, ProjectilePolicy)> {
    let default = ProjectilePolicy::default();
    let control = default.without_retirement();
    vec![
        ("default", default),
        ("control", control),
        (
            "R30",
            ProjectilePolicy {
                restitution: 0.30,
                ..control
            },
        ),
        (
            "R15",
            ProjectilePolicy {
                restitution: 0.15,
                ..control
            },
        ),
        (
            "D05",
            ProjectilePolicy {
                retire_speed_m_s: Some(0.5),
                ..control
            },
        ),
        (
            "D10",
            ProjectilePolicy {
                retire_speed_m_s: Some(1.0),
                ..control
            },
        ),
        (
            "D20",
            ProjectilePolicy {
                retire_speed_m_s: Some(2.0),
                ..control
            },
        ),
        (
            "D10_immediate",
            ProjectilePolicy {
                retire_speed_m_s: Some(1.0),
                retire_dwell_ns: 0,
                ..control
            },
        ),
        (
            "R30_D10",
            ProjectilePolicy {
                restitution: 0.30,
                retire_speed_m_s: Some(1.0),
                ..control
            },
        ),
        (
            "TTL3",
            ProjectilePolicy {
                max_flight_ns: 3_000_000_000,
                ..control
            },
        ),
        (
            "TTL2",
            ProjectilePolicy {
                max_flight_ns: 2_000_000_000,
                ..control
            },
        ),
    ]
}

/// A bare field with a box to bounce off, under one arm's policy.
fn field(policy: ProjectilePolicy) -> Field {
    let config = FieldConfig {
        runes: Vec::new(),
        outposts: Vec::new(),
        projectile_policy: policy,
        ..FieldConfig::default()
    };
    let mut field = Field::new(&config).unwrap();
    field
        .add_static_mesh(
            vec![
                [4.0, -1.0, 0.0],
                [4.0, 1.0, 0.0],
                [4.0, -1.0, 1.5],
                [4.0, 1.0, 1.5],
            ],
            vec![[0, 1, 2], [1, 3, 2]],
        )
        .unwrap();
    field
}

/// Volley of slow and fast shots that bounce, roll and settle.
fn volley(field: &mut Field, tick_offset: u64) {
    for index in 0..6u64 {
        let pitch = -0.05 + index as f64 * 0.03;
        let (s, c) = (pitch / 2.0).sin_cos();
        let muzzle = Pose {
            translation_m: [0.0, index as f64 * 0.2 - 0.5, 0.3],
            rotation_wxyz: [c, 0.0, s, 0.0],
        };
        let shot = Shot {
            caliber: if index % 2 == 0 {
                Caliber::Mm17
            } else {
                Caliber::Mm42
            },
            speed_m_s: 4.0 + index as f64,
        };
        field.fire(muzzle, shot, None).unwrap();
        field.step(20 + tick_offset).unwrap();
    }
}

#[test]
fn restored_fields_retire_the_same_balls_as_the_run_they_came_from() {
    for (name, policy) in arms() {
        let mut continuous = field(policy);
        volley(&mut continuous, 0);
        continuous.step(1_500).unwrap();

        let checkpoint = continuous.snapshot();
        assert_eq!(
            checkpoint.restore.as_ref().unwrap().projectile_policy,
            policy,
            "{name}: the checkpoint must carry the policy"
        );
        let geometry = continuous.static_geometry_snapshot();
        let mut replayed = Field::restore(&checkpoint, &geometry, 0.0).unwrap();
        assert_eq!(replayed.projectile_policy(), policy, "{name}");

        // Both fields step the same ticks from the same checkpoint.
        for _ in 0..30 {
            continuous.step(50).unwrap();
            replayed.step(50).unwrap();
            let live = continuous.projectile_snapshots();
            let replay = replayed.projectile_snapshots();
            let live_ids: Vec<u64> = live.iter().map(|ball| ball.id).collect();
            let replay_ids: Vec<u64> = replay.iter().map(|ball| ball.id).collect();
            assert_eq!(
                live_ids, replay_ids,
                "{name}: a restored field retired a different set of balls"
            );
            for (a, b) in live.iter().zip(&replay) {
                let distance_m = (0..3)
                    .map(|axis| (a.position_m[axis] - b.position_m[axis]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                assert!(
                    distance_m < 0.05,
                    "{name}: ball {} drifted {distance_m:.4} m after restore",
                    a.id
                );
                assert_eq!(
                    a.dwell_since_ns.is_some(),
                    b.dwell_since_ns.is_some(),
                    "{name}: ball {} disagrees about its dwell window",
                    a.id
                );
            }
        }
    }
}
