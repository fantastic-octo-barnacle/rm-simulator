// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Bounded client estimate of simulation time. Never advances host physics.
/// A client's map from its own monotonic reading to simulation time, built from
/// owner anchors and snapshot checkpoints on one epoch. It never advances host
/// physics and never sends anything.
#[derive(Default)]
pub(super) struct ClockEstimate {
    epoch: Option<u64>,
    anchor_local_ns: u64,
    anchor_simulation_ns: u64,
    latest_ns: u64,
    paused: bool,
    initialized: bool,
    synchronized: bool,
}
impl ClockEstimate {
    /// Take one observation: `epoch` is the input epoch it belongs to,
    /// `local_ns` the client's monotonic reading, `simulation_ns` the world time
    /// the host reported and `paused` whether the world was paused.
    ///
    /// An observation from an older epoch is ignored, because the owner anchor
    /// and the snapshot arrive independently and a lower epoch is a stale
    /// context, not a rewind. A lower simulation time within the current epoch
    /// after the first observation is ignored for the same reason. A new epoch
    /// resets the estimate. The anchor moves when the report is the first one,
    /// when the pause state flips or when the report disagrees with the
    /// projected time by more than 500 ms; a moved anchor clears the
    /// synchronized flag until a probe confirms the mapping.
    pub(super) fn observe(&mut self, epoch: u64, local_ns: u64, simulation_ns: u64, paused: bool) {
        // Owner anchors and world snapshots arrive independently. An older
        // context is not a simulation rewind and must not reset the clock.
        if self.epoch.is_some_and(|current| epoch < current)
            || (self.epoch == Some(epoch) && self.initialized && simulation_ns < self.latest_ns)
        {
            return;
        }
        if self.epoch != Some(epoch) {
            *self = Self::default();
            self.epoch = Some(epoch);
        }
        let expected = self
            .anchor_simulation_ns
            .saturating_add(local_ns.saturating_sub(self.anchor_local_ns));
        if !self.initialized
            || paused != self.paused
            || simulation_ns < self.latest_ns
            || simulation_ns.abs_diff(expected) > 500_000_000
        {
            self.synchronized = false;
            self.anchor_local_ns = local_ns;
            self.anchor_simulation_ns = simulation_ns;
        }
        self.latest_ns = simulation_ns;
        self.paused = paused;
        self.initialized = true;
    }
    /// Adopt a clock probe's measurement of simulation time. A paused probe, or
    /// one that disagrees with the current pause state, changes nothing. A
    /// trusted estimate limits the correction to 5 ms either way, so one
    /// retransmitted probe cannot jump the presentation; a probe that differs
    /// by more than 100 ms is treated as a fresh start and adopted whole,
    /// because a later lower-round-trip sample must undo a bad first offset
    /// rather than slew at 5 ms per second for a whole session. The anchor
    /// never moves the estimate backwards past the latest observed time.
    pub(super) fn synchronize(&mut self, local_ns: u64, simulation_ns: u64, paused: bool) {
        if paused != self.paused || paused {
            return;
        }
        let current = self.at(local_ns);
        // An early reliable probe can include a retransmission. A later
        // lower-RTT sample must undo that bad initial offset promptly; slewing
        // 5 ms/s could otherwise reject controls for an entire play session.
        let corrected = if self.synchronized && simulation_ns.abs_diff(current) <= 100_000_000 {
            simulation_ns.clamp(
                current.saturating_sub(5_000_000),
                current.saturating_add(5_000_000),
            )
        } else {
            simulation_ns
        };
        self.anchor_local_ns = local_ns;
        self.anchor_simulation_ns = corrected.max(self.latest_ns);
        self.synchronized = true;
    }
    /// The estimated simulation time at the client's `local_ns`, saturated to
    /// at least the latest observation. While paused, or before any
    /// observation, the latest observed time is returned unchanged. The result
    /// never leads the latest observation by more than
    /// [`crate::prediction::MAX_CONTINUOUS_REPLAY_NS`], so a long stall cannot
    /// make the presentation extrapolate without bound.
    pub(super) fn at(&self, local_ns: u64) -> u64 {
        if self.paused || !self.initialized {
            return self.latest_ns;
        }
        self.anchor_simulation_ns
            .saturating_add(local_ns.saturating_sub(self.anchor_local_ns))
            .max(self.latest_ns)
            .min(
                self.latest_ns
                    .saturating_add(crate::prediction::MAX_CONTINUOUS_REPLAY_NS),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn older_world_context_does_not_reset_owner_clock_but_new_epoch_does() {
        let mut clock = ClockEstimate::default();
        clock.observe(1, 0, 100_000_000, false);
        clock.synchronize(0, 120_000_000, false);
        clock.observe(1, 10_000_000, 110_000_000, false);
        clock.observe(1, 20_000_000, 105_000_000, false);
        assert_eq!(clock.at(30_000_000), 150_000_000);
        clock.observe(2, 40_000_000, 0, false);
        assert_eq!(clock.at(50_000_000), 10_000_000);
        clock.observe(1, 50_000_000, 200_000_000, false);
        assert_eq!(clock.at(60_000_000), 20_000_000);
    }
    #[test]
    fn jitter_does_not_restart_motion_and_stalls_are_bounded() {
        let mut clock = ClockEstimate::default();
        clock.observe(0, 100_000_000, 0, false);
        clock.synchronize(100_000_000, 100_000_000, false);
        clock.observe(0, 156_000_000, 32_000_000, false);
        // Jittered checkpoints preserve the established clock mapping.
        clock.synchronize(156_000_000, 156_000_000, false);
        clock.observe(0, 164_000_000, 80_000_000, false);
        assert_eq!(clock.at(180_000_000), 180_000_000);
        assert_eq!(clock.at(9_000_000_000), 1_580_000_000);
    }
    #[test]
    fn a_clean_probe_recovers_from_a_retransmitted_initial_probe() {
        let mut clock = ClockEstimate::default();
        clock.observe(0, 0, 0, false);
        clock.synchronize(1_000_000_000, 1_400_000_000, false);
        clock.observe(0, 2_000_000_000, 1_900_000_000, false);
        clock.synchronize(2_000_000_000, 2_000_000_000, false);
        assert_eq!(clock.at(2_100_000_000), 2_100_000_000);
    }

    #[test]
    fn pause_step_and_resume_reset_the_time_mapping() {
        let mut clock = ClockEstimate::default();
        clock.observe(0, 0, 100, false);
        clock.observe(0, 20, 120, true);
        clock.synchronize(30, 200, false);
        assert_eq!(clock.at(999), 120);
        clock.observe(0, 40, 136, true);
        assert_eq!(clock.at(999), 136);
        clock.observe(0, 50, 136, false);
        assert_eq!(clock.at(60), 146);
    }
}

/// Bounded lead controller driven by new host arrival observations. Values are
/// experimental application settings, not RoboMaster rules.
/// The lead grows while input arrives late and shrinks slowly once it is early
/// enough, always between 32 ms and 150 ms.
#[derive(Default)]
pub(super) struct InputLead {
    identity: Option<(u64, u64)>,
    sequence: u64,
    lead_ns: Option<u64>,
    stable: u8,
}
impl InputLead {
    /// Start a new adjustment when `epoch` or `revision` changes; readings from
    /// the same pair keep the current lead.
    pub(super) fn reset(&mut self, epoch: u64, revision: u64) {
        if self.identity != Some((epoch, revision)) {
            *self = Self {
                identity: Some((epoch, revision)),
                ..Self::default()
            };
        }
    }
    /// Feed one host telemetry sample into the controller. `feedback` is
    /// `(epoch, simulation_ns, chassis, input telemetry)` and `initial_ns` is
    /// the lead to use before the first decision, clamped to 32 ms to 150 ms.
    ///
    /// A sample from another chassis placement revision, or one whose received
    /// sequence is not newer, is ignored. Fewer than 8 arrival samples never
    /// decides anything. When the 5th percentile arrival margin is negative the
    /// lead grows by that shortfall plus 8 ms, at least 16 ms and at most 32 ms
    /// per step, and the stable counter restarts. When the margin is above
    /// 16 ms the lead loses 2 ms after 5 consecutive such samples, never below
    /// 32 ms. Any other margin leaves the lead alone and restarts the counter.
    pub(super) fn observe(
        &mut self,
        feedback: crate::network_stats::HostTelemetry,
        initial_ns: u64,
    ) {
        let stats = feedback.3;
        if self.identity != Some((feedback.0, stats.placement_revision))
            || stats.received_sequence <= self.sequence
        {
            return;
        }
        self.sequence = stats.received_sequence;
        if stats.arrival_samples < 8 {
            return;
        }
        let lead = self
            .lead_ns
            .get_or_insert(initial_ns.clamp(32_000_000, 150_000_000));
        if stats.arrival_p05_ns < 0 {
            let increase = stats
                .arrival_p05_ns
                .unsigned_abs()
                .saturating_add(8_000_000)
                .clamp(16_000_000, 32_000_000);
            *lead = lead.saturating_add(increase).min(150_000_000);
            self.stable = 0;
        } else if stats.arrival_p05_ns > 16_000_000 {
            self.stable += 1;
            if self.stable >= 5 {
                *lead = lead.saturating_sub(2_000_000).max(32_000_000);
                self.stable = 0;
            }
        } else {
            self.stable = 0;
        }
    }
    /// The current lead in nanoseconds, clamped to 32 ms to 150 ms. Before the
    /// first accepted sample this clamps `initial_ns` instead.
    pub(super) fn get(&self, initial_ns: u64) -> u64 {
        self.lead_ns
            .unwrap_or(initial_ns)
            .clamp(32_000_000, 150_000_000)
    }
}

#[cfg(test)]
mod lead_tests {
    use super::*;
    fn feedback(sequence: u64, margin: i64) -> crate::network_stats::HostTelemetry {
        crate::network_stats::HostTelemetry(
            1,
            sequence * 1_000_000_000,
            7,
            crate::network_stats::InputTelemetry {
                placement_revision: 2,
                received_sequence: sequence,
                arrival_samples: 64,
                arrival_p05_ns: margin,
                ..Default::default()
            },
        )
    }
    #[test]
    fn late_feedback_increases_lead_and_stable_feedback_decreases_slowly() {
        let mut lead = InputLead::default();
        lead.reset(1, 2);
        lead.observe(feedback(1, -20_000_000), 32_000_000);
        assert_eq!(lead.get(0), 60_000_000);
        lead.observe(feedback(1, -20_000_000), 32_000_000);
        assert_eq!(lead.get(0), 60_000_000);
        for sequence in 2..=5 {
            lead.observe(feedback(sequence, 30_000_000), 32_000_000);
        }
        assert_eq!(lead.get(0), 60_000_000);
        lead.observe(feedback(6, 30_000_000), 32_000_000);
        assert_eq!(lead.get(0), 58_000_000);
        for sequence in 7..=30 {
            lead.observe(feedback(sequence, i64::MIN), 32_000_000);
        }
        assert_eq!(lead.get(0), 150_000_000);
        lead.reset(2, 3);
        lead.observe(feedback(31, -99_000_000), 32_000_000);
        assert_eq!(lead.get(32_000_000), 32_000_000);
    }
}
