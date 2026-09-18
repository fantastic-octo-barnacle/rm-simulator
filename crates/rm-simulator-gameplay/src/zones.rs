// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Section 5.5.3 buff point constants and the terrain crossing courses.
//!
//! Occupation effects are derived from a robot's zone contacts whenever they
//! are read, so they end with the contact's two-second expiry. Terrain
//! crossing buffs are timed and live on [`crate::RobotState`].
use crate::{SECOND_TICKS, ZoneKind};

/// Section 5.5.3.1: an Occupy status outlives the last detection by 2 s.
pub const ZONE_EXPIRY_TICKS: u64 = 2 * SECOND_TICKS;
/// Section 5.5.3.2: own Base Buff Point defense.
pub const BASE_ZONE_DEFENSE_PCT: u32 = 50;
/// Section 5.5.3.3: Central Elevated Ground Buff Point defense.
pub const CENTRAL_HIGHLAND_DEFENSE_PCT: u32 = 25;
/// Section 5.5.3.4: own Trapezoid-Shaped Elevated Ground defense.
pub const TRAPEZOID_HIGHLAND_DEFENSE_PCT: u32 = 50;
/// Section 5.5.3.6: occupiable Outpost Buff Point defense.
pub const OUTPOST_ZONE_DEFENSE_PCT: u32 = 25;
/// Section 5.5.3.6: an opponent's destroyed outpost's point is occupiable only
/// within the first five minutes.
pub const OPPONENT_OUTPOST_ZONE_UNTIL_TICKS: u64 = 300 * SECOND_TICKS;
/// Section 5.5.3.9: own Fortress Buff Point defense.
pub const FORTRESS_DEFENSE_PCT: u32 = 50;
/// Section 5.5.3.9: vulnerability while occupying the opponent's Fortress.
pub const FORTRESS_VULNERABILITY_PCT: u32 = 100;
/// Section 5.5.3.9: the opponent's Fortress opens three minutes into the round.
pub const OPPONENT_FORTRESS_FROM_TICKS: u64 = 180 * SECOND_TICKS;
/// Section 5.5.3.9: uninterrupted occupation that expands the opponent's Base
/// Protective Armor.
pub const FORTRESS_CAPTURE_TICKS: u64 = 20 * SECOND_TICKS;
/// Section 5.5.3.9: the capture timer is kept, paused, this long after the
/// robot is defeated or its Occupy status expires.
pub const FORTRESS_CAPTURE_RETAIN_TICKS: u64 = 3 * SECOND_TICKS;
/// Section 5.5.3.9: the Fortress heat cooling bonus is `Δ / 40`, at most 75.
pub const FORTRESS_COOLING_MAX: u32 = 75;
/// Section 5.5.3.9: the reserved allowance is `100 + 2 × ⌊Δ / 15⌋`, at most
/// 500 units.
pub const FORTRESS_RESERVE_MAX: u32 = 500;
/// Section 5.5.3.5: repeating a Launch Ramp, Elevated Ground or Road crossing
/// while such a buff lasts raises its defense to 50 %.
pub const CROSSING_STACKED_DEFENSE_PCT: u32 = 50;
/// Section 5.5.3.5: a Road crossing buff cannot be gained again for 15 s.
pub const ROAD_BUFF_COOLDOWN_TICKS: u64 = 15 * SECOND_TICKS;
/// Section 5.5.3.5: the Tunnel's double heat cooling lasts 120 s.
pub const TUNNEL_COOLING_TICKS: u64 = 120 * SECOND_TICKS;
/// Section 5.5.3.5: the Tunnel doubles heat cooling.
pub const TUNNEL_COOLING_MULTIPLIER: u32 = 2;

/// One terrain crossing course: the pads a robot must detect in turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Course {
    /// Pad numbers ([`crate::Zone::pad`]) in crossing order.
    pub pads: &'static [u8],
    /// Whether the pads may also be crossed in reverse. Section 5.5.3.5
    /// orders Road and Elevated Ground lower then higher; a Tunnel runs from
    /// either end through the middle.
    pub reversible: bool,
    /// Time allowed from the first pad to the last (section 5.5.3.5).
    pub window_ticks: u64,
    /// Defense granted on completion, in percent.
    pub defense_pct: u32,
    /// How long that defense lasts.
    pub defense_ticks: u64,
}

