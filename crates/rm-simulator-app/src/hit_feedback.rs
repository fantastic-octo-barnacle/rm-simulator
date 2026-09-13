// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Receipt-timed authoritative contact effects, independent of world coalescing.
use rm_simulator_world::{ArmorHit, FieldSnapshot};
use std::{collections::VecDeque, time::Instant};

const CAPACITY: usize = 4096;
const RECOVERY_AGE_NS: u64 = 250_000_000;

#[derive(Default)]
pub(crate) struct HitFeedback {
    epoch: u64,
    last_event: u64,
    seen: VecDeque<ArmorHit>,
    visible: VecDeque<(Instant, ArmorHit, bool)>,
    received_events: u64,
    presented_events: u64,
    last_impact_ns: Option<u64>,
    last_receipt: Option<Instant>,
    last_receipt_to_scene_ms: Option<f64>,
}
impl HitFeedback {
    pub fn observe_epoch(&mut self, epoch: u64) {
        if epoch > self.epoch {
            self.epoch = epoch;
            self.visible.clear();
        }
    }
    pub fn receive(&mut self, epoch: u64, id: u64, hit: ArmorHit, now: Instant, time_ns: u64) {
        if epoch < self.epoch || id <= self.last_event {
            return;
        }
        self.observe_epoch(epoch);
        self.last_event = id;
        self.received_events += 1;
        self.last_impact_ns = Some(hit.time_ns);
        self.last_receipt = Some(now);
        self.insert(hit, now, time_ns);
    }
    fn insert(&mut self, hit: ArmorHit, now: Instant, time_ns: u64) {
        if self.seen.iter().any(|old| old == &hit) {
            return;
        }
        if hit.detected && time_ns.saturating_sub(hit.time_ns) <= RECOVERY_AGE_NS {
            if self.visible.len() == CAPACITY {
                self.visible.pop_front();
            }
            self.visible.push_back((now, hit.clone(), false));
        }
        if self.seen.len() == CAPACITY {
            self.seen.pop_front();
        }
        self.seen.push_back(hit);
    }
    pub fn recover(&mut self, epoch: u64, snapshot: &FieldSnapshot, now: Instant, time_ns: u64) {
        if epoch != self.epoch {
            return;
        }
        for hit in &snapshot.hits {
            self.insert(hit.clone(), now, time_ns.max(snapshot.time_ns));
        }
    }
    /// Record first scene submission. This measures app presentation, not GPU scanout.
    pub fn present(&mut self, now: Instant, duration_ns: u64) {
        for (received, _, shown) in &mut self.visible {
            let elapsed = now.saturating_duration_since(*received);
            if !*shown && elapsed.as_nanos() < u128::from(duration_ns) {
                *shown = true;
                self.presented_events += 1;
                self.last_receipt_to_scene_ms = Some(elapsed.as_secs_f64() * 1000.);
            }
        }
    }
    pub fn diagnostics(&self, now: Instant) -> serde_json::Value {
        serde_json::json!({
            "received_events": self.received_events,
            "presented_events": self.presented_events,
            "last_impact_time_ns": self.last_impact_ns,
            "last_receipt_age_ms": self.last_receipt.map(|at| now.saturating_duration_since(at).as_secs_f64() * 1000.),
            "last_receipt_to_scene_ms": self.last_receipt_to_scene_ms,
        })
    }
    pub fn visible(&self, now: Instant, duration_ns: u64) -> impl Iterator<Item = &ArmorHit> {
        self.visible.iter().filter_map(move |(received, hit, _)| {
            (now.saturating_duration_since(*received).as_nanos() < u128::from(duration_ns))
                .then_some(hit)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rm_simulator_world::{ArmorTarget, Caliber};
    use std::time::Duration;
    fn hit() -> ArmorHit {
        ArmorHit {
            time_ns: 10_000_000,
            projectile: 1,
            shooter: Some(1),
            caliber: Caliber::Mm17,
            target: ArmorTarget::Chassis {
                chassis: 2,
                plate: 0,
            },
            position_m: [0.; 3],
            local_offset_m: [0.; 2],
            normal_speed_m_s: 20.,
            detected: true,
            rejection: None,
            rune_outcome: None,
            damage: 10,
        }
    }
    #[test]
    fn delayed_hit_displays_once_and_expires_without_another_snapshot() {
        let now = Instant::now();
        let mut feedback = HitFeedback::default();
        feedback.receive(0, 1, hit(), now, 110_000_000);
        assert_eq!(feedback.visible(now, 50_000_000).count(), 1);
        feedback.present(now, 50_000_000);
        feedback.present(now, 50_000_000);
        assert_eq!(feedback.presented_events, 1);
        assert_eq!(feedback.last_receipt_to_scene_ms, Some(0.));
        let later = now + Duration::from_millis(60);
        feedback.receive(0, 1, hit(), later, 170_000_000);
        feedback.insert(hit(), later, 170_000_000);
        assert_eq!(feedback.visible(later, 50_000_000).count(), 0);
    }
    #[test]
    fn epoch_and_old_recovery_do_not_replay_effects() {
        let now = Instant::now();
        let mut feedback = HitFeedback::default();
        feedback.receive(0, 1, hit(), now, 110_000_000);
        feedback.observe_epoch(1);
        feedback.receive(0, 2, hit(), now, 110_000_000);
        assert_eq!(feedback.visible(now, 50_000_000).count(), 0);
        feedback.receive(1, 3, hit(), now, 1_000_000_000);
        assert_eq!(feedback.visible(now, 50_000_000).count(), 0);
    }
}
