// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Bounded transport metadata tracing. Never records payloads, names or passwords.
use crate::{
    clock::TimeSource,
    protocol::{ClientMessage, Command, ServerMessage},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const CAPACITY: usize = 2048;
const FILE_LIMIT: u64 = 64 * 1024 * 1024;
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

/// Counts for one stage and message class. Bytes exclude UDP/IP and GNS overhead.
#[derive(Clone, Default, Serialize)]
pub struct Counts {
    /// Events observed, including events dropped by the trace writer.
    pub events: u64,
    /// Application bytes observed, absent for typed messages with no wire encoding.
    pub bytes: Option<u64>,
    /// Total measured work in nanoseconds, absent for non-work events.
    pub work_ns: Option<u64>,
    /// Longest measured operation in nanoseconds.
    pub max_work_ns: Option<u64>,
}
/// Current trace status and cumulative counters, safe to expose through the console.
#[derive(Clone, Default, Serialize)]
pub struct Report {
    /// Schema version of this report and the JSONL trace.
    pub schema_version: u32,
    /// Counters keyed first by stage, then message class.
    pub stages: BTreeMap<&'static str, BTreeMap<&'static str, Counts>>,
    /// Trace path when recording was requested and the file was opened.
    pub path: Option<PathBuf>,
    /// Events dropped because the writer queue was full or recording stopped.
    pub dropped: u64,
    /// Observations skipped to avoid waiting on a contended diagnostics lock.
    pub contended: u64,
    /// Recording stopped at the per-file size limit.
    pub capped: bool,
    /// File creation or writer failure. Gameplay continues with counters only.
    pub error: Option<String>,
}
/// One metadata observation. Optional identities allow correlation without payloads.
#[derive(Clone, Default, Serialize)]
pub(crate) struct Event {
    pub stage: &'static str,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shooter: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectile: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulation_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intended_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_bytes: Option<[usize; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_age_ms: Option<[f64; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<crate::network_stats::EncodingStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_ns: Option<u64>,
}
#[derive(Serialize)]
struct Row {
    elapsed_ns: u64,
    #[serde(flatten)]
    event: Event,
}
#[derive(Default)]
struct Health {
    dropped: AtomicU64,
    contended: AtomicU64,
    capped: AtomicBool,
    error: Mutex<Option<String>>,
}
/// Local counters plus an optional nonblocking disk writer. No socket or simulation access.
pub(crate) struct Observer {
    time: TimeSource,
    started: Instant,
    report: Mutex<Report>,
    sender: Option<mpsc::SyncSender<Row>>,
    health: Arc<Health>,
}
impl Observer {
    pub(crate) fn new(label: &'static str, time: TimeSource) -> Self {
        let directory = std::env::var_os("RM_NET_TRACE_DIR").map(PathBuf::from);
        Self::open(label, time, directory.as_deref(), FILE_LIMIT)
    }
    fn open(label: &'static str, time: TimeSource, directory: Option<&Path>, limit: u64) -> Self {
        let mut observer = Self {
            started: time.now(),
            time,
            report: Mutex::new(Report {
                schema_version: 1,
                ..Default::default()
            }),
            sender: None,
            health: Arc::default(),
        };
        if let Some(directory) = directory {
            let result = (|| -> io::Result<_> {
                std::fs::create_dir_all(directory)?;
                // Never truncate an earlier recording, including after PID reuse.
                let (path, file) = loop {
                    let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
                    let path = directory.join(format!("{}-{label}-{id}.jsonl", std::process::id()));
                    match OpenOptions::new().write(true).create_new(true).open(&path) {
                        Ok(file) => break (path, file),
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(e) => return Err(e),
                    }
                };
                let (sender, receiver) = mpsc::sync_channel(CAPACITY);
                let health = observer.health.clone();
                std::thread::Builder::new()
                    .name("rm-network-trace".into())
                    .spawn(move || {
                        if let Err(error) = write_trace(file, receiver, &health, label, limit) {
                            *health.error.lock().unwrap_or_else(|p| p.into_inner()) =
                                Some(error.to_string());
                        }
                    })?;
                Ok((path, sender))
            })();
            match result {
                Ok((path, sender)) => {
                    observer.report.get_mut().unwrap().path = Some(path);
                    observer.sender = Some(sender);
                }
                Err(error) => {
                    eprintln!("network trace disabled: {error}");
                    *observer.health.error.lock().unwrap() = Some(error.to_string());
                }
            }
        }
        observer
    }
    pub(crate) fn record(&self, event: Event) {
        {
            let Ok(mut report) = self.report.try_lock() else {
                self.health.contended.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let count = report
                .stages
                .entry(event.stage)
                .or_default()
                .entry(event.kind)
                .or_default();
            count.events += 1;
            if let Some(bytes) = event.bytes {
                *count.bytes.get_or_insert(0) += bytes as u64;
            }
            if let Some(ns) = event.work_ns {
                *count.work_ns.get_or_insert(0) += ns;
                count.max_work_ns = Some(count.max_work_ns.unwrap_or(0).max(ns));
            }
        }
        if let Some(sender) = &self.sender {
            if self.health.capped.load(Ordering::Relaxed) {
                self.health.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let row = Row {
                elapsed_ns: self
                    .time
                    .since(self.started)
                    .as_nanos()
                    .min(u64::MAX as u128) as u64,
                event,
            };
            if sender.try_send(row).is_err() {
                self.health.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    /// Frame-side reads never wait for a worker holding the counters lock.
    pub(crate) fn report(&self) -> Option<Report> {
        let mut report = self.report.try_lock().ok()?.clone();
        report.dropped = self.health.dropped.load(Ordering::Relaxed);
        report.contended = self.health.contended.load(Ordering::Relaxed);
        report.capped = self.health.capped.load(Ordering::Relaxed);
        report.error = self.health.error.try_lock().ok()?.clone();
        Some(report)
    }
    pub(crate) fn packet(&self, stage: &'static str, peer: Option<u32>, bytes: &[u8]) {
        let kind = match bytes.get(..4) {
            Some(b"RMO3") => "owner",
            Some(b"RMI2") => "inputs",
            Some(b"RMC1") => "shot",
            Some(b"RMA1") => "baseline_ack",
            Some(b"RMG1") if bytes.get(4) == Some(&0) => "world_fragment",
            Some(b"RMG1") => "control_fragment",
            _ => "control",
        };
        let frame = if bytes.starts_with(b"RMG1") {
            bytes
                .get(5..13)
                .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        } else {
            None
        };
        self.record(Event {
            stage,
            kind,
            peer,
            frame,
            bytes: Some(bytes.len()),
            ..Default::default()
        });
    }
    pub(crate) fn work(&self, kind: &'static str, elapsed: Duration) {
        self.record(Event {
            stage: "work",
            kind,
            work_ns: Some(elapsed.as_nanos().min(u64::MAX as u128) as u64),
            ..Default::default()
        });
    }
    pub(crate) fn client(
        &self,
        stage: &'static str,
        peer: Option<u32>,
        message: &ClientMessage,
        tick_ns: Option<u64>,
    ) {
        let mut event = Event {
            stage,
            peer,
            simulation_ns: tick_ns,
            ..Default::default()
        };
        match message {
            ClientMessage::Command(Command::PilotInput { chassis, frame }) => {
                event.shooter = Some(*chassis);
                event.kind = "input";
                event.input = Some(frame.sequence);
                event.epoch = Some(frame.input_epoch);
                event.intended_ns = Some(frame.sampled_time_ns);
            }
            ClientMessage::Command(Command::FireAimed {
                shooter,
                shot_id,
                input,
                ..
            }) => {
                event.shooter = Some(*shooter);
                event.kind = "shot";
                event.shot = Some(*shot_id);
                event.input = Some(input.sequence);
                event.epoch = Some(input.input_epoch);
                event.intended_ns = Some(input.sampled_time_ns);
            }
            ClientMessage::Command(_) => event.kind = "command",
            ClientMessage::Hello { .. } => event.kind = "hello",
            ClientMessage::Ping { .. } => event.kind = "barrier",
            ClientMessage::TimeProbe { .. } => event.kind = "time_probe",
        }
        self.record(event);
    }
    pub(crate) fn server(&self, stage: &'static str, message: &ServerMessage) {
        let mut event = Event {
            stage,
            ..Default::default()
        };
        match message {
            ServerMessage::Snapshot(state) => {
                event.kind = "snapshot";
                event.snapshot = Some(state.snapshot_id);
                event.epoch = Some(state.input_epoch);
                event.simulation_ns = Some(state.field.time_ns);
            }
            ServerMessage::ShotResult(result) => {
                event.shooter = Some(result.shooter);
                event.projectile = result.result.as_ref().ok().copied();
                event.kind = if result.result.is_ok() {
                    "shot_result"
                } else {
                    "shot_rejected"
                };
                event.shot = Some(result.shot_id);
                event.simulation_ns = result.executed_time_ns;
            }
            ServerMessage::ShotScheduled {
                shot_id,
                intended_time_ns,
                ..
            } => {
                event.kind = "shot_scheduled";
                event.shot = Some(*shot_id);
                event.simulation_ns = Some(*intended_time_ns);
            }
            ServerMessage::ShotFinished { shot_id, .. } => {
                event.kind = "shot_finished";
                event.shot = Some(*shot_id);
            }
            ServerMessage::Hit {
                epoch,
                event_id,
                hit,
            } => {
                event.event_id = Some(*event_id);
                event.shooter = hit.shooter;
                event.projectile = Some(hit.projectile);
                event.kind = "hit";
                event.epoch = Some(*epoch);
                event.simulation_ns = Some(hit.time_ns);
            }
            ServerMessage::Welcome(welcome) => {
                event.kind = "welcome";
                event.peer = Some(welcome.client_id);
            }
            ServerMessage::OwnerConfig(_) => event.kind = "owner_config",
            ServerMessage::Roster(_) => event.kind = "roster",
            ServerMessage::Rejected { .. } => event.kind = "rejected",
            ServerMessage::Notice(_) => event.kind = "notice",
            ServerMessage::Pong { .. } => event.kind = "barrier",
            ServerMessage::TimeSample { time_ns, .. } => {
                event.kind = "time_sample";
                event.simulation_ns = Some(*time_ns);
            }
            ServerMessage::DeliveryStats(stats) => {
                event.kind = "delivery_stats";
                event.queue_bytes = Some(stats.queued_bytes);
                event.queue_age_ms = Some(stats.oldest_age_ms);
                event.encoding = stats.encoding.clone();
            }
            ServerMessage::Telemetry(_) => event.kind = "telemetry",
        }
        self.record(event);
    }
}
fn write_trace(
    file: File,
    receiver: mpsc::Receiver<Row>,
    health: &Health,
    label: &str,
    limit: u64,
) -> io::Result<()> {
    let mut writer = BufWriter::new(file);
    let header = serde_json::to_vec(
        &serde_json::json!({"schema_version":1,"type":"header","role":label,"pid":std::process::id(),"protocol":crate::protocol::PROTOCOL_VERSION,"byte_scope":"application payload; excludes IP/UDP/GNS overhead","clock":"elapsed_ns is observer-local; do not subtract across files"}),
    )?;
    writer.write_all(&header)?;
    writer.write_all(b"\n")?;
    let mut written = header.len() as u64 + 1;
    let mut since_flush = 0;
    loop {
        match receiver.recv_timeout(Duration::from_millis(250)) {
            Ok(row) => {
                if health.capped.load(Ordering::Relaxed) {
                    health.dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let bytes = serde_json::to_vec(&row)?;
                // Leave room for the terminal status record.
                if written + bytes.len() as u64 + 1 + 256 > limit {
                    health.capped.store(true, Ordering::Relaxed);
                    health.dropped.fetch_add(1, Ordering::Relaxed);
                    writer.flush()?;
                    continue;
                }
                writer.write_all(&bytes)?;
                writer.write_all(b"\n")?;
                written += bytes.len() as u64 + 1;
                since_flush += 1;
                if since_flush >= 128 {
                    writer.flush()?;
                    since_flush = 0;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                writer.flush()?;
                since_flush = 0;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    writeln!(
        writer,
        "{}",
        serde_json::json!({"type":"end","capped":health.capped.load(Ordering::Relaxed),"dropped_at_close":health.dropped.load(Ordering::Relaxed)})
    )?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counters_do_not_invent_wire_bytes_and_busy_reads_never_wait() {
        let time = crate::clock::ManualTime::new();
        let observer = Observer::open("test", time.source(), None, FILE_LIMIT);
        observer.client(
            "enqueue_attempt",
            Some(1),
            &ClientMessage::Hello {
                protocol: 27,
                password: "do-not-log-this".into(),
                name: "private-name".into(),
                team: None,
                role: crate::protocol::Role::Pilot,
            },
            None,
        );
        observer.packet("receive", None, b"RMI2data");
        let report = observer.report().unwrap();
        assert_eq!(report.stages["enqueue_attempt"]["hello"].bytes, None);
        assert_eq!(report.stages["receive"]["inputs"].bytes, Some(8));
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("do-not-log-this") && !json.contains("private-name"));
        let lock = observer.report.lock().unwrap();
        assert!(observer.report().is_none());
        observer.packet("receive", None, b"RMI2data");
        drop(lock);
        assert_eq!(observer.report().unwrap().contended, 1);
    }
    #[test]
    fn writer_caps_file_and_records_truncation_without_backpressure() {
        let path = std::env::temp_dir().join(format!(
            "rm-trace-test-{}-{}.jsonl",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let health = Health::default();
        let (sender, receiver) = mpsc::sync_channel(CAPACITY);
        for n in 0..100 {
            sender
                .try_send(Row {
                    elapsed_ns: n,
                    event: Event {
                        stage: "receive",
                        kind: "input",
                        ..Default::default()
                    },
                })
                .unwrap();
        }
        drop(sender);
        write_trace(file, receiver, &health, "test", 1024).unwrap();
        assert!(health.capped.load(Ordering::Relaxed));
        assert!(health.dropped.load(Ordering::Relaxed) > 0);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.len() <= 1024);
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows[0]["schema_version"], 1);
        assert_eq!(rows.last().unwrap()["type"], "end");
        assert_eq!(rows.last().unwrap()["capped"], true);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn saturated_writer_counts_drops_and_keeps_counters() {
        let mut observer = Observer::open("test", TimeSource::system(), None, FILE_LIMIT);
        let (sender, _receiver) = mpsc::sync_channel(1);
        observer.sender = Some(sender);
        observer.packet("receive", None, b"RMI2one");
        observer.packet("receive", None, b"RMI2two");
        let report = observer.report().unwrap();
        assert_eq!(report.dropped, 1);
        assert_eq!(report.stages["receive"]["inputs"].events, 2);
    }
}
