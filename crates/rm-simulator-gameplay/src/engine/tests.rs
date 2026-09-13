// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
use super::*;

fn config() -> Config {
    Config {
        robots: vec![
            RobotConfig {
                id: 1,
                team: Team::Red,
                kind: RobotKind::Infantry,
                max_hp: 200,
                heat_limit: 100,
                cooling_per_s: 20,
            },
            RobotConfig {
                id: 2,
                team: Team::Blue,
                kind: RobotKind::Infantry,
                max_hp: 200,
                heat_limit: 100,
                cooling_per_s: 20,
            },
            RobotConfig {
                id: 3,
                team: Team::Red,
                kind: RobotKind::Engineer,
                max_hp: 500,
                heat_limit: 0,
                cooling_per_s: 0,
            },
            RobotConfig {
                id: 4,
                team: Team::Red,
                kind: RobotKind::Sentry,
                max_hp: 400,
                heat_limit: 100,
                cooling_per_s: 20,
            },
            RobotConfig {
                id: 5,
                team: Team::Red,
                kind: RobotKind::Drone,
                max_hp: 100,
                heat_limit: 100,
                cooling_per_s: 20,
            },
        ],
        ..Config::default()
    }
}
fn running() -> Game {
    let mut game = Game::new(config()).unwrap();
    game.command(Command::BeginCountdown).unwrap();
    game.step(COUNTDOWN_TICKS).unwrap();
    game
}
fn damage(game: &mut Game, target: Target, amount: u32, kind: DamageKind) {
    game.command(Command::Damage {
        target,
        amount,
        kind,
        attacker: None,
    })
    .unwrap();
}
fn zone(game: &mut Game, robot: u32, kind: ZoneKind, detected: bool) {
    game.command(Command::ZoneDetection {
        robot,
        zone: Zone {
            kind,
            owner: Team::Red,
        },
        detected,
    })
    .unwrap();
}
#[test]
fn lifecycle_boundaries_and_partitioning() {
    let mut whole = Game::new(config()).unwrap();
    whole.command(Command::BeginRound).unwrap();
    let mut split = whole.clone();
    whole
        .step(SETUP_TICKS + INITIALIZATION_TICKS + COUNTDOWN_TICKS + ROUND_TICKS)
        .unwrap();
    for (ticks, phase) in [
        (SETUP_TICKS - 1, Phase::Setup),
        (1, Phase::Initialization),
        (INITIALIZATION_TICKS, Phase::Countdown),
        (COUNTDOWN_TICKS, Phase::Running),
        (ROUND_TICKS, Phase::RoundEnded),
    ] {
        split.step(ticks).unwrap();
        assert_eq!(split.snapshot().phase, phase);
    }
    assert_eq!(whole, split);
    let frozen = whole.snapshot().round_elapsed_ticks;
    whole.step(1234).unwrap();
    assert_eq!(whole.snapshot().round_elapsed_ticks, frozen);
}

#[test]
fn due_deliveries_drain_equal_and_staggered_prefixes_in_any_partition() {
    let mut whole = running();
    damage(&mut whole, Target::Robot(1), 100, DamageKind::Projectile);
    let now = whole.state.round_elapsed_ticks;
    whole.state.pending_deliveries = vec![
        Delivery {
            at_ticks: now + 2,
            robot: 1,
            kind: DeliveryKind::Ammo(Caliber::Mm17, 7),
        },
        Delivery {
            at_ticks: now + 2,
            robot: 1,
            kind: DeliveryKind::Hp,
        },
        Delivery {
            at_ticks: now + 4,
            robot: 1,
            kind: DeliveryKind::Ammo(Caliber::Mm17, 11),
        },
    ];
    let mut split = whole.clone();

    whole.step(4).unwrap();
    split.step(1).unwrap();
    assert_eq!(split.state.pending_deliveries.len(), 3);
    split.step(1).unwrap();
    assert_eq!(split.state.pending_deliveries.len(), 1);
    assert_eq!(split.state.pending_deliveries[0].at_ticks, now + 4);
    assert_eq!(
        split.snapshot().robots[0].allowance[Caliber::Mm17.index()],
        7
    );
    assert_eq!(split.snapshot().robots[0].hp, 160);
    split.step(2).unwrap();

    assert!(whole.state.pending_deliveries.is_empty());
    assert_eq!(
        whole.snapshot().robots[0].allowance[Caliber::Mm17.index()],
        18
    );
    assert_eq!(whole, split);
}

