// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Rule inventory pinned to the English V2.1.0 manual dated 2026-07-17.
//! `External` requires an authoritative caller input. `Tracked` records state
//! only. Neither label claims that the interactive simulator enforces the rule.
use serde::{Deserialize, Serialize};

/// Mechanism groups the coverage register accounts for (sections 5.1 to 5.8
/// and sections 6 to 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Mechanic {
    /// Setup, initialization, countdown, round, confirmation and match result
    /// (sections 6.3 to 6.9).
    MatchLifecycle,
    /// Round comparison and match progression (section 5.8).
    Victory,
    /// Detected raw damage, attribution, defenses, HP and attack totals
    /// (sections 5.1.1 and 5.5.3.1).
    Damage,
    /// Measured launch speed against the configured limit and its launch locks
    /// (section 5.1.2, Table 5-3).
    LaunchSpeed,
    /// Heat per shot, 10 Hz cooling and temporary and permanent launch locks
    /// (section 5.1.3, Figure 5-1).
    Heat,
    /// Power telemetry. Buffer energy and chassis power-off are not enforced
    /// (section 5.1.4).
    Power,
    /// Irregular disconnection, which blocks recovery and launch
    /// (sections 5.1.5.1 to 5.1.5.5).
    Disconnection,
    /// Explicit penalty damage and ejection. Card escalation is not automatic
    /// (section 5.1.6, sections 7.1 to 7.3).
    Penalties,
    /// Resupply healing and remote HP purchase (section 5.2.1).
    Recovery,
    /// Respawn timers, accelerated progress, paid respawn, weakness and
    /// invincibility (section 5.2.2).
    Respawn,
    /// Scheduled gold, spending and refunds (section 5.3.1, Tables 5-5, 5-6).
    Economy,
    /// Initial allowance, exchange limits and delays, consumption and sentry
    /// resupply (section 5.3.2, Tables 5-7 to 5-9).
    Allowance,
    /// Certified assembly completions and their rewards (section 5.3.3).
    Assembly,
    /// Explicit and shot experience awards with capped level thresholds
    /// (section 5.4.1, Table 5-11).
    Experience,
    /// Caller-supplied HP, heat and cooling. Level-up does not select
    /// performance tables (section 5.4.2, Tables 5-12 to 5-15).
    Performance,
    /// Base HP, shield, outpost protection, cumulative loss and rebuild
    /// opportunities (section 5.5.1).
    Base,
    /// Outpost damage and RFID rebuilding (section 5.5.1).
    Outpost,
    /// Rune activation and opportunities, owned by the world referee
    /// (section 5.5.2).
    Rune,
    /// Own-base defense, exchange and weakened-state removal (section 5.5.3.2).
    BaseZone,
    /// Central highland occupation (section 5.5.3.3).
    CentralHighland,
    /// Trapezoid highland occupation (section 5.5.3.4).
    TrapezoidHighland,
    /// Road, elevated ground, ramp and tunnel contacts (section 5.5.3.5).
    TerrainCrossing,
    /// Own-outpost exchange and weakness removal, plus the rebuild scan
    /// (section 5.5.3.6).
    OutpostZone,
    /// Assembly zone occupation (section 5.5.3.7).
    AssemblyZone,
    /// Healing, exchanges, sentry allowance and respawn acceleration at
    /// resupply (section 5.5.3.8).
    ResupplyZone,
    /// Fortress occupation (section 5.5.3.9).
    Fortress,
    /// Hero deployment and its speed report thresholds (section 5.6.1).
    HeroDeployment,
    /// Engineer special state (section 5.6.2).
    Engineer,
    /// Drone air-support time, grants, paid continuation and firing gate
    /// (section 5.6.3).
    Drone,
    /// Sentry pose state (section 5.6.4).
    Sentry,
    /// Dart launch windows, target selection and hits (section 5.6.5).
    Dart,
    /// Marks, vulnerability, drone counter and decoding (section 5.6.6,
    /// Appendix 1).
    Radar,
    /// Chassis energy expenditure and recharge in joules (section 5.6.7).
    Energy,
    /// Inspection and staging decisions (sections 6.1 and 6.2).
    Inspection,
    /// Timeout decisions (sections 6.3.1 and 6.3.2).
    Timeouts,
    /// Referee irregularity decisions (section 8).
    Irregularities,
    /// Appeal status and decisions (section 9).
    Appeals,
    /// Library snapshots and commands for custom clients (section 5.7).
    CustomClient,
}
/// How far the engine supports a mechanism group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Support {
    /// The engine executes the listed state transitions. The entry names its
    /// exclusions.
    Implemented,
    /// An authoritative caller supplies the observation and the engine applies
    /// the listed consequences.
    External,
    /// State can be recorded but the mechanic's consequences are not executed
    /// automatically.
    Tracked,
}
/// One row of the rule coverage register.
#[derive(Clone, Copy, Debug)]
pub struct RuleCoverage {
    /// Mechanism group the row describes.
    pub mechanic: Mechanic,
    /// Rulebook sections, tables or figures the row is pinned to.
    pub section: &'static str,
    /// Implementation status of the group.
    pub support: Support,
    /// What the engine does, and what remains outside it.
    pub behavior: &'static str,
}
macro_rules! rules {
    ($(($m:ident, $s:literal, $support:ident, $b:literal)),* $(,)?) => {
        /// Complete inventory of gameplay mechanism groups from sections 5.1 to
        /// 5.8, plus the competition process and human adjudication in
        /// sections 6 to 9, one row per [`Mechanic`].
        ///
        /// ```
        /// use rm_simulator_gameplay::coverage::{Mechanic, Support, RULES};
        ///
        /// let economy = RULES
        ///     .iter()
        ///     .find(|row| row.mechanic == Mechanic::Economy)
        ///     .expect("economy is registered");
        /// assert_eq!(economy.support, Support::Implemented);
        /// assert_eq!(economy.section, "5.3.1; Tables 5-5, 5-6");
        ///
        /// // Every mechanic group appears exactly once.
        /// let unique: std::collections::BTreeSet<_> = RULES.iter().map(|row| row.mechanic).collect();
        /// assert_eq!(unique.len(), RULES.len());
        /// ```
        pub const RULES: &[RuleCoverage] = &[$(RuleCoverage {
            mechanic: Mechanic::$m, section: $s, support: Support::$support, behavior: $b,
        }),*];
    }
}
rules![
    (
        MatchLifecycle,
        "6.3-6.9",
        Implemented,
        "Setup, initialization, countdown, round, confirmation, next round and match result"
    ),
    (
        Victory,
        "5.8",
        Implemented,
        "Ordered comparison and BO2/BO3/BO5; ambiguous comparisons require adjudication"
    ),
    (
        Damage,
        "5.1.1; 5.5.3.1; Table 5-2",
        External,
        "Caller reports detected hits; Table 5-2 amounts, strongest attack/defense/vulnerability, centre-square 150 %, Hero 42 mm immunity and attack totals applied here"
    ),
    (
        LaunchSpeed,
        "5.1.2; Table 5-3",
        External,
        "Caller reports measured speed and configured limit; launch locks applied here"
    ),
    (
        Heat,
        "5.1.3; Figure 5-1",
        Implemented,
        "Heat per shot, 10 Hz cooling and independent temporary/permanent locks; caller supplies performance"
    ),
    (
        Power,
        "5.1.4",
        Tracked,
        "Power telemetry only; buffer energy and chassis power-off not enforced"
    ),
    (
        Disconnection,
        "5.1.5.1-5.1.5.5",
        External,
        "Irregular-disconnection flag blocks recovery and launch; per-module penalties remain tracked"
    ),
    (
        Penalties,
        "5.1.6; 7.1-7.3",
        External,
        "Explicit penalty damage and ejection; automatic yellow/red card escalation not implemented"
    ),
    (
        Recovery,
        "5.2.1",
        External,
        "Observed resupply occupation/combat state drives healing; remote HP purchase and six-second delivery"
    ),
    (
        Respawn,
        "5.2.2",
        Implemented,
        "Timer, accelerated progress, paid respawn, weakened state and invincibility; power boost tracked only"
    ),
    (
        Economy,
        "5.3.1; Tables 5-5, 5-6",
        Implemented,
        "Scheduled gold, spending and refunds policy"
    ),
    (
        Allowance,
        "5.3.2; Tables 5-7-5-9",
        Implemented,
        "Initial allowance, exchange limits/delays, consumption and sentry resupply; over-allowance Hero 42 mm suspension"
    ),
    (
        Assembly,
        "5.3.3",
        External,
        "Certified completion grants income, level caps, defense and base healing; poses, interlocks and failure penalties tracked only"
    ),
    (
        Experience,
        "5.4.1; 5.5.2; Table 5-11",
        Implemented,
        "Launch, damage and kill experience, shared unattributed awards, Small Rune doubling and Large Rune sharing, capped level thresholds"
    ),
    (
        Performance,
        "5.4.2; Tables 5-12-5-14",
        Implemented,
        "Hero and Infantry performance types by level; other kinds use caller-fixed values; chassis power reported only"
    ),
    (
        Base,
        "5.5.1",
        Implemented,
        "HP, shield, outpost protection, cumulative loss and rebuild opportunities"
    ),
    (
        Outpost,
        "5.5.1",
        External,
        "Damage and RFID rebuilding; rotor animation/stop trajectory remains in world and is not corrected here"
    ),
    (
        Rune,
        "5.5.2",
        Tracked,
        "Existing world referee owns activation and opportunities; submit resulting buffs explicitly"
    ),
    (
        BaseZone,
        "5.5.3.2",
        External,
        "Own-side defense, exchange and weakened-state removal"
    ),
    (
        CentralHighland,
        "5.5.3.3",
        Tracked,
        "Occupation records; buffs not generated"
    ),
    (
        TrapezoidHighland,
        "5.5.3.4",
        Tracked,
        "Occupation records; buffs not generated"
    ),
    (
        TerrainCrossing,
        "5.5.3.5",
        Tracked,
        "Road, elevated ground, ramp and tunnel contacts; sequencing and rewards not generated"
    ),
    (
        OutpostZone,
        "5.5.3.6",
        External,
        "Own living outpost enables exchange/weakness removal; rebuild scan; other buffs tracked"
    ),
    (
        AssemblyZone,
        "5.5.3.7",
        Tracked,
        "Occupation records; buffs not generated"
    ),
    (
        ResupplyZone,
        "5.5.3.8",
        External,
        "Healing, exchanges, sentry allowance and respawn acceleration; other effects tracked"
    ),
    (
        Fortress,
        "5.5.3.9",
        Tracked,
        "Occupation records; reserves, cooling, vulnerability and armor-opening timer not implemented"
    ),
    (
        HeroDeployment,
        "5.6.1",
        Tracked,
        "Deployment observation; speed report can select deployment penalty thresholds"
    ),
    (
        Engineer,
        "5.6.2",
        Tracked,
        "Engineer special state observation"
    ),
    (
        Drone,
        "5.6.3",
        External,
        "Air-support time, income, paid continuation and firing gate; laser/radar counters and landing checks external"
    ),
    (
        Sentry,
        "5.6.4",
        Tracked,
        "Pose state; special multipliers not generated"
    ),
    (
        Dart,
        "5.6.5",
        Tracked,
        "Launch windows, target selection and hits may be recorded; effects need explicit damage/XP input"
    ),
    (
        Radar,
        "5.6.6; Appendix 1",
        Tracked,
        "Marks, vulnerability, drone counter and electromagnetic decoding observations"
    ),
    (
        Energy,
        "5.6.7",
        External,
        "Energy expenditure/recharge in joules, clamped to 0..40000; physical power limits not enforced"
    ),
    (
        Inspection,
        "6.1-6.2",
        Tracked,
        "Inspection and staging decisions"
    ),
    (
        Timeouts,
        "6.3.1-6.3.2",
        Tracked,
        "Timeout decisions; caller pauses by withholding ticks"
    ),
    (
        Irregularities,
        "8",
        Tracked,
        "Referee decisions; no automatic fault adjudication"
    ),
    (
        Appeals,
        "9",
        Tracked,
        "Appeal status and decision observations"
    ),
    (
        CustomClient,
        "5.7",
        Tracked,
        "Library snapshots/commands only; no official referee wire protocol"
    ),
];
