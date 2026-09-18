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
mod performance;
mod policy;
mod state;
pub mod zones;
pub use engine::*;
pub use performance::*;
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
