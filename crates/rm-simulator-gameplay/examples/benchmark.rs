// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Reproducible microbenchmarks for coarse stepping and command transaction cost.
use rm_simulator_gameplay::*;
use std::hint::black_box;
use std::time::{Duration, Instant};

fn measured(mut run: impl FnMut()) -> Duration {
    let start = Instant::now();
    run();
    start.elapsed()
}

fn game() -> Game {
    Game::new(Config {
        robots: vec![RobotConfig {
            id: 1,
            team: Team::Red,
            kind: RobotKind::Sentry,
            max_hp: 400,
            heat_limit: 1_000_000,
            cooling_per_s: 0,
        }],
        ..Config::default()
    })
    .unwrap()
}

fn main() {
    const TICKS: u64 = SETUP_TICKS + INITIALIZATION_TICKS + COUNTDOWN_TICKS;
    let coarse = measured(|| {
        let mut game = game();
        game.command(Command::BeginRound).unwrap();
        game.step(black_box(TICKS)).unwrap();
        black_box(game);
    });
    let partitioned = measured(|| {
        let mut game = game();
        game.command(Command::BeginRound).unwrap();
        for _ in 0..black_box(TICKS) {
            game.step(1).unwrap();
        }
        black_box(game);
    });

    let mut direct_game = game();
    direct_game.command(Command::BeginCountdown).unwrap();
    direct_game.step(COUNTDOWN_TICKS).unwrap();
    for n in 0..512 {
        direct_game
            .command(Command::CombatState {
                robot: 1,
                out_of_combat: n % 2 == 0,
            })
            .unwrap();
    }
    let mut cloned_game = direct_game.clone();
    const COMMANDS: u32 = 20_000;
    let direct = measured(|| {
        for n in 0..black_box(COMMANDS) {
            direct_game
                .command(Command::CombatState {
                    robot: 1,
                    out_of_combat: n % 2 == 0,
                })
                .unwrap();
        }
        black_box(&direct_game);
    });
    let clone_transaction = measured(|| {
        for n in 0..black_box(COMMANDS) {
            let mut next = cloned_game.clone();
            next.command(Command::CombatState {
                robot: 1,
                out_of_combat: n % 2 == 0,
            })
            .unwrap();
            cloned_game = next;
        }
        black_box(&cloned_game);
    });

    println!("setup/countdown coarse step: {coarse:?}");
    println!("same ticks as 1-tick calls: {partitioned:?}");
    println!("{COMMANDS} direct simple commands: {direct:?}");
    println!("{COMMANDS} commands with full clone transaction: {clone_transaction:?}");
}