#[test]
fn clone_free_command_rejections_are_atomic() {
    let mut game = running();
    for command in [
        Command::CombatState {
            robot: 99,
            out_of_combat: true,
        },
        Command::Disconnection {
            robot: 99,
            irregular: true,
        },
        Command::Launch {
            robot: 1,
            caliber: Caliber::Mm17,
        },
        Command::ObserveLaunch {
            robot: 1,
            caliber: Caliber::Mm42,
        },
    ] {
        let before = game.clone();
        assert!(game.command(command).is_err());
        assert_eq!(game, before);
    }
}
#[test]
fn passive_income_uses_printed_countdown_boundaries() {
    let mut game = running();
    game.step(999).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 0);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 400);
    game.step(59_999).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 400);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 450);
    game.step(300_000).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 800);
    game.step(59_000).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 800);
}
#[test]
fn remote_ammo_delivers_at_six_seconds_and_rejection_is_atomic() {
    let mut game = running();
    game.step(1000).unwrap();
    let exchange = Command::ExchangeAmmo {
        robot: 1,
        caliber: Caliber::Mm17,
        amount: 100,
        remote: true,
    };
    let before = game.clone();
    assert_eq!(game.command(exchange.clone()), Err(Error::Ineligible));
    assert_eq!(game, before);
    game.command(Command::CombatState {
        robot: 1,
        out_of_combat: true,
    })
    .unwrap();
    game.command(exchange).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 250);
    game.step(5999).unwrap();
    assert_eq!(game.snapshot().robots[0].allowance[0], 0);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().robots[0].allowance[0], 100);
    let before = game.clone();
    assert_eq!(
        game.command(Command::ExchangeAmmo {
            robot: 1,
            caliber: Caliber::Mm17,
            amount: 1000,
            remote: true
        }),
        Err(Error::Limit)
    );
    assert_eq!(game, before);
}
#[test]
fn own_service_zone_required_and_false_samples_do_not_extend_expiry() {
    let mut game = running();
    game.step(1000).unwrap();
    zone(&mut game, 1, ZoneKind::Base, true);
    game.command(Command::ExchangeAmmo {
        robot: 1,
        caliber: Caliber::Mm17,
        amount: 10,
        remote: false,
    })
    .unwrap();
    zone(&mut game, 1, ZoneKind::Base, false);
    game.step(1000).unwrap();
    zone(&mut game, 1, ZoneKind::Base, false);
    game.step(999).unwrap();
    assert!(!game.snapshot().robots[0].zones.is_empty());
    game.step(1).unwrap();
    assert!(game.snapshot().robots[0].zones.is_empty());
}
#[test]
fn base_protection_shield_and_damage_credit() {
    let mut game = running();
    damage(
        &mut game,
        Target::Base(Team::Blue),
        9000,
        DamageKind::Projectile,
    );
    assert_eq!(game.snapshot().teams[1].base_hp, BASE_HP);
    assert_eq!(game.snapshot().teams[0].attack_damage, 0);
    damage(
        &mut game,
        Target::Outpost(Team::Blue),
        2000,
        DamageKind::Projectile,
    );
    damage(
        &mut game,
        Target::Base(Team::Blue),
        1150,
        DamageKind::Projectile,
    );
    let t = &game.snapshot().teams[1];
    assert_eq!(
        (t.base_hp, t.base_shield_hp, t.outpost_rebuild_opportunities),
        (4000, 0, 1)
    );
    assert_eq!(game.snapshot().teams[0].attack_damage, 2650);
    damage(
        &mut game,
        Target::Base(Team::Blue),
        9000,
        DamageKind::Projectile,
    );
    assert_eq!(game.snapshot().phase, Phase::RoundEnded);
    assert_eq!(
        game.snapshot().result,
        Some(RoundResult::Decided {
            winner: Some(Team::Red),
            reason: DecisionReason::BaseDestroyed
        })
    );
    let before = game.clone();
    assert_eq!(
        game.command(Command::Damage {
            target: Target::Robot(1),
            amount: 1,
            kind: DamageKind::Projectile,
            attacker: None
        }),
        Err(Error::Phase)
    );
    assert_eq!(game, before);
}
#[test]
fn penalties_ignore_defense_and_collision_does_not_credit_attack() {
    let mut game = running();
    zone(&mut game, 1, ZoneKind::Base, true);
    damage(&mut game, Target::Robot(1), 20, DamageKind::Collision);
    assert_eq!(game.snapshot().robots[0].hp, 190);
    assert_eq!(game.snapshot().teams[1].attack_damage, 0);
    damage(&mut game, Target::Robot(1), 20, DamageKind::Penalty);
    assert_eq!(game.snapshot().robots[0].hp, 170);
    assert_eq!(game.snapshot().teams[1].attack_damage, 20);
}
#[test]
fn normal_respawn_weakness_and_minimum_invincibility() {
    let mut game = running();
    damage(&mut game, Target::Robot(1), 200, DamageKind::Projectile);
    assert_eq!(
        game.snapshot().robots[0]
            .respawn
            .as_ref()
            .unwrap()
            .required_ticks,
        10_000
    );
    game.step(9999).unwrap();
    assert_eq!(game.snapshot().robots[0].hp, 0);
    game.step(1).unwrap();
    let r = &game.snapshot().robots[0];
    assert_eq!(r.hp, 20);
    assert!(r.weakened);
    assert_eq!(r.invincible_until_ticks, 40_000);
    zone(&mut game, 1, ZoneKind::Base, true);
    assert!(!game.snapshot().robots[0].weakened);
    assert_eq!(game.snapshot().robots[0].invincible_until_ticks, 20_000);
    damage(&mut game, Target::Robot(1), 200, DamageKind::Projectile);
    assert_eq!(game.snapshot().robots[0].hp, 20);
    game.step(10_000).unwrap();
    damage(&mut game, Target::Robot(1), 200, DamageKind::Projectile);
    assert_eq!(game.snapshot().robots[0].hp, 0);
}
#[test]
fn resupply_accelerates_respawn_but_disconnection_blocks_it() {
    let mut game = running();
    zone(&mut game, 1, ZoneKind::Resupply, true);
    damage(&mut game, Target::Robot(1), 200, DamageKind::Projectile);
    game.command(Command::Disconnection {
        robot: 1,
        irregular: true,
    })
    .unwrap();
    game.step(5000).unwrap();
    assert_eq!(game.snapshot().robots[0].hp, 0);
    game.command(Command::Disconnection {
        robot: 1,
        irregular: false,
    })
    .unwrap();
    game.step(2499).unwrap();
    assert_eq!(game.snapshot().robots[0].hp, 0);
    game.step(1).unwrap();
    assert!(game.snapshot().robots[0].alive());
}
#[test]
fn paid_respawn_cost_and_three_second_weakness() {
    let mut game = running();
    game.step(1000).unwrap();
    damage(&mut game, Target::Robot(1), 200, DamageKind::Projectile);
    game.command(Command::InstantRespawn { robot: 1 }).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 300);
    assert_eq!(game.snapshot().robots[0].hp, 200);
    assert!(game.snapshot().robots[0].weakened);
    zone(&mut game, 1, ZoneKind::Base, true);
    assert!(game.snapshot().robots[0].weakened);
    game.step(3000).unwrap();
    assert!(!game.snapshot().robots[0].weakened);
    damage(&mut game, Target::Robot(1), 1000, DamageKind::Penalty);
    assert_eq!(
        game.snapshot().robots[0]
            .respawn
            .as_ref()
            .unwrap()
            .required_ticks,
        30_400
    );
}
#[test]
fn remote_hp_is_cancelled_on_death_even_if_instantly_revived() {
    let mut game = running();
    game.step(1000).unwrap();
    damage(&mut game, Target::Robot(1), 100, DamageKind::Projectile);
    game.command(Command::CombatState {
        robot: 1,
        out_of_combat: true,
    })
    .unwrap();
    game.command(Command::ExchangeHp { robot: 1 }).unwrap();
    game.step(5999).unwrap();
    assert_eq!(game.snapshot().robots[0].hp, 100);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().robots[0].hp, 160);
    game.command(Command::ExchangeHp { robot: 1 }).unwrap();
    damage(&mut game, Target::Robot(1), 999, DamageKind::Projectile);
    assert!(game.state.pending_deliveries.is_empty());
    game.command(Command::InstantRespawn { robot: 1 }).unwrap();
    game.step(3000).unwrap();
    damage(&mut game, Target::Robot(1), 100, DamageKind::Penalty);
    game.step(3000).unwrap();
    assert_eq!(game.snapshot().robots[0].hp, 100);
}
#[test]
fn outpost_rebuild_requires_continuous_scan_and_opportunity() {
    let mut game = running();
    damage(
        &mut game,
        Target::Outpost(Team::Red),
        1500,
        DamageKind::Projectile,
    );
    damage(
        &mut game,
        Target::Base(Team::Red),
        1150,
        DamageKind::Projectile,
    );
    zone(&mut game, 3, ZoneKind::Outpost, true);
    game.step(4999).unwrap();
    assert_eq!(game.snapshot().teams[0].outpost_hp, 0);
    zone(&mut game, 3, ZoneKind::Outpost, false);
    game.step(1).unwrap();
    zone(&mut game, 3, ZoneKind::Outpost, true);
    game.step(4999).unwrap();
    assert_eq!(game.snapshot().teams[0].outpost_hp, 0);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().teams[0].outpost_hp, 750);
    assert!(game.snapshot().teams[0].outpost_ever_destroyed);
    assert_eq!(game.snapshot().teams[0].outpost_rebuild_opportunities, 0);
}
#[test]
fn rebuilt_outpost_cannot_be_rebuilt_at_five_minutes() {
    let mut game = running();
    game.step(295_000).unwrap();
    damage(
        &mut game,
        Target::Outpost(Team::Red),
        1500,
        DamageKind::Projectile,
    );
    damage(
        &mut game,
        Target::Base(Team::Red),
        1150,
        DamageKind::Projectile,
    );
    zone(&mut game, 3, ZoneKind::Outpost, true);
    game.step(5000).unwrap();
    assert_eq!(game.snapshot().teams[0].outpost_hp, 0);
}
#[test]
fn heat_cools_on_100_ms_boundaries_and_locks_until_zero() {
    let mut game = running();
    for _ in 0..11 {
        game.command(Command::Launch {
            robot: 4,
            caliber: Caliber::Mm17,
        })
        .unwrap();
    }
    assert!(game.snapshot().robots[3].overheated);
    assert_eq!(
        game.command(Command::Launch {
            robot: 4,
            caliber: Caliber::Mm17
        }),
        Err(Error::Ineligible)
    );
    game.step(99).unwrap();
    assert_eq!(game.snapshot().robots[3].heat_tenths, 1100);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().robots[3].heat_tenths, 1080);
    game.step(5300).unwrap();
    assert!(game.snapshot().robots[3].overheated);
    game.step(100).unwrap();
    assert!(!game.snapshot().robots[3].overheated);
}
#[test]
fn speed_lock_is_independent_of_heat_and_respawn() {
    let mut game = running();
    game.command(Command::LaunchSpeed {
        robot: 4,
        caliber: Caliber::Mm17,
        measured_mm_s: 35_000,
        limit_mm_s: 30_000,
        deployed: false,
    })
    .unwrap();
    assert_eq!(game.snapshot().robots[3].speed_locked_until_ticks, 20_000);
    game.step(19_999).unwrap();
    assert!(!game.snapshot().robots[3].can_launch(19_999, Caliber::Mm17));
    game.step(1).unwrap();
    assert!(game.snapshot().robots[3].can_launch(20_000, Caliber::Mm17));
    game.command(Command::LaunchSpeed {
        robot: 4,
        caliber: Caliber::Mm17,
        measured_mm_s: 40_000,
        limit_mm_s: 30_000,
        deployed: false,
    })
    .unwrap();
    damage(&mut game, Target::Robot(4), 1000, DamageKind::Penalty);
    game.command(Command::InstantRespawn { robot: 4 }).unwrap();
    game.step(3000).unwrap();
    assert!(game.snapshot().robots[3].speed_locked_for_round);
}
#[test]
fn assembly_caps_and_rewards() {
    let mut game = running();
    game.command(Command::AwardExperience {
        robot: 1,
        tenths: 999_999,
    })
    .unwrap();
    assert_eq!(game.snapshot().robots[0].level, 5);
    game.command(Command::AssemblyCompleted {
        team: Team::Red,
        level: 1,
    })
    .unwrap();
    game.step(60_000).unwrap();
    game.command(Command::AssemblyCompleted {
        team: Team::Red,
        level: 2,
    })
    .unwrap();
    game.command(Command::AwardExperience {
        robot: 1,
        tenths: 999_999,
    })
    .unwrap();
    assert_eq!(game.snapshot().robots[0].level, 7);
    game.step(60_000).unwrap();
    game.command(Command::AssemblyCompleted {
        team: Team::Red,
        level: 3,
    })
    .unwrap();
    game.step(60_000).unwrap();
    game.command(Command::AssemblyCompleted {
        team: Team::Red,
        level: 4,
    })
    .unwrap();
    assert_eq!(game.snapshot().teams[0].base_shield_hp, 2150);
    assert_eq!(game.snapshot().teams[0].assembly_income_per_10_s, 150);
    let before = game.clone();
    assert_eq!(
        game.command(Command::AssemblyCompleted {
            team: Team::Red,
            level: 4
        }),
        Err(Error::Ineligible)
    );
    assert_eq!(game, before);
}
#[test]
fn sentry_claims_accumulated_resupply_and_drone_cannot_purchase_ammo() {
    let mut game = running();
    game.step(181_000).unwrap();
    zone(&mut game, 4, ZoneKind::Resupply, true);
    game.step(1000).unwrap();
    assert_eq!(game.snapshot().robots[3].allowance[0], 600);
    game.step(1000).unwrap();
    assert_eq!(game.snapshot().robots[3].allowance[0], 600);
    game.command(Command::CombatState {
        robot: 5,
        out_of_combat: true,
    })
    .unwrap();
    assert_eq!(
        game.command(Command::ExchangeAmmo {
            robot: 5,
            caliber: Caliber::Mm17,
            amount: 100,
            remote: true
        }),
        Err(Error::Ineligible)
    );
}
#[test]
fn drone_support_consumes_time_then_gold() {
    let mut game = running();
    game.command(Command::AirSupport {
        robot: 5,
        active: true,
    })
    .unwrap();
    game.step(30_000).unwrap();
    assert_eq!(game.snapshot().robots[4].air_support_ticks, 0);
    assert_eq!(game.snapshot().teams[0].gold, 400);
    game.step(1).unwrap();
    assert_eq!(game.snapshot().teams[0].gold, 399);
    assert_eq!(game.snapshot().robots[4].air_support_ticks, 999);
    game.command(Command::AirSupport {
        robot: 5,
        active: false,
    })
    .unwrap();
    game.step(29_999).unwrap();
    assert_eq!(game.snapshot().robots[4].air_support_ticks, 20_999);
}
#[test]
fn unequal_surviving_bases_require_adjudication() {
    let mut game = running();
    damage(
        &mut game,
        Target::Outpost(Team::Blue),
        1500,
        DamageKind::Projectile,
    );
    damage(
        &mut game,
        Target::Base(Team::Blue),
        200,
        DamageKind::Projectile,
    );
    game.command(Command::EndRound).unwrap();
    assert_eq!(
        game.snapshot().result,
        Some(RoundResult::NeedsRefereeDecision)
    );
    assert_eq!(game.command(Command::ConfirmResult), Err(Error::Ineligible));
    game.command(Command::Adjudicate {
        winner: Some(Team::Red),
    })
    .unwrap();
    game.command(Command::ConfirmResult).unwrap();
    assert_eq!(game.snapshot().phase, Phase::Confirmed);
}
#[test]
fn bo3_draw_replays_and_confirm_is_not_repeatable() {
    let mut game = running();
    for winner in [None, Some(Team::Blue), Some(Team::Blue)] {
        game.command(Command::EndRound).unwrap();
        game.command(Command::Adjudicate { winner }).unwrap();
        game.command(Command::ConfirmResult).unwrap();
        assert_eq!(game.command(Command::ConfirmResult), Err(Error::Phase));
        if game.snapshot().phase != Phase::MatchEnded {
            game.command(Command::BeginCountdown).unwrap();
            game.step(COUNTDOWN_TICKS).unwrap();
        }
    }
    assert_eq!(game.snapshot().match_winner, Some(Team::Blue));
    assert_eq!(game.snapshot().rounds.len(), 3);
    assert_eq!(game.command(Command::BeginRound), Err(Error::Phase));
    let tick = game.snapshot().tick;
    game.command(Command::ResetMatch).unwrap();
    assert_eq!(game.snapshot().tick, tick);
    assert!(game.snapshot().rounds.is_empty());
    assert_eq!(game.snapshot().phase, Phase::Idle);
}
#[test]
fn unknown_observations_remain_distinct_from_implemented_effects() {
    let mut game = running();
    game.command(Command::Observe(ElementObservation {
        mechanic: coverage::Mechanic::Radar,
        team: Some(Team::Red),
        robot: None,
        state: "decoding".into(),
        measurements: Default::default(),
        observed_at_ticks: 999,
    }))
    .unwrap();
    assert_eq!(game.snapshot().observations[0].observed_at_ticks, 0);
    assert!(game.snapshot().buffs.is_empty());
    let json = serde_json::to_string(game.snapshot()).unwrap();
    let decoded: Snapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(game.snapshot(), &decoded);
    let unique: std::collections::BTreeSet<_> =
        coverage::RULES.iter().map(|r| r.mechanic).collect();
    assert_eq!(unique.len(), coverage::RULES.len());
}
#[test]
fn mixed_timers_are_partition_independent() {
    let mut game = running();
    game.step(1000).unwrap();
    zone(&mut game, 1, ZoneKind::Resupply, true);
    game.command(Command::CombatState {
        robot: 1,
        out_of_combat: true,
    })
    .unwrap();
    game.command(Command::ExchangeAmmo {
        robot: 1,
        caliber: Caliber::Mm17,
        amount: 100,
        remote: true,
    })
    .unwrap();
    damage(&mut game, Target::Robot(1), 200, DamageKind::Projectile);
    game.command(Command::Launch {
        robot: 4,
        caliber: Caliber::Mm17,
    })
    .unwrap();
    let mut split = game.clone();
    game.step(100_000).unwrap();
    for ticks in [1, 99, 900, 5999, 1, 70_123, 22_877] {
        split.step(ticks).unwrap();
    }
    assert_eq!(game, split);
}
#[test]
fn invalid_config_and_clock_overflow_reject_without_mutation() {
    let mut c = config();
    c.robots.push(c.robots[0].clone());
    assert_eq!(Game::new(c), Err(Error::Invalid));
    let mut game = running();
    let before = game.clone();
    assert_eq!(game.step(u64::MAX), Err(Error::ClockOverflow));
    assert_eq!(game, before);
}

