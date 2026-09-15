// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Resources driven by an existing referee's round clock. This component has
//! no phase or physics authority; hosts call `reset`, `advance_to` and `launch`.
use crate::policy::{IncomeSchedule, allowance_permits_launch, record_ammo_launch};
use serde::{Deserialize, Serialize};

/// Tunable live-play policy. Defaults: Table 5-5 income and Table 5-7
/// infantry allowance. Enforcement is disabled by default for simulator practice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Whether a zero allowance refuses a launch. Off by default for practice.
    pub enforce_allowance: bool,
    /// Rounds granted at reset and to a newly added robot, in caliber index
    /// order.
    pub initial_allowance: [u32; 2],
    /// Whether scheduled income is granted. Disabling skips crossed boundaries
    /// without back-payment.
    pub economy_enabled: bool,
    /// Team gold granted at 1 s elapsed (Table 5-5).
    pub initial_gold: u32,
    /// Gold granted at each whole minute from 61 s through 301 s (Table 5-5).
    pub minute_gold: u32,
    /// Gold granted at 361 s, the final minute (Table 5-5).
    pub final_minute_gold: u32,
    /// Referee resupply price per projectile, based on Table 5-6 local exchange.
    pub ammo_price: [u32; 2],
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enforce_allowance: false,
            initial_allowance: [0; 2],
            economy_enabled: true,
            initial_gold: IncomeSchedule::DEFAULT.initial,
            minute_gold: IncomeSchedule::DEFAULT.minute,
            final_minute_gold: IncomeSchedule::DEFAULT.final_minute,
            ammo_price: [1, 10],
        }
    }
}
/// Resource counters tracked for one chassis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobotResources {
    /// Chassis id assigned by the server. Ids are never reused.
    pub id: u32,
    /// Team the chassis belongs to.
    pub team: crate::Team,
    /// Remaining projectile allowance in caliber index order.
    pub allowance: [u32; 2],
    /// Successful shots counted per caliber.
    pub shots: [u64; 2],
}
/// Live resource counters for one match, driven by the referee's clock. Clone
/// it to snapshot a state; it has no clock or physics of its own.
///
/// ```
/// use rm_simulator_gameplay::live::Resources;
/// use rm_simulator_gameplay::{Caliber, Team};
///
/// let mut resources = Resources::default();
/// resources.settings.initial_allowance = [10, 0];
/// resources.settings.enforce_allowance = true;
/// resources.add_robot(7, Team::Red);
/// resources.reset();
///
/// resources.advance_to(1_000_000_000); // 1 s elapsed
/// assert_eq!(resources.gold, [400, 400]);
///
/// assert!(resources.check_launch(Some(7), Caliber::Mm17).is_ok());
/// resources.launch(Some(7), Caliber::Mm17);
/// assert_eq!(resources.robots[0].shots[0], 1);
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    /// Tunable policy. Edits affect future operations only.
    pub settings: Settings,
    /// Team gold in [`crate::Team::BOTH`] order.
    pub gold: [u32; 2],
    /// Counters for the chassis currently in the match.
    pub robots: Vec<RobotResources>,
    /// Free-camera shots have no chassis; they never consume a robot's ammo.
    pub unassigned_shots: [u64; 2],
    /// Highest match time advanced to, in nanoseconds. `advance_to` never goes
    /// backwards.
    pub match_time_ns: u64,
    /// Schedule index of the next Table 5-5 grant to pay, `0..=7`.
    ///
    /// This is paid history, not a function of `match_time_ns`: a boundary
    /// crossed while the economy was disabled is never paid, even after the
    /// economy is re-enabled, and the clock alone cannot say which grants are
    /// still due. Recomputing it from `match_time_ns` (or skipping it in serde)
    /// would back-pay those skipped boundaries. The boundary times themselves
    /// depend only on the index, so editing the amounts leaves it valid.
    next_income: usize,
}
/// Referee-only edits. Caliber index 0 is 17 mm; index 1 is 42 mm.
///
/// ```
/// use rm_simulator_gameplay::live::{Edit, Resources};
/// use rm_simulator_gameplay::{Caliber, Team};
///
/// let mut resources = Resources::default();
/// resources.add_robot(7, Team::Red);
/// resources.edit(Edit::Gold {
///     team: Team::Red,
///     gold: 100,
/// })?;
/// resources.edit(Edit::BuyAmmo {
///     id: 7,
///     caliber: Caliber::Mm17,
///     amount: 50,
/// })?;
/// // Table 5-6 local exchange prices 17 mm at 1 gold per projectile.
/// assert_eq!(resources.robots[0].allowance[0], 50);
/// assert_eq!(resources.gold[0], 50);
/// # Ok::<(), &'static str>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edit {
    /// Replace the whole policy.
    Settings(Settings),
    /// Set one team's gold balance.
    Gold {
        /// Team to set.
        team: crate::Team,
        /// New balance.
        gold: u32,
    },
    /// Set a chassis's allowance and shot counters.
    Robot {
        /// Chassis to edit; an unknown id is rejected.
        id: u32,
        /// New allowance in caliber index order.
        allowance: [u32; 2],
        /// New shot counters in caliber index order.
        shots: [u64; 2],
    },
    /// Set the free-camera shot counters.
    UnassignedShots {
        /// New counters in caliber index order.
        shots: [u64; 2],
    },
    /// Operator resupply; does not assert an RFID zone or remote purchase.
    BuyAmmo {
        /// Chassis buying the ammo; an unknown id is rejected.
        id: u32,
        /// Caliber bought.
        caliber: crate::Caliber,
        /// Positive rounds to buy; the cost is this amount times the
        /// configured price.
        amount: u32,
    },
}
impl Resources {
    /// Add a chassis with the configured initial allowance. Adding an id that
    /// is already present does nothing.
    pub fn add_robot(&mut self, id: u32, team: crate::Team) {
        if !self.robots.iter().any(|r| r.id == id) {
            self.robots.push(RobotResources {
                id,
                team,
                allowance: self.settings.initial_allowance,
                shots: [0; 2],
            });
        }
    }
    /// Remove a chassis and its counters. Removing an unknown id does nothing.
    pub fn remove_robot(&mut self, id: u32) {
        self.robots.retain(|r| r.id != id);
    }
    /// Preserve policy and roster, reset per-round state. Used at start and reset.
    pub fn reset(&mut self) {
        self.gold = [0; 2];
        self.unassigned_shots = [0; 2];
        self.match_time_ns = 0;
        self.next_income = 0;
        for r in &mut self.robots {
            r.allowance = self.settings.initial_allowance;
            r.shots = [0; 2];
        }
    }
    /// Table 5-5: elapsed 1, 61, 121, 181, 241, 301, 361 seconds.
    /// Disabled economy skips grants, without back-paying when re-enabled.
    pub fn advance_to(&mut self, match_time_ns: u64) {
        self.match_time_ns = self.match_time_ns.max(match_time_ns);
        let schedule = IncomeSchedule {
            initial: self.settings.initial_gold,
            minute: self.settings.minute_gold,
            final_minute: self.settings.final_minute_gold,
        };
        while let Some((at_ticks, amount)) = schedule.grant(self.next_income)
            && at_ticks * crate::TICK_NS <= self.match_time_ns
        {
            if self.settings.economy_enabled {
                for gold in &mut self.gold {
                    *gold = gold.saturating_add(amount);
                }
            }
            self.next_income += 1;
        }
    }
    /// Apply one referee edit. An unknown robot id, a zero purchase amount, an
    /// overflowing sum or insufficient gold returns a message and leaves the
    /// resources unchanged.
    pub fn edit(&mut self, edit: Edit) -> Result<(), &'static str> {
        match edit {
            Edit::Settings(settings) => self.settings = settings,
            Edit::Gold { team, gold } => self.gold[team.index()] = gold,
            Edit::Robot {
                id,
                allowance,
                shots,
            } => {
                let r = self
                    .robots
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or("unknown robot id")?;
                r.allowance = allowance;
                r.shots = shots;
            }
            Edit::UnassignedShots { shots } => self.unassigned_shots = shots,
            Edit::BuyAmmo {
                id,
                caliber,
                amount,
            } => {
                let r = self
                    .robots
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or("unknown robot id")?;
                if amount == 0 {
                    return Err("purchase amount must be positive");
                }
                let index = caliber.index();
                let cost = amount
                    .checked_mul(self.settings.ammo_price[index])
                    .ok_or("purchase cost overflow")?;
                let allowance = r.allowance[index]
                    .checked_add(amount)
                    .ok_or("allowance overflow")?;
                let gold = self.gold[r.team.index()]
                    .checked_sub(cost)
                    .ok_or("insufficient team gold")?;
                self.gold[r.team.index()] = gold;
                r.allowance[index] = allowance;
            }
        }
        Ok(())
    }
    /// Check a launch against the chassis's allowance when enforcement is on.
    /// A free-camera shot has no chassis and always passes.
    pub fn check_launch(
        &self,
        robot: Option<u32>,
        caliber: crate::Caliber,
    ) -> Result<(), &'static str> {
        if let Some(id) = robot {
            let r = self
                .robots
                .iter()
                .find(|r| r.id == id)
                .ok_or("unknown robot id")?;
            if !allowance_permits_launch(
                r.allowance[caliber.index()],
                self.settings.enforce_allowance,
            ) {
                return Err("projectile allowance exhausted");
            }
        }
        Ok(())
    }
    /// Call only after the physical launch succeeds and check_launch passes.
    pub fn launch(&mut self, robot: Option<u32>, caliber: crate::Caliber) {
        let index = caliber.index();
        if let Some(r) = robot.and_then(|id| self.robots.iter_mut().find(|r| r.id == id)) {
            record_ammo_launch(&mut r.allowance[index], &mut r.shots[index]);
        } else if robot.is_none() {
            self.unassigned_shots[index] = self.unassigned_shots[index].saturating_add(1);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn income_boundaries_skips_and_policy_changes() {
        let mut r = Resources::default();
        r.advance_to(999_999_999);
        assert_eq!(r.gold, [0; 2]);
        r.advance_to(1_000_000_000);
        assert_eq!(r.gold, [400; 2]);
        r.settings.economy_enabled = false;
        r.advance_to(61_000_000_000);
        r.settings.economy_enabled = true;
        // The 61 s boundary was crossed while the economy was off, so it is never
        // paid. That is why the paid-boundary counter has to be state: deriving
        // income from the clock alone would re-grant it here.
        assert_eq!(r.gold, [400; 2]);
        r.advance_to(62_000_000_000);
        assert_eq!(r.gold, [400; 2]);
        r.settings.minute_gold = 7;
        let mut split = r.clone();
        r.advance_to(420_000_000_000);
        for second in 62..=420 {
            split.advance_to(second * 1_000_000_000);
        }
        assert_eq!(r, split);
        assert_eq!(r.gold, [578; 2]);
    }
    #[test]
    fn resources_follow_roster_and_reset_and_calibers() {
        let mut r = Resources::default();
        r.settings.initial_allowance = [2, 1];
        r.add_robot(7, crate::Team::Red);
        r.settings.enforce_allowance = true;
        r.launch(Some(7), crate::Caliber::Mm42);
        assert!(r.check_launch(Some(7), crate::Caliber::Mm42).is_err());
        assert!(r.check_launch(Some(7), crate::Caliber::Mm17).is_ok());
        r.reset();
        assert_eq!(r.robots[0].allowance, [2, 1]);
        assert_eq!(r.robots[0].shots, [0; 2]);
        r.remove_robot(7);
        assert!(r.check_launch(Some(7), crate::Caliber::Mm17).is_err());
    }
}
