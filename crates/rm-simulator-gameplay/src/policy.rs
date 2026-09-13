// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Small rule policies shared by the standalone engine and live adapter.

use crate::SECOND_TICKS;

/// Number of Table 5-5 income grants in a round: the initial grant, five
/// minute grants and the final-minute grant.
pub(crate) const INCOME_GRANTS: usize = 7;

/// Table 5-5 income amounts: the initial grant, the recurring minute grant and
/// the final-minute grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IncomeSchedule {
    /// Gold granted at 1 s elapsed.
    pub initial: u32,
    /// Gold granted at each whole minute from 61 s through 301 s.
    pub minute: u32,
    /// Gold granted at 361 s, the final minute.
    pub final_minute: u32,
}

impl IncomeSchedule {
    /// RMUC 2026 V2.1.0, Table 5-5: initial, recurring and final income grants.
    pub(crate) const DEFAULT: Self = Self {
        initial: 400,
        minute: 50,
        final_minute: 150,
    };

    /// The grant at a zero-based schedule index, as its due round tick and
    /// amount, or `None` once the schedule is exhausted.
    pub(crate) fn grant(self, index: usize) -> Option<(u64, u32)> {
        (index < INCOME_GRANTS).then(|| {
            let amount = match index {
                0 => self.initial,
                6 => self.final_minute,
                _ => self.minute,
            };
            ((1 + index as u64 * 60) * SECOND_TICKS, amount)
        })
    }

    /// The amount due at an elapsed round tick, or `None` when no grant falls
    /// on that tick.
    pub(crate) fn amount_at(self, elapsed_ticks: u64) -> Option<u32> {
        if elapsed_ticks < SECOND_TICKS {
            return None;
        }
        let since_initial = elapsed_ticks - SECOND_TICKS;
        if !since_initial.is_multiple_of(60 * SECOND_TICKS) {
            return None;
        }
        let index = usize::try_from(since_initial / (60 * SECOND_TICKS)).ok()?;
        self.grant(index).map(|(_, amount)| amount)
    }
}

/// Record one physical launch. Returns whether it exceeded the allowance.
pub(crate) fn record_ammo_launch(allowance: &mut u32, shots: &mut u64) -> bool {
    *shots = shots.saturating_add(1);
    let over_allowance = *allowance == 0;
    *allowance = allowance.saturating_sub(1);
    over_allowance
}

/// Whether an allowance permits a launch. An unenforced policy always permits.
pub(crate) fn allowance_permits_launch(allowance: u32, enforce: bool) -> bool {
    !enforce || allowance > 0
}
