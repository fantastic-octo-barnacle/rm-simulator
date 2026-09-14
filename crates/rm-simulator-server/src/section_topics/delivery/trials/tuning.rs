// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! T1 test-only diagnostics. No instrument or trace state exists in non-test builds.
use super::*;
use std::{cell::RefCell, rc::Rc};

#[path = "tuning/link.rs"]
mod link;
#[path = "tuning/run.rs"]
mod run;
#[path = "tuning/t2.rs"]
mod t2;

pub(in super::super) type Trace = Rc<RefCell<Recorder>>;

#[derive(Default, Serialize)]
struct Class {
    offered_payload_bytes: u64,
    duplicate_suppressed_payload_bytes: u64,
    replaced_unsent_payload_bytes: u64,
    sent_payload_bytes: u64,
    fully_sent_payload_bytes: u64,
    queued_payload_bytes_at_end: usize,
    fragment_header_bytes: u64,
    fragments: u64,
    assembled_payload_bytes: u64,
    fresh_pose_payload_bytes: u64,
    innovative_section_frame_payload_bytes: u64,
    discarded_partial_received_payload_bytes: u64,
    discarded_partial_frames: u64,
    max_discarded_partial_lifetime_ms: u64,
    peak_queued_payload_bytes: usize,
    peak_partial_payload_bytes: usize,
    max_backlogged_service_gap_ms: u64,
    #[serde(skip)]
    gap_start: Option<u64>,
    #[serde(skip)]
    last_service: Option<u64>,
    #[serde(skip)]
    enqueue_to_first_send: Vec<f64>,
    #[serde(skip)]
    enqueue_to_last_send: Vec<f64>,
    #[serde(skip)]
    first_to_last_receipt: Vec<f64>,
    #[serde(skip)]
    enqueue_to_assembly: Vec<f64>,
}
#[derive(Serialize)]
struct TransferRecord {
    class: usize,
    len: usize,
    enqueue_ms: u64,
    first_send_ms: Option<u64>,
    last_send_ms: Option<u64>,
    assembled_ms: Option<u64>,
    first_receipt_ms: Option<u64>,
    replaced: bool,
}
#[derive(Default)]
pub(in super::super) struct Recorder {
    now_ms: u64,
    classes: [Class; CLASSES],
    transfers: BTreeMap<u64, TransferRecord>,
    captures: BTreeMap<u64, u64>,
    simulation_times: BTreeMap<u64, u64>,
    decoded_revisions: std::collections::BTreeSet<(u64, usize, u64)>,
    first_manifests: BTreeMap<u64, u64>,
    capture_delay_ms: Vec<f64>,
    checkpoint_delivery_ms: Vec<f64>,
    manifest_to_checkpoint_ms: Vec<f64>,
    completion_trigger: [u64; CLASSES],
    missing_dependency_ms: [u64; TOPICS],
    datagram_header_bytes: u64,
    datagram_bytes: u64,
    datagrams: u64,
    checkpoint_events: Vec<serde_json::Value>,
}
impl Recorder {
    pub(in super::super) fn offer(&mut self, class: usize, len: usize) {
        self.classes[class].offered_payload_bytes += len as u64;
    }
    pub(in super::super) fn suppressed(&mut self, class: usize, len: usize) {
        self.classes[class].duplicate_suppressed_payload_bytes += len as u64;
    }
    pub(in super::super) fn enqueue(
        &mut self,
        class: usize,
        id: u64,
        len: usize,
        replaced: Option<u64>,
    ) {
        if let Some(old) = replaced {
            let old = self.transfers.get_mut(&old).unwrap();
            assert!(old.first_send_ms.is_none());
            old.replaced = true;
            self.classes[class].replaced_unsent_payload_bytes += old.len as u64;
            self.classes[class].queued_payload_bytes_at_end -= old.len;
        }
        let c = &mut self.classes[class];
        c.queued_payload_bytes_at_end += len;
        c.peak_queued_payload_bytes = c
            .peak_queued_payload_bytes
            .max(c.queued_payload_bytes_at_end);
        c.gap_start.get_or_insert(self.now_ms);
        assert!(
            self.transfers
                .insert(
                    id,
                    TransferRecord {
                        class,
                        len,
                        enqueue_ms: self.now_ms,
                        first_send_ms: None,
                        last_send_ms: None,
                        assembled_ms: None,
                        first_receipt_ms: None,
                        replaced: false,
                    }
                )
                .is_none()
        );
    }
    pub(in super::super) fn sent(&mut self, class: usize, id: u64, len: usize, last: bool) {
        let t = self.transfers.get_mut(&id).unwrap();
        let c = &mut self.classes[class];
        c.sent_payload_bytes += len as u64;
        c.fragment_header_bytes += HEADER as u64;
        c.fragments += 1;
        if t.first_send_ms.is_none() {
            t.first_send_ms = Some(self.now_ms);
            c.enqueue_to_first_send
                .push((self.now_ms - t.enqueue_ms) as f64);
        }
        if last {
            c.fully_sent_payload_bytes += t.len as u64;
            c.queued_payload_bytes_at_end -= t.len;
            t.last_send_ms = Some(self.now_ms);
            c.enqueue_to_last_send
                .push((self.now_ms - t.enqueue_ms) as f64);
        }
        if let Some(start) = c.gap_start {
            c.max_backlogged_service_gap_ms =
                c.max_backlogged_service_gap_ms.max(self.now_ms - start);
            c.gap_start = Some(self.now_ms);
        }
        c.last_service = Some(self.now_ms);
    }
    pub(in super::super) fn packet(&mut self, bytes: usize) {
        self.datagram_bytes += bytes as u64;
        self.datagram_header_bytes += 4;
        self.datagrams += 1;
    }
    pub(in super::super) fn receipt(&mut self, id: u64) {
        self.transfers
            .get_mut(&id)
            .unwrap()
            .first_receipt_ms
            .get_or_insert(self.now_ms);
    }
    pub(in super::super) fn assembled(&mut self, class: usize, id: u64, started: Duration) {
        let t = self.transfers.get_mut(&id).unwrap();
        assert_eq!(t.class, class);
        assert!(t.assembled_ms.is_none());
        assert!(t.last_send_ms.is_some());
        t.assembled_ms = Some(self.now_ms);
        t.first_receipt_ms.get_or_insert(started.as_millis() as u64);
        let c = &mut self.classes[class];
        c.assembled_payload_bytes += t.len as u64;
        c.first_to_last_receipt
            .push((self.now_ms - started.as_millis() as u64) as f64);
        c.enqueue_to_assembly
            .push((self.now_ms - t.enqueue_ms) as f64);
    }
    pub(in super::super) fn discarded(&mut self, class: usize, partial: &Partial) {
        let c = &mut self.classes[class];
        c.discarded_partial_frames += 1;
        c.discarded_partial_received_payload_bytes += partial
            .received
            .iter()
            .enumerate()
            .filter(|(_, received)| **received)
            .map(|(i, _)| (partial.bytes.len() - i * CHUNK).min(CHUNK) as u64)
            .sum::<u64>();
        c.max_discarded_partial_lifetime_ms = c
            .max_discarded_partial_lifetime_ms
            .max(self.now_ms - partial.started.as_millis() as u64);
    }
    pub(in super::super) fn decoded(&mut self, class: usize, bytes: usize, decoder: &Decoder) {
        let mut innovative = false;
        if let Some(epoch) = decoder.epoch {
            for (i, cache) in decoder.caches.iter().enumerate() {
                for &revision in cache.values.keys() {
                    innovative |= self.decoded_revisions.insert((epoch, i, revision));
                }
            }
        }
        if innovative {
            self.classes[class].innovative_section_frame_payload_bytes += bytes as u64;
        }
    }
    pub(in super::super) fn useful_pose(&mut self, class: usize, bytes: usize) {
        self.classes[class].fresh_pose_payload_bytes += bytes as u64;
    }
    pub(in super::super) fn capture(&mut self, id: u64, time_ns: u64) {
        // T1 sources are captured at id * 16 ms, independently of paused world time.
        self.captures.insert(id, self.now_ms);
        self.simulation_times.insert(id, time_ns);
        self.capture_delay_ms.push((self.now_ms - id * 16) as f64);
    }
    pub(in super::super) fn manifest(&mut self, id: u64) {
        self.first_manifests.entry(id).or_insert(self.now_ms);
    }
    pub(in super::super) fn checkpoint(&mut self, id: u64, class: usize) {
        self.completion_trigger[class] += 1;
        let capture = self.captures[&id];
        let first = self.first_manifests[&id];
        self.checkpoint_delivery_ms
            .push((self.now_ms - capture) as f64);
        self.manifest_to_checkpoint_ms
            .push((self.now_ms - first) as f64);
        self.checkpoint_events.push(serde_json::json!({
            "checkpoint":id, "source_capture_ms":id*16, "simulation_time_ns":self.simulation_times[&id], "encoded_ms":capture,
            "first_manifest_ms":first, "usable_ms":self.now_ms, "trigger_class":class
        }));
    }
    fn sample(&mut self, sender: &Sender, receiver: &Receiver) {
        for (i, c) in self.classes.iter_mut().enumerate() {
            let bytes = if i == CONTROL_CLASS {
                sender.queue.control.iter().map(|t| t.bytes.len()).sum()
            } else {
                sender.queue.slots[i].bytes()
            };
            assert_eq!(c.queued_payload_bytes_at_end, bytes);
            c.peak_queued_payload_bytes = c.peak_queued_payload_bytes.max(bytes);
            if bytes == 0 {
                c.gap_start = None;
            } else {
                let start = *c.gap_start.get_or_insert(self.now_ms);
                c.max_backlogged_service_gap_ms =
                    c.max_backlogged_service_gap_ms.max(self.now_ms - start);
            }
            if let Some(p) = &receiver.parts.pending[i] {
                c.peak_partial_payload_bytes = c.peak_partial_payload_bytes.max(p.bytes.len());
            }
        }
        // Sum waiting across retained manifests, not an independent sample count.
        for manifest in receiver.decoder.pending.values() {
            for (i, rev) in manifest.revisions.iter().enumerate() {
                if !receiver.decoder.caches[i].values.contains_key(rev) {
                    self.missing_dependency_ms[i] += 2;
                }
            }
        }
    }
    fn json(&self) -> serde_json::Value {
        for c in &self.classes {
            assert_eq!(
                c.offered_payload_bytes
                    - c.duplicate_suppressed_payload_bytes
                    - c.replaced_unsent_payload_bytes,
                c.fully_sent_payload_bytes + c.queued_payload_bytes_at_end as u64
            );
        }
        let classes: Vec<_> = self
            .classes
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut value = serde_json::to_value(c).unwrap();
                let value = value.as_object_mut().unwrap();
                value.insert("class".into(), i.into());
                value.insert("name".into(), class_name(i).into());
                value.insert("group".into(), group(i).into());
                for (name, samples) in [
                    ("enqueue_to_first_send_ms", &c.enqueue_to_first_send),
                    ("enqueue_to_last_send_ms", &c.enqueue_to_last_send),
                    ("first_to_last_receipt_ms", &c.first_to_last_receipt),
                    ("enqueue_to_assembly_ms", &c.enqueue_to_assembly),
                ] {
                    value.insert(name.into(), distribution(samples));
                }
                let incomplete: Vec<_> = self
                    .transfers
                    .values()
                    .filter(|t| t.class == i && !t.replaced && t.assembled_ms.is_none())
                    .collect();
                value.insert(
                    "not_assembled_at_end_frames".into(),
                    incomplete.len().into(),
                );
                value.insert(
                    "not_assembled_at_end_payload_bytes".into(),
                    incomplete.iter().map(|t| t.len as u64).sum::<u64>().into(),
                );
                serde_json::Value::Object(value.clone())
            })
            .collect();
        let attributed: u64 = self
            .classes
            .iter()
            .map(|c| c.sent_payload_bytes + c.fragment_header_bytes)
            .sum();
        assert_eq!(attributed + self.datagram_header_bytes, self.datagram_bytes);
        let mut groups = BTreeMap::<&str, u64>::new();
        for (i, c) in self.classes.iter().enumerate() {
            *groups.entry(group(i)).or_default() += c.sent_payload_bytes + c.fragment_header_bytes;
        }
        serde_json::json!({
            "window":"connection lifetime including warm-up; no end drain",
            "classes":classes, "group_fragment_bytes":groups,
            "shared_datagram_header_bytes":self.datagram_header_bytes,
            "downstream_bytes":self.datagram_bytes, "downstream_datagrams":self.datagrams,
            "source_capture_to_encode_ms":distribution(&self.capture_delay_ms),
            "encode_to_checkpoint_ms":distribution(&self.checkpoint_delivery_ms),
            "first_manifest_to_checkpoint_ms":distribution(&self.manifest_to_checkpoint_ms),
            "checkpoint_completion_trigger_counts":self.completion_trigger,
            "summed_pending_checkpoint_missing_dependency_ms":self.missing_dependency_ms,
            "checkpoint_events":self.checkpoint_events,
            "transfer_events":self.transfers,

        })
    }
}
fn group(class: usize) -> &'static str {
    match class {
        CONTROL_CLASS => "reliable_control_and_baselines",
        presentation::CHASSIS => "chassis_poses",
        presentation::PROJECTILES => "projectile_poses",
        10..=REPAIR_MANIFEST => "checkpoint_repair",
        _ => "checkpoint_normal",
    }
}
fn class_name(class: usize) -> String {
    match class {
        0 => "manifest".into(),
        REPAIR_MANIFEST => "repair_manifest".into(),
        CONTROL_CLASS => "ordered_controls_and_baselines".into(),
        presentation::CHASSIS => "chassis_poses".into(),
        presentation::PROJECTILES => "projectile_poses".into(),
        _ => format!(
            "{}_{:?}",
            if class <= TOPICS { "normal" } else { "repair" },
            Topic::ALL[(class - 1) % TOPICS]
        ),
    }
}
fn distribution(values: &[f64]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    serde_json::json!({"samples": sorted.len(), "p50":sorted[sorted.len()/2],
        "p95":sorted[sorted.len()*95/100], "p99":sorted[sorted.len()*99/100],
        "max":sorted.last().unwrap(), "mean":sorted.iter().sum::<f64>()/sorted.len() as f64})
}
