// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Deterministic RMUC 2026 gameplay, independent of physics, CAD and Bevy.
//!
//! Call [`Game::command`] with authoritative observations and [`Game::step`]
//! with explicit 1 ms ticks. Commands are transactional; rejected commands do
//! not change state. [`coverage::RULES`] distinguishes implemented transitions
//! from external observations and unsupported rules. See `docs/gameplay.md`
//! for integration responsibilities and documented rule ambiguities.
#![deny(missing_docs)]
pub mod coverage;
mod engine;
pub mod live;
mod policy;
mod state;
pub use engine::*;
pub use state::*;

/// Simulator clock resolution, an application choice, not a rulebook constant.
pub const TICK_NS: u64 = 1_000_000;
/// Ticks in one second at the 1 ms resolution, so rule durations multiply
/// cleanly.
pub const SECOND_TICKS: u64 = 1_000;
/// Sections 6.3, 6.4, 6.5 and 6.6 of the V2.1.0 manual.
pub const SETUP_TICKS: u64 = 180 * SECOND_TICKS;
/// Section 6.4 of the V2.1.0 manual, 15 s.
pub const INITIALIZATION_TICKS: u64 = 15 * SECOND_TICKS;
/// Section 6.5 of the V2.1.0 manual, 5 s.
pub const COUNTDOWN_TICKS: u64 = 5 * SECOND_TICKS;
/// Section 6.6 of the V2.1.0 manual, 420 s.
pub const ROUND_TICKS: u64 = 420 * SECOND_TICKS;
/// Section 5.5.1.
pub const BASE_HP: u32 = 5_000;
/// Section 5.5.1; shield damage counts as attack damage, not base HP loss.
pub const BASE_SHIELD_HP: u32 = 150;
/// Section 5.5.1.
pub const OUTPOST_HP: u32 = 1_500;

#[cfg(test)]
mod conformance_tests {
    use super::*;

    fn sentry_game() -> Game {
        Game::new(Config {
            robots: vec![RobotConfig {
                id: 7,
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

    #[test]
    fn live_and_standalone_share_default_income_and_launch_accounting() {
        let mut game = sentry_game();
        game.command(Command::BeginCountdown).unwrap();
        game.step(COUNTDOWN_TICKS).unwrap();

        let mut live = live::Resources::default();
        live.settings.initial_allowance = [300, 0];
        live.settings.enforce_allowance = true;
        live.add_robot(7, Team::Red);
        live.reset();

        let mut previous = 0;
        for elapsed in [999, 1_000, 60_999, 61_000, 360_999, 361_000] {
            game.step(elapsed - previous).unwrap();
            live.advance_to(elapsed * TICK_NS);
            assert_eq!(
                live.gold,
                std::array::from_fn(|index| game.snapshot().teams[index].gold)
            );
            previous = elapsed;
        }

        for _ in 0..3 {
            assert!(live.check_launch(Some(7), Caliber::Mm17).is_ok());
            live.launch(Some(7), Caliber::Mm17);
            game.command(Command::Launch {
                robot: 7,
                caliber: Caliber::Mm17,
            })
            .unwrap();
        }
        let standalone = &game.snapshot().robots[0];
        assert_eq!(live.robots[0].allowance, standalone.allowance);
        assert_eq!(live.robots[0].shots, standalone.shots_launched);
    }
}
