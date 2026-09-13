// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! A deterministic datagram link between two transport legs.
//!
//! This is a scripted delivery trace, not a network simulator. Each direction
//! carries two lanes, matching what [`crate::pacing::Datagram`] asks a carrier
//! for. The unreliable lane drops, reorders, duplicates and delays packets. The
//! reliable lane is ordered and head-of-line blocked, the way GNS (or TCP)
//! presents it: a "lost" packet does not disappear, it delays the lane until a
//! scripted retransmission completes. Nothing here sleeps, reads the clock or
//! opens a socket; the caller hands in `now` and drains what has arrived.
use crate::pacing::Datagram;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// SplitMix64. A seeded, portable stream so a scenario replays identically.
struct Rng(u64);
impl Rng {
    /// Next word of the seeded stream. The same seed and call count give the
    /// same word on every platform, which is what makes a trace reproducible.
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// What one direction of the link does to the packets crossing it.
/// The default is a perfect link: no loss, no delay, no reordering.
///
/// Every scripted draw comes from one seeded stream, so a profile never needs
/// a random seed of its own and a scenario replays byte for byte.
///
/// The unreliable counters bite on the lane that may lose and reorder. The
/// reliable lane ignores all of them except `recovery` and `blackout`: a
/// packet "lost" there is retransmitted rather than discarded, so the ordered
/// lane only stalls.
///
/// ```
/// use rm_simulator_server::pacing::Datagram;
/// use rm_simulator_server::scripted_link::{Impairment, Link};
/// use std::time::{Duration, Instant};
///
/// let epoch = Instant::now();
/// let rules = Impairment {
///     drop_packets: vec![3],
///     duplicate: 1.0,
///     reorder_every: 4,
///     reorder_hold: 2,
///     delay: Duration::from_millis(10),
///     ..Default::default()
/// };
/// let mut link = Link::new(epoch, 7, Impairment::default(), rules);
/// for step in 1..=8u64 {
///     let now = epoch + Duration::from_millis(step * 5);
///     link.send_to_client(
///         now,
///         Datagram {
///             bytes: vec![step as u8],
///             reliable: false,
///         },
///     );
/// }
/// let arrived = link.take_to_client(epoch + Duration::from_secs(1));
/// // Packet 3 was scripted away, and holding cannot save it.
/// assert!(!arrived.contains(&vec![3u8]));
/// // Every packet that arrives is duplicated exactly once, except packet 4:
/// // it was held back and released later, and a held packet is not duplicated.
/// let mut seen: Vec<u8> = arrived.iter().map(|bytes| bytes[0]).collect();
/// seen.sort_unstable();
/// assert_eq!(seen, vec![1, 1, 2, 2, 4, 5, 5, 6, 6, 7, 7]);
/// // Holding is visible in the delivery order: 5 is delivered before 4.
/// let order: Vec<u8> = arrived.iter().map(|bytes| bytes[0]).collect();
/// let first_four = order.iter().position(|byte| *byte == 4).unwrap();
/// let first_five = order.iter().position(|byte| *byte == 5).unwrap();
/// assert!(first_five < first_four);
/// let stats = link.downstream();
/// assert_eq!(stats.sent, 8);
/// assert_eq!(stats.dropped, 1);
/// // Packets 4 and 6 were held; every other delivered packet duplicated.
/// assert_eq!(stats.duplicated, 5);
/// assert_eq!(stats.reordered, 2);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Impairment {
    /// Fraction of unreliable packets dropped, drawn from the seeded stream.
    pub loss: f64,
    /// Ordinals (1-based, counting every packet this direction carries) that are
    /// dropped whatever `loss` says. An explicit "lose packet 7".
    pub drop_packets: Vec<u64>,
    /// Fraction of unreliable packets delivered twice.
    pub duplicate: f64,
    /// Hold every Nth unreliable packet back. Zero never reorders.
    pub reorder_every: u64,
    /// Release a held packet only after this many later packets were sent.
    pub reorder_hold: u64,
    /// One-way delay, plus a jittered extra up to `jitter`.
    pub delay: Duration,
    /// Largest extra one-way delay drawn from the seeded stream, in the range
    /// 0 to this value. Zero makes delivery exactly `delay` after sending.
    pub jitter: Duration,
    /// A lost reliable packet delays its ordered lane by this much.
    pub recovery: Duration,
    /// Everything sent while the link age is inside this window is lost: the
    /// unreliable lane drops it, the reliable lane retransmits after the end.
    pub blackout: Option<(Duration, Duration)>,
}

