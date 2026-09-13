// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 hxyulin <hxyulin@proton.me>
//! Run state machine, capability checks and report writing.
//! A run that loses a sample or a GPU timestamp fails instead of reporting a partial capture.
use crate::{
    assets::Loaded,
    config::{Args, Case, Detail},
    gpu::Capture,
    scene::{GeometryState, Target},
};
use bevy::{
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        render_resource::PipelineCache,
        renderer::{RenderAdapter, RenderAdapterInfo, RenderDevice, RenderQueue},
    },
};
use serde_json::{Value, json};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Resource, Clone)]
struct Ready(Arc<AtomicUsize>);
#[derive(Resource)]
struct State {
    started: Instant,
    warm: Option<Instant>,
    sample: Option<Instant>,
    drain: Option<Instant>,
    previous: Option<Instant>,
    cpu: Vec<f64>,
    done: bool,
    checked: bool,
    screenshot_started: Option<Instant>,
    actual_resolution: Option<[u32; 2]>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            warm: None,
            sample: None,
            drain: None,
            previous: None,
            cpu: Vec::new(),
            done: false,
            checked: false,
            screenshot_started: None,
            actual_resolution: None,
        }
    }
}
/// Checks device capabilities, times the run, and writes the report.
///
/// Warmup starts after the CAD scene and its pipelines are ready, sampling lasts
/// `sample_seconds`, and readbacks drain for up to five seconds. A constraint that cannot be
/// met, such as unavailable MSAA or occlusion culling, ends the run before sampling starts.
pub struct ReportPlugin;
impl Plugin for ReportPlugin {
    fn build(&self, app: &mut App) {
        let ready = Ready(Arc::new(AtomicUsize::new(usize::MAX)));
        app.insert_resource(ready.clone())
            .init_resource::<State>()
            .add_systems(Last, tick);
        if let Some(render) = app.get_sub_app_mut(RenderApp) {
            render
                .insert_resource(ready)
                .add_systems(Render, pipelines.in_set(RenderSystems::Cleanup));
        }
    }
}
fn pipelines(cache: Res<PipelineCache>, ready: Res<Ready>) {
    ready
        .0
        .store(cache.waiting_pipelines().count(), Ordering::Release);
}
fn capability_error(world: &mut World) -> Option<String> {
    let args = world.resource::<Args>();
    let device = world.resource::<RenderDevice>();
    let adapter = world.resource::<RenderAdapter>();
    let s = world.resource::<Case>().graphics.resolved();
    if !args.cpu_only
        && !device.features().contains(
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS,
        )
    {
        return Some("adapter lacks encoder timestamp queries; choose a supported backend or explicitly use --cpu-only".into());
    }
    for format in [
        wgpu::TextureFormat::Rgba16Float,
        wgpu::TextureFormat::Depth32Float,
    ] {
        let features = if device
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
        {
            adapter.get_texture_format_features(format)
        } else {
            format.guaranteed_format_features(device.features())
        };
        if !features.flags.sample_count_supported(s.msaa_samples) {
            return Some(format!(
                "requested {}x MSAA unavailable for {format:?}",
                s.msaa_samples
            ));
        }
    }
    if s.shadow_map_size > device.limits().max_texture_dimension_2d as usize {
        return Some("shadow map exceeds device limit".into());
    }
    if s.occlusion_culling
        && !bevy::render::batching::gpu_preprocessing::GpuPreprocessingSupport::from_world(world)
            .is_culling_supported()
    {
        return Some("occlusion culling unavailable on this adapter".into());
    }
    None
}
fn tick(world: &mut World) {
    world.resource_scope(|world, mut state: Mut<State>| {
        let now = Instant::now();
        if state.done {
            if state
                .screenshot_started
                .is_some_and(|t| t.elapsed() > Duration::from_secs(10))
            {
                error!("screenshot timed out");
                world.write_message(AppExit::error());
                state.screenshot_started = None;
            }
            return;
        }
        let case = world.resource::<Case>().clone();
        if now.duration_since(state.started).as_secs_f64() > case.timeout_seconds {
            finish(world, &mut state, Some("benchmark timed out".into()));
            return;
        }
        if !state.checked {
            if !world.contains_resource::<RenderDevice>() {
                return;
            }
            if let Some(error) = capability_error(world) {
                finish(world, &mut state, Some(error));
                return;
            }
            state.checked = true;
        }
        let status = world.resource::<rm_simulator_render::cad::CadSceneStatus>();
        if let Some(error) = &status.failed {
            let error = error.clone();
            finish(world, &mut state, Some(error));
            return;
        }
        if state.drain.is_some() {
            let capture = world.resource::<Capture>();
            if capture.0.pending.load(Ordering::Acquire) == 0 {
                finish(world, &mut state, None);
            } else if state.drain.unwrap().elapsed() > Duration::from_secs(5) {
                finish(
                    world,
                    &mut state,
                    Some("GPU readbacks did not drain within five seconds".into()),
                );
            }
            return;
        }
        let ready = status.ready()
            && world.resource::<GeometryState>().ready
            && world.resource::<Ready>().0.load(Ordering::Acquire) == 0;
        let actual_resolution = world.query::<&Camera>().iter(world)
            .find_map(|camera| camera.physical_target_size().map(|size| size.to_array()));
        state.actual_resolution = actual_resolution;
        // Check the measured target after scene pipelines have settled. Rejecting
        // during the first ready frame can tear down an adapter while newly
        // spawned scene pipelines are still being created on worker threads.
        let warmed = state.warm.is_some_and(|t| now.duration_since(t).as_secs_f64() >= case.warmup_seconds);
        if ready && (state.sample.is_some() || warmed) && actual_resolution != Some(case.resolution) {
            finish(world, &mut state, Some(format!(
                "render target is {actual_resolution:?}, requested {:?}; use --headless if the desktop constrains the window",
                case.resolution
            )));
            return;
        }
        if state.sample.is_none() {
            if !ready {
                state.warm = None;
                return;
            }
            let warm = *state.warm.get_or_insert(now);
            if now.duration_since(warm).as_secs_f64() < case.warmup_seconds {
                return;
            }
            info!("sampling {} for {} seconds", case.name, case.sample_seconds);
            state.sample = Some(now);
            state.previous = Some(now);
            world
                .resource::<Capture>()
                .0
                .measuring
                .store(true, Ordering::Release);
            return;
        }
        if !ready {
            finish(
                world,
                &mut state,
                Some("pipelines or scene became unready during sampling".into()),
            );
            return;
        }
        if let Some(previous) = state.previous.replace(now) {
            if state.cpu.len() == 1_000_000 {
                finish(
                    world,
                    &mut state,
                    Some("CPU sample capacity exceeded".into()),
                );
                return;
            }
            state
                .cpu
                .push(now.duration_since(previous).as_secs_f64() * 1000.);
        }
        if now.duration_since(state.sample.unwrap()).as_secs_f64() >= case.sample_seconds {
            world
                .resource::<Capture>()
                .0
                .measuring
                .store(false, Ordering::Release);
            state.drain = Some(now);
        }
    });
}
/// Summary statistics of one millisecond distribution, as a JSON object.
///
/// Percentiles use nearest rank: `p95_ms` is the value at index `ceil(0.95 * n) - 1` of the
/// sorted samples. `rate_from_mean_hz` is 1000 / `mean_ms`, which for GPU timings is a throughput
/// estimate rather than game FPS. `one_percent_low_hz` is 1000 divided by the mean of the slowest
/// 1 percent, at least one sample. An empty distribution yields `{"samples":0}`.
pub fn stats(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({"samples":0});
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let q = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    let slow = sorted.len().div_ceil(100);
    let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
    json!({"samples":sorted.len(),"mean_ms":mean,"median_ms":q(0.5),"p95_ms":q(0.95),"p99_ms":q(0.99),"max_ms":sorted.last(),"rate_from_mean_hz":1000./mean,"one_percent_low_hz":1000./(sorted[sorted.len()-slow..].iter().sum::<f64>()/slow as f64)})
}
fn finish(world: &mut World, state: &mut State, mut error: Option<String>) {
    state.done = true;
    let args = world.resource::<Args>().clone();
    let capture = world.resource::<Capture>().clone();
    capture.0.measuring.store(false, Ordering::Release);
    let samples = capture.0.samples.lock().expect("sample lock");
    if error.is_none() && !args.cpu_only && !samples.iter().any(|s| s.elapsed_ms.is_some()) {
        error = Some("no valid GPU timestamp samples; CPU times are not GPU times".into());
    }
    if error.is_none() && state.cpu.is_empty() {
        error = Some("no CPU frame samples".into());
    }
    let invalid_stages = if args.cpu_only {
        0
    } else {
        samples
            .iter()
            .flat_map(|s| &s.stages)
            .filter(|s| s.gpu_ms.is_none())
            .count()
    };
    if error.is_none()
        && !args.cpu_only
        && (capture.0.invalid.load(Ordering::Acquire) > 0 || invalid_stages > 0)
    {
        error = Some(
            "invalid GPU frame or stage timestamps; this run cannot be used for comparison".into(),
        );
    }
    if error.is_none() && capture.0.dropped.load(Ordering::Acquire) > 0 {
        error = Some("capture dropped samples; this run cannot be used for comparison".into());
    }
    let gpu_ms: Vec<_> = samples.iter().filter_map(|s| s.elapsed_ms).collect();
    let device = world.get_resource::<RenderDevice>();
    let adapter = world.get_resource::<RenderAdapterInfo>();
    let mut effective_graphics = world.resource::<Case>().graphics.resolved();
    effective_graphics.depth_prepass |= effective_graphics.occlusion_culling;
    let mut report = json!({"schema_version":1,"status":if error.is_some(){"failed"}else{"ok"},"error":error,
        "case":world.resource::<Case>(),"resolved_graphics":effective_graphics,"detail":args.detail,"cpu_only":args.cpu_only,
        "actual_resolution":state.actual_resolution,
        "presentation":if args.headless{"offscreen_uncapped"}else{"window"},
        "adapter":adapter.map(|a|json!({"name":a.name,"backend":format!("{:?}",a.backend),"vendor":a.vendor,"device":a.device,"driver":a.driver,"driver_info":a.driver_info})),
        "device_features":device.map(|d|format!("{:?}",d.features())),"timestamp_period_ns":world.get_resource::<RenderQueue>().map(|q|q.get_timestamp_period()),
        "build":{"version":env!("CARGO_PKG_VERSION"),"debug_assertions":cfg!(debug_assertions),"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "asset_root":args.cad_assets.canonicalize().ok(),"asset_sha256":world.resource::<Loaded>().hashes,"geometry":world.resource::<GeometryState>().report,
        "cpu_frame":stats(&state.cpu),"gpu_render":stats(&gpu_ms),"gpu_readbacks":{"dropped":capture.0.dropped.load(Ordering::Acquire),"invalid":capture.0.invalid.load(Ordering::Acquire),"pending":capture.0.pending.load(Ordering::Acquire),"invalid_stages":invalid_stages},
        "measurement":{"cpu":"Last-to-Last wall time, includes scheduling and presentation; no simulation runs", "gpu":"encoder timestamps bracket render-graph command buffers in one queue submission; excludes presentation, CPU work and the final query resolve/copy", "scene":"frozen visual CAD and optional stadium; no robots, projectiles, physics, networking or gameplay UI", "stages":"contiguous render-stage intervals, not individual draw calls; compare runs with the same detail level"}});
    if args.detail != Detail::Summary {
        report["stages"] = json!(
            crate::gpu::STAGE_NAMES
                .iter()
                .map(|name| {
                    let cpu: Vec<_> = samples
                        .iter()
                        .flat_map(|s| &s.stages)
                        .filter(|s| &s.name == name)
                        .map(|s| s.cpu_ms)
                        .collect();
                    let gpu: Vec<_> = samples
                        .iter()
                        .flat_map(|s| &s.stages)
                        .filter(|s| &s.name == name)
                        .filter_map(|s| s.gpu_ms)
                        .collect();
                    (*name, json!({"cpu":stats(&cpu), "gpu":stats(&gpu)}))
                })
                .collect::<std::collections::BTreeMap<_, _>>()
        );
    }
    let save = || -> anyhow::Result<()> {
        std::fs::write(
            args.output.join("report.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        if args.detail == Detail::Raw {
            let mut gpu = std::io::BufWriter::new(std::fs::File::create(
                args.output.join("render-frames.jsonl"),
            )?);
            for sample in samples.iter() {
                serde_json::to_writer(&mut gpu, sample)?;
                writeln!(gpu)?;
            }
            gpu.flush()?;
            let mut cpu =
                std::io::BufWriter::new(std::fs::File::create(args.output.join("cpu-frames.csv"))?);
            writeln!(cpu, "sample,elapsed_ms")?;
            for (index, ms) in state.cpu.iter().enumerate() {
                writeln!(cpu, "{index},{ms}")?;
            }
            cpu.flush()?;
        }
        Ok(())
    };
    if let Err(failure) = save() {
        error = Some(format!("writing output: {failure:#}"));
    }
    info!(
        "CPU: {}; GPU: {}; output: {}",
        report["cpu_frame"],
        report["gpu_render"],
        args.output.display()
    );
    if let Some(error) = error {
        error!("{error}");
        world.write_message(AppExit::error());
        return;
    }
    if !args.screenshot {
        world.write_message(AppExit::Success);
        return;
    }
    use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
    let screenshot = world
        .resource::<Target>()
        .0
        .clone()
        .map_or_else(Screenshot::primary_window, Screenshot::image);
    state.screenshot_started = Some(Instant::now());
    let path = args.output.join("scene.png");
    world.spawn(screenshot).observe(
        move |event: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| match event
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|e| e.to_string())
            .and_then(|image| image.save(&path).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                exit.write(AppExit::Success);
            }
            Err(error) => {
                error!("saving screenshot: {error}");
                exit.write(AppExit::error());
            }
        },
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tails_use_nearest_rank_and_slowest_one_percent_mean() {
        let values: Vec<_> = (1..=100).map(f64::from).collect();
        let s = stats(&values);
        assert_eq!(s["p99_ms"], 99.);
        assert_eq!(s["one_percent_low_hz"], 10.);
        assert_eq!(stats(&[])["samples"], 0);
    }
}