#[test]
fn observed_illegal_launch_reaches_exact_q2_permanent_lock() {
    let mut game = running();
    for _ in 0..19 {
        game.command(Command::ObserveLaunch {
            robot: 1,
            caliber: Caliber::Mm17,
        })
        .unwrap();
    }
    assert!(!game.snapshot().robots[0].heat_locked_for_round);
    game.command(Command::ObserveLaunch {
        robot: 1,
        caliber: Caliber::Mm17,
    })
    .unwrap();
    let r = &game.snapshot().robots[0];
    assert!(r.heat_locked_for_round);
    assert_eq!(r.shots_launched, [20, 0]);
    assert_eq!(r.shots_over_allowance, [20, 0]);
    game.step(10_000).unwrap();
    assert_eq!(game.snapshot().robots[0].heat_tenths, 0);
    assert!(game.snapshot().robots[0].heat_locked_for_round);
}

#[test]
fn strongest_defense_and_vulnerability_expire_before_damage() {
    let mut game = running();
    for (source, defense, vulnerability) in [
        (coverage::Mechanic::Rune, 25, 0),
        (coverage::Mechanic::Radar, 0, 15),
        (coverage::Mechanic::Fortress, 0, 10),
    ] {
        game.command(Command::ApplyBuff(Buff {
            target: Target::Robot(1),
            source,
            attack_pct: 100,
            defense_pct: defense,
            vulnerability_pct: vulnerability,
            cooling_multiplier: 1,
            expires_ticks: 1000,
        }))
        .unwrap();
    }
    zone(&mut game, 1, ZoneKind::Base, true);
    damage(&mut game, Target::Robot(1), 100, DamageKind::Projectile);
    assert_eq!(game.snapshot().robots[0].hp, 135); // 50% defense, 15% vulnerability.
    game.step(1000).unwrap();
    damage(&mut game, Target::Robot(1), 100, DamageKind::Projectile);
    assert_eq!(game.snapshot().robots[0].hp, 85);
    assert!(game.snapshot().buffs.is_empty());
}

