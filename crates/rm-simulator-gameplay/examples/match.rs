// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Run with `just gameplay-demo`. No CAD, renderer, sockets or wall clock.
use rm_simulator_gameplay::{
    COUNTDOWN_TICKS, Command, Config, DamageKind, Game, INITIALIZATION_TICKS, MatchFormat, Phase,
    RobotConfig, RobotKind, SETUP_TICKS, Target, Team,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut game = Game::new(Config {
        format: MatchFormat::Bo3,
        robots: Team::BOTH
            .into_iter()
            .enumerate()
            .map(|(i, team)| RobotConfig {
                id: i as u32,
                team,
                kind: RobotKind::Infantry,
                // Level-1 HP-focused infantry HP, Table 5-13. Heat and cooling
                // below are scenario parameters, not a performance-table preset.
                max_hp: 200,
                heat_limit: 100,
                cooling_per_s: 20,
            })
            .collect(),
    })?;
    while game.snapshot().phase != Phase::MatchEnded {
        game.command(Command::BeginRound)?;
        game.step(SETUP_TICKS + INITIALIZATION_TICKS + COUNTDOWN_TICKS)?;
        game.step(1_000)?;
        // Certified contacts supplied by a hypothetical physics adapter.
        for (target, amount) in [
            (Target::Outpost(Team::Blue), 1500),
            (Target::Base(Team::Blue), 5150),
        ] {
            game.command(Command::Damage {
                target,
                amount,
                kind: DamageKind::Projectile,
                attacker: Some(Team::Red),
            })?;
        }
        println!(
            "Round {}: {:?}",
            game.snapshot().round,
            game.snapshot().result
        );
        game.command(Command::ConfirmResult)?;
    }
    println!("Match winner: {:?}", game.snapshot().match_winner);
    Ok(())
}
