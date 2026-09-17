// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Section 5.4.2 performance system: HP, chassis power, heat limit and cooling
//! by robot type and level (Tables 5-12 to 5-14 of the V2.1.0 manual).
use serde::{Deserialize, Serialize};

/// Highest robot level (Table 5-11).
pub const MAX_LEVEL: u8 = 10;

/// Hero type (Table 5-12). Section 5.4.2 defaults an unselected Hero to
/// long-range attack-focused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeroType {
    /// Melee-focused: more HP and power, slower cooling.
    MeleeFocused,
    /// Long-range attack-focused, the section 5.4.2 default.
    #[default]
    LongRangeFocused,
}

/// Infantry chassis type (Table 5-13). Section 5.4.2 defaults an unselected
/// chassis to HP-focused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InfantryChassis {
    /// Power-focused: less HP, more chassis power.
    PowerFocused,
    /// HP-focused, the section 5.4.2 default.
    #[default]
    HpFocused,
}

/// Infantry 17 mm launching mechanism type (Table 5-14). Section 5.4.2
/// defaults an unselected launcher to cooling-focused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InfantryLauncher {
    /// Burst-focused: high heat limit, slow cooling.
    BurstFocused,
    /// Cooling-focused, the section 5.4.2 default.
    #[default]
    CoolingFocused,
}

/// A robot's performance values at one level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Maximum HP; positive.
    pub max_hp: u32,
    /// Chassis power limit in watts; reported, not enforced (section 5.1.4).
    pub chassis_power_w: u32,
    /// Barrel heat limit Q0 in heat units (section 5.1.3).
    pub heat_limit: u32,
    /// Barrel heat cooling in heat units per second (section 5.1.3).
    pub cooling_per_s: u32,
}

/// Where a robot's [`Stats`] come from.
///
/// ```
/// use rm_simulator_gameplay::{HeroType, InfantryChassis, InfantryLauncher, Performance};
///
/// // Table 5-13 and Table 5-14, level 1: HP-focused chassis, cooling-focused launcher.
/// let infantry = Performance::Infantry {
///     chassis: InfantryChassis::HpFocused,
///     launcher: InfantryLauncher::CoolingFocused,
/// };
/// let stats = infantry.stats(1);
/// assert_eq!((stats.max_hp, stats.heat_limit, stats.cooling_per_s), (200, 40, 12));
/// // Table 5-12, level 10 long-range Hero.
/// let hero = Performance::Hero(HeroType::LongRangeFocused).stats(10);
/// assert_eq!((hero.max_hp, hero.heat_limit, hero.cooling_per_s), (400, 190, 50));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Performance {
    /// Table 5-12 by Hero type.
    Hero(HeroType),
    /// Table 5-13 chassis and Table 5-14 launcher.
    Infantry {
        /// Chassis type, which sets HP and power.
        chassis: InfantryChassis,
        /// Launching mechanism type, which sets heat and cooling.
        launcher: InfantryLauncher,
    },
    /// Caller-supplied values for robots the performance system does not cover
    /// (section 5.4.2: Engineer, Sentry, Drone HP) and for scenarios. Levels do
    /// not change them.
    Fixed(Stats),
}

/// Table 5-12, melee-focused Hero: HP, power, heat limit, cooling by level.
const HERO_MELEE: [[u32; 4]; 10] = [
    [260, 70, 200, 12],
    [300, 75, 210, 14],
    [330, 80, 220, 16],
    [360, 85, 230, 18],
    [400, 90, 240, 20],
    [430, 95, 250, 22],
    [460, 100, 260, 24],
    [500, 105, 270, 26],
    [530, 110, 280, 28],
    [600, 120, 300, 30],
];
/// Table 5-12, long-range attack-focused Hero.
const HERO_LONG_RANGE: [[u32; 4]; 10] = [
    [200, 50, 160, 20],
    [220, 55, 162, 23],
    [240, 60, 164, 26],
    [260, 65, 166, 29],
    [280, 70, 168, 32],
    [300, 75, 170, 35],
    [320, 80, 175, 38],
    [340, 85, 180, 41],
    [360, 90, 185, 44],
    [400, 100, 190, 50],
];
/// Table 5-13, power-focused Infantry chassis: HP and power by level.
const INFANTRY_POWER: [[u32; 2]; 10] = [
    [150, 60],
    [175, 65],
    [200, 70],
    [225, 75],
    [250, 80],
    [275, 85],
    [300, 90],
    [325, 95],
    [350, 100],
    [400, 100],
];
/// Table 5-13, HP-focused Infantry chassis.
const INFANTRY_HP: [[u32; 2]; 10] = [
    [200, 45],
    [225, 50],
    [250, 55],
    [275, 60],
    [300, 65],
    [325, 70],
    [350, 75],
    [375, 80],
    [400, 90],
    [400, 100],
];
/// Table 5-14, burst-focused 17 mm launcher: heat limit and cooling by level.
const INFANTRY_BURST: [[u32; 2]; 10] = [
    [170, 5],
    [180, 7],
    [190, 9],
    [200, 11],
    [210, 12],
    [220, 13],
    [230, 14],
    [240, 16],
    [250, 18],
    [260, 20],
];
/// Table 5-14, cooling-focused 17 mm launcher.
const INFANTRY_COOLING: [[u32; 2]; 10] = [
    [40, 12],
    [48, 14],
    [56, 16],
    [64, 18],
    [72, 20],
    [80, 22],
    [88, 24],
    [96, 26],
    [114, 28],
    [120, 30],
];