#[test]
fn bo2_draw_finishes_after_two_rounds_and_launch_stops_at_end() {
    let mut c = config();
    c.format = MatchFormat::Bo2;
    let mut game = Game::new(c).unwrap();
    for _ in 0..2 {
        game.command(Command::BeginCountdown).unwrap();
        game.step(COUNTDOWN_TICKS).unwrap();
        assert!(game.can_launch(4, Caliber::Mm17));
        game.command(Command::EndRound).unwrap();
        assert!(!game.can_launch(4, Caliber::Mm17));
        game.command(Command::Adjudicate { winner: None }).unwrap();
        game.command(Command::ConfirmResult).unwrap();
    }
    assert_eq!(game.snapshot().phase, Phase::MatchEnded);
    assert_eq!(game.snapshot().match_winner, None);
}

#[test]
fn result_order_outpost_then_attack_then_robot_hp() {
    let mut game = running();
    damage(
        &mut game,
        Target::Outpost(Team::Blue),
        1,
        DamageKind::Projectile,
    );
    damage(&mut game, Target::Robot(1), 199, DamageKind::Projectile);
    game.command(Command::EndRound).unwrap();
    assert_eq!(
        game.snapshot().result,
        Some(RoundResult::Decided {
            winner: Some(Team::Red),
            reason: DecisionReason::Outpost
        })
    );
    let mut game = running();
    damage(&mut game, Target::Robot(1), 1, DamageKind::Projectile);
    game.command(Command::EndRound).unwrap();
    assert_eq!(
        game.snapshot().result,
        Some(RoundResult::Decided {
            winner: Some(Team::Blue),
            reason: DecisionReason::AttackDamage
        })
    );
    let mut game = running();
    game.command(Command::EndRound).unwrap();
    assert_eq!(
        game.snapshot().result,
        Some(RoundResult::Decided {
            winner: Some(Team::Red),
            reason: DecisionReason::RobotHp
        })
    );
}

#[test]
fn chassis_energy_is_clamped_and_requires_resupply_for_charging() {
    let mut game = running();
    game.command(Command::Energy {
        robot: 1,
        consumed_j: 30_000,
        recharge_surplus_j: 0,
    })
    .unwrap();
    assert_eq!(game.snapshot().robots[0].chassis_energy_j, Some(0));
    let before = game.clone();
    assert_eq!(
        game.command(Command::Energy {
            robot: 1,
            consumed_j: 0,
            recharge_surplus_j: 1
        }),
        Err(Error::Ineligible)
    );
    assert_eq!(game, before);
    zone(&mut game, 1, ZoneKind::Resupply, true);
    game.command(Command::Energy {
        robot: 1,
        consumed_j: 0,
        recharge_surplus_j: u32::MAX,
    })
    .unwrap();
    assert_eq!(game.snapshot().robots[0].chassis_energy_j, Some(40_000));
}