const ROAD: [Course; 1] = [Course {
    pads: &[0, 1],
    reversible: false,
    window_ticks: 3 * SECOND_TICKS,
    defense_pct: 25,
    defense_ticks: 5 * SECOND_TICKS,
}];
const ELEVATED: [Course; 1] = [Course {
    pads: &[0, 1],
    reversible: false,
    window_ticks: 5 * SECOND_TICKS,
    defense_pct: 25,
    defense_ticks: 30 * SECOND_TICKS,
}];
// Assumption: the manual orders only Road and Elevated Ground; the ramp's
// pads are numbered in the jump's direction and crossed in that order.
const RAMP: [Course; 1] = [Course {
    pads: &[0, 1],
    reversible: false,
    window_ticks: 10 * SECOND_TICKS,
    defense_pct: 25,
    defense_ticks: 30 * SECOND_TICKS,
}];
// Assumption: a team's six Tunnel pads form two tunnels of three pads each.
const TUNNEL: [Course; 2] = [
    Course {
        pads: &[0, 1, 2],
        reversible: true,
        window_ticks: 3 * SECOND_TICKS,
        defense_pct: 50,
        defense_ticks: 10 * SECOND_TICKS,
    },
    Course {
        pads: &[3, 4, 5],
        reversible: true,
        window_ticks: 3 * SECOND_TICKS,
        defense_pct: 50,
        defense_ticks: 10 * SECOND_TICKS,
    },
];

/// The terrain crossing courses of a zone kind; empty for other kinds.
///
/// ```
/// use rm_simulator_gameplay::{ZoneKind, zones::courses};
///
/// assert_eq!(courses(ZoneKind::Tunnel).len(), 2);
/// assert!(courses(ZoneKind::Base).is_empty());
/// ```
pub fn courses(kind: ZoneKind) -> &'static [Course] {
    match kind {
        ZoneKind::Road => &ROAD,
        ZoneKind::ElevatedCrossing => &ELEVATED,
        ZoneKind::LaunchRamp => &RAMP,
        ZoneKind::Tunnel => &TUNNEL,
        _ => &[],
    }
}

/// The course index and position of a terrain crossing pad, if it has one.
///
/// ```
/// use rm_simulator_gameplay::{ZoneKind, zones::course_position};
///
/// assert_eq!(course_position(ZoneKind::Tunnel, 4), Some((1, 1)));
/// assert_eq!(course_position(ZoneKind::Road, 7), None);
/// ```
pub fn course_position(kind: ZoneKind, pad: u8) -> Option<(u8, usize)> {
    courses(kind).iter().enumerate().find_map(|(course, c)| {
        c.pads
            .iter()
            .position(|p| *p == pad)
            .map(|position| (course as u8, position))
    })
}

/// Section 5.5.3.9 Fortress heat cooling bonus for base HP lost `delta`.
///
/// ```
/// use rm_simulator_gameplay::zones::fortress_cooling_bonus;
///
/// assert_eq!(fortress_cooling_bonus(1_000), 25);
/// assert_eq!(fortress_cooling_bonus(5_000), 75);
/// ```
pub fn fortress_cooling_bonus(delta: u32) -> u32 {
    (delta / 40).min(FORTRESS_COOLING_MAX)
}

/// Section 5.5.3.9 Fortress reserved allowance for base HP lost `delta`.
///
/// ```
/// use rm_simulator_gameplay::zones::fortress_reserve;
///
/// assert_eq!(fortress_reserve(0), 100);
/// assert_eq!(fortress_reserve(1_500), 300);
/// assert_eq!(fortress_reserve(5_000), 500);
/// ```
pub fn fortress_reserve(delta: u32) -> u32 {
    100u32
        .saturating_add((delta / 15).saturating_mul(2))
        .min(FORTRESS_RESERVE_MAX)
}