/// What actually happened to one direction's traffic.
///
/// The counters cover the whole lifetime of the direction, not a window, and
/// they count packets rather than bytes or lanes. `delivered` counts
/// transmissions, so a duplicate both raises `sent` and `delivered` and
/// `duplicated` records that the second copy was scripted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaneStats {
    /// Packets handed to this direction, reliable lane included.
    pub sent: u64,
    /// Transmissions released to the receiver, duplicates included.
    pub delivered: u64,
    /// Unreliable packets discarded here. A held packet that is dropped
    /// counts once, on its original send.
    pub dropped: u64,
    /// Unreliable packets whose second copy was scripted.
    pub duplicated: u64,
    /// Unreliable packets held back for a later release. Held packets are
    /// checked for loss first, so a packet counted here always arrives.
    pub reordered: u64,
    /// Reliable packets whose first arrival was pushed back by loss, a
    /// blackout or the tail of a packet still ahead of them.
    pub retransmitted: u64,
}

/// One direction's rules, seeded stream and queued packets.
struct Lane {
    rules: Impairment,
    rng: Rng,
    epoch: Instant,
    stats: LaneStats,
    /// Unreliable packets in flight, released when `at` passes.
    flight: Vec<(Instant, u64, Vec<u8>)>,
    /// Held for reordering: released once `sent` reaches the recorded ordinal.
    held: Vec<(u64, Vec<u8>)>,
    /// The reliable lane, in order. Nothing overtakes a packet ahead of it.
    ordered: VecDeque<(Instant, Vec<u8>)>,
    /// Release time of the last reliable packet. The next one is pushed past
    /// it, which is what keeps the ordered lane in order and head-of-line
    /// blocked behind a retransmission.
    tail: Instant,
}
impl Lane {
    /// Empty lane in `rules`, with independent random draws from `seed`.
    fn new(epoch: Instant, seed: u64, rules: Impairment) -> Self {
        Self {
            rules,
            rng: Rng(seed),
            epoch,
            stats: LaneStats::default(),
            flight: Vec::new(),
            held: Vec::new(),
            ordered: VecDeque::new(),
            tail: epoch,
        }
    }
    /// Remaining blackout time at `now`, or `None` when the window is absent
    /// or already closed. The age is measured from the anchor `epoch`, so the
    /// window covers `[start, end)` on the link's own clock.
    fn blacked_out(&self, now: Instant) -> Option<Duration> {
        let (start, end) = self.rules.blackout?;
        let age = now.saturating_duration_since(self.epoch);
        (age >= start && age < end).then(|| end - age)
    }
    /// Draw the jittered part of one packet's delay. Draws only when
    /// `jitter` is non-zero, so a profile without jitter consumes no stream.
    fn jitter(&mut self) -> Duration {
        if self.rules.jitter.is_zero() {
            return Duration::ZERO;
        }
        self.rules.jitter.mul_f64(self.rng.unit())
    }
    /// Apply the rules to one offered packet and queue what survives.
    ///
    /// The ordinal counted here is 1-based over every packet this direction
    /// has carried, reliable lane included, and it is what `drop_packets` and
    /// `reorder_every` match. A reliable packet is never discarded: loss, a
    /// blackout or a scripted drop adds `recovery` and the remaining blackout
    /// to its arrival instead.
    fn send(&mut self, now: Instant, packet: Datagram) {
        self.stats.sent += 1;
        let ordinal = self.stats.sent;
        let outage = self.blacked_out(now);
        let scripted = self.rules.drop_packets.contains(&ordinal);
        let random = self.rules.loss > 0. && self.rng.unit() < self.rules.loss;
        let lost = outage.is_some() || scripted || random;
        if packet.reliable {
            // Ordered and reliable: loss costs a retransmission, not the packet.
            let mut arrival = now + self.rules.delay + self.jitter();
            if lost {
                self.stats.retransmitted += 1;
                arrival += outage.unwrap_or_default() + self.rules.recovery;
            }
            self.tail = self.tail.max(arrival);
            self.ordered.push_back((self.tail, packet.bytes));
            return;
        }
        if lost {
            self.stats.dropped += 1;
            self.release_held(now);
            return;
        }
        if self.rules.reorder_every != 0 && ordinal.is_multiple_of(self.rules.reorder_every) {
            self.stats.reordered += 1;
            self.held
                .push((ordinal + self.rules.reorder_hold.max(1), packet.bytes));
            return;
        }
        let at = now + self.rules.delay + self.jitter();
        if self.rules.duplicate > 0. && self.rng.unit() < self.rules.duplicate {
            self.stats.duplicated += 1;
            let again = at + self.jitter();
            self.flight.push((again, ordinal, packet.bytes.clone()));
        }
        self.flight.push((at, ordinal, packet.bytes));
        self.release_held(now);
    }
    /// A held packet rejoins the flight once enough later packets have passed it.
    ///
    /// A held packet is released when the direction's send count reaches the
    /// ordinal recorded at hold time, which is `reorder_hold` sends later, so
    /// it lands behind the packets that overtook it. Its new entry carries the
    /// current send count, which places it in that arrival order.
    fn release_held(&mut self, now: Instant) {
        let sent = self.stats.sent;
        let delay = self.rules.delay;
        let mut released = Vec::new();
        self.held.retain(|(threshold, bytes)| {
            if *threshold <= sent {
                released.push(bytes.clone());
                false
            } else {
                true
            }
        });
        for bytes in released {
            self.flight.push((now + delay, sent, bytes));
        }
    }
    /// Everything that has arrived by `now`: the ordered lane first, then the
    /// unreliable packets in release order.
    ///
    /// Release order is arrival time first, then send ordinal, so two packets
    /// that land on the same instant come out in the order they were sent.
    /// Each released transmission counts toward `delivered`, duplicates too.
    fn take(&mut self, now: Instant) -> Vec<Vec<u8>> {
        let mut arrived = Vec::new();
        while self.ordered.front().is_some_and(|(at, _)| *at <= now) {
            arrived.push(self.ordered.pop_front().unwrap().1);
            self.stats.delivered += 1;
        }
        let mut ready: Vec<_> = Vec::new();
        self.flight.retain(|entry| {
            if entry.0 <= now {
                ready.push(entry.clone());
                false
            } else {
                true
            }
        });
        ready.sort_by_key(|(at, ordinal, _)| (*at, *ordinal));
        self.stats.delivered += ready.len() as u64;
        arrived.extend(ready.into_iter().map(|(.., bytes)| bytes));
        arrived
    }
}

