// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Byte pacing before native transport queues, with independent replaceable motion.
use std::{collections::VecDeque, time::Duration};
const MAX_QUEUED: usize = 256 * 1024;
const MAX_CONTROL_AGE: Duration = Duration::from_secs(2);
const WORLD_AGE: Duration = Duration::from_millis(250);

/// One encoded packet offered to the transport, with the lane it must travel on.
pub struct Datagram {
    /// Encoded payload bytes, already framed by the codec that produced them.
    pub bytes: Vec<u8>,
    /// True on the ordered lane, which never drops a packet; false on the
    /// unreliable lane, where a newer packet may replace this one.
    pub reliable: bool,
}
/// Queue occupancy and service for control, owner and world classes, in that order.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct QueueStats {
    /// Optional host encoding counters; older senders omit them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<crate::network_stats::EncodingStats>,
    /// Bytes awaiting transport submission in each class, including pending world data.
    pub queued_bytes: [usize; 3],
    /// Age in milliseconds of each class's oldest queued packet; zero when empty.
    pub oldest_age_ms: [f64; 3],
    /// Cumulative bytes submitted in each class since this connection began.
    pub sent_bytes: [u64; 3],
}
struct Transfer {
    started: Duration,
    packets: VecDeque<Datagram>,
}
/// Token-bucket pacer in front of the native transport queues.
///
/// Three classes share one byte budget: reliable control (retries, confirmations,
/// feedback), the replaceable owner anchor and the replaceable world transfer.
/// All three are drained round-robin, so bulk recovery still makes progress
/// while control traffic is queued. The owner and world classes hold only the
/// newest payload, which keeps stale motion out of the transport instead of
/// growing a queue behind it.
pub struct Pacer {
    rate: f64,
    tokens: f64,
    last: Duration,
    cursor: usize,
    waiting: Option<(usize, bool)>,
    control: VecDeque<(Duration, Datagram)>,
    owner: Option<Datagram>,
    owner_started: Duration,
    class_sent_bytes: [u64; 3],
    world: Option<Transfer>,
    pending_world: Option<Transfer>,
    /// Owner anchors discarded because a newer one arrived first.
    pub replaced: u64,
    /// Unreliable control or world packets discarded because they went stale.
    pub expired: u64,
    /// Bytes handed to the transport by [`Pacer::next`].
    pub sent_bytes: u64,
}
impl Pacer {
    /// Pacer for `bytes_per_s`, floored at 1 KiB/s so a misconfigured rate still
    /// moves traffic. The burst allowance is 32 ms of bytes, never below 1 KiB.
    ///
    /// ```
    /// use rm_simulator_server::pacing::Pacer;
    /// use std::time::Duration;
    ///
    /// // 40 KiB/s, so the burst allowance is about 1310 bytes.
    /// let mut pacer = Pacer::new(40 * 1024);
    /// pacer.control(Duration::ZERO, vec![vec![1u8; 1000]]).unwrap();
    /// // The bucket starts empty, so the packet has to wait for tokens.
    /// assert!(pacer.next(Duration::ZERO).unwrap().is_none());
    /// let packet = pacer.next(Duration::from_millis(30)).unwrap().unwrap();
    /// assert!(packet.reliable);
    /// assert_eq!(packet.bytes.len(), 1000);
    /// ```
    pub fn new(bytes_per_s: u32) -> Self {
        Self {
            rate: bytes_per_s.max(1024) as f64,
            tokens: 0.,
            last: Duration::ZERO,
            cursor: 0,
            waiting: None,
            control: VecDeque::new(),
            owner: None,
            owner_started: Duration::ZERO,
            class_sent_bytes: [0; 3],
            world: None,
            pending_world: None,
            replaced: 0,
            expired: 0,
            sent_bytes: 0,
        }
    }
    /// Snapshot of queued bytes, oldest enqueue age and bytes sent by class.
    /// `now` uses the same monotonic clock as `next` and packet enqueue times.
    ///
    /// ```
    /// use rm_simulator_server::pacing::Pacer;
    /// use std::time::Duration;
    /// let mut pacer = Pacer::new(64 * 1024);
    /// pacer.owner(Duration::from_millis(10), vec![0; 100]);
    /// let stats = pacer.stats(Duration::from_millis(15));
    /// assert_eq!(stats.queued_bytes, [0, 100, 0]);
    /// assert_eq!(stats.oldest_age_ms, [0., 5., 0.]);
    /// ```
    pub fn stats(&self, now: Duration) -> QueueStats {
        let control_bytes = self.control.iter().map(|(_, p)| p.bytes.len()).sum();
        let owner_bytes = self.owner.as_ref().map_or(0, |p| p.bytes.len());
        let age =
            |at: Option<Duration>| at.map_or(0., |at| now.saturating_sub(at).as_secs_f64() * 1000.);
        QueueStats {
            encoding: None,
            queued_bytes: [
                control_bytes,
                owner_bytes,
                self.queue_bytes() - control_bytes - owner_bytes,
            ],
            oldest_age_ms: [
                age(self.control.front().map(|(at, _)| *at)),
                age(self.owner.as_ref().map(|_| self.owner_started)),
                age(self
                    .world
                    .as_ref()
                    .or(self.pending_world.as_ref())
                    .map(|t| t.started)),
            ],
            sent_bytes: self.class_sent_bytes,
        }
    }
    /// Bytes still held in the three queues, owner and world payloads included.
    pub fn queue_bytes(&self) -> usize {
        self.control
            .iter()
            .map(|(_, p)| p.bytes.len())
            .sum::<usize>()
            + self.owner.as_ref().map_or(0, |p| p.bytes.len())
            + [&self.world, &self.pending_world]
                .into_iter()
                .flatten()
                .flat_map(|t| &t.packets)
                .map(|p| p.bytes.len())
                .sum::<usize>()
    }
    /// Queues a retryable control packet on the unreliable lane. An identical
    /// packet already waiting is dropped instead of queued twice, and the
    /// duplicate is counted in [`Pacer::replaced`].
    pub fn unreliable_control(
        &mut self,
        now: Duration,
        bytes: Vec<u8>,
    ) -> Result<(), &'static str> {
        if self
            .control
            .iter()
            .any(|(_, p)| !p.reliable && p.bytes == bytes)
        {
            self.replaced += 1;
            return Ok(());
        }
        self.control(now, vec![bytes])?;
        if let Some((_, packet)) = self.control.back_mut() {
            packet.reliable = false;
        }
        Ok(())
    }
    /// Queues reliable control packets in order, preserving the application
    /// order the caller needs (a confirmation snapshot before its Pong). Errors
    /// when a packet exceeds the burst allowance or the queue would overflow;
    /// both are caller bugs, not congestion.
    pub fn control(&mut self, now: Duration, packets: Vec<Vec<u8>>) -> Result<(), &'static str> {
        if packets
            .iter()
            .any(|p| p.len() > (self.rate * 0.032).max(1024.) as usize)
        {
            return Err("paced control packet exceeds burst allowance");
        }
        if self.queue_bytes() + packets.iter().map(Vec::len).sum::<usize>() > MAX_QUEUED {
            return Err("paced control queue overflow");
        }
        self.control.extend(packets.into_iter().map(|bytes| {
            (
                now,
                Datagram {
                    bytes,
                    reliable: true,
                },
            )
        }));
        Ok(())
    }
    /// Holds one owner anchor, replacing any anchor not yet sent. The newest
    /// anchor is the only useful one, so an old unsent anchor is discarded
    /// rather than queued behind newer state.
    ///
    /// ```
    /// use rm_simulator_server::pacing::Pacer;
    ///
    /// let mut pacer = Pacer::new(40 * 1024);
    /// pacer.owner(std::time::Duration::ZERO, vec![0u8; 600]);
    /// pacer.owner(std::time::Duration::ZERO, vec![1u8; 600]);
    /// assert_eq!(pacer.queue_bytes(), 600);
    /// assert_eq!(pacer.replaced, 1);
    /// ```
    pub fn owner(&mut self, now: Duration, bytes: Vec<u8>) {
        self.owner_started = now;
        self.replaced += u64::from(self.owner.is_some());
        self.owner = Some(Datagram {
            bytes,
            reliable: false,
        });
    }
    /// Offers one fragmented world update, replacing any update not yet started
    /// and expiring it when the queue is too full to accept it. An update that
    /// cannot be started within 250 ms is dropped rather than delivered late.
    pub fn world(&mut self, now: Duration, packets: Vec<Vec<u8>>) {
        if packets.iter().map(Vec::len).sum::<usize>() > MAX_QUEUED / 4 {
            self.expired += 1;
            return;
        }
        if self.queue_bytes() + packets.iter().map(Vec::len).sum::<usize>() > MAX_QUEUED {
            self.expired += 1;
            return;
        }
        self.replaced += u64::from(self.pending_world.is_some());
        self.pending_world = Some(Transfer {
            started: now,
            packets: packets
                .into_iter()
                .map(|bytes| Datagram {
                    bytes,
                    reliable: false,
                })
                .collect(),
        });
    }
    fn packet_len(&self, class: usize) -> Option<usize> {
        match class {
            0 => self.control.front().map(|(_, p)| p.bytes.len()),
            1 => self.owner.as_ref().map(|p| p.bytes.len()),
            _ => self
                .world
                .as_ref()
                .and_then(|t| t.packets.front())
                .map(|p| p.bytes.len()),
        }
    }
    /// Next packet the bucket allows at `now`, or `None` until tokens accrue.
    /// Reliable control packets keep their order and expire only by the 2 s
    /// deadline, which is an error: a control barrier must not be silently
    /// dropped. `now` is monotonic; a caller that rewinds it accrues no tokens.
    pub fn next(&mut self, now: Duration) -> Result<Option<Datagram>, &'static str> {
        self.tokens = (self.tokens + now.saturating_sub(self.last).as_secs_f64() * self.rate)
            .min((self.rate * 0.032).max(1024.));
        self.last = now;
        let before = self.control.len();
        self.control
            .retain(|(at, p)| p.reliable || now.saturating_sub(*at) <= WORLD_AGE);
        self.expired += (before - self.control.len()) as u64;
        if self
            .control
            .front()
            .is_some_and(|(at, _)| now.saturating_sub(*at) > MAX_CONTROL_AGE)
        {
            return Err("paced control deadline exceeded");
        }
        if self
            .world
            .as_ref()
            .is_some_and(|t| t.packets.is_empty() || now.saturating_sub(t.started) > WORLD_AGE)
        {
            if self.world.as_ref().is_some_and(|t| !t.packets.is_empty()) {
                self.expired += 1;
            }
            self.world = None;
        }
        if self.world.is_none() {
            self.world = self.pending_world.take();
        }
        // Permit one smaller packet to bypass a blocked class, then reserve
        // enough tokens for that class. Unlimited bypass starves world fragments.
        if let Some((class, bypassed)) = self.waiting {
            match self.packet_len(class) {
                None => self.waiting = None,
                Some(len) if len as f64 <= self.tokens => {
                    self.cursor = class;
                    self.waiting = None;
                }
                Some(_) if bypassed => return Ok(None),
                Some(_) => {}
            }
        }
        // Round-robin gives bulk recovery service even during control/owner traffic.
        for _ in 0..3 {
            let class = self.cursor;
            self.cursor = (self.cursor + 1) % 3;
            let len = self.packet_len(class);
            if let Some(len) = len {
                if len as f64 > self.tokens {
                    self.waiting.get_or_insert((class, false));
                    continue;
                }
                let packet = match class {
                    0 => self.control.pop_front().map(|(_, p)| p),
                    1 => self.owner.take(),
                    _ => self.world.as_mut().and_then(|t| t.packets.pop_front()),
                };
                if let Some((waiting, bypassed)) = &mut self.waiting
                    && *waiting != class
                {
                    *bypassed = true;
                }
                self.tokens -= len as f64;
                self.sent_bytes += len as u64;
                self.class_sent_bytes[class] += len as u64;
                return Ok(packet);
            }
        }
        Ok(None)
    }
}

