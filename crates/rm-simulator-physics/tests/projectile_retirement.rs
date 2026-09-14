// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Projectile lifetime arms: projectile-specific contact restitution and low-speed
//! post-contact retirement. Checks what the combine rule actually does to
//! ball/scenery and ball/robot pairs, that free flight is never retired, and
//! that the dwell window survives a checkpoint.
use rm_simulator_physics::{
    Caliber, Pose, Shot, TargetFrames, Team, WorldPhysics,
    chassis::ChassisConfig,
    projectile::{MAX_FLIGHT_NS, ProjectilePolicy, RETIRE_DWELL_NS, RemovalReason},
    tick_ns,
};

/// A muzzle pose pitched about world +y: a positive angle aims downward.
fn pitched(translation_m: [f64; 3], pitch_rad: f64) -> Pose {
    let (s, c) = (pitch_rad / 2.0).sin_cos();
    Pose {
        translation_m,
        rotation_wxyz: [c, 0.0, s, 0.0],
    }
}

/// Peak rebound speed of a ball dropped straight onto the flat floor.
fn bounce_speed_m_s(policy: ProjectilePolicy) -> f64 {
    let mut physics = WorldPhysics::new(&[], 0.0);
    physics.set_projectile_policy(policy).unwrap();
    let frames = TargetFrames::new(Vec::new());
    physics
        .fire(
            0,
            pitched([0.0, 0.0, 0.3], std::f64::consts::FRAC_PI_2),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 5.0,
            },
            None,
        )
        .unwrap();
    let mut rebound: f64 = 0.0;
    for tick in 0..200 {
        physics.step(tick * tick_ns(), &frames).unwrap();
        if let Some(ball) = physics.snapshot().first() {
            rebound = rebound.max(ball.velocity_m_s[2]);
        }
    }
    rebound
}

#[test]
fn projectile_restitution_scales_ball_bounces_and_keeps_robot_contacts() {
    // Rapier resolves a pair with the higher-priority combine rule, so the
    // projectile's `Min` beats the scenery's default `Average`.
    let control = bounce_speed_m_s(ProjectilePolicy::default());
    let softer = bounce_speed_m_s(ProjectilePolicy {
        restitution: 0.15,
        ..ProjectilePolicy::default()
    });
    let middle = bounce_speed_m_s(ProjectilePolicy {
        restitution: 0.30,
        ..ProjectilePolicy::default()
    });
    println!("rebound m/s: 0.45 {control:.3}, 0.30 {middle:.3}, 0.15 {softer:.3}");
    assert!(control > 1.0, "{control}");
    assert!(middle < control, "{control} {middle}");
    assert!(softer < control * 0.6, "{control} {softer}");

    // A chassis already asks for `Min` at its own low restitution, so the
    // ball's value cannot change a robot contact.
    let mut robot_rebound = Vec::new();
    for restitution in [0.45, 0.15] {
        let mut physics = WorldPhysics::new(&[], 0.0);
        physics
            .set_projectile_policy(ProjectilePolicy {
                restitution,
                ..ProjectilePolicy::default()
            })
            .unwrap();
        let frames = TargetFrames::new(Vec::new());
        physics
            .add_chassis(
                Team::Red,
                ChassisConfig::default(),
                Pose::at([2.0, 0.0, 0.2]),
            )
            .unwrap();
        physics
            .fire(
                0,
                Pose::at([0.0, 0.0, 0.45]),
                Shot::at_limit(Caliber::Mm17),
                None,
            )
            .unwrap();
        let mut back: f64 = 0.0;
        for tick in 0..300 {
            physics.step(tick * tick_ns(), &frames).unwrap();
            if let Some(ball) = physics.snapshot().first() {
                back = back.max(-ball.velocity_m_s[0]);
            }
        }
        robot_rebound.push(back);
    }
    assert!(
        (robot_rebound[0] - robot_rebound[1]).abs() < 0.2,
        "{robot_rebound:?}"
    );
}

#[test]
fn retirement_waits_for_the_dwell_and_spares_balls_in_flight() {
    let policy = ProjectilePolicy {
        retire_speed_m_s: Some(1.0),
        retire_dwell_ns: RETIRE_DWELL_NS,
        ..ProjectilePolicy::default()
    };
    let mut physics = WorldPhysics::new(&[], 0.0);
    physics.set_projectile_policy(policy).unwrap();
    let frames = TargetFrames::new(Vec::new());
    // A lobbed shot rises through an apex slower than the threshold.
    physics
        .fire(
            0,
            pitched([0.0, 0.0, 0.5], -std::f64::consts::FRAC_PI_2),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 0.8,
            },
            None,
        )
        .unwrap();
    let mut removed_ns = None;
    for tick in 0..4_000 {
        physics.step(tick * tick_ns(), &frames).unwrap();
        let alive = !physics.snapshot().is_empty();
        if tick < 60 {
            // Climbing, at the apex or falling, never touching anything.
            assert!(alive, "a ball in free flight was retired at tick {tick}");
        }
        if !alive {
            removed_ns = Some((tick + 1) * tick_ns());
            break;
        }
    }
    let removed_ns = removed_ns.expect("the settled ball is retired");
    assert!(removed_ns < MAX_FLIGHT_NS, "{removed_ns}");
    let removals = physics.take_removals();
    assert_eq!(removals.len(), 1);
    assert_eq!(removals[0].reason, RemovalReason::Retired);
    assert!(removals[0].speed_m_s <= 1.0, "{:?}", removals[0]);
    assert!(removals[0].since_first_contact_ns.is_some());

    // The rollback policy keeps the same ball to its hard flight limit.
    let mut control = WorldPhysics::new(&[], 0.0);
    control
        .set_projectile_policy(ProjectilePolicy::default().without_retirement())
        .unwrap();
    control
        .fire(
            0,
            pitched([0.0, 0.0, 0.5], -std::f64::consts::FRAC_PI_2),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 0.8,
            },
            None,
        )
        .unwrap();
    for tick in 0..2_000 {
        control.step(tick * tick_ns(), &frames).unwrap();
    }
    assert_eq!(control.snapshot().len(), 1);
}

#[test]
fn dwell_state_survives_a_snapshot_and_restore() {
    let policy = ProjectilePolicy {
        retire_speed_m_s: Some(1.0),
        ..ProjectilePolicy::default()
    };
    let mut physics = WorldPhysics::new(&[], 0.0);
    physics.set_projectile_policy(policy).unwrap();
    let frames = TargetFrames::new(Vec::new());
    physics
        .fire(
            0,
            Pose::at([0.0, 0.0, 0.05]),
            Shot {
                caliber: Caliber::Mm17,
                speed_m_s: 0.5,
            },
            None,
        )
        .unwrap();
    let mut open_ns = None;
    for tick in 0..2_000 {
        physics.step(tick * tick_ns(), &frames).unwrap();
        if let Some(ball) = physics.snapshot().first()
            && ball.dwell_since_ns.is_some()
        {
            open_ns = Some((tick + 1) * tick_ns());
            break;
        }
    }
    let open_ns = open_ns.expect("the ball settles on the floor");
    let checkpoint = physics.snapshot();
    assert_eq!(checkpoint[0].dwell_since_ns, Some(open_ns));

    let mut restored = WorldPhysics::new(&[], 0.0);
    restored.set_projectile_policy(policy).unwrap();
    restored.restore_projectile(&checkpoint[0]).unwrap();
    assert_eq!(restored.snapshot()[0].dwell_since_ns, Some(open_ns));
}