/// A datagram link with independent per-direction rules. `epoch` anchors the
/// blackout window; both ends must be driven from the same clock as the link.
///
/// The two directions run on separate seeded streams derived from `seed`, so
/// one profile can be changed without disturbing the other's draws. Nothing
/// here advances on its own: a packet moves only when `now` is handed to a
/// `send` or `take` call, which is what keeps a test trace reproducible.
///
/// The reliable lane is ordered and head-of-line blocked, the way GNS or TCP
/// presents it. Each reliable packet leaves after the one before it, so a
/// packet held back delays the whole lane and nothing overtakes it. A packet
/// "lost" on that lane by `loss`, a scripted drop or a blackout is never
/// discarded: it is pushed back by `recovery` and the remaining blackout, and
/// the packets behind it wait on the new release time.
///
/// A blackout loses unreliable traffic outright and only stalls the reliable
/// lane, which delivers after the window closes. `loss`, a scripted drop and a
/// blackout therefore all leave `dropped` at zero on the reliable lane; they
/// show up as `retransmitted` instead, and nothing overtakes the stalled
/// packet:
///
/// ```
/// use rm_simulator_server::pacing::Datagram;
/// use rm_simulator_server::scripted_link::{Impairment, Link};
/// use std::time::{Duration, Instant};
///
/// let epoch = Instant::now();
/// // The window is [100 ms, 300 ms) on the link clock. The send ordinals are
/// // chosen so that no packet is held for reordering, since a held packet is
/// // released after the window and would bypass it.
/// let mut link = Link::new(
///     epoch,
///     1,
///     Impairment {
///         blackout: Some((Duration::from_millis(100), Duration::from_millis(300))),
///         ..Default::default()
///     },
///     Impairment::default(),
/// );
/// let send = |link: &mut Link, ms: u64, byte: u8, reliable: bool| {
///     link.send_to_host(
///         epoch + Duration::from_millis(ms),
///         Datagram {
///             bytes: vec![byte],
///             reliable,
///         },
///     );
/// };
/// // An unreliable send and a reliable one, both inside the window, then one
/// // unreliable send after it. None of the three is a reordering ordinal.
/// send(&mut link, 120, 1, false);
/// send(&mut link, 130, 2, true);
/// send(&mut link, 400, 3, false);
/// // The reliable send from inside the window is not lost: it is held back to
/// // the end of the window, so it is still waiting at 250 ms and 400 ms.
/// assert!(link.take_to_host(epoch + Duration::from_millis(250)).is_empty());
/// let arrived = link.take_to_host(epoch + Duration::from_secs(1));
/// assert_eq!(arrived, vec![vec![2u8], vec![3u8]]);
/// let stats = link.upstream();
/// assert_eq!(stats.sent, 3);
/// // Only the unreliable send inside the window is gone for good.
/// assert_eq!(stats.dropped, 1);
/// assert_eq!(stats.retransmitted, 1);
/// ```
///
/// A reliable send inside the window is not lost either. It is held until the
/// window closes and then delivered, which is what `recovery` adds to:
///
/// ```
/// use rm_simulator_server::pacing::Datagram;
/// use rm_simulator_server::scripted_link::{Impairment, Link};
/// use std::time::{Duration, Instant};
///
/// let epoch = Instant::now();
/// let rules = Impairment {
///     blackout: Some((Duration::from_millis(300), Duration::from_millis(800))),
///     recovery: Duration::from_millis(50),
///     ..Default::default()
/// };
/// let mut link = Link::new(epoch, 1, rules, Impairment::default());
/// // Sent inside the window, so its arrival is pushed to the window end plus
/// // the recovery time, 850 ms.
/// link.send_to_host(
///     epoch + Duration::from_millis(400),
///     Datagram {
///         bytes: vec![7u8],
///         reliable: true,
///     },
/// );
/// assert!(link.take_to_host(epoch + Duration::from_millis(800)).is_empty());
/// let arrived = link.take_to_host(epoch + Duration::from_secs(1));
/// assert_eq!(arrived, vec![vec![7u8]]);
/// let stats = link.upstream();
/// assert_eq!(stats.dropped, 0);
/// assert_eq!(stats.retransmitted, 1);
/// ```
pub struct Link {
    to_host: Lane,
    to_client: Lane,
}
impl Link {
    /// Link between a client and a host: `to_host` impairs what the client
    /// sends, `to_client` what the host sends. Each direction draws from its
    /// own stream derived from `seed`. `epoch` is the same instant both
    /// directions measure their blackout windows from.
    pub fn new(epoch: Instant, seed: u64, to_host: Impairment, to_client: Impairment) -> Self {
        Self {
            to_host: Lane::new(epoch, seed ^ 0x746F_686F_7374, to_host),
            to_client: Lane::new(epoch, seed ^ 0x746F_636C_6E74, to_client),
        }
    }
    /// Offer a packet sent by the client. Applies the `to_host` rules.
    pub fn send_to_host(&mut self, now: Instant, packet: Datagram) {
        self.to_host.send(now, packet);
    }
    /// Offer a packet sent by the host. Applies the `to_client` rules.
    pub fn send_to_client(&mut self, now: Instant, packet: Datagram) {
        self.to_client.send(now, packet);
    }
    /// Payloads the host can read at `now`, in delivery order. The ordered
    /// lane comes first, then the unreliable packets. Taking removes them.
    pub fn take_to_host(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.to_host.take(now)
    }
    /// Payloads the client can read at `now`, in delivery order. The ordered
    /// lane comes first, then the unreliable packets. Taking removes them.
    pub fn take_to_client(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.to_client.take(now)
    }
    /// Counters for the client-to-host direction.
    pub fn upstream(&self) -> LaneStats {
        self.to_host.stats
    }
    /// Counters for the host-to-client direction.
    pub fn downstream(&self) -> LaneStats {
        self.to_client.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datagram(byte: u8, reliable: bool) -> Datagram {
        Datagram {
            bytes: vec![byte],
            reliable,
        }
    }

    #[test]
    fn unreliable_traffic_is_dropped_reordered_and_duplicated_on_a_fixed_stream() {
        let epoch = Instant::now();
        let rules = Impairment {
            loss: 0.25,
            drop_packets: vec![3],
            duplicate: 0.25,
            reorder_every: 5,
            reorder_hold: 2,
            delay: Duration::from_millis(20),
            ..Default::default()
        };
        let run = || {
            let mut link = Link::new(epoch, 7, Impairment::default(), rules.clone());
            let mut seen = Vec::new();
            for step in 0..60u64 {
                let now = epoch + Duration::from_millis(step * 5);
                link.send_to_client(now, datagram(step as u8, false));
                seen.extend(link.take_to_client(now));
            }
            (seen, link.downstream())
        };
        let (first, stats) = run();
        assert_eq!(run().0, first, "the same seed replays the same trace");
        assert!(stats.dropped > 0 && stats.duplicated > 0 && stats.reordered > 0);
        assert!(!first.contains(&vec![2u8]), "packet 3 is scripted away");
        // Reordering is observable: some delivery arrives after a later one.
        let order: Vec<u8> = first.iter().map(|bytes| bytes[0]).collect();
        assert!(order.windows(2).any(|pair| pair[0] > pair[1]));
    }

    #[test]
    fn the_reliable_lane_keeps_order_and_a_blackout_only_delays_it() {
        let epoch = Instant::now();
        let mut link = Link::new(
            epoch,
            1,
            Impairment::default(),
            Impairment {
                loss: 1.0,
                recovery: Duration::from_millis(50),
                delay: Duration::from_millis(10),
                blackout: Some((Duration::from_millis(100), Duration::from_millis(300))),
                ..Default::default()
            },
        );
        let mut seen = Vec::new();
        for step in 0..240u64 {
            let now = epoch + Duration::from_millis(step * 5);
            if step < 200 && step.is_multiple_of(10) {
                link.send_to_client(now, datagram(step as u8, true));
            }
            seen.extend(link.take_to_client(now).into_iter().map(|bytes| bytes[0]));
        }
        let sent: Vec<u8> = (0..200u64)
            .filter(|step| step.is_multiple_of(10))
            .map(|step| step as u8)
            .collect();
        assert_eq!(
            seen, sent,
            "nothing is lost or reordered on the reliable lane"
        );
        assert_eq!(link.downstream().retransmitted, sent.len() as u64);
        assert_eq!(link.downstream().dropped, 0);
    }
}
