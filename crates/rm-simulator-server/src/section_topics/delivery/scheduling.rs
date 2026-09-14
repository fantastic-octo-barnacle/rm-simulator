// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Bounded logical-group byte DRR for the offline T2 experiment.
use super::*;

const GROUPS: usize = 4;
const QUANTUM: usize = CHUNK + HEADER + 4;
const DEFICIT_CAP: usize = QUANTUM * 32;

pub(super) struct Drr {
    weights: [u8; GROUPS],
    deficits: [usize; GROUPS],
    group: usize,
    checkpoint: usize,
    visiting: bool,
}
impl Drr {
    fn new(weights: [u8; GROUPS]) -> io::Result<Self> {
        if weights.iter().any(|w| !(1..=16).contains(w)) {
            return Err(invalid("DRR weights must be in 1..=16"));
        }
        Ok(Self {
            weights,
            deficits: [0; GROUPS],
            group: 0,
            checkpoint: 0,
            visiting: false,
        })
    }
    fn advance(&mut self) {
        self.group = (self.group + 1) % GROUPS;
        self.visiting = false;
    }
    fn select(&mut self, fronts: [Option<(usize, usize)>; GROUPS]) -> Option<usize> {
        // Each fragment costs at most one quantum, so two visits suffice.
        for _ in 0..GROUPS * 2 {
            let Some((class, cost)) = fronts[self.group] else {
                self.deficits[self.group] = 0;
                self.advance();
                continue;
            };
            if !self.visiting {
                self.deficits[self.group] = (self.deficits[self.group]
                    + QUANTUM * self.weights[self.group] as usize)
                    .min(DEFICIT_CAP);
                self.visiting = true;
            }
            if cost <= self.deficits[self.group] {
                return Some(class);
            }
            self.advance();
        }
        None
    }
}
impl Sender {
    /// Creates an offline sender with byte-weighted logical-group DRR.
    /// Weights name checkpoint (normal, manifest and repair), chassis poses,
    /// projectile poses and ordered reliable controls/baselines, in that order.
    /// Each weight must be in `1..=16`. Idle groups lend unused capacity; stored
    /// credit is capped at 32 maximum fragments. Controls remain FIFO and cannot
    /// be replaced. Positive weights do not promise latency during overload.
    /// The budget is application bytes per second, including framing.
    ///
    /// ```
    /// use rm_simulator_server::section_topics::delivery::Sender;
    /// let sender = Sender::with_weights(40 * 1024, [2, 1, 1, 1]).unwrap();
    /// assert_eq!(sender.queued_bytes(), 0);
    /// assert!(Sender::with_weights(40 * 1024, [0, 1, 1, 1]).is_err());
    /// ```
    pub fn with_weights(bytes_per_s: u32, weights: [u8; GROUPS]) -> io::Result<Self> {
        let mut sender = Self::new(bytes_per_s);
        sender.queue.drr = Some(Drr::new(weights)?);
        Ok(sender)
    }
}
impl Queue {
    fn weighted_fronts(&mut self, header: usize) -> [Option<(usize, usize)>; GROUPS] {
        let start = self.drr.as_ref().unwrap().checkpoint;
        let checkpoint = (0..=REPAIR_MANIFEST)
            .map(|offset| (start + offset) % (REPAIR_MANIFEST + 1))
            .find(|&class| self.front(class).is_some());
        let checkpoint = self.completion_front(checkpoint);
        [
            checkpoint,
            Some(presentation::CHASSIS),
            Some(presentation::PROJECTILES),
            Some(CONTROL_CLASS),
        ]
        .map(|class| {
            class.and_then(|class| {
                self.front(class).map(|t| {
                    (
                        class,
                        header + HEADER + (t.bytes.len() - t.offset).min(CHUNK),
                    )
                })
            })
        })
    }
    pub(super) fn next_weighted(&mut self) -> io::Result<Option<Datagram>> {
        let mut bytes = Vec::with_capacity(MTU);
        let mut reliable = false;
        loop {
            let shared_header = if bytes.is_empty() { 4 } else { 0 };
            let fronts = self.weighted_fronts(shared_header);
            let Some(class) = self.drr.as_mut().unwrap().select(fronts) else {
                break;
            };
            let transfer = self.front(class).unwrap();
            let cost = HEADER + (transfer.bytes.len() - transfer.offset).min(CHUNK);
            if bytes.is_empty() {
                if (cost + 4) as f64 > self.budget.tokens {
                    break;
                }
                reliable = class == CONTROL_CLASS;
                bytes.extend(if reliable { RELIABLE } else { DATA });
            } else if reliable != (class == CONTROL_CLASS) {
                break;
            }
            if bytes.len() + cost > MTU || (bytes.len() + cost) as f64 > self.budget.tokens {
                break;
            }
            let served = self.consume(class, &mut bytes);
            let drr = self.drr.as_mut().unwrap();
            drr.deficits[drr.group] -= served + shared_header;
            if class <= REPAIR_MANIFEST {
                drr.checkpoint = (class + 1) % (REPAIR_MANIFEST + 1);
                self.completion_served(served + shared_header);
            }
            if (1..REPAIR_MANIFEST).contains(&class) {
                self.stats.topic_service_bytes[(class - 1) % TOPICS] += served as u64;
            }
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        self.budget.tokens -= bytes.len() as f64;
        self.stats.sent_bytes += bytes.len() as u64;
        self.stats.sent_packets += 1;
        #[cfg(test)]
        if let Some(trace) = &self.trace {
            trace.borrow_mut().packet(bytes.len());
        }
        Ok(Some(Datagram { bytes, reliable }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_shares_ignore_topic_count_and_idle_credit_is_bounded() {
        let mut drr = Drr::new([2, 1, 1, 1]).unwrap();
        let fronts = [
            Some((0, 1023)),
            Some((20, 83)),
            Some((21, 523)),
            Some((22, 1023)),
        ];
        let mut served = [0; 4];
        for _ in 0..100_000 {
            drr.select(fronts).unwrap();
            let group = drr.group;
            let cost = fronts[group].unwrap().1;
            drr.deficits[group] -= cost;
            served[group] += cost;
        }
        for total in &served[1..] {
            assert!((served[0] as f64 / *total as f64 - 2.).abs() < 0.01);
        }
        for _ in 0..100 {
            assert!(drr.select([None; 4]).is_none());
        }
        assert_eq!(drr.deficits, [0; 4]);
        assert!(drr.deficits.iter().all(|&d| d <= DEFICIT_CAP));
    }
    #[test]
    fn backlog_borrows_idle_capacity_and_controls_stay_fifo() {
        let mut sender = Sender::with_weights(40 * 1024, [1, 1, 1, 1]).unwrap();
        for class in [
            0,
            presentation::CHASSIS,
            presentation::PROJECTILES,
            CONTROL_CLASS,
        ] {
            sender
                .queue
                .offer(class, vec![class as u8; 10_000])
                .unwrap();
        }
        sender.queue.offer(CONTROL_CLASS, vec![99; 2000]).unwrap();
        let mut received = Reassembly::default();
        let mut classes = Vec::new();
        let mut controls = Vec::new();
        for ms in (2..=2000).step_by(2) {
            while let Some(packet) = sender.next(Duration::from_millis(ms)).unwrap() {
                assert!(packet.bytes.len() <= MTU);
                for (class, frame) in received
                    .receive(&packet.bytes, Duration::from_millis(ms))
                    .unwrap()
                {
                    classes.push(class);
                    if class == CONTROL_CLASS {
                        controls.push(frame.bytes);
                    }
                }
            }
        }
        assert_eq!(classes.iter().filter(|&&c| c == CONTROL_CLASS).count(), 2);
        assert_eq!(
            controls,
            vec![vec![CONTROL_CLASS as u8; 10_000], vec![99; 2000]]
        );
        assert_eq!(sender.queued_bytes(), 0);
        assert!(sender.stats().sent_bytes <= 40 * 1024 * 2);
    }
}