/// Downstream allowance in KiB/s. The default `lan` profile uses 512 KiB/s;
/// `RM_NET_PROFILE=limited` keeps the original 40 KiB/s development budget.
/// Native congestion control still limits delivery on constrained links.
pub fn downstream_default() -> u32 {
    if std::env::var("RM_NET_PROFILE").as_deref() == Ok("limited") {
        40
    } else {
        512
    }
}

/// Upstream allowance in KiB/s. LAN uses 64 KiB/s for sustained fire and input
/// redundancy; `RM_NET_PROFILE=limited` retains the original 10 KiB/s budget.
pub fn upstream_default() -> u32 {
    if std::env::var("RM_NET_PROFILE").as_deref() == Ok("limited") {
        10
    } else {
        64
    }
}

/// Byte rate from environment variable `name`, in bytes per second. A value
/// outside 4 to 2048 KiB/s, or one that does not parse, falls back to
/// `default_kib_s`.
pub fn configured_rate(name: &str, default_kib_s: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| (4..=2048).contains(n))
        .unwrap_or(default_kib_s)
        * 1024
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smaller_owner_packet_can_pass_a_waiting_world_fragment() {
        let mut p = Pacer::new(40 * 1024);
        p.world(Duration::ZERO, vec![vec![2; 1000]]);
        p.cursor = 2;
        p.owner(Duration::ZERO, vec![1; 100]);
        assert_eq!(
            p.next(Duration::from_millis(5)).unwrap().unwrap().bytes[0],
            1
        );
        assert!(p.next(Duration::from_millis(5)).unwrap().is_none());
        assert_eq!(
            p.next(Duration::from_millis(30)).unwrap().unwrap().bytes[0],
            2
        );
    }
    #[test]
    fn caps_rate_and_replaces_owner_without_starving_world() {
        let mut p = Pacer::new(40 * 1024);
        let mut owner = 0;
        let mut world = 0;
        for ms in 0..1000 {
            let now = Duration::from_millis(ms);
            if ms % 16 == 0 {
                p.owner(now, vec![1; 600]);
                p.world(now, vec![vec![2; 1000]; 4]);
            }
            while let Some(packet) = p.next(now).unwrap() {
                if packet.bytes[0] == 1 {
                    owner += 1;
                } else {
                    world += 1;
                }
            }
        }
        assert!(p.sent_bytes <= 40 * 1024);
        assert!(owner > 10 && world > 10);
        assert!(p.replaced > 0);
        let before = p.sent_bytes;
        while p.next(Duration::from_secs(30)).unwrap().is_some() {}
        assert!(p.sent_bytes - before <= 1311);
    }
    #[test]
    fn retries_coalesce_and_expire_without_delaying_reliable_barriers() {
        let mut p = Pacer::new(4096);
        for ms in 0..100 {
            p.unreliable_control(Duration::from_millis(ms), vec![9; 500])
                .unwrap();
        }
        assert_eq!(p.queue_bytes(), 500);
        p.control(Duration::from_millis(200), vec![vec![3; 100]])
            .unwrap();
        let packet = p.next(Duration::from_millis(300)).unwrap().unwrap();
        assert!(packet.reliable);
        assert_eq!(packet.bytes[0], 3);
        assert_eq!(p.expired, 1);
    }
    #[test]
    fn controls_keep_order_and_expire_instead_of_growing() {
        let mut p = Pacer::new(4096);
        p.control(Duration::ZERO, vec![vec![3; 100], vec![4; 100]])
            .unwrap();
        assert_eq!(
            p.next(Duration::from_millis(100)).unwrap().unwrap().bytes[0],
            3
        );
        assert_eq!(
            p.next(Duration::from_millis(100)).unwrap().unwrap().bytes[0],
            4
        );
        assert!(
            p.control(Duration::ZERO, vec![vec![0; MAX_QUEUED + 1]])
                .is_err()
        );
        p.control(Duration::ZERO, vec![vec![5; 100]]).unwrap();
        assert!(p.next(Duration::from_secs(3)).is_err());
    }
}
