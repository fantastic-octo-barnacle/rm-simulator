// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! T3: bounded completion preference within the existing checkpoint byte share.
use super::*;

const BURST_BYTES: usize = 4 * (CHUNK + HEADER + 4);
const FOCUS_LIFETIME: Duration = Duration::from_secs(1);

struct Focus {
    checkpoint: u64,
    transfer: Option<u64>,
    since: Duration,
}
#[derive(Default)]
pub(super) struct Completion {
    focus: Option<Focus>,
    requested: Option<u64>,
    completed: u64,
    burst_bytes: usize,
    selected_priority: bool,
    selections: u64,
    timeouts: u64,
    priority_bytes: u64,
    rotation_bytes: u64,
}
impl Completion {
    pub(super) fn feedback(&mut self, completed: u64, requested: Option<u64>) {
        self.completed = self.completed.max(completed);
        self.requested = requested.filter(|&id| id > self.completed);
    }
    #[cfg(test)]
    pub(super) fn json(&self) -> serde_json::Value {
        serde_json::json!({"focus_selections":self.selections,"focus_timeouts":self.timeouts,
            "priority_bytes":self.priority_bytes,"rotation_bytes":self.rotation_bytes,
            "burst_cap_bytes":BURST_BYTES,"focus_lifetime_ms":FOCUS_LIFETIME.as_millis()})
    }
}
impl Sender {
    /// Creates an offline DRR sender that favours finishing one checkpoint.
    /// Cadence and the four logical-group byte weights are caller-owned, exactly
    /// as in [`Self::with_weights`]. Only checkpoint child-queue order changes.
    /// Preference follows queued transfer identities and received repair feedback;
    /// it never reads receiver-private state. A focus expires after one injected
    /// second. After at most four maximum-fragment byte quanta of preference,
    /// one ordinary checkpoint round-robin fragment is served. Reliable controls,
    /// queue/repair limits and exact checkpoint assembly remain unchanged.
    ///
    /// ```
    /// use rm_simulator_server::section_topics::delivery::Sender;
    /// let sender = Sender::with_completion_priority(40 * 1024, [2, 1, 1, 1]).unwrap();
    /// assert_eq!(sender.queued_bytes(), 0);
    /// ```
    pub fn with_completion_priority(bytes_per_s: u32, weights: [u8; 4]) -> io::Result<Self> {
        let mut sender = Self::with_weights(bytes_per_s, weights)?;
        sender.queue.completion = Some(Completion::default());
        Ok(sender)
    }
}
impl Queue {
    pub(super) fn completion_front(&mut self, ordinary: Option<usize>) -> Option<usize> {
        let Some(policy) = &mut self.completion else {
            return ordinary;
        };
        policy.selected_priority = false;
        let now = self.budget.last;
        let fronts: Vec<(usize, u64, u64, usize, usize)> = self.slots[..=REPAIR_MANIFEST]
            .iter()
            .enumerate()
            .filter_map(|(class, slot)| {
                slot.active.as_ref().and_then(|t| {
                    t.checkpoint
                        .filter(|&id| id > policy.completed)
                        .map(|id| (class, id, t.id, t.offset, t.bytes.len() - t.offset))
                })
            })
            .collect();
        let expired = policy
            .focus
            .as_ref()
            .is_some_and(|f| now.saturating_sub(f.since) >= FOCUS_LIFETIME);
        let previous = policy.focus.as_ref().map(|f| f.checkpoint);
        if expired {
            policy.timeouts += 1;
        }
        if expired
            || policy
                .focus
                .as_ref()
                .is_some_and(|f| !fronts.iter().any(|t| t.1 == f.checkpoint))
        {
            policy.focus = None;
        }
        if policy.focus.is_none() {
            // A validated received repair request is preferred; otherwise finish
            // the oldest started transfer, then the oldest queued capture.
            // On timeout give another queued checkpoint an opportunity first.
            let alternatives = expired && fronts.iter().any(|t| Some(t.1) != previous);
            let chosen = fronts
                .iter()
                .filter(|t| !alternatives || Some(t.1) != previous)
                .min_by_key(|t| (Some(t.1) != policy.requested, t.3 == 0, t.2));
            if let Some(t) = chosen {
                policy.focus = Some(Focus {
                    checkpoint: t.1,
                    transfer: None,
                    since: now,
                });
                policy.selections += 1;
            }
        }
        if policy.burst_bytes >= BURST_BYTES {
            return ordinary;
        }
        let Some(focus) = &mut policy.focus else {
            return ordinary;
        };
        let chosen = fronts
            .iter()
            .find(|t| Some(t.2) == focus.transfer)
            .or_else(|| {
                fronts
                    .iter()
                    .filter(|t| t.1 == focus.checkpoint)
                    .min_by_key(|t| (t.3 == 0, t.4, t.2))
            });
        if let Some(t) = chosen {
            if policy.burst_bytes + HEADER + 4 + t.4.min(CHUNK) > BURST_BYTES {
                return ordinary;
            }
            focus.transfer = Some(t.2);
            policy.selected_priority = true;
            Some(t.0)
        } else {
            ordinary
        }
    }
    pub(super) fn completion_served(&mut self, bytes: usize) {
        if let Some(policy) = &mut self.completion {
            if policy.selected_priority {
                policy.priority_bytes += bytes as u64;
                policy.burst_bytes += bytes;
            } else {
                policy.rotation_bytes += bytes as u64;
                policy.burst_bytes = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn priority_finishes_a_started_transfer_and_rotation_remains_available() {
        let mut q = Sender::with_completion_priority(512 * 1024, [2, 1, 1, 1])
            .unwrap()
            .queue;
        q.offer_checkpoint(1, vec![1; 10_000], Some(1)).unwrap();
        q.offer_checkpoint(2, vec![2; 10_000], Some(2)).unwrap();
        q.budget.last = Duration::from_millis(2);
        assert_eq!(q.completion_front(Some(1)), Some(1));
        let mut bytes = Vec::new();
        q.consume(1, &mut bytes);
        q.completion_served(CHUNK + HEADER + 4);
        assert_eq!(q.completion_front(Some(2)), Some(1));
        for _ in 0..3 {
            q.completion_served(CHUNK + HEADER + 4);
        }
        assert_eq!(q.completion_front(Some(2)), Some(2));
        q.completion_served(CHUNK + HEADER + 4);
        assert_eq!(q.completion_front(Some(2)), Some(1));
        q.budget.last = Duration::from_millis(1002);
        assert_eq!(q.completion_front(Some(1)), Some(2));
        q.completion.as_mut().unwrap().feedback(2, None);
        assert_eq!(q.completion_front(Some(1)), Some(1));
        assert!(q.completion.as_ref().unwrap().focus.is_none());
    }
    #[test]
    fn received_request_can_select_repair_but_never_replaces_controls() {
        let mut sender = Sender::with_completion_priority(40 * 1024, [2, 1, 1, 1]).unwrap();
        sender
            .queue
            .offer_checkpoint(1, vec![1; 2000], Some(5))
            .unwrap();
        sender
            .queue
            .offer_checkpoint(10, vec![2; 2000], Some(3))
            .unwrap();
        sender
            .queue
            .completion
            .as_mut()
            .unwrap()
            .feedback(0, Some(3));
        assert_eq!(sender.queue.completion_front(Some(1)), Some(10));
        sender.queue.offer(CONTROL_CLASS, vec![3; 2000]).unwrap();
        sender.queue.offer(CONTROL_CLASS, vec![4; 1000]).unwrap();
        let mut parts = Reassembly::default();
        let mut controls = Vec::new();
        for ms in (2..=2000).step_by(2) {
            while let Some(packet) = sender.next(Duration::from_millis(ms)).unwrap() {
                for (class, frame) in parts
                    .receive(&packet.bytes, Duration::from_millis(ms))
                    .unwrap()
                {
                    if class == CONTROL_CLASS {
                        controls.push(frame.bytes);
                    }
                }
            }
        }
        assert_eq!(controls, vec![vec![3; 2000], vec![4; 1000]]);
        assert_eq!(sender.queued_bytes(), 0);
    }
}
