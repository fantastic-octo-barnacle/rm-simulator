// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Exogenous two-millisecond impairment bins, independent of packet count.
use super::*;

#[derive(Clone, Serialize)]
struct Bin {
    delay_ms: u64,
    drop: bool,
    recover_at_ms: u64,
}
#[derive(Clone)]
pub(super) struct Schedule {
    bins: [Vec<Bin>; 2],
    pub(super) blackout_end_ms: Option<u64>,
}
impl Schedule {
    pub(super) fn new(profile: &str, seed: u64, end_ms: u64, warmup_ms: u64) -> Self {
        let blackout = (warmup_ms + 20_000, warmup_ms + 20_500);
        let bins = std::array::from_fn(|direction| {
            (0..=end_ms / 2)
                .map(|slot| {
                    // SplitMix64 keyed by time bin and direction: no packet-driven RNG.
                    let mut x = seed.wrapping_add(slot.wrapping_mul(0x9e3779b97f4a7c15))
                        ^ (direction as u64).wrapping_mul(0xd1b54a32d192ed03);
                    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
                    x ^= x >> 31;
                    let ms = slot * 2;
                    let in_blackout =
                        profile == "blackout" && (blackout.0..blackout.1).contains(&ms);
                    let drop = in_blackout || (profile == "limited" && x % 100 < 1);
                    let delay_ms = match profile {
                        "clean" => 0,
                        "blackout" => 20,
                        "limited" => 40 + (x >> 32) % 21,
                        _ => panic!("unknown time-bin profile"),
                    };
                    Bin {
                        delay_ms,
                        drop,
                        recover_at_ms: if in_blackout {
                            blackout.1 + 100
                        } else {
                            ms + 100
                        },
                    }
                })
                .collect()
        });
        Self {
            bins,
            blackout_end_ms: (profile == "blackout").then_some(blackout.1),
        }
    }
    pub(super) fn hash(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&self.bins).unwrap())
        )
    }
}
pub(super) struct TimeLink {
    schedule: Schedule,
    pending: [BTreeMap<(u64, u64), Vec<u8>>; 2],
    reliable_due: [u64; 2],
    order: u64,
}
impl TimeLink {
    pub(super) fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            pending: Default::default(),
            reliable_due: [0; 2],
            order: 0,
        }
    }
    pub(super) fn send(&mut self, direction: usize, ms: u64, packet: Datagram) {
        let bin = &self.schedule.bins[direction][ms as usize / 2];
        if bin.drop && !packet.reliable {
            return;
        }
        let mut due = if bin.drop {
            bin.recover_at_ms + bin.delay_ms
        } else {
            ms + bin.delay_ms
        };
        if packet.reliable {
            due = due.max(self.reliable_due[direction]);
            self.reliable_due[direction] = due;
        }
        self.order += 1;
        self.pending[direction].insert((due, self.order), packet.bytes);
    }
    pub(super) fn take(&mut self, direction: usize, ms: u64) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        while self.pending[direction]
            .first_key_value()
            .is_some_and(|(&(due, _), _)| due <= ms)
        {
            packets.push(self.pending[direction].pop_first().unwrap().1);
        }
        packets
    }
}

#[test]
fn impairment_is_time_based_and_reliable_order_survives_blackout() {
    let schedule = Schedule::new("blackout", 701, 21_000, 0);
    let mut a = TimeLink::new(schedule.clone());
    let mut b = TimeLink::new(schedule);
    let packet = |byte, reliable| Datagram {
        bytes: vec![byte],
        reliable,
    };
    a.send(0, 20_002, packet(1, false));
    a.send(0, 20_002, packet(2, false));
    b.send(0, 20_002, packet(1, false));
    assert!(a.take(0, 21_000).is_empty() && b.take(0, 21_000).is_empty());
    a.send(0, 20_002, packet(3, true));
    a.send(0, 20_502, packet(4, true));
    assert!(a.take(0, 20_600).is_empty());
    assert_eq!(a.take(0, 20_620), vec![vec![3], vec![4]]);
}