impl Performance {
    /// Values at `level`, clamped to 1..=[`MAX_LEVEL`].
    pub fn stats(self, level: u8) -> Stats {
        let row = usize::from(level.clamp(1, MAX_LEVEL) - 1);
        match self {
            Self::Hero(kind) => {
                let [max_hp, chassis_power_w, heat_limit, cooling_per_s] = match kind {
                    HeroType::MeleeFocused => HERO_MELEE[row],
                    HeroType::LongRangeFocused => HERO_LONG_RANGE[row],
                };
                Stats {
                    max_hp,
                    chassis_power_w,
                    heat_limit,
                    cooling_per_s,
                }
            }
            Self::Infantry { chassis, launcher } => {
                let [max_hp, chassis_power_w] = match chassis {
                    InfantryChassis::PowerFocused => INFANTRY_POWER[row],
                    InfantryChassis::HpFocused => INFANTRY_HP[row],
                };
                let [heat_limit, cooling_per_s] = match launcher {
                    InfantryLauncher::BurstFocused => INFANTRY_BURST[row],
                    InfantryLauncher::CoolingFocused => INFANTRY_COOLING[row],
                };
                Stats {
                    max_hp,
                    chassis_power_w,
                    heat_limit,
                    cooling_per_s,
                }
            }
            Self::Fixed(stats) => stats,
        }
    }
    /// The section 5.4.2 default for a robot that selected nothing: a
    /// long-range Hero, an HP-focused cooling-focused Infantry. Engineer and
    /// Sentry have fixed values in section 3.2 (an Engineer's 250 HP and
    /// 120 W; a full-automatic Sentry's 400 HP, 100 W, 260 heat limit and
    /// 30/s cooling). Drone, Dart and Radar have no HP and return `None`.
    ///
    /// ```
    /// use rm_simulator_gameplay::{Performance, RobotKind};
    ///
    /// let sentry = Performance::default_for(RobotKind::Sentry).unwrap().stats(1);
    /// assert_eq!((sentry.max_hp, sentry.heat_limit), (400, 260));
    /// assert!(Performance::default_for(RobotKind::Drone).is_none());
    /// ```
    pub fn default_for(kind: crate::RobotKind) -> Option<Self> {
        match kind {
            crate::RobotKind::Hero => Some(Self::Hero(HeroType::default())),
            crate::RobotKind::Infantry => Some(Self::Infantry {
                chassis: InfantryChassis::default(),
                launcher: InfantryLauncher::default(),
            }),
            crate::RobotKind::Engineer => Some(Self::Fixed(Stats {
                max_hp: 250,
                chassis_power_w: 120,
                heat_limit: 0,
                cooling_per_s: 0,
            })),
            crate::RobotKind::Sentry => Some(Self::Fixed(Stats {
                max_hp: 400,
                chassis_power_w: 100,
                heat_limit: 260,
                cooling_per_s: 30,
            })),
            crate::RobotKind::Drone | crate::RobotKind::Dart | crate::RobotKind::Radar => None,
        }
    }
    /// Whether this source may describe a robot of `kind`: Hero tables for a
    /// Hero, Infantry tables for Infantry, fixed values for anything, and a
    /// positive maximum HP in every case.
    pub fn fits(self, kind: crate::RobotKind) -> bool {
        match self {
            Self::Hero(_) => kind == crate::RobotKind::Hero,
            Self::Infantry { .. } => kind == crate::RobotKind::Infantry,
            Self::Fixed(stats) => stats.max_hp > 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_monotonic_in_hp_and_match_printed_endpoints() {
        for performance in [
            Performance::Hero(HeroType::MeleeFocused),
            Performance::Hero(HeroType::LongRangeFocused),
            Performance::Infantry {
                chassis: InfantryChassis::PowerFocused,
                launcher: InfantryLauncher::BurstFocused,
            },
            Performance::Infantry {
                chassis: InfantryChassis::HpFocused,
                launcher: InfantryLauncher::CoolingFocused,
            },
        ] {
            for level in 1..MAX_LEVEL {
                let (a, b) = (performance.stats(level), performance.stats(level + 1));
                assert!(b.max_hp >= a.max_hp && b.heat_limit >= a.heat_limit);
                assert!(b.cooling_per_s >= a.cooling_per_s);
            }
        }
        assert_eq!(
            Performance::Hero(HeroType::MeleeFocused).stats(10),
            Stats {
                max_hp: 600,
                chassis_power_w: 120,
                heat_limit: 300,
                cooling_per_s: 30
            }
        );
        let burst = Performance::Infantry {
            chassis: InfantryChassis::PowerFocused,
            launcher: InfantryLauncher::BurstFocused,
        };
        assert_eq!(burst.stats(1).max_hp, 150);
        assert_eq!(burst.stats(0), burst.stats(1));
        assert_eq!(burst.stats(99).heat_limit, 260);
    }
}
