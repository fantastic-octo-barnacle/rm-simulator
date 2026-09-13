// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Nonblocking timestamps in one render submission, with contiguous stage intervals.
//! Frame and stage samples share the same timestamp query readback.
use crate::config::{Args, Detail};
use bevy::{
    core_pipeline::{Core3dSystems, schedule::Core3d},
    prelude::*,
    render::{
        RenderApp,
        renderer::{
            PendingCommandBuffers, RenderContext, RenderDevice, RenderGraph, RenderGraphSystems,
            RenderQueue,
        },
    },
};
use serde::Serialize;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Instant,
};
const MARKERS: usize = 5;
// Offscreen rendering can queue far more frames than a presented window.
// Keep query storage bounded while allowing delayed nonblocking readbacks.
const READBACK_SLOTS: usize = 256;
/// Names of the four contiguous render-stage intervals, in submission order.
///
/// `pre_main_including_shadows` opens before the main pass, `main_pass` covers it,
/// `early_post_process` follows, and `post_process_and_upscale` closes the submission.
/// Each name labels one interval between two timestamps, not an individual draw call.
pub const STAGE_NAMES: [&str; 4] = [
    "pre_main_including_shadows",
    "main_pass",
    "early_post_process",
    "post_process_and_upscale",
];
/// One contiguous render-stage interval of a measured frame.
#[derive(Clone, Debug, Serialize)]
pub struct Stage {
    /// Interval name, one of `STAGE_NAMES`.
    pub name: &'static str,
    /// Render-thread wall time in milliseconds, including encoding and scheduling.
    pub cpu_ms: f64,
    /// Device timestamp interval in milliseconds; `None` when timestamps are off or out of order.
    pub gpu_ms: Option<f64>,
}
/// One measured frame: its whole-submission total and the stage intervals inside it.
#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    /// Render frame counter; the first frame of the process is 1.
    pub frame: u64,
    /// GPU timestamp taken before the first render commands, in raw query ticks.
    pub start_tick: Option<u64>,
    /// GPU timestamp taken after the last stage, in raw query ticks.
    pub end_tick: Option<u64>,
    /// Whole-submission GPU time in milliseconds; `None` when timestamps are missing, equal or reversed.
    pub elapsed_ms: Option<f64>,
    /// Stage intervals with both ends marked; empty at `Detail::Summary`.
    pub stages: Vec<Stage>,
}
/// Counters and samples written by the render graph and read by the report state machine.
#[derive(Default)]
pub struct Shared {
    /// Set while the sampling window is open; frames outside it are not recorded.
    pub measuring: AtomicBool,
    /// Readbacks submitted and not yet mapped; the drain phase waits for this to reach zero.
    pub pending: AtomicUsize,
    /// Measured frames discarded because no readback slot was free or the sample buffer was full.
    pub dropped: AtomicUsize,
    /// Measured frames with no usable whole-frame timestamp, including failed readbacks.
    pub invalid: AtomicUsize,
    /// Recorded frames in completion order, capped at one million.
    pub samples: Mutex<Vec<Sample>>,
}
/// Timing state shared between the app and the render sub-app.
#[derive(Resource, Clone, Default)]
pub struct Capture(
    /// Counters and samples that the render graph systems fill in.
    pub Arc<Shared>,
);
struct Slot {
    query: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    busy: Arc<AtomicBool>,
}
struct Frame {
    slot: Option<usize>,
    id: u64,
    measured: bool,
    cpu: [Option<Instant>; MARKERS],
}
#[derive(Resource)]
struct Timer {
    slots: Vec<Slot>,
    current: Option<Frame>,
    frame: u64,
    enabled: bool,
}
/// Brackets render commands with five GPU timestamps and reads them back without blocking.
///
/// One timestamp query set is allocated per measured frame in a fixed ring of 256 slots, so an
/// offscreen run cannot queue unbounded readbacks. A measured frame that finds every slot busy is
/// dropped and fails the run. At `Detail::Summary` the Core3d stage markers are not installed, so
/// only whole-frame times are recorded.
pub struct GpuTimerPlugin;
impl Plugin for GpuTimerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Capture>();
    }
    fn finish(&self, app: &mut App) {
        let capture = app.world().resource::<Capture>().clone();
        let args = app.world().resource::<Args>().clone();
        let Some(render) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        let device = render.world().resource::<RenderDevice>();
        let required =
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let enabled = !args.cpu_only && device.features().contains(required);
        let slots = (0..if enabled { READBACK_SLOTS } else { 0 })
            .map(|_| Slot {
                query: device
                    .wgpu_device()
                    .create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("bench frame queries"),
                        ty: wgpu::QueryType::Timestamp,
                        count: MARKERS as u32,
                    }),
                resolve: device.wgpu_device().create_buffer(&wgpu::BufferDescriptor {
                    label: Some("bench query resolve"),
                    size: 256 * MARKERS as u64,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                read: device.wgpu_device().create_buffer(&wgpu::BufferDescriptor {
                    label: Some("bench query readback"),
                    size: 8 * MARKERS as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                busy: Arc::default(),
            })
            .collect();
        render
            .insert_resource(capture)
            .insert_resource(Timer {
                slots,
                current: None,
                frame: 0,
                enabled,
            })
            .add_systems(
                RenderGraph,
                (
                    begin.before(RenderGraphSystems::Begin),
                    end.after(RenderGraphSystems::Render)
                        .before(RenderGraphSystems::Submit),
                    readback.after(RenderGraphSystems::Submit),
                ),
            );
        if args.detail != Detail::Summary {
            // The benchmark has exactly one camera. These boundaries partition its
            // ordered Core3d stages; they are not individual draw-call timings.
            render.add_systems(
                Core3d,
                (
                    marker::<1>
                        .in_set(Core3dSystems::MainPass)
                        .before(bevy::core_pipeline::core_3d::main_opaque_pass_3d),
                    marker::<2>
                        .after(Core3dSystems::MainPass)
                        .before(Core3dSystems::EarlyPostProcess),
                    marker::<3>
                        .after(Core3dSystems::EarlyPostProcess)
                        .before(Core3dSystems::PostProcess),
                ),
            );
        }
    }
}
fn begin(
    mut timer: ResMut<Timer>,
    capture: Res<Capture>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let _ = device.poll(wgpu::PollType::Poll);
    timer.frame += 1;
    let measured = capture.0.measuring.load(Ordering::Acquire);
    let slot = if timer.enabled {
        let Some(index) = timer
            .slots
            .iter()
            .position(|s| !s.busy.swap(true, Ordering::AcqRel))
        else {
            if measured {
                capture.0.dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        };
        if measured {
            capture.0.pending.fetch_add(1, Ordering::AcqRel);
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench frame start"),
        });
        encoder.write_timestamp(&timer.slots[index].query, 0);
        pending.push([encoder.finish()]);
        Some(index)
    } else {
        None
    };
    let mut cpu = [None; MARKERS];
    cpu[0] = Some(Instant::now());
    timer.current = Some(Frame {
        slot,
        id: timer.frame,
        measured,
        cpu,
    });
}
fn marker<const INDEX: usize>(mut timer: ResMut<Timer>, mut context: RenderContext) {
    let Some(frame) = timer.current.as_mut() else {
        return;
    };
    frame.cpu[INDEX] = Some(Instant::now());
    if let Some(index) = frame.slot {
        context
            .command_encoder()
            .write_timestamp(&timer.slots[index].query, INDEX as u32);
    }
}
fn end(
    mut timer: ResMut<Timer>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let Some(frame) = timer.current.as_mut() else {
        return;
    };
    frame.cpu[MARKERS - 1] = Some(Instant::now());
    let Some(index) = frame.slot else {
        return;
    };
    let written = frame.cpu.map(|t| t.is_some());
    // Finish pending encoders before appending the end marker. Otherwise their
    // commands would be appended after it by PendingCommandBuffers::take.
    let buffers = pending.take();
    pending.push(buffers);
    let slot = &timer.slots[index];
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench frame end"),
    });
    encoder.write_timestamp(&slot.query, (MARKERS - 1) as u32);
    for (i, written) in written.iter().enumerate() {
        if *written {
            encoder.resolve_query_set(
                &slot.query,
                i as u32..i as u32 + 1,
                &slot.resolve,
                i as u64 * 256,
            );
            encoder.copy_buffer_to_buffer(
                &slot.resolve,
                i as u64 * 256,
                &slot.read,
                i as u64 * 8,
                8,
            );
        }
    }
    pending.push([encoder.finish()]);
}
fn readback(mut timer: ResMut<Timer>, capture: Res<Capture>, queue: Res<RenderQueue>) {
    let Some(frame) = timer.current.take() else {
        return;
    };
    let Some(index) = frame.slot else {
        if frame.measured {
            push(&capture.0, sample(&frame, None, 0.));
        }
        return;
    };
    let slot = &timer.slots[index];
    let read = slot.read.clone();
    let busy = slot.busy.clone();
    let shared = capture.0.clone();
    let period = f64::from(queue.get_timestamp_period());
    slot.read
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            if result.is_ok() {
                let bytes = read.slice(..).get_mapped_range();
                let ticks = std::array::from_fn(|i| {
                    u64::from_le_bytes(bytes[i * 8..i * 8 + 8].try_into().expect("eight bytes"))
                });
                if frame.measured {
                    let value = sample(&frame, Some(ticks), period);
                    if value.elapsed_ms.is_none() {
                        shared.invalid.fetch_add(1, Ordering::Relaxed);
                    }
                    push(&shared, value);
                }
                drop(bytes);
                read.unmap();
            } else if frame.measured {
                shared.invalid.fetch_add(1, Ordering::Relaxed);
            }
            if frame.measured {
                shared.pending.fetch_sub(1, Ordering::AcqRel);
            }
            busy.store(false, Ordering::Release);
        });
}
fn push(shared: &Shared, sample: Sample) {
    let mut samples = shared.samples.lock().expect("sample lock");
    if samples.len() < 1_000_000 {
        samples.push(sample);
    } else {
        shared.dropped.fetch_add(1, Ordering::Relaxed);
    }
}
fn sample(frame: &Frame, ticks: Option<[u64; MARKERS]>, period: f64) -> Sample {
    let elapsed_ms = ticks
        .and_then(|t| duration_ms(t[0], t[MARKERS - 1], period))
        .filter(|ms| *ms > 0.);
    let stages = STAGE_NAMES
        .iter()
        .enumerate()
        .filter_map(|(i, name)| {
            let (start, end) = (frame.cpu[i]?, frame.cpu[i + 1]?);
            Some(Stage {
                name,
                cpu_ms: end.duration_since(start).as_secs_f64() * 1000.,
                gpu_ms: ticks.and_then(|t| duration_ms(t[i], t[i + 1], period)),
            })
        })
        .collect();
    Sample {
        frame: frame.id,
        start_tick: ticks.map(|t| t[0]),
        end_tick: ticks.map(|t| t[MARKERS - 1]),
        elapsed_ms,
        stages,
    }
}
fn duration_ms(start: u64, end: u64, period: f64) -> Option<f64> {
    let ms = end.checked_sub(start)? as f64 * period / 1e6;
    (ms.is_finite() && ms >= 0. && period > 0.).then_some(ms)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamp_units_and_invalid_results() {
        assert_eq!(duration_ms(100, 1000100, 2.), Some(2.));
        assert_eq!(duration_ms(1, 0, 1.), None);
        assert_eq!(duration_ms(1, 1, 1.), Some(0.));
        assert_eq!(duration_ms(0, 1, f64::NAN), None);
    }
    #[test]
    fn stage_intervals_partition_the_whole_gpu_frame() {
        let start = Instant::now();
        let frame = Frame {
            slot: Some(0),
            id: 1,
            measured: true,
            cpu: std::array::from_fn(|i| Some(start + std::time::Duration::from_millis(i as u64))),
        };
        let s = sample(&frame, Some([100, 200, 500, 600, 1100]), 1000.);
        assert_eq!(s.elapsed_ms, Some(1.));
        assert_eq!(s.stages.iter().map(|s| s.gpu_ms.unwrap()).sum::<f64>(), 1.);
        assert!(sample(&frame, None, 0.).elapsed_ms.is_none());
    }
}
